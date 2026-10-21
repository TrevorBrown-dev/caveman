//! The DLL's worker thread: checks the exe, installs the hook, finds the assets, follows the skater, runs the rules
//! every tick (8 ms), and serves the trainer through the pipe thread. Any mismatch leaves it dormant (the pipe still
//! answers, so the trainer can say why). `unload` puts the vtable slot back, stops both threads and frees the DLL.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;
use serde_json::{json, Value};

use crate::game::{self, layout, rva};
use crate::hook;
use crate::pipe;
use crate::proto::{self, Request};
use crate::rules::{self, Engine, FxNames, Kind};
use crate::scan::{self, Hit, ScanSpec, Scanner};
use crate::skater::{self, Record, Runtime};
use crate::win;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Caveman's own fx state assets: `Animation/Caveman/FX.<Name>` (lower case here).
pub const OUR_PREFIX: &str = "animation/caveman/fx.";
const TICK_MS: u32 = 8;
/// Bytes the scanner reads per tick (a few milliseconds of work).
const SCAN_BUDGET: usize = 32 << 20;
/// And at most this long per tick.
const SCAN_STEP_MS: u64 = 6;
const VALIDATE_MS: u64 = 500;
const WORLDS_MS: u64 = 1000;
const FIRST_BACKOFF_MS: u64 = 5_000;
const MAX_BACKOFF_MS: u64 = 60_000;
const LOG_LINES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Starting,
    Dormant,
    Active,
    Stopping,
}

pub static STOP: AtomicBool = AtomicBool::new(false);
/// The phase, readable without the lock: 0 starting, 1 dormant, 2 active, 3 stopping.
pub static PHASE: AtomicU8 = AtomicU8::new(0);
pub static MODULE: AtomicUsize = AtomicUsize::new(0);
static PIPE_THREAD: AtomicUsize = AtomicUsize::new(0);

/// What the worker shows the trainer (rebuilt every tick).
#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub exe_base: String,
    pub scan: ScanInfo,
    pub skater: Option<Record>,
    pub records: Vec<Record>,
    pub worlds: usize,
    pub inputs: Vec<InputInfo>,
    pub fx_values: Vec<bool>,
    pub fx_assets: Vec<usize>,
    pub ticks: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScanInfo {
    pub running: bool,
    pub scans: u32,
    pub last_ms: u64,
    pub last_mb: u64,
    pub regions: u32,
    pub candidates: u64,
    pub ours: usize,
    pub states: usize,
    pub next_in_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InputInfo {
    pub name: String,
    pub short: Option<&'static str>,
    pub kind: Option<Kind>,
    pub assets: usize,
    pub value: Option<f64>,
    pub live: bool,
}

pub struct Shared {
    pub phase: Phase,
    pub reason: String,
    pub engine: Engine,
    pub fx: FxNames,
    pub rules_changed: bool,
    pub rescan: bool,
    pub snapshot: Snapshot,
    pub log: VecDeque<String>,
    pub pipe: String,
    pub hooked: bool,
}

pub fn shared() -> &'static Mutex<Shared> {
    static SHARED: OnceLock<Mutex<Shared>> = OnceLock::new();
    SHARED.get_or_init(|| {
        let mut fx = FxNames::default();
        let engine = Engine::compile(&rules::default_rules(), &mut fx).expect("the default rules compile");
        Mutex::new(Shared {
            phase: Phase::Starting,
            reason: "starting".into(),
            engine,
            fx,
            rules_changed: true,
            rescan: false,
            snapshot: Snapshot::default(),
            log: VecDeque::new(),
            pipe: String::new(),
            hooked: false,
        })
    })
}

pub fn lock() -> MutexGuard<'static, Shared> {
    shared().lock().unwrap_or_else(|e| e.into_inner())
}

fn set_phase(s: &mut Shared, phase: Phase, reason: &str) {
    s.phase = phase;
    s.reason = reason.to_string();
    PHASE.store(phase as u8, Ordering::Release);
}

/// `%LOCALAPPDATA%\Caveman\fx`.
pub fn data_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| std::path::PathBuf::from(d).join("Caveman").join("fx"))
}

