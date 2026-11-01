//! The staging ring: the fix for the skater build's memory spike (`docs/skater-build-memory.md`).
//!
//! At a skater build the game stages hundreds of volume textures through dedicated committed upload buffers of 10
//! to 34 MB each (one call site, `Skate.exe+0x40ec43e` returning into `+0x40eb328`: the fallback of the game's own
//! staging ring, which is far too small for a build), writes each in full, and records the copies out of a whole
//! batch of them at once: about 380 buffers, 8 GB, then the next batch, both held until the build is over. Measured
//! on 2026-10-24: 16 to 17 GB of system memory for two seconds, and on a machine near its commit limit the end of
//! the game.
//!
//! Here that call site gets a **placed buffer in an upload heap of ours** instead, carved from a ring of regions in
//! allocation order, and a region's memory is **used again once the one copy out of it has run on the GPU**: the
//! copy is seen when the game records it (the copy hook), the list that carries it when the game submits it (the
//! `ExecuteCommandLists` hook), and a fence of ours signalled on that queue right after says when it ran. The game
//! keeps a valid resource object for as long as it likes; only the memory under it is recycled, which D3D12 allows
//! for placed resources. Regions retire in order from the oldest, like the game's own ring, so the ring must hold one
//! batch plus what the next batch stages while the first batch's copies run: 10 to 12 GB stands in for the build's
//! 16 to 17 GB of committed buffers. When the ring is full, or has no heap yet, the game's own committed buffer is
//! made at once: a game thread is never made to wait here, not even a millisecond (the freezes of 2026-10-23 were our
//! waits), and the heap itself is made on our worker thread ([`tick`]), never in the hook. A second copy out of a
//! recycled region (the game re-reading a buffer, which no trace has shown) is caught and switches the ring off for
//! the rest of the run, so at worst one texture is wrong once. Buffers under 8 MB or over 64 MB, other heaps, other
//! call sites: untouched.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};
use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Graphics::Direct3D12::*;

use crate::win;

#[link(name = "kernel32")]
extern "system" {
    fn RtlCaptureStackBackTrace(skip: u32, count: u32, out: *mut *mut c_void, hash: *mut u32) -> u16;
}

/// The ring as shipped (MB; off until `mem ring on`), and its bounds.
pub const DEFAULT_MB: u64 = 1024;
pub const MIN_MB: u64 = 64;
pub const MAX_MB: u64 = 16384;
/// Only upload buffers between these sizes are taken: the game's own ring handles the small ones, and anything
/// larger than the staging buffers (10 to 34 MB) is not the pattern.
pub const MIN_BYTES: u64 = 8 << 20;
pub const MAX_BYTES: u64 = 64 << 20;
/// One D3D12 heap of the ring: the runtime refused single upload heaps of 6 GB and more (E_INVALIDARG, 2026-10-24)
/// while 2 GB went through, so the ring is made of chunks, and no region straddles one.
const CHUNK: u64 = 2048 << 20;
/// Regions start at this alignment (the placement alignment of buffers).
const ALIGN: u64 = D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64;
/// The call site, as return addresses in Skate.exe (build 20260929): the staging fallback's `CreateCommittedResource`
/// and the function that calls it.
pub const SITE: [usize; 2] = [0x40ec43e, 0x40eb328];
/// A region whose copy never ran is given up when it reaches the front after this long (the game's own release of it
/// is seen only while the trace's resource hooks are in).
const HELD_TIMEOUT_MS: u64 = 30_000;
/// Old occupants of retired regions are remembered this long (a copy out of one is a copy out of recycled memory;
/// after this the game may have freed the object and made another at the same address).
const RETIRED_MS: u64 = 10_000;
const RETIRED_KEPT: usize = 4096;

type CprFn = unsafe extern "system" fn(
    *mut c_void,
    *mut c_void,
    u64,
    *const D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_STATES,
    *const D3D12_CLEAR_VALUE,
    *const GUID,
    *mut *mut c_void,
) -> HRESULT;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    /// Handed to the game, no copy out of it seen yet.
    Held,
    /// A copy out of it is recorded on a list not yet submitted.
    Copied,
    /// The copy's list was submitted; the region is done once the queue's fence passes `fence`.
    Submitted,
    /// The game let go of it before any copy ran (or the placed resource could not be made): done.
    Done,
}

