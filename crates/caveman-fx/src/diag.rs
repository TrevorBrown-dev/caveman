//! The depth test's diagnostics: what each present did, kept for the last [`RING`] presents, an objective blink count
//! from the GPU, and ways to save the record when something goes wrong.
//!
//! - **The record** ([`Entry`], one per present the overlay handles): when (a monotonic clock in microseconds), the
//!   copy of the game's depth it tested against (which picture it was taken for, the window it was recorded and the
//!   one it was submitted in, when its pass ended and when the list carrying it was submitted, and on which queue), the
//!   transforms the bars were drawn with (the camera and the board, and how the renderer's proxies had moved since the
//!   last present: one step, none, or two), where the board's centre fell on screen, and, a few frames later, the GPU's
//!   count of the bars' pixels tested against the depth and hidden by it. Per present window: the main depth's pass
//!   ends, and those in the G-buffer's state, copied or not ([`window_end`]).
//! - **Blinks** ([`detect`]): the share of the bars' tested pixels that the depth hides changes smoothly from frame to
//!   frame (legs move in front gradually); a frame (or two) whose share jumps away from both neighbours while they
//!   agree is a blink: `unhidden` (the scene failed to hide the bars) or `overhidden`.
//! - **Saving**: `caveman-trainer draw log dump [FILE]` writes the whole record as JSON; a key (F8 by default, free in
//!   ReSkate, whose own keys are Insert and the backtick) saves the 3 s before it and the 1 s after it to
//!   `%TEMP%\caveman-fx\marks`, and so does each blink found (one every 10 s at most).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde_json::{json, Value};

/// Presents kept.
pub const RING: usize = 2048;

/// Microseconds on a monotonic clock (the performance counter), since the DLL first asked.
pub fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

/// How a proxy moved since the transforms drawn at the last present (see `proxy::Step`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Unknown,
    /// Its previous matrix is the one drawn last: one frame on.
    One,
    /// Unchanged since.
    None,
    /// Two frames on (its previous matrix is not the one drawn last): read early.
    Two,
    /// The last one drawn is from more than a present ago.
    Gap,
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Step::Unknown => "?",
            Step::One => "one",
            Step::None => "none",
            Step::Two => "two",
            Step::Gap => "gap",
        }
    }
}

/// The copy a present tested against.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CopyInfo {
    pub picture: u64,
    pub slot: u32,
    pub recorded: u64,
    pub submitted: u64,
    pub end_us: u64,
    pub submit_us: u64,
    pub queue: usize,
    /// The fence value our draw waited for (0: none).
    pub waited: u64,
}

/// One present.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Entry {
    pub present: u64,
    pub t_us: u64,
    /// The bars were drawn.
    pub drawn: bool,
    /// 0 none, 1 fresh, 2 late, 3 off.
    pub depth: u8,
    pub age: u32,
    pub copy: Option<CopyInfo>,
    pub draw_queue: usize,
    pub cam_pos: [f32; 3],
    pub cam_back: [f32; 3],
    pub fov: f32,
    pub board_pos: [f32; 3],
    /// The board's centre on screen (pixels).
    pub screen: [f32; 2],
    pub cam_step: Step,
    pub board_step: Step,
    /// How the copy paired with the frame (depth.rs `Pairing`: 0 exact, 1 one old, 2 ahead, 3 none).
    pub pairing: u8,
    /// Extra reads of the proxies until two agreed (the game writing them meanwhile).
    pub torn: u32,
    /// The GPU's count (sampled: one pixel in 16) of the bars' pixels tested against the depth, and hidden by it.
    pub tested: Option<u32>,
    pub hidden: Option<u32>,
    /// -1 unhidden, +1 overhidden.
    pub blink: i8,
}

const DEPTH_NAMES: [&str; 4] = ["-", "fresh", "late", "off"];

