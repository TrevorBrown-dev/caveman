//! The Live panel's back end: The Caveman in the running game, through `caveman-trainer.exe` (the pipe to the FX
//! DLL). Short commands run to completion and return their output (`run`, bounded by `RUN_TIMEOUT`); the panel's poll
//! is `status`, a compact summary of the DLL's state (the trainer's `board status --json` is about 9 MB, so only the
//! few fields the panel shows are passed on); the video producer is a child process kept until it is stopped, in a job
//! object with the ffmpeg it starts, its progress lines sent to the window as events.
//!
//! `CAVEMAN_LIVE_DRY=1` in the app's environment is a dry run: the read-only trainer commands run, every other one is
//! only reported (its command line), and no video producer starts. For trying the panel without touching the game.

use std::ffi::c_void;
use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::config::Config;

/// No console window for the trainer (the app has none).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The manual-reset event the producer watches (caveman-fx `video::STOP_EVENT_NAME`).
const STOP_EVENT: &str = "Local\\caveman-fx-video-stop";
const EVENT_MODIFY_STATE: u32 = 0x0002;
const WAIT_OBJECT_0: u32 = 0;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
/// A short command still running after this is killed. The slow ones take seconds: a finish encodes three BC7 chains,
/// `pip on` reads the meshes from the install, a picture waits up to 15 s for the game to take it.
const RUN_TIMEOUT: Duration = Duration::from_secs(90);
/// How long a stopped producer gets to end by itself before its job is ended.
const STOP_GRACE: Duration = Duration::from_secs(3);

type Handle = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn WaitForSingleObject(handle: Handle, ms: u32) -> u32;
    fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn TerminateJobObject(job: Handle, code: u32) -> i32;
}

/// JOBOBJECT_EXTENDED_LIMIT_INFORMATION (x64 layout; `repr(C)` gives the same padding).
#[repr(C)]
#[derive(Default)]
struct JobLimits {
    per_process_user_time: i64,
    per_job_user_time: i64,
    limit_flags: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
    io: [u64; 6],
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

/// A job object that ends every process in it when it is closed: the producer and the ffmpeg it starts, so neither
/// outlives a stop (killing the trainer alone would leave ffmpeg running).
struct Job(Handle);

// SAFETY: a job handle is a kernel handle, usable from any thread
unsafe impl Send for Job {}

impl Job {
    fn new() -> Option<Job> {
        // SAFETY: plain Win32 calls; the limits struct outlives the call
        unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let limits = JobLimits { limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, ..Default::default() };
            SetInformationJobObject(
                h,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &limits as *const JobLimits as *const c_void,
                std::mem::size_of::<JobLimits>() as u32,
            );
            Some(Job(h))
        }
    }

    fn assign(&self, child: &Child) -> bool {
        // SAFETY: both handles are live
        unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as Handle) != 0 }
    }

    fn terminate(&self) {
        // SAFETY: a live job handle
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is ours; closing it ends what is left in the job (KILL_ON_JOB_CLOSE)
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// The running video producer.
struct Producer {
    child: Child,
    job: Option<Job>,
}

#[derive(Default)]
pub struct LiveState {
    video: Mutex<Option<Producer>>,
    /// The board cam window's pose stream (`pose`), while it runs.
    pose: Mutex<Option<Child>>,
}

/// Starts `pose --rate N` (the camera and the board read from outside the game, one JSON line per tick) and relays
/// each line as a `board-pose` event (the parsed JSON; `{"done": true}` when the stream ends, `{"error": true, "why"}`
/// for a line on stderr). One stream at a time; returns the command line.
pub fn pose_start(app: &AppHandle, trainer: &Path, state: &LiveState, rate: u32) -> Result<String, String> {
    use std::io::BufRead;
    // with the rig's 16 bones each tick (the window moves the trucks and wheels by them)
    let args = vec!["pose".to_string(), "--rate".to_string(), rate.clamp(1, 240).to_string(), "--bones".to_string()];
    let command = command_line(&args);
    if dry() {
        return Ok(format!("dry run: not run: {command}"));
    }
    let mut slot = state.pose.lock().unwrap();
    if let Some(child) = slot.as_mut() {
        if child.try_wait().map(|s| s.is_none()).unwrap_or(false) {
            return Ok(command);
        }
    }
    let mut child = Command::new(trainer)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{}: {e}", trainer.display()))?;
    if let Some(out) = child.stdout.take() {
        let app = app.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(|l| l.ok()) {
                if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                    let _ = app.emit("board-pose", v);
                }
            }
            let _ = app.emit("board-pose", json!({"done": true}));
        });
    }
    if let Some(err) = child.stderr.take() {
        let app = app.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(err).lines().map_while(|l| l.ok()) {
                let _ = app.emit("board-pose", json!({"error": true, "why": line}));
            }
        });
    }
    *slot = Some(child);
    Ok(command)
}

