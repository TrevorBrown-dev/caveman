//! The OBS link's video, read back for the board cam window. The DLL puts the frames a producer writes into the shared
//! memory ring `Local\caveman-fx-video` onto the ridden board (caveman-fx/src/video.rs: a 4 KiB header, then 2 to 4
//! slots of tightly packed RGBA8 mip chains of the art copy's shape); the window reads the same ring, so the deck in
//! the window shows the frame the game shows. Only the reading side lives here; the layout is the DLL's and changes
//! with it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAPPING_NAME: &str = "Local\\caveman-fx-video";
const MAGIC: u32 = 0x4449_5643;
const VERSION: u32 = 1;
const HEADER_BYTES: usize = 4096;
const MAX_SLOTS: u32 = 4;
const H_MAGIC: usize = 0;
const H_VERSION: usize = 4;
const H_WIDTH: usize = 8;
const H_HEIGHT: usize = 12;
const H_LEVELS: usize = 16;
const H_SLOTS: usize = 20;
const H_SLOT_BYTES: usize = 24;
const H_SEQ: usize = 32;
const H_PRODUCER_ID: usize = 56;
const FILE_MAP_READ: u32 = 0x0004;
/// A ring whose sequence has not moved for this long is let go of (a view of ours would keep a finished producer's
/// mapping alive by name); the next ask opens it again if it is still there.
const IDLE: Duration = Duration::from_secs(3);

type Handle = *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> Handle;
    fn MapViewOfFile(mapping: Handle, access: u32, offset_hi: u32, offset_lo: u32, bytes: usize) -> *mut c_void;
    fn UnmapViewOfFile(view: *const c_void) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
}

/// A slot's stride: its bytes rounded up to 4 KiB.
fn slot_stride(slot_bytes: usize) -> usize {
    slot_bytes.div_ceil(HEADER_BYTES) * HEADER_BYTES
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

/// Level `level` of such a chain: its width, height and byte offset in the chain.
pub fn level_of(width: u32, height: u32, level: u32) -> (u32, u32, usize) {
    let (mut w, mut h, mut off) = (width, height, 0usize);
    for _ in 0..level {
        off += w as usize * h as usize * 4;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    (w, h, off)
}

/// A mapped ring.
struct View {
    handle: Handle,
    view: usize,
    width: u32,
    height: u32,
    levels: u32,
    slots: u32,
    slot_bytes: usize,
    producer: u64,
    /// The sequence last seen and when it last changed (for `IDLE`).
    seen: u64,
    moved: Instant,
}

// SAFETY: the handle and the view are only used under the state's lock, one thread at a time
unsafe impl Send for View {}

impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: ours, mapped by `open`
        unsafe {
            UnmapViewOfFile(self.view as *const c_void);
            CloseHandle(self.handle);
        }
    }
}

/// The ring as this app has it open (none until a producer runs; let go when it stops).
#[derive(Default)]
pub struct RingState {
    view: Mutex<Option<View>>,
}

/// A frame copied out of the ring: one level of its chain.
pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub level: u32,
    pub rgba: Vec<u8>,
}

fn u32_at(view: usize, off: usize) -> u32 {
    // SAFETY: an aligned u32 inside the mapped header
    unsafe { std::ptr::read_volatile((view + off) as *const u32) }
}

fn u64_at(view: usize, off: usize) -> u64 {
    // SAFETY: an aligned u64 inside the mapped header; the view outlives the load
    unsafe { AtomicU64::from_ptr((view + off) as *mut u64) }.load(Ordering::Acquire)
}