struct Region {
    id: u64,
    offset: u64,
    size: u64,
    state: State,
    /// The game's resource over it (raw), while the game holds one.
    resource: usize,
    /// When it was handed out (ms of the memory guard's clock).
    since_ms: u64,
    /// The list that carries its copy (raw), while recorded and not submitted.
    list: usize,
    /// The queue (raw) and the fence value after its copy's submission.
    queue: usize,
    fence: u64,
}

struct QueueFence {
    queue: usize,
    fence: ID3D12Fence,
    value: u64,
}

// COM objects used under the mutex only; free-threaded.
unsafe impl Send for QueueFence {}

struct Ring {
    /// The chunks, each `chunk` bytes; a region's offset is over all of them.
    heaps: Vec<ID3D12Heap>,
    chunk: u64,
    bytes: u64,
    /// Where the next region starts; the oldest live region's offset is the tail.
    head: u64,
    next_id: u64,
    /// The live regions in allocation order (the front is the oldest).
    regions: VecDeque<Region>,
    fences: Vec<QueueFence>,
    /// The game's resources over our regions: raw pointer -> region id (dropped when the region retires or the
    /// game's release is seen).
    by_res: HashMap<usize, u64>,
    /// Old occupants of retired regions (raw pointer, when), the last few thousand of the last few seconds.
    retired: VecDeque<(usize, u64)>,
    retired_set: HashSet<usize>,
}

unsafe impl Send for Ring {}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// Any upload buffer of the size qualifies (a test host has no Skate.exe call site). Refused in the game.
static ANY_SITE: AtomicBool = AtomicBool::new(false);
static WANT_MB: AtomicU64 = AtomicU64::new(DEFAULT_MB);
static RING: Mutex<Option<Ring>> = Mutex::new(None);
/// A ring is being made (on one thread at a time).
static MAKING: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicU64 = AtomicU64::new(0);
static HANDED: AtomicU64 = AtomicU64::new(0);
static RECYCLED: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE: AtomicU64 = AtomicU64::new(0);
static PEAK_USED: AtomicU64 = AtomicU64::new(0);
static FALLBACK_BIG: AtomicU64 = AtomicU64::new(0);
static FALLBACK_FULL: AtomicU64 = AtomicU64::new(0);
static FALLBACK_NO_HEAP: AtomicU64 = AtomicU64::new(0);
static FALLBACK_FAILED: AtomicU64 = AtomicU64::new(0);
static REREADS: AtomicU64 = AtomicU64::new(0);
static COPIES: AtomicU64 = AtomicU64::new(0);
static SUBMITS: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<String> = Mutex::new(String::new());

pub fn active() -> bool {
    ENABLED.load(Ordering::Relaxed) || LIVE.load(Ordering::Relaxed) > 0
}

/// Whether the game holds a resource of ours (for the release hook's fast path).
pub fn has_live() -> bool {
    LIVE.load(Ordering::Relaxed) > 0
}

impl Ring {
    fn find(&mut self, id: u64) -> Option<&mut Region> {
        self.regions.iter_mut().find(|r| r.id == id)
    }

    fn fence_passed(&self, r: &Region) -> bool {
        // SAFETY: the fence is ours, alive while the ring is
        self.fences.iter().find(|f| f.queue == r.queue).is_some_and(|f| unsafe { f.fence.GetCompletedValue() } >= r.fence)
    }

    fn used(&self) -> u64 {
        self.regions.iter().map(|r| r.size).sum()
    }

