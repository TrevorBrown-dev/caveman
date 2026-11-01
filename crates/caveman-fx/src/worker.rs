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
use crate::{audio, bars, overlay, scene};

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
    /// Loaded into Skate.exe (not a test host).
    pub in_game: bool,
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
            in_game: false,
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
            "render": overlay::status(),
            "bars": overlay::settings(),
            "audio": audio::status(),
            "board": crate::board::brief(),
            "rim": crate::rim::status(),
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
            Request::Draw { test, queue, reset, streamline } => {
                if let Some(q) = queue {
                    let mode = match q.to_ascii_lowercase().as_str() {
                        "auto" => 0,
                        "swapchain" => 1,
                        "scan" => 2,
                        "busiest" => 3,
                        other => return (fail(format!("queue {other:?}: auto, swapchain, scan or busiest")), false),
                    };
                    overlay::QUEUE_MODE.store(mode, Ordering::Release);
                }
                if let Some(t) = test {
                    overlay::TEST.store(t, Ordering::Release);
                }
                if reset == Some(true) {
                    overlay::reset();
                }
                INSTALL_TRIED.store(false, Ordering::Release);
                if let Some(on) = streamline {
                    drop(s);
                    let note = overlay::hook_streamline(on);
                    log(&format!("overlay: {}", note.as_ref().unwrap_or_else(|e| e)));
                    return match note {
                        Ok(n) => (ok(json!({"render": overlay::status(), "note": n})), false),
                        Err(e) => (fail(e), false),
                    };
                }
                ok(json!({"render": overlay::status()}))
            }
            Request::Depth { on, near, copies } => {
                if let Some(c) = &copies {
                    match c.trim() {
                        "picture" => crate::depth::COPY_PER_PICTURE.store(true, Ordering::Release),
                        "window" => crate::depth::COPY_PER_PICTURE.store(false, Ordering::Release),
                        other => return (fail(format!("copies {other}: picture or window")), false),
                    }
                }
                let mut hooks_note = None;
                if let Some(on) = on {
                    let was = crate::depth::ENABLED.swap(on, Ordering::AcqRel);
                    if was != on {
                        // the shared command list table's hooks only while something wants them (audit M3): the
                        // test, or the staging ring and the trace, which keep them through a `depth off`
                        hooks_note = Some(if on || !crate::depth::hooks_wanted() {
                            crate::depth::set_shared_hooks(on)
                        } else {
                            "the command list hooks stay (the staging ring or a trace reads the copies)".into()
                        });
                    }
                }
                if let Some(n) = near {
                    if !n.is_finite() || !(0.0..=5.0).contains(&n) {
                        return (fail(format!("near {n}: metres between 0 (calibrate) and 5")), false);
                    }
                    crate::depth::NEAR_OVERRIDE.store(n.to_bits(), Ordering::Release);
                }
                drop(s);
                if on.is_some() || near.is_some() || copies.is_some() {
                    log(&format!("depth: on {on:?}, near {near:?}, copies {copies:?}{}", hooks_note.map(|n| format!("; {n}")).unwrap_or_default()));
                }
                return (ok(json!({"render": overlay::status()})), false);
            }
            Request::Bars { set, land } => {
                let mut changed = None;
                if let Some(patch) = set {
                    match overlay::settings().patched(&patch) {
                        Ok(new) => {
                            overlay::set_settings(new.clone());
                            changed = Some((new, patch));
                        }
                        Err(e) => return (fail(e), false),
                    }
                }
                if land == Some(true) {
                    overlay::LANDINGS.fetch_add(1, Ordering::AcqRel);
                }
                INSTALL_TRIED.store(false, Ordering::Release);
                let in_game = s.in_game;
                drop(s);
                if let Some((new, patch)) = changed {
                    if in_game {
                        save_settings(&new);
                    }
                    log(&format!("bars: {patch}"));
                }
                return (ok(json!({"bars": overlay::settings(), "render": overlay::status(), "audio": audio::status(), "rim": crate::rim::status()})), false);
            }
            Request::Board {
                art,
                keep,
                state,
                find,
                fx,
                on,
                ms,
                verify,
                arts,
                siblings,
                state_for,
                composite_common,
                target,
                uploads,
                finals,
                copies,
                peek,
                force,
                owners,
                slices,
                slices_state,
                hunt,
                owners_of,
                worldcopy,
                worldcopy_hashes,
                hunt_abort,
            } => {
                // The Caveman (board.rs): the overlay installs for it on the next tick (its Present hook uploads)
                INSTALL_TRIED.store(false, Ordering::Release);
                let mut changed = Value::Null;
                if let Some(name) = fx {
                    let key = cave_fx_key(&name);
                    let Some(i) = s.fx.index(&key) else { return (fail(format!("no room for another fx state ({} at most)", hook::MAX_FX)), false) };
                    let ms = match (on, ms) {
                        (Some(true), _) => CAVE_LATCH_MS,
                        (Some(false), _) => 0,
                        (None, Some(ms)) => ms.clamp(1, 600_000),
                        (None, None) => return (fail("an fx needs on (true or false) or ms"), false),
                    };
                    s.engine.fire(i, now, ms);
                    changed = json!({"fx": key, "ms": ms, "assets": s.snapshot.fx_assets.get(i).copied().unwrap_or(0)});
                }
                let fx_list = s.cave_fx(now);
                drop(s);
                // the arts: one per deck slot, from the chains sent (`art`: the graphic's BC7 chain, the digest the
                // status reports; `arts`: other chains, each naming its slot: the same slot's chains make one art,
                // labelled and digested by the first of them)
                let mut arts_in: Vec<crate::board::Art> = Vec::new();
                for (i, c) in art.iter().chain(arts.iter().flatten()).enumerate() {
                    let Some(slot) = crate::board::Slot::parse(&c.slot) else {
                        return (fail(format!("art chain {i}: slot {:?}: art, osk, ny or rgb", c.slot)), false);
                    };
                    let chain = match (c.chain_format(), c.chain()) {
                        (Ok(format), Ok(bytes)) => crate::board::Chain { format, width: c.width, height: c.height, levels: c.levels as usize, bytes },
                        (Err(e), _) | (_, Err(e)) => return (fail(format!("art chain {i}: {e}")), false),
                    };
                    match arts_in.iter_mut().find(|a| a.slot == slot) {
                        Some(a) => a.chains.push(chain),
                        None => arts_in.push(crate::board::Art { chains: vec![chain], label: c.label.clone(), sha256: c.sha256.to_ascii_lowercase(), slot }),
                    }
                }
                if let Err(e) = crate::board::request(
                    now,
                    arts_in,
                    keep,
                    state.as_deref(),
                    state_for.as_deref(),
                    find == Some(true),
                    verify == Some(true),
                    siblings,
                    composite_common,
                    target.as_deref(),
                    uploads == Some(true),
                    finals,
                    copies == Some(true),
                    peek.as_ref(),
                    force == Some(true),
                    owners == Some(true),
                    slices.as_deref(),
                    slices_state.as_deref(),
                    hunt.as_ref(),
                    owners_of.as_deref(),
                    worldcopy,
                    worldcopy_hashes.as_deref(),
                    hunt_abort == Some(true),
                ) {
                    return (fail(e), false);
                }
                if !changed.is_null() {
                    log(&format!("board: {changed}"));
                }
                return (ok(json!({"board": crate::board::status(now), "fx": fx_list, "changed": changed})), false);
            }
            Request::Diag { dump, key, auto } => {
                drop(s);
                if let Some(k) = &key {
                    let code = match k.trim().to_ascii_lowercase().as_str() {
                        "off" | "none" => 0,
                        t if t.starts_with('f') && t[1..].parse::<u32>().is_ok_and(|n| (1..=24).contains(&n)) => 0x70 + t[1..].parse::<u32>().unwrap() - 1,
                        t if t.starts_with("0x") && u32::from_str_radix(&t[2..], 16).is_ok() => u32::from_str_radix(&t[2..], 16).unwrap(),
                        other => return (fail(format!("key {other}: F1 to F24, a hex virtual-key code, or off")), false),
                    };
                    crate::diag::KEY.store(code, Ordering::Release);
                    log(&format!("diag: the mark key is {k}"));
                }
                if let Some(a) = auto {
                    crate::diag::AUTO.store(a, Ordering::Release);
                }
                let mut dumped = Value::Null;
                if let Some(d) = dump {
                    let path = match &d {
                        Value::String(p) if !p.is_empty() => Some(std::path::PathBuf::from(p)),
                        _ => None,
                    };
                    if d != Value::Bool(false) {
                        match crate::diag::dump(path) {
                            Ok((p, n)) => {
                                log(&format!("diag: {n} presents dumped to {}", p.display()));
                                dumped = json!({"path": p.display().to_string(), "presents": n});
                            }
                            Err(e) => return (fail(format!("dump: {e}")), false),
                        }
                    }
                }
                return (ok(json!({"diag": crate::diag::status(), "dumped": dumped})), false);
            }
            Request::Pip { mesh, clear, art } => {
                // the board cam's meshes (pip.rs); the overlay installs for it on the next tick if the cam is on
                let in_game = s.in_game;
                drop(s);
                if let Some(art) = art {
                    if in_game {
                        return (fail("a test art texture is for a test host, not the game"), false);
                    }
                    let raw = usize::from_str_radix(art.trim_start_matches("0x"), 16).unwrap_or(0);
                    crate::pip::set_test_art(raw);
                    log(&format!("board cam: test art texture {raw:#x}"));
                }
                if clear == Some(true) {
                    crate::pip::clear();
                    if in_game {
                        crate::pip::forget_saved();
                    }
                    log("board cam: meshes cleared");
                }
                if let Some(mesh) = mesh {
                    if let Err(e) = mesh.check() {
                        return (fail(format!("board cam meshes: {e}")), false);
                    }
                    log(&format!(
                        "board cam: meshes set ({}: {} vertices, {} triangles, {} part(s))",
                        mesh.label,
                        mesh.vertex_count(),
                        mesh.indices.len() / 3,
                        mesh.parts.len()
                    ));
                    if in_game {
                        crate::pip::save(&mesh);
                    }
                    crate::pip::set(mesh);
                }
                INSTALL_TRIED.store(false, Ordering::Release);
                return (ok(json!({"pip": crate::pip::status(), "bars": overlay::settings()})), false);
            }
            Request::Mem { trace, min_kb, stop, report, ring, ring_mb, ring_any } => {
                // the ring and the trace need the hooks: the overlay installs for them on the next tick
                INSTALL_TRIED.store(false, Ordering::Release);
                let in_game = s.in_game;
                drop(s);
                if ring.is_some() || ring_mb.is_some() || ring_any.is_some() {
                    if ring_any.is_some() && in_game {
                        return (fail("ring_any is for a test host, not the game"), false);
                    }
                    let v = crate::stagering::set(ring, ring_mb, ring_any);
                    // the ring reads the copies through the command lists' hooks: in again when they were taken out
                    if ring == Some(true) && overlay::installed() {
                        log(&format!("staging ring: {}", crate::depth::set_shared_hooks(true)));
                    }
                    return (ok(json!({"mem": crate::alloctrace::status(), "ring": v, "copy_hooks": crate::depth::copy_hooks_active()})), false);
                }
                if stop == Some(true) {
                    return (ok(json!({"mem": crate::alloctrace::stop()})), false);
                }
                if let Some(seconds) = trace {
                    let v = crate::alloctrace::start(seconds, min_kb.unwrap_or(64) * 1024);
                    if overlay::installed() {
                        log(&format!("allocation trace: {}", crate::depth::set_shared_hooks(true)));
                    }
                    return (ok(json!({"mem": v})), false);
                }
                if report == Some(true) {
                    return match crate::alloctrace::report() {
                        Ok(v) => (ok(json!({"mem": v})), false),
                        Err(e) => (fail(e), false),
                    };
                }
                return (ok(json!({"mem": crate::alloctrace::status(), "ring": crate::stagering::status()})), false);
            }
            Request::Video { on } => {
                drop(s);
                let v = match on {
                    Some(on) => {
                        let v = crate::video::set(on);
                        log(&format!("video: {} ({})", if on { "on" } else { "off" }, v["note"].as_str().unwrap_or("")));
                        v
                    }
                    None => crate::video::status(),
                };
                return (ok(json!({"video": v})), false);
            }
            Request::Material { key, values, int, raw, publish } => {
                // the ridden board's live material parameters (material.rs): read, or one written and published
                drop(s);
                let base = crate::scene::EXE_BASE.load(Ordering::Acquire);
                let ctx = crate::scene::CLIENT_CTX.load(Ordering::Acquire);
                return match crate::material::handle(base, ctx, key.as_deref(), values.as_deref(), int, raw.as_deref(), publish != Some(false), now) {
                    Ok(v) => {
                        if let Some(k) = v.get("written") {
                            log(&format!(
                                "material: {} written into {} node(s), published: {}",
                                key.as_deref().unwrap_or(""),
                                k.as_array().map_or(0, |a| a.len()),
                                v["published"]
                            ));
                        }
                        (ok(json!({"material": v})), false)
                    }
                    Err(e) => (fail(e), false),
                };
            }
            Request::Rim { path, clear } => {
                // the deck's rim for the bars' rim layout (rim.rs); kept for the next load in the game
                let in_game = s.in_game;
                drop(s);
                if clear == Some(true) {
                    crate::rim::clear();
                    if in_game {
                        crate::rim::forget_saved();
                    }
                    log("bars: deck rim cleared (the rim layout draws the ring)");
                }
                if let Some(path) = path {
                    let kept = path.clone();
                    match crate::rim::Rim::new(path) {
                        Ok(rim) => {
                            log(&format!("bars: deck rim set ({}, {} points, {:.3} m round)", rim.path.label, rim.len(), rim.length()));
                            crate::rim::set(rim);
                            if in_game {
                                crate::rim::save(&kept);
                            }
                        }
                        Err(e) => return (fail(e), false),
                    }
                }
                return (ok(json!({"rim": crate::rim::status(), "bars": overlay::settings()})), false);
            }
        };
        (text, false)
    }
}