impl Entry {
    pub fn to_json(&self) -> Value {
        let r = |v: f32| (v * 10_000.0).round() / 10_000.0;
        let v3 = |v: [f32; 3]| v.map(r);
        let paired = ["exact", "one old", "ahead", "none"][(self.pairing as usize).min(3)];
        json!({
            "present": self.present,
            "t_us": self.t_us,
            "drawn": self.drawn,
            "depth": DEPTH_NAMES[(self.depth as usize).min(3)],
            "age": self.age,
            "copy": self.copy.map(|c| json!({
                "picture": c.picture, "slot": c.slot, "recorded_window": c.recorded, "submitted_window": c.submitted,
                "pass_end_us": c.end_us, "submit_us": c.submit_us, "queue": format!("{:#x}", c.queue),
                "same_queue": c.queue == self.draw_queue, "waited": c.waited,
            })),
            "cam": {"pos": v3(self.cam_pos), "back": v3(self.cam_back), "fov": r(self.fov), "step": self.cam_step.name()},
            "board": {"pos": v3(self.board_pos), "step": self.board_step.name(), "screen": self.screen.map(|v| v.round())},
            "paired": paired,
            "torn": self.torn,
            "tested": self.tested,
            "hidden": self.hidden,
            "blink": match self.blink { -1 => "unhidden", 1 => "overhidden", _ => "" },
        })
    }
}

/// The main depth's pass ends in a present window (the window before present `window + 1`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowStat {
    pub window: u64,
    pub ends: u32,
    /// In the G-buffer's state, and of those copied.
    pub gbuffer: u32,
    pub copied: u32,
    pub first_gbuffer_us: u64,
}

struct Record {
    entries: Vec<Entry>,
    windows: Vec<WindowStat>,
    blinks: u64,
    unhidden: u64,
    overhidden: u64,
    last_blinks: VecDeque<Value>,
}

static RECORD: Mutex<Option<Record>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Record) -> R) -> R {
    let mut g = RECORD.lock().unwrap_or_else(|e| e.into_inner());
    let r = g.get_or_insert_with(|| Record {
        entries: vec![Entry::default(); RING],
        windows: vec![WindowStat::default(); RING],
        blinks: 0,
        unhidden: 0,
        overhidden: 0,
        last_blinks: VecDeque::new(),
    });
    f(r)
}

/// Keeps a present's entry (the render thread, once a present).
pub fn record(e: Entry) {
    with(|r| r.entries[(e.present % RING as u64) as usize] = e);
}

/// A pass end of the main depth in `window` (a game thread, inside its barrier: never waits for the lock; a sample
/// dropped is nothing): in the G-buffer's state or not, copied or not.
pub fn window_end(window: u64, gbuffer: bool, copied: bool) {
    let now = now_us();
    let Ok(mut g) = RECORD.try_lock() else { return };
    let Some(r) = g.as_mut() else { return };
    {
        let w = &mut r.windows[(window % RING as u64) as usize];
        if w.window != window {
            *w = WindowStat { window, ..Default::default() };
        }
        w.ends += 1;
        if gbuffer {
            if w.gbuffer == 0 {
                w.first_gbuffer_us = now;
            }
            w.gbuffer += 1;
        }
        if copied {
            w.copied += 1;
        }
    }
}

/// The GPU's counts for a present (the render thread, a few frames later), then the blink check on the frames around.
pub fn counts(present: u64, tested: u32, hidden: u32) {
    let found = with(|r| {
        let i = (present % RING as u64) as usize;
        if r.entries[i].present != present {
            return Vec::new();
        }
        r.entries[i].tested = Some(tested);
        r.entries[i].hidden = Some(hidden);
        // the frame two before this one now has two neighbours each side
        let centre = present.saturating_sub(2);
        let series: Vec<(u64, Option<f32>)> = (centre.saturating_sub(2)..=centre + 2).map(|p| (p, share(&r.entries[(p % RING as u64) as usize], p))).collect();
        let mut found = Vec::new();
        for (p, kind) in detect(&series) {
            let e = &mut r.entries[(p % RING as u64) as usize];
            if e.present == p && e.blink == 0 {
                e.blink = kind;
                r.blinks += 1;
                if kind < 0 {
                    r.unhidden += 1;
                } else {
                    r.overhidden += 1;
                }
                r.last_blinks.push_back(json!({"present": p, "kind": if kind < 0 { "unhidden" } else { "overhidden" }, "depth": DEPTH_NAMES[(e.depth as usize).min(3)], "age": e.age, "cam_step": e.cam_step.name(), "board_step": e.board_step.name(), "share": share(e, p)}));
                if r.last_blinks.len() > 16 {
                    r.last_blinks.pop_front();
                }
                found.push((p, e.t_us));
            }
        }
        found
    });
    for (p, t) in found {
        auto_mark(p, t);
    }
}

