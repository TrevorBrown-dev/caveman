//! The game's depth, so the bars hide behind the scene (the skater's legs and shoes, the deck, the wheels).
//!
//! **Finding it.** No code is patched: slots of D3D12Core's command list tables are swapped for hooks that call
//! through (the shared table, learnt from a dummy list of ours, and each list's own table: see "the game's lists' own
//! tables" below): `ResourceBarrier`, `RSSetViewports`, `OMSetRenderTargets`, `Reset` and, when the class has it,
//! `Barrier` (enhanced barriers; only counted). A transition of a resource out of DEPTH_WRITE is the end of a depth
//! pass; `GetDesc` says what the resource is. The main view's depth is the largest 2D, single-sample depth target with
//! the back buffer's aspect (within 3 %) and at least a quarter of its width that ended a pass recently; kept while it
//! goes on ending passes, and replaced within two presents when it stops (the game made it anew).
//!
//! **Reading it.** The depth is not read at Present: by then Frostbite may have reused its memory for other transient
//! resources, and its state there is only known by replaying every list in submission order. Instead, right after the
//! game's own barrier takes the main depth out of DEPTH_WRITE for the first time in a frame (the G-buffer's pass end;
//! later ones are left alone), in the same command list (on the game's recording thread, inside our hook, through the
//! original functions), we record: the depth's subresource 0 (the depth plane) from the state the game just gave it to
//! COPY_SOURCE, a copy into a texture of our own, and back to the game's state. The depth is left exactly as the game
//! expects, in the state it really has (it was set by the barrier just before ours). Our copies: a ring of 4 textures
//! (R32_FLOAT, or R16_UNORM for a 16-bit depth), in PIXEL_SHADER_RESOURCE between copies. The copies submitted
//! (`ExecuteCommandLists`, in submission order) are kept with the present window they were recorded in and the frame's
//! render viewport (dynamic resolution: see "the frame's render viewport" below); at Present the overlay reads the
//! frame's own, or when there is none (a window with no G-buffer pass end of the right state: see "which pass end is
//! the G-buffer's"), the last good one in the camera it was rendered with ("which frame a copy belongs to"). A copy
//! never waits on, or is skipped for, the overlay: the copies' lock is only held for moments.
//!
//! **Its scale.** The game's own projection: reversed depth, near plane 0.06 m ([`GAME_NEAR`]; read from the game's
//! memory). A check on the deck ([`Calibration`]) tells whether the texels are right, and which way of finding them
//! (the frame's viewport or the whole depth) agrees with the deck.

use std::cell::Cell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};
use windows::core::Interface;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::vhook::VSlot;

type RbFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_RESOURCE_BARRIER);
type VpFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_VIEWPORT);
type BarrierFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_BARRIER_GROUP);
type OmFn = unsafe extern "system" fn(*mut c_void, u32, *const D3D12_CPU_DESCRIPTOR_HANDLE, BOOL, *const D3D12_CPU_DESCRIPTOR_HANDLE);
type ResetFn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> windows::core::HRESULT;
type CtrFn = unsafe extern "system" fn(*mut c_void, *const D3D12_TEXTURE_COPY_LOCATION, u32, u32, u32, *const D3D12_TEXTURE_COPY_LOCATION, *const D3D12_BOX);
type CrFn = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void);
type CclFn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    D3D12_COMMAND_LIST_TYPE,
    *mut c_void,
    *mut c_void,
    *const windows::core::GUID,
    *mut *mut c_void,
) -> windows::core::HRESULT;
type Ccl1Fn = unsafe extern "system" fn(
    *mut c_void,
    u32,
    D3D12_COMMAND_LIST_TYPE,
    D3D12_COMMAND_LIST_FLAGS,
    *const windows::core::GUID,
    *mut *mut c_void,
) -> windows::core::HRESULT;

/// The device's `CreateCommandList` (ID3D12Device) and `CreateCommandList1` (ID3D12Device4) slots: hooked on the
/// device's vtable (as the queue's `ExecuteCommandLists` is), so a new list's own table is patched at creation
/// (`adopt`) and not only at its first submission: the copies recorded on a fresh list before that would be missed
/// by the copy tracer. `adopt` only patches a table it recognises; a list whose own table is not there yet at
/// creation is patched at its first submission as before.
pub fn slot_create_command_list() -> usize {
    std::mem::offset_of!(ID3D12Device_Vtbl, CreateCommandList) / 8
}
pub fn slot_create_command_list1() -> usize {
    std::mem::offset_of!(ID3D12Device4_Vtbl, CreateCommandList1) / 8
}
pub(crate) static CCL: VSlot = VSlot::new();
pub(crate) static CCL1: VSlot = VSlot::new();
/// Lists adopted at creation (their own table was there to patch).
pub static ADOPTED_AT_CREATE: AtomicU64 = AtomicU64::new(0);
pub static CREATED: AtomicU64 = AtomicU64::new(0);

unsafe fn adopt_created(ppv: *mut *mut c_void) {
    if ppv.is_null() || (*ppv).is_null() || !hooks_wanted() || crate::overlay::stopping() {
        return;
    }
    CREATED.fetch_add(1, Ordering::Relaxed);
    let before = PATCHED.load(Ordering::Relaxed);
    let _ = std::panic::catch_unwind(|| adopt(*ppv as usize));
    if PATCHED.load(Ordering::Relaxed) > before {
        ADOPTED_AT_CREATE.fetch_add(1, Ordering::Relaxed);
    }
}

pub(crate) unsafe extern "system" fn create_command_list(
    this: *mut c_void,
    node: u32,
    ty: D3D12_COMMAND_LIST_TYPE,
    allocator: *mut c_void,
    pso: *mut c_void,
    riid: *const windows::core::GUID,
    ppv: *mut *mut c_void,
) -> windows::core::HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: CclFn = std::mem::transmute::<usize, CclFn>(CCL.original());
    let r = f(this, node, ty, allocator, pso, riid, ppv);
    if r.is_ok() {
        adopt_created(ppv);
    }
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

pub(crate) unsafe extern "system" fn create_command_list1(
    this: *mut c_void,
    node: u32,
    ty: D3D12_COMMAND_LIST_TYPE,
    flags: D3D12_COMMAND_LIST_FLAGS,
    riid: *const windows::core::GUID,
    ppv: *mut *mut c_void,
) -> windows::core::HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: Ccl1Fn = std::mem::transmute::<usize, Ccl1Fn>(CCL1.original());
    let r = f(this, node, ty, flags, riid, ppv);
    if r.is_ok() {
        adopt_created(ppv);
    }
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

/// The command list vtable slots hooked, from the `windows` crate's vtable layouts.
pub fn slot_resource_barrier() -> usize {
    std::mem::offset_of!(ID3D12GraphicsCommandList_Vtbl, ResourceBarrier) / 8
}
pub fn slot_viewports() -> usize {
    std::mem::offset_of!(ID3D12GraphicsCommandList_Vtbl, RSSetViewports) / 8
}
pub fn slot_barrier() -> usize {
    std::mem::offset_of!(ID3D12GraphicsCommandList7_Vtbl, Barrier) / 8
}
pub fn slot_render_targets() -> usize {
    std::mem::offset_of!(ID3D12GraphicsCommandList_Vtbl, OMSetRenderTargets) / 8
}
pub fn slot_reset() -> usize {
    std::mem::offset_of!(ID3D12GraphicsCommandList_Vtbl, Reset) / 8
}
pub fn slot_copy_texture_region() -> usize {
    std::mem::offset_of!(ID3D12GraphicsCommandList_Vtbl, CopyTextureRegion) / 8
}
pub fn slot_copy_resource() -> usize {
    std::mem::offset_of!(ID3D12GraphicsCommandList_Vtbl, CopyResource) / 8
}

/// The slots by kind (the order of [`hooks`]): ResourceBarrier, RSSetViewports, Barrier, OMSetRenderTargets, Reset,
/// CopyTextureRegion, CopyResource (the last two for the copy tracer, `copies.rs`).
pub fn hook_slots() -> [usize; KINDS] {
    [slot_resource_barrier(), slot_viewports(), slot_barrier(), slot_render_targets(), slot_reset(), slot_copy_texture_region(), slot_copy_resource()]
}

const KINDS: usize = 7;

pub(crate) static RB: VSlot = VSlot::new();
pub(crate) static VP: VSlot = VSlot::new();
pub(crate) static CTR: VSlot = VSlot::new();
pub(crate) static CR: VSlot = VSlot::new();
pub(crate) static BAR: VSlot = VSlot::new();
pub(crate) static OM: VSlot = VSlot::new();
pub(crate) static RS: VSlot = VSlot::new();

/// `draw depth on|off`: hide the bars behind the scene (on by default). Off: no copies are recorded at all.
pub static ENABLED: AtomicBool = AtomicBool::new(true);
/// `draw depth near X`: the near plane in metres instead of the game's own (0: the game's, [`GAME_NEAR`]), as f32 bits.
pub static NEAR_OVERRIDE: AtomicU32 = AtomicU32::new(0);
/// The back buffer's size (width << 32 | height), set by the overlay at Present.
pub static BACK_SIZE: AtomicU64 = AtomicU64::new(0);
/// Presents so far (the overlay counts them): ages things.
pub static PRESENT_NO: AtomicU64 = AtomicU64::new(0);

const DEPTH_WRITE: u32 = 0x10;
const COPY_DEST: u32 = 0x400;
const COPY_SOURCE: u32 = 0x800;
const PIXEL_SHADER_RESOURCE: u32 = 0x80;
const ALL_SUBRESOURCES: u32 = 0xffff_ffff;
pub const RING: usize = 8;

thread_local! {
    /// The last viewport this thread set (x, y, width, height).
    static VIEWPORT: Cell<[f32; 4]> = const { Cell::new([0.0; 4]) };
    /// Whether this thread's list has a depth target bound now (OMSetRenderTargets).
    static DSV_BOUND: Cell<bool> = const { Cell::new(false) };
    /// Recording our own commands: our hooks pass through.
    static OURS: Cell<bool> = const { Cell::new(false) };
}

// ---------------------------------------------------------------- the frame's render viewport
//
// With a dynamic resolution (Trevor's 80-100 %, usually with DLSS) the game renders each frame into a viewport of its
// main depth that changes from frame to frame; and the viewport a thread set last before the depth's barrier can be
// another pass's (a half-size one: seen in game 2026-10-21 15:15). The scene's viewport of a frame is taken as the
// largest viewport set while a depth target is bound, among those that fit the main depth (no larger, its aspect
// within 3 %, at least a quarter of its width): the scene's passes (and the G-buffer's) all use it; half-size passes
// are smaller, shadow maps another shape, and passes at the output size do not fit a render-size depth. Kept per
// present window: the copy recorded in a window uses that window's (or, before any, the last window's).

/// The main depth's size (width << 32 | height), for the viewport test (0: no main depth yet).
static MAIN_SIZE: AtomicU64 = AtomicU64::new(0);
/// The largest fitting viewport of the current and the last present window: (present << 32 | area) and its rect.
static SCENE_VP: Mutex<[(u64, [f32; 4]); 2]> = Mutex::new([(0, [0.0; 4]); 2]);
static SCENE_VP_KEY: AtomicU64 = AtomicU64::new(0);

/// Notes a viewport set while a depth target is bound.
fn note_scene_viewport(vp: [f32; 4]) {
    let size = MAIN_SIZE.load(Ordering::Relaxed);
    if size == 0 {
        return;
    }
    let (w, h) = ((size >> 32) as f32, (size & 0xffff_ffff) as f32);
    let [x, y, vw, vh] = vp;
    if !(vw >= w * 0.25 && vh > 0.0 && x >= 0.0 && y >= 0.0 && x + vw <= w + 0.5 && y + vh <= h + 0.5 && ((vw / vh) / (w / h) - 1.0).abs() < 0.03) {
        return;
    }
    let now = PRESENT_NO.load(Ordering::Relaxed);
    let key = ((now & 0xffff_ffff) << 32) | (vw * vh) as u64;
    // only the first viewport of a window, or a larger one, takes the lock
    let cur = SCENE_VP_KEY.load(Ordering::Relaxed);
    if cur >> 32 == now & 0xffff_ffff && cur & 0xffff_ffff >= key & 0xffff_ffff {
        return;
    }
    if let Ok(mut s) = SCENE_VP.lock() {
        let window = (now % 2) as usize;
        if s[window].0 != now {
            s[window] = (now, vp);
        } else if vw * vh > s[window].1[2] * s[window].1[3] {
            s[window].1 = vp;
        }
        SCENE_VP_KEY.store(((now & 0xffff_ffff) << 32) | (s[window].1[2] * s[window].1[3]) as u64, Ordering::Relaxed);
    }
}