/// A line in the status log and in `%LOCALAPPDATA%\Caveman\fx\caveman-fx.log`.
pub fn log(line: &str) {
    let stamp = {
        let mut t = win::SYSTEMTIME::default();
        unsafe { win::GetLocalTime(&mut t) };
        format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    };
    let text = format!("{stamp} {line}");
    {
        let mut s = lock();
        s.log.push_back(text.clone());
        while s.log.len() > LOG_LINES {
            s.log.pop_front();
        }
    }
    if let Some(dir) = data_dir() {
        use std::io::Write;
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("caveman-fx.log");
        if std::fs::metadata(&path).map(|m| m.len() > 1 << 20).unwrap_or(false) {
            let _ = std::fs::remove_file(&path);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "[{}] {text}", unsafe { win::GetCurrentProcessId() });
        }
    }
}

// ---------------------------------------------------------------- the trainer's commands

struct Commands;

fn ok(mut v: Value) -> String {
    v["ok"] = json!(true);
    v.to_string()
}

fn fail(e: impl std::fmt::Display) -> String {
    json!({"ok": false, "error": e.to_string()}).to_string()
}

impl Shared {
    pub fn status(&self, now: u64) -> Value {
        let driven = self.engine.fx_driven();
        let fx: Vec<Value> = self
            .fx
            .names()
            .iter()
            .enumerate()
            .map(|(i, n)| {
                json!({
                    "name": n,
                    "value": self.snapshot.fx_values.get(i).copied().unwrap_or(false),
                    "assets": self.snapshot.fx_assets.get(i).copied().unwrap_or(0),
                    "reads": hook::reads(i),
                    "override_ms": self.engine.override_left(i, now),
                    "ruled": driven.contains(&i),
                })
            })
            .collect();
        let sn = &self.snapshot;
        json!({
            "ok": true,
            "version": VERSION,
            "build": game::BUILD,
            "pid": unsafe { win::GetCurrentProcessId() },
            "pipe": self.pipe,
            "phase": self.phase,
            "reason": self.reason,
            "hooked": self.hooked,
            "exe_base": sn.exe_base,
            "scan": sn.scan,
            "worlds": sn.worlds,
            "skater": sn.skater.map(|r| json!({"id": r.id, "ctx": format!("{:#x}", r.ctx), "pages": r.pages})),
            "records": sn.records.iter().map(|r| json!({"id": r.id, "pages": r.pages})).collect::<Vec<_>>(),
            "inputs": sn.inputs,
            "fx": fx,
            "rules": self.engine.file.rules.len(),
            "ticks": sn.ticks,
            "log": self.log.iter().rev().take(20).rev().collect::<Vec<_>>(),
        })
    }
}

impl pipe::Answer for Commands {
    fn answer(&self, line: &str) -> (String, bool) {
        let req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => return (fail(format!("bad request: {e}")), false),
        };
        let now = win::now_ms();
        let mut s = lock();
        let text = match req {
            Request::Ping => ok(json!({"version": VERSION, "pid": unsafe { win::GetCurrentProcessId() }, "phase": s.phase})),
            Request::Status => s.status(now).to_string(),
            Request::Fire { fx, ms } => {
                let ms = ms.unwrap_or(proto::FIRE_MS).min(600_000);
                match s.fx.index(&fx) {
                    Some(i) => {
                        s.engine.fire(i, now, ms);
                        let assets = s.snapshot.fx_assets.get(i).copied().unwrap_or(0);
                        ok(json!({"fx": rules::fx_key(&fx), "ms": ms, "assets": assets}))
                    }
                    None => fail(format!("no room for another fx state ({} at most)", hook::MAX_FX)),
                }
            }
            Request::Stop { fx: Some(fx) } => match s.fx.find(&fx) {
                Some(i) => {
                    s.engine.fire(i, now, 0);
                    ok(json!({"fx": rules::fx_key(&fx)}))
                }
                None => fail(format!("no fx state {}", rules::fx_key(&fx))),
            },
            Request::Stop { fx: None } => {
                s.engine.clear_overrides();
                ok(json!({}))
            }
            Request::Rules { rules: file } => {
                let mut names = s.fx.clone();
                match Engine::compile(&file, &mut names) {
                    Ok(mut e) => {
                        e.keep_overrides(&s.engine);
                        let inputs: Vec<String> = e.inputs.iter().map(|d| d.name.clone()).collect();
                        let n = file.rules.len();
                        s.fx = names;
                        s.engine = e;
                        s.rules_changed = true;
                        drop(s);
                        log(&format!("rules loaded: {n} rule(s)"));
                        return (ok(json!({"rules": n, "inputs": inputs})), false);
                    }
                    Err(e) => fail(e),
                }
            }
            Request::RulesGet => ok(json!({"rules": s.engine.file})),
            Request::Rescan => {
                s.rescan = true;
                ok(json!({}))
            }
            Request::Unload => {
                set_phase(&mut s, Phase::Stopping, "unloading");
                return (ok(json!({"unloading": true})), true);
            }
        };
        (text, false)
    }
}

