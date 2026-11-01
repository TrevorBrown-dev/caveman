//! Video on the ridden board: a producer (the trainer's `board video`: a file through ffmpeg, or OBS's virtual
//! camera) puts RGBA8 mip chains of the in-world art copy's shape into a shared memory ring, and the DLL uploads the
//! newest one into that copy once per present (board.rs `video_present`), so whatever the producer shows plays on
//! the board at the game's frame rate. The menu's board and the composite keep the still art.
//!
//! The ring (`Local\caveman-fx-video`): a 4 KiB header, then `slots` frames, each the chain's levels one after
//! another, tightly packed, in a slot padded to 4 KiB. The producer fills slot `seq % slots` and then stores `seq`
//! (release); the DLL reads `seq` (acquire), copies the slot into its staging buffer, and reads `seq` again: a
//! producer that lapped it (`seq` moved on by `slots` or more) may have been writing that slot, so the frame is
//! dropped, not shown. The DLL writes back what it took (`consumer_seq`) and what it dropped, for the producer's own
//! report. The frame may be the copy's size or a power-of-two multiple of it (the chain's later levels are used then).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::win;

pub const MAPPING_NAME: &str = "Local\\caveman-fx-video";
/// A manual-reset event a producer watches: set (by `board video stop`, or the app), the producer stops cleanly.
pub const STOP_EVENT_NAME: &str = "Local\\caveman-fx-video-stop";
/// "CVID"
pub const MAGIC: u32 = 0x4449_5643;
pub const VERSION: u32 = 1;
pub const HEADER_BYTES: usize = 4096;
pub const MAX_SLOTS: u32 = 4;

/// Offsets in the header.
pub const H_MAGIC: usize = 0;
pub const H_VERSION: usize = 4;
pub const H_WIDTH: usize = 8;
pub const H_HEIGHT: usize = 12;
pub const H_LEVELS: usize = 16;
pub const H_SLOTS: usize = 20;
pub const H_SLOT_BYTES: usize = 24;
pub const H_FPS_X1000: usize = 28;
pub const H_SEQ: usize = 32;
pub const H_CONSUMER_SEQ: usize = 40;
pub const H_CONSUMER_DROPS: usize = 48;
pub const H_PRODUCER_ID: usize = 56;

/// A slot's stride: its bytes rounded up to 4 KiB.
pub fn slot_stride(slot_bytes: usize) -> usize {
    slot_bytes.div_ceil(HEADER_BYTES) * HEADER_BYTES
}

/// The whole mapping's size.
pub fn mapping_bytes(slot_bytes: usize, slots: u32) -> usize {
    HEADER_BYTES + slot_stride(slot_bytes) * slots as usize
}

/// The bytes of an RGBA8 chain of `levels` levels from `width` x `height` (each level the half of the one before,
/// never under 1 x 1), tightly packed.
pub fn chain_bytes(width: u32, height: u32, levels: u32) -> usize {
    let (mut w, mut h, mut n) = (width as usize, height as usize, 0usize);
    for _ in 0..levels {
        n += w * h * 4;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    n
}

static ON: AtomicBool = AtomicBool::new(false);
static VIEW: AtomicUsize = AtomicUsize::new(0);
static MAP_HANDLE: AtomicUsize = AtomicUsize::new(0);
static LAST_OPEN_TRY: AtomicU64 = AtomicU64::new(0);
pub static FRAMES: AtomicU64 = AtomicU64::new(0);
pub static DROPPED_BUSY: AtomicU64 = AtomicU64::new(0);
pub static DROPPED_TORN: AtomicU64 = AtomicU64::new(0);
pub static SKIPPED: AtomicU64 = AtomicU64::new(0);
pub static LAST_SEQ: AtomicU64 = AtomicU64::new(0);
pub static LAST_MS: AtomicU64 = AtomicU64::new(0);
pub static LAST_FILL_US: AtomicU64 = AtomicU64::new(0);
pub static LAST_TOTAL_US: AtomicU64 = AtomicU64::new(0);
static NOTE: Mutex<String> = Mutex::new(String::new());

/// A frame of the ring, by reference (valid while the view is mapped: the present it was taken in).
pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    pub ptr: *const u8,
    pub len: usize,
}

fn u32_at(view: usize, off: usize) -> u32 {
    // SAFETY: the view is mapped and at least HEADER_BYTES long while VIEW holds it
    unsafe { std::ptr::read_volatile((view + off) as *const u32) }
}

fn atomic_at(view: usize, off: usize) -> &'static AtomicU64 {
    // SAFETY: an aligned u64 inside the mapped header; the mapping outlives any use within a present
    unsafe { AtomicU64::from_ptr((view + off) as *mut u64) }
}