/// The scene's viewport for a copy recorded now: this window's, else the last window's.
fn scene_viewport() -> Option<[f32; 4]> {
    let now = PRESENT_NO.load(Ordering::Relaxed);
    let s = SCENE_VP.lock().ok()?;
    let this = s[(now % 2) as usize];
    if this.0 == now && this.1[2] > 0.0 {
        return Some(this.1);
    }
    let last = s[((now + 1) % 2) as usize];
    (last.0 + 1 == now && last.1[2] > 0.0).then_some(last.1)
}

/// A depth resource's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desc {
    pub width: u32,
    pub height: u32,
    pub format: i32,
}

impl Desc {
    /// The copy's format and how to view it, when the format is one we can copy (the depth plane is 32 or 16 bits).
    pub fn copy_format(&self) -> Option<DXGI_FORMAT> {
        match DXGI_FORMAT(self.format) {
            DXGI_FORMAT_R32_TYPELESS | DXGI_FORMAT_D32_FLOAT | DXGI_FORMAT_R32G8X24_TYPELESS | DXGI_FORMAT_D32_FLOAT_S8X24_UINT => Some(DXGI_FORMAT_R32_FLOAT),
            DXGI_FORMAT_R16_TYPELESS | DXGI_FORMAT_D16_UNORM => Some(DXGI_FORMAT_R16_UNORM),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    res: usize,
    desc: Desc,
    /// A 2D single-sample depth target of a format we can copy.
    usable: bool,
    /// Passes ended (transitions out of DEPTH_WRITE) seen.
    ends: u64,
    /// The present it was last seen at.
    last: u64,
    viewport: [f32; 4],
    enhanced: bool,
    /// Present windows in a row it ended a pass in (up to the last one).
    streak: u64,
}

static CANDIDATES: Mutex<Vec<Candidate>> = Mutex::new(Vec::new());
/// The main depth resource (0: none yet).
pub static CHOSEN: AtomicUsize = AtomicUsize::new(0);

/// Our copies of the depth, made by the overlay on the game's device: one set, made once at the largest size needed
/// (the back buffer's, or a larger depth's) in the copy format of the main depth's; a smaller depth (another render
/// size, one the game makes anew at another size) is copied into its top left corner, never a set of its own (a set at
/// every size change, with each replaced set kept for 3 s, filled the video memory and crashed the game on a pause,
/// 2026-10-21 18:29).
pub struct CopySet {
    pub textures: Vec<ID3D12Resource>,
    /// The textures' size.
    pub size: (u32, u32),
    pub format: DXGI_FORMAT,
    pub generation: u64,
}

impl CopySet {
    /// Whether a depth of this shape can be copied into the set.
    pub fn takes(&self, d: &Desc) -> bool {
        d.copy_format() == Some(self.format) && d.width <= self.size.0 && d.height <= self.size.1
    }

    /// The video memory it holds (bytes).
    pub fn bytes(&self) -> u64 {
        let texel = if self.format == DXGI_FORMAT_R16_UNORM { 2 } else { 4 };
        self.size.0 as u64 * self.size.1 as u64 * texel * self.textures.len() as u64
    }
}

// The textures are free-threaded COM objects.
unsafe impl Send for CopySet {}

pub static COPIES: Mutex<Option<CopySet>> = Mutex::new(None);
/// The overlay's account of the copies' video memory: bytes held (the set in use and a replaced one), sets alive, sets
/// made, and why a set is not being made now (empty: it is not held back).
pub static HELD_BYTES: AtomicU64 = AtomicU64::new(0);
pub static SETS_ALIVE: AtomicU64 = AtomicU64::new(0);
pub static SETS_MADE: AtomicU64 = AtomicU64::new(0);
pub static SETS_LEAKED: AtomicU64 = AtomicU64::new(0);
pub static HELD_BACK: Mutex<String> = Mutex::new(String::new());
static NEXT: AtomicU32 = AtomicU32::new(0);

struct Pending {
    list: usize,
    slot: u32,
    at: u64,
    viewport: [f32; 4],
    generation: u64,
    end_us: u64,
    src: (u32, u32),
    cam: Snap,
    board: Snap,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
static PENDING_N: AtomicU32 = AtomicU32::new(0);

/// A copy submitted.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Latest {
    pub slot: u32,
    /// The present count when it was recorded.
    pub at: u64,
    pub viewport: [f32; 4],
    pub generation: u64,
    /// The picture it belongs to (the present that shows it): see [`picture_of`].
    pub picture: u64,
    /// The present count when the list carrying it was submitted, on which queue, and when (microseconds), and when
    /// its pass ended.
    pub submitted: u64,
    pub queue: usize,
    pub submit_us: u64,
    pub end_us: u64,
    /// The queue's fence value signalled after it (0: none).
    pub fence: u64,
    /// The depth's size (the set's textures are that size).
    pub src: (u32, u32),
    /// Submitted on the queue the overlay draws on (else never read).
    pub usable: bool,
    /// The camera's and the board's proxies when it was recorded.
    pub cam: Snap,
    pub board: Snap,
}

// ---------------------------------------------------------------- which picture a copy belongs to
//
// By the queue's order, a copy submitted before Present k is in the picture Present k shows, at the latest; a copy
// submitted in the window Present k ends (count k - 1 then) belongs to picture k. Two copies submitted in one window
// (the next frame's G-buffer submitted early) belong to that picture and the next: each copy's picture is one past the
// last one's at least. (Up to 2026-10-21 evening the window a copy was recorded in decided, and only the first
// G-buffer pass end of a window was copied: a next frame's G-buffer recorded before the present went uncopied, and the
// next picture had none of its own.)

/// The picture of a copy submitted at present count `submitted`, after one for `last` (0: none); at most the picture
/// after the window's own (so that more G-buffer pass ends than pictures never run ahead of the presents).
pub fn picture_of(submitted: u64, last: u64) -> u64 {
    (submitted + 1).max(if last == 0 { 0 } else { last + 1 }).min(submitted + 2)
}

static LAST_PICTURE: AtomicU64 = AtomicU64::new(0);

/// `draw depth copies picture|window`: copy every pass end in the G-buffer's state (picture, the default), or only
/// the first of each present window (window: as up to 2026-10-21 evening).
pub static COPY_PER_PICTURE: AtomicBool = AtomicBool::new(true);
/// G-buffer pass ends after the first in a window (copied in picture mode).
static SECOND_GBUFFER: AtomicU64 = AtomicU64::new(0);

/// The queue the overlay draws on (raw; 0: not known yet). A copy submitted on another queue is never read (the
/// draw could run before it: across queues only a fence orders work), only counted ([`COPIES_OTHER_QUEUE`]).
pub static DRAW_QUEUE: AtomicUsize = AtomicUsize::new(0);
static COPIES_OTHER_QUEUE: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------- one fence per queue, for freeing a replaced set
//
// A set of copies replaced by another is freed only once every copy recorded into it has run: nothing recorded still
// waits to be submitted, and on each queue a copy went to, a fence of ours signalled right after that submission has
// passed. One fence per queue (a fence signalled from two queues could go backwards), made with the queue's device
// the first time a copy goes to it.

struct QueueFence {
    queue: usize,
    fence: ID3D12Fence,
    value: u64,
}

// The fence is a free-threaded COM object.
unsafe impl Send for QueueFence {}

static QUEUE_FENCES: Mutex<Vec<QueueFence>> = Mutex::new(Vec::new());
static SIGNALS: AtomicU64 = AtomicU64::new(0);

/// Signals the queue's fence after what was just submitted on it; the value, with the queue (0: no fence could be made).
fn signal_after(queue: *mut c_void) -> u64 {
    let Ok(mut fences) = QUEUE_FENCES.lock() else { return 0 };
    let i = match fences.iter().position(|f| f.queue == queue as usize) {
        Some(i) => i,
        None => {
            if fences.len() >= 16 {
                return 0;
            }
            // SAFETY: the queue is the one the game is submitting on
            let q = unsafe { ID3D12CommandQueue::from_raw_borrowed(&queue) };
            let Some(q) = q else { return 0 };
            let mut device: Option<ID3D12Device> = None;
            if unsafe { q.GetDevice(&mut device) }.is_err() {
                return 0;
            }
            let Some(device) = device else { return 0 };
            let Ok(fence) = (unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }) else { return 0 };
            fences.push(QueueFence { queue: queue as usize, fence, value: 0 });
            fences.len() - 1
        }
    };
    let f = &mut fences[i];
    let next = f.value + 1;
    // SAFETY: the queue is alive (it is submitting now)
    let q = unsafe { ID3D12CommandQueue::from_raw_borrowed(&queue) };
    match q.map(|q| unsafe { q.Signal(&f.fence, next) }) {
        Some(Ok(())) => {
            f.value = next;
            SIGNALS.fetch_add(1, Ordering::Relaxed);
            next
        }
        _ => 0,
    }
}

static WAITS: AtomicU64 = AtomicU64::new(0);
static WAITS_FAILED: AtomicU64 = AtomicU64::new(0);

/// The overlay's draw fence (raw `ID3D12Fence`, kept alive by the overlay) and, per copy slot, the value of it
/// signalled after the last draw that read the slot: a copy into that slot waits for it on the GPU (`before_execute`),
/// so the copy engine never rewrites a slot the draw is still reading (a draw on the presenting queue can run frames
/// behind the game's rendering).
static DRAW_FENCE: AtomicUsize = AtomicUsize::new(0);
static SLOT_READ: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static REUSE_WAITS: AtomicU64 = AtomicU64::new(0);

/// How often the hooks run at all (the status): a count that stands still while the game goes on means the lists'
/// tables no longer hold them; barriers seen while `OURS` is set are our own recording's.
static RB_CALLS: AtomicU64 = AtomicU64::new(0);
static RB_OURS: AtomicU64 = AtomicU64::new(0);
static OM_CALLS: AtomicU64 = AtomicU64::new(0);
static VP_CALLS: AtomicU64 = AtomicU64::new(0);
static RS_CALLS: AtomicU64 = AtomicU64::new(0);

/// `OURS` set for a scope and cleared when it ends, however it ends (a panic caught by the hook would otherwise
/// leave it set on that thread, and every barrier it records afterwards would go unseen).
struct Ours;

impl Ours {
    fn begin() -> Ours {
        OURS.with(|o| o.set(true));
        Ours
    }
}

impl Drop for Ours {
    fn drop(&mut self) {
        OURS.with(|o| o.set(false));
    }
}

/// After the overlay's draw that read copy `slot` was submitted and `fence` signalled with `value` after it.
pub fn note_read(slot: u32, fence: &ID3D12Fence, value: u64) {
    DRAW_FENCE.store(fence.as_raw() as usize, Ordering::Release);
    if let Some(s) = SLOT_READ.get(slot as usize) {
        s.store(value, Ordering::Release);
    }
}

/// From the overlay's `ExecuteCommandLists` hook, before the lists go to the queue: a copy among them into a slot the
/// overlay's last read of has not finished on the GPU makes the queue wait for that draw's fence first.
pub fn before_execute(queue: *mut c_void, lists: &[*mut c_void]) {
    if PENDING_N.load(Ordering::Acquire) == 0 {
        return;
    }
    let fence_raw = DRAW_FENCE.load(Ordering::Acquire);
    if fence_raw == 0 {
        return;
    }
    let mut wanted: u64 = 0;
    if let Ok(p) = PENDING.lock() {
        for x in p.iter() {
            if lists.iter().any(|&l| l as usize == x.list) {
                if let Some(s) = SLOT_READ.get(x.slot as usize) {
                    wanted = wanted.max(s.load(Ordering::Acquire));
                }
            }
        }
    }
    if wanted == 0 {
        return;
    }
    // SAFETY: the fence is the overlay's, alive while the overlay is installed; the queue is submitting now
    unsafe {
        let raw = fence_raw as *mut c_void;
        let Some(fence) = ID3D12Fence::from_raw_borrowed(&raw) else { return };
        if fence.GetCompletedValue() >= wanted {
            return;
        }
        let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&queue) else { return };
        if q.Wait(fence, wanted).is_ok() {
            REUSE_WAITS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Makes `draw_queue` wait, on the GPU, for `queue`'s fence to pass `value` (a copy submitted there), before what is
/// submitted on it next. False when the queue has no fence (nothing was waited for).
pub fn wait_on(draw_queue: &ID3D12CommandQueue, queue: usize, value: u64) -> bool {
    if value == 0 {
        return false;
    }
    let Ok(fences) = QUEUE_FENCES.lock() else { return false };
    let Some(f) = fences.iter().find(|x| x.queue == queue) else {
        WAITS_FAILED.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    // SAFETY: the draw queue is alive (the overlay is submitting on it now) and the fence is ours
    match unsafe { draw_queue.Wait(&f.fence, value) } {
        Ok(()) => {
            WAITS.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(_) => {
            WAITS_FAILED.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

/// Whether a queue's fence has passed `value` (true for a value of 0: nothing to wait for; false for a queue whose
/// fence is gone).
fn fence_passed(queue: usize, value: u64) -> bool {
    if value == 0 {
        return true;
    }
    QUEUE_FENCES.lock().map(|f| f.iter().find(|x| x.queue == queue).is_some_and(|x| unsafe { x.fence.GetCompletedValue() } >= value)).unwrap_or(false)
}

/// The renderer's proxies' matrices at a moment (`proxy.rs`: +0x50 this frame's, +0x90 the previous frame's), for
/// pairing a copy with the frame it was recorded in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub taken: bool,
    pub now: [u8; 64],
    pub prev: [u8; 64],
}

impl Default for Snap {
    fn default() -> Self {
        Snap { taken: false, now: [0; 64], prev: [0; 64] }
    }
}

impl Snap {
    /// The proxy's matrices now (false when it is not found or not readable).
    pub fn of(proxy: usize) -> Snap {
        let mut s = Snap::default();
        if proxy != 0 && crate::win::read_into(proxy + crate::proxy::MATRIX, &mut s.now) && crate::win::read_into(proxy + crate::proxy::PREVIOUS, &mut s.prev) {
            s.taken = true;
        }
        s
    }
}

/// Resource states with a write bit: a depth left in one is being written by the next pass, never copied (even if
/// the learnt G-buffer state were one): RENDER_TARGET, UNORDERED_ACCESS, DEPTH_WRITE, STREAM_OUT, COPY_DEST,
/// RESOLVE_DEST, VIDEO_DECODE_WRITE, VIDEO_PROCESS_WRITE, VIDEO_ENCODE_WRITE.
pub const WRITE_STATES: u32 = 0x4 | 0x8 | 0x10 | 0x100 | 0x400 | 0x1000 | 0x20000 | 0x80000 | 0x80_0000;
static WRITE_STATE_ENDS: AtomicU64 = AtomicU64::new(0);

/// The copies submitted last, newest last (as many as the ring holds).
static LATEST: Mutex<VecDeque<Latest>> = Mutex::new(VecDeque::new());

static ENDS: AtomicU64 = AtomicU64::new(0);
static INJECTED: AtomicU64 = AtomicU64::new(0);
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static CONTENDED: AtomicU64 = AtomicU64::new(0);
static ENHANCED: AtomicU64 = AtomicU64::new(0);
static NOT_DIRECT: AtomicU64 = AtomicU64::new(0);
/// Pass ends of the main depth after the frame's first (no copy taken), and the present window of the last copy + 1.
static EXTRA: AtomicU64 = AtomicU64::new(0);
static LAST_COPY_AT: AtomicU64 = AtomicU64::new(0);

/// `pResource` from a raw pointer, without a reference.
unsafe fn res_field(p: usize) -> ManuallyDrop<Option<ID3D12Resource>> {
    std::mem::transmute_copy(&p)
}

fn transition(res: usize, sub: u32, before: u32, after: u32) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { res_field(res) },
                Subresource: sub,
                StateBefore: D3D12_RESOURCE_STATES(before as i32),
                StateAfter: D3D12_RESOURCE_STATES(after as i32),
            }),
        },
    }
}

/// A resource's shape, and whether it is a depth target we can use.
unsafe fn describe(res: usize) -> Option<(Desc, bool)> {
    let raw = res as *mut c_void;
    let r = ID3D12Resource::from_raw_borrowed(&raw)?;
    let d = r.GetDesc();
    let desc = Desc { width: d.Width as u32, height: d.Height, format: d.Format.0 };
    let usable = d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && d.SampleDesc.Count == 1
        && d.DepthOrArraySize == 1
        && (d.Flags & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL).0 != 0
        && desc.copy_format().is_some()
        && d.Width <= 16384
        && d.Height <= 16384;
    Some((desc, usable))
}

/// Notes a resource that ended a depth pass; true when it is the chosen main depth.
unsafe fn note_end(res: usize, enhanced: bool) -> bool {
    ENDS.fetch_add(1, Ordering::Relaxed);
    let now = PRESENT_NO.load(Ordering::Relaxed);
    let viewport = VIEWPORT.with(|v| v.get());
    // described every time (GetDesc is cheap): a resource the game freed and made again at the same address may have
    // another shape
    let Some((desc, usable)) = describe(res) else { return false };
    let Ok(mut list) = CANDIDATES.lock() else { return false };
    if let Some(c) = list.iter_mut().find(|c| c.res == res) {
        if c.desc != desc {
            *c = Candidate { res, desc, usable, ends: 0, last: now, viewport, enhanced, streak: 0 };
        }
        c.ends += 1;
        if c.last + 1 == now || c.streak == 0 {
            c.streak += 1;
        } else if c.last != now {
            c.streak = 1;
        }
        c.last = now;
        c.viewport = viewport;
        c.enhanced |= enhanced;
    } else {
        if list.len() >= 32 {
            // forget the one seen longest ago
            if let Some(i) = list.iter().enumerate().min_by_key(|(_, c)| c.last).map(|(i, _)| i) {
                list.swap_remove(i);
            }
        }
        list.push(Candidate { res, desc, usable, ends: 1, last: now, viewport, enhanced, streak: 1 });
    }
    res == CHOSEN.load(Ordering::Acquire)
}

unsafe fn resource_barrier_body(orig: usize, this: *mut c_void, n: u32, barriers: *const D3D12_RESOURCE_BARRIER) {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    RB_CALLS.fetch_add(1, Ordering::Relaxed);
    if OURS.with(|o| o.get()) {
        RB_OURS.fetch_add(1, Ordering::Relaxed);
    }
    let f: RbFn = std::mem::transmute::<usize, RbFn>(orig);
    f(this, n, barriers);
    if !OURS.with(|o| o.get()) && ENABLED.load(Ordering::Relaxed) && !barriers.is_null() && n > 0 && !crate::overlay::stopping() {
        let list = std::slice::from_raw_parts(barriers, n as usize);
        // The Caveman's texture: what the game's own barriers do to it (board.rs)
        let _ = std::panic::catch_unwind(|| crate::board::observe_on(this as usize, list));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| after_barriers(orig, this, list)));
    }
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
}