    /// Pops the regions at the front that are done with (the copy ran, the game let go before one, or none came for
    /// far too long) and retires their resources; the head goes back to the start when nothing is left.
    fn retire_front(&mut self, now: u64) {
        loop {
            let why = {
                let Some(front) = self.regions.front() else {
                    self.head = 0;
                    return;
                };
                match front.state {
                    State::Done => "",
                    State::Submitted if self.fence_passed(front) => "",
                    State::Held | State::Copied if now.saturating_sub(front.since_ms) > HELD_TIMEOUT_MS => "no copy out of it ran",
                    _ => return,
                }
            };
            let Some(r) = self.regions.pop_front() else { return };
            if !why.is_empty() {
                say(format!("staging ring: a {} MB region: {why} in {} s: its memory is used again", r.size >> 20, HELD_TIMEOUT_MS / 1000));
            }
            if r.resource != 0 {
                RECYCLED.fetch_add(1, Ordering::Relaxed);
                self.by_res.remove(&r.resource);
                self.retired.push_back((r.resource, now));
                self.retired_set.insert(r.resource);
            }
            while self.retired.len() > RETIRED_KEPT || self.retired.front().is_some_and(|&(_, at)| now.saturating_sub(at) > RETIRED_MS) {
                if let Some((old, _)) = self.retired.pop_front() {
                    self.retired_set.remove(&old);
                }
            }
        }
    }

    /// Room for `size` bytes after the head, wrapping to the start when the end is too short, and never across a
    /// chunk: the offset.
    fn place(&mut self, size: u64) -> Option<u64> {
        let chunk = self.chunk;
        if size > chunk {
            return None;
        }
        // the free space runs from the head to the tail (the oldest region), the whole ring when empty
        let tail = match self.regions.front() {
            None => {
                self.head = 0;
                None
            }
            Some(front) => Some(front.offset),
        };
        // a region that would straddle a chunk starts at the next one instead (the rest of the chunk is skipped)
        let fits = |at: u64, end: u64| -> Option<u64> {
            let at = if at % chunk + size > chunk { (at / chunk + 1) * chunk } else { at };
            (at + size <= end).then_some(at)
        };
        let at = match tail {
            None => fits(0, self.bytes)?,
            // full: the regions cover the whole ring
            Some(tail) if self.head == tail => return None,
            // the end is skipped until the tail passes it
            Some(tail) if self.head > tail => match fits(self.head, self.bytes) {
                Some(at) => at,
                None => fits(0, tail)?,
            },
            Some(tail) => fits(self.head, tail)?,
        };
        self.head = (at + size) % self.bytes;
        Some(at)
    }
}

/// `mem ring on|off [--mb N]`. From the worker thread: a new size takes effect at once when nothing of the game's is
/// on the ring, and the heap for an `on` is made here, not in a hook.
pub fn set(on: Option<bool>, mb: Option<u64>, any_site: Option<bool>) -> Value {
    if let Some(mb) = mb {
        WANT_MB.store(mb.clamp(MIN_MB, MAX_MB), Ordering::Release);
        let want = WANT_MB.load(Ordering::Relaxed) << 20;
        // a ring with nothing live on it is dropped (outside the lock), to be made again at the new size
        let old = RING.lock().ok().and_then(|mut g| {
            if let Some(r) = g.as_mut() {
                r.retire_front(crate::memguard::now_ms());
            }
            if g.as_ref().is_some_and(|r| r.regions.is_empty() && r.bytes != want) {
                g.take()
            } else {
                None
            }
        });
        if old.is_some() {
            drop(old);
            say(format!("staging ring: the heap released, to be made again at {} MB", want >> 20));
        }
    }
    if let Some(any) = any_site {
        ANY_SITE.store(any, Ordering::Release);
    }
    if let Some(on) = on {
        ENABLED.store(on, Ordering::Release);
        crate::worker::log(&format!("staging ring: {} ({} MB)", if on { "on" } else { "off" }, WANT_MB.load(Ordering::Relaxed)));
    }
    tick(crate::memguard::now_ms());
    status()
}