/// `board fx <name> on`: on until turned off (a week).
const CAVE_LATCH_MS: u64 = 7 * 24 * 3600 * 1000;

/// The fx state a board effect name means: `aura`, `cave.aura` and `fx.cave.aura` are `fx.cave.aura`.
pub fn cave_fx_key(name: &str) -> String {
    let n = rules::fx_key(name);
    let tail = n.strip_prefix("fx.").unwrap_or(&n);
    if tail.starts_with("cave.") {
        format!("fx.{tail}")
    } else {
        format!("fx.cave.{tail}")
    }
}

impl Shared {
    /// The fx.cave.* states the DLL knows (found in the game, or switched by hand): value, assets, the game's reads,
    /// what is forcing them.
    pub fn cave_fx(&self, now: u64) -> Vec<Value> {
        self.fx
            .names()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.starts_with("fx.cave."))
            .map(|(i, n)| {
                json!({
                    "name": n,
                    "value": self.snapshot.fx_values.get(i).copied().unwrap_or(false),
                    "assets": self.snapshot.fx_assets.get(i).copied().unwrap_or(0),
                    "reads": hook::reads(i),
                    "override_ms": self.engine.override_left(i, now),
                })
            })
            .collect()
    }

    /// The game's reads of the fx.cave.* states, summed: they climb while The Caveman's effects are live.
    fn cave_reads(&self) -> u64 {
        self.fx.names().iter().enumerate().filter(|(_, n)| n.starts_with("fx.cave.")).map(|(i, _)| hook::reads(i) as u64).sum()
    }
}

