//! The allocation trace: for a window of seconds (`caveman-trainer mem trace`), every GPU allocation the game makes
//! through the device (the memory guard's hooks: committed and placed resources, heaps), with its size, heap, format,
//! the time the call took, the thread and the call stack; for each one that is a resource, when the game released
//! it (its lifetime) and, for a staging buffer, how much of it the game copied out of (the copy hooks' footprints)
//! and how many of its pages were ever touched. Beside that, the video memory watch sampled every 50 ms. The report
//! groups the allocations by the game's call site, so a burst (the skater build's staging, 2026-10-23: 8 to 16 GB
//! of upload buffers inside a second) shows who makes it, how big each piece is, and whether the pieces are freed
//! at once or held.
//!
//! The release and map hooks sit on the shared vtable of D3D12Core's resource class (learnt from a dummy buffer at
//! install): a resource that is not tracked passes straight through.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use windows::core::{Interface, HRESULT};
use windows::Win32::Graphics::Direct3D12::*;

use crate::memguard::{now_ms, AllocInfo};
use crate::vhook::VSlot;
use crate::win;

#[link(name = "kernel32")]
extern "system" {
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, out: *mut *mut c_void, hash: *mut u32) -> u16;
    fn K32QueryWorkingSetEx(process: win::HANDLE, info: *mut c_void, cb: u32) -> i32;
}

const FRAMES: usize = 12;
const CAP: usize = 120_000;
const BUCKET_MS: u64 = 250;

type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
type MapFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_RANGE, *mut *mut c_void) -> HRESULT;

#[derive(Clone)]
pub struct Entry {
    pub t_ms: u64,
    pub thread: u32,
    pub what: &'static str,
    pub info: AllocInfo,
    pub ms: f32,
    pub hr: i32,
    pub stack: [usize; FRAMES],
    pub frames: u8,
    pub raw: usize,
    /// When the game's last reference went (0: still held at the report).
    pub freed_ms: u64,
    pub mapped: usize,
    /// Bytes of its pages resident when it was freed (-1: not measured).
    pub touched: i64,
    /// The furthest byte copied out of it (buffers: the copy hooks' placed footprints) and how many copies read it.
    pub copied_to: u64,
    pub copies: u32,
    /// When its first copy was recorded (0: none yet).
    pub copy_ms: u64,
    /// Where its first copy went: the destination resource, its subresource, the x, y, z, and what that resource is
    /// (kind, width, height, depth, mips, format; from the trace when it made it, else asked of it).
    pub dst: usize,
    pub dst_sub: u32,
    pub dst_at: [u32; 3],
    pub dst_desc: (u8, u64, u32, u16, u16, u32),
}

#[derive(Default)]
struct Trace {
    until_ms: u64,
    min_bytes: u64,
    started_ms: u64,
    entries: Vec<Entry>,
    live: HashMap<usize, usize>,
    /// (ms, video memory used, system memory for the GPU used)
    samples: Vec<(u64, u64, u64)>,
    dropped: u64,
    sampler: bool,
    /// The file the trace is appended to every two seconds (so a crash keeps what came before it), what was
    /// written so far, and the first copies and releases not yet written.
    path: String,
    flushed: usize,
    flushed_samples: usize,
    freed_since: Vec<usize>,
    copies_since: Vec<usize>,
}

/// The trace's file and the module-name cache, written by our threads outside the trace lock (`write_pending`).
#[derive(Default)]
struct Writer {
    file: Option<std::fs::File>,
    names: HashMap<usize, String>,
}

static TRACING: AtomicBool = AtomicBool::new(false);
/// Set at unload: the sampler thread ends at once (a thread of ours asleep in the DLL's code when it is unmapped
/// crashes the process; seen in the host test 2026-10-23).
static STOP: AtomicBool = AtomicBool::new(false);
static LIVE_N: AtomicU64 = AtomicU64::new(0);
static TRACE: LazyLock<Mutex<Trace>> = LazyLock::new(|| Mutex::new(Trace::default()));
static WRITER: LazyLock<Mutex<Writer>> = LazyLock::new(|| Mutex::new(Writer::default()));
/// Records and releases lost because a game thread found the trace lock taken (it never waits for it).
static DROPPED_BUSY: AtomicU64 = AtomicU64::new(0);
static RELEASE: VSlot = VSlot::new();
static MAP: VSlot = VSlot::new();
/// The resource class's own AddRef (slot 1), for the release hook's extra reference.
static ADDREF: AtomicUsize = AtomicUsize::new(0);

