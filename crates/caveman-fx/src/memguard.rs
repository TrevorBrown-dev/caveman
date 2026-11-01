//! The memory guard (`docs/fx-audit.md`, the other half of guardrail 1): the game's own allocations, through the
//! device's `CreateCommittedResource` (and its 1, 2 and 3 forms) and `CreateHeap` (and 1), hooked on the device's
//! vtable as the `CreateCommandList` hooks are. The game has no second try: an `E_OUTOFMEMORY` from one of these
//! (seen twice on 2026-10-22, at a respawn, when the game's own use spikes by some 16 GB while the old skater's
//! resources are still held) is a crash. Here the call is made again every little while for up to [`RETRY_FOR_MS`],
//! the calling thread blocked (the game frozen, not dead): the old resources go on other threads meanwhile, and the
//! OS grows the page file, so a later try can go through. Each episode is logged with what was asked (heap, size,
//! format) and how it ended, and for [`SQUEEZE_MS`] after one the DLL allocates nothing of its own
//! ([`crate::memwatch::room_for`]).
//!
//! The same hooks (plus `CreatePlacedResource`) feed the allocation trace ([`crate::alloctrace`]): every call's size,
//! heap, format, time taken and call stack while a trace runs.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;

use crate::vhook::VSlot;

/// How long a failing allocation is tried again before the game gets its error.
pub const RETRY_FOR_MS: u64 = 45_000;
/// How long after an episode the DLL allocates nothing of its own.
pub const SQUEEZE_MS: u64 = 15_000;
/// No retry while the machine itself has less than this much commit left: waiting then only keeps the game's
/// memory held while everything else on the PC starves (a hard freeze of the whole machine followed a session of
/// retries under an exhausted commit limit, 2026-10-23 12:49). The game gets its error as it would have.
pub const COMMIT_FLOOR: u64 = 768 << 20;
const E_OUTOFMEMORY: HRESULT = HRESULT(0x8007000Eu32 as i32);

#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended: u64,
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
}

/// The machine's commit charge left (bytes; u64::MAX when it cannot be asked).
pub fn commit_free() -> u64 {
    let mut s = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended: 0,
    };
    // SAFETY: a plain Win32 call into our own struct, sized as told
    if unsafe { GlobalMemoryStatusEx(&mut s) } == 0 {
        return u64::MAX;
    }
    s.avail_page_file
}

static CCR: VSlot = VSlot::new();
static CCR1: VSlot = VSlot::new();
static CCR2: VSlot = VSlot::new();
static CCR3: VSlot = VSlot::new();
static CH: VSlot = VSlot::new();
static CH1: VSlot = VSlot::new();
static CPR: VSlot = VSlot::new();

static CALLS: AtomicU64 = AtomicU64::new(0);
static EPISODES: AtomicU64 = AtomicU64::new(0);
static RETRIES: AtomicU64 = AtomicU64::new(0);
static RESCUED: AtomicU64 = AtomicU64::new(0);
static GAVE_UP: AtomicU64 = AtomicU64::new(0);
/// The game's device (raw), from the last hooked call: for work of ours that needs it on our own thread (the staging
/// ring's heap).
static DEVICE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The game's device as last seen by the hooks (0: none yet). Not a reference of ours: the device outlives the DLL.
pub fn device() -> usize {
    DEVICE.load(Ordering::Acquire)
}
/// When the last episode began (ms of [`now_ms`]; 0: none yet).
static SQUEEZE_AT: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<String> = Mutex::new(String::new());
static START: OnceLock<Instant> = OnceLock::new();

/// Milliseconds of a clock that starts with the first allocation seen.
pub(crate) fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

type CcrFn = unsafe extern "system" fn(
    *mut c_void,
    *const D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_FLAGS,
    *const D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_STATES,
    *const D3D12_CLEAR_VALUE,
    *const GUID,
    *mut *mut c_void,
) -> HRESULT;
type Ccr1Fn = unsafe extern "system" fn(
    *mut c_void,
    *const D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_FLAGS,
    *const D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_STATES,
    *const D3D12_CLEAR_VALUE,
    *mut c_void,
    *const GUID,
    *mut *mut c_void,
) -> HRESULT;
type Ccr2Fn = unsafe extern "system" fn(
    *mut c_void,
    *const D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_FLAGS,
    *const D3D12_RESOURCE_DESC1,
    D3D12_RESOURCE_STATES,
    *const D3D12_CLEAR_VALUE,
    *mut c_void,
    *const GUID,
    *mut *mut c_void,
) -> HRESULT;
type Ccr3Fn = unsafe extern "system" fn(
    *mut c_void,
    *const D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_FLAGS,
    *const D3D12_RESOURCE_DESC1,
    D3D12_BARRIER_LAYOUT,
    *const D3D12_CLEAR_VALUE,
    *mut c_void,
    u32,
    *const DXGI_FORMAT,
    *const GUID,
    *mut *mut c_void,
) -> HRESULT;
type ChFn = unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_DESC, *const GUID, *mut *mut c_void) -> HRESULT;
type Ch1Fn = unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_DESC, *mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
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

