//! The memory watch (`docs/fx-audit.md`, guardrail 1): the game process's video memory budget and use, both segments
//! (LOCAL: the card's memory; NON_LOCAL: system memory the GPU uses, where upload heaps live), asked of DXGI once a
//! second on the worker thread. In `draw status`; a log line whenever either use moves by more than
//! [`JUMP_BYTES`] within a second (the game's own spikes at every skater creation show with their time); and every
//! allocation of ours is refused within [`MARGIN`] of either budget ([`room_for`]).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};
use windows::Win32::Graphics::Dxgi::*;

/// Allocations of ours stay this far under either budget.
pub const MARGIN: u64 = 384 << 20;
/// A move of either segment's use by this much within a second is logged.
pub const JUMP_BYTES: u64 = 256 << 20;

/// The adapter the game's device is on (its LUID as one i64; 0: none yet).
static LUID: AtomicI64 = AtomicI64::new(0);
static MEASURED: AtomicBool = AtomicBool::new(false);
/// The last sample: (budget, use) of the local and the non-local segment, and when (ms).
static LOCAL: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static NON_LOCAL: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static AT_MS: AtomicU64 = AtomicU64::new(0);
static SAMPLES: AtomicU64 = AtomicU64::new(0);
static JUMPS: AtomicU64 = AtomicU64::new(0);
static ERROR: Mutex<String> = Mutex::new(String::new());

struct Watch {
    adapter: IDXGIAdapter3,
    luid: i64,
}

// The adapter is a free-threaded COM object used from the worker thread only.
unsafe impl Send for Watch {}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

/// From the overlay, once it has the game's device: which adapter to watch.
pub fn set_adapter(luid: windows::Win32::Foundation::LUID) {
    let v = ((luid.HighPart as i64) << 32) | luid.LowPart as i64;
    LUID.store(v, Ordering::Release);
}

/// Once a second from the worker's tick: samples both segments, logs a jump.
pub fn tick(now_ms: u64) {
    let luid = LUID.load(Ordering::Acquire);
    if luid == 0 || now_ms < AT_MS.load(Ordering::Relaxed) + 1000 {
        return;
    }
    AT_MS.store(now_ms.max(1), Ordering::Relaxed);
    let Ok(mut w) = WATCH.lock() else { return };
    if w.as_ref().is_none_or(|x| x.luid != luid) {
        *w = None;
        let made = unsafe { CreateDXGIFactory1::<IDXGIFactory4>() }.and_then(|f| {
            let l = windows::Win32::Foundation::LUID { LowPart: luid as u32, HighPart: (luid >> 32) as i32 };
            unsafe { f.EnumAdapterByLuid::<IDXGIAdapter3>(l) }
        });
        match made {
            Ok(adapter) => *w = Some(Watch { adapter, luid }),
            Err(e) => {
                if let Ok(mut err) = ERROR.lock() {
                    *err = format!("the adapter could not be opened for the memory watch: {e}");
                }
                return;
            }
        }
    }
    let adapter = &w.as_ref().unwrap().adapter;
    let mut jumps = Vec::new();
    for (group, cells, name) in
        [(DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &LOCAL, "video memory"), (DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &NON_LOCAL, "system memory for the GPU")]
    {
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        if unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut info) }.is_err() {
            continue;
        }
        let before = cells[1].load(Ordering::Relaxed);
        cells[0].store(info.Budget, Ordering::Relaxed);
        cells[1].store(info.CurrentUsage, Ordering::Relaxed);
        if MEASURED.load(Ordering::Relaxed) && info.CurrentUsage.abs_diff(before) > JUMP_BYTES {
            jumps.push(format!("{name}: {} -> {} MB of a {} MB budget", before >> 20, info.CurrentUsage >> 20, info.Budget >> 20));
        }
    }
    MEASURED.store(true, Ordering::Release);
    SAMPLES.fetch_add(1, Ordering::Relaxed);
    drop(w);
    for j in jumps {
        JUMPS.fetch_add(1, Ordering::Relaxed);
        crate::worker::log(&format!("memory watch: {j}"));
    }
}