fn open() -> Result<View, String> {
    let name: Vec<u16> = MAPPING_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: plain Win32 calls with a NUL-terminated name
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
    if handle.is_null() {
        return Err("no video ring".into());
    }
    let close = |why: String| {
        // SAFETY: the handle opened above
        unsafe { CloseHandle(handle) };
        Err(why)
    };
    // SAFETY: a view of the header of a mapping that is at least that large (the producer's is)
    let head = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, HEADER_BYTES) } as usize;
    if head == 0 {
        return close("the video ring's header could not be mapped".into());
    }
    let (magic, version) = (u32_at(head, H_MAGIC), u32_at(head, H_VERSION));
    let (width, height, levels, slots, slot_bytes) =
        (u32_at(head, H_WIDTH), u32_at(head, H_HEIGHT), u32_at(head, H_LEVELS), u32_at(head, H_SLOTS), u32_at(head, H_SLOT_BYTES) as usize);
    let producer = u64_at(head, H_PRODUCER_ID);
    // SAFETY: the view mapped above
    unsafe { UnmapViewOfFile(head as *const c_void) };
    if magic != MAGIC || version != VERSION {
        return close(format!("the video ring's header is not ours (magic {magic:#x}, version {version})"));
    }
    if !(1..=MAX_SLOTS).contains(&slots)
        || !(1..=4096).contains(&width)
        || !(1..=8192).contains(&height)
        || !(1..=12).contains(&levels)
        || slot_bytes != chain_bytes(width, height, levels)
    {
        return close(format!("the video ring's shape is not usable ({width}x{height}, {levels} levels, {slots} slots, {slot_bytes} bytes a slot)"));
    }
    let total = HEADER_BYTES + slot_stride(slot_bytes) * slots as usize;
    // SAFETY: the whole ring, whose size the header gave
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, total) } as usize;
    if view == 0 {
        return close(format!("the video ring could not be mapped whole ({total} bytes)"));
    }
    Ok(View { handle, view, width, height, levels, slots, slot_bytes, producer, seen: 0, moved: Instant::now() })
}

/// The newest frame the producer finished, at level `level` of its chain (clamped to the chain's), when it is not
/// frame `after`. None when there is no producer, when nothing new has come, or when the producer lapped the slot
/// while it was being copied (torn: the next ask gets a whole one). Each ask is one copy of the level; the window
/// asks a few dozen times a second while frames come.
pub fn newest(state: &RingState, after: u64, level: u32) -> Option<Frame> {
    let mut guard = state.view.lock().ok()?;
    if guard.is_none() {
        *guard = open().ok();
    }
    let v = guard.as_mut()?;
    let seq = u64_at(v.view, H_SEQ);
    if seq != v.seen {
        v.seen = seq;
        v.moved = Instant::now();
    } else if v.moved.elapsed() >= IDLE {
        *guard = None;
        return None;
    }
    // another producer took the name over (its own id): read its shape
    if u64_at(v.view, H_PRODUCER_ID) != v.producer {
        *guard = open().ok();
        return None;
    }
    if seq == 0 || seq == after {
        return None;
    }
    let level = level.min(v.levels - 1);
    let (width, height, offset) = level_of(v.width, v.height, level);
    let slot = (seq % v.slots as u64) as usize;
    let src = v.view + HEADER_BYTES + slot * slot_stride(v.slot_bytes) + offset;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    // SAFETY: the level lies inside the slot, inside the mapped ring
    unsafe { std::ptr::copy_nonoverlapping(src as *const u8, rgba.as_mut_ptr(), rgba.len()) };
    if u64_at(v.view, H_SEQ) >= seq + v.slots as u64 {
        return None;
    }
    Some(Frame { seq, width, height, level, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_and_levels() {
        assert_eq!(chain_bytes(512, 2048, 1), 512 * 2048 * 4);
        assert_eq!(chain_bytes(512, 2048, 2), 512 * 2048 * 4 + 256 * 1024 * 4);
        assert_eq!(level_of(512, 2048, 0), (512, 2048, 0));
        assert_eq!(level_of(512, 2048, 1), (256, 1024, 512 * 2048 * 4));
        assert_eq!(level_of(512, 2048, 2), (128, 512, 512 * 2048 * 4 + 256 * 1024 * 4));
        assert_eq!(level_of(1, 1, 3), (1, 1, 12));
        assert_eq!(slot_stride(4096), 4096);
        assert_eq!(slot_stride(4097), 8192);
    }

    #[test]
    fn asking_without_a_producer_is_quiet() {
        // no ring (none runs during the tests): none, and the state stays empty for the next ask
        let state = RingState::default();
        assert!(newest(&state, 0, 0).is_none());
        assert!(state.view.lock().unwrap().is_none());
    }
}