/// The board's turn in a worker tick (only once a board request made it live).
fn board_tick(base: Option<usize>) {
    if !crate::board::wanted() {
        return;
    }
    let reads = lock().cave_reads();
    crate::board::tick(win::now_ms(), base, reads);
}

// ---------------------------------------------------------------- the overlay's side

/// An install was tried since the last draw or bars request (a failure is retried only on a new request).
static INSTALL_TRIED: AtomicBool = AtomicBool::new(false);
/// The audio capture is wanted (and was started).
static AUDIO_ON: AtomicBool = AtomicBool::new(false);

/// `%LOCALAPPDATA%\Caveman\fx\bars.json` (in the game only: a test host never touches it).
fn settings_path() -> Option<std::path::PathBuf> {
    data_dir().map(|d| d.join("bars.json"))
}

fn load_settings() {
    let Some(path) = settings_path() else { return };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    match serde_json::from_str::<bars::BarSettings>(&text).map_err(|e| e.to_string()).and_then(|s| s.sanitized()) {
        Ok(s) => {
            let on = s.enabled;
            overlay::set_settings(s);
            log(&format!("bars settings loaded from {} (bars {})", path.display(), if on { "on" } else { "off" }));
        }
        Err(e) => log(&format!("bars settings in {} ignored: {e}", path.display())),
    }
}