/// What an allocation was, for the log and the trace. `heap`: the D3D12 heap type (1 default, 2 upload, 3 readback,
/// 4 custom; 9 a placed resource, whose heap is the game's; 0 unknown); `kind`: 0 buffer, 1 to 3 a texture of that
/// many dimensions, 8 a heap.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct AllocInfo {
    pub heap: u8,
    pub kind: u8,
    pub bytes: u64,
    pub w: u64,
    pub h: u32,
    pub d: u16,
    pub mips: u16,
    pub format: u32,
    pub flags: u32,
}

pub fn size(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KB", bytes >> 10)
    }
}

pub fn heap_name(t: u8) -> &'static str {
    match t {
        1 => "default (video memory)",
        2 => "upload (system memory)",
        3 => "readback (system memory)",
        4 => "custom",
        9 => "a heap of the game's (placed)",
        _ => "?",
    }
}

impl AllocInfo {
    pub fn describe(&self) -> String {
        match self.kind {
            8 => format!("a heap of {} ({})", size(self.bytes), heap_name(self.heap)),
            0 => format!("buffer of {} on the {} heap", size(self.bytes), heap_name(self.heap)),
            k => format!(
                "texture {}D {}x{}x{} with {} mip(s), format {}, {} on the {} heap",
                k,
                self.w,
                self.h,
                self.d,
                self.mips,
                self.format,
                size(self.bytes),
                heap_name(self.heap)
            ),
        }
    }
}

unsafe fn heap_type_of(heap: *const D3D12_HEAP_PROPERTIES) -> u8 {
    if heap.is_null() {
        0
    } else {
        (*heap).Type.0 as u8
    }
}

/// What a resource is: its kind, size, format and bytes (asked of the device).
unsafe fn info_resource(device: *mut c_void, heap: u8, desc: Option<D3D12_RESOURCE_DESC>) -> AllocInfo {
    let Some(desc) = desc else { return AllocInfo { heap, ..Default::default() } };
    let kind = if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
        0
    } else if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE1D {
        1
    } else if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        2
    } else if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D {
        3
    } else {
        0
    };
    let bytes = ID3D12Device::from_raw_borrowed(&device).map(|d| d.GetResourceAllocationInfo(0, &[desc]).SizeInBytes).unwrap_or(0);
    let bytes = if kind == 0 { bytes.max(desc.Width) } else { bytes };
    AllocInfo {
        heap,
        kind,
        bytes,
        w: desc.Width,
        h: desc.Height,
        d: desc.DepthOrArraySize,
        mips: desc.MipLevels,
        format: desc.Format.0 as u32,
        flags: desc.Flags.0 as u32,
    }
}