pub fn wanted() -> bool {
    ON.load(Ordering::Acquire)
}

pub fn note(s: &str) {
    if let Ok(mut n) = NOTE.lock() {
        if *n != s {
            *n = s.to_string();
        }
    }
}

/// `board video on|off`: whether frames are looked for (and the ring opened now, when there is one).
pub fn set(on: bool) -> Value {
    if on {
        ON.store(true, Ordering::Release);
        match open() {
            Ok(what) => note(&what),
            Err(e) => note(&e),
        }
    } else {
        ON.store(false, Ordering::Release);
        close();
        note("off");
        crate::board::video_stopped();
    }
    status()
}

fn close() {
    let v = VIEW.swap(0, Ordering::AcqRel);
    if v != 0 {
        unsafe { win::UnmapViewOfFile(v as *const _) };
    }
    let h = MAP_HANDLE.swap(0, Ordering::AcqRel);
    if h != 0 {
        unsafe { win::CloseHandle(h as win::HANDLE) };
    }
}

/// Opens the producer's ring, checking its header. What was found.
fn open() -> Result<String, String> {
    close();
    let name = win::wide(MAPPING_NAME);
    // SAFETY: plain Win32 calls with a NUL-terminated name
    let h = unsafe { win::OpenFileMappingW(win::FILE_MAP_ALL_ACCESS, 0, name.as_ptr()) };
    if h.is_null() {
        return Err(format!("no video ring (no producer running; error {})", unsafe { win::GetLastError() }));
    }
    let head = unsafe { win::MapViewOfFile(h, win::FILE_MAP_ALL_ACCESS, 0, 0, HEADER_BYTES) } as usize;
    if head == 0 {
        unsafe { win::CloseHandle(h) };
        return Err("the video ring's header could not be mapped".into());
    }
    let (magic, version) = (u32_at(head, H_MAGIC), u32_at(head, H_VERSION));
    let (width, height, levels, slots, slot_bytes) =
        (u32_at(head, H_WIDTH), u32_at(head, H_HEIGHT), u32_at(head, H_LEVELS), u32_at(head, H_SLOTS), u32_at(head, H_SLOT_BYTES) as usize);
    unsafe { win::UnmapViewOfFile(head as *const _) };
    let bad = |why: String| {
        unsafe { win::CloseHandle(h) };
        Err(why)
    };
    if magic != MAGIC || version != VERSION {
        return bad(format!("the video ring's header is not ours (magic {magic:#x}, version {version})"));
    }
    if !(1..=MAX_SLOTS).contains(&slots)
        || !(1..=4096).contains(&width)
        || !(1..=8192).contains(&height)
        || !(1..=12).contains(&levels)
        || slot_bytes != chain_bytes(width, height, levels)
    {
        return bad(format!("the video ring's shape is not usable ({width}x{height}, {levels} levels, {slots} slots, {slot_bytes} bytes a slot)"));
    }
    let total = mapping_bytes(slot_bytes, slots);
    let view = unsafe { win::MapViewOfFile(h, win::FILE_MAP_ALL_ACCESS, 0, 0, total) } as usize;
    if view == 0 {
        return bad(format!("the video ring could not be mapped whole ({total} bytes; error {})", unsafe { win::GetLastError() }));
    }
    LAST_SEQ.store(0, Ordering::Release);
    MAP_HANDLE.store(h as usize, Ordering::Release);
    VIEW.store(view, Ordering::Release);
    Ok(format!("ring open: {width}x{height}, {levels} levels, {slots} slots"))
}

/// The newest frame the producer finished, when it is not the one taken last (the render thread, once per present).
pub fn next_frame() -> Option<Frame> {
    if !ON.load(Ordering::Acquire) {
        return None;
    }
    let mut view = VIEW.load(Ordering::Acquire);
    if view == 0 {
        let now = win::now_ms();
        if now.saturating_sub(LAST_OPEN_TRY.load(Ordering::Relaxed)) >= 1000 {
            LAST_OPEN_TRY.store(now, Ordering::Relaxed);
            match open() {
                Ok(what) => note(&what),
                Err(e) => note(&e),
            }
            view = VIEW.load(Ordering::Acquire);
        }
        if view == 0 {
            return None;
        }
    }
    let seq = atomic_at(view, H_SEQ).load(Ordering::Acquire);
    if seq == 0 || seq == LAST_SEQ.load(Ordering::Acquire) {
        return None;
    }
    let (width, height, levels, slots, slot_bytes) =
        (u32_at(view, H_WIDTH), u32_at(view, H_HEIGHT), u32_at(view, H_LEVELS), u32_at(view, H_SLOTS).max(1), u32_at(view, H_SLOT_BYTES) as usize);
    let slot = (seq % slots as u64) as usize;
    Some(Frame { seq, width, height, levels, ptr: (view + HEADER_BYTES + slot * slot_stride(slot_bytes)) as *const u8, len: slot_bytes })
}