/// Ends the pose stream (nothing when none runs).
pub fn pose_stop(state: &LiveState) {
    if let Some(mut child) = state.pose.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// `caveman-trainer attach --ring N|off --timeout S`, detached: waits for the game (up to `timeout_s`), puts the FX
/// DLL in unless it already answers, and switches the staging ring on (`ring_mb` 0: off), as Caveman.exe does at
/// launch (it logs to `%LOCALAPPDATA%\Caveman\fx\attach.log`). Returns the command line when it was started.
pub fn attach_now(trainer: &Path, game: &Path, ring_mb: u64, timeout_s: u64) -> Option<String> {
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let args = vec![
        "attach".to_string(),
        "--ring".to_string(),
        if ring_mb == 0 { "off".to_string() } else { ring_mb.to_string() },
        "--timeout".to_string(),
        timeout_s.to_string(),
    ];
    if dry() {
        return Some(format!("dry run: not run: {}", command_line(&args)));
    }
    Command::new(trainer)
        .args(&args)
        .current_dir(game)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .ok()?;
    Some(command_line(&args))
}

/// What a trainer command returned: its exit, everything it printed (empty when it printed JSON: that is in `json`),
/// the command line as it would be typed, and whether it was only reported (a dry run).
#[derive(Serialize, Clone, Debug)]
pub struct LiveOut {
    pub ok: bool,
    pub code: Option<i32>,
    pub text: String,
    pub json: Option<Value>,
    pub command: String,
    pub dry: bool,
}

/// One of The Caveman's effects, as `caveman fx` builds them (fx.cave.<name>).
#[derive(Serialize, Clone, Debug)]
pub struct CaveFxInfo {
    pub name: &'static str,
    pub what: &'static str,
    pub one_shot: bool,
    pub cooldown: f64,
}

pub fn cave_effects() -> Vec<CaveFxInfo> {
    caveman_core::fxmod::CAVE
        .iter()
        .map(|f| CaveFxInfo { name: f.name, what: f.what, one_shot: matches!(f.usage, caveman_core::effects::Usage::OneOff), cooldown: f.cooldown })
        .collect()
}

/// A dry run (`CAVEMAN_LIVE_DRY` set and not 0): only the read-only commands run.
pub fn dry() -> bool {
    std::env::var("CAVEMAN_LIVE_DRY").is_ok_and(|v| !v.trim().is_empty() && v.trim() != "0")
}

/// The trainer commands that only read: the states, the board's status, its parameters, the effects list, the video
/// and overlay states, and `inject` without `--yes` (its checks). Everything else changes the board or the game.
pub fn read_only(args: &[String]) -> bool {
    let lower: Vec<String> = args.iter().map(|a| a.to_ascii_lowercase()).collect();
    let words: Vec<&str> = lower.iter().map(String::as_str).filter(|a| !a.starts_with("--")).collect();
    match words.as_slice() {
        ["status", ..] => true,
        ["inject", ..] => !lower.iter().any(|a| a == "--yes"),
        ["board"] | ["board", "status", ..] | ["board", "params", ..] | ["board", "fx", "list", ..] | ["board", "video", "status", ..] => true,
        ["pip"] | ["pip", "status", ..] => true,
        ["bars"] | ["bars", "status", ..] => true,
        _ => false,
    }
}

/// The trainer: the configured path, else beside the app's exe, else (for `pnpm tauri dev`, whose exe is in
/// `target\debug`) the release build's.
pub fn trainer_exe(config: &Config) -> Result<PathBuf, String> {
    let t = config.trainer.trim().trim_matches('"');
    if !t.is_empty() {
        let p = PathBuf::from(t);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("The trainer at {t} is not there (Settings, Advanced)."));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no exe folder")?;
    let mut look = vec![dir.join("caveman-trainer.exe")];
    if let Some(target) = dir.parent() {
        look.push(target.join("release").join("caveman-trainer.exe"));
    }
    look.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        format!(
            "caveman-trainer.exe is not beside the app ({}): build it (cargo build --release -p caveman-trainer) or set its path in Settings, Advanced.",
            dir.display()
        )
    })
}