/// From the worker loop (our thread, every tick): the heap is made when the ring is on and has none, and released
/// when the ring is off and nothing of the game's is left on it. Never from a hook.
pub fn tick(now_ms: u64) {
    let want = ENABLED.load(Ordering::Relaxed);
    let have = RING.lock().map(|g| g.is_some()).unwrap_or(true);
    if want && !have {
        let dev = crate::memguard::device();
        if dev == 0 {
            return;
        }
        // one maker at a time: the pipe's thread and the worker loop both tick (two 2 GB heaps were made at once on
        // 2026-10-24, one of them for a size no longer wanted)
        if MAKING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return;
        }
        let raw = dev as *mut c_void;
        // SAFETY: the game's device, alive for the game's life
        let Some(device) = (unsafe { ID3D12Device::from_raw_borrowed(&raw) }) else {
            MAKING.store(false, Ordering::Release);
            return;
        };
        // SAFETY: a D3D12 call with a valid device, outside the ring's lock
        let made = unsafe { make_ring(device) };
        match made {
            Ok(ring) => {
                let (mb, chunks) = (ring.bytes >> 20, ring.heaps.len());
                // kept only when it is still the size wanted and nothing got there first; else it goes (outside the lock)
                let still = geometry(WANT_MB.load(Ordering::Acquire)).1 == ring.bytes;
                let spare = {
                    let Ok(mut g) = RING.lock() else { return };
                    if g.is_none() && still {
                        *g = Some(ring);
                        None
                    } else {
                        Some(ring)
                    }
                };
                match spare {
                    None => say(format!("staging ring: {mb} MB made as {chunks} heap(s) of {} MB", (CHUNK >> 20).min(mb))),
                    Some(r) => {
                        drop(r);
                        crate::worker::log("staging ring: a ring made for a size no longer wanted was released");
                    }
                }
            }
            Err(e) => {
                ENABLED.store(false, Ordering::Release);
                say(format!("staging ring: off: {e}"));
            }
        }
        MAKING.store(false, Ordering::Release);
    } else if !want && have {
        let old = RING.lock().ok().and_then(|mut g| {
            if let Some(r) = g.as_mut() {
                r.retire_front(now_ms);
            }
            if g.as_ref().is_some_and(|r| r.regions.is_empty() && r.by_res.is_empty()) {
                LIVE.store(0, Ordering::Release);
                g.take()
            } else {
                None
            }
        });
        if old.is_some() {
            drop(old);
            say("staging ring: off, the heap released".into());
        }
    }
}

fn say(line: String) {
    crate::worker::log(&line);
    if let Ok(mut l) = LAST.lock() {
        *l = line;
    }
}

/// Whether this allocation comes from the staging fallback (the two return addresses), unless any site will do.
unsafe fn from_site() -> bool {
    if ANY_SITE.load(Ordering::Relaxed) {
        return true;
    }
    let base = win::GetModuleHandleW(std::ptr::null()) as usize;
    if base == 0 {
        return false;
    }
    let mut frames = [std::ptr::null_mut::<c_void>(); 6];
    let n = RtlCaptureStackBackTrace(1, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) as usize;
    let want = [base + SITE[0], base + SITE[1]];
    frames[..n].iter().any(|&f| want.contains(&(f as usize)))
}

/// The ring's chunk size and total for a wanted size (MB): chunks of [`CHUNK`] at most, the total rounded up to
/// whole chunks.
fn geometry(want_mb: u64) -> (u64, u64) {
    let want = want_mb << 20;
    let chunk = CHUNK.min(want);
    (chunk, want.div_ceil(chunk) * chunk)
}

/// Makes the heaps at the wanted size, with the memory watch's and the machine's leave (an upload heap is system
/// memory: the non-local budget and the commit limit are what matter).
unsafe fn make_ring(device: &ID3D12Device) -> Result<Ring, String> {
    let (chunk, bytes) = geometry(WANT_MB.load(Ordering::Acquire));
    if crate::memwatch::measured() {
        crate::memwatch::room_for_system(bytes)?;
    }
    let free = crate::memguard::commit_free();
    if free < bytes + (2 << 30) {
        return Err(format!("the machine has {} MB of commit left, the ring wants {} MB and 2 GB spare", free >> 20, bytes >> 20));
    }
    let desc = D3D12_HEAP_DESC {
        SizeInBytes: chunk,
        Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() },
        Alignment: ALIGN,
        Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS | D3D12_HEAP_FLAG_CREATE_NOT_ZEROED,
    };
    let n = bytes / chunk;
    let mut heaps = Vec::with_capacity(n as usize);
    for k in 0..n {
        let mut heap: Option<ID3D12Heap> = None;
        device.CreateHeap(&desc, &mut heap).map_err(|e| format!("the ring's heap {} of {n} ({} MB): {e}", k + 1, chunk >> 20))?;
        let heap = heap.ok_or("no heap")?;
        let _ = heap.SetName(windows::core::w!("Caveman FX staging ring"));
        heaps.push(heap);
    }
    Ok(Ring {
        heaps,
        chunk,
        bytes,
        head: 0,
        next_id: 1,
        regions: VecDeque::new(),
        fences: Vec::new(),
        by_res: HashMap::new(),
        retired: VecDeque::new(),
        retired_set: HashSet::new(),
    })
}