unsafe fn desc_of(desc: *const D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_DESC> {
    if desc.is_null() {
        None
    } else {
        Some(*desc)
    }
}

unsafe fn desc1_of(desc: *const D3D12_RESOURCE_DESC1) -> Option<D3D12_RESOURCE_DESC> {
    if desc.is_null() {
        return None;
    }
    let d = &*desc;
    Some(D3D12_RESOURCE_DESC {
        Dimension: d.Dimension,
        Alignment: d.Alignment,
        Width: d.Width,
        Height: d.Height,
        DepthOrArraySize: d.DepthOrArraySize,
        MipLevels: d.MipLevels,
        Format: d.Format,
        SampleDesc: d.SampleDesc,
        Layout: d.Layout,
        Flags: d.Flags,
    })
}

unsafe fn info_heap(desc: *const D3D12_HEAP_DESC) -> AllocInfo {
    if desc.is_null() {
        return AllocInfo { kind: 8, ..Default::default() };
    }
    let d = &*desc;
    AllocInfo { heap: d.Properties.Type.0 as u8, kind: 8, bytes: d.SizeInBytes, flags: d.Flags.0 as u32, ..Default::default() }
}

/// Makes the call, timed; the trace takes it; on `E_OUTOFMEMORY` makes it again until it goes through or the window
/// passes. `out` is the call's result pointer (the resource or heap made), for the trace.
unsafe fn guarded(what: &'static str, info: impl Fn() -> AllocInfo, mut call: impl FnMut() -> HRESULT, out: *mut *mut c_void) -> HRESULT {
    CALLS.fetch_add(1, Ordering::Relaxed);
    let t0 = Instant::now();
    let mut r = call();
    if crate::alloctrace::tracing() {
        crate::alloctrace::record(what, &info(), t0.elapsed().as_secs_f32() * 1000.0, r, out);
    }
    if r != E_OUTOFMEMORY || crate::overlay::stopping() {
        return r;
    }
    let started = Instant::now();
    let asked = info().describe();
    EPISODES.fetch_add(1, Ordering::Relaxed);
    SQUEEZE_AT.store(now_ms().max(1), Ordering::Release);
    let free = commit_free();
    if free < COMMIT_FLOOR {
        GAVE_UP.fetch_add(1, Ordering::Relaxed);
        let line = format!(
            "memory guard: {what} came back out of memory ({asked}) with the machine's commit exhausted ({} MB left): not waiting, the game gets its error",
            free >> 20
        );
        crate::worker::log(&line);
        if let Ok(mut l) = LAST.lock() {
            *l = line;
        }
        return r;
    }
    crate::worker::log(&format!("memory guard: {what} came back out of memory ({asked}; the machine has {} MB of commit left): trying again for up to {} s, the game paused on this thread meanwhile", free >> 20, RETRY_FOR_MS / 1000));
    let mut tries = 0u64;
    let mut wait = 10u64;
    loop {
        std::thread::sleep(Duration::from_millis(wait));
        if commit_free() < COMMIT_FLOOR {
            GAVE_UP.fetch_add(1, Ordering::Relaxed);
            let line = format!(
                "memory guard: {what}: the machine's commit ran out while waiting ({} tries): not waiting on, the game gets its error ({asked})",
                tries + 1
            );
            crate::worker::log(&line);
            if let Ok(mut l) = LAST.lock() {
                *l = line;
            }
            return r;
        }
        wait = (wait * 2).min(250);
        tries += 1;
        RETRIES.fetch_add(1, Ordering::Relaxed);
        r = call();
        let ms = started.elapsed().as_millis() as u64;
        let more = if tries == 1 { "1 more try".to_string() } else { format!("{tries} more tries") };
        let line = if r.is_ok() {
            RESCUED.fetch_add(1, Ordering::Relaxed);
            format!("memory guard: {what} went through after {more} ({ms} ms): {asked}")
        } else if r != E_OUTOFMEMORY {
            format!("memory guard: {what} came back with another error ({r:?}) after {more} ({ms} ms): {asked}")
        } else if ms >= RETRY_FOR_MS || crate::overlay::stopping() {
            GAVE_UP.fetch_add(1, Ordering::Relaxed);
            format!("memory guard: {what} still out of memory after {more} ({ms} ms): giving up, the game gets the error ({asked})")
        } else {
            continue;
        };
        crate::worker::log(&line);
        if let Ok(mut l) = LAST.lock() {
            *l = line;
        }
        return r;
    }
}

unsafe extern "system" fn create_committed_resource(
    this: *mut c_void,
    heap: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    DEVICE.store(this as usize, Ordering::Release);
    // the staging ring takes the skater build's big upload buffers (stagering.rs)
    if crate::stagering::active() {
        let t0 = Instant::now();
        if let Some(r) = crate::stagering::intercept(this, heap, desc, state, clear, riid, ppv, CPR.original()) {
            CALLS.fetch_add(1, Ordering::Relaxed);
            if crate::alloctrace::tracing() {
                crate::alloctrace::record("StagingRing", &info_resource(this, heap_type_of(heap), desc_of(desc)), t0.elapsed().as_secs_f32() * 1000.0, r, ppv);
            }
            crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
            return r;
        }
    }
    let f: CcrFn = std::mem::transmute::<usize, CcrFn>(CCR.original());
    let r = guarded(
        "CreateCommittedResource",
        || info_resource(this, heap_type_of(heap), desc_of(desc)),
        || f(this, heap, flags, desc, state, clear, riid, ppv),
        ppv,
    );
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn create_committed_resource1(
    this: *mut c_void,
    heap: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    session: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: Ccr1Fn = std::mem::transmute::<usize, Ccr1Fn>(CCR1.original());
    let r = guarded(
        "CreateCommittedResource1",
        || info_resource(this, heap_type_of(heap), desc_of(desc)),
        || f(this, heap, flags, desc, state, clear, session, riid, ppv),
        ppv,
    );
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn create_committed_resource2(
    this: *mut c_void,
    heap: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC1,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    session: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: Ccr2Fn = std::mem::transmute::<usize, Ccr2Fn>(CCR2.original());
    let r = guarded(
        "CreateCommittedResource2",
        || info_resource(this, heap_type_of(heap), desc1_of(desc)),
        || f(this, heap, flags, desc, state, clear, session, riid, ppv),
        ppv,
    );
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn create_committed_resource3(
    this: *mut c_void,
    heap: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC1,
    layout: D3D12_BARRIER_LAYOUT,
    clear: *const D3D12_CLEAR_VALUE,
    session: *mut c_void,
    castable: u32,
    formats: *const DXGI_FORMAT,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: Ccr3Fn = std::mem::transmute::<usize, Ccr3Fn>(CCR3.original());
    let r = guarded(
        "CreateCommittedResource3",
        || info_resource(this, heap_type_of(heap), desc1_of(desc)),
        || f(this, heap, flags, desc, layout, clear, session, castable, formats, riid, ppv),
        ppv,
    );
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn create_heap(this: *mut c_void, desc: *const D3D12_HEAP_DESC, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: ChFn = std::mem::transmute::<usize, ChFn>(CH.original());
    let r = guarded("CreateHeap", || info_heap(desc), || f(this, desc, riid, ppv), ppv);
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn create_heap1(
    this: *mut c_void,
    desc: *const D3D12_HEAP_DESC,
    session: *mut c_void,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: Ch1Fn = std::mem::transmute::<usize, Ch1Fn>(CH1.original());
    let r = guarded("CreateHeap1", || info_heap(desc), || f(this, desc, session, riid, ppv), ppv);
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn create_placed_resource(
    this: *mut c_void,
    heap: *mut c_void,
    offset: u64,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: CprFn = std::mem::transmute::<usize, CprFn>(CPR.original());
    let r = guarded("CreatePlacedResource", || info_resource(this, 9, desc_of(desc)), || f(this, heap, offset, desc, state, clear, riid, ppv), ppv);
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

/// The slots, our functions and their names, for the overlay's install and uninstall (the vtable interface each
/// slot belongs to decides whether it exists: the device must have it).
fn table() -> [(&'static VSlot, usize, usize, &'static str, u8); 7] {
    [
        (
            &CCR,
            std::mem::offset_of!(ID3D12Device_Vtbl, CreateCommittedResource) / 8,
            create_committed_resource as *const () as usize,
            "CreateCommittedResource",
            0,
        ),
        (
            &CCR1,
            std::mem::offset_of!(ID3D12Device4_Vtbl, CreateCommittedResource1) / 8,
            create_committed_resource1 as *const () as usize,
            "CreateCommittedResource1",
            4,
        ),
        (
            &CCR2,
            std::mem::offset_of!(ID3D12Device8_Vtbl, CreateCommittedResource2) / 8,
            create_committed_resource2 as *const () as usize,
            "CreateCommittedResource2",
            8,
        ),
        (
            &CCR3,
            std::mem::offset_of!(ID3D12Device10_Vtbl, CreateCommittedResource3) / 8,
            create_committed_resource3 as *const () as usize,
            "CreateCommittedResource3",
            10,
        ),
        (&CH, std::mem::offset_of!(ID3D12Device_Vtbl, CreateHeap) / 8, create_heap as *const () as usize, "CreateHeap", 0),
        (&CH1, std::mem::offset_of!(ID3D12Device4_Vtbl, CreateHeap1) / 8, create_heap1 as *const () as usize, "CreateHeap1", 4),
        (&CPR, std::mem::offset_of!(ID3D12Device_Vtbl, CreatePlacedResource) / 8, create_placed_resource as *const () as usize, "CreatePlacedResource", 0),
    ]
}

/// For the overlay's `ours()`: every slot of ours (installed or not).
pub fn hooks() -> Vec<(&'static VSlot, usize, &'static str)> {
    table().into_iter().map(|(vs, _, f, what, _)| (vs, f, what)).collect()
}

/// Which device interfaces the game's device class has (ID3D12Device4, 8, 10), from the dummy device.
pub fn interfaces(device: &ID3D12Device) -> [bool; 3] {
    [device.cast::<ID3D12Device4>().is_ok(), device.cast::<ID3D12Device8>().is_ok(), device.cast::<ID3D12Device10>().is_ok()]
}

/// Hooks the slots on the device's vtable (from the overlay's install). A note for the install summary.
///
/// # Safety
/// `device_vt` must be the vtable of the process's D3D12 device class, and `has` what [`interfaces`] said of it.
pub unsafe fn install(device_vt: usize, has: [bool; 3]) -> String {
    let mut hooked = Vec::new();
    let mut failed = Vec::new();
    for (vs, slot, f, what, needs) in table() {
        let present = match needs {
            0 => true,
            4 => has[0],
            8 => has[1],
            _ => has[2],
        };
        if !present {
            continue;
        }
        match vs.install(device_vt + slot * 8, f) {
            Ok(()) => hooked.push(what),
            Err(e) => failed.push(format!("{what}: {e}")),
        }
    }
    let mut note = format!("; memory guard on {}", if hooked.is_empty() { "nothing".to_string() } else { hooked.join(", ") });
    if !failed.is_empty() {
        note.push_str(&format!(" (not hooked: {})", failed.join("; ")));
    }
    note
}

/// While the game's last out-of-memory episode is recent: why nothing of ours may allocate.
pub fn squeeze_note() -> Option<String> {
    let at = SQUEEZE_AT.load(Ordering::Acquire);
    if at == 0 {
        return None;
    }
    let ago = now_ms().saturating_sub(at);
    (ago < SQUEEZE_MS)
        .then(|| format!("the game ran out of memory {:.1} s ago: nothing of ours is allocated for {} s after that", ago as f64 / 1000.0, SQUEEZE_MS / 1000))
}

/// For `draw status`.
pub fn status() -> Value {
    let hooked: Vec<&str> = table().into_iter().filter(|(vs, ..)| vs.installed()).map(|(_, _, _, what, _)| what).collect();
    json!({
        "hooked": hooked,
        "calls": CALLS.load(Ordering::Relaxed),
        "episodes": EPISODES.load(Ordering::Relaxed),
        "retries": RETRIES.load(Ordering::Relaxed),
        "rescued": RESCUED.load(Ordering::Relaxed),
        "gave_up": GAVE_UP.load(Ordering::Relaxed),
        "retry_for_ms": RETRY_FOR_MS,
        "squeezed": squeeze_note().is_some(),
        "last": LAST.lock().map(|l| l.clone()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slots_are_the_known_ones() {
        // ID3D12Device: IUnknown (3), ID3D12Object (4), then GetNodeCount, CreateCommandQueue, CreateCommandAllocator,
        // CreateGraphicsPipelineState, CreateComputePipelineState, CreateCommandList, CheckFeatureSupport,
        // CreateDescriptorHeap, GetDescriptorHandleIncrementSize, CreateRootSignature, CreateConstantBufferView,
        // CreateShaderResourceView, CreateUnorderedAccessView, CreateRenderTargetView, CreateDepthStencilView,
        // CreateSampler, CopyDescriptors, CopyDescriptorsSimple, GetResourceAllocationInfo, GetCustomHeapProperties,
        // CreateCommittedResource (27), CreateHeap (28), CreatePlacedResource (29)
        let t = table();
        assert_eq!(t[0].1, 27);
        assert_eq!(t[4].1, 28);
        assert_eq!(t[6].1, 29);
        assert_eq!(hooks().len(), 7);
    }

    #[test]
    fn sizes_and_descriptions_read_well() {
        assert_eq!(size(512), "0 KB");
        assert_eq!(size(300 << 10), "300 KB");
        assert_eq!(size(1536 << 10), "1.5 MB");
        let b = AllocInfo { heap: 2, kind: 0, bytes: 35 << 20, ..Default::default() };
        assert_eq!(b.describe(), "buffer of 35.0 MB on the upload (system memory) heap");
        let t = AllocInfo { heap: 1, kind: 2, bytes: 1 << 20, w: 512, h: 2048, d: 1, mips: 12, format: 29, flags: 0 };
        assert!(t.describe().starts_with("texture 2D 512x2048x1 with 12 mip(s), format 29, 1.0 MB on the default"));
    }
}