unsafe extern "system" fn pipe_main(_: *mut c_void) -> u32 {
    let name = lock().pipe.clone();
    let result = std::panic::catch_unwind(|| pipe::serve(&name, &Commands, &STOP));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            log(&format!("pipe: {e}"));
            1
        }
        Err(_) => {
            log("pipe: panicked");
            2
        }
    }
}

// ---------------------------------------------------------------- the live side

/// Everything the worker tracks in the game.
struct Live {
    rt: Runtime,
    vt_bool: usize,
    vt_enum: usize,
    vt_float: usize,
    view_vt: usize,
    scanner: Option<Scanner>,
    scan_started: u64,
    info: ScanInfo,
    next_scan: u64,
    backoff: u64,
    /// Caveman's assets: the hit and its fx index.
    ours: Vec<(Hit, usize)>,
    /// The skater's states by lower-case name.
    states: HashMap<String, Vec<Hit>>,
    wanted: Vec<(String, Option<&'static str>, Option<Kind>)>,
    worlds: Vec<(usize, u32)>,
    worlds_at: u64,
    validated_at: u64,
    records: Vec<Record>,
    skater: Option<Record>,
    ticks: u64,
}

impl Live {
    fn new(base: usize) -> Option<Live> {
        let rt = Runtime::new(base)?;
        Some(Live {
            rt,
            vt_bool: base + rva::BOOL_ASSET_VTABLE,
            vt_enum: base + rva::ENUM_ASSET_VTABLE,
            vt_float: base + rva::FLOAT_ASSET_VTABLE,
            view_vt: base + rva::BOOL_VIEW_VTABLE,
            scanner: None,
            scan_started: 0,
            info: ScanInfo::default(),
            next_scan: 0,
            backoff: FIRST_BACKOFF_MS,
            ours: Vec::new(),
            states: HashMap::new(),
            wanted: Vec::new(),
            worlds: Vec::new(),
            worlds_at: 0,
            validated_at: 0,
            records: Vec::new(),
            skater: None,
            ticks: 0,
        })
    }

    fn vtable_of(&self, kind: Kind) -> usize {
        match kind {
            Kind::Bool => self.vt_bool,
            Kind::Int => self.vt_enum,
            Kind::Float => self.vt_float,
        }
    }

    /// The states to read: every known short-named state, and any full name the rules use.
    fn set_wanted(&mut self, engine: &Engine) {
        let mut wanted: Vec<(String, Option<&'static str>, Option<Kind>)> = rules::INPUTS.iter().map(|(s, n, k)| (n.to_string(), Some(*s), Some(*k))).collect();
        for d in &engine.inputs {
            if !wanted.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(&d.name)) {
                wanted.push((d.name.clone(), None, d.kind));
            }
        }
        let missing = wanted.iter().any(|(n, _, _)| !self.states.contains_key(&n.to_ascii_lowercase()));
        self.wanted = wanted;
        if missing && self.scanner.is_none() {
            self.next_scan = self.next_scan.min(win::now_ms() + 500);
        }
    }

    fn start_scan(&mut self, now: u64) {
        let spec = ScanSpec {
            classes: vec![(self.vt_bool, Kind::Bool), (self.vt_enum, Kind::Int), (self.vt_float, Kind::Float)],
            bool_view_vtable: Some(self.view_vt),
            wanted: self.wanted.iter().map(|(n, _, _)| n.clone()).collect(),
            prefix: OUR_PREFIX.into(),
            mapped: false,
        };
        match Scanner::new(spec) {
            Ok(s) => {
                self.scanner = Some(s);
                self.scan_started = now;
                self.info.running = true;
            }
            Err(e) => {
                log(&format!("scan: {e}"));
                self.next_scan = now + MAX_BACKOFF_MS;
            }
        }
    }