/// From the device's `CreateCommittedResource` hook, before the original: Some(result) when this allocation was
/// served from the ring, None to let the game's own call go ahead.
///
/// # Safety
/// The arguments are the hook's, valid for the call.
#[allow(clippy::too_many_arguments)]
pub unsafe fn intercept(
    device: *mut c_void,
    heap: *const D3D12_HEAP_PROPERTIES,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    riid: *const GUID,
    ppv: *mut *mut c_void,
    placed: usize,
) -> Option<HRESULT> {
    if !ENABLED.load(Ordering::Relaxed) || heap.is_null() || desc.is_null() || ppv.is_null() || crate::overlay::stopping() {
        return None;
    }
    let d = &*desc;
    if (*heap).Type != D3D12_HEAP_TYPE_UPLOAD || d.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || d.Width < MIN_BYTES {
        return None;
    }
    if d.Width > MAX_BYTES {
        FALLBACK_BIG.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    if !from_site() {
        return None;
    }
    let create: CprFn = if placed != 0 {
        std::mem::transmute::<usize, CprFn>(placed)
    } else {
        std::mem::transmute::<usize, CprFn>(win::read_ptr(win::read_ptr(device as usize)? + std::mem::offset_of!(ID3D12Device_Vtbl, CreatePlacedResource))?)
    };
    let now = crate::memguard::now_ms();
    let size = (d.Width + ALIGN - 1) & !(ALIGN - 1);
    // a region reserved under the lock, the resource made outside it; no room (or no heap yet): the game's own path
    // at once. A game thread is never made to wait here, not even a millisecond: the freezes of 2026-10-23 (17:53,
    // 18:06) were our waits, and a ring that cannot keep up costs the game nothing this way.
    let (id, offset, heap_raw, chunk) = {
        let mut guard = RING.lock().ok()?;
        let Some(ring) = guard.as_mut() else {
            drop(guard);
            let n = FALLBACK_NO_HEAP.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 3 {
                say(format!("staging ring: no heap yet (the worker makes it): the game's own path for a {} MB buffer", d.Width >> 20));
            }
            return None;
        };
        ring.retire_front(now);
        match ring.place(size) {
            Some(offset) => {
                let id = ring.next_id;
                ring.next_id += 1;
                ring.regions.push_back(Region { id, offset, size, state: State::Held, resource: 0, since_ms: now, list: 0, queue: 0, fence: 0 });
                (id, offset, ring.heaps[(offset / ring.chunk) as usize].as_raw(), ring.chunk)
            }
            None => {
                drop(guard);
                let full = FALLBACK_FULL.fetch_add(1, Ordering::Relaxed) + 1;
                if full <= 3 || full.is_multiple_of(200) {
                    say(format!("staging ring: full: the game's own path for a {} MB buffer ({full} so far)", d.Width >> 20));
                }
                return None;
            }
        }
    };
    let hr = create(device, heap_raw, offset % chunk, desc, state, clear, riid, ppv);
    let res = if hr.is_ok() && !ppv.is_null() { *ppv as usize } else { 0 };
    let mut guard = RING.lock().ok()?;
    let ring = guard.as_mut()?;
    if res == 0 {
        if let Some(r) = ring.find(id) {
            r.state = State::Done;
        }
        FALLBACK_FAILED.fetch_add(1, Ordering::Relaxed);
        drop(guard);
        say(format!("staging ring: a placed buffer of {} MB failed ({hr:?}): the game's own path", d.Width >> 20));
        return None;
    }
    if let Some(r) = ring.find(id) {
        r.resource = res;
    }
    ring.by_res.insert(res, id);
    LIVE.store(ring.by_res.len() as u64, Ordering::Release);
    PEAK_LIVE.fetch_max(ring.regions.len() as u64, Ordering::Relaxed);
    PEAK_USED.fetch_max(ring.used(), Ordering::Relaxed);
    let handed = HANDED.fetch_add(1, Ordering::Relaxed);
    drop(guard);
    if handed < 3 {
        crate::worker::log(&format!("staging ring: a region at {} MB handed out as {res:#x} ({} MB asked)", offset >> 20, d.Width >> 20));
    }
    Some(hr)
}

/// From the copy hook: the game recorded a copy out of `src` on `list`.
pub fn note_copy(list: usize, src: usize) {
    let Ok(mut guard) = RING.lock() else { return };
    let Some(ring) = guard.as_mut() else { return };
    let Some(&id) = ring.by_res.get(&src) else {
        if ring.retired_set.contains(&src) {
            REREADS.fetch_add(1, Ordering::Relaxed);
            if ENABLED.swap(false, Ordering::AcqRel) {
                say(format!(
                    "staging ring: a copy out of a retired buffer ({src:#x}, its memory handed on): the ring is off from here on, the game's own path"
                ));
            }
        }
        return;
    };
    let Some(r) = ring.find(id) else { return };
    match r.state {
        State::Held => {
            r.state = State::Copied;
            r.list = list;
            COPIES.fetch_add(1, Ordering::Relaxed);
        }
        State::Copied if r.list == list => {}
        other => {
            // a second copy out of a buffer: not the pattern the ring is for
            REREADS.fetch_add(1, Ordering::Relaxed);
            if ENABLED.swap(false, Ordering::AcqRel) {
                say(format!(
                    "staging ring: a second copy out of a {} MB region (state {other:?}): the ring is off from here on, the game's own path",
                    r.size >> 20
                ));
            }
        }
    }
}

/// From the `ExecuteCommandLists` hook, after the submission: the regions whose copies these lists carry wait for
/// the queue's fence from here.
///
/// # Safety
/// `queue` is the queue that was just submitted to; `lists` what it took.
pub unsafe fn on_execute(queue: *mut c_void, lists: &[*mut c_void]) {
    let q = queue as usize;
    // the regions these lists carry copies of, and the queue's fence (made outside the lock when there is none yet:
    // D3D12 calls may come back through our release hook, which takes the lock)
    let (hit, fence, value) = {
        let Ok(mut guard) = RING.lock() else { return };
        let Some(ring) = guard.as_mut() else { return };
        let hit: Vec<u64> = ring.regions.iter().filter(|r| r.state == State::Copied && lists.iter().any(|&l| l as usize == r.list)).map(|r| r.id).collect();
        if hit.is_empty() {
            return;
        }
        match ring.fences.iter_mut().find(|f| f.queue == q) {
            Some(f) => {
                f.value += 1;
                (hit, Some(f.fence.clone()), f.value)
            }
            None => (hit, None, 0),
        }
    };
    let (fence, value) = match fence {
        Some(f) => (f, value),
        None => {
            let Some(queue_ref) = ID3D12CommandQueue::from_raw_borrowed(&queue) else { return };
            let mut dev: Option<ID3D12Device> = None;
            if queue_ref.GetDevice(&mut dev).is_err() {
                return;
            }
            let Some(dev) = dev else { return };
            let Ok(fence) = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE) else { return };
            let Ok(mut guard) = RING.lock() else { return };
            let Some(ring) = guard.as_mut() else { return };
            let f = match ring.fences.iter_mut().find(|f| f.queue == q) {
                Some(f) => f,
                None => {
                    ring.fences.push(QueueFence { queue: q, fence, value: 0 });
                    ring.fences.last_mut().unwrap()
                }
            };
            f.value += 1;
            (f.fence.clone(), f.value)
        }
    };
    let Some(queue_ref) = ID3D12CommandQueue::from_raw_borrowed(&queue) else { return };
    if queue_ref.Signal(&fence, value).is_err() {
        return;
    }
    let n = SUBMITS.fetch_add(1, Ordering::Relaxed);
    if n < 3 {
        crate::worker::log(&format!("staging ring: {} region(s) submitted on queue {q:#x}, fence {value}", hit.len()));
    }
    let Ok(mut guard) = RING.lock() else { return };
    let Some(ring) = guard.as_mut() else { return };
    for id in hit {
        if let Some(r) = ring.find(id) {
            if r.state == State::Copied {
                r.state = State::Submitted;
                r.queue = q;
                r.fence = value;
            }
        }
    }
}