/// Enough sampled pixels to tell a share.
const MIN_TESTED: u32 = 24;

fn share(e: &Entry, present: u64) -> Option<f32> {
    if e.present != present || !e.drawn {
        return None;
    }
    let (t, h) = (e.tested?, e.hidden?);
    (t >= MIN_TESTED).then(|| h as f32 / t as f32)
}

/// Blinks among five frames (present, hidden share), the middle one (or the middle two) against the outer ones: a
/// share that jumps away from both neighbours while they agree. Returns (present, -1 unhidden or +1 overhidden).
pub fn detect(series: &[(u64, Option<f32>)]) -> Vec<(u64, i8)> {
    let mut out = Vec::new();
    if series.len() != 5 {
        return out;
    }
    let s: Vec<Option<f32>> = series.iter().map(|x| x.1).collect();
    let jump = |v: f32, base: f32, spread: f32| {
        let dev = v - base;
        let big = dev.abs() >= 0.10f32.max(0.4 * base.max(v));
        (big && spread < 0.5 * dev.abs()).then_some(if dev < 0.0 { -1i8 } else { 1 })
    };
    // one frame: the middle against its neighbours
    if let (Some(a), Some(m), Some(b)) = (s[1], s[2], s[3]) {
        if let Some(k) = jump(m, (a + b) * 0.5, (a - b).abs()) {
            out.push((series[2].0, k));
            return out;
        }
    }
    // two frames: the middle and the next against the ones either side of them
    if let (Some(a), Some(m), Some(n), Some(b)) = (s[1], s[2], s[3], s[4]) {
        let (base, spread) = ((a + b) * 0.5, (a - b).abs());
        if let (Some(k1), Some(k2)) = (jump(m, base, spread), jump(n, base, spread)) {
            if k1 == k2 && (m - n).abs() < 0.5 * (m - base).abs().min((n - base).abs()) {
                out.push((series[2].0, k1));
                out.push((series[3].0, k2));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- saving

/// The key that saves the moments around it (a virtual-key code; 0: none). F8.
pub static KEY: AtomicU32 = AtomicU32::new(0x77);
static KEY_DOWN: AtomicBool = AtomicBool::new(false);
/// Save the moments around each blink found (one every 10 s at most).
pub static AUTO: AtomicBool = AtomicBool::new(true);
static LAST_AUTO_US: AtomicU64 = AtomicU64::new(0);
static MARKS_SAVED: AtomicU64 = AtomicU64::new(0);

struct Mark {
    kind: &'static str,
    present: u64,
    t_us: u64,
    before_us: u64,
    after_us: u64,
}

static MARKS: Mutex<Vec<Mark>> = Mutex::new(Vec::new());
static MARKS_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static SAVED: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

/// At each present (the render thread): the key, pressed now (not held from before), marks the moment.
pub fn poll_key(present: u64) {
    let key = KEY.load(Ordering::Relaxed);
    if key == 0 {
        return;
    }
    let down = unsafe { GetAsyncKeyState(key as i32) } as u16 & 0x8000 != 0;
    if down && !KEY_DOWN.swap(true, Ordering::AcqRel) {
        mark("key", present, now_us(), 3_000_000, 1_000_000);
    } else if !down {
        KEY_DOWN.store(false, Ordering::Release);
    }
}

fn auto_mark(present: u64, t_us: u64) {
    if !AUTO.load(Ordering::Relaxed) {
        return;
    }
    let last = LAST_AUTO_US.load(Ordering::Relaxed);
    if last != 0 && t_us < last + 10_000_000 {
        return;
    }
    LAST_AUTO_US.store(t_us.max(1), Ordering::Relaxed);
    mark("blink", present, t_us, 1_000_000, 500_000);
}

/// Saves the record from `before_us` before `t_us` to `after_us` after it, once that is past.
pub fn mark(kind: &'static str, present: u64, t_us: u64, before_us: u64, after_us: u64) {
    if let Ok(mut m) = MARKS.lock() {
        if m.len() < 32 {
            m.push(Mark { kind, present, t_us, before_us, after_us });
        }
    }
}

/// Where marks go: `%TEMP%\caveman-fx\marks` unless set.
pub fn set_marks_dir(dir: Option<PathBuf>) {
    *MARKS_DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

fn marks_dir() -> PathBuf {
    MARKS_DIR.lock().ok().and_then(|d| d.clone()).unwrap_or_else(|| std::env::temp_dir().join("caveman-fx").join("marks"))
}

/// From the worker's tick: writes the marks whose time is past.
pub fn service() {
    let now = now_us();
    let due: Vec<Mark> = match MARKS.lock() {
        Ok(mut m) => {
            let (due, keep): (Vec<Mark>, Vec<Mark>) = m.drain(..).partition(|x| now >= x.t_us + x.after_us + 100_000);
            *m = keep;
            due
        }
        Err(_) => return,
    };
    for m in due {
        let dir = marks_dir();
        let stamp = stamp();
        let path = dir.join(format!("{stamp}-{}-{}.json", m.kind, m.present));
        let result = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| {
            write(&path, Some((m.t_us.saturating_sub(m.before_us), m.t_us + m.after_us)), json!({"mark": m.kind, "present": m.present, "t_us": m.t_us}))
        });
        let line = match result {
            Ok(n) => {
                MARKS_SAVED.fetch_add(1, Ordering::Relaxed);
                format!("{} ({n} presents)", path.display())
            }
            Err(e) => format!("{}: {e}", path.display()),
        };
        crate::worker::log(&format!("diag: {} mark saved: {line}", m.kind));
        if let Ok(mut s) = SAVED.lock() {
            s.push_back(line);
            if s.len() > 8 {
                s.pop_front();
            }
        }
    }
}

/// Local date and time for file names (UTC: no time zone lookups here).
fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// The record (between two times, or all of it), oldest first, with the windows' pass ends, as JSON in a file.
/// Returns how many presents went in.
pub fn write(path: &std::path::Path, between: Option<(u64, u64)>, about: Value) -> Result<usize, String> {
    let (entries, windows, summary) = with(|r| {
        let mut e: Vec<Entry> = r.entries.iter().filter(|e| e.present > 0).copied().collect();
        e.sort_by_key(|x| x.present);
        if let Some((a, b)) = between {
            e.retain(|x| x.t_us >= a && x.t_us <= b);
        }
        let first = e.first().map_or(0, |x| x.present.saturating_sub(2));
        let last = e.last().map_or(0, |x| x.present);
        let mut w: Vec<WindowStat> = r.windows.iter().filter(|w| w.window >= first && w.window <= last && w.ends > 0).copied().collect();
        w.sort_by_key(|x| x.window);
        (e, w, json!({"blinks": r.blinks, "unhidden": r.unhidden, "overhidden": r.overhidden, "last_blinks": r.last_blinks}))
    });
    let doc = json!({
        "about": about,
        "summary": summary,
        "status": crate::overlay::status(),
        "presents": entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
        "windows": windows.iter().map(|w| json!({"window": w.window, "ends": w.ends, "gbuffer": w.gbuffer, "copied": w.copied, "first_gbuffer_us": w.first_gbuffer_us})).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    Ok(entries.len())
}

/// `draw log dump`: the whole record into `path` (default: a new file in the marks folder).
pub fn dump(path: Option<PathBuf>) -> Result<(PathBuf, usize), String> {
    let path = match path {
        Some(p) => p,
        None => {
            let dir = marks_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            dir.join(format!("{}-dump.json", stamp()))
        }
    };
    let n = write(&path, None, json!({"dump": true}))?;
    Ok((path, n))
}

/// The status's part.
pub fn status() -> Value {
    let (blinks, unhidden, overhidden, last, counted) = with(|r| {
        (r.blinks, r.unhidden, r.overhidden, r.last_blinks.iter().cloned().collect::<Vec<_>>(), r.entries.iter().filter(|e| e.tested.is_some()).count())
    });
    let key = KEY.load(Ordering::Relaxed);
    json!({
        "blinks": blinks,
        "unhidden": unhidden,
        "overhidden": overhidden,
        "last_blinks": last,
        "presents_counted": counted,
        "key": if key == 0 { "off".to_string() } else { format!("{key:#04x}") },
        "auto_save": AUTO.load(Ordering::Relaxed),
        "marks_dir": marks_dir().display().to_string(),
        "marks_saved": MARKS_SAVED.load(Ordering::Relaxed),
        "saved": SAVED.lock().map(|s| s.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
    })
}

/// Forgets the record (a test host between checks).
pub fn clear() {
    *RECORD.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(v: [Option<f32>; 5]) -> Vec<(u64, Option<f32>)> {
        (0..5).map(|i| (100 + i as u64, v[i])).collect()
    }

    #[test]
    fn finds_blinks_and_not_motion() {
        // a leg moving in front: no blink
        assert!(detect(&series([Some(0.1), Some(0.2), Some(0.3), Some(0.4), Some(0.5)])).is_empty());
        assert!(detect(&series([Some(0.3), Some(0.31), Some(0.3), Some(0.32), Some(0.3)])).is_empty());
        // one frame the depth hides nothing
        assert_eq!(detect(&series([Some(0.3), Some(0.31), Some(0.02), Some(0.3), Some(0.3)])), vec![(102, -1)]);
        // one frame it hides everything
        assert_eq!(detect(&series([Some(0.3), Some(0.3), Some(0.95), Some(0.29), Some(0.3)])), vec![(102, 1)]);
        // two frames
        assert_eq!(detect(&series([Some(0.3), Some(0.3), Some(0.05), Some(0.06), Some(0.31)])), vec![(102, -1), (103, -1)]);
        // a cut (it stays changed): no blink
        assert!(detect(&series([Some(0.3), Some(0.3), Some(0.05), Some(0.05), Some(0.05)])).is_empty());
        // too few pixels to tell
        assert!(detect(&series([Some(0.3), None, Some(0.0), Some(0.3), Some(0.3)])).is_empty());
        // small shares: a big relative jump of a few pixels' worth is not enough
        assert!(detect(&series([Some(0.02), Some(0.02), Some(0.06), Some(0.02), Some(0.02)])).is_empty());
    }

    #[test]
    fn records_counts_and_marks_blinks() {
        clear();
        for p in 1..=12u64 {
            record(Entry { present: p, t_us: p * 10_000, drawn: true, depth: 1, ..Default::default() });
            let hidden = if p == 7 { 0 } else { 60 };
            counts(p, 200, hidden);
        }
        let s = status();
        assert_eq!(s["blinks"], 1, "{s}");
        assert_eq!(s["last_blinks"][0]["present"], 7);
        assert_eq!(s["last_blinks"][0]["kind"], "unhidden");
        window_end(11, true, true);
        window_end(11, false, false);
        let dir = std::env::temp_dir().join(format!("caveman-fx-diag-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dump.json");
        let n = write(&path, Some((50_000, 90_000)), json!({})).unwrap();
        assert_eq!(n, 5);
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["presents"][2]["blink"], "unhidden");
        assert_eq!(doc["presents"][2]["present"], 7);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(stamp().ends_with('Z') && stamp().len() == 16, "{}", stamp());
        clear();
    }
}