/// The command line as it would be typed in a terminal (for the panel's log).
pub fn command_line(args: &[String]) -> String {
    let quoted =
        args.iter().map(|a| if a.is_empty() || a.contains(|c: char| " \"&()".contains(c)) { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.clone() });
    std::iter::once("caveman-trainer".to_string()).chain(quoted).collect::<Vec<_>>().join(" ")
}

/// `args` with `--game DIR` added for the commands that read the install (a picture's natural wood, a finish's
/// default layers, the board cam's meshes), when the app's game folder is not the trainer's default.
pub fn with_game(args: &[String], game: &Path) -> Vec<String> {
    let mut out = args.to_vec();
    let first = args.first().map(|a| a.to_ascii_lowercase());
    let second = args.get(1).map(|a| a.to_ascii_lowercase());
    let reads_install = matches!((first.as_deref(), second.as_deref()), (Some("board"), Some("art" | "deck" | "finish")) | (Some("pip"), Some("on" | "load")));
    let text = game.display().to_string();
    let default = text.trim().trim_end_matches('\\').eq_ignore_ascii_case(caveman_cli::DEFAULT_GAME.trim_end_matches('\\'));
    if reads_install && !text.trim().is_empty() && !default && !args.iter().any(|a| a == "--game") {
        out.push("--game".into());
        out.push(text);
    }
    out
}

fn drain(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut s) = stream {
            let _ = s.read_to_end(&mut buf);
        }
        buf
    })
}

/// Runs the trainer with `args` to completion (killed after `RUN_TIMEOUT`). In a dry run a command that would change
/// something is only reported.
pub fn run(trainer: &Path, args: &[String]) -> Result<LiveOut, String> {
    let command = command_line(args);
    if dry() && !read_only(args) {
        return Ok(LiveOut { ok: true, code: Some(0), text: "dry run: not run (CAVEMAN_LIVE_DRY)".into(), json: None, command, dry: true });
    }
    let mut child = Command::new(trainer)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{}: {e}", trainer.display()))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    // SAFETY: the child's process handle is live until `child` drops
    let waited = unsafe { WaitForSingleObject(child.as_raw_handle() as Handle, RUN_TIMEOUT.as_millis() as u32) };
    let timed_out = waited != WAIT_OBJECT_0;
    if timed_out {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let mut text = String::from_utf8_lossy(&stdout).replace("\r\n", "\n").replace('\r', "\n");
    let err = String::from_utf8_lossy(&stderr);
    if !err.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(err.trim());
    }
    if timed_out {
        text.push_str(&format!("\nno answer within {} s: stopped", RUN_TIMEOUT.as_secs()));
    }
    let json = if text.trim_start().starts_with('{') { serde_json::from_str::<Value>(text.trim()).ok() } else { None };
    let text = if json.is_some() { String::new() } else { text.trim().to_string() };
    Ok(LiveOut { ok: status.success() && !timed_out, code: status.code(), text, json, command, dry: false })
}

fn last_line(text: &str) -> String {
    text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("").to_string()
}