/// Both segments' use right now, asked of the adapter (for the allocation trace's sampler): (video memory, system
/// memory for the GPU). None before the watch has its adapter.
pub fn quick() -> Option<(u64, u64)> {
    let w = WATCH.lock().ok()?;
    let adapter = &w.as_ref()?.adapter;
    let mut out = [0u64; 2];
    for (k, group) in [DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL].into_iter().enumerate() {
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        unsafe { adapter.QueryVideoMemoryInfo(0, group, &mut info) }.ok()?;
        out[k] = info.CurrentUsage;
    }
    Some((out[0], out[1]))
}

/// Whether an allocation of `bytes` of ours (video memory, or system memory for an upload heap) stays under both
/// budgets by the margin. Err names what is in the way (also when nothing has been measured yet).
pub fn room_for(bytes: u64) -> Result<(), String> {
    if !MEASURED.load(Ordering::Acquire) {
        return Err("the video memory has not been measured yet".into());
    }
    if let Some(note) = crate::memguard::squeeze_note() {
        return Err(note);
    }
    for (cells, name) in [(&LOCAL, "video memory"), (&NON_LOCAL, "system memory for the GPU")] {
        let (budget, used) = (cells[0].load(Ordering::Relaxed), cells[1].load(Ordering::Relaxed));
        if budget > 0 && used + bytes + MARGIN > budget {
            return Err(format!("{name} near the game's budget: {} of {} MB used, {} MB asked", used >> 20, budget >> 20, bytes >> 20));
        }
    }
    Ok(())
}

/// Like [`room_for`], for system memory only: an upload heap lives in the non-local segment, and the video budget
/// has nothing to say about it (the 8 GB staging ring was refused against the video budget on 2026-10-24).
pub fn room_for_system(bytes: u64) -> Result<(), String> {
    if !MEASURED.load(Ordering::Acquire) {
        return Err("the video memory has not been measured yet".into());
    }
    if let Some(note) = crate::memguard::squeeze_note() {
        return Err(note);
    }
    let (budget, used) = (NON_LOCAL[0].load(Ordering::Relaxed), NON_LOCAL[1].load(Ordering::Relaxed));
    if budget > 0 && used + bytes + MARGIN > budget {
        return Err(format!("system memory for the GPU near the game's budget: {} of {} MB used, {} MB asked", used >> 20, budget >> 20, bytes >> 20));
    }
    Ok(())
}

/// Whether a sample was taken yet.
pub fn measured() -> bool {
    MEASURED.load(Ordering::Acquire)
}

/// For the status: both segments in MB, and how the watch is doing.
pub fn status() -> Value {
    let mb = |v: u64| v >> 20;
    json!({
        "measured": MEASURED.load(Ordering::Relaxed),
        "samples": SAMPLES.load(Ordering::Relaxed),
        "jumps_logged": JUMPS.load(Ordering::Relaxed),
        "local_mb": {"used": mb(LOCAL[1].load(Ordering::Relaxed)), "budget": mb(LOCAL[0].load(Ordering::Relaxed))},
        "non_local_mb": {"used": mb(NON_LOCAL[1].load(Ordering::Relaxed)), "budget": mb(NON_LOCAL[0].load(Ordering::Relaxed))},
        "margin_mb": mb(MARGIN),
        "error": ERROR.lock().map(|e| e.clone()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_is_refused_within_the_margin_of_either_budget() {
        MEASURED.store(false, Ordering::Release);
        assert!(room_for(1).is_err());
        LOCAL[0].store(16 << 30, Ordering::Relaxed);
        LOCAL[1].store(15 << 30, Ordering::Relaxed);
        NON_LOCAL[0].store(32 << 30, Ordering::Relaxed);
        NON_LOCAL[1].store(1 << 30, Ordering::Relaxed);
        MEASURED.store(true, Ordering::Release);
        assert!(room_for(100 << 20).is_ok());
        assert!(room_for(700 << 20).unwrap_err().starts_with("video memory"));
        LOCAL[1].store(10 << 30, Ordering::Relaxed);
        NON_LOCAL[1].store(31 << 30, Ordering::Relaxed);
        assert!(room_for(100 << 20).is_ok());
        assert!(room_for(800 << 20).unwrap_err().starts_with("system memory"));
        MEASURED.store(false, Ordering::Release);
    }
}