unsafe extern "system" fn resource_barrier(this: *mut c_void, n: u32, barriers: *const D3D12_RESOURCE_BARRIER) {
    resource_barrier_body(RB.original(), this, n, barriers)
}

/// The same, in a list's own table whose slot held another function than the shared table's (`ORIG[0][V]`).
unsafe extern "system" fn resource_barrier_v<const V: usize>(this: *mut c_void, n: u32, barriers: *const D3D12_RESOURCE_BARRIER) {
    resource_barrier_body(ORIG[0][V].load(Ordering::Acquire), this, n, barriers)
}

unsafe fn after_barriers(orig: usize, list: *mut c_void, barriers: &[D3D12_RESOURCE_BARRIER]) {
    for b in barriers {
        if b.Type != D3D12_RESOURCE_BARRIER_TYPE_TRANSITION || (b.Flags & D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY).0 != 0 {
            continue;
        }
        let t = &b.Anonymous.Transition;
        let (before, after) = (t.StateBefore.0 as u32, t.StateAfter.0 as u32);
        if before & DEPTH_WRITE == 0 || after & DEPTH_WRITE != 0 || (t.Subresource != ALL_SUBRESOURCES && t.Subresource != 0) {
            continue;
        }
        let res: usize = std::mem::transmute_copy(&t.pResource);
        if res != 0 && note_end(res, false) {
            inject(orig, list, res, after);
        }
    }
}

// ---------------------------------------------------------------- which pass end is the G-buffer's
//
// The main depth ends several passes a frame (about 5 in game: the G-buffer's, then others). The copy is taken at the
// first one in each present window, and only when it leaves the depth in the G-buffer's state: the state the first
// pass end of a window leaves it in most often (learnt over the last 64 windows; until then, or when no state
// dominates, any). So a window whose first pass end is another pass's (the next frame's recording overlapping the
// present, or a pass end before the G-buffer's) gets no copy, rather than a copy of another stage of the depth (a
// cleared or partly drawn depth would hide the bars everywhere, or nowhere, for a frame: a blink).

/// The learnt G-buffer state (1 << 32 | state; 0: not learnt).
static SIGNATURE: AtomicU64 = AtomicU64::new(0);
/// The states the first pass end of the last windows left the main depth in.
static FIRSTS: Mutex<(VecDeque<u32>, u64)> = Mutex::new((VecDeque::new(), 0));
/// The present window of the last main-depth pass end + 1.
static LAST_END_WINDOW: AtomicU64 = AtomicU64::new(0);
/// Pass ends not copied for leaving the depth in another state than the G-buffer's.
static MISMATCHED: AtomicU64 = AtomicU64::new(0);
/// The main depth's pass ends of the last windows (state, what was done: c copied, x a later end of a copied window,
/// s another state than the G-buffer's, n not the copies' shape or not a direct list), for the status.
static ENDS_LOG: Mutex<VecDeque<(u64, Vec<(u32, char)>)>> = Mutex::new(VecDeque::new());
const SIGNATURE_WINDOWS: usize = 64;

/// Notes a main-depth pass end for the log and the learning; true when it is the first of its window.
fn note_main_end(now: u64, after: u32) -> bool {
    let first = LAST_END_WINDOW.swap(now + 1, Ordering::AcqRel) != now + 1;
    // (a state with a write bit is never learnt as the G-buffer's: see WRITE_STATES)
    if first && after & WRITE_STATES == 0 {
        if let Ok(mut f) = FIRSTS.try_lock() {
            f.0.push_back(after);
            if f.0.len() > SIGNATURE_WINDOWS {
                f.0.pop_front();
            }
            f.1 += 1;
            // every 8 windows: the state most first pass ends leave the depth in, when it is 3 in 4 of at least 32
            if f.1 % 8 == 0 && f.0.len() >= 32 {
                let mut best = (0u32, 0usize);
                for &s in &f.0 {
                    let n = f.0.iter().filter(|&&x| x == s).count();
                    if n > best.1 {
                        best = (s, n);
                    }
                }
                let sig = if best.1 * 4 >= f.0.len() * 3 { (1u64 << 32) | best.0 as u64 } else { 0 };
                SIGNATURE.store(sig, Ordering::Release);
            }
        }
    }
    first
}

fn log_end(now: u64, after: u32, what: char) {
    if let Ok(mut l) = ENDS_LOG.try_lock() {
        if l.back().is_none_or(|(w, _)| *w != now) {
            l.push_back((now, Vec::new()));
            if l.len() > 4 {
                l.pop_front();
            }
        }
        if let Some((_, v)) = l.back_mut() {
            if v.len() < 12 {
                v.push((after, what));
            }
        }
    }
}