/// The panel's poll: the DLL's state in a few fields. `with_board` adds The Caveman's (found, equipped, the picture on
/// it), read from `board status --json`: about 9 MB that the DLL builds under the locks its render-side hooks take, so
/// the panel asks for it only at open, after a change and every 30 s.
pub fn status(trainer: &Path, with_board: bool) -> Value {
    let base = json!({ "trainer": trainer.display().to_string(), "dry": dry() });
    let s = match run(trainer, &["status".into(), "--json".into()]) {
        Ok(s) => s,
        Err(e) => return merge(base, json!({ "game": crate::bake::game_running(), "injected": false, "error": e })),
    };
    let Some(v) = s.json.filter(|_| s.ok) else {
        return merge(base, json!({ "game": crate::bake::game_running(), "injected": false, "error": last_line(&s.text) }));
    };
    let fx: Vec<Value> = v["fx"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["name"].as_str().is_some_and(|n| n.starts_with("fx.cave.")))
        .map(|f| json!({ "name": f["name"], "value": f["value"], "override_ms": f["override_ms"] }))
        .collect();
    let mut out = merge(
        base,
        json!({
            "game": true,
            "injected": true,
            "error": "",
            "dll": { "pid": v["pid"], "phase": v["phase"], "reason": v["reason"], "version": v["version"], "build": v["build"], "hooked": v["hooked"] },
            "fx": fx,
            "bars": v["bars"],
            "audio": v["audio"],
            // the board module in brief (board::brief: found, uploads, pending, error, and from the DLL of
            // 2026-10-30 on `art`, the label of the picture on the board); null while it is not active
            "brief": v["board"],
        }),
    );
    if with_board {
        match run(trainer, &["board".into(), "status".into(), "--json".into()]) {
            Ok(b) => match b.json.filter(|_| b.ok) {
                Some(j) => {
                    let bd = &j["board"];
                    let log = bd["log"].as_array().and_then(|l| l.last()).and_then(Value::as_str).unwrap_or("").to_string();
                    out["board"] = json!({
                        "found": bd["found"],
                        "usable": bd["usable"],
                        "not_usable": bd["not_usable"],
                        "equipped": bd["equipped"],
                        "active": bd["active"],
                        "finding": bd["finding"],
                        "pending": !bd["pending"].is_null(),
                        "art": bd["art"]["label"],
                        "ridden": bd["ridden"]["found"],
                        "last_error": bd["last_error"],
                        "log": log,
                    });
                }
                None => out["board_error"] = json!(last_line(&b.text)),
            },
            Err(e) => out["board_error"] = json!(e),
        }
    }
    out
}

/// The DLL's label for the picture on the board, from the light `status` (DLLs from 2026-10-30 on carry it in
/// `board.art`; null there is the plain deck): `Ok(Some(label))`; `Ok(None)` when this DLL does not say (an older
/// one, or its board state busy: then `board_label_full`); Err when the trainer or the DLL did not answer.
pub fn board_label(trainer: &Path) -> Result<Option<Option<String>>, String> {
    let s = run(trainer, &["status".into(), "--json".into()])?;
    let v = s.json.filter(|_| s.ok).ok_or_else(|| last_line(&s.text))?;
    let board = &v["board"];
    if board.is_null() {
        return Ok(Some(None));
    }
    Ok(match board.get("art") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(a) => Some(a.as_str().filter(|l| !l.is_empty()).map(str::to_string)),
    })
}

/// The same from `board status --json`: the whole board state (about 9 MB the DLL builds under its locks), so asked
/// rarely.
pub fn board_label_full(trainer: &Path) -> Result<Option<String>, String> {
    let b = run(trainer, &["board".into(), "status".into(), "--json".into()])?;
    let j = b.json.filter(|_| b.ok).ok_or_else(|| last_line(&b.text))?;
    Ok(j["board"]["art"]["label"].as_str().filter(|l| !l.is_empty()).map(str::to_string))
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(a), Value::Object(b)) = (a.as_object_mut(), b) {
        a.extend(b);
    }
    a
}

fn emit_lines(app: AppHandle, mut stream: impl Read + Send + 'static, kind: &'static str, done_on_end: bool) {
    std::thread::spawn(move || {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &byte in &chunk[..n] {
                        if byte == b'\r' || byte == b'\n' {
                            let line = String::from_utf8_lossy(&buf).trim().to_string();
                            if !line.is_empty() {
                                let _ = app.emit("live-video", json!({"line": line, "stream": kind}));
                            }
                            buf.clear();
                        } else {
                            buf.push(byte);
                        }
                    }
                }
            }
        }
        let line = String::from_utf8_lossy(&buf).trim().to_string();
        if !line.is_empty() {
            let _ = app.emit("live-video", json!({"line": line, "stream": kind}));
        }
        if done_on_end {
            let _ = app.emit("live-video", json!({"done": true}));
        }
    });
}