    /// A finished scan: its hits merged with the assets still valid, the hook table rebuilt.
    fn finish_scan(&mut self, now: u64, s: Scanner) {
        let mut sh = lock();
        let mut ours: Vec<(Hit, usize)> = Vec::new();
        let mut states: HashMap<String, Vec<Hit>> = HashMap::new();
        let old_ours: Vec<Hit> = self.ours.iter().map(|(h, _)| h.clone()).collect();
        let old_states: Vec<Hit> = self.states.values().flatten().cloned().collect();
        let kept = old_ours.into_iter().chain(old_states).filter(|h| scan::still_there(h, self.vtable_of(h.kind)));
        for h in s.hits.iter().cloned().chain(kept) {
            let lower = h.name.to_ascii_lowercase();
            if lower.starts_with(OUR_PREFIX) {
                if h.kind == Kind::Bool && !ours.iter().any(|(o, _)| o.asset == h.asset) {
                    match sh.fx.index(&h.name) {
                        Some(i) => ours.push((h, i)),
                        None => continue,
                    }
                }
            } else {
                let list = states.entry(lower).or_default();
                if !list.iter().any(|o| o.asset == h.asset) {
                    list.push(h);
                }
            }
        }
        self.ours = ours;
        self.states = states;
        drop(sh);
        let placed = self.publish_views();
        self.info = ScanInfo {
            running: false,
            scans: self.info.scans + 1,
            last_ms: now - self.scan_started,
            last_mb: s.bytes >> 20,
            regions: s.regions,
            candidates: s.candidates_seen,
            ours: self.ours.len(),
            states: self.states.values().map(Vec::len).sum(),
            next_in_ms: None,
        };
        let missing: Vec<&str> = self.wanted.iter().filter(|(n, _, _)| !self.states.contains_key(&n.to_ascii_lowercase())).map(|(n, s, _)| s.unwrap_or(n.as_str())).collect();
        if missing.is_empty() && !self.ours.is_empty() {
            self.next_scan = u64::MAX;
            self.backoff = FIRST_BACKOFF_MS;
        } else {
            self.next_scan = now + self.backoff;
            self.backoff = (self.backoff * 2).min(MAX_BACKOFF_MS);
        }
        log(&format!(
            "scan {}: {} MB in {} ms ({} regions, {} candidates): {} of Caveman's fx assets ({placed} in the hook table), {} skater states{}",
            self.info.scans,
            self.info.last_mb,
            self.info.last_ms,
            self.info.regions,
            self.info.candidates,
            self.ours.len(),
            self.info.states,
            if missing.is_empty() { String::new() } else { format!("; not found yet: {}", missing.join(", ")) }
        ));
    }

    fn publish_views(&self) -> usize {
        let views: Vec<(usize, u8)> = self.ours.iter().map(|(h, i)| (h.asset + layout::BOOL_VIEW, *i as u8)).collect();
        hook::set_views(&views)
    }

    /// Drops assets that went away (a level unloaded); a rescan follows soon.
    fn validate(&mut self, now: u64) {
        let before = (self.ours.len(), self.states.values().map(Vec::len).sum::<usize>());
        let (vb, ve, vf) = (self.vt_bool, self.vt_enum, self.vt_float);
        let vt = |k: Kind| match k {
            Kind::Bool => vb,
            Kind::Int => ve,
            Kind::Float => vf,
        };
        self.ours.retain(|(h, _)| scan::still_there(h, vt(h.kind)));
        for list in self.states.values_mut() {
            list.retain(|h| scan::still_there(h, vt(h.kind)));
        }
        self.states.retain(|_, l| !l.is_empty());
        let after = (self.ours.len(), self.states.values().map(Vec::len).sum::<usize>());
        if after != before {
            if after.0 != before.0 {
                self.publish_views();
            }
            log(&format!("assets went away (fx {} -> {}, states {} -> {}): rescanning soon", before.0, after.0, before.1, after.1));
            self.backoff = FIRST_BACKOFF_MS;
            self.next_scan = self.next_scan.min(now + 1000);
        }
    }

    fn follow_skater(&mut self, now: u64) {
        if self.worlds_at == 0 || now >= self.worlds_at + if self.skater.is_some() { WORLDS_MS } else { WORLDS_MS / 2 } {
            // a thread shows the world in its TLS only while it works in it: keep the worlds seen before while their
            // manager is still up, and add the ones seen now
            let mut worlds = self.rt.worlds();
            for &(w, t) in &self.worlds {
                if worlds.len() < 4 && !worlds.iter().any(|(x, _)| *x == w) && self.rt.manager(w).is_some() {
                    worlds.push((w, t));
                }
            }
            self.worlds = worlds;
            self.worlds_at = now;
        }
        let mut records = Vec::new();
        let mut managers = Vec::new();
        for &(world, _) in &self.worlds {
            if let Some(m) = self.rt.manager(world) {
                if !managers.contains(&m) {
                    managers.push(m);
                    records.extend(self.rt.records(m));
                }
            }
        }
        // the skater's record: the context with the most pages (the board's has 2), the lower id on a tie
        self.skater = records.iter().copied().max_by(|a, b| a.pages.cmp(&b.pages).then(b.id.cmp(&a.id))).filter(|r| r.pages > 0);
        self.records = records;
    }