/// Whether a pass end leaves the depth in the G-buffer's state (anything, before it is learnt).
fn is_gbuffer_end(after: u32) -> bool {
    let sig = SIGNATURE.load(Ordering::Acquire);
    sig == 0 || (sig & 0xffff_ffff) as u32 == after
}

/// What a copy needs from the copies, taken under their lock and used after it: the source shape, the generation, the
/// textures (references of our own: alive while the copy is recorded, whatever the overlay does meanwhile).
fn copies_snapshot() -> Option<(DXGI_FORMAT, (u32, u32), u64, [Option<ID3D12Resource>; RING], usize)> {
    let take = |g: &Option<CopySet>| {
        let c = g.as_ref()?;
        let n = c.textures.len().min(RING);
        let t: [Option<ID3D12Resource>; RING] = std::array::from_fn(|k| c.textures.get(k).filter(|_| k < n).cloned());
        (n > 0).then_some((c.format, c.size, c.generation, t, n))
    };
    match COPIES.try_lock() {
        Ok(g) => take(&g),
        Err(_) => {
            // held for a moment by the overlay or a status request: wait for it (never skip the frame's copy)
            CONTENDED.fetch_add(1, Ordering::Relaxed);
            COPIES.lock().ok().and_then(|g| take(&g))
        }
    }
}