/// From the release hook: the game's last reference to `res` went. True when it was over a live region of ours.
pub fn on_release(res: usize) -> bool {
    let Ok(mut guard) = RING.lock() else { return false };
    let Some(ring) = guard.as_mut() else { return false };
    // an old occupant the game let go of: another object may take its address, so a copy out of it is no reread
    ring.retired_set.remove(&res);
    let Some(id) = ring.by_res.remove(&res) else { return false };
    if let Some(r) = ring.find(id) {
        r.resource = 0;
        // never copied, or copied but not submitted: done (a submitted one still waits for its fence)
        if matches!(r.state, State::Held | State::Copied) {
            r.state = State::Done;
        }
    }
    LIVE.store(ring.by_res.len() as u64, Ordering::Release);
    true
}

/// Whether `res` is a resource of ours the game holds.
pub fn holds(res: usize) -> bool {
    RING.lock().map(|g| g.as_ref().is_some_and(|r| r.by_res.contains_key(&res))).unwrap_or(false)
}

pub fn status() -> Value {
    let (heap_mb, heaps, regions, used_mb, held, copied, submitted, done, head_mb, tail_mb) = RING
        .lock()
        .ok()
        .and_then(|g| {
            g.as_ref().map(|r| {
                let count = |st: State| r.regions.iter().filter(|x| x.state == st).count();
                (
                    r.bytes >> 20,
                    r.heaps.len(),
                    r.regions.len(),
                    r.used() >> 20,
                    count(State::Held),
                    count(State::Copied),
                    count(State::Submitted),
                    count(State::Done),
                    r.head >> 20,
                    r.regions.front().map(|f| f.offset >> 20).unwrap_or(0),
                )
            })
        })
        .unwrap_or((0, 0, 0, 0, 0, 0, 0, 0, 0, 0));
    json!({
        "enabled": ENABLED.load(Ordering::Relaxed),
        "any_site": ANY_SITE.load(Ordering::Relaxed),
        "mb": WANT_MB.load(Ordering::Relaxed),
        "heap_mb": heap_mb,
        "heaps": heaps,
        "regions": regions,
        "used_mb": used_mb,
        "held": held, "copied": copied, "submitted": submitted, "done": done,
        "head_mb": head_mb, "tail_mb": tail_mb,
        "live": LIVE.load(Ordering::Relaxed),
        "peak_live": PEAK_LIVE.load(Ordering::Relaxed),
        "peak_used_mb": PEAK_USED.load(Ordering::Relaxed) >> 20,
        "handed": HANDED.load(Ordering::Relaxed),
        "recycled": RECYCLED.load(Ordering::Relaxed),
        "copies": COPIES.load(Ordering::Relaxed),
        "submits": SUBMITS.load(Ordering::Relaxed),
        "fallback_big": FALLBACK_BIG.load(Ordering::Relaxed),
        "fallback_full": FALLBACK_FULL.load(Ordering::Relaxed),
        "fallback_no_heap": FALLBACK_NO_HEAP.load(Ordering::Relaxed),
        "fallback_failed": FALLBACK_FAILED.load(Ordering::Relaxed),
        "rereads": REREADS.load(Ordering::Relaxed),
        "last": LAST.lock().map(|l| l.clone()).unwrap_or_default(),
    })
}

/// At unload: nothing more is handed out; the heap is released only when the game holds nothing on it (the game's
/// placed resources keep it alive otherwise).
pub fn shutdown() {
    ENABLED.store(false, Ordering::Release);
    let old = RING.lock().ok().and_then(|mut g| if g.as_ref().is_some_and(|r| r.by_res.is_empty()) { g.take() } else { None });
    drop(old);
}