    /// Reads every wanted state in the skater's context: (lower-case name -> reading).
    fn read_states(&self) -> HashMap<String, skater::Reading> {
        let mut out = HashMap::new();
        let ctx = self.skater.map(|r| r.ctx);
        for (name, _, kind) in &self.wanted {
            let lower = name.to_ascii_lowercase();
            let Some(hits) = self.states.get(&lower) else { continue };
            let mut best: Option<skater::Reading> = None;
            for h in hits {
                let k = kind.unwrap_or(h.kind);
                let r = skater::read_state(h.asset, k, ctx);
                if r.live {
                    best = Some(r);
                    break;
                }
                if best.is_none_or(|b| b.value.is_none()) {
                    best = Some(r);
                }
            }
            if let Some(b) = best {
                out.insert(lower, b);
            }
        }
        out
    }

    fn tick(&mut self, now: u64, values_out: &mut Vec<bool>) {
        self.ticks += 1;
        // the trainer's requests that change what is looked for
        {
            let mut sh = lock();
            if sh.rescan {
                sh.rescan = false;
                if self.scanner.is_none() {
                    self.backoff = FIRST_BACKOFF_MS;
                    self.next_scan = now;
                }
            }
            if sh.rules_changed {
                sh.rules_changed = false;
                let engine = sh.engine.clone();
                drop(sh);
                self.set_wanted(&engine);
            }
        }
        // scanning
        if let Some(s) = self.scanner.as_mut() {
            if s.step(SCAN_BUDGET, SCAN_STEP_MS) {
                let s = self.scanner.take().unwrap();
                self.finish_scan(now, s);
            }
        } else if now >= self.next_scan {
            self.start_scan(now);
        }
        if now >= self.validated_at + VALIDATE_MS {
            self.validated_at = now;
            self.validate(now);
        }
        self.follow_skater(now);
        let readings = self.read_states();
        // the rules
        let mut sh = lock();
        let values: Vec<Option<f64>> = sh.engine.inputs.iter().map(|d| readings.get(&d.name.to_ascii_lowercase()).and_then(|r| r.value)).collect();
        sh.engine.tick(now, &values, values_out);
        let n = sh.fx.names().len();
        values_out.resize(n.max(values_out.len()), false);
        for (i, &on) in values_out.iter().enumerate() {
            hook::set_value(i, on);
        }
        // the snapshot
        let mut fx_assets = vec![0usize; n];
        for (_, i) in &self.ours {
            if *i < n {
                fx_assets[*i] += 1;
            }
        }
        let mut info = self.info.clone();
        info.next_in_ms = (self.scanner.is_none() && self.next_scan != u64::MAX).then(|| self.next_scan.saturating_sub(now));
        info.running = self.scanner.is_some();
        sh.snapshot = Snapshot {
            exe_base: format!("{:#x}", self.rt.base),
            scan: info,
            skater: self.skater,
            records: self.records.clone(),
            worlds: self.worlds.len(),
            inputs: self
                .wanted
                .iter()
                .map(|(name, short, kind)| {
                    let lower = name.to_ascii_lowercase();
                    let r = readings.get(&lower);
                    let hits = self.states.get(&lower);
                    InputInfo {
                        name: name.clone(),
                        short: *short,
                        kind: kind.or_else(|| hits.and_then(|h| h.first()).map(|h| h.kind)),
                        assets: hits.map_or(0, Vec::len),
                        value: r.and_then(|r| r.value),
                        live: r.is_some_and(|r| r.live),
                    }
                })
                .collect(),
            fx_values: values_out[..n].to_vec(),
            fx_assets,
            ticks: self.ticks,
        };
    }
}

/// Waits between ticks: a high-resolution timer when the system has one.
struct Ticker(win::Handle);

impl Ticker {
    fn new() -> Ticker {
        Ticker(win::Handle(unsafe { win::CreateWaitableTimerExW(std::ptr::null_mut(), std::ptr::null(), win::CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, win::TIMER_ALL_ACCESS) }))
    }