/// Records, in the game's list, the copy of the depth's plane 0 into the next texture of the ring (see the module's
/// notes). `after` is the state the game's barrier just gave the depth; `orig` the list's own `ResourceBarrier`.
unsafe fn inject(orig: usize, list: *mut c_void, res: usize, after: u32) {
    let now = PRESENT_NO.load(Ordering::Relaxed);
    let end_us = crate::diag::now_us();
    note_main_end(now, after);
    let gbuffer = is_gbuffer_end(after);
    let learnt = SIGNATURE.load(Ordering::Acquire) != 0;
    // one copy a picture: every pass end of the main depth that leaves it in the G-buffer's state (normally one a
    // frame: the next frame's may come before the present, in the same window); before that state is learnt, and in
    // window mode, the first such end of each present window. Later ones (decals, a second depth pass) are left alone:
    // the copy is 4 bytes a pixel of the whole depth
    let per_picture = learnt && COPY_PER_PICTURE.load(Ordering::Relaxed);
    let copied_here = LAST_COPY_AT.load(Ordering::Acquire) == now + 1;
    if !gbuffer || (copied_here && !per_picture) {
        if copied_here {
            EXTRA.fetch_add(1, Ordering::Relaxed);
            log_end(now, after, 'x');
        } else {
            MISMATCHED.fetch_add(1, Ordering::Relaxed);
            log_end(now, after, 's');
        }
        crate::diag::window_end(now, gbuffer, false);
        return;
    }
    // a state with a write bit: the next pass writes the depth now (never copied, whatever was learnt)
    if after & WRITE_STATES != 0 {
        WRITE_STATE_ENDS.fetch_add(1, Ordering::Relaxed);
        log_end(now, after, 'w');
        crate::diag::window_end(now, gbuffer, false);
        return;
    }
    let Some((format, size, generation, textures, count)) = copies_snapshot() else { return };
    let Some((source, _)) = describe(res) else { return };
    // the set is the main depth's exact size and copy format; another depth (one the game made anew at another size)
    // is not copied until the set is made again for it (the overlay, once it has been steady a while)
    if source.copy_format() != Some(format) || (source.width, source.height) != size {
        log_end(now, after, 'n');
        crate::diag::window_end(now, gbuffer, false);
        return;
    }
    let Some(l) = ID3D12GraphicsCommandList::from_raw_borrowed(&list) else { return };
    if l.GetType() != D3D12_COMMAND_LIST_TYPE_DIRECT {
        NOT_DIRECT.fetch_add(1, Ordering::Relaxed);
        log_end(now, after, 'n');
        crate::diag::window_end(now, gbuffer, false);
        return;
    }
    let last = LAST_COPY_AT.swap(now + 1, Ordering::AcqRel);
    if last == now + 1 {
        if !per_picture {
            EXTRA.fetch_add(1, Ordering::Relaxed);
            log_end(now, after, 'x');
            crate::diag::window_end(now, gbuffer, false);
            return;
        }
        SECOND_GBUFFER.fetch_add(1, Ordering::Relaxed);
    }
    log_end(now, after, 'c');
    crate::diag::window_end(now, gbuffer, true);
    let slot = NEXT.fetch_add(1, Ordering::Relaxed) % count as u32;
    let Some(dst_tex) = textures[slot as usize].as_ref() else { return };
    let dst = dst_tex.as_raw() as usize;
    let rb: RbFn = std::mem::transmute::<usize, RbFn>(orig);
    let ours = Ours::begin();
    let mut pre = Vec::with_capacity(2);
    if after & COPY_SOURCE == 0 {
        pre.push(transition(res, 0, after, COPY_SOURCE));
    }
    pre.push(transition(dst, 0, PIXEL_SHADER_RESOURCE, COPY_DEST));
    rb(list, pre.len() as u32, pre.as_ptr());
    let to = D3D12_TEXTURE_COPY_LOCATION {
        pResource: res_field(dst),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    let from = D3D12_TEXTURE_COPY_LOCATION {
        pResource: res_field(res),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    l.CopyTextureRegion(&to, 0, 0, 0, &from, None);
    let mut post = Vec::with_capacity(2);
    if after & COPY_SOURCE == 0 {
        post.push(transition(res, 0, COPY_SOURCE, after));
    }
    post.push(transition(dst, 0, COPY_DEST, PIXEL_SHADER_RESOURCE));
    rb(list, post.len() as u32, post.as_ptr());
    drop(ours);
    INJECTED.fetch_add(1, Ordering::Relaxed);
    // the part of the depth this frame rendered: the scene's viewport (dynamic resolution), else the whole depth
    let viewport = scene_viewport().unwrap_or([0.0, 0.0, source.width as f32, source.height as f32]);
    // the renderer's transforms at this moment: the frame this copy belongs to is the one drawn with the same ones
    let cam = Snap::of(crate::proxy::CAMERA.load(Ordering::Acquire));
    let board = Snap::of(crate::proxy::BOARD.load(Ordering::Acquire));
    if let Ok(mut p) = PENDING.lock() {
        // kept until the list is submitted (`on_execute`) or reset (`reset_body`: the recording is discarded), never
        // dropped by age: a copy dropped while its list could still run would read a freed texture
        p.push(Pending { list: list as usize, slot, at: now, viewport, generation, end_us, src: (source.width, source.height), cam, board });
        PENDING_N.store(p.len() as u32, Ordering::Release);
    }
}

/// `Reset` discards a list's recording: the copies recorded in it will never run.
fn drop_pending_of(list: usize) {
    if PENDING_N.load(Ordering::Acquire) == 0 {
        return;
    }
    if let Ok(mut p) = PENDING.lock() {
        let before = p.len();
        p.retain(|x| x.list != list);
        if p.len() != before {
            DROPPED_AT_RESET.fetch_add((before - p.len()) as u64, Ordering::Relaxed);
        }
        PENDING_N.store(p.len() as u32, Ordering::Release);
    }
}

static DROPPED_AT_RESET: AtomicU64 = AtomicU64::new(0);

/// A pending copy older than this is logged (its list was neither submitted nor reset: the set it was recorded into
/// is kept alive for good).
pub const STALE_PENDING_MS: u64 = 60_000;

unsafe fn viewports_body(orig: usize, this: *mut c_void, n: u32, v: *const D3D12_VIEWPORT) {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    VP_CALLS.fetch_add(1, Ordering::Relaxed);
    if n > 0 && !v.is_null() {
        let v = &*v;
        let vp = [v.TopLeftX, v.TopLeftY, v.Width, v.Height];
        VIEWPORT.with(|c| c.set(vp));
        if DSV_BOUND.with(|b| b.get()) && !OURS.with(|o| o.get()) {
            note_scene_viewport(vp);
        }
    }
    let f: VpFn = std::mem::transmute::<usize, VpFn>(orig);
    f(this, n, v);
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
}

unsafe fn render_targets_body(
    orig: usize,
    this: *mut c_void,
    n: u32,
    rts: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    single: BOOL,
    dsv: *const D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    OM_CALLS.fetch_add(1, Ordering::Relaxed);
    let bound = !dsv.is_null();
    DSV_BOUND.with(|b| b.set(bound));
    if bound && !OURS.with(|o| o.get()) {
        // a viewport set before the depth target was bound counts too
        note_scene_viewport(VIEWPORT.with(|v| v.get()));
    }
    let f: OmFn = std::mem::transmute::<usize, OmFn>(orig);
    f(this, n, rts, single, dsv);
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
}

unsafe extern "system" fn render_targets(
    this: *mut c_void,
    n: u32,
    rts: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    single: BOOL,
    dsv: *const D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    render_targets_body(OM.original(), this, n, rts, single, dsv)
}

unsafe extern "system" fn render_targets_v<const V: usize>(
    this: *mut c_void,
    n: u32,
    rts: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    single: BOOL,
    dsv: *const D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    render_targets_body(ORIG[3][V].load(Ordering::Acquire), this, n, rts, single, dsv)
}

/// A list starts afresh: no depth target bound, no viewport (this thread's view of the list being recorded).
unsafe fn reset_body(orig: usize, this: *mut c_void, allocator: *mut c_void, pso: *mut c_void) -> windows::core::HRESULT {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    RS_CALLS.fetch_add(1, Ordering::Relaxed);
    DSV_BOUND.with(|b| b.set(false));
    VIEWPORT.with(|v| v.set([0.0; 4]));
    drop_pending_of(this as usize);
    let f: ResetFn = std::mem::transmute::<usize, ResetFn>(orig);
    let r = f(this, allocator, pso);
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    r
}

unsafe extern "system" fn list_reset(this: *mut c_void, allocator: *mut c_void, pso: *mut c_void) -> windows::core::HRESULT {
    reset_body(RS.original(), this, allocator, pso)
}

unsafe extern "system" fn list_reset_v<const V: usize>(this: *mut c_void, allocator: *mut c_void, pso: *mut c_void) -> windows::core::HRESULT {
    reset_body(ORIG[4][V].load(Ordering::Acquire), this, allocator, pso)
}

unsafe extern "system" fn viewports(this: *mut c_void, n: u32, v: *const D3D12_VIEWPORT) {
    viewports_body(VP.original(), this, n, v)
}

/// The copy tracer (`copies.rs`): a `CopyTextureRegion` the game records, after the call (never our own).
unsafe fn copy_texture_region_body(
    orig: usize,
    this: *mut c_void,
    dst: *const D3D12_TEXTURE_COPY_LOCATION,
    x: u32,
    y: u32,
    z: u32,
    src: *const D3D12_TEXTURE_COPY_LOCATION,
    bx: *const D3D12_BOX,
) {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: CtrFn = std::mem::transmute::<usize, CtrFn>(orig);
    f(this, dst, x, y, z, src, bx);
    if !OURS.with(|o| o.get()) && !dst.is_null() && !src.is_null() && !crate::overlay::stopping() {
        let _ = std::panic::catch_unwind(|| {
            let at = |l: &D3D12_TEXTURE_COPY_LOCATION| -> (usize, u32) {
                let res: usize = std::mem::transmute_copy(&l.pResource);
                (res, if l.Type == D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX { l.Anonymous.SubresourceIndex } else { crate::copies::SUB_FOOTPRINT })
            };
            let (s, d) = (at(&*src), at(&*dst));
            if s.0 != 0 && d.0 != 0 {
                crate::copies::record(this, crate::copies::KIND_REGION, s.0, s.1, d.0, d.1, !bx.is_null());
            }
            // the staging ring: a copy out of a slot of ours (its list is watched for submission)
            if s.0 != 0 && crate::stagering::has_live() {
                crate::stagering::note_copy(this as usize, s.0);
            }
            // the allocation trace: how far into a traced staging buffer the game's copies reach, and where they go
            if s.0 != 0 && (*src).Type == D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT && crate::alloctrace::tracking() {
                let f = &(*src).Anonymous.PlacedFootprint;
                let bytes = f.Footprint.RowPitch as u64 * f.Footprint.Height.max(1) as u64 * f.Footprint.Depth.max(1) as u64;
                crate::alloctrace::note_copy(s.0, f.Offset, bytes, d.0, d.1, [x, y, z]);
            }
        });
    }
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
}

unsafe extern "system" fn copy_texture_region(
    this: *mut c_void,
    dst: *const D3D12_TEXTURE_COPY_LOCATION,
    x: u32,
    y: u32,
    z: u32,
    src: *const D3D12_TEXTURE_COPY_LOCATION,
    bx: *const D3D12_BOX,
) {
    copy_texture_region_body(CTR.original(), this, dst, x, y, z, src, bx)
}

unsafe extern "system" fn copy_texture_region_v<const V: usize>(
    this: *mut c_void,
    dst: *const D3D12_TEXTURE_COPY_LOCATION,
    x: u32,
    y: u32,
    z: u32,
    src: *const D3D12_TEXTURE_COPY_LOCATION,
    bx: *const D3D12_BOX,
) {
    copy_texture_region_body(ORIG[5][V].load(Ordering::Acquire), this, dst, x, y, z, src, bx)
}

/// A `CopyResource` the game records (the whole resource).
unsafe fn copy_resource_body(orig: usize, this: *mut c_void, dst: *mut c_void, src: *mut c_void) {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: CrFn = std::mem::transmute::<usize, CrFn>(orig);
    f(this, dst, src);
    if !OURS.with(|o| o.get()) && !dst.is_null() && !src.is_null() && !crate::overlay::stopping() {
        let _ = std::panic::catch_unwind(|| {
            crate::copies::record(this, crate::copies::KIND_RESOURCE, src as usize, crate::copies::SUB_ALL, dst as usize, crate::copies::SUB_ALL, false)
        });
    }
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
}

unsafe extern "system" fn copy_resource(this: *mut c_void, dst: *mut c_void, src: *mut c_void) {
    copy_resource_body(CR.original(), this, dst, src)
}

unsafe extern "system" fn copy_resource_v<const V: usize>(this: *mut c_void, dst: *mut c_void, src: *mut c_void) {
    copy_resource_body(ORIG[6][V].load(Ordering::Acquire), this, dst, src)
}

/// In a list's own table, where the slot held another function (the driver's, called directly: `ORIG[1][V]`).
unsafe extern "system" fn viewports_v<const V: usize>(this: *mut c_void, n: u32, v: *const D3D12_VIEWPORT) {
    viewports_body(ORIG[1][V].load(Ordering::Acquire), this, n, v)
}

unsafe extern "system" fn barrier(this: *mut c_void, n: u32, groups: *const D3D12_BARRIER_GROUP) {
    barrier_body(BAR.original(), this, n, groups)
}

unsafe extern "system" fn barrier_v<const V: usize>(this: *mut c_void, n: u32, groups: *const D3D12_BARRIER_GROUP) {
    barrier_body(ORIG[2][V].load(Ordering::Acquire), this, n, groups)
}

unsafe fn barrier_body(orig: usize, this: *mut c_void, n: u32, groups: *const D3D12_BARRIER_GROUP) {
    crate::overlay::INFLIGHT.fetch_add(1, Ordering::AcqRel);
    let f: BarrierFn = std::mem::transmute::<usize, BarrierFn>(orig);
    f(this, n, groups);
    if !OURS.with(|o| o.get()) && ENABLED.load(Ordering::Relaxed) && !groups.is_null() && n > 0 {
        let _ = std::panic::catch_unwind(|| crate::board::observe_enhanced(std::slice::from_raw_parts(groups, n as usize)));
        let _ = std::panic::catch_unwind(|| {
            for g in std::slice::from_raw_parts(groups, n as usize) {
                if g.Type != D3D12_BARRIER_TYPE_TEXTURE || g.NumBarriers == 0 || g.Anonymous.pTextureBarriers.is_null() {
                    continue;
                }
                for t in std::slice::from_raw_parts(g.Anonymous.pTextureBarriers, g.NumBarriers as usize) {
                    if t.LayoutBefore == D3D12_BARRIER_LAYOUT_DEPTH_STENCIL_WRITE && t.LayoutAfter != D3D12_BARRIER_LAYOUT_DEPTH_STENCIL_WRITE {
                        ENHANCED.fetch_add(1, Ordering::Relaxed);
                        let res: usize = std::mem::transmute_copy(&t.pResource);
                        if res != 0 {
                            note_end(res, true);
                        }
                    }
                }
            }
        });
    }
    crate::overlay::INFLIGHT.fetch_sub(1, Ordering::AcqRel);
}

/// The hooks, for the overlay's install and uninstall lists (in the order of [`hook_slots`]).
pub fn hooks() -> [(&'static VSlot, usize, &'static str); KINDS] {
    [
        (&RB, resource_barrier as *const () as usize, "ResourceBarrier"),
        (&VP, viewports as *const () as usize, "RSSetViewports"),
        (&BAR, barrier as *const () as usize, "Barrier"),
        (&OM, render_targets as *const () as usize, "OMSetRenderTargets"),
        (&RS, list_reset as *const () as usize, "Reset"),
        (&CTR, copy_texture_region as *const () as usize, "CopyTextureRegion"),
        (&CR, copy_resource as *const () as usize, "CopyResource"),
    ]
}

// ---------------------------------------------------------------- the game's lists' own tables
//
// In the game (2026-10-21 14:10) the hooks on the shared table never ran: D3D12Core gives each command list a table
// of its own, inside the list object (at the interface + 0x588 in the game's D3D12Core), copied from the shared table
// once (`0x18020d840`: the template by list type, then the driver's own functions put into about 40 slots for direct
// calls, RSSetViewports among them; ResourceBarrier and Barrier stay D3D12Core's) and switched to as the list's
// vtable while it is in use. So each list's own table is patched too, the first time the list is submitted
// (`ExecuteCommandLists`, hooked on the queue): the same three slots, each calling what that slot held (the driver's
// function, for RSSetViewports). The shared table's hooks are taken out again once a list's own table is seen, so that
// lists made later copy D3D12Core's functions, not ours (they are patched when submitted). Every table patched is
// remembered and put back at unload, and a last scan of the process's memory puts back any other copy of our
// functions found in a list's table (one copied from the shared table before it was put back).

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZEROS: [AtomicUsize; VARIANTS] = [ZERO; VARIANTS];
const VARIANTS: usize = 4;
const SEEN_SIZE: usize = 1024;

/// What lists' own tables held in a slot when it was not the shared table's function (variant 0 is the shared
/// table's, the `VSlot`'s original): by kind (the order of [`hooks`]), then variant.
static ORIG: [[AtomicUsize; VARIANTS]; KINDS] = [ZEROS; KINDS];
/// The shared table (D3D12Core's direct-list table, in its .data) and its first three slots (QueryInterface, AddRef,
/// Release): a list's own table starts the same.
pub(crate) static TEMPLATE: AtomicUsize = AtomicUsize::new(0);
static TEMPLATE_HEAD: [AtomicUsize; 3] = [ZERO; 3];
static TEMPLATE_RELEASED: AtomicBool = AtomicBool::new(false);
/// A list's own table, from its interface pointer (learnt from the first one seen; 0: not yet).
static OWN_OFFSET: AtomicUsize = AtomicUsize::new(0);
/// Lists' own tables patched or found holding our functions, for the unload.
static TABLES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Lists done, by their interface pointer (a small cache: a miss only costs a check).
static SEEN: [AtomicUsize; SEEN_SIZE] = [ZERO; SEEN_SIZE];
static ADOPTED: AtomicU64 = AtomicU64::new(0);
static PATCHED: AtomicU64 = AtomicU64::new(0);
static ODD: AtomicU64 = AtomicU64::new(0);
static RESTORED: AtomicU64 = AtomicU64::new(0);

fn kind_slot(kind: usize) -> usize {
    hook_slots()[kind]
}

fn hook_fn(kind: usize, v: usize) -> usize {
    let f: [[*const (); VARIANTS]; KINDS] = [
        [resource_barrier as *const (), resource_barrier_v::<1> as *const (), resource_barrier_v::<2> as *const (), resource_barrier_v::<3> as *const ()],
        [viewports as *const (), viewports_v::<1> as *const (), viewports_v::<2> as *const (), viewports_v::<3> as *const ()],
        [barrier as *const (), barrier_v::<1> as *const (), barrier_v::<2> as *const (), barrier_v::<3> as *const ()],
        [render_targets as *const (), render_targets_v::<1> as *const (), render_targets_v::<2> as *const (), render_targets_v::<3> as *const ()],
        [list_reset as *const (), list_reset_v::<1> as *const (), list_reset_v::<2> as *const (), list_reset_v::<3> as *const ()],
        [
            copy_texture_region as *const (),
            copy_texture_region_v::<1> as *const (),
            copy_texture_region_v::<2> as *const (),
            copy_texture_region_v::<3> as *const (),
        ],
        [copy_resource as *const (), copy_resource_v::<1> as *const (), copy_resource_v::<2> as *const (), copy_resource_v::<3> as *const ()],
    ];
    f[kind][v] as usize
}

fn shared_original(kind: usize) -> usize {
    [&RB, &VP, &BAR, &OM, &RS, &CTR, &CR][kind].original()
}

fn variant_original(kind: usize, v: usize) -> usize {
    if v == 0 {
        shared_original(kind)
    } else {
        ORIG[kind][v].load(Ordering::Acquire)
    }
}

/// Which of our functions `value` is, for a kind.
fn ours(kind: usize, value: usize) -> Option<usize> {
    (0..VARIANTS).find(|&v| hook_fn(kind, v) == value)
}

/// Remembers the shared table (at install, before it is hooked).
pub fn set_template(vt: usize) {
    for (k, h) in TEMPLATE_HEAD.iter().enumerate() {
        h.store(crate::win::read_ptr(vt + k * 8).unwrap_or(0), Ordering::Release);
    }
    TEMPLATE.store(vt, Ordering::Release);
}

/// Whether `t` is a command list table as D3D12Core makes them (its first three slots the shared table's).
fn head_ok(t: usize) -> bool {
    let mut head = [0u8; 24];
    crate::win::read_into(t, &mut head)
        && TEMPLATE_HEAD[0].load(Ordering::Acquire) != 0
        && (0..3).all(|k| usize::from_le_bytes(head[k * 8..k * 8 + 8].try_into().unwrap()) == TEMPLATE_HEAD[k].load(Ordering::Acquire))
}

/// Points one slot of a list's own table at our hook for it (calling what the slot held). True when it holds ours.
unsafe fn patch(t: usize, kind: usize) -> bool {
    let at = t + kind_slot(kind) * 8;
    let Some(cur) = crate::win::read_ptr(at) else { return false };
    if ours(kind, cur).is_some() {
        return true;
    }
    if shared_original(kind) == 0 {
        return false;
    }
    let v = if cur == shared_original(kind) {
        0
    } else if let Some(v) = (1..VARIANTS).find(|&v| ORIG[kind][v].load(Ordering::Acquire) == cur) {
        v
    } else {
        if !crate::vhook::is_code(cur) {
            ODD.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match (1..VARIANTS)
            .find(|&v| ORIG[kind][v].compare_exchange(0, cur, Ordering::AcqRel, Ordering::Acquire).is_ok() || ORIG[kind][v].load(Ordering::Acquire) == cur)
        {
            Some(v) => v,
            None => {
                ODD.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        }
    };
    if !writable(at) {
        ODD.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let cell = AtomicUsize::from_ptr(at as *mut usize);
    let ok = cell.compare_exchange(cur, hook_fn(kind, v), Ordering::AcqRel, Ordering::Acquire).is_ok();
    if ok {
        PATCHED.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// From the overlay's `ExecuteCommandLists` hook, for each list submitted (before it goes to the queue): patches the
/// list's own table the first time (see above).
///
/// # Safety
/// `list` must be a command list the game is submitting.
pub unsafe fn adopt(list: usize) {
    if !hooks_wanted() || crate::overlay::stopping() || TEMPLATE.load(Ordering::Acquire) == 0 {
        return;
    }
    let h = (list >> 4) % SEEN_SIZE;
    let known = SEEN[h].load(Ordering::Relaxed) == list;
    let Some(vt) = crate::win::read_ptr(list) else { return };
    // its own table: the one it uses now when that lies inside the object, else where the others' were
    let own = if vt > list && vt < list + 0x2000 && head_ok(vt) {
        if OWN_OFFSET.load(Ordering::Relaxed) == 0 {
            OWN_OFFSET.store(vt - list, Ordering::Release);
        }
        vt
    } else {
        let off = OWN_OFFSET.load(Ordering::Acquire);
        if off == 0 || !head_ok(list + off) {
            return; // the shared table (hooked there) or not a D3D12Core list
        }
        list + off
    };
    if known && crate::win::read_ptr(own + kind_slot(0) * 8).is_some_and(|v| ours(0, v).is_some()) {
        return;
    }
    let mut any = false;
    for kind in 0..KINDS {
        any |= patch(own, kind);
    }
    SEEN[h].store(list, Ordering::Relaxed);
    if any {
        if let Ok(mut t) = TABLES.lock() {
            if !t.contains(&own) && t.len() < 1 << 16 {
                t.push(own);
                ADOPTED.fetch_add(1, Ordering::Relaxed);
            }
        }
        // lists made from now on: D3D12Core's own functions in their copies, not ours
        if !TEMPLATE_RELEASED.swap(true, Ordering::AcqRel) {
            for (vs, f, _) in hooks() {
                let _ = vs.uninstall(f);
            }
        }
    }
}

/// Whether the 8 bytes at `at` (8-byte aligned) are in committed read-write memory of this process.
fn writable(at: usize) -> bool {
    at.is_multiple_of(8)
        && crate::win::query(at).is_some_and(|m| {
            m.State == crate::win::MEM_COMMIT
                && m.Protect & crate::win::PAGE_GUARD == 0
                && matches!(m.Protect & 0xFF, crate::win::PAGE_READWRITE | crate::win::PAGE_EXECUTE_READWRITE)
        })
}

/// Puts back one table's slots that hold our functions. The number of slots put back.
fn restore_table(t: usize) -> u64 {
    if !head_ok(t) {
        return 0;
    }
    let mut n = 0;
    for kind in 0..KINDS {
        let at = t + kind_slot(kind) * 8;
        let Some(cur) = crate::win::read_ptr(at) else { continue };
        if let Some(v) = ours(kind, cur) {
            let orig = variant_original(kind, v);
            // SAFETY: an aligned slot of a list table in committed read-write memory
            if orig != 0
                && writable(at)
                && unsafe { AtomicUsize::from_ptr(at as *mut usize) }.compare_exchange(cur, orig, Ordering::AcqRel, Ordering::Acquire).is_ok()
            {
                n += 1;
            }
        }
    }
    n
}

/// At unload, after the hooks stopped: every list table patched put back, then (with `scan`) any other table found
/// holding one of our functions in this process's memory (resident pages, at most 20 s). A note for the log.
pub fn restore_tables(scan: bool) -> String {
    let tables: Vec<usize> = TABLES.lock().map(|t| t.clone()).unwrap_or_default();
    let mut slots: u64 = tables.iter().map(|&t| restore_table(t)).sum();
    let mut note = format!("{} list table(s) put back ({slots} slots)", tables.len());
    if scan {
        let module = crate::worker::MODULE.load(Ordering::Acquire);
        if let Some((lo, hi)) = crate::win::module_range(module as crate::win::HANDLE) {
            if let Ok(mut s) = crate::scan::WordScan::with(crate::scan::Words::Range(lo as u64, hi as u64), false, 1 << 20) {
                let t0 = std::time::Instant::now();
                while !s.step(1 << 30, 1000) && t0.elapsed().as_secs() < 20 {}
                let mut more = 0;
                for &a in &s.found {
                    let Some(v) = crate::win::read_ptr(a) else { continue };
                    for kind in 0..KINDS {
                        if ours(kind, v).is_some() {
                            more += restore_table(a - kind_slot(kind) * 8);
                        }
                    }
                }
                slots += more;
                note = format!(
                    "{note}; the memory scan ({} MB, {} pointer(s) into the DLL, {} ms) put back {more} more",
                    s.bytes >> 20,
                    s.found.len(),
                    t0.elapsed().as_millis()
                );
            }
        }
    }
    RESTORED.fetch_add(slots, Ordering::Relaxed);
    if let Ok(mut t) = TABLES.lock() {
        t.clear();
    }
    for s in &SEEN {
        s.store(0, Ordering::Relaxed);
    }
    note
}

/// From the overlay's `ExecuteCommandLists` hook, after the lists went to the queue: the newest copy submitted.
pub fn on_execute(queue: *mut c_void, lists: &[*mut c_void]) {
    if PENDING_N.load(Ordering::Acquire) == 0 {
        return;
    }
    let Ok(mut p) = PENDING.lock() else { return };
    let now = PRESENT_NO.load(Ordering::Acquire);
    let submit_us = crate::diag::now_us();
    let mut carried = Vec::new();
    for &l in lists {
        while let Some(i) = p.iter().position(|x| x.list == l as usize) {
            carried.push(p.remove(i));
        }
    }
    PENDING_N.store(p.len() as u32, Ordering::Release);
    drop(p);
    if carried.is_empty() {
        return;
    }
    // the queue's fence signalled after these lists: the overlay frees a replaced copy set only once every copy into
    // it has run, and a draw on another queue waits for it (`wait_on`) before reading the copy
    let fence = signal_after(queue);
    let draw_queue = DRAW_QUEUE.load(Ordering::Acquire);
    let other_queue = draw_queue != 0 && draw_queue != queue as usize;
    if other_queue {
        COPIES_OTHER_QUEUE.fetch_add(carried.len() as u64, Ordering::Relaxed);
    }
    // a copy from another queue is usable through its fence; without one (no fence could be made) it is not
    let usable = !other_queue || fence != 0;
    if let Ok(mut latest) = LATEST.lock() {
        for x in carried {
            SUBMITTED.fetch_add(1, Ordering::Relaxed);
            let picture = picture_of(now, LAST_PICTURE.load(Ordering::Acquire));
            LAST_PICTURE.store(picture, Ordering::Release);
            note_signal(x.generation, queue as usize, fence);
            // the slot is written again: an older entry on it no longer holds what it says
            latest.retain(|l| l.slot != x.slot || l.generation != x.generation);
            latest.push_back(Latest {
                slot: x.slot,
                at: x.at,
                viewport: x.viewport,
                generation: x.generation,
                picture,
                submitted: now,
                queue: queue as usize,
                submit_us,
                end_us: x.end_us,
                fence,
                src: x.src,
                usable,
                cam: x.cam,
                board: x.board,
            });
            while latest.len() > RING {
                latest.pop_front();
            }
        }
    }
}

/// For each of the last copy sets (by generation): on each queue copies into it went to, the last fence value
/// signalled after one.
static SIGNALS_BY_SET: Mutex<Vec<(u64, Vec<(usize, u64)>)>> = Mutex::new(Vec::new());

fn note_signal(generation: u64, queue: usize, value: u64) {
    let Ok(mut s) = SIGNALS_BY_SET.lock() else { return };
    let i = match s.iter().position(|e| e.0 == generation) {
        Some(i) => i,
        None => {
            if s.len() >= 4 {
                s.remove(0);
            }
            s.push((generation, Vec::new()));
            s.len() - 1
        }
    };
    match s[i].1.iter_mut().find(|q| q.0 == queue) {
        Some(q) => q.1 = q.1.max(value),
        None => s[i].1.push((queue, value)),
    }
}

/// Whether every copy recorded into a set has run: none still waits to be submitted (whatever its age: a list the
/// game keeps may be submitted much later; `Reset` drops what is discarded), and on each queue the fence signalled
/// after the last submission carrying one has passed. `stale`: the oldest pending copy's age in presents, for the log.
pub fn set_done(generation: u64) -> (bool, u64) {
    let now = PRESENT_NO.load(Ordering::Acquire);
    let oldest = PENDING.lock().map(|p| p.iter().filter(|x| x.generation == generation).map(|x| now.saturating_sub(x.at)).max()).unwrap_or(Some(u64::MAX));
    if let Some(age) = oldest {
        return (false, age);
    }
    let queues: Vec<(usize, u64)> =
        SIGNALS_BY_SET.lock().map(|s| s.iter().find(|e| e.0 == generation).map(|e| e.1.clone()).unwrap_or_default()).unwrap_or_default();
    (queues.iter().all(|&(q, v)| fence_passed(q, v)), 0)
}

/// The copies submitted lately from the current copies, newest first (whatever their age: the overlay decides).
pub fn history(generation: u64) -> Vec<Latest> {
    LATEST.lock().map(|l| l.iter().rev().filter(|x| x.generation == generation).copied().collect()).unwrap_or_default()
}

/// A main depth that ended a pass within this many presents is still in use.
const FRESH: u64 = 2;

/// A new main depth is followed only once it has ended passes in this many present windows in a row.
pub const SWAP_STREAK: u64 = 24;

/// Picks the main depth among the candidates (at Present): a usable depth target with the back buffer's aspect and at
/// least a quarter of its width. The current one is kept while it still ends passes (within [`FRESH`] presents); when
/// it stops (the game made its main depth anew: seen in game after 98,000 presents), the largest that has ended passes
/// in [`SWAP_STREAK`] windows in a row takes over (then the one seen last, then the most passes); until one has, the
/// current one stays (no copies come: the test uses the last one a while, then goes off). A pause or a menu makes and
/// drops depths at other sizes within a second (2026-10-21 18:29): they are never chased. Returns it and its shape.
pub fn choose(back_w: u32, back_h: u32) -> Option<(usize, Desc, u64)> {
    BACK_SIZE.store(((back_w as u64) << 32) | back_h as u64, Ordering::Release);
    let now = PRESENT_NO.load(Ordering::Acquire);
    let list = CANDIDATES.lock().ok()?;
    let aspect = back_w as f32 / back_h.max(1) as f32;
    let fits = |c: &&Candidate| {
        let a = c.desc.width as f32 / c.desc.height.max(1) as f32;
        c.usable && !c.enhanced && (a / aspect - 1.0).abs() < 0.03 && c.desc.width * 4 >= back_w
    };
    let current = CHOSEN.load(Ordering::Acquire);
    let area = |c: &Candidate| c.desc.width as u64 * c.desc.height as u64;
    let steady = |c: &&Candidate| c.last + FRESH >= now && c.streak >= SWAP_STREAK;
    let current_c = list.iter().filter(fits).find(|c| c.res == current);
    let kept = current_c.filter(|c| c.last + FRESH >= now);
    // the current one, unless a larger steady one appeared (a resolution change)
    let larger = kept.and_then(|k| list.iter().filter(fits).filter(steady).filter(|c| area(c) > area(k)).max_by_key(|c| (area(c), c.last, c.ends)));
    let best = larger
        .or(kept)
        .or_else(|| list.iter().filter(fits).filter(steady).max_by_key(|c| (area(c), c.last, c.ends)))
        .or(current_c)
        .map(|c| (c.res, c.desc, c.streak));
    let why = best.filter(|b| b.0 != current).map(|b| {
        list.iter().find(|c| c.res == b.0).map_or(String::new(), |c| {
            format!("{}x{}, streak {}, ended a pass {} present(s) ago, {} passes", c.desc.width, c.desc.height, c.streak, now.saturating_sub(c.last), c.ends)
        })
    });
    drop(list);
    let chosen = best.map_or(0, |b| b.0);
    if chosen != current && current != 0 && chosen != 0 {
        note_change(format!("main depth {current:#x} -> {chosen:#x} (the old one stopped ending passes; the new one {})", why.unwrap_or_default()));
    }
    CHOSEN.store(chosen, Ordering::Release);
    MAIN_SIZE.store(best.map_or(0, |b| ((b.1.width as u64) << 32) | b.1.height as u64), Ordering::Release);
    best
}

/// `draw depth on|off`: the command list hooks on the shared table (the template every new list copies) are in place
/// only while the test is on; off, they are taken out, so lists made meanwhile copy D3D12Core's own functions (the
/// lists' own tables already patched keep calling through our hooks, which pass through while off, and are put back
/// at unload). From the worker thread. A note for the log.
/// Whether the command lists' hooks are wanted: the depth test, or the staging ring and the allocation trace, which
/// read the copies these hooks see (the depth copies themselves stay gated by `ENABLED`). Without them the ring is
/// blind: no copy is ever seen, no slot ever recycled (the freeze of 2026-10-23 17:53, with the depth test off).
pub fn hooks_wanted() -> bool {
    ENABLED.load(Ordering::Relaxed) || crate::stagering::active() || crate::alloctrace::tracing()
}

/// Whether the copy hooks reach the game's lists now (the shared table hooked, or lists adopted).
pub fn copy_hooks_active() -> bool {
    RB.installed() || CTR.installed() || ADOPTED.load(Ordering::Relaxed) > 0
}

pub fn set_shared_hooks(on: bool) -> String {
    let vt = TEMPLATE.load(Ordering::Acquire);
    if vt == 0 {
        return "no command list table known yet".into();
    }
    if on {
        if TEMPLATE_RELEASED.load(Ordering::Acquire) {
            return "the lists' own tables are hooked (the shared table was put back)".into();
        }
        let mut n = 0;
        for (slot, (vs, f, _)) in hook_slots().into_iter().zip(hooks()) {
            if !vs.installed() && (vs.original() != 0 || BAR.original() == 0 || slot != kind_slot(2)) {
                // SAFETY: the slot was hooked before (or is the one install hooked); the table is D3D12Core's
                if unsafe { vs.install(vt + slot * 8, f) }.is_ok() {
                    n += 1;
                }
            }
        }
        format!("{n} shared table slot(s) hooked again")
    } else {
        let mut n = 0;
        for (vs, f, _) in hooks() {
            if vs.installed() && unsafe { vs.uninstall(f) }.is_ok() {
                n += 1;
            }
        }
        format!("{n} shared table slot(s) put back while the test is off")
    }
}

/// Clears everything learnt (unload; a new device).
pub fn reset() {
    CHOSEN.store(0, Ordering::Release);
    LAST_PICTURE.store(0, Ordering::Release);
    if let Ok(mut f) = QUEUE_FENCES.lock() {
        f.clear();
    }
    if let Ok(mut s) = SIGNALS_BY_SET.lock() {
        s.clear();
    }
    if let Ok(mut l) = CANDIDATES.lock() {
        l.clear();
    }
    if let Ok(mut p) = PENDING.lock() {
        p.clear();
    }
    PENDING_N.store(0, Ordering::Release);
    if let Ok(mut l) = LATEST.lock() {
        l.clear();
    }
    SIGNATURE.store(0, Ordering::Release);
    if let Ok(mut f) = FIRSTS.lock() {
        f.0.clear();
    }
}

/// Whether the barrier hooks are in place: on the shared table, or in lists' own tables.
pub fn hooked() -> bool {
    RB.installed() || ADOPTED.load(Ordering::Relaxed) > 0
}

/// Whether copies were ever recorded into the game's lists (then unload waits a few frames before freeing them).
pub fn injected_any() -> bool {
    INJECTED.load(Ordering::Relaxed) > 0
}

// ---------------------------------------------------------------- the scale, and the deck check

/// The game's projection (build 20260929; read from its memory 2026-10-21 15:4x, `caveman-trainer probe-projection`:
/// every projection matrix the game keeps for the main camera has these depth terms): reversed depth, the near plane at
/// 0.06 m, a far term of 0.000005 (a far plane about 12 km away). A stored depth d at view distance z is
/// d = 0.06 / z - 0.000005, so z = 0.06 / (d + 0.000005).
pub const GAME_NEAR: f32 = 0.06;
pub const GAME_FAR_TERM: f32 = 0.000005;

/// The depth test's last changes (on, off and why), newest last, with the present count.
static CHANGES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Notes a change of the depth test (shown in the status, and the DLL's log).
pub fn note_change(what: String) {
    let line = format!("present {}: {what}", PRESENT_NO.load(Ordering::Relaxed));
    if let Ok(mut c) = CHANGES.lock() {
        if c.len() >= 12 {
            c.pop_front();
        }
        c.push_back(line.clone());
    }
    crate::overlay::log_later(format!("depth test {line}"));
}

/// (x, y) of the shader's view depth = y / (d - x): the game's projection, or the near plane set by hand.
pub fn linear() -> (f32, f32) {
    let manual = f32::from_bits(NEAR_OVERRIDE.load(Ordering::Relaxed));
    (-GAME_FAR_TERM, if manual > 0.0 { manual } else { GAME_NEAR })
}

/// A check of the depth copy against the deck: the depth stored at points of the deck whose view depth is known says
/// what near plane the copy implies (d x z, reversed). With the right texels it comes out at the game's 0.06 m (a little
/// more: the deck's top is a centimetre or two above the bars' plane); with wrong texels (a wrong viewport) it scatters.
/// It no longer switches the depth test off (the scale is the game's own): it only tells which way of finding the texel
/// is right, and shows in the status.
#[derive(Default)]
pub struct Calibration {
    samples: VecDeque<(f32, f32)>,
    /// The median near plane the samples imply, and the middle half's spread as a share of it.
    pub median: f32,
    pub spread: f32,
}

impl Calibration {
    pub const KEEP: usize = 300;
    pub const NEED: usize = 40;

    /// A stored depth value `d` where the view depth is `z` metres.
    pub fn add(&mut self, d: f32, z: f32) {
        if !(d.is_finite() && z.is_finite() && d > 0.0 && d < 1.0 && z > 0.3 && z < 60.0) {
            return;
        }
        if self.samples.len() >= Self::KEEP {
            self.samples.pop_front();
        }
        self.samples.push_back((d, z));
        self.solve();
    }

    pub fn samples(&self) -> usize {
        self.samples.len()
    }

    fn solve(&mut self) {
        if self.samples.len() < Self::NEED {
            return;
        }
        let mut k: Vec<f32> = self.samples.iter().map(|(d, z)| (d + GAME_FAR_TERM) * z).collect();
        k.sort_by(|a, b| a.total_cmp(b));
        let at = |p: f32| k[((k.len() - 1) as f32 * p).round() as usize];
        self.median = at(0.5);
        self.spread = if self.median > 0.0 { (at(0.75) - at(0.25)) / self.median } else { f32::INFINITY };
    }

    /// Whether enough samples agree with a near plane `n` (the median within 15 %, the middle half within 25 %).
    pub fn agrees(&self, n: f32) -> bool {
        self.samples.len() >= Self::NEED && (self.median / n - 1.0).abs() < 0.15 && self.spread < 0.25
    }

    /// How far off the samples are from `n` (for choosing between two ways): the median's error plus the spread.
    pub fn error(&self, n: f32) -> f32 {
        if self.samples.len() < Self::NEED {
            return f32::INFINITY;
        }
        (self.median / n - 1.0).abs() + self.spread
    }
}

// ---------------------------------------------------------------- which frame a copy belongs to, and lapses
//
// Each copy carries the renderer's camera and board proxies as they were when it was recorded (`Snap`: the frame's
// matrix at +0x50 and the previous frame's at +0x90). The overlay draws with the proxies' matrices read at Present;
// the frame's own copy is the one whose camera matrix is byte for byte the one drawn with (`Pairing::Exact`); a copy
// whose camera matrix is the one the proxy now holds as its previous frame's is one frame old (`OneOld`); a copy whose
// previous matrix is the one drawn with was recorded a frame ahead of the picture (`Ahead`: the next frame's G-buffer
// recorded before this present). With no match (the proxies not found, or a copy from before they changed twice) the
// picture count decides (`picture_of`).

/// How a copy pairs with the frame drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pairing {
    Exact,
    OneOld,
    Ahead,
    None,
}

impl Pairing {
    pub fn name(self) -> &'static str {
        match self {
            Pairing::Exact => "exact",
            Pairing::OneOld => "one old",
            Pairing::Ahead => "ahead",
            Pairing::None => "none",
        }
    }

    /// How a copy taken with the camera proxy at `copy` pairs with a frame drawn with `drawn` (the camera proxy's
    /// matrix now) whose proxy now holds `prev` as the previous frame's.
    pub fn of(copy: &Snap, drawn: &[u8; 64], prev: &[u8; 64]) -> Pairing {
        if !copy.taken {
            Pairing::None
        } else if copy.now == *drawn {
            Pairing::Exact
        } else if copy.now == *prev {
            Pairing::OneOld
        } else if copy.prev == *drawn {
            Pairing::Ahead
        } else {
            Pairing::None
        }
    }
}

/// How the depth test went at a present the bars were drawn at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Use {
    /// The frame's own copy.
    Fresh,
    /// An older copy (presents older than the frame), taken in its own camera.
    Late(u64),
    /// No test (bars drawn over everything).
    Off,
}

/// The depth test's frames, and its short lapses (the blinks: a few presents late or off between fresh ones).
#[derive(Debug, Default)]
pub struct Lapses {
    pub fresh: u64,
    pub late: u64,
    pub off: u64,
    pub blinks: u64,
    /// The lapse going on: where it started, how long, the oldest copy, and why when off.
    run: Option<(u64, u64, u64, String)>,
    /// The last lapses, newest last.
    pub last: VecDeque<String>,
}

impl Lapses {
    /// A lapse at most this long between fresh frames is a blink.
    pub const BLINK: u64 = 10;

    pub fn note(&mut self, present: u64, u: Use, why: &str) {
        match u {
            Use::Fresh => {
                self.fresh += 1;
                if let Some((start, len, oldest, reason)) = self.run.take() {
                    let what = if reason.is_empty() { format!("late (copies up to {oldest} presents old)") } else { format!("off ({reason})") };
                    let blink = len <= Self::BLINK;
                    if blink {
                        self.blinks += 1;
                    }
                    self.last.push_back(format!("present {start}: {len} present(s) {what}{}", if blink { "" } else { ", not a blink" }));
                    if self.last.len() > 8 {
                        self.last.pop_front();
                    }
                }
            }
            Use::Late(age) => {
                self.late += 1;
                let r = self.run.get_or_insert((present, 0, 0, String::new()));
                r.1 += 1;
                r.2 = r.2.max(age);
            }
            Use::Off => {
                self.off += 1;
                let r = self.run.get_or_insert((present, 0, 0, String::new()));
                r.1 += 1;
                if r.3.is_empty() {
                    r.3 = why.to_string();
                }
            }
        }
    }

    pub fn status(&self) -> Value {
        json!({"fresh": self.fresh, "late": self.late, "off": self.off, "blinks": self.blinks, "last_lapses": self.last})
    }
}

// ---------------------------------------------------------------- status

pub fn status() -> Value {
    let now = PRESENT_NO.load(Ordering::Relaxed);
    let chosen = CHOSEN.load(Ordering::Relaxed);
    // the candidates copied out under the lock (a game thread's barrier takes it): the JSON built after
    let list: Vec<Candidate> = CANDIDATES.try_lock().map(|l| l.clone()).unwrap_or_default();
    let candidates: Vec<Value> = list
        .iter()
        .map(|c| {
            json!({
                "resource": format!("{:#x}", c.res),
                "size": [c.desc.width, c.desc.height],
                "format": c.desc.format,
                "usable": c.usable,
                "enhanced_barriers": c.enhanced,
                "passes": c.ends,
                "presents_ago": now.saturating_sub(c.last),
                "streak": c.streak,
                "viewport": c.viewport,
                "chosen": c.res == chosen,
            })
        })
        .collect();
    let copies = COPIES.try_lock().ok().and_then(|g| {
        g.as_ref().map(|c| json!({"size": [c.size.0, c.size.1], "format": c.format.0, "generation": c.generation, "mb": c.bytes() as f64 / 1_048_576.0}))
    });
    let latest = LATEST.try_lock().ok().and_then(|g| g.back().copied()).map(|l| json!({"slot": l.slot, "presents_ago": now.saturating_sub(l.at), "picture": l.picture, "submitted": l.submitted, "queue": format!("{:#x}", l.queue), "usable": l.usable, "paired": l.cam.taken, "viewport": l.viewport}));
    let pending: Vec<Value> = PENDING
        .try_lock()
        .map(|p| p.iter().map(|x| json!({"list": format!("{:#x}", x.list), "presents_ago": now.saturating_sub(x.at), "generation": x.generation})).collect())
        .unwrap_or_default();
    let sig = SIGNATURE.load(Ordering::Relaxed);
    let ends_log: Vec<Value> = ENDS_LOG
        .try_lock()
        .map(|l| l.iter().map(|(w, v)| json!({"window": w, "ends": v.iter().map(|(s, c)| format!("{s:#x} {}", match c { 'c' => "copied", 'x' => "later", 's' => "other state", _ => "not copyable" })).collect::<Vec<_>>()})).collect())
        .unwrap_or_default();
    json!({
        "enabled": ENABLED.load(Ordering::Relaxed),
        "hooked": RB.installed() || ADOPTED.load(Ordering::Relaxed) > 0,
        "shared_table": if TEMPLATE_RELEASED.load(Ordering::Relaxed) { "put back (the lists' own tables are hooked)" } else if RB.installed() { "hooked" } else { "not hooked" },
        "lists_adopted": ADOPTED.load(Ordering::Relaxed),
        "lists_created": CREATED.load(Ordering::Relaxed),
        "lists_adopted_at_create": ADOPTED_AT_CREATE.load(Ordering::Relaxed),
        "slots_patched": PATCHED.load(Ordering::Relaxed),
        "own_table_offset": format!("{:#x}", OWN_OFFSET.load(Ordering::Relaxed)),
        "driver_functions": (0..KINDS).map(|k| (1..VARIANTS).filter(|&v| ORIG[k][v].load(Ordering::Relaxed) != 0).count()).collect::<Vec<_>>(),
        "slots_skipped": ODD.load(Ordering::Relaxed),
        "slots_restored": RESTORED.load(Ordering::Relaxed),
        "enhanced_hooked": BAR.original() != 0,
        "near_override": f32::from_bits(NEAR_OVERRIDE.load(Ordering::Relaxed)),
        "candidates": candidates,
        "chosen": format!("{chosen:#x}"),
        "copies": copies,
        // the video memory the depth copies hold (the set in use and a replaced one not yet freed), the sets made, and why
        // none is being made
        "copies_mb": (HELD_BYTES.load(Ordering::Relaxed) as f64 / 1_048_576.0 * 10.0).round() / 10.0,
        "copy_sets_alive": SETS_ALIVE.load(Ordering::Relaxed),
        "copy_sets_made": SETS_MADE.load(Ordering::Relaxed),
        "copy_sets_leaked": SETS_LEAKED.load(Ordering::Relaxed),
        "copies_held_back": HELD_BACK.lock().map(|h| h.clone()).unwrap_or_default(),
        "latest": latest,
        "passes_ended": ENDS.load(Ordering::Relaxed),
        "copies_recorded": INJECTED.load(Ordering::Relaxed),
        "later_pass_ends_skipped": EXTRA.load(Ordering::Relaxed),
        "changes": CHANGES.lock().map(|c| c.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
        "scene_viewport": scene_viewport(),
        "copies_submitted": SUBMITTED.load(Ordering::Relaxed),
        // the copies' lock was held when a copy was due: it waited (before 2026-10-21 evening the copy was skipped, and
        // the window's next pass end, another stage of the depth, copied instead)
        "lock_waits": CONTENDED.load(Ordering::Relaxed),
        "gbuffer_state": if sig == 0 { "not learnt yet (any)".to_string() } else { format!("{:#x}", sig & 0xffff_ffff) },
        "other_state_ends": MISMATCHED.load(Ordering::Relaxed),
        "pass_ends_by_window": ends_log,
        "skipped_not_direct": NOT_DIRECT.load(Ordering::Relaxed),
        "copies_per": if COPY_PER_PICTURE.load(Ordering::Relaxed) { "picture" } else { "window" },
        "second_gbuffer_ends": SECOND_GBUFFER.load(Ordering::Relaxed),
        "write_state_ends": WRITE_STATE_ENDS.load(Ordering::Relaxed),
        "copies_other_queue": COPIES_OTHER_QUEUE.load(Ordering::Relaxed),
        "cross_queue_waits": WAITS.load(Ordering::Relaxed),
        "slot_reuse_waits": REUSE_WAITS.load(Ordering::Relaxed),
        "hook_calls": {"barriers": RB_CALLS.load(Ordering::Relaxed), "barriers_while_ours": RB_OURS.load(Ordering::Relaxed), "render_targets": OM_CALLS.load(Ordering::Relaxed), "viewports": VP_CALLS.load(Ordering::Relaxed), "resets": RS_CALLS.load(Ordering::Relaxed)},
        "cross_queue_waits_failed": WAITS_FAILED.load(Ordering::Relaxed),
        "draw_queue": format!("{:#x}", DRAW_QUEUE.load(Ordering::Relaxed)),
        "queue_fences_signalled": SIGNALS.load(Ordering::Relaxed),
        "pending": pending,
        "pending_dropped_at_reset": DROPPED_AT_RESET.load(Ordering::Relaxed),
        "enhanced_barriers": ENHANCED.load(Ordering::Relaxed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_match_the_d3d12_headers() {
        // d3d12.h: ID3D12GraphicsCommandList's RSSetViewports is method 21, ResourceBarrier 26; ID3D12GraphicsCommandList7's
        // Barrier 80
        assert_eq!((slot_viewports(), slot_resource_barrier(), slot_barrier()), (21, 26, 80));
        // Reset 10, OMSetRenderTargets 46 (neither among the slots D3D12Core fills with the driver's functions)
        assert_eq!((slot_reset(), slot_render_targets()), (10, 46));
        // CopyTextureRegion 16, CopyResource 17 (the copy tracer)
        assert_eq!((slot_copy_texture_region(), slot_copy_resource()), (16, 17));
        assert_eq!(hook_slots(), [26, 21, 80, 46, 10, 16, 17]);
        // the device's CreateCommandList is method 12 of ID3D12Device, CreateCommandList1 method 51 of ID3D12Device4
        assert_eq!((slot_create_command_list(), slot_create_command_list1()), (12, 51));
    }

    #[test]
    fn the_deck_check_tells_right_texels_from_wrong() {
        // the game's projection: the deck seen through the right texels gives back 0.06 (a quarter of the samples are a
        // leg in front or the ground behind)
        let stored = |z: f32| GAME_NEAR / z - GAME_FAR_TERM;
        let mut right = Calibration::default();
        for k in 0..200 {
            let z = 2.0 + (k % 7) as f32 * 0.1;
            let d = match k % 8 {
                0 => stored(z - 0.4),
                1 => stored(z + 0.6),
                _ => stored(z),
            };
            right.add(d, z);
        }
        assert!((right.median - GAME_NEAR).abs() < 0.001 && right.agrees(GAME_NEAR), "{} {}", right.median, right.spread);
        // wrong texels: the depth of other places, all over
        let mut wrong = Calibration::default();
        for k in 0..200 {
            wrong.add(stored(0.8 + (k % 13) as f32 * 0.5), 2.3);
        }
        assert!(!wrong.agrees(GAME_NEAR) && wrong.error(GAME_NEAR) > right.error(GAME_NEAR));
        // too few: no verdict
        let mut few = Calibration::default();
        few.add(stored(2.0), 2.0);
        assert!(!few.agrees(GAME_NEAR) && few.error(GAME_NEAR).is_infinite());
        // the scale: the game's, or the one set by hand
        assert_eq!(linear(), (-GAME_FAR_TERM, GAME_NEAR));
    }

    #[test]
    fn copyable_formats() {
        let d = |f: DXGI_FORMAT| Desc { width: 1920, height: 1080, format: f.0 }.copy_format();
        assert_eq!(d(DXGI_FORMAT_R32_TYPELESS), Some(DXGI_FORMAT_R32_FLOAT));
        assert_eq!(d(DXGI_FORMAT_D32_FLOAT_S8X24_UINT), Some(DXGI_FORMAT_R32_FLOAT));
        assert_eq!(d(DXGI_FORMAT_D16_UNORM), Some(DXGI_FORMAT_R16_UNORM));
        assert_eq!(d(DXGI_FORMAT_R24G8_TYPELESS), None);
    }

    #[test]
    fn lapses_count_the_blinks() {
        let mut l = Lapses::default();
        for p in 0..5 {
            l.note(p, Use::Fresh, "");
        }
        l.note(5, Use::Late(1), "");
        l.note(6, Use::Late(2), "");
        l.note(7, Use::Fresh, "");
        assert_eq!((l.fresh, l.late, l.blinks), (6, 2, 1));
        assert_eq!(l.last.back().unwrap(), "present 5: 2 present(s) late (copies up to 2 presents old)");
        // a long stretch off (a menu) is not a blink
        for p in 8..40 {
            l.note(p, Use::Off, "no copy this frame (the game's depth pass was not seen)");
        }
        l.note(40, Use::Fresh, "");
        assert_eq!((l.off, l.blinks), (32, 1));
        assert!(l.last.back().unwrap().ends_with("not a blink"), "{:?}", l.last);
        l.note(41, Use::Off, "no copy this frame (the newest is 31 presents old)");
        l.note(42, Use::Fresh, "");
        assert_eq!(l.blinks, 2);
        assert!(l.last.back().unwrap().contains("1 present(s) off (no copy this frame (the newest is 31"), "{:?}", l.last);
    }

    /// The globals (one test: the learning and the choice share them).
    #[test]
    fn learns_the_gbuffer_state_and_follows_a_new_main_depth() {
        reset();
        // each window: the G-buffer's end (to shader resource) first, then two others (to depth read)
        for w in 1000..1040u64 {
            assert!(note_main_end(w, PIXEL_SHADER_RESOURCE));
            assert!(!note_main_end(w, 0x20));
            assert!(!note_main_end(w, 0x20));
        }
        assert!(is_gbuffer_end(PIXEL_SHADER_RESOURCE) && !is_gbuffer_end(0x20), "{:#x}", SIGNATURE.load(Ordering::Relaxed));
        // no state dominating: anything goes
        for w in 2000..2064u64 {
            note_main_end(w, if w % 2 == 0 { PIXEL_SHADER_RESOURCE } else { 0x20 });
        }
        assert!(is_gbuffer_end(0x20) && is_gbuffer_end(PIXEL_SHADER_RESOURCE));
        reset();
        assert!(is_gbuffer_end(0x20));

        // the main depth: kept while it ends passes; when the game makes it anew, the new one at once
        let desc = Desc { width: 1600, height: 900, format: DXGI_FORMAT_R32_TYPELESS.0 };
        let cand = |res: usize, last: u64, streak: u64| Candidate { res, desc, usable: true, ends: 100, last, viewport: [0.0; 4], enhanced: false, streak };
        PRESENT_NO.store(500, Ordering::Release);
        CANDIDATES.lock().unwrap().extend([cand(0xA000, 499, 40), cand(0xB000, 450, 1)]);
        assert_eq!(choose(1920, 1080).map(|c| c.0), Some(0xA000));
        // B starts ending passes too (same size): A is kept while it still does
        CANDIDATES.lock().unwrap()[1].last = 499;
        CANDIDATES.lock().unwrap()[1].ends = 10_000;
        assert_eq!(choose(1920, 1080).map(|c| c.0), Some(0xA000));
        // A stops: B only once it has ended passes in SWAP_STREAK windows in a row (a pause makes and drops depths)
        PRESENT_NO.store(502, Ordering::Release);
        CANDIDATES.lock().unwrap()[1].last = 501;
        CANDIDATES.lock().unwrap()[1].streak = 5;
        assert_eq!(choose(1920, 1080).map(|c| c.0), Some(0xA000));
        CANDIDATES.lock().unwrap()[1].streak = SWAP_STREAK;
        assert_eq!(choose(1920, 1080).map(|c| c.0), Some(0xB000));
        assert!(CHANGES.lock().unwrap().iter().any(|c| c.contains("0xa000 -> 0xb000")));
        // nothing ends passes for a while (a loading screen): the last one stays
        PRESENT_NO.store(560, Ordering::Release);
        assert_eq!(choose(1920, 1080).map(|c| c.0), Some(0xB000));
        reset();
        PRESENT_NO.store(0, Ordering::Release);
    }
}