/// Starts the producer (`board video ARGS...`) as a child kept in `state`; its lines come as `live-video` events
/// (`{line, stream}`), and `{done: true}` when it ends. Returns the command line.
pub fn video_start(app: &AppHandle, trainer: &Path, args: &[String], state: &LiveState) -> Result<String, String> {
    let mut slot = state.video.lock().map_err(|_| "the video state is poisoned")?;
    if let Some(p) = slot.as_mut() {
        if p.child.try_wait().map(|s| s.is_none()).unwrap_or(false) {
            return Err("A video is already playing: stop it first.".into());
        }
    }
    let mut full: Vec<String> = vec!["board".into(), "video".into()];
    full.extend(args.iter().cloned());
    let command = command_line(&full);
    if dry() {
        let _ = app.emit("live-video", json!({"line": format!("dry run: not run: {command}"), "stream": "out"}));
        let _ = app.emit("live-video", json!({"done": true}));
        return Ok(command);
    }
    let mut child = Command::new(trainer)
        .args(&full)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{}: {e}", trainer.display()))?;
    // the job first, before the trainer gets to start ffmpeg (it probes the source and opens the ring before that)
    let job = Job::new().filter(|j| j.assign(&child));
    if let Some(out) = child.stdout.take() {
        emit_lines(app.clone(), out, "out", true);
    }
    if let Some(err) = child.stderr.take() {
        emit_lines(app.clone(), err, "err", false);
    }
    *slot = Some(Producer { child, job });
    Ok(command)
}

/// Takes the running producer out of `state` (None: none running).
fn video_take(state: &LiveState) -> Option<Producer> {
    state.video.lock().ok().and_then(|mut s| s.take())
}