    fn wait(&self, ms: u32) {
        if self.0.valid() {
            let due = -(ms as i64) * 10_000;
            if unsafe { win::SetWaitableTimer(self.0 .0, &due, 0, std::ptr::null_mut(), std::ptr::null_mut(), 0) } != 0 {
                unsafe { win::WaitForSingleObject(self.0 .0, ms + 100) };
                return;
            }
        }
        unsafe { win::Sleep(ms) };
    }
}

/// How the worker ended.
enum End {
    /// `unload`: free the DLL.
    Unload,
    /// Something went wrong past the point of a clean exit: stay loaded and inert.
    Stay,
}

fn run() -> End {
    let pid = unsafe { win::GetCurrentProcessId() };
    let exe = win::module_path(std::ptr::null_mut()).unwrap_or_default();
    let in_game = std::path::Path::new(&exe).file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case("skate.exe"));
    lock().pipe = proto::pipe_for(in_game, pid);
    let h = unsafe { win::CreateThread(std::ptr::null_mut(), 0, pipe_main, std::ptr::null_mut(), 0, std::ptr::null_mut()) };
    PIPE_THREAD.store(h as usize, Ordering::Release);
    log(&format!("caveman_fx {VERSION} loaded into {exe} (pid {pid})"));
    let ticker = Ticker::new();

    let base = match game::verify_exe() {
        Ok(base) => base,
        Err(why) => {
            log(&format!("dormant: {why}"));
            set_phase(&mut lock(), Phase::Dormant, &why);
            while !STOP.load(Ordering::Acquire) {
                ticker.wait(100);
            }
            return End::Unload;
        }
    };
    let Some(mut live) = Live::new(base) else {
        let why = "the game's globals are not readable";
        log(&format!("dormant: {why}"));
        set_phase(&mut lock(), Phase::Dormant, why);
        while !STOP.load(Ordering::Acquire) {
            ticker.wait(100);
        }
        return End::Unload;
    };
    let slot = base + rva::BOOL_VIEW_VTABLE + rva::BOOL_VIEW_GETTER_SLOT * 8;
    if let Err(e) = unsafe { hook::install(slot, base + rva::BOOL_GETTER) } {
        let why = format!("cannot hook the bool view getter: {e}");
        log(&format!("dormant: {why}"));
        set_phase(&mut lock(), Phase::Dormant, &why);
        while !STOP.load(Ordering::Acquire) {
            ticker.wait(100);
        }
        return End::Unload;
    }
    {
        let mut s = lock();
        s.hooked = true;
        set_phase(&mut s, Phase::Active, "hooked; finding the assets");
    }
    log(&format!("active: Skate.exe {} at {base:#x}; hooked slot {slot:#x}", game::BUILD));
    let mut values = Vec::new();
    let mut had_skater = false;
    while !STOP.load(Ordering::Acquire) {
        let now = win::now_ms();
        live.tick(now, &mut values);
        if live.skater.is_some() != had_skater {
            had_skater = live.skater.is_some();
            match live.skater {
                Some(r) => log(&format!("skater found: record {} ({} pages, {} records)", r.id, r.pages, live.records.len())),
                None => log("no skater (menu or loading)"),
            }
        }
        ticker.wait(TICK_MS);
    }
    for i in 0..hook::MAX_FX {
        hook::set_value(i, false);
    }
    End::Unload
}

/// The worker thread (started from DllMain).
///
/// # Safety
/// Started once per load, by DllMain.
pub unsafe extern "system" fn thread_main(_: *mut c_void) -> u32 {
    let end = std::panic::catch_unwind(run).unwrap_or(End::Stay);
    STOP.store(true, Ordering::Release);
    let unhooked = match hook::uninstall() {
        Ok(()) => true,
        Err(e) => {
            log(&format!("cannot restore the vtable slot ({e}): staying loaded, forwarding every call"));
            false
        }
    };
    lock().hooked = !unhooked && hook::installed();
    let pipe = PIPE_THREAD.load(Ordering::Acquire) as win::HANDLE;
    let pipe_done = pipe.is_null() || win::WaitForSingleObject(pipe, 5_000) == win::WAIT_OBJECT_0;
    if !pipe.is_null() && pipe_done {
        win::CloseHandle(pipe);
    }
    if matches!(end, End::Stay) || !unhooked || !pipe_done {
        log("stopped; the DLL stays loaded (inert)");
        return 0;
    }
    log("unloading");
    // a game thread that read the slot just before the swap back may still be inside the hook: give it time
    win::Sleep(1000);
    win::FreeLibraryAndExitThread(MODULE.load(Ordering::Acquire) as win::HANDLE, 0)
}