pub fn tracing() -> bool {
    TRACING.load(Ordering::Relaxed)
}

/// Whether any traced resource is still held (the copy hooks ask before looking up a source).
pub fn tracking() -> bool {
    LIVE_N.load(Ordering::Relaxed) > 0
}

/// From the memory guard's hooks: one allocation made (or refused: `hr`).
///
/// # Safety
///
/// `out` is the hooked call's own result slot: null, or a pointer the call wrote when `hr` is ok; it is read only then.
pub unsafe fn record(what: &'static str, info: &AllocInfo, ms: f32, hr: HRESULT, out: *mut *mut c_void) {
    let now = now_ms();
    let mut stack = [0usize; FRAMES];
    // SAFETY: a plain Win32 call writing at most FRAMES pointers
    let frames = unsafe { RtlCaptureStackBackTrace(1, FRAMES as u32, stack.as_mut_ptr().cast(), std::ptr::null_mut()) } as u8;
    // SAFETY: `out` is the call's result pointer, valid when the call succeeded
    let raw = if hr.is_ok() && !out.is_null() { (unsafe { *out }) as usize } else { 0 };
    // never a wait on a game thread: a record lost to a busy lock is counted instead
    let Ok(mut t) = TRACE.try_lock() else {
        DROPPED_BUSY.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if now > t.until_ms {
        TRACING.store(false, Ordering::Relaxed);
        return;
    }
    if info.bytes < t.min_bytes {
        return;
    }
    if t.entries.len() >= CAP {
        t.dropped += 1;
        return;
    }
    let thread = unsafe { win::GetCurrentThreadId() };
    t.entries.push(Entry {
        t_ms: now,
        thread,
        what,
        info: *info,
        ms,
        hr: hr.0,
        stack,
        frames,
        raw,
        freed_ms: 0,
        mapped: 0,
        touched: -1,
        copied_to: 0,
        copies: 0,
        copy_ms: 0,
        dst: 0,
        dst_sub: 0,
        dst_at: [0; 3],
        dst_desc: (0, 0, 0, 0, 0, 0),
    });
    if raw != 0 {
        let i = t.entries.len() - 1;
        t.live.insert(raw, i);
        LIVE_N.fetch_add(1, Ordering::Relaxed);
    }
}

/// From the copy hooks: a copy read `bytes` of `res` from `offset` (a placed footprint source) into `dst`'s
/// subresource `dst_sub` at `at`.
///
/// # Safety
/// `dst` is the destination resource of a copy being recorded now: alive.
pub unsafe fn note_copy(res: usize, offset: u64, bytes: u64, dst: usize, dst_sub: u32, at: [u32; 3]) {
    let Ok(mut t) = TRACE.try_lock() else { return };
    let Some(&i) = t.live.get(&res) else { return };
    let first = t.entries[i].copies == 0;
    let desc = if first {
        match t.live.get(&dst) {
            Some(&j) => {
                let d = &t.entries[j].info;
                (d.kind, d.w, d.h, d.d, d.mips, d.format)
            }
            None => {
                let raw = dst as *mut c_void;
                match ID3D12Resource::from_raw_borrowed(&raw) {
                    Some(r) => {
                        let d = r.GetDesc();
                        let kind = if d.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER { 0 } else { d.Dimension.0 as u8 - 1 };
                        (kind, d.Width, d.Height, d.DepthOrArraySize, d.MipLevels, d.Format.0 as u32)
                    }
                    None => (0, 0, 0, 0, 0, 0),
                }
            }
        }
    } else {
        (0, 0, 0, 0, 0, 0)
    };
    let e = &mut t.entries[i];
    e.copied_to = e.copied_to.max(offset + bytes);
    e.copies += 1;
    if first {
        e.copy_ms = now_ms();
        e.dst = dst;
        e.dst_sub = dst_sub;
        e.dst_at = at;
        e.dst_desc = desc;
    }
    if first {
        t.copies_since.push(i);
    }
}

/// Bytes of the pages of `[va, va + bytes)` that are resident now.
fn touched_bytes(va: usize, bytes: u64) -> i64 {
    #[repr(C)]
    struct WsEx {
        va: *mut c_void,
        attrs: u64,
    }
    let pages = bytes.div_ceil(4096).min(65536) as usize;
    if va == 0 || pages == 0 {
        return -1;
    }
    let mut v: Vec<WsEx> = (0..pages).map(|k| WsEx { va: (va + k * 4096) as *mut c_void, attrs: 0 }).collect();
    // SAFETY: the array is ours and sized as told
    let ok = unsafe { K32QueryWorkingSetEx(win::GetCurrentProcess(), v.as_mut_ptr().cast(), (v.len() * std::mem::size_of::<WsEx>()) as u32) };
    if ok == 0 {
        return -1;
    }
    v.iter().filter(|e| e.attrs & 1 != 0).count() as i64 * 4096
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let orig: ReleaseFn = std::mem::transmute::<usize, ReleaseFn>(RELEASE.original());
    let traced_any = LIVE_N.load(Ordering::Relaxed) > 0;
    let ring_any = crate::stagering::has_live();
    if !traced_any && !ring_any {
        let r = orig(this);
        crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
        return r;
    }
    let key = this as usize;
    let tracked = (traced_any && TRACE.try_lock().map(|t| t.live.contains_key(&key)).unwrap_or(false)) || (ring_any && crate::stagering::holds(key));
    if !tracked {
        let r = orig(this);
        crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
        return r;
    }
    // one reference of ours while the game's goes: when the game's last one went (the count is back to 1), the object
    // still lives to be measured; then ours goes, and the game gets the count it would have had
    let addref: ReleaseFn = std::mem::transmute::<usize, ReleaseFn>(ADDREF.load(Ordering::Acquire));
    addref(this);
    let n = orig(this);
    if n == 1 {
        // the game's last reference: a ring slot under it is free again (its copy's fence permitting)
        if ring_any {
            crate::stagering::on_release(key);
        }
        // never a wait on a game thread: a busy lock loses this release's record (counted)
        let (mapped, bytes) = TRACE.try_lock().ok().and_then(|t| t.live.get(&key).map(|&i| (t.entries[i].mapped, t.entries[i].info.bytes))).unwrap_or((0, 0));
        let touched = if mapped != 0 { touched_bytes(mapped, bytes) } else { -1 };
        match TRACE.try_lock() {
            Ok(mut t) => {
                if let Some(i) = t.live.remove(&key) {
                    t.entries[i].freed_ms = now_ms().max(1);
                    t.entries[i].touched = touched;
                    t.freed_since.push(i);
                    LIVE_N.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                DROPPED_BUSY.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    let r = orig(this);
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn map(this: *mut c_void, sub: u32, range: *const D3D12_RANGE, data: *mut *mut c_void) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let orig: MapFn = std::mem::transmute::<usize, MapFn>(MAP.original());
    let r = orig(this, sub, range, data);
    if r.is_ok() && LIVE_N.load(Ordering::Relaxed) > 0 && !data.is_null() && !(*data).is_null() {
        if let Ok(mut t) = TRACE.try_lock() {
            if let Some(&i) = t.live.get(&(this as usize)) {
                t.entries[i].mapped = *data as usize;
            }
        }
    }
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

/// The resource class's vtable (learnt at the overlay's install from a dummy buffer; 0: not yet).
static RESOURCE_VT: AtomicUsize = AtomicUsize::new(0);

/// From the overlay's install: the resource class's vtable, for the hooks a trace installs when it starts. A note.
///
/// The `Release` and `Map` slots are NOT hooked here: ReSkate's GPU diagnostics (2.1.2, `gpu_diagnostics` on)
/// detours whatever function a new resource's `Release` slot holds, so with our slot hook in place it detours our
/// function, and a DLL another hook jumps into can never be unmapped (it stays inert at unload). Only a trace, which
/// wants lifetimes, pays that price; the staging ring works from the copy and submission hooks alone.
pub fn remember_vtable(vt: usize) -> String {
    RESOURCE_VT.store(vt, Ordering::Release);
    ADDREF.store(win::read_ptr(vt + 8).unwrap_or(0), Ordering::Release);
    if ADDREF.load(Ordering::Acquire) == 0 {
        return "; allocation trace: the resource vtable has no AddRef".into();
    }
    format!("; allocation trace ready (resource vtable {vt:#x}, its hooks go in when a trace starts)")
}

/// Hooks the resource class's `Release` and `Map` (once, at the first trace). A note.
pub fn install_hooks() -> String {
    let vt = RESOURCE_VT.load(Ordering::Acquire);
    if vt == 0 || ADDREF.load(Ordering::Acquire) == 0 {
        return "no resource vtable yet (the overlay is not installed): lifetimes and page counts stay unknown".into();
    }
    if RELEASE.installed() && MAP.installed() {
        return "resource hooks in place".into();
    }
    let mut notes = Vec::new();
    // SAFETY: the vtable is D3D12Core's resource class's, read at install; the slots hold code
    unsafe {
        if !RELEASE.installed() {
            if let Err(e) = RELEASE.install(vt + 2 * 8, release as *const () as usize) {
                notes.push(format!("Release not hooked: {e}"));
            }
        }
        if !MAP.installed() {
            if let Err(e) = MAP.install(vt + std::mem::offset_of!(ID3D12Resource_Vtbl, Map), map as *const () as usize) {
                notes.push(format!("Map not hooked: {e}"));
            }
        }
    }
    if notes.is_empty() {
        "resource Release and Map hooked for lifetimes (with ReSkate's GPU diagnostics on, the DLL then stays loaded inert at unload)".into()
    } else {
        notes.join(", ")
    }
}

/// For the overlay's `ours()`.
pub fn hooks() -> Vec<(&'static VSlot, usize, &'static str)> {
    vec![(&RELEASE, release as *const () as usize, "resource Release"), (&MAP, map as *const () as usize, "resource Map")]
}

/// One allocation as a line of the file (its stack as module+offset).
fn entry_line(e: &Entry, started: u64, i: usize, frames: &[String]) -> String {
    json!({"alloc": i, "t": e.t_ms - started, "thread": e.thread, "what": e.what, "info": e.info, "ms": e.ms, "hr": e.hr, "raw": format!("{:#x}", e.raw), "stack": frames}).to_string()
}

/// What the file lacks, taken under the lock to be formatted and written outside it: the new allocations, the first
/// copies known since, the releases since, the watch's samples since. A game thread's record never waits on a file
/// write (the stutter of 2026-10-23 18:06 was the trace's two-second write holding the lock the job threads needed).
struct Pending {
    started: u64,
    entries: Vec<(usize, Entry)>,
    copies: Vec<(usize, Entry)>,
    /// (index, freed at, touched, copied to, copies)
    frees: Vec<(usize, u64, i64, u64, u32)>,
    samples: Vec<(u64, u64, u64)>,
}

fn drain(t: &mut Trace) -> Pending {
    let started = t.started_ms;
    let entries: Vec<(usize, Entry)> = (t.flushed..t.entries.len()).map(|i| (i, t.entries[i].clone())).collect();
    t.flushed = t.entries.len();
    let copies: Vec<(usize, Entry)> = t.copies_since.iter().map(|&i| (i, t.entries[i].clone())).collect();
    t.copies_since.clear();
    let frees: Vec<(usize, u64, i64, u64, u32)> = t
        .freed_since
        .iter()
        .map(|&i| {
            let e = &t.entries[i];
            (i, e.freed_ms.saturating_sub(started), e.touched, e.copied_to, e.copies)
        })
        .collect();
    t.freed_since.clear();
    let samples = t.samples[t.flushed_samples..].to_vec();
    t.flushed_samples = t.samples.len();
    Pending { started, entries, copies, frees, samples }
}

/// Appends what `drain` took to the trace's file (none open: dropped). Our threads only.
fn write_pending(p: Pending) {
    use std::io::Write;
    if p.entries.is_empty() && p.copies.is_empty() && p.frees.is_empty() && p.samples.is_empty() {
        return;
    }
    let Ok(mut guard) = WRITER.lock() else { return };
    let Writer { file, names } = &mut *guard;
    let Some(file) = file.as_mut() else { return };
    let started = p.started;
    let mut out = String::new();
    for (i, e) in &p.entries {
        let frames: Vec<String> = e.stack[..e.frames as usize].iter().filter(|&&a| a != 0).map(|&a| module_of(a, names)).collect();
        out.push_str(&entry_line(e, started, *i, &frames));
        out.push('\n');
    }
    // the first copy out of each traced buffer, once known (its destination says what the staging was for; `t_copy`
    // is when the game recorded it, `at` where in the destination it landed)
    for (i, e) in &p.copies {
        let (k, w, h, d, m, f) = e.dst_desc;
        out.push_str(&json!({"copy": i, "t_copy": e.copy_ms.saturating_sub(started), "dst": format!("{:#x}", e.dst), "sub": e.dst_sub, "at": e.dst_at, "dst_kind": k, "dst_w": w, "dst_h": h, "dst_d": d, "dst_mips": m, "dst_format": f}).to_string());
        out.push('\n');
    }
    for (i, t, touched, copied_to, copies) in &p.frees {
        out.push_str(&json!({"free": i, "t": t, "touched": touched, "copied_to": copied_to, "copies": copies}).to_string());
        out.push('\n');
    }
    for (ms, l, nl) in &p.samples {
        out.push_str(&json!({"sample": ms.saturating_sub(started), "video": l, "system": nl}).to_string());
        out.push('\n');
    }
    let _ = file.write_all(out.as_bytes());
    let _ = file.flush();
}

/// Starts (or restarts) a trace of `seconds`, keeping allocations of at least `min_bytes`.
pub fn start(seconds: f64, min_bytes: u64) -> Value {
    let now = now_ms();
    let (spawn, path, pending, new_file) = {
        let Ok(mut t) = TRACE.lock() else { return json!({"error": "the trace is poisoned"}) };
        let pending = drain(&mut t);
        t.until_ms = now + (seconds.clamp(1.0, 1800.0) * 1000.0) as u64;
        t.min_bytes = min_bytes;
        t.started_ms = now;
        t.entries.clear();
        t.live.clear();
        t.samples.clear();
        t.dropped = 0;
        t.flushed = 0;
        t.flushed_samples = 0;
        t.freed_since.clear();
        t.copies_since.clear();
        LIVE_N.store(0, Ordering::Relaxed);
        DROPPED_BUSY.store(0, Ordering::Relaxed);
        let spawn = !t.sampler;
        t.sampler = true;
        t.path = String::new();
        let mut new_file = None;
        if let Some(dir) = crate::worker::data_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let file = dir.join(format!("alloc-trace-{secs}.jsonl"));
            if let Ok(f) = std::fs::File::create(&file) {
                new_file = Some(f);
                t.path = file.display().to_string();
            }
        }
        (spawn, t.path.clone(), pending, new_file)
    };
    // the old trace's remainder to its file, then the new file takes over
    write_pending(pending);
    if let Ok(mut w) = WRITER.lock() {
        w.file = new_file;
    }
    TRACING.store(true, Ordering::Release);
    if spawn && !STOP.load(Ordering::Acquire) {
        std::thread::Builder::new()
            .name("caveman-fx alloc sampler".into())
            .spawn(|| {
                let mut ticks = 0u32;
                loop {
                    std::thread::sleep(Duration::from_millis(50));
                    ticks += 1;
                    let (over, until) = TRACE.lock().map(|t| (now_ms() > t.until_ms + 3000, t.until_ms)).unwrap_or((true, 0));
                    if over || until == 0 || STOP.load(Ordering::Acquire) || crate::overlay::stopping() {
                        let pending = TRACE.lock().ok().map(|mut t| {
                            t.sampler = false;
                            drain(&mut t)
                        });
                        if let Some(p) = pending {
                            write_pending(p);
                        }
                        return;
                    }
                    let sample = crate::memwatch::quick();
                    // the lock is held for the push and the drain's clones only; the formatting and the write happen
                    // after it is released
                    let pending = TRACE.lock().ok().and_then(|mut t| {
                        if let Some((local, non_local)) = sample {
                            t.samples.push((now_ms(), local, non_local));
                        }
                        ticks.is_multiple_of(40).then(|| drain(&mut t))
                    });
                    if let Some(p) = pending {
                        write_pending(p);
                    }
                }
            })
            .ok();
    }
    let hooks = install_hooks();
    crate::worker::log(&format!(
        "allocation trace: on for {seconds:.0} s, allocations of {} and more, appended to {path}; {hooks}",
        crate::memguard::size(min_bytes)
    ));
    status()
}
pub fn stop() -> Value {
    TRACING.store(false, Ordering::Release);
    if let Ok(mut t) = TRACE.lock() {
        t.until_ms = now_ms();
    }
    status()
}

/// At unload: ends the trace and waits (at most half a second) for the sampler thread to leave our code.
pub fn shutdown() {
    STOP.store(true, Ordering::Release);
    TRACING.store(false, Ordering::Release);
    for _ in 0..50 {
        let running = TRACE.lock().map(|t| t.sampler).unwrap_or(false);
        if !running {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let pending = TRACE.lock().ok().map(|mut t| {
        let p = drain(&mut t);
        t.live.clear();
        LIVE_N.store(0, Ordering::Relaxed);
        p
    });
    if let Some(p) = pending {
        write_pending(p);
    }
    if let Ok(mut w) = WRITER.lock() {
        w.file = None;
    }
}
pub fn status() -> Value {
    let now = now_ms();
    let Ok(t) = TRACE.lock() else { return json!({"error": "the trace is poisoned"}) };
    json!({
        "tracing": tracing() && now <= t.until_ms,
        "seconds_left": if now <= t.until_ms { (t.until_ms - now) as f64 / 1000.0 } else { 0.0 },
        "allocations": t.entries.len(),
        "live": t.live.len(),
        "samples": t.samples.len(),
        "dropped": t.dropped,
        "dropped_busy": DROPPED_BUSY.load(Ordering::Relaxed),
        "file": t.path,
        "hooks": {"release": RELEASE.installed(), "map": MAP.installed()},
    })
}

/// The module and offset of a code address, cached.
fn module_of(addr: usize, cache: &mut HashMap<usize, String>) -> String {
    let Some(mbi) = win::query(addr) else { return format!("{addr:#x}") };
    let base = mbi.AllocationBase as usize;
    if base == 0 {
        return format!("{addr:#x}");
    }
    let name = cache.entry(base).or_insert_with(|| {
        let mut buf = [0u16; 260];
        // SAFETY: a plain Win32 call into our buffer
        let n = unsafe { win::GetModuleFileNameW(base as win::HANDLE, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let full = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        full.rsplit(['\\', '/']).next().unwrap_or("?").to_string()
    });
    format!("{name}+{:#x}", addr - base)
}

/// The summary; the file (`%LOCALAPPDATA%\Caveman\fx\alloc-trace-<seconds>.jsonl`, appended to as the trace ran)
/// gets what it still lacks.
pub fn report() -> Result<Value, String> {
    TRACING.store(false, Ordering::Release);
    let (entries, samples, started, until, dropped, min_bytes, path, pending) = {
        let mut t = TRACE.lock().map_err(|_| "the trace is poisoned".to_string())?;
        let pending = drain(&mut t);
        (t.entries.clone(), t.samples.clone(), t.started_ms, t.until_ms, t.dropped, t.min_bytes, t.path.clone(), pending)
    };
    write_pending(pending);
    if entries.is_empty() && samples.is_empty() {
        return Err("nothing traced yet (mem trace SECONDS first)".into());
    }
    let now = now_ms();
    let mut cache = HashMap::new();
    let mut names: HashMap<usize, String> = HashMap::new();
    let mut name = |a: usize, cache: &mut HashMap<usize, String>| -> String { names.entry(a).or_insert_with(|| module_of(a, cache)).clone() };
    let own = |s: &str| {
        s.starts_with("caveman_fx")
            || s.starts_with("ReSkate")
            || s.starts_with("D3D12")
            || s.starts_with("d3d12")
            || s.starts_with("nvwgf2umx")
            || s.starts_with("KERNEL")
            || s.starts_with("ntdll")
    };
    // by the game's call site: the first two frames in Skate.exe (else the first two not ours)
    #[derive(Default)]
    struct Agg {
        count: u64,
        bytes: u64,
        max: u64,
        ms: f64,
        life_n: u64,
        life_ms: f64,
        held: u64,
        copied_n: u64,
        copied_frac: f64,
        touched_n: u64,
        touched_frac: f64,
        heaps: HashMap<u8, u64>,
        sample: String,
    }
    let mut by: HashMap<String, Agg> = HashMap::new();
    let mut resolved: Vec<Vec<String>> = Vec::with_capacity(entries.len());
    for e in &entries {
        let frames: Vec<String> = e.stack[..e.frames as usize].iter().filter(|&&a| a != 0).map(|&a| name(a, &mut cache)).collect();
        let game: Vec<&String> = frames.iter().filter(|s| s.starts_with("Skate.exe")).take(2).collect();
        let key = if game.is_empty() {
            frames.iter().filter(|s| !own(s)).take(2).cloned().collect::<Vec<_>>().join(" < ")
        } else {
            game.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" < ")
        };
        let a = by.entry(if key.is_empty() { "(no game frame)".into() } else { key }).or_default();
        a.count += 1;
        a.bytes += e.info.bytes;
        a.max = a.max.max(e.info.bytes);
        a.ms += e.ms as f64;
        *a.heaps.entry(e.info.heap).or_default() += e.info.bytes;
        if e.freed_ms > 0 {
            a.life_n += 1;
            a.life_ms += (e.freed_ms - e.t_ms) as f64;
        } else if e.raw != 0 {
            a.held += 1;
        }
        if e.copies > 0 && e.info.bytes > 0 {
            a.copied_n += 1;
            a.copied_frac += (e.copied_to.min(e.info.bytes)) as f64 / e.info.bytes as f64;
        }
        if e.touched >= 0 && e.info.bytes > 0 {
            a.touched_n += 1;
            a.touched_frac += (e.touched as u64).min(e.info.bytes) as f64 / e.info.bytes as f64;
        }
        if a.sample.is_empty() {
            a.sample = frames.iter().filter(|s| !s.starts_with("caveman_fx")).take(6).cloned().collect::<Vec<_>>().join(" < ");
        }
        resolved.push(frames);
    }
    let mut groups: Vec<Value> = by
        .iter()
        .map(|(k, a)| {
            json!({
                "site": k,
                "count": a.count,
                "mb": a.bytes as f64 / 1048576.0,
                "largest_mb": a.max as f64 / 1048576.0,
                "mean_ms": if a.count > 0 { a.ms / a.count as f64 } else { 0.0 },
                "freed": a.life_n,
                "held": a.held,
                "mean_life_ms": if a.life_n > 0 { a.life_ms / a.life_n as f64 } else { -1.0 },
                "copied_frac": if a.copied_n > 0 { a.copied_frac / a.copied_n as f64 } else { -1.0 },
                "copied_n": a.copied_n,
                "touched_frac": if a.touched_n > 0 { a.touched_frac / a.touched_n as f64 } else { -1.0 },
                "heaps_mb": a.heaps.iter().map(|(h, b)| (crate::memguard::heap_name(*h).to_string(), *b as f64 / 1048576.0)).collect::<HashMap<_, _>>(),
                "stack": a.sample,
            })
        })
        .collect();
    groups.sort_by(|x, y| y["mb"].as_f64().partial_cmp(&x["mb"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    // the timeline: created and freed per bucket, with the watch's last sample in it
    let end = until.max(now).max(entries.last().map(|e| e.t_ms).unwrap_or(0));
    let buckets = ((end.saturating_sub(started)) / BUCKET_MS + 1).min(4000) as usize;
    let mut created = vec![(0u64, 0u64); buckets];
    let mut freed = vec![0u64; buckets];
    for e in &entries {
        let b = ((e.t_ms - started) / BUCKET_MS) as usize;
        if b < buckets {
            created[b].0 += 1;
            created[b].1 += e.info.bytes;
        }
        if e.freed_ms > 0 {
            let b = ((e.freed_ms.saturating_sub(started)) / BUCKET_MS) as usize;
            if b < buckets {
                freed[b] += e.info.bytes;
            }
        }
    }
    let mut watch = vec![(0u64, 0u64); buckets];
    for (ms, l, nl) in &samples {
        let b = ((ms.saturating_sub(started)) / BUCKET_MS) as usize;
        if b < buckets {
            watch[b] = (*l, *nl);
        }
    }
    let timeline: Vec<Value> = (0..buckets)
        .filter(|&b| created[b].0 > 0 || freed[b] > 0 || b % 4 == 0)
        .map(|b| json!({"t": (b as u64 * BUCKET_MS) as f64 / 1000.0, "allocs": created[b].0, "mb": created[b].1 as f64 / 1048576.0, "freed_mb": freed[b] as f64 / 1048576.0, "video_mb": watch[b].0 >> 20, "system_mb": watch[b].1 >> 20}))
        .collect();
    let mut largest: Vec<(usize, &Entry)> = entries.iter().enumerate().collect();
    largest.sort_by_key(|e| std::cmp::Reverse(e.1.info.bytes));
    let largest: Vec<Value> = largest
        .iter()
        .take(30)
        .map(|(i, e)| {
            json!({
                "t": (e.t_ms - started) as f64 / 1000.0, "thread": e.thread, "what": e.what, "describe": e.info.describe(), "ms": e.ms,
                "life_ms": if e.freed_ms > 0 { (e.freed_ms - e.t_ms) as i64 } else { -1 }, "copied_to_mb": e.copied_to as f64 / 1048576.0, "copies": e.copies,
                "touched_mb": if e.touched >= 0 { e.touched as f64 / 1048576.0 } else { -1.0 }, "stack": resolved[*i].iter().filter(|s| !s.starts_with("caveman_fx")).take(5).cloned().collect::<Vec<_>>().join(" < "),
            })
        })
        .collect();
    let path = (!path.is_empty()).then_some(path);
    let total: u64 = entries.iter().map(|e| e.info.bytes).sum();
    let by_heap: HashMap<String, f64> = entries.iter().fold(HashMap::new(), |mut m, e| {
        *m.entry(crate::memguard::heap_name(e.info.heap).to_string()).or_default() += e.info.bytes as f64 / 1048576.0;
        m
    });
    let peak_system = samples.iter().map(|s| s.2).max().unwrap_or(0) >> 20;
    let base_system = samples.first().map(|s| s.2).unwrap_or(0) >> 20;
    Ok(json!({
        "window_s": (until.saturating_sub(started)) as f64 / 1000.0,
        "allocations": entries.len(),
        "dropped": dropped,
        "min_bytes": min_bytes,
        "total_mb": total as f64 / 1048576.0,
        "by_heap_mb": by_heap,
        "held_at_report": entries.iter().filter(|e| e.raw != 0 && e.freed_ms == 0).count(),
        "system_memory_mb": {"first": base_system, "peak": peak_system},
        "sites": groups,
        "timeline": timeline,
        "largest": largest,
        "file": path,
    }))
}