fn save_settings(s: &bars::BarSettings) {
    let Some(path) = settings_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(&path, text);
    }
}

/// Every tick: installs the overlay when something is to be drawn and drawing is allowed (the game verified, or a
/// test host); starts or stops the audio capture as the bars' source asks.
fn service_overlay(allowed: bool) {
    if allowed && overlay::hooks_wanted() && !overlay::installed() && !INSTALL_TRIED.swap(true, Ordering::AcqRel) {
        match overlay::install() {
            Ok(summary) => log(&format!("overlay {summary}")),
            Err(e) => log(&format!("overlay not installed: {e}")),
        }
    }
    let want_audio = allowed && overlay::audio_wanted();
    if want_audio != AUDIO_ON.load(Ordering::Acquire) {
        AUDIO_ON.store(want_audio, Ordering::Release);
        if want_audio {
            audio::start();
            log("audio capture started (the bars follow the PC's sound)");
        } else {
            audio::stop();
            log("audio capture stopped");
        }
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
    /// fx.landed's last value (its rises count landings for the bars).
    landed: bool,
    /// Finds the renderer's per-frame transforms for the bars (proxy.rs), with its own read of the simulation's scene.
    proxies: crate::proxy::Finder,
    scene: scene::Reader,
    t0: std::time::Instant,
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
            landed: false,
            proxies: crate::proxy::Finder::default(),
            scene: scene::Reader::default(),
            t0: std::time::Instant::now(),
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
        let missing: Vec<&str> =
            self.wanted.iter().filter(|(n, _, _)| !self.states.contains_key(&n.to_ascii_lowercase())).map(|(n, s, _)| s.unwrap_or(n.as_str())).collect();
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
        scene::publish_worlds(self.rt.base, &self.worlds);
        // the bars' per-frame transforms: looked for while the bars are on and placed by the frame's pair
        let s = overlay::settings();
        if s.enabled && s.timing == bars::Timing::Frame && overlay::installed() {
            let sim = self.scene.read(self.t0.elapsed().as_secs_f64());
            self.proxies.tick(self.rt.base, now, sim);
        }
        let readings = self.read_states();
        // the rules
        let mut sh = lock();
        let values: Vec<Option<f64>> = sh.engine.inputs.iter().map(|d| readings.get(&d.name.to_ascii_lowercase()).and_then(|r| r.value)).collect();
        sh.engine.tick(now, &values, values_out);
        if let Some(i) = sh.fx.find("fx.landed") {
            let on = values_out.get(i).copied().unwrap_or(false);
            if on && !self.landed {
                overlay::LANDINGS.fetch_add(1, Ordering::AcqRel);
            }
            self.landed = on;
        }
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
        Ticker(win::Handle(unsafe {
            win::CreateWaitableTimerExW(std::ptr::null_mut(), std::ptr::null(), win::CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, win::TIMER_ALL_ACCESS)
        }))
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
    {
        let mut s = lock();
        s.pipe = proto::pipe_for(in_game, pid);
        s.in_game = in_game;
    }
    let h = unsafe { win::CreateThread(std::ptr::null_mut(), 0, pipe_main, std::ptr::null_mut(), 0, std::ptr::null_mut()) };
    PIPE_THREAD.store(h as usize, Ordering::Release);
    log(&format!("caveman_fx {VERSION} loaded into {exe} (pid {pid})"));
    if in_game {
        load_settings();
        if let Some(line) = crate::pip::load_saved() {
            log(&line);
        }
        if let Some(line) = crate::rim::load_saved() {
            log(&line);
        }
    }
    let ticker = Ticker::new();

    let base = match game::verify_exe() {
        Ok(base) => base,
        Err(why) => {
            log(&format!("dormant: {why}"));
            set_phase(&mut lock(), Phase::Dormant, &why);
            while !STOP.load(Ordering::Acquire) {
                service_overlay(!in_game);
                board_tick(None);
                crate::memwatch::tick(win::now_ms());
                crate::stagering::tick(win::now_ms());
                crate::diag::service();
                overlay::flush_log();
                ticker.wait(if crate::board::wanted() { TICK_MS } else { 100 });
            }
            return End::Unload;
        }
    };
    // The game's globals (the engine singletons `Runtime::new` reads) come up some time after the exe is mapped and
    // ReSkate is in, and the renderer later still. A DLL put in as soon as skate. ran (Caveman's attach) found them
    // unreadable and stayed dormant for the whole session (2026-10-30 evening); made to try again every second, it
    // hooked the renderer and made the staging ring's heaps ten seconds into the start, and the game died twice
    // within ten seconds of that (2026-10-31 morning; without the DLL it started fine). Every activation before
    // those two had been into a game already running. So: dormant while the globals are not there, and after that
    // until the game has been in a world with a camera (the frontend at the earliest, well past the renderer's
    // start) for five seconds, and at least half a minute after the globals came when the DLL was in before them.
    let mut fresh_start = false;
    let mut live = match Live::new(base) {
        Some(live) => live,
        None => {
            fresh_start = true;
            let why = "the game's globals are not readable yet (tried again every second)";
            log(&format!("dormant: {why}"));
            set_phase(&mut lock(), Phase::Dormant, why);
            let mut next_try = win::now_ms() + 1000;
            let live = loop {
                if STOP.load(Ordering::Acquire) {
                    return End::Unload;
                }
                service_overlay(!in_game);
                board_tick(None);
                crate::memwatch::tick(win::now_ms());
                crate::stagering::tick(win::now_ms());
                crate::diag::service();
                overlay::flush_log();
                ticker.wait(if crate::board::wanted() { TICK_MS } else { 100 });
                if win::now_ms() >= next_try {
                    next_try = win::now_ms() + 1000;
                    if let Some(live) = Live::new(base) {
                        break live;
                    }
                }
            };
            log("the game's globals are readable now");
            live
        }
    };
    {
        let why = if fresh_start { "the game is starting (waiting for a world and a camera, and half a minute)" } else { "waiting for a world and a camera" };
        log(&format!("dormant: {why}"));
        set_phase(&mut lock(), Phase::Dormant, why);
        let globals_at = win::now_ms();
        let least = if fresh_start { 30_000 } else { 0 };
        let mut both_since: Option<u64> = None;
        let mut next_look = globals_at;
        loop {
            if STOP.load(Ordering::Acquire) {
                return End::Unload;
            }
            service_overlay(!in_game);
            board_tick(None);
            crate::memwatch::tick(win::now_ms());
            crate::stagering::tick(win::now_ms());
            crate::diag::service();
            overlay::flush_log();
            ticker.wait(if crate::board::wanted() { TICK_MS } else { 100 });
            let now = win::now_ms();
            if now < next_look {
                continue;
            }
            next_look = now + 1000;
            let ready = !live.rt.worlds().is_empty() && scene::camera_object(base).is_ok();
            if !ready {
                both_since = None;
                continue;
            }
            let since = *both_since.get_or_insert(now);
            if now.saturating_sub(since) >= 5000 && now.saturating_sub(globals_at) >= least {
                break;
            }
        }
        log(&format!("the game is up (a world and a camera for 5 s, {} s after its globals came): starting", win::now_ms().saturating_sub(globals_at) / 1000));
    }
    let slot = base + rva::BOOL_VIEW_VTABLE + rva::BOOL_VIEW_GETTER_SLOT * 8;
    if let Err(e) = unsafe { hook::install(slot, base + rva::BOOL_GETTER) } {
        let why = format!("cannot hook the bool view getter: {e}");
        log(&format!("dormant: {why}"));
        set_phase(&mut lock(), Phase::Dormant, &why);
        while !STOP.load(Ordering::Acquire) {
            service_overlay(!in_game);
            board_tick(None);
            crate::memwatch::tick(win::now_ms());
            crate::stagering::tick(win::now_ms());
            crate::diag::service();
            overlay::flush_log();
            ticker.wait(if crate::board::wanted() { TICK_MS } else { 100 });
        }
        return End::Unload;
    }
    {
        let mut s = lock();
        s.hooked = true;
        set_phase(&mut s, Phase::Active, "hooked; finding the assets");
    }
    log(&format!("active: Skate.exe {} at {base:#x}; hooked slot {slot:#x}", game::BUILD));
    scene::EXE_BASE.store(base, Ordering::Release);
    let mut values = Vec::new();
    let mut had_skater = false;
    while !STOP.load(Ordering::Acquire) {
        let now = win::now_ms();
        live.tick(now, &mut values);
        service_overlay(true);
        board_tick(Some(base));
        crate::memwatch::tick(now);
        crate::stagering::tick(now);
        crate::diag::service();
        overlay::flush_log();
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
    scene::EXE_BASE.store(0, Ordering::Release);
    audio::stop();
    // the allocation trace's sampler thread must be out of our code before the DLL goes; the staging ring hands out
    // nothing more (its heap lives on through the game's placed buffers on it)
    crate::alloctrace::shutdown();
    crate::stagering::shutdown();
    let overlay_ok = match overlay::uninstall() {
        Ok(()) => true,
        Err(e) => {
            log(&format!("overlay: {e}: staying loaded"));
            false
        }
    };
    // the video ring (its GPU side goes with the board's objects)
    crate::video::shutdown();
    // The Caveman's GPU objects, once our last upload is done (the Present hook is out now)
    crate::board::shutdown();
    // the client tick's jump back to ReSkate's hook (tick.rs); if it cannot go back, the DLL must stay
    let tick_ok = match unsafe { crate::tick::uninstall() } {
        Ok(()) => true,
        Err(e) => {
            log(&format!("client tick hook: {e}: staying loaded"));
            false
        }
    };
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
    if matches!(end, End::Stay) || !unhooked || !pipe_done || !overlay_ok || !tick_ok {
        log("stopped; the DLL stays loaded (inert)");
        return 0;
    }
    log("unloading");
    // a game thread that read the slot just before the swap back may still be inside the hook: give it time
    win::Sleep(1000);
    win::FreeLibraryAndExitThread(MODULE.load(Ordering::Acquire) as win::HANDLE, 0)
}