/// Whether the producer has not lapped `seq`'s slot since it was taken (else the copy may be torn).
pub fn still_valid(seq: u64) -> bool {
    let view = VIEW.load(Ordering::Acquire);
    if view == 0 {
        return false;
    }
    let slots = u32_at(view, H_SLOTS).max(1) as u64;
    atomic_at(view, H_SEQ).load(Ordering::Acquire) < seq + slots
}

/// A frame uploaded.
pub fn took(seq: u64, fill_us: u64, total_us: u64) {
    LAST_SEQ.store(seq, Ordering::Release);
    FRAMES.fetch_add(1, Ordering::Relaxed);
    LAST_MS.store(win::now_ms(), Ordering::Relaxed);
    LAST_FILL_US.store(fill_us, Ordering::Relaxed);
    LAST_TOTAL_US.store(total_us, Ordering::Relaxed);
    let view = VIEW.load(Ordering::Acquire);
    if view != 0 {
        atomic_at(view, H_CONSUMER_SEQ).store(seq, Ordering::Release);
    }
}

/// A frame not uploaded: its staging buffer still in use (`busy`), or the producer lapped it (torn).
pub fn dropped(busy: bool) {
    if busy {
        DROPPED_BUSY.fetch_add(1, Ordering::Relaxed);
    } else {
        DROPPED_TORN.fetch_add(1, Ordering::Relaxed);
    }
    let view = VIEW.load(Ordering::Acquire);
    if view != 0 {
        atomic_at(view, H_CONSUMER_DROPS).fetch_add(1, Ordering::AcqRel);
    }
}

/// A frame passed over for good (it does not fit the copy): counted as taken so it is not retried every present.
pub fn skip(seq: u64, why: &str) {
    SKIPPED.fetch_add(1, Ordering::Relaxed);
    LAST_SEQ.store(seq, Ordering::Release);
    note(why);
}

pub fn status() -> Value {
    let view = VIEW.load(Ordering::Acquire);
    let ring = if view == 0 {
        Value::Null
    } else {
        json!({
            "width": u32_at(view, H_WIDTH),
            "height": u32_at(view, H_HEIGHT),
            "levels": u32_at(view, H_LEVELS),
            "slots": u32_at(view, H_SLOTS),
            "fps": u32_at(view, H_FPS_X1000) as f64 / 1000.0,
            "seq": atomic_at(view, H_SEQ).load(Ordering::Acquire),
            "producer_id": format!("{:#x}", atomic_at(view, H_PRODUCER_ID).load(Ordering::Acquire)),
        })
    };
    let last = LAST_MS.load(Ordering::Relaxed);
    json!({
        "on": wanted(),
        "ring": ring,
        "frames": FRAMES.load(Ordering::Relaxed),
        "dropped_busy": DROPPED_BUSY.load(Ordering::Relaxed),
        "dropped_torn": DROPPED_TORN.load(Ordering::Relaxed),
        "skipped": SKIPPED.load(Ordering::Relaxed),
        "last_seq": LAST_SEQ.load(Ordering::Relaxed),
        "last_ms_ago": if last == 0 { Value::Null } else { json!(win::now_ms().saturating_sub(last)) },
        "fill_us": LAST_FILL_US.load(Ordering::Relaxed),
        "total_us": LAST_TOTAL_US.load(Ordering::Relaxed),
        "note": NOTE.lock().map(|n| n.clone()).unwrap_or_default(),
    })
}

/// At unload: the ring unmapped (the GPU side goes with the board's objects).
pub fn shutdown() {
    ON.store(false, Ordering::Release);
    close();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_and_mapping_sizes() {
        // 256 x 1024, 9 levels: 1 MiB + its halves down to 1 x 4
        assert_eq!(chain_bytes(256, 1024, 9), 4 * (256 * 1024 + 128 * 512 + 64 * 256 + 32 * 128 + 16 * 64 + 8 * 32 + 4 * 16 + 2 * 8 + 4));
        assert_eq!(slot_stride(4096), 4096);
        assert_eq!(slot_stride(4097), 8192);
        assert_eq!(mapping_bytes(4096, 3), HEADER_BYTES + 3 * 4096);
        assert_eq!(H_PRODUCER_ID + 8, 64);
    }

    #[test]
    fn no_ring_means_no_frame() {
        assert!(next_frame().is_none());
        assert!(!still_valid(1));
    }
}