/// Stops the producer: the stop event first (it ends cleanly and tells the DLL), the whole job (the trainer and its
/// ffmpeg) after `STOP_GRACE`; then the DLL's side off (the kept art returns). What the trainer said.
pub fn video_stop(trainer: &Path, state: &LiveState) -> Result<String, String> {
    if dry() {
        return Ok("dry run: not run: the stop event, then caveman-trainer board video off".into());
    }
    let producer = video_take(state);
    let name: Vec<u16> = STOP_EVENT.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: plain Win32 calls with a NUL-terminated name
    unsafe {
        let ev = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if !ev.is_null() {
            SetEvent(ev);
            CloseHandle(ev);
        }
    }
    let mut said = Vec::new();
    if let Some(Producer { mut child, job }) = producer {
        let t0 = Instant::now();
        let mut ended = false;
        while t0.elapsed() < STOP_GRACE {
            if child.try_wait().map(|s| s.is_some()).unwrap_or(true) {
                ended = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !ended {
            said.push(format!("the producer did not stop within {} s: ended", STOP_GRACE.as_secs()));
            match &job {
                Some(j) => j.terminate(),
                None => {
                    let _ = child.kill();
                }
            }
        }
        let _ = child.wait();
        drop(job);
    }
    let off = run(trainer, &["board".into(), "video".into(), "off".into()])?;
    said.push(if off.text.is_empty() { "board video off".into() } else { off.text });
    Ok(said.join("\n"))
}

pub fn video_running(state: &LiveState) -> bool {
    state.video.lock().ok().is_some_and(|mut s| s.as_mut().is_some_and(|p| p.child.try_wait().map(|x| x.is_none()).unwrap_or(false)))
}

/// The picture types the panel takes; PNGs go to the trainer as they are, the others are made PNGs first (the
/// trainer reads PNG only).
pub const PICTURES: [&str; 6] = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];

/// A picture's bytes (for the webview to make a PNG of a JPG and the like).
pub fn read_picture(path: &Path) -> Result<Vec<u8>, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !PICTURES.contains(&ext.as_str()) {
        return Err(format!("not a picture (PNG, JPG, WebP, BMP or GIF): {}", path.display()));
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A PNG made in the webview (a picked file, or a JPG made PNG) written to `dir` as `<name>.png` (the name the board's
/// status shows); returns its path. Only PNG bytes are taken.
pub fn stage_png(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    if !bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Err("not PNG bytes".into());
    }
    let clean: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || " ._-()".contains(c) { c } else { '_' }).collect();
    let stem = clean.trim().trim_end_matches(".png").trim_matches('.').trim();
    let stem = if stem.is_empty() { "picture" } else { stem };
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{stem}.png"));
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn read_only_commands() {
        for ro in [
            "status --json",
            "board",
            "board status --json",
            "board --json",
            "board params --json",
            "board fx list",
            "board video status",
            "pip",
            "pip status",
            "bars",
            "bars status",
            "inject",
        ] {
            assert!(read_only(&words(ro)), "{ro} reads only");
        }
        for w in [
            "inject --yes",
            "unload",
            "board deck x.png",
            "board art x.png",
            "board reset",
            "board finish holo",
            "board foil pearl",
            "board holotint ff0000",
            "board tint neutral",
            "board param iridescence 1",
            "board fx aura on",
            "board video obs",
            "board video off",
            "board video stop",
            "pip on",
            "pip size 0.3",
            "bars on",
            "bars set window 30",
            "bars source audio",
            "rules load x",
        ] {
            assert!(!read_only(&words(w)), "{w} writes");
        }
    }

    #[test]
    fn command_lines_quote_spaces() {
        assert_eq!(command_line(&words("board fx aura on")), "caveman-trainer board fx aura on");
        assert_eq!(command_line(&["board".into(), "art".into(), r"C:\My Pictures\a.png".into()]), r#"caveman-trainer board art "C:\My Pictures\a.png""#);
    }

    #[test]
    fn game_folder_only_when_not_the_default() {
        let a = words("board art x.png");
        assert_eq!(with_game(&a, Path::new(caveman_cli::DEFAULT_GAME)), a);
        assert_eq!(with_game(&a, Path::new(r"D:\ReSkate")), words(r"board art x.png --game D:\ReSkate"));
        assert_eq!(with_game(&words("board tint neutral"), Path::new(r"D:\ReSkate")), words("board tint neutral"));
        assert_eq!(with_game(&words("pip on"), Path::new(r"D:\ReSkate")), words(r"pip on --game D:\ReSkate"));
    }

    /// JOBOBJECT_BASIC_ACCOUNTING_INFORMATION
    #[repr(C)]
    #[derive(Default)]
    struct Accounting {
        times: [i64; 4],
        page_faults: u32,
        total_processes: u32,
        active_processes: u32,
        terminated_processes: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn QueryInformationJobObject(job: Handle, class: i32, info: *mut c_void, len: u32, returned: *mut u32) -> i32;
    }

    fn processes(job: &Job) -> (u32, u32) {
        let mut a = Accounting::default();
        // SAFETY: a live job handle and a buffer of the class's size
        let ok = unsafe {
            QueryInformationJobObject(job.0, 1, &mut a as *mut Accounting as *mut c_void, std::mem::size_of::<Accounting>() as u32, std::ptr::null_mut())
        };
        assert_ne!(ok, 0, "QueryInformationJobObject");
        (a.total_processes, a.active_processes)
    }

    /// A stop that has to end the producer ends what it started too: `cmd` stands in for the trainer, the `ping` it
    /// starts for ffmpeg.
    #[test]
    fn ending_a_job_ends_the_children_too() {
        for by_close in [false, true] {
            let mut child =
                Command::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >nul"]).stdin(Stdio::null()).creation_flags(CREATE_NO_WINDOW).spawn().unwrap();
            let job = Job::new().unwrap();
            assert!(job.assign(&child));
            let t = Instant::now();
            while processes(&job).1 < 2 && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(50));
            }
            // cmd, the ping it started, and the console host Windows starts for cmd
            assert!(processes(&job).1 >= 2, "cmd and the ping it started are in the job: {:?}", processes(&job));
            if by_close {
                // closing the job ends them (exit code 0): cmd would otherwise wait 30 s for its ping
                let t = Instant::now();
                drop(job);
                child.wait().unwrap();
                assert!(t.elapsed() < Duration::from_secs(5), "closing the job ended cmd");
            } else {
                job.terminate();
                assert!(!child.wait().unwrap().success());
                let t = Instant::now();
                while processes(&job).1 > 0 && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                assert_eq!(processes(&job).1, 0, "the ping ended with cmd");
            }
        }
    }

    #[test]
    fn staged_names_are_safe() {
        let dir = std::env::temp_dir().join(format!("caveman-live-stage-{}", std::process::id()));
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0];
        let p = stage_png(&dir, "a/b:c*.png", &png).unwrap();
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "a_b_c_.png");
        assert!(stage_png(&dir, "x", b"GIF89a").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
