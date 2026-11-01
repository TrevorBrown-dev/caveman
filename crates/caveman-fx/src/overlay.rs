//! The overlay: Caveman FX draws into the game's frames (the "DLL draws" route of docs/fx-hook-plan.md).
//!
//! **Hooks.** The game presents through NVIDIA Streamline's swap chain wrapper (`sl.interposer.dll`), which calls
//! DXGI's swap chain; ReSkate draws its ImGui overlay from code detours on both `Present` functions (MinHook style,
//! `Extension/UI/Overlay/overlay.cpp`), and the Steam overlay detours DXGI's too. We patch no code: we swap vtable
//! slots (as [`crate::hook`] does for the getter) and call whatever the slot held, so every detour still runs:
//!
//! - DXGI's swap chain vtable (read from a dummy swap chain made on a dummy queue: a composition swap chain, no
//!   window; a hidden window if that fails): `Present` (8) and `Present1` (22);
//! - Streamline's wrapper vtable (found by its RTTI name in `sl.interposer.dll`), the same two slots: the game calls
//!   these first, on its own thread, before ReSkate's detour draws, so our quads go under ReSkate's menus. When the
//!   wrapper hook draws, the DXGI hook below it does not (a thread-local depth guard and a time stamp);
//! - the D3D12 command queue's `ExecuteCommandLists` (10): which queues the game submits on, and which of our depth
//!   copies went to the GPU last;
//! - the command list's `ResourceBarrier`, `RSSetViewports` and `Barrier`: the game's depth ([`crate::depth`]).
//!
//! When our dummy swap chain comes back as Streamline's wrapper (it did in the game on 2026-10-21), the swap chain it
//! wraps is the one hooked: DXGI's.
//!
//! **Drawing**, in `Present` before calling through: the back buffer (`GetBuffer`, released after the frame, so a
//! resize is never blocked by us), our own command allocators (a ring of 4, each with its fence value), one command
//! list, a root signature with 12 constants and one SRV (the depth copy), a pipeline per back buffer format,
//! premultiplied-alpha blending (the solid bars replace the picture under them, the glow adds), the game's depth
//! tested in the pixel shader; executed on the queue the swap chain presents with, then our fence signalled. The
//! camera and the board come from the renderer's per-frame copies when found ([`crate::proxy`]), else from the
//! simulation ([`crate::scene`]). The presenting queue comes from the swap chain itself (`GetDevice`), or a queue
//! pointer inside the (unwrapped) swap chain object (a known queue vtable), or the busiest DIRECT queue seen in
//! `ExecuteCommandLists`; Streamline queue wrappers are unwrapped to the real queue (its base interface).
//!
//! **Safety.** Nothing here ever blocks a frame for more than 2 ms; any failure skips the frame (and after many in a
//! row turns drawing off, `draw reset` re-arms it); panics are caught; on unload the slots go back (refused, and the
//! DLL stays loaded, when another hook sits on top of ours), threads inside our hooks are waited for, our GPU work is
//! waited for, everything is released.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};
use windows::core::{s, IUnknown, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{BOOL, FALSE, HANDLE, HWND, LPARAM, LRESULT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::bars::{self, BarSettings, Bars, HdrMode};
use crate::depth;
use crate::diag;
use crate::proxy;
use crate::rtti;
use crate::scene;
use crate::vhook::{self, VSlot};
use crate::win;

const VS: &[u8] = include_bytes!("../shaders/overlay_vs.dxbc");
const PS: &[u8] = include_bytes!("../shaders/overlay_ps.dxbc");
/// The board cam's shaders (shaders/pip.hlsl).
const PIP_VS: &[u8] = include_bytes!("../shaders/pip_vs.dxbc");
const PIP_PS: &[u8] = include_bytes!("../shaders/pip_ps.dxbc");
/// The board cam's root constants: mvp (16), light, colour, art, flags (4 each).
const PIP_CONSTANTS: usize = 32;
/// The two descriptors after the SRV banks: the board's live art for the cam, written into the one no frame in flight
/// reads when the texture changes (the game remakes it at an equip).
const PIP_ART_SRV: u32 = 2 * SRV_BANK;
const PIP_ART_SLOTS: u32 = 2;
/// The board cam is on (mirrors the settings, for `wanted`).
pub static PIP_ON: AtomicBool = AtomicBool::new(false);

type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT;
type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const DXGI_PRESENT_PARAMETERS) -> HRESULT;
type EclFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void);

const SLOT_PRESENT: usize = 8;
const SLOT_PRESENT1: usize = 22;
const SLOT_ECL: usize = 10;
const DXGI_PRESENT_TEST: u32 = 1;
/// Streamline's "give me the object you wrap" interface (`StreamlineRetrieveBaseInterface`).
const SL_BASE_INTERFACE: GUID = GUID::from_u128(0xadec44e2_61f0_45c3_ad9f_1b37379284ff);

static DXGI_PRESENT: VSlot = VSlot::new();
static DXGI_PRESENT1: VSlot = VSlot::new();
static SL_PRESENT: VSlot = VSlot::new();
static SL_PRESENT1: VSlot = VSlot::new();
static ECL: VSlot = VSlot::new();

/// Vtables learnt at install: DXGI's swap chain, the real D3D12 queue, Streamline's wrappers (0: none).
static DXGI_CHAIN_VT: AtomicUsize = AtomicUsize::new(0);
static REAL_QUEUE_VT: AtomicUsize = AtomicUsize::new(0);
static SL_CHAIN_VT: AtomicUsize = AtomicUsize::new(0);
static SL_QUEUE_VT: AtomicUsize = AtomicUsize::new(0);

/// Threads inside one of our hooks (the depth module's command list hooks count too).
pub(crate) static INFLIGHT: AtomicU32 = AtomicU32::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Unloading: hooks only pass through.
pub(crate) fn stopping() -> bool {
    STOPPING.load(Ordering::Acquire)
}
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// Drawing turned itself off after too many failures in a row.
static FAILED: AtomicBool = AtomicBool::new(false);
/// `draw test on`: the screen-space test rectangle.
pub static TEST: AtomicBool = AtomicBool::new(false);
/// Which queue strategy to use: 0 auto, 1 the swap chain's only, 2 the scan only, 3 the busiest only (for tests).
pub static QUEUE_MODE: AtomicU8 = AtomicU8::new(0);
/// When the Streamline wrapper's hook last drew (ms on [`win::now_ms`]).
static SL_DREW_MS: AtomicU64 = AtomicU64::new(0);
/// Landings seen (the worker counts fx.landed's rises; `bars land` too).
pub static LANDINGS: AtomicU32 = AtomicU32::new(0);

static PRESENTS: AtomicU64 = AtomicU64::new(0);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicU64 = AtomicU64::new(0);

/// The bars' settings and a sequence number the render thread watches.
static SETTINGS: Mutex<Option<BarSettings>> = Mutex::new(None);
static SETTINGS_SEQ: AtomicU64 = AtomicU64::new(1);
static BARS_ON: AtomicBool = AtomicBool::new(false);
static BARS_AUDIO: AtomicBool = AtomicBool::new(false);

/// What the render thread last saw, for the status.
static INFO: Mutex<Info> = Mutex::new(Info::new());

#[derive(Clone)]
struct Info {
    error: String,
    queue: &'static str,
    via: &'static str,
    format: i32,
    size: (u32, u32),
    mode: &'static str,
    scene: &'static str,
    vertices: usize,
    install: String,
    /// Which transforms placed the bars (the frame's, or the simulation's and why).
    timing: &'static str,
    /// The depth test's state.
    depth: String,
    /// The depth test's frames (fresh, late, off, blinks) and the copies' alignment.
    depth_frames: Value,
}

impl Info {
    const fn new() -> Info {
        Info {
            error: String::new(),
            queue: "",
            via: "",
            format: 0,
            size: (0, 0),
            mode: "",
            scene: "",
            vertices: 0,
            install: String::new(),
            timing: "",
            depth: String::new(),
            depth_frames: Value::Null,
        }
    }
}

fn info() -> std::sync::MutexGuard<'static, Info> {
    INFO.lock().unwrap_or_else(|e| e.into_inner())
}

fn note_error(e: impl Into<String>) {
    if let Ok(mut i) = INFO.try_lock() {
        i.error = e.into();
    }
}

// ---------------------------------------------------------------- settings

/// The bars' current settings.
pub fn settings() -> BarSettings {
    SETTINGS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// Replaces the bars' settings (already sanitized).
pub fn set_settings(s: BarSettings) {
    BARS_ON.store(s.enabled, Ordering::Release);
    PIP_ON.store(s.pip, Ordering::Release);
    BARS_AUDIO.store(s.enabled && s.source == bars::Source::Audio, Ordering::Release);
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
    SETTINGS_SEQ.fetch_add(1, Ordering::AcqRel);
}

/// Whether the bars want the audio capture (on, with the audio source).
pub fn audio_wanted() -> bool {
    BARS_AUDIO.load(Ordering::Acquire)
}

/// Whether anything is to be drawn.
pub fn wanted() -> bool {
    TEST.load(Ordering::Acquire) || BARS_ON.load(Ordering::Acquire) || PIP_ON.load(Ordering::Acquire) || crate::board::wanted() || crate::video::wanted()
}

/// Whether the hooks are worth installing: something to draw, or the staging ring or the allocation trace, which
/// live on the device, resource, copy and submission hooks and draw nothing.
pub fn hooks_wanted() -> bool {
    wanted() || crate::stagering::active() || crate::alloctrace::tracing()
}

pub fn installed() -> bool {
    INSTALLED.load(Ordering::Acquire)
}

/// Re-arms drawing after it turned itself off.
pub fn reset() {
    FAILED.store(false, Ordering::Release);
    FAILURES.store(0, Ordering::Relaxed);
    note_error("");
}

// ---------------------------------------------------------------- hooks

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Inside one of our hooks: counted (unload waits for zero) and nested (only the outermost present draws).
struct Enter {
    outermost: bool,
}

impl Enter {
    fn new() -> Enter {
        INFLIGHT.fetch_add(1, Ordering::AcqRel);
        let outermost = DEPTH.with(|d| {
            let v = d.get();
            d.set(v + 1);
            v == 0
        });
        Enter { outermost }
    }
}

impl Drop for Enter {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        INFLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Via {
    Dxgi,
    Streamline,
}

impl Via {
    fn name(self) -> &'static str {
        match self {
            Via::Dxgi => "dxgi",
            Via::Streamline => "streamline",
        }
    }
}

unsafe extern "system" fn dxgi_present(this: *mut c_void, sync: u32, flags: u32) -> HRESULT {
    let e = Enter::new();
    if e.outermost && flags & DXGI_PRESENT_TEST == 0 {
        on_present(this, Via::Dxgi);
    }
    let f: PresentFn = std::mem::transmute::<usize, PresentFn>(DXGI_PRESENT.original());
    f(this, sync, flags)
}

unsafe extern "system" fn dxgi_present1(this: *mut c_void, sync: u32, flags: u32, params: *const DXGI_PRESENT_PARAMETERS) -> HRESULT {
    let e = Enter::new();
    if e.outermost && flags & DXGI_PRESENT_TEST == 0 {
        on_present(this, Via::Dxgi);
    }
    let f: Present1Fn = std::mem::transmute::<usize, Present1Fn>(DXGI_PRESENT1.original());
    f(this, sync, flags, params)
}

unsafe extern "system" fn sl_present(this: *mut c_void, sync: u32, flags: u32) -> HRESULT {
    let e = Enter::new();
    if e.outermost && flags & DXGI_PRESENT_TEST == 0 {
        on_present(this, Via::Streamline);
    }
    let f: PresentFn = std::mem::transmute::<usize, PresentFn>(SL_PRESENT.original());
    f(this, sync, flags)
}

unsafe extern "system" fn sl_present1(this: *mut c_void, sync: u32, flags: u32, params: *const DXGI_PRESENT_PARAMETERS) -> HRESULT {
    let e = Enter::new();
    if e.outermost && flags & DXGI_PRESENT_TEST == 0 {
        on_present(this, Via::Streamline);
    }
    let f: Present1Fn = std::mem::transmute::<usize, Present1Fn>(SL_PRESENT1.original());
    f(this, sync, flags, params)
}

const QUEUES: usize = 16;
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_USIZE: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_U64: AtomicU64 = AtomicU64::new(0);
/// Queues seen submitting, and how often.
static Q_PTR: [AtomicUsize; QUEUES] = [ZERO_USIZE; QUEUES];
static Q_COUNT: [AtomicU64; QUEUES] = [ZERO_U64; QUEUES];

unsafe extern "system" fn ecl_hook(this: *mut c_void, count: u32, lists: *const *mut c_void) {
    INFLIGHT.fetch_add(1, Ordering::AcqRel);
    // count the submission (a table of 16 queues; more are not counted)
    let q = this as usize;
    for i in 0..QUEUES {
        let p = Q_PTR[i].load(Ordering::Relaxed);
        if p == q || (p == 0 && Q_PTR[i].compare_exchange(0, q, Ordering::AcqRel, Ordering::Relaxed).is_ok()) {
            Q_COUNT[i].fetch_add(1, Ordering::Relaxed);
            break;
        }
    }
    // each list's own vtable (D3D12Core gives every list one): its depth hooks, the first time it is submitted
    if count > 0 && !lists.is_null() && crate::depth::hooks_wanted() {
        let _ = std::panic::catch_unwind(|| {
            for &l in std::slice::from_raw_parts(lists, count as usize) {
                if !l.is_null() {
                    crate::depth::adopt(l as usize);
                }
            }
        });
    }
    // a depth copy among these lists into a slot the overlay's last draw may still be reading: the queue waits for
    // that draw's fence first (depth::before_execute)
    if count > 0 && !lists.is_null() {
        let _ = std::panic::catch_unwind(|| crate::depth::before_execute(this, std::slice::from_raw_parts(lists, count as usize)));
    }
    let f: EclFn = std::mem::transmute::<usize, EclFn>(ECL.original());
    f(this, count, lists);
    // the depth copies these lists carry are now submitted, in this order
    if count > 0 && !lists.is_null() {
        let submitted = std::slice::from_raw_parts(lists, count as usize);
        crate::depth::on_execute(this, submitted);
        // the staging ring: slots whose copies these lists carry wait for this queue's fence from here
        if crate::stagering::has_live() {
            let _ = std::panic::catch_unwind(|| crate::stagering::on_execute(this, submitted));
        }
        // the copy tracer: the copies these lists carry are on this queue now
        let _ = std::panic::catch_unwind(|| crate::copies::on_execute(this, submitted));
        // The Caveman: a list that carried a barrier on a watched texture, submitted on a COPY queue (board.rs: a
        // composite kept in COMMON and copied into on a copy queue would show so)
        if crate::board::wanted() {
            let _ = std::panic::catch_unwind(|| crate::board::on_execute(this, submitted));
        }
    }
    INFLIGHT.fetch_sub(1, Ordering::AcqRel);
}

fn on_present(chain: *mut c_void, via: Via) {
    PRESENTS.fetch_add(1, Ordering::Relaxed);
    depth::PRESENT_NO.fetch_add(1, Ordering::Relaxed);
    if STOPPING.load(Ordering::Acquire) || FAILED.load(Ordering::Acquire) || !wanted() || chain.is_null() {
        return;
    }
    let sl_drew = SL_DREW_MS.load(Ordering::Relaxed);
    if via == Via::Dxgi && sl_drew != 0 && win::now_ms().saturating_sub(sl_drew) < 250 {
        return; // the wrapper's hook draws these frames
    }
    let Ok(mut guard) = RENDERER.try_lock() else { return };
    let r = guard.get_or_insert_with(Renderer::new);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { r.frame(chain, via) }));
    match result {
        Ok(Ok(true)) => {
            FRAMES.fetch_add(1, Ordering::Relaxed);
            if FAILURES.swap(0, Ordering::Relaxed) > 0 {
                note_error("");
            }
            if via == Via::Streamline {
                SL_DREW_MS.store(win::now_ms().max(1), Ordering::Relaxed);
            }
        }
        Ok(Ok(false)) => {
            SKIPPED.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Err(e)) => fail(e),
        Err(_) => fail("panicked".into()),
    }
}

fn fail(e: String) {
    let n = FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
    note_error(e);
    if n >= 300 {
        FAILED.store(true, Ordering::Release);
    }
}

// ---------------------------------------------------------------- install

fn fn_addr(f: usize) -> usize {
    f
}

fn ours() -> Vec<(&'static VSlot, usize, &'static str)> {
    let mut v = vec![
        (&SL_PRESENT1, fn_addr(sl_present1 as *const () as usize), "streamline Present1"),
        (&SL_PRESENT, fn_addr(sl_present as *const () as usize), "streamline Present"),
        (&DXGI_PRESENT1, fn_addr(dxgi_present1 as *const () as usize), "dxgi Present1"),
        (&DXGI_PRESENT, fn_addr(dxgi_present as *const () as usize), "dxgi Present"),
        (&ECL, fn_addr(ecl_hook as *const () as usize), "ExecuteCommandLists"),
        (&crate::depth::CCL, fn_addr(crate::depth::create_command_list as *const () as usize), "CreateCommandList"),
        (&crate::depth::CCL1, fn_addr(crate::depth::create_command_list1 as *const () as usize), "CreateCommandList1"),
    ];
    v.extend(crate::depth::hooks());
    v.extend(crate::memguard::hooks());
    v.extend(crate::alloctrace::hooks());
    v
}

/// The command list vtable (D3D12Core's command list class, learnt from a dummy list), for the depth hooks.
static LIST_VT: AtomicUsize = AtomicUsize::new(0);

/// The first bytes of each hook function at install: changed at unload means someone detoured our code (then the DLL
/// must stay loaded).
static PROLOGUES: Mutex<Vec<(usize, [u8; 16])>> = Mutex::new(Vec::new());

unsafe extern "system" fn dummy_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, w, l)
}

/// A swap chain to read DXGI's vtable from: composition (no window), else on a hidden window.
unsafe fn dummy_chain(factory: &IDXGIFactory2, queue: &ID3D12CommandQueue) -> Result<(IDXGISwapChain1, Option<HWND>), String> {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let mut desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: 64,
        Height: 64,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: FALSE,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        Flags: 0,
    };
    if let Ok(c) = factory.CreateSwapChainForComposition(queue, &desc, None::<&IDXGIOutput>) {
        return Ok((c, None));
    }
    let class = windows::core::w!("CavemanFxDummy");
    let mut raw_module = crate::worker::MODULE.load(Ordering::Relaxed);
    if raw_module == 0 {
        raw_module = win::GetModuleHandleW(std::ptr::null()) as usize;
    }
    let module = windows::Win32::Foundation::HINSTANCE(raw_module as isize);
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(dummy_wndproc),
        hInstance: module,
        lpszClassName: class,
        ..Default::default()
    };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 64, 64, None, None, module, None);
    if hwnd.0 == 0 {
        return Err("cannot make a dummy swap chain (composition refused, no window)".into());
    }
    desc.AlphaMode = DXGI_ALPHA_MODE_UNSPECIFIED;
    match factory.CreateSwapChainForHwnd(queue, hwnd, &desc, None, None::<&IDXGIOutput>) {
        Ok(c) => Ok((c, Some(hwnd))),
        Err(e) => {
            let _ = DestroyWindow(hwnd);
            Err(format!("cannot make a dummy swap chain: {e}"))
        }
    }
}

/// Learns the vtables and installs the hooks (from the worker thread, never a render thread). Ok(summary).
pub fn install() -> Result<String, String> {
    if installed() {
        return Ok("already installed".into());
    }
    let result = unsafe { install_inner() };
    if let Err(e) = &result {
        unsafe { remove_slots() };
        info().install = format!("failed: {e}");
    }
    result
}

/// Test support: the Agility SDK (version, folder) the dummy device is made with, as a test host's own device is
/// (`cmfx_test_sdk`); None: the process's own D3D12, as in the game.
pub static TEST_SDK: Mutex<Option<(u32, String)>> = Mutex::new(None);

/// A D3D12 device of the D3D12 the process uses (or the test host's SDK).
unsafe fn make_device() -> Result<ID3D12Device, String> {
    let sdk = TEST_SDK.lock().ok().and_then(|g| g.clone());
    let mut device: Option<ID3D12Device> = None;
    match sdk {
        Some((version, path)) => {
            let mut config: Option<ID3D12SDKConfiguration1> = None;
            D3D12GetInterface(&CLSID_D3D12SDKConfiguration, &mut config).map_err(|e| format!("no SDK configuration: {e}"))?;
            let path = std::ffi::CString::new(path).map_err(|e| e.to_string())?;
            let factory: ID3D12DeviceFactory = config
                .ok_or("no SDK configuration")?
                .CreateDeviceFactory(version, windows::core::PCSTR(path.as_ptr().cast()))
                .map_err(|e| format!("no device factory: {e}"))?;
            factory.CreateDevice(None::<&IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).map_err(|e| format!("no D3D12 device: {e}"))?;
        }
        None => D3D12CreateDevice(None::<&IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).map_err(|e| format!("no D3D12 device: {e}"))?,
    }
    device.ok_or_else(|| "no D3D12 device".to_string())
}

unsafe fn install_inner() -> Result<String, String> {
    let device = make_device()?;
    let qdesc = D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() };
    let queue: ID3D12CommandQueue = device.CreateCommandQueue(&qdesc).map_err(|e| format!("no D3D12 queue: {e}"))?;
    let factory: IDXGIFactory2 = CreateDXGIFactory2(0).map_err(|e| format!("no DXGI factory: {e}"))?;
    let (chain, window) = dummy_chain(&factory, &queue)?;
    let mut chain_vt = vhook::vtable_of(chain.as_raw() as usize).ok_or("the dummy swap chain has no vtable")?;
    // Streamline may hand out its wrapper even for our own swap chain (seen in the game 2026-10-21 13:05: the dummy's
    // vtable was sl.interposer's); the swap chain under it is DXGI's, which is the one to hook (see "How we chain")
    let mut unwrap_note = String::new();
    {
        let mut base: *mut c_void = std::ptr::null_mut();
        if chain.query(&SL_BASE_INTERFACE, &mut base).is_ok() && !base.is_null() {
            let inner = IUnknown::from_raw(base);
            let inner_vt = vhook::vtable_of(inner.as_raw() as usize);
            let dxgi = win::module_range(win::GetModuleHandleW(win::wide("dxgi.dll").as_ptr()));
            match inner_vt {
                Some(vt) if vt != chain_vt && dxgi.is_some_and(|(lo, hi)| (lo..hi).contains(&vt)) => {
                    unwrap_note = format!("; our dummy swap chain came wrapped (vtable {chain_vt:#x}): hooked DXGI's under it");
                    if SL_CHAIN_VT.load(Ordering::Acquire) == 0 {
                        SL_CHAIN_VT.store(chain_vt, Ordering::Release);
                    }
                    chain_vt = vt;
                }
                Some(vt) if vt != chain_vt => {
                    unwrap_note = format!("; our dummy swap chain wraps an object outside dxgi.dll (vtable {vt:#x}): hooked the wrapper")
                }
                _ => {}
            }
        }
    }
    let queue_vt = vhook::vtable_of(queue.as_raw() as usize).ok_or("the dummy queue has no vtable")?;
    // the device's class (its vtable), for the CreateCommandList hooks (lists adopted at creation)
    let device_vt = vhook::vtable_of(device.as_raw() as usize).unwrap_or(0);
    let device4 = device.cast::<ID3D12Device4>().is_ok();
    let guard_interfaces = crate::memguard::interfaces(&device);
    // the memory watch's adapter, from here already (the renderer tells it again from the game's own device when it
    // draws; with nothing to draw it never would)
    crate::memwatch::set_adapter(device.GetAdapterLuid());
    // the resource class (its shared vtable, from a dummy buffer), for the allocation trace's release and map hooks
    let resource_vt = {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: 65536,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut r: Option<ID3D12Resource> = None;
        let _ = device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut r);
        r.and_then(|r| vhook::vtable_of(r.as_raw() as usize)).unwrap_or(0)
    };
    // the command list class, for the depth hooks
    let mut list7 = false;
    if let Ok(allocator) = device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT) {
        if let Ok(list) =
            device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>)
        {
            let _ = list.Close();
            list7 = list.cast::<ID3D12GraphicsCommandList7>().is_ok();
            LIST_VT.store(vhook::vtable_of(list.as_raw() as usize).unwrap_or(0), Ordering::Release);
        }
    }
    drop(chain);
    if let Some(h) = window {
        let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(h);
    }
    drop(factory);
    drop(queue);
    drop(device);
    DXGI_CHAIN_VT.store(chain_vt, Ordering::Release);
    REAL_QUEUE_VT.store(queue_vt, Ordering::Release);

    // Streamline's wrappers, when the game presents through them
    let mut sl_note = "no sl.interposer.dll".to_string();
    let sl = win::GetModuleHandleW(win::wide("sl.interposer.dll").as_ptr()) as usize;
    if sl != 0 {
        let range = win::module_range(sl as win::HANDLE);
        let in_module = |a: usize| range.is_some_and(|(lo, hi)| (lo..hi).contains(&a));
        match rtti::find_vtable(sl, sl, rtti::SL_SWAPCHAIN) {
            Some(vt) if win::read_ptr(vt + SLOT_PRESENT * 8).is_some_and(in_module) && win::read_ptr(vt + SLOT_PRESENT1 * 8).is_some_and(in_module) => {
                SL_CHAIN_VT.store(vt, Ordering::Release);
                sl_note = format!("streamline swap chain vtable sl.interposer+{:#x} (not hooked: draw streamline on)", vt - sl);
            }
            Some(vt) => sl_note = format!("streamline vtable at sl.interposer+{:#x} does not look right: not used", vt - sl),
            None => sl_note = "sl.interposer.dll loaded, its swap chain class not found".into(),
        }
        if let Some(vt) = rtti::find_vtable(sl, sl, rtti::SL_QUEUE) {
            SL_QUEUE_VT.store(vt, Ordering::Release);
        }
    }

    let mut prologues = Vec::new();
    for (_, f, _) in ours() {
        let mut b = [0u8; 16];
        win::read_into(f, &mut b);
        prologues.push((f, b));
    }
    *PROLOGUES.lock().unwrap_or_else(|e| e.into_inner()) = prologues;

    STOPPING.store(false, Ordering::Release);
    ECL.install(queue_vt + SLOT_ECL * 8, ecl_hook as *const () as usize).map_err(|e| format!("ExecuteCommandLists: {e}"))?;
    let mut create_note = String::new();
    if device_vt != 0 {
        match crate::depth::CCL.install(device_vt + crate::depth::slot_create_command_list() * 8, crate::depth::create_command_list as *const () as usize) {
            Ok(()) => create_note = "; CreateCommandList hooked".into(),
            Err(e) => create_note = format!("; CreateCommandList not hooked: {e}"),
        }
        if device4 {
            match crate::depth::CCL1
                .install(device_vt + crate::depth::slot_create_command_list1() * 8, crate::depth::create_command_list1 as *const () as usize)
            {
                Ok(()) => create_note.push_str(", CreateCommandList1 hooked"),
                Err(e) => create_note.push_str(&format!(", CreateCommandList1 not hooked: {e}")),
            }
        }
        // the memory guard: the game's allocations tried again when one comes back out of memory
        create_note.push_str(&crate::memguard::install(device_vt, guard_interfaces));
        if resource_vt != 0 {
            create_note.push_str(&crate::alloctrace::remember_vtable(resource_vt));
        }
    }
    DXGI_PRESENT.install(chain_vt + SLOT_PRESENT * 8, dxgi_present as *const () as usize).map_err(|e| format!("dxgi Present: {e}"))?;
    DXGI_PRESENT1.install(chain_vt + SLOT_PRESENT1 * 8, dxgi_present1 as *const () as usize).map_err(|e| format!("dxgi Present1: {e}"))?;
    // the depth hooks: optional (without them the bars draw unhidden, as before), and only while the depth test is on
    // (`draw depth off`: lists made meanwhile copy D3D12Core's own functions from the shared table, not ours)
    let list_vt = LIST_VT.load(Ordering::Acquire);
    let mut depth_note = "no depth hooks (no command list vtable)".to_string();
    if list_vt != 0 {
        crate::depth::set_template(list_vt);
    }
    if list_vt != 0 && !crate::depth::hooks_wanted() {
        depth_note = format!("command list vtable {list_vt:#x}: not hooked (the depth test, the staging ring and the trace are off)");
    } else if list_vt != 0 {
        let mut hooked = Vec::new();
        for (slot, (vs, f, what)) in crate::depth::hook_slots().into_iter().zip(crate::depth::hooks()) {
            if what == "Barrier" && !list7 {
                continue;
            }
            match vs.install(list_vt + slot * 8, f) {
                Ok(()) => hooked.push(what),
                Err(e) => hooked.push(if what == "Barrier" { "(Barrier not hooked)" } else { return Err(format!("command list {what}: {e}")) }),
            }
        }
        depth_note = format!("command list vtable {list_vt:#x}: {}", hooked.join(", "));
    }
    INSTALLED.store(true, Ordering::Release);
    if STREAMLINE.load(Ordering::Acquire) && SL_CHAIN_VT.load(Ordering::Acquire) != 0 {
        match hook_streamline(true) {
            Ok(note) => sl_note = format!("{sl_note}; {note}"),
            Err(e) => sl_note = format!("{sl_note}; {e}"),
        }
    }
    let module_of = |a: usize| -> String {
        for name in ["dxgi.dll", "d3d12core.dll", "d3d12.dll", "sl.interposer.dll"] {
            let h = win::GetModuleHandleW(win::wide(name).as_ptr());
            if let Some((lo, hi)) = win::module_range(h) {
                if (lo..hi).contains(&a) {
                    return format!("{name}+{:#x}", a - lo);
                }
            }
        }
        format!("{a:#x}")
    };
    let summary = format!(
        "hooked: swap chain vtable {} (Present was {}){unwrap_note}, queue vtable {}{create_note}; {sl_note}; {depth_note}",
        module_of(chain_vt),
        module_of(DXGI_PRESENT.original()),
        module_of(queue_vt)
    );
    info().install = summary.clone();
    Ok(summary)
}

/// Also hook Streamline's wrapper (`draw streamline on`): off by default. The wrapper's Present runs first, on the
/// game's thread, before ReSkate's detour draws its menus, so drawing there puts the bars under ReSkate's menus; but
/// while its slot holds our hook, a swap chain the game makes later fails ReSkate's own hook setup (ReSkate compares
/// the wrapper's slot with the function it detoured, and would find ours): its overlay would not bind to the new
/// swap chain. Hooking only DXGI's swap chain (the default) never gets in ReSkate's way: the bars are then drawn
/// after ReSkate's menus, on top of them.
pub static STREAMLINE: AtomicBool = AtomicBool::new(false);

/// Hooks (or unhooks) Streamline's wrapper Present slots, when its vtable was found. From the worker thread.
pub fn hook_streamline(on: bool) -> Result<String, String> {
    STREAMLINE.store(on, Ordering::Release);
    let vt = SL_CHAIN_VT.load(Ordering::Acquire);
    if !installed() {
        return Ok(format!("streamline hook {} (applies when the overlay installs)", if on { "wanted" } else { "off" }));
    }
    if vt == 0 {
        return Err("no Streamline swap chain wrapper here (sl.interposer.dll not loaded or its class not found)".into());
    }
    unsafe {
        if on {
            if !SL_PRESENT.installed() {
                SL_PRESENT.install(vt + SLOT_PRESENT * 8, sl_present as *const () as usize).map_err(|e| format!("streamline Present not hooked: {e}"))?;
            }
            if !SL_PRESENT1.installed() {
                SL_PRESENT1.install(vt + SLOT_PRESENT1 * 8, sl_present1 as *const () as usize).map_err(|e| format!("streamline Present1 not hooked: {e}"))?;
            }
            Ok("streamline Present hooked: drawn under ReSkate's menus".into())
        } else {
            SL_PRESENT1.uninstall(sl_present1 as *const () as usize).map_err(|e| format!("streamline Present1: {e}"))?;
            SL_PRESENT.uninstall(sl_present as *const () as usize).map_err(|e| format!("streamline Present: {e}"))?;
            SL_DREW_MS.store(0, Ordering::Release);
            Ok("streamline Present unhooked: drawn in DXGI's Present".into())
        }
    }
}

/// Test support: hooks `vt` as if it were Streamline's swap chain wrapper vtable (a test host's fake wrapper; the
/// export refuses in the game). The overlay must be installed and no wrapper hooked yet.
///
/// # Safety
/// `vt` must be a vtable whose slots 8 and 22 are `Present` and `Present1` of an `IDXGISwapChain1`.
pub unsafe fn hook_wrapper_for_test(vt: usize) -> Result<(), String> {
    if !installed() {
        return Err("the overlay is not installed".into());
    }
    if SL_PRESENT.installed() {
        return Err("a wrapper is hooked already".into());
    }
    SL_CHAIN_VT.store(vt, Ordering::Release);
    SL_PRESENT.install(vt + SLOT_PRESENT * 8, sl_present as *const () as usize)?;
    SL_PRESENT1.install(vt + SLOT_PRESENT1 * 8, sl_present1 as *const () as usize)?;
    Ok(())
}

/// Puts every slot back that still holds our hook. Returns the ones that could not be restored.
unsafe fn remove_slots() -> Vec<String> {
    let mut stuck = Vec::new();
    for (slot, f, what) in ours() {
        if let Err(e) = slot.uninstall(f) {
            stuck.push(format!("{what}: {e}"));
        }
    }
    stuck
}

/// Unhooks, waits for threads in our hooks and our GPU work, releases everything. Err when the DLL must stay
/// loaded (a slot could not be put back, or our code was detoured by someone else).
pub fn uninstall() -> Result<(), String> {
    STOPPING.store(true, Ordering::Release);
    let was = INSTALLED.swap(false, Ordering::AcqRel);
    let stuck = unsafe { remove_slots() };
    // the command lists' own tables (and any copy of our functions in one): back to what they held
    let restored = crate::depth::restore_tables(true);
    crate::worker::log(&format!("overlay: {restored}"));
    let start = Instant::now();
    while INFLIGHT.load(Ordering::Acquire) > 0 && start.elapsed().as_millis() < 3000 {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let busy = INFLIGHT.load(Ordering::Acquire);
    // lists the game recorded with our depth copies in them may not have run yet: give them a few frames before the
    // copies' textures go
    if crate::depth::injected_any() {
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    // the renderer: wait for our GPU work, then release
    let renderer = {
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(mut g) = RENDERER.try_lock() {
                break g.take();
            }
            if Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    if let Some(r) = renderer {
        r.wait_idle(1000);
        drop(r);
    }
    if let Ok(mut c) = crate::depth::COPIES.lock() {
        *c = None;
    }
    crate::depth::reset();
    let mut problems = stuck;
    if busy > 0 {
        problems.push(format!("{busy} thread(s) still inside the hooks"));
    }
    if was {
        for (f, b) in PROLOGUES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            let mut now = [0u8; 16];
            if !win::read_into(*f, &mut now) || now != *b {
                problems.push(format!("our hook at {f:#x} was detoured by another hook"));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// From DllMain on an unexpected FreeLibrary: only put the slots back (no waiting under the loader lock).
pub fn emergency_uninstall() {
    STOPPING.store(true, Ordering::Release);
    unsafe { remove_slots() };
    crate::depth::restore_tables(false);
}

// ---------------------------------------------------------------- the renderer

static RENDERER: Mutex<Option<Renderer>> = Mutex::new(None);

const RING: usize = 4;
/// Room for 512 bars a frame (80-byte vertices, 6 a bar), with some to spare.
const VB_BYTES: usize = 512 * 1024;
const RTVS: u32 = 8;
/// The pixel shader's root constants (shaders/overlay.hlsl's `Output`).
const CONSTANTS: usize = 16;
/// Frames of blink counters in flight.
const COUNT_SLOTS: usize = 64;

struct Slot {
    allocator: ID3D12CommandAllocator,
    vb: ID3D12Resource,
    map: *mut u8,
    fence: u64,
}

struct Gpu {
    device: ID3D12Device,
    root: ID3D12RootSignature,
    psos: Vec<(DXGI_FORMAT, ID3D12PipelineState)>,
    rtv_heap: ID3D12DescriptorHeap,
    rtv_size: u32,
    /// Shader-visible SRVs: two banks of a null SRV and the ring of depth copies (a bank per copy generation, so a
    /// new generation never rewrites descriptors a frame in flight reads).
    srv_heap: ID3D12DescriptorHeap,
    srv_size: u32,
    list: ID3D12GraphicsCommandList,
    slots: Vec<Slot>,
    next: usize,
    fence: ID3D12Fence,
    value: u64,
    event: win::Handle,
    /// The depth copies in use (the textures themselves are in `depth::COPIES`, where the command list hooks reach
    /// them), and the one set replaced, kept until every copy into it and every frame of ours reading it is done.
    copies: Option<CopyInfo>,
    generation: u64,
    retired: Option<Retired>,
    /// Sets kept for good (see `retire_old`).
    leaked: Vec<depth::CopySet>,
    /// When a set was last made (at most one every [`REMAKE_SECS`]).
    last_make: Option<Instant>,
    /// Depth values read back at points of the deck (the calibration).
    readback: ID3D12Resource,
    readback_map: *const u8,
    probes: [Probe; PROBE_FRAMES],
    probe_next: usize,
    /// The copies' textures (the same as `depth::COPIES`'), for the read-backs without taking that lock (a copy due
    /// meanwhile on a game thread would wait for it).
    copy_textures: Vec<ID3D12Resource>,
    /// The board cam's objects (made on its first frame).
    pip: Option<PipGpu>,
    /// The blink counters (two u32 a frame, [`COUNT_SLOTS`] frames), zeros to reset a frame's pair, the read-back
    /// (mapped) and, per frame slot, the fence it waits for and its present.
    counters: ID3D12Resource,
    zeros: ID3D12Resource,
    count_readback: ID3D12Resource,
    count_map: *const u8,
    count_pending: [(u64, u64); COUNT_SLOTS],
    count_next: usize,
}

#[derive(Clone, Copy)]
struct CopyInfo {
    size: (u32, u32),
    format: DXGI_FORMAT,
    generation: u64,
}

/// A replaced set of copies: freed once nothing can still use it.
struct Retired {
    set: depth::CopySet,
    /// The present it was replaced at, our fence's value then (our frames that may read it), and when.
    present: u64,
    value: u64,
    at: Instant,
}

/// At most one new set of copies this often (another size or format of the main depth).
const REMAKE_SECS: f32 = 5.0;

/// Lines for the DLL's log from the render thread, written by the worker's next tick (`worker::log` takes the
/// worker's lock and appends to a file: never on a frame).
static LOG_LATER: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn log_later(line: String) {
    if let Ok(mut l) = LOG_LATER.try_lock() {
        if l.len() < 64 {
            l.push(line);
        }
    }
}

/// From the worker's tick: the render thread's lines into the log.
pub fn flush_log() {
    let lines: Vec<String> = match LOG_LATER.try_lock() {
        Ok(mut l) if !l.is_empty() => std::mem::take(&mut *l),
        _ => return,
    };
    for line in lines {
        crate::worker::log(&line);
    }
}

/// One frame's depth read-back: the fence it waits for, and each point's view depth and way of finding its texel.
#[derive(Clone, Copy, Default)]
struct Probe {
    fence: u64,
    z: [f32; PROBE_POINTS],
    way: [u8; PROBE_POINTS],
    count: usize,
    pending: bool,
}

const SRV_BANK: u32 = 1 + depth::RING as u32;
const PROBE_FRAMES: usize = 8;
/// Three points of the deck through each of the two ways of finding the texel (the frame's viewport, the whole depth),
/// and the alignment test's two points under each of three frame offsets.
const PROBE_POINTS: usize = 12;
const PROBE_STRIDE: u64 = D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT as u64;
/// The scenes kept (by present number), for late copies' cameras.
const SCENES: usize = 32;
/// A copy at most this many presents older than the frame is still used (in its own camera), unless the camera cut
/// since: moved more than `CUT_M` or turned more than `CUT_DEG`.
const MAX_LATE: u64 = 30;
const CUT_M: f32 = 1.0;
const CUT_DEG: f32 = 30.0;

// ---------------------------------------------------------------- the board cam (pip.rs: the meshes; shaders/pip.hlsl)

type M4 = [f32; 16];

fn v3_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn v3_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn v3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn v3_norm(a: [f32; 3]) -> [f32; 3] {
    let l = v3_dot(a, a).sqrt();
    if l > 1e-9 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Row-vector matrices (`p * M`, as the shader multiplies): `a * b` applies `a` first.
fn m4_mul(a: &M4, b: &M4) -> M4 {
    let mut c = [0.0f32; 16];
    for r in 0..4 {
        for k in 0..4 {
            c[r * 4 + k] = (0..4).map(|j| a[r * 4 + j] * b[j * 4 + k]).sum();
        }
    }
    c
}
fn m4_transpose(a: &M4) -> M4 {
    let mut t = [0.0f32; 16];
    for r in 0..4 {
        for k in 0..4 {
            t[r * 4 + k] = a[k * 4 + r];
        }
    }
    t
}
/// A rotation whose rows are the frame's axes (a point in the frame times it lands in the world).
fn m4_rows(r: [f32; 3], u: [f32; 3], f: [f32; 3]) -> M4 {
    [r[0], r[1], r[2], 0.0, u[0], u[1], u[2], 0.0, f[0], f[1], f[2], 0.0, 0.0, 0.0, 0.0, 1.0]
}
/// A left-handed view (x right, y up, z into the picture) from `eye` towards `target`.
fn m4_look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> M4 {
    let f = v3_norm(v3_sub(target, eye));
    let s = v3_norm(v3_cross(up, f));
    let u = v3_cross(f, s);
    [s[0], u[0], f[0], 0.0, s[1], u[1], f[1], 0.0, s[2], u[2], f[2], 0.0, -v3_dot(s, eye), -v3_dot(u, eye), -v3_dot(f, eye), 1.0]
}
/// A left-handed perspective, depth 0 at `near` to 1 at `far`.
fn m4_perspective(fov_y_deg: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let h = 1.0 / (fov_y_deg.to_radians() * 0.5).tan();
    let w = h / aspect;
    let q = far / (far - near);
    [w, 0.0, 0.0, 0.0, 0.0, h, 0.0, 0.0, 0.0, 0.0, q, 1.0, 0.0, 0.0, -q * near, 0.0]
}
/// A direction times a rotation (no translation).
fn m4_dir(v: [f32; 3], m: &M4) -> [f32; 3] {
    [v[0] * m[0] + v[1] * m[4] + v[2] * m[8], v[0] * m[1] + v[1] * m[5] + v[2] * m[9], v[0] * m[2] + v[1] * m[6] + v[2] * m[10]]
}

/// The art's place on the deck: the 512 x 2048 art covers the deck's planform with rows 79 to 1970 from the nose to
/// the tail and its widest row 489 of 512 pixels wide (assets/deck_outline.json).
const ART_ROW_NOSE: f32 = 79.0;
const ART_ROW_TAIL: f32 = 1970.0;
const ART_WIDEST: f32 = 489.0 / 512.0;

/// What the board cam draws this frame.
#[derive(Clone, Copy, Debug)]
struct PipFrame {
    /// Its square on the back buffer (x, y, width, height in pixels), and the back buffer's size.
    viewport: [f32; 4],
    target: [u32; 2],
    mvp: M4,
    /// Towards the light in the mesh's frame, and the ambient share.
    light: [f32; 4],
    /// u per metre of x, v per metre of z, u at x = 0, v at z = 0.
    art: [f32; 4],
    art_on: bool,
    backdrop: f32,
    mode: u8,
    nits: f32,
}

/// The cam's transform and placement for a frame's scene (None without a board this frame).
fn pip_frame(s: &BarSettings, scene: &bars::Scene, width: u32, height: u32, mode: u8, bounds: [[f32; 3]; 2]) -> Option<PipFrame> {
    let b = scene.board?;
    // the board's rotation: its axes as rows (mesh frame: x right, y up, z to the nose)
    let rot = m4_rows(v3_norm(b.right), v3_norm(b.up), v3_norm(b.forward));
    // relative to the game camera's heading: the world turned so that heading is +z
    let yaw = if s.pip_view == "camera" {
        let f = [-scene.camera.back[0], 0.0, -scene.camera.back[2]];
        if v3_dot(f, f) > 1e-4 {
            let f = v3_norm(f);
            m4_transpose(&m4_rows([f[2], 0.0, -f[0]], [0.0, 1.0, 0.0], f))
        } else {
            m4_rows([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0])
        }
    } else {
        m4_rows([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0])
    };
    let model = m4_mul(&rot, &yaw);
    let (lo, hi) = (bounds[0], bounds[1]);
    let length = (hi[2] - lo[2]).max(0.3);
    let fov = s.pip_fov.clamp(5.0, 120.0);
    // far enough that the board fills about three fifths of the square seen lengthways, with little perspective
    let distance = (length * 1.7) / (2.0 * (fov.to_radians() * 0.5).tan()) / s.pip_zoom.max(0.2);
    let (el, az) = (s.pip_elevation.to_radians(), s.pip_azimuth.to_radians());
    let eye = [distance * az.sin() * el.cos(), distance * el.sin(), -distance * az.cos() * el.cos()];
    let target = [0.0, (lo[1] + hi[1]) * 0.5, 0.0];
    let view = m4_look_at(eye, target, [0.0, 1.0, 0.0]);
    let proj = m4_perspective(fov, 1.0, 0.05, 50.0);
    let mvp = m4_mul(&m4_mul(&model, &view), &proj);
    // the light: a headlight from the viewpoint, a little above it (the side the cam sees is always lit, the
    // underside included when it looks from below), carried into the mesh's frame
    let light_cam = v3_norm([eye[0], eye[1] + 0.35 * distance, eye[2]]);
    let light_model = v3_norm(m4_dir(light_cam, &m4_transpose(&model)));
    // the art's map (the deck's bounds), mirrored or flipped on demand
    let su = ART_WIDEST / (hi[0] - lo[0]).max(1e-3) * if s.pip_mirror { -1.0 } else { 1.0 };
    let u0 = 0.5 - (lo[0] + hi[0]) * 0.5 * su;
    let rows = (ART_ROW_TAIL - ART_ROW_NOSE) / 2048.0;
    let sv0 = rows / (hi[2] - lo[2]).max(1e-3);
    let (sv, v0) = if s.pip_flip { (-sv0, ART_ROW_NOSE / 2048.0 + lo[2] * sv0) } else { (sv0, ART_ROW_NOSE / 2048.0 + hi[2] * sv0) };
    // the square: a fraction of the screen's height, a margin from the chosen corner
    let side = (s.pip_size * height as f32).round().max(16.0);
    let margin = (s.pip_margin * height as f32).round();
    let x = if s.pip_corner.ends_with('l') { margin } else { width as f32 - margin - side };
    let y = if s.pip_corner.starts_with('t') { margin } else { height as f32 - margin - side };
    Some(PipFrame {
        viewport: [x, y, side, side],
        target: [width, height],
        mvp,
        light: [light_model[0], light_model[1], light_model[2], 0.35],
        art: [su, sv, u0, v0],
        art_on: s.pip_art,
        backdrop: s.pip_backdrop,
        mode,
        nits: 200.0 * s.brightness,
    })
}

/// The board cam's GPU objects.
struct PipGpu {
    root: ID3D12RootSignature,
    psos: Vec<(DXGI_FORMAT, ID3D12PipelineState)>,
    dsv_heap: ID3D12DescriptorHeap,
    /// The depth texture and its size: the back buffer's (a depth target must match the render target's size; only
    /// the cam's square of it is cleared and drawn).
    depth: Option<(ID3D12Resource, u32, u32)>,
    /// The meshes' vertex buffer (six floats a vertex; the backdrop's six quad vertices last) and index buffer.
    vb: Option<(ID3D12Resource, u32)>,
    ib: Option<(ID3D12Resource, u32)>,
    quad_first: u32,
    parts: Vec<crate::pip::PipPart>,
    generation: u64,
    /// The board's live art behind each of the two art descriptors (a reference of ours, so the texture outlives
    /// the game's release of it while a frame of ours still samples it) and its raw pointer. A slot is written again,
    /// its old reference dropped, only once the fence after the last frame that read it has passed.
    arts: [Option<(ID3D12Resource, usize)>; 2],
    /// Which of the two art descriptors is bound, and the overlay fence value after the last frame that read each.
    art_slot: u32,
    art_used: [u64; 2],
}

/// The format to view a texture as: a typeless one (the game makes many so) as its sRGB kind, the deck art being sRGB.
fn srv_format(f: DXGI_FORMAT) -> DXGI_FORMAT {
    let typed = [
        (DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB),
        (DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB),
        (DXGI_FORMAT_BC1_TYPELESS, DXGI_FORMAT_BC1_UNORM_SRGB),
        (DXGI_FORMAT_BC2_TYPELESS, DXGI_FORMAT_BC2_UNORM_SRGB),
        (DXGI_FORMAT_BC3_TYPELESS, DXGI_FORMAT_BC3_UNORM_SRGB),
        (DXGI_FORMAT_BC7_TYPELESS, DXGI_FORMAT_BC7_UNORM_SRGB),
    ];
    typed.iter().find(|(t, _)| *t == f).map_or(f, |(_, s)| *s)
}

// Used by one thread at a time (the overlay's); the COM objects are free-threaded.
unsafe impl Send for PipGpu {}

impl PipGpu {
    unsafe fn new(device: &ID3D12Device) -> Result<PipGpu, String> {
        let e = |what: &'static str| move |err: windows::core::Error| format!("board cam {what}: {err}");
        let range = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        };
        let params = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 {
                    Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: PIP_CONSTANTS as u32 },
                },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
        ];
        let sampler = D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            MipLODBias: 0.0,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
            BorderColor: D3D12_STATIC_BORDER_COLOR_OPAQUE_BLACK,
            MinLOD: 0.0,
            MaxLOD: f32::MAX,
            ShaderRegister: 0,
            RegisterSpace: 0,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        };
        let rs = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: params.len() as u32,
            pParameters: params.as_ptr(),
            NumStaticSamplers: 1,
            pStaticSamplers: &sampler,
            Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        };
        let mut blob: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        if let Err(err) = D3D12SerializeRootSignature(&rs, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut errors)) {
            let text = errors
                .map(|b| String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).into_owned())
                .unwrap_or_default();
            return Err(format!("board cam root signature: {err} {text}"));
        }
        let blob = blob.ok_or("no root signature blob")?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize());
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes).map_err(e("root signature"))?;
        let heap = D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV, NumDescriptors: 1, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE, NodeMask: 0 };
        let dsv_heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&heap).map_err(e("DSV heap"))?;
        Ok(PipGpu {
            root,
            psos: Vec::new(),
            dsv_heap,
            depth: None,
            vb: None,
            ib: None,
            quad_first: 0,
            parts: Vec::new(),
            generation: 0,
            arts: [None, None],
            art_slot: 0,
            art_used: [0; 2],
        })
    }

    unsafe fn pso(&mut self, device: &ID3D12Device, format: DXGI_FORMAT) -> Result<ID3D12PipelineState, String> {
        if let Some((_, p)) = self.psos.iter().find(|(f, _)| *f == format) {
            return Ok(p.clone());
        }
        let input = [
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("NORMAL"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 12,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];
        let target = D3D12_RENDER_TARGET_BLEND_DESC {
            BlendEnable: TRUE,
            LogicOpEnable: FALSE,
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ZERO,
            DestBlendAlpha: D3D12_BLEND_ONE,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            LogicOp: D3D12_LOGIC_OP_NOOP,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let stencil = D3D12_DEPTH_STENCILOP_DESC {
            StencilFailOp: D3D12_STENCIL_OP_KEEP,
            StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
            StencilPassOp: D3D12_STENCIL_OP_KEEP,
            StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        };
        let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
        formats[0] = format;
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&self.root),
            VS: D3D12_SHADER_BYTECODE { pShaderBytecode: PIP_VS.as_ptr().cast(), BytecodeLength: PIP_VS.len() },
            PS: D3D12_SHADER_BYTECODE { pShaderBytecode: PIP_PS.as_ptr().cast(), BytecodeLength: PIP_PS.len() },
            BlendState: D3D12_BLEND_DESC { AlphaToCoverageEnable: FALSE, IndependentBlendEnable: FALSE, RenderTarget: [target; 8] },
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                FrontCounterClockwise: FALSE,
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: TRUE,
                MultisampleEnable: FALSE,
                AntialiasedLineEnable: FALSE,
                ForcedSampleCount: 0,
                ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
            },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: TRUE,
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D12_COMPARISON_FUNC_LESS_EQUAL,
                StencilEnable: FALSE,
                StencilReadMask: 0xFF,
                StencilWriteMask: 0xFF,
                FrontFace: stencil,
                BackFace: stencil,
            },
            InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: input.as_ptr(), NumElements: input.len() as u32 },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            RTVFormats: formats,
            DSVFormat: DXGI_FORMAT_D32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let pso: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc).map_err(|e| format!("board cam pipeline for format {}: {e}", format.0))?;
        self.psos.push((format, pso.clone()));
        Ok(pso)
    }

    /// The depth texture, the back buffer's size, made again when that changes.
    unsafe fn ensure_depth(&mut self, device: &ID3D12Device, width: u32, height: u32) -> Result<(), String> {
        if self.depth.as_ref().is_some_and(|(_, w, h)| *w == width && *h == height) {
            return Ok(());
        }
        crate::memwatch::room_for((width as u64) * (height as u64) * 4).map_err(|why| format!("board cam depth: {why}"))?;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_D32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        };
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let clear = D3D12_CLEAR_VALUE {
            Format: DXGI_FORMAT_D32_FLOAT,
            Anonymous: D3D12_CLEAR_VALUE_0 { DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: 1.0, Stencil: 0 } },
        };
        let mut tex: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_DEPTH_WRITE, Some(&clear), &mut tex)
            .map_err(|e| format!("board cam depth texture: {e}"))?;
        let tex = tex.ok_or("no depth texture")?;
        let _ = tex.SetName(windows::core::w!("Caveman FX board cam depth"));
        let view = D3D12_DEPTH_STENCIL_VIEW_DESC {
            Format: DXGI_FORMAT_D32_FLOAT,
            ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
            Flags: D3D12_DSV_FLAG_NONE,
            Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 } },
        };
        device.CreateDepthStencilView(&tex, Some(&view), self.dsv_heap.GetCPUDescriptorHandleForHeapStart());
        self.depth = Some((tex, width, height));
        Ok(())
    }

    /// The meshes' buffers, made again when the meshes change (the old ones are dropped with the frame: a list in
    /// flight that reads them holds the GPU, not the objects, so this waits for the fence first).
    unsafe fn ensure_mesh(&mut self, device: &ID3D12Device, mesh: &crate::pip::PipMesh, generation: u64) -> Result<(), String> {
        if self.generation == generation && self.vb.is_some() {
            return Ok(());
        }
        let n = mesh.vertex_count() as u32;
        // the backdrop quad: clip-space corners, in the same vertex format
        let quad: [f32; 36] = [
            -1.0, -1.0, 0.0, 0.0, 1.0, 0.0, 1.0, -1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, -1.0, -1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 0.0,
            1.0, 0.0, -1.0, 1.0, 0.0, 0.0, 1.0, 0.0,
        ];
        let vbytes = (mesh.vertices.len() + quad.len()) * 4;
        let ibytes = mesh.indices.len() * 4;
        crate::memwatch::room_for((vbytes + ibytes) as u64).map_err(|why| format!("board cam buffers: {why}"))?;
        let vb =
            buffer(device, D3D12_HEAP_TYPE_UPLOAD, vbytes as u64, D3D12_RESOURCE_STATE_GENERIC_READ).map_err(|e| format!("board cam vertex buffer: {e}"))?;
        let ib =
            buffer(device, D3D12_HEAP_TYPE_UPLOAD, ibytes as u64, D3D12_RESOURCE_STATE_GENERIC_READ).map_err(|e| format!("board cam index buffer: {e}"))?;
        let none = D3D12_RANGE { Begin: 0, End: 0 };
        let mut map: *mut c_void = std::ptr::null_mut();
        vb.Map(0, Some(&none), Some(&mut map)).map_err(|e| format!("board cam vertex buffer map: {e}"))?;
        std::ptr::copy_nonoverlapping(mesh.vertices.as_ptr().cast::<u8>(), map as *mut u8, mesh.vertices.len() * 4);
        std::ptr::copy_nonoverlapping(quad.as_ptr().cast::<u8>(), (map as *mut u8).add(mesh.vertices.len() * 4), quad.len() * 4);
        vb.Unmap(0, None);
        let mut map: *mut c_void = std::ptr::null_mut();
        ib.Map(0, Some(&none), Some(&mut map)).map_err(|e| format!("board cam index buffer map: {e}"))?;
        std::ptr::copy_nonoverlapping(mesh.indices.as_ptr().cast::<u8>(), map as *mut u8, ibytes);
        ib.Unmap(0, None);
        self.vb = Some((vb, n + 6));
        self.ib = Some((ib, mesh.indices.len() as u32));
        self.quad_first = n;
        self.parts = mesh.parts.clone();
        self.generation = generation;
        Ok(())
    }
}

fn pip_constants(mvp: &M4, light: [f32; 4], colour: [f32; 4], art: [f32; 4], flags: [f32; 4]) -> [f32; PIP_CONSTANTS] {
    let mut c = [0.0f32; PIP_CONSTANTS];
    c[..16].copy_from_slice(mvp);
    c[16..20].copy_from_slice(&light);
    c[20..24].copy_from_slice(&colour);
    c[24..28].copy_from_slice(&art);
    c[28..32].copy_from_slice(&flags);
    c
}

/// What the depth test needs this frame.
struct DepthUse {
    /// The SRV to bind (a null one when off).
    srv: u32,
    on: bool,
    map: [f32; 4],
    lin: [f32; 4],
    /// The copy read (its ring slot), and depth texels to read back with their view depth, way (0 the frame's
    /// viewport, 1 the whole depth; 2 to 4 the alignment test's frame offsets -1, 0, +1) and the copy they are read
    /// from.
    copy: Option<u32>,
    probes: Vec<(u32, u32, f32, u8, u32)>,
    /// The camera and board the copy was rendered with, when it is not this frame's (a late copy): the bars' points are
    /// taken there for the test, carried by the board (the skater and the deck move with it).
    camera: Option<bars::Scene>,
    /// How the copy paired with the frame.
    pairing: depth::Pairing,
    /// The copy's picture and window, its pass end's and submission's times and queue, for the record.
    copy_info: Option<diag::CopyInfo>,
    /// The queue the copy was submitted on and the fence value signalled after it: a copy from another queue than
    /// the draw's is waited for on the GPU before the draw (`depth::wait_on`).
    wait_queue: usize,
    wait_fence: u64,
}

/// A proxy's matrix as the bytes in the game's memory (the `Snap`s compare bytes).
fn matrix_bytes(m: &[f32; 16]) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (k, v) in m.iter().enumerate() {
        out[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

struct QueueChoice {
    chain: usize,
    queue: ID3D12CommandQueue,
    at: f64,
}

struct Renderer {
    gpu: Option<Gpu>,
    queue: Option<QueueChoice>,
    counts: [u64; QUEUES],
    output: Option<(usize, DXGI_FORMAT, u8, f64)>,
    bars: Bars,
    settings: BarSettings,
    seq: u64,
    scene: scene::Reader,
    follow: proxy::Follow,
    /// The deck check of the depth copy, through the frame's viewport (0) and the whole depth (1), and which way is used.
    checks: [depth::Calibration; 2],
    whole: bool,
    /// The scene drawn at each of the last presents (present number, scene): a late depth copy's camera.
    scenes: [(u64, Option<bars::Scene>); SCENES],
    /// The depth test's frames and lapses, and how the copies drawn with paired with the frame (exact, one old,
    /// ahead, none).
    lapses: depth::Lapses,
    pairings: [u64; 4],
    /// The camera proxy's matrix drawn with at the last present (the proxies' steps between presents, for the record).
    last_drawn: Option<[u8; 64]>,
    torn_seen: u64,
    /// The depth test's last state (on, and why), to log its changes.
    depth_was: Option<(bool, String)>,
    t0: Instant,
    verts: Vec<bars::Vertex>,
    frame_no: u64,
}

// The renderer lives behind a mutex and is used by one thread at a time; its COM objects are free-threaded.
unsafe impl Send for Renderer {}

impl Renderer {
    fn new() -> Renderer {
        Renderer {
            gpu: None,
            queue: None,
            counts: [0; QUEUES],
            output: None,
            bars: Bars::new(),
            settings: BarSettings::default(),
            seq: 0,
            scene: scene::Reader::default(),
            follow: proxy::Follow::default(),
            checks: Default::default(),
            whole: false,
            depth_was: None,
            scenes: [(0, None); SCENES],
            lapses: depth::Lapses::default(),
            pairings: [0; 4],
            last_drawn: None,
            torn_seen: 0,
            t0: Instant::now(),
            verts: Vec::with_capacity(512),
            frame_no: 0,
        }
    }

    /// Waits (at most `ms`) for our last GPU work.
    fn wait_idle(&self, ms: u32) {
        if let Some(g) = &self.gpu {
            unsafe {
                if g.fence.GetCompletedValue() < g.value && g.fence.SetEventOnCompletion(g.value, HANDLE(g.event.0 as isize)).is_ok() {
                    win::WaitForSingleObject(g.event.0, ms);
                }
            }
        }
    }

    /// One frame. Ok(true) drawn, Ok(false) nothing to draw or skipped, Err(why).
    unsafe fn frame(&mut self, chain_raw: *mut c_void, via: Via) -> Result<bool, String> {
        self.frame_no += 1;
        let seq = SETTINGS_SEQ.load(Ordering::Acquire);
        if seq != self.seq {
            if let Ok(g) = SETTINGS.try_lock() {
                self.settings = g.clone().unwrap_or_default();
                self.seq = seq;
            }
        }
        let now = self.t0.elapsed().as_secs_f64();
        let chain = IDXGISwapChain::from_raw_borrowed(&chain_raw).ok_or("no swap chain")?;
        let chain3: IDXGISwapChain3 = match chain.cast() {
            Ok(c) => c,
            Err(_) => return Ok(false), // not a flip-model swap chain we can draw on
        };
        let index = chain3.GetCurrentBackBufferIndex();
        let buffer: ID3D12Resource = match chain3.GetBuffer(index) {
            Ok(b) => b,
            Err(_) => return Ok(false), // a D3D11 swap chain (a launcher, an overlay window): not ours to draw on
        };
        let desc = buffer.GetDesc();
        if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D || desc.SampleDesc.Count != 1 || (desc.Flags & D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET).0 == 0 {
            return Err("the back buffer is not a render target".into());
        }
        let (width, height) = (desc.Width as u32, desc.Height);
        if width == 0 || height == 0 {
            return Ok(false);
        }
        let mut device: Option<ID3D12Device> = None;
        buffer.GetDevice(&mut device).map_err(|e| format!("the back buffer has no device: {e}"))?;
        let device = device.ok_or("the back buffer has no device")?;
        if self.gpu.as_ref().is_none_or(|g| g.device.as_raw() != device.as_raw()) {
            self.wait_idle(500);
            self.gpu = None;
            self.queue = None;
            if let Ok(mut c) = depth::COPIES.lock() {
                *c = None;
            }
            depth::reset();
            self.checks = Default::default();
            self.gpu = Some(Gpu::new(&device)?);
        }

        // The Caveman's texture (board.rs): a waiting upload goes on the presenting queue, before anything is drawn
        let want_board = crate::board::pending();
        let want_video = crate::video::wanted();
        if want_board || want_video {
            let queue = self.queue_for(chain_raw as usize, &chain3, &device, now).ok();
            if want_board {
                crate::board::on_present(&device, queue.as_ref());
            }
            // video (video.rs): the producer's newest frame into the ridden art copy, after any waiting upload
            if want_video {
                crate::board::video_present(&device, queue.as_ref());
            }
        }

        // the game's depth: the main view's, and our copies of it in its shape (only for something drawn: not for the
        // board alone)
        let mut depth_note = String::new();
        if depth::ENABLED.load(Ordering::Acquire) && (TEST.load(Ordering::Acquire) || BARS_ON.load(Ordering::Acquire)) {
            if let Some((_, d, streak)) = depth::choose(width, height) {
                if let Err(e) = self.gpu.as_mut().unwrap().ensure_copies(d, streak >= depth::SWAP_STREAK) {
                    depth_note = format!("copies not made: {e}");
                }
            }
        }
        self.gpu.as_mut().unwrap().retire_old();
        for (d, z, way) in self.gpu.as_mut().unwrap().harvest() {
            self.checks[(way as usize).min(1)].add(d, z);
        }
        // the blink counters of frames the GPU finished, to the record (diag.rs finds the blinks among them)
        for (present, tested, hidden) in self.gpu.as_mut().unwrap().harvest_counts() {
            diag::counts(present, tested, hidden);
        }
        // which way of finding the texel the deck agrees with: the frame's viewport, unless only the whole depth does
        let (near, scene_ok, whole_ok) = (depth::linear().1, self.checks[0].agrees(depth::linear().1), self.checks[1].agrees(depth::linear().1));
        if scene_ok {
            self.whole = false;
        } else if whole_ok && self.checks[1].error(near) < self.checks[0].error(near) {
            self.whole = true;
        }

        // what to draw
        self.verts.clear();
        if TEST.load(Ordering::Acquire) {
            let c = bars::parse_colour(&self.settings.colour).unwrap_or([1.0, 0.82, 0.12]);
            bars::test_rect(c, now, &mut self.verts);
        }
        let landings = LANDINGS.load(Ordering::Acquire);
        let mut drawn_scene = None;
        if self.settings.enabled || self.settings.pip {
            if self.settings.enabled {
                self.bars.update(&self.settings, now, landings);
            }
            if let Some(sim) = self.scene.read(now) {
                let base = scene::EXE_BASE.load(Ordering::Acquire);
                let aspect = width as f32 / height as f32;
                let scene = if self.settings.timing == bars::Timing::Frame && base != 0 && !scene::test_scene_active() {
                    match proxy::frame_scene(base, sim, aspect, &mut self.follow) {
                        proxy::Framed::Frame(s) | proxy::Framed::Sim(s) => Some(s),
                        proxy::Framed::Hidden => None, // the board is not drawn: nothing to draw the bars on
                    }
                } else {
                    self.follow.why = if self.settings.timing == bars::Timing::Tick { "tick (setting)" } else { "tick (no game)" };
                    Some(sim)
                };
                if let Some(scene) = scene {
                    if self.settings.enabled {
                        let now_bars = self.bars.bars_now(&self.settings, now);
                        bars::build(&self.settings, &scene, aspect, &now_bars, &mut self.verts);
                    }
                    drawn_scene = Some(scene);
                }
            }
        }
        // this present's scene, for a later frame drawn with this frame's depth copy
        let present = depth::PRESENT_NO.load(Ordering::Acquire);
        self.scenes[(present % SCENES as u64) as usize] = (present, drawn_scene);
        let (du, depth_age) = self.depth_use(drawn_scene, width, height, &mut depth_note);
        // a late copy: the bars' points where that frame had them (on its board, in its camera)
        if let (Some(dc), Some(sc)) = (du.camera, drawn_scene) {
            let aspect = width as f32 / height as f32;
            for v in self.verts.iter_mut().filter(|v| v.extra[1] < 0.5) {
                v.dpos = bars::reproject_on_board(&sc, &dc, aspect, v.pos);
            }
        }
        if self.frame_no < 4 || self.frame_no.is_multiple_of(30) {
            if let Ok(mut i) = INFO.try_lock() {
                i.scene = if self.settings.enabled { self.scene.why } else { "bars off" };
                i.format = desc.Format.0;
                i.size = (width, height);
                i.via = via.name();
                i.vertices = self.verts.len();
                i.timing = self.follow.why;
                i.depth = depth_note.clone();
                i.depth_frames = json!({
                    "frames": self.lapses.status(),
                    // how the copies drawn with paired with the frame (depth.rs, "which frame a copy belongs to")
                    "paired": {"exact": self.pairings[0], "one_old": self.pairings[1], "ahead": self.pairings[2], "none": self.pairings[3]},
                    "blinks": diag::status(),
                });
            }
        }
        // the record of this present (diag.rs): the depth use, the copy, the transforms and how the proxies stepped
        let drawn_now = if self.follow.why == "frame" { self.follow.drawn().map(|(cm, _)| matrix_bytes(&cm)) } else { None };
        let step = |drawn: Option<[u8; 64]>, last: Option<[u8; 64]>, proxy: usize| -> diag::Step {
            let (Some(d), Some(l)) = (drawn, last) else { return diag::Step::Unknown };
            if d == l {
                return diag::Step::None;
            }
            let mut p = [0u8; 64];
            if win::read_into(proxy + proxy::PREVIOUS, &mut p) && p == l {
                diag::Step::One
            } else {
                diag::Step::Two
            }
        };
        let torn_now = proxy::TORN.load(Ordering::Relaxed);
        let entry = diag::Entry {
            present,
            t_us: diag::now_us(),
            drawn: drawn_scene.is_some(),
            depth: match (du.on, depth_note.is_empty()) {
                (true, _) => 1,
                (false, false) if drawn_scene.is_some() => 3,
                _ => 0,
            },
            age: depth_age as u32,
            copy: du.copy_info,
            draw_queue: depth::DRAW_QUEUE.load(Ordering::Relaxed),
            cam_pos: drawn_scene.map_or([0.0; 3], |s| s.camera.pos),
            cam_back: drawn_scene.map_or([0.0; 3], |s| s.camera.back),
            fov: drawn_scene.map_or(0.0, |s| s.camera.fov_y),
            board_pos: drawn_scene.and_then(|s| s.board).map_or([0.0; 3], |b| b.pos),
            screen: drawn_scene.and_then(|s| s.board.map(|b| (s, b))).map_or([0.0; 2], |(s, b)| {
                let c = bars::project(&s.camera, width as f32 / height as f32, b.pos);
                if c[3] > 0.05 {
                    [(c[0] / c[3] + 1.0) * 0.5 * width as f32, (1.0 - c[1] / c[3]) * 0.5 * height as f32]
                } else {
                    [-1.0, -1.0]
                }
            }),
            cam_step: step(drawn_now, self.last_drawn, proxy::CAMERA.load(Ordering::Relaxed)),
            board_step: diag::Step::Unknown,
            pairing: du.pairing as u8,
            torn: torn_now.saturating_sub(self.torn_seen) as u32,
            tested: None,
            hidden: None,
            blink: 0,
        };
        self.torn_seen = torn_now;
        self.last_drawn = drawn_now;
        diag::record(entry);
        diag::poll_key(present);
        if self.verts.is_empty() {
            return Ok(false);
        }
        let queue = self.queue_for(chain_raw as usize, &chain3, &device, now)?;
        depth::DRAW_QUEUE.store(queue.as_raw() as usize, Ordering::Release);
        let mode = self.output_mode(chain_raw as usize, &chain3, desc.Format, now);
        let gpu = self.gpu.as_mut().unwrap();
        let pip = if self.settings.pip {
            crate::pip::current().and_then(|(m, _)| drawn_scene.and_then(|s| pip_frame(&self.settings, &s, width, height, mode, m.bounds)))
        } else {
            None
        };
        gpu.draw(&queue, &buffer, index, desc.Format, width, height, mode, &self.settings, &self.verts, &du, pip.as_ref())
    }

    /// The depth test's inputs this frame, and why it is off when it is (in `note`).
    fn depth_use(&mut self, scene: Option<bars::Scene>, width: u32, height: u32, note: &mut String) -> (DepthUse, u64) {
        let (du, age) = self.depth_use_inner(scene, width, height, note);
        // log the test's changes (on, off and why), not every frame's numbers
        let reason = if du.on { "on".to_string() } else { note.split(':').next().unwrap_or("").split('(').next().unwrap_or("").trim().to_string() };
        // and count its frames and short lapses, at presents the bars are drawn at
        if scene.is_some() && depth::ENABLED.load(Ordering::Relaxed) {
            let u = match (du.on, age) {
                (true, 0) => depth::Use::Fresh,
                (true, a) => depth::Use::Late(a),
                (false, _) => depth::Use::Off,
            };
            self.lapses.note(depth::PRESENT_NO.load(Ordering::Relaxed), u, note);
        }
        if self.depth_was.as_ref().is_none_or(|(on, r)| *on != du.on || (!du.on && *r != reason)) {
            if self.depth_was.is_some() {
                depth::note_change(format!("{} ({note})", if du.on { "on" } else { "off" }));
            }
            self.depth_was = Some((du.on, reason));
        }
        (du, age)
    }

    /// The scene drawn at a present, if it is still kept.
    fn scene_at(&self, present: u64) -> Option<bars::Scene> {
        let (at, s) = self.scenes[(present % SCENES as u64) as usize];
        if at == present {
            s
        } else {
            None
        }
    }

    /// The test's inputs, and how many presents older than this frame the copy used is (0: the frame's own).
    fn depth_use_inner(&mut self, scene: Option<bars::Scene>, width: u32, height: u32, note: &mut String) -> (DepthUse, u64) {
        let gpu = self.gpu.as_ref().unwrap();
        let mut off = DepthUse {
            srv: gpu.null_srv(),
            on: false,
            map: [0.0; 4],
            lin: [0.0; 4],
            copy: None,
            probes: Vec::new(),
            camera: None,
            pairing: depth::Pairing::None,
            copy_info: None,
            wait_queue: 0,
            wait_fence: 0,
        };
        if !depth::ENABLED.load(Ordering::Acquire) {
            *note = "off (draw depth off)".into();
            return (off, 0);
        }
        if !note.is_empty() {
            return (off, 0);
        }
        let Some(info) = gpu.copies else {
            *note = if depth::hooked() { "no main depth found yet".into() } else { "not hooked".into() };
            return (off, 0);
        };
        // the copy for this frame: the one recorded with the very transforms the bars are drawn with (`Pairing::Exact`,
        // the renderer's proxies as they were at the pass end), else one recorded one frame before (`OneOld`: used in
        // that frame's camera and board), else, with nothing paired, the newest whose picture (`depth::picture_of`) is
        // this one or an earlier one; an earlier one is used in its own camera (the last good copy, rather than none:
        // a skipped or late copy, the game making its main depth anew), unless the camera cut since
        let now = depth::PRESENT_NO.load(Ordering::Acquire);
        let history: Vec<depth::Latest> = depth::history(info.generation).into_iter().filter(|c| c.usable).collect();
        let drawn = if self.follow.why == "frame" { self.follow.drawn().map(|(cm, _)| matrix_bytes(&cm)) } else { None };
        let mut prev = [0u8; 64];
        let prev_ok = drawn.is_some() && win::read_into(proxy::CAMERA.load(Ordering::Acquire) + proxy::PREVIOUS, &mut prev);
        let pairing_of = |c: &depth::Latest| match (&drawn, prev_ok) {
            (Some(d), true) => depth::Pairing::of(&c.cam, d, &prev),
            _ => depth::Pairing::None,
        };
        let exact = history.iter().find(|c| pairing_of(c) == depth::Pairing::Exact);
        let one_old = history.iter().find(|c| pairing_of(c) == depth::Pairing::OneOld);
        let (latest, image, pairing) = if let Some(c) = exact {
            (*c, now, depth::Pairing::Exact)
        } else if let Some(c) = one_old {
            (*c, now.saturating_sub(1), depth::Pairing::OneOld)
        } else {
            match history.iter().find(|c| c.picture <= now).copied() {
                Some(c) => (c, c.picture, pairing_of(&c)),
                None => {
                    *note = "no copy this frame (the game's depth pass was not seen)".into();
                    return (off, 0);
                }
            }
        };
        self.pairings[pairing as usize] += 1;
        off.pairing = pairing;
        let age = now.saturating_sub(image);
        if age > MAX_LATE {
            *note = format!("no copy this frame (the newest is {age} presents old)");
            return (off, age);
        }
        let depth_scene = if age == 0 { scene } else { self.scene_at(image) };
        if age > 0 {
            match (scene, depth_scene) {
                (Some(cur), Some(then)) => {
                    let turned = bars::dot(cur.camera.back, then.camera.back).clamp(-1.0, 1.0).acos().to_degrees();
                    if bars::length(bars::sub(cur.camera.pos, then.camera.pos)) > CUT_M || turned > CUT_DEG {
                        *note = format!("no copy this frame (the camera cut since the newest, {age} presents old)");
                        return (off, age);
                    }
                }
                (Some(_), None) if age > 1 => {
                    *note = format!("no copy this frame (the newest is {age} presents old, its camera not kept)");
                    return (off, age);
                }
                _ => {}
            }
        }
        // the depth's size when this copy was taken (it fills the copies from their top left corner)
        let (dw, dh) = (latest.src.0 as f32, latest.src.1 as f32);
        let (ow, oh) = (width as f32, height as f32);
        let aspect = ow / oh;
        // the two ways to a texel: the viewport the frame rendered into (dynamic resolution), or the whole depth
        let maps = |viewport: [f32; 4], src: (u32, u32)| {
            let (dw, dh) = (src.0 as f32, src.1 as f32);
            let [vx, vy, vw, vh] = viewport;
            let ok = vw >= 16.0 && vh >= 16.0 && vx >= 0.0 && vy >= 0.0 && vx + vw <= dw + 0.5 && vy + vh <= dh + 0.5;
            (ok, if ok { [vw / ow, vh / oh, vx, vy] } else { [dw / ow, dh / oh, 0.0, 0.0] })
        };
        let [vx, vy, vw, vh] = latest.viewport;
        let (viewport_ok, scene_map) = maps(latest.viewport, latest.src);
        let whole_map = [dw / ow, dh / oh, 0.0, 0.0];
        let map = if self.whole { whole_map } else { scene_map };
        let texel = |cam: &bars::Camera, p: bars::V3, m: [f32; 4]| -> Option<(u32, u32, f32, (f32, f32))> {
            let c = bars::project(cam, aspect, p);
            if c[3] <= 0.3 {
                return None;
            }
            let (px, py) = ((c[0] / c[3] + 1.0) * 0.5 * ow, (1.0 - c[1] / c[3]) * 0.5 * oh);
            let (tx, ty) = (px * m[0] + m[2], py * m[1] + m[3]);
            (tx >= 0.0 && ty >= 0.0 && tx < dw && ty < dh).then_some((tx as u32, ty as u32, c[3], (px, py)))
        };
        let long_of = |b: &bars::Board| match self.settings.axis {
            bars::Axis::Z => b.forward,
            bars::Axis::X => b.right,
        };
        // the deck check's points: the deck's centre and 12 cm either way along it, through both ways, as the copy's
        // frame placed them
        let mut probes = Vec::new();
        if let Some(bars::Scene { camera, board: Some(b) }) = depth_scene {
            for (way, m) in [(0u8, scene_map), (1u8, whole_map)] {
                for off_m in [0.0f32, 0.12, -0.12] {
                    let p = bars::add(bars::add(b.pos, bars::mul(long_of(&b), off_m)), bars::mul(b.up, self.settings.height));
                    if let Some((tx, ty, z, _)) = texel(&camera, p, m) {
                        probes.push((tx, ty, z, way, latest.slot));
                    }
                }
            }
        }
        let (x, y) = depth::linear();
        let check = |c: &depth::Calibration| {
            if c.samples() >= depth::Calibration::NEED {
                format!("{:.4} m, spread {:.0} %", c.median, c.spread * 100.0)
            } else {
                format!("{} samples", c.samples())
            }
        };
        *note = format!(
            "on: reversed depth, near {y:.4} m ({}), {}x{} copy, {} (the deck says: viewport {}, whole {}){}",
            if f32::from_bits(depth::NEAR_OVERRIDE.load(Ordering::Relaxed)) > 0.0 { "set by hand" } else { "the game's projection" },
            latest.src.0,
            latest.src.1,
            if self.whole {
                "the whole depth".to_string()
            } else if viewport_ok {
                format!("viewport {vw}x{vh} at ({vx}, {vy})")
            } else {
                "the whole depth (no viewport)".to_string()
            },
            check(&self.checks[0]),
            check(&self.checks[1]),
            if age > 0 { format!("; the copy {age} present(s) late, in its own camera") } else { String::new() },
        );
        if pairing != depth::Pairing::Exact {
            note.push_str(&format!("; paired: {}", pairing.name()));
        }
        // the shader takes the point's normalised device coordinates (in the copy's camera) to the texel
        let ndc_map = [0.5 * ow * map[0], -0.5 * oh * map[1], 0.5 * ow * map[0] + map[2], 0.5 * oh * map[1] + map[3]];
        off.on = true;
        off.srv = gpu.srv_index(info.generation, Some(latest.slot));
        off.map = ndc_map;
        off.lin = [x, y, dw - 1.0, dh - 1.0];
        off.copy = Some(latest.slot);
        off.wait_queue = latest.queue;
        off.wait_fence = latest.fence;
        off.probes = probes;
        off.camera = if age > 0 { depth_scene } else { None };
        off.copy_info = Some(diag::CopyInfo {
            picture: latest.picture,
            slot: latest.slot,
            recorded: latest.at,
            submitted: latest.submitted,
            end_us: latest.end_us,
            submit_us: latest.submit_us,
            queue: latest.queue,
            waited: 0,
        });
        (off, age)
    }

    /// The queue the swap chain presents with (cached per swap chain, re-checked every 2 s).
    unsafe fn queue_for(&mut self, chain_raw: usize, chain3: &IDXGISwapChain3, device: &ID3D12Device, now: f64) -> Result<ID3D12CommandQueue, String> {
        if let Some(q) = &self.queue {
            if q.chain == chain_raw && now - q.at < 2.0 {
                return Ok(q.queue.clone());
            }
        }
        let mode = QUEUE_MODE.load(Ordering::Relaxed);
        let mut found: Option<(ID3D12CommandQueue, &'static str)> = None;
        if mode == 0 || mode == 1 {
            if let Ok(q) = chain3.GetDevice::<ID3D12CommandQueue>() {
                if let Some(q) = usable_queue(q, device) {
                    found = Some((q, "the swap chain's"));
                }
            }
        }
        if found.is_none() && (mode == 0 || mode == 2) {
            found = scan_for_queue(chain_raw, device).map(|q| (q, "found in the swap chain"));
        }
        if found.is_none() && (mode == 0 || mode == 3) {
            found = self.busiest_queue(device).map(|q| (q, "the busiest direct queue"));
        }
        let (queue, how) = found.ok_or("no queue to draw with yet")?;
        if let Ok(mut i) = INFO.try_lock() {
            i.queue = how;
        }
        self.queue = Some(QueueChoice { chain: chain_raw, queue: queue.clone(), at: now });
        Ok(queue)
    }

    /// The DIRECT queue that submitted most since the last look.
    unsafe fn busiest_queue(&mut self, device: &ID3D12Device) -> Option<ID3D12CommandQueue> {
        let mut best: Option<(u64, usize)> = None;
        for i in 0..QUEUES {
            let p = Q_PTR[i].load(Ordering::Acquire);
            let c = Q_COUNT[i].load(Ordering::Relaxed);
            let delta = c.saturating_sub(self.counts[i]);
            self.counts[i] = c;
            if p != 0 && delta > 0 && best.is_none_or(|(d, _)| delta > d) {
                best = Some((delta, p));
            }
        }
        let (_, p) = best?;
        if vhook::vtable_of(p) != Some(REAL_QUEUE_VT.load(Ordering::Acquire)) {
            return None;
        }
        let raw = p as *mut c_void;
        let q = ID3D12CommandQueue::from_raw_borrowed(&raw)?.clone();
        usable_queue(q, device)
    }

    /// How to write colours: 0 SDR, 1 scRGB, 2 HDR10 (re-checked every 2 s).
    unsafe fn output_mode(&mut self, chain_raw: usize, chain3: &IDXGISwapChain3, format: DXGI_FORMAT, now: f64) -> u8 {
        let forced = match self.settings.hdr {
            HdrMode::Auto => None,
            HdrMode::Sdr => Some(0),
            HdrMode::Scrgb => Some(1),
            HdrMode::Hdr10 => Some(2),
        };
        let mode = match forced {
            Some(m) => m,
            None => match self.output {
                Some((c, f, m, at)) if c == chain_raw && f == format && now - at < 2.0 => m,
                _ => {
                    let m = if format == DXGI_FORMAT_R16G16B16A16_FLOAT {
                        1
                    } else if format == DXGI_FORMAT_R10G10B10A2_UNORM {
                        let hdr = chain3.GetContainingOutput().ok().and_then(|o| o.cast::<IDXGIOutput6>().ok()).is_some_and(|o6| {
                            let mut d = DXGI_OUTPUT_DESC1::default();
                            o6.GetDesc1(&mut d).is_ok() && d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020
                        });
                        if hdr {
                            2
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    self.output = Some((chain_raw, format, m, now));
                    m
                }
            },
        };
        if self.frame_no < 4 || self.frame_no.is_multiple_of(30) {
            if let Ok(mut i) = INFO.try_lock() {
                i.mode = ["sdr", "scrgb", "hdr10"][mode as usize];
            }
        }
        mode
    }
}

/// `q` as a real, DIRECT queue of `device` (a Streamline wrapper is unwrapped); None if it is not one.
unsafe fn usable_queue(q: ID3D12CommandQueue, device: &ID3D12Device) -> Option<ID3D12CommandQueue> {
    let real_vt = REAL_QUEUE_VT.load(Ordering::Acquire);
    let vt = vhook::vtable_of(q.as_raw() as usize)?;
    let q = if vt == real_vt {
        q
    } else {
        // a wrapper: ask it for the object it wraps
        let mut base: *mut c_void = std::ptr::null_mut();
        if q.query(&SL_BASE_INTERFACE, &mut base).is_err() || base.is_null() {
            return None;
        }
        let unknown = IUnknown::from_raw(base);
        let inner: ID3D12CommandQueue = unknown.cast().ok()?;
        if vhook::vtable_of(inner.as_raw() as usize)? != real_vt {
            return None;
        }
        inner
    };
    if q.GetDesc().Type != D3D12_COMMAND_LIST_TYPE_DIRECT {
        return None;
    }
    let mut d: Option<ID3D12Device> = None;
    q.GetDevice(&mut d).ok()?;
    (d?.as_raw() == device.as_raw()).then_some(q)
}

/// How far into a DXGI swap chain object the scan looks for its queue.
const SCAN_BYTES: usize = 0x200;
/// Where the scan last found the queue (offset in the DXGI swap chain object), for the status.
static SCAN_AT: AtomicUsize = AtomicUsize::new(0);

/// The queue a swap chain presents with, from the swap chain object itself. A DXGI swap chain keeps it among its first
/// fields (seen at +0x140 in the game, 2026-10-21, read only: a D3D12Core `VCGraphicsCommandQueue`, the queue the
/// game's renderer and Streamline's swap chain data also hold), and holds a reference to it, so the pointer is live
/// while the swap chain presents. Streamline's wrapper gives the DXGI swap chain it wraps through its base interface
/// (a real COM call, reference counted; never a guess at its fields).
unsafe fn scan_for_queue(chain_raw: usize, device: &ID3D12Device) -> Option<ID3D12CommandQueue> {
    let queue_in = |obj: usize| -> Option<ID3D12CommandQueue> {
        let real_vt = REAL_QUEUE_VT.load(Ordering::Acquire);
        let mut raw = vec![0u8; SCAN_BYTES];
        if !win::read_into(obj, &mut raw) {
            return None;
        }
        for (k, c) in raw.chunks_exact(8).enumerate().skip(1) {
            let p = usize::from_le_bytes(c.try_into().unwrap());
            if !(win::LOWEST..=win::HIGHEST).contains(&p) || p % 8 != 0 || p == obj || vhook::vtable_of(p) != Some(real_vt) {
                continue;
            }
            let rawp = p as *mut c_void;
            if let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&rawp) {
                if let Some(q) = usable_queue(q.clone(), device) {
                    SCAN_AT.store(k * 8, Ordering::Relaxed);
                    return Some(q);
                }
            }
        }
        None
    };
    // a wrapper (Streamline's) gives the swap chain it wraps: the queue is there. Asked of every swap chain (DXGI's
    // own answers E_NOINTERFACE), whatever vtable the hook learnt: in the game on 2026-10-21 13:05 our dummy swap
    // chain was itself Streamline's wrapper, so the "DXGI" vtable was the wrapper's and the queue was never found
    let chain_ptr = chain_raw as *mut c_void;
    if let Some(wrapper) = IUnknown::from_raw_borrowed(&chain_ptr) {
        let mut base: *mut c_void = std::ptr::null_mut();
        if wrapper.query(&SL_BASE_INTERFACE, &mut base).is_ok() && !base.is_null() {
            let base = IUnknown::from_raw(base);
            if let Ok(under) = base.cast::<IDXGISwapChain>() {
                let raw = under.as_raw() as usize;
                if raw != chain_raw {
                    if let Some(q) = queue_in(raw) {
                        return Some(q);
                    }
                }
            }
        }
    }
    queue_in(chain_raw)
}

impl Gpu {
    unsafe fn new(device: &ID3D12Device) -> Result<Gpu, String> {
        let e = |what: &str| {
            let what = what.to_string();
            move |err: windows::core::Error| format!("{what}: {err}")
        };
        // root signature: 12 constants and one SRV (the depth copy, t0) for the pixel shader
        let range = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: 0,
        };
        let params = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: CONSTANTS as u32 } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
            // the blink counters (u1, a raw buffer)
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
                Anonymous: D3D12_ROOT_PARAMETER_0 { Descriptor: D3D12_ROOT_DESCRIPTOR { ShaderRegister: 1, RegisterSpace: 0 } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
        ];
        let rs = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: params.len() as u32,
            pParameters: params.as_ptr(),
            NumStaticSamplers: 0,
            pStaticSamplers: std::ptr::null(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        };
        let mut blob: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        if let Err(err) = D3D12SerializeRootSignature(&rs, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, Some(&mut errors)) {
            let text = errors
                .map(|b| String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).into_owned())
                .unwrap_or_default();
            return Err(format!("root signature: {err} {text}"));
        }
        let blob = blob.ok_or("no root signature blob")?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize());
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes).map_err(e("root signature"))?;
        let heap =
            D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: RTVS, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE, NodeMask: 0 };
        let rtv_heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&heap).map_err(e("RTV heap"))?;
        let rtv_size = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
        let heap = D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            NumDescriptors: 2 * SRV_BANK + PIP_ART_SLOTS,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        };
        let srv_heap: ID3D12DescriptorHeap = device.CreateDescriptorHeap(&heap).map_err(e("SRV heap"))?;
        let srv_size = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        // every descriptor starts as a null SRV (reads 0: no occlusion)
        let null = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 },
            },
        };
        for k in 0..2 * SRV_BANK + PIP_ART_SLOTS {
            let mut h = srv_heap.GetCPUDescriptorHandleForHeapStart();
            h.ptr += (k * srv_size) as usize;
            device.CreateShaderResourceView(None, Some(&null), h);
        }
        let mut slots = Vec::new();
        for _ in 0..RING {
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).map_err(e("command allocator"))?;
            let vb = buffer(device, D3D12_HEAP_TYPE_UPLOAD, VB_BYTES as u64, D3D12_RESOURCE_STATE_GENERIC_READ).map_err(e("vertex buffer"))?;
            let mut map: *mut c_void = std::ptr::null_mut();
            let none = D3D12_RANGE { Begin: 0, End: 0 };
            vb.Map(0, Some(&none), Some(&mut map)).map_err(e("vertex buffer map"))?;
            if map.is_null() {
                return Err("the vertex buffer did not map".into());
            }
            slots.push(Slot { allocator, vb, map: map as *mut u8, fence: 0 });
        }
        let readback = buffer(device, D3D12_HEAP_TYPE_READBACK, PROBE_STRIDE * (PROBE_FRAMES * PROBE_POINTS) as u64, D3D12_RESOURCE_STATE_COPY_DEST)
            .map_err(e("read-back buffer"))?;
        let mut map: *mut c_void = std::ptr::null_mut();
        readback.Map(0, None, Some(&mut map)).map_err(e("read-back map"))?;
        if map.is_null() {
            return Err("the read-back buffer did not map".into());
        }
        // the blink counters: a default buffer the pixel shader adds to, 8 zero bytes to reset a frame's pair, and the
        // read-back (kept mapped)
        let counter_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: (COUNT_SLOTS * 8) as u64,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        };
        let mut counters: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                D3D12_HEAP_FLAG_NONE,
                &counter_desc,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut counters,
            )
            .map_err(e("counters"))?;
        let counters = counters.ok_or("no counters")?;
        let zeros = buffer(device, D3D12_HEAP_TYPE_UPLOAD, 256, D3D12_RESOURCE_STATE_GENERIC_READ).map_err(e("zeros"))?;
        let mut zmap: *mut c_void = std::ptr::null_mut();
        zeros.Map(0, None, Some(&mut zmap)).map_err(e("zeros map"))?;
        if zmap.is_null() {
            return Err("the zeros did not map".into());
        }
        std::ptr::write_bytes(zmap as *mut u8, 0, 256);
        zeros.Unmap(0, None);
        let count_readback =
            buffer(device, D3D12_HEAP_TYPE_READBACK, (COUNT_SLOTS * 8) as u64, D3D12_RESOURCE_STATE_COPY_DEST).map_err(e("counter read-back"))?;
        let mut cmap: *mut c_void = std::ptr::null_mut();
        count_readback.Map(0, None, Some(&mut cmap)).map_err(e("counter read-back map"))?;
        if cmap.is_null() {
            return Err("the counter read-back did not map".into());
        }
        // the adapter, for the memory watch (worker thread)
        crate::memwatch::set_adapter(device.GetAdapterLuid());
        let list: ID3D12GraphicsCommandList =
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &slots[0].allocator, None::<&ID3D12PipelineState>).map_err(e("command list"))?;
        list.Close().map_err(e("command list"))?;
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).map_err(e("fence"))?;
        let event = win::Handle(win::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()));
        if !event.valid() {
            return Err("no event".into());
        }
        let _ = list.SetName(windows::core::w!("Caveman FX overlay"));
        Ok(Gpu {
            device: device.clone(),
            root,
            pip: None,
            psos: Vec::new(),
            rtv_heap,
            rtv_size,
            srv_heap,
            srv_size,
            list,
            slots,
            next: 0,
            fence,
            value: 0,
            event,
            copies: None,
            generation: 0,
            retired: None,
            leaked: Vec::new(),
            last_make: None,
            readback,
            readback_map: map as *const u8,
            probes: [Probe::default(); PROBE_FRAMES],
            probe_next: 0,
            copy_textures: Vec::new(),
            counters,
            zeros,
            count_readback,
            count_map: cmap as *const u8,
            count_pending: [(0, 0); COUNT_SLOTS],
            count_next: 0,
        })
    }

    /// The blink counters of frames the GPU finished: (present, tested, hidden).
    unsafe fn harvest_counts(&mut self) -> Vec<(u64, u32, u32)> {
        let done = self.fence.GetCompletedValue();
        let mut out = Vec::new();
        if done == u64::MAX {
            return out;
        }
        for (k, p) in self.count_pending.iter_mut().enumerate() {
            if p.0 != 0 && p.0 <= done {
                let at = self.count_map.add(k * 8);
                let tested = std::ptr::read_unaligned(at as *const u32);
                let hidden = std::ptr::read_unaligned(at.add(4) as *const u32);
                out.push((p.1, tested, hidden));
                *p = (0, 0);
            }
        }
        out.sort_by_key(|x| x.0);
        out
    }

    /// The SRV index of a copy of a generation (None: the bank's null SRV).
    fn srv_index(&self, generation: u64, slot: Option<u32>) -> u32 {
        (generation % 2) as u32 * SRV_BANK + slot.map_or(0, |s| 1 + s)
    }

    fn null_srv(&self) -> u32 {
        0
    }

    /// Makes the ring of copies for the main depth's exact size and copy format when the set there is (if any) is not
    /// that, and hands them to the command list hooks. Only once the depth has been steady (`steady`: it ended passes
    /// in `depth::SWAP_STREAK` windows in a row), never while a replaced set is still alive, at most one new set every
    /// [`REMAKE_SECS`], and never within the margin of the process's video memory budget (`memwatch`, either segment).
    /// Meanwhile no copy is taken (the test runs late on the last copy, then off).
    unsafe fn ensure_copies(&mut self, source: depth::Desc, steady: bool) -> Result<(), String> {
        let format = source.copy_format().ok_or_else(|| format!("depth format {} cannot be copied", source.format))?;
        let size = (source.width, source.height);
        if self.copies.is_some_and(|c| c.format == format && c.size == size) {
            hold_back("");
            return Ok(());
        }
        if !steady {
            return Err(hold_back(&format!("waiting for the main depth ({}x{}) to be steady for {} windows", size.0, size.1, depth::SWAP_STREAK)));
        }
        if self.retired.is_some() {
            return Err(hold_back("waiting for the replaced set of copies to be freed"));
        }
        if self.last_make.is_some_and(|t| t.elapsed().as_secs_f32() < REMAKE_SECS) {
            return Err(hold_back(&format!("a new set of copies at most every {REMAKE_SECS} s")));
        }
        let texel = if format == DXGI_FORMAT_R16_UNORM { 2u64 } else { 4 };
        let bytes = size.0 as u64 * size.1 as u64 * texel * depth::RING as u64;
        if let Err(why) = crate::memwatch::room_for(bytes) {
            return Err(hold_back(&format!("the copies ({} MB) not made: {why}", bytes >> 20)));
        }
        self.last_make = Some(Instant::now());
        let mut textures = Vec::new();
        for _ in 0..depth::RING {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Alignment: 0,
                Width: size.0 as u64,
                Height: size.1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
                Flags: D3D12_RESOURCE_FLAG_NONE,
            };
            let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
            let mut t: Option<ID3D12Resource> = None;
            self.device
                .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, None, &mut t)
                .map_err(|e| format!("depth copy {}x{}: {e}", size.0, size.1))?;
            let t = t.ok_or("no depth copy")?;
            let _ = t.SetName(windows::core::w!("Caveman FX depth copy"));
            textures.push(t);
        }
        self.generation += 1;
        let generation = self.generation;
        let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 },
            },
        };
        for (k, t) in textures.iter().enumerate() {
            let mut h = self.srv_heap.GetCPUDescriptorHandleForHeapStart();
            h.ptr += (self.srv_index(generation, Some(k as u32)) * self.srv_size) as usize;
            self.device.CreateShaderResourceView(t, Some(&view), h);
        }
        self.copy_textures = textures.clone();
        let set = depth::CopySet { textures, size, format, generation };
        let old = depth::COPIES.lock().map_err(|_| "the copies' lock is poisoned")?.replace(set);
        if let Some(old) = old {
            self.retired = Some(Retired { set: old, present: depth::PRESENT_NO.load(Ordering::Acquire), value: self.value, at: Instant::now() });
        }
        self.copies = Some(CopyInfo { size, format, generation });
        depth::SETS_MADE.fetch_add(1, Ordering::Relaxed);
        log_later(format!("depth copies: a set of {} at {}x{} format {} ({} MB)", depth::RING, size.0, size.1, format.0, bytes >> 20));
        self.account();
        Ok(())
    }

    /// Frees the replaced set once nothing can use it: every copy recorded into it submitted (or reset) and run on its
    /// queue, our frames that read it done, and 10 presents past. A set with a copy still unsubmitted after a minute
    /// (a list the game neither submitted nor reset) is kept for good (`leaked`: a set lost is better than a texture
    /// freed under a list that may still run) and logged, and the way is clear for a new set.
    fn retire_old(&mut self) {
        let Some(r) = &self.retired else { return };
        let ours = unsafe { self.fence.GetCompletedValue() };
        let now = depth::PRESENT_NO.load(Ordering::Acquire);
        let (done, stale_presents) = depth::set_done(r.set.generation);
        if ours == u64::MAX || (ours >= r.value && now >= r.present + 10 && done) {
            self.retired = None;
            self.account();
        } else if r.at.elapsed().as_millis() as u64 > depth::STALE_PENDING_MS {
            let r = self.retired.take().unwrap();
            log_later(format!("depth copies: the replaced set (generation {}, {} MB) kept for good: a copy recorded into it {stale_presents} presents ago was never submitted nor reset", r.set.generation, r.set.bytes() >> 20));
            self.leaked.push(r.set);
            self.account();
        }
    }

    /// The copies' video memory, for the status.
    fn account(&self) {
        let current = depth::COPIES.try_lock().ok().and_then(|g| g.as_ref().map(|c| c.bytes())).unwrap_or(0);
        let retired = self.retired.as_ref().map_or(0, |r| r.set.bytes());
        let leaked: u64 = self.leaked.iter().map(|s| s.bytes()).sum();
        depth::HELD_BYTES.store(current + retired + leaked, Ordering::Relaxed);
        depth::SETS_ALIVE.store(self.copies.is_some() as u64 + self.retired.is_some() as u64 + self.leaked.len() as u64, Ordering::Relaxed);
        depth::SETS_LEAKED.store(self.leaked.len() as u64, Ordering::Relaxed);
    }

    /// The depth values read back for frames the GPU finished: (stored value, view depth, way).
    unsafe fn harvest(&mut self) -> Vec<(f32, f32, u8)> {
        let done = self.fence.GetCompletedValue();
        let mut out = Vec::new();
        let format = self.copies.map(|c| c.format);
        for (i, p) in self.probes.iter_mut().enumerate() {
            if !p.pending || p.fence > done || done == u64::MAX {
                continue;
            }
            p.pending = false;
            for k in 0..p.count {
                let at = self.readback_map.add(((i * PROBE_POINTS + k) as u64 * PROBE_STRIDE) as usize);
                let d = match format {
                    Some(DXGI_FORMAT_R16_UNORM) => std::ptr::read_unaligned(at as *const u16) as f32 / 65535.0,
                    _ => std::ptr::read_unaligned(at as *const f32),
                };
                out.push((d, p.z[k], p.way[k]));
            }
        }
        out
    }

    unsafe fn pso(&mut self, format: DXGI_FORMAT) -> Result<ID3D12PipelineState, String> {
        if let Some((_, p)) = self.psos.iter().find(|(f, _)| *f == format) {
            return Ok(p.clone());
        }
        let input = [
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 16,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("COLOR"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 32,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 1,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 48,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 2,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 64,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 3,
                Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 80,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];
        // premultiplied alpha: the solid style writes (colour x coverage, coverage) and replaces what is under it; the
        // test rectangle and the glow style write (colour x coverage, 0) and add to it. The back buffer's alpha stays.
        let target = D3D12_RENDER_TARGET_BLEND_DESC {
            BlendEnable: TRUE,
            LogicOpEnable: FALSE,
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ZERO,
            DestBlendAlpha: D3D12_BLEND_ONE,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            LogicOp: D3D12_LOGIC_OP_NOOP,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let stencil = D3D12_DEPTH_STENCILOP_DESC {
            StencilFailOp: D3D12_STENCIL_OP_KEEP,
            StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
            StencilPassOp: D3D12_STENCIL_OP_KEEP,
            StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        };
        let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
        formats[0] = format;
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&self.root),
            VS: D3D12_SHADER_BYTECODE { pShaderBytecode: VS.as_ptr().cast(), BytecodeLength: VS.len() },
            PS: D3D12_SHADER_BYTECODE { pShaderBytecode: PS.as_ptr().cast(), BytecodeLength: PS.len() },
            BlendState: D3D12_BLEND_DESC { AlphaToCoverageEnable: FALSE, IndependentBlendEnable: FALSE, RenderTarget: [target; 8] },
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                FrontCounterClockwise: FALSE,
                DepthBias: 0,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: 0.0,
                DepthClipEnable: TRUE,
                MultisampleEnable: FALSE,
                AntialiasedLineEnable: FALSE,
                ForcedSampleCount: 0,
                ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
            },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: FALSE,
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
                DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                StencilEnable: FALSE,
                StencilReadMask: 0xFF,
                StencilWriteMask: 0xFF,
                FrontFace: stencil,
                BackFace: stencil,
            },
            InputLayout: D3D12_INPUT_LAYOUT_DESC { pInputElementDescs: input.as_ptr(), NumElements: input.len() as u32 },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            RTVFormats: formats,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let pso: ID3D12PipelineState = self.device.CreateGraphicsPipelineState(&desc).map_err(|e| format!("pipeline for format {}: {e}", format.0))?;
        self.psos.push((format, pso.clone()));
        Ok(pso)
    }

    /// The board cam into its square of the back buffer (after the bars, in the same list): the backdrop, then the
    /// meshes, the deck's underside with the board's live art.
    unsafe fn draw_pip(
        &mut self,
        list: &ID3D12GraphicsCommandList,
        rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
        format: DXGI_FORMAT,
        pf: &PipFrame,
    ) -> Result<(), String> {
        let Some((mesh, generation)) = crate::pip::current() else { return Ok(()) };
        if self.pip.is_none() {
            self.pip = Some(PipGpu::new(&self.device)?);
        }
        let device = self.device.clone();
        let (srv_heap, srv_size) = (self.srv_heap.clone(), self.srv_size);
        // the meshes' buffers and the depth texture are replaced only once the GPU is done with every frame of ours:
        // a list in flight reads them, and dropping them under it is a page fault (the device hung, the game dead,
        // seen twice 2026-10-22 after a mesh re-send). Until then the old meshes stay; a depth of the wrong size
        // means no cam this frame
        let (mesh_changes, depth_changes) = match self.pip.as_ref() {
            Some(p) => (p.generation != generation && p.vb.is_some(), p.depth.as_ref().is_some_and(|(_, w, h)| *w != pf.target[0] || *h != pf.target[1])),
            None => (false, false),
        };
        let replace = !(mesh_changes || depth_changes) || self.frames_done(200);
        if !replace && depth_changes {
            return Ok(());
        }
        let p = self.pip.as_mut().unwrap();
        if replace {
            p.ensure_mesh(&device, &mesh, generation)?;
            p.ensure_depth(&device, pf.target[0], pf.target[1])?;
        }
        // the board's live art: the copy the in-world board samples, held while the cam shows it. A new texture
        // (the game remakes it at an equip) goes into the other descriptor, and only once no frame in flight reads
        // that one (the fence after its last use has passed); else this frame keeps the old
        let art_raw = match crate::pip::test_art() {
            0 => crate::board::ridden_art_resource(),
            test => test,
        };
        if art_raw != 0 && p.arts[p.art_slot as usize].as_ref().is_none_or(|(_, raw)| *raw != art_raw) {
            let other = 1 - p.art_slot;
            if self.fence.GetCompletedValue() >= p.art_used[other as usize] {
                let raw = art_raw as *mut c_void;
                if let Some(r) = ID3D12Resource::from_raw_borrowed(&raw) {
                    let r = r.clone();
                    let d = r.GetDesc();
                    if d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D && d.MipLevels >= 1 {
                        let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
                            Format: srv_format(d.Format),
                            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                                Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: d.MipLevels as u32, PlaneSlice: 0, ResourceMinLODClamp: 0.0 },
                            },
                        };
                        let mut h = srv_heap.GetCPUDescriptorHandleForHeapStart();
                        h.ptr += ((PIP_ART_SRV + other) * srv_size) as usize;
                        device.CreateShaderResourceView(&r, Some(&view), h);
                        crate::worker::log(&format!(
                            "board cam: the art bound: resource {art_raw:#x}, {}x{} format {} with {} mip(s), viewed as format {}",
                            d.Width,
                            d.Height,
                            d.Format.0,
                            d.MipLevels,
                            srv_format(d.Format).0
                        ));
                        p.arts[other as usize] = Some((r, art_raw));
                        p.art_slot = other;
                    }
                }
            }
        }
        let art_ok = p.arts[p.art_slot as usize].is_some() && pf.art_on;
        // this frame's list reads the bound descriptor until the fence signalled after it passes
        p.art_used[p.art_slot as usize] = self.value + 1;
        let pso = p.pso(&device, format)?;
        let [x, y, w, h] = pf.viewport;
        let rect = RECT { left: x as i32, top: y as i32, right: (x + w) as i32, bottom: (y + h) as i32 };
        list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: x, TopLeftY: y, Width: w, Height: h, MinDepth: 0.0, MaxDepth: 1.0 }]);
        list.RSSetScissorRects(&[rect]);
        let dsv = p.dsv_heap.GetCPUDescriptorHandleForHeapStart();
        list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 1.0, 0, &[rect]);
        static PIP_FIRST_DRAW: AtomicBool = AtomicBool::new(false);
        if !PIP_FIRST_DRAW.swap(true, Ordering::AcqRel) {
            crate::worker::log(&format!(
                "board cam: drawing {}x{} at ({}, {}) of a {}x{} target, {} part(s), art {}",
                w,
                h,
                x,
                y,
                pf.target[0],
                pf.target[1],
                p.parts.len(),
                if art_ok { "on" } else { "off" }
            ));
        }
        list.OMSetRenderTargets(1, Some(&rtv), BOOL::from(false), Some(&dsv));
        list.SetPipelineState(&pso);
        list.SetGraphicsRootSignature(&p.root);
        let mut table = srv_heap.GetGPUDescriptorHandleForHeapStart();
        table.ptr += ((PIP_ART_SRV + p.art_slot) * srv_size) as u64;
        list.SetGraphicsRootDescriptorTable(1, table);
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        let (vb, nverts) = p.vb.as_ref().ok_or("no vertex buffer")?;
        let vbv = D3D12_VERTEX_BUFFER_VIEW { BufferLocation: vb.GetGPUVirtualAddress(), SizeInBytes: nverts * 24, StrideInBytes: 24 };
        list.IASetVertexBuffers(0, Some(&[vbv]));
        let (ib, nidx) = p.ib.as_ref().ok_or("no index buffer")?;
        let ibv = D3D12_INDEX_BUFFER_VIEW { BufferLocation: ib.GetGPUVirtualAddress(), SizeInBytes: nidx * 4, Format: DXGI_FORMAT_R32_UINT };
        list.IASetIndexBuffer(Some(&ibv));
        let (mode, nits) = (pf.mode as f32, pf.nits);
        if pf.backdrop > 0.0 {
            let c = pip_constants(&pf.mvp, pf.light, [0.05, 0.05, 0.06, pf.backdrop], pf.art, [0.0, mode, nits, 1.0]);
            list.SetGraphicsRoot32BitConstants(0, PIP_CONSTANTS as u32, c.as_ptr().cast(), 0);
            list.DrawInstanced(6, 1, p.quad_first, 0);
        }
        for part in &p.parts {
            let (colour, art) = match part.kind.as_str() {
                "deck" => ([0.17, 0.17, 0.19, 1.0], art_ok),
                "trucks" => ([0.70, 0.71, 0.74, 1.0], false),
                _ => ([0.93, 0.93, 0.90, 1.0], false),
            };
            let c = pip_constants(&pf.mvp, pf.light, colour, pf.art, [if art { 1.0 } else { 0.0 }, mode, nits, 0.0]);
            list.SetGraphicsRoot32BitConstants(0, PIP_CONSTANTS as u32, c.as_ptr().cast(), 0);
            list.DrawIndexedInstanced(part.index_count, 1, part.first_index, 0, 0);
        }
        list.OMSetRenderTargets(1, Some(&rtv), BOOL::from(false), None);
        crate::pip::note_drawn(art_ok);
        Ok(())
    }

    /// Waits, at most `ms`, for the GPU to finish every frame of ours submitted so far. False when it has not.
    unsafe fn frames_done(&self, ms: u32) -> bool {
        if self.fence.GetCompletedValue() >= self.value {
            return true;
        }
        self.fence.SetEventOnCompletion(self.value, HANDLE(self.event.0 as isize)).is_ok() && win::WaitForSingleObject(self.event.0, ms) == win::WAIT_OBJECT_0
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn draw(
        &mut self,
        queue: &ID3D12CommandQueue,
        buffer: &ID3D12Resource,
        index: u32,
        format: DXGI_FORMAT,
        width: u32,
        height: u32,
        mode: u8,
        settings: &BarSettings,
        verts: &[bars::Vertex],
        du: &DepthUse,
        pip: Option<&PipFrame>,
    ) -> Result<bool, String> {
        let pso = self.pso(format)?;
        let slot_i = self.next;
        let done = self.fence.GetCompletedValue();
        if done == u64::MAX {
            return Err("the device was removed".into());
        }
        if done < self.slots[slot_i].fence {
            // at most 2 ms for the GPU to finish this slot's last frame; else skip ours
            if self.fence.SetEventOnCompletion(self.slots[slot_i].fence, HANDLE(self.event.0 as isize)).is_err()
                || win::WaitForSingleObject(self.event.0, 2) != win::WAIT_OBJECT_0
            {
                return Ok(false);
            }
        }
        self.next = (self.next + 1) % self.slots.len();
        let slot = &self.slots[slot_i];
        let bytes = std::mem::size_of_val(verts).min(VB_BYTES);
        let count = (bytes / std::mem::size_of::<bars::Vertex>()) as u32;
        std::ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, slot.map, bytes);
        slot.allocator.Reset().map_err(|e| format!("allocator reset: {e}"))?;
        let list = self.list.clone();
        list.Reset(&slot.allocator, &pso).map_err(|e| format!("list reset: {e}"))?;
        let mut rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        rtv.ptr += (index % RTVS) as usize * self.rtv_size as usize;
        self.device.CreateRenderTargetView(buffer, None, rtv);
        let barrier = |res: &ID3D12Resource, before, after| D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: std::mem::transmute_copy(res),
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: before,
                    StateAfter: after,
                }),
            },
        };
        // the blink counters: this frame's pair zeroed (from the zeros buffer), then written by the pixel shader, then
        // copied out to the read-back; explicit states throughout (a buffer decays to COMMON after the list runs)
        let count_slot = self.count_next;
        let counting = du.on && self.count_pending[count_slot].0 == 0;
        if counting {
            list.ResourceBarrier(&[barrier(&self.counters, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST)]);
            list.CopyBufferRegion(&self.counters, count_slot as u64 * 8, &self.zeros, 0, 8);
            list.ResourceBarrier(&[barrier(&self.counters, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
        }
        list.ResourceBarrier(&[barrier(buffer, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
        list.OMSetRenderTargets(1, Some(&rtv), BOOL::from(false), None);
        list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: width as f32, Height: height as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
        list.RSSetScissorRects(&[RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 }]);
        list.SetGraphicsRootSignature(&self.root);
        list.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
        let mut table = self.srv_heap.GetGPUDescriptorHandleForHeapStart();
        table.ptr += (du.srv * self.srv_size) as u64;
        list.SetGraphicsRootDescriptorTable(1, table);
        let constants: [f32; CONSTANTS] = [
            mode as f32,
            200.0 * settings.brightness,
            if du.on { 1.0 } else { 0.0 },
            settings.depth_bias,
            du.map[0],
            du.map[1],
            du.map[2],
            du.map[3],
            du.lin[0],
            du.lin[1],
            du.lin[2],
            du.lin[3],
            // the blink counters: the slot, on, padding
            count_slot as f32,
            if counting { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ];
        list.SetGraphicsRoot32BitConstants(0, CONSTANTS as u32, constants.as_ptr().cast(), 0);
        list.SetGraphicsRootUnorderedAccessView(2, self.counters.GetGPUVirtualAddress());
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        let vbv = D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: slot.vb.GetGPUVirtualAddress(),
            SizeInBytes: bytes as u32,
            StrideInBytes: std::mem::size_of::<bars::Vertex>() as u32,
        };
        list.IASetVertexBuffers(0, Some(&[vbv]));
        list.DrawInstanced(count, 1, 0, 0);
        // the board cam, in its corner (its own pipeline, depth and art; a failure is logged once and skipped)
        if let Some(pf) = pip {
            if let Err(e) = self.draw_pip(&list, rtv, format, pf) {
                static PIP_ERR_LOGGED: AtomicBool = AtomicBool::new(false);
                if !PIP_ERR_LOGGED.swap(true, Ordering::AcqRel) {
                    crate::worker::log(&format!("board cam: {e}"));
                }
            }
        }
        list.ResourceBarrier(&[barrier(buffer, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT)]);
        if counting {
            list.ResourceBarrier(&[barrier(&self.counters, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            list.CopyBufferRegion(&self.count_readback, count_slot as u64 * 8, &self.counters, count_slot as u64 * 8, 8);
            list.ResourceBarrier(&[barrier(&self.counters, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON)]);
        }
        // the calibration: the depth at points of the deck, into the read-back buffer
        let mut probe = None;
        if let (Some(_), false) = (du.copy, du.probes.is_empty()) {
            let copy_format = self.copies.map(|c| c.format);
            if let Some(f) = copy_format {
                let pi = self.probe_next;
                if !self.probes[pi].pending {
                    let one = |res: &ID3D12Resource, before, after| D3D12_RESOURCE_BARRIER {
                        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                        Anonymous: D3D12_RESOURCE_BARRIER_0 {
                            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                                pResource: std::mem::transmute_copy(res),
                                Subresource: 0,
                                StateBefore: before,
                                StateAfter: after,
                            }),
                        },
                    };
                    let mut p = Probe { fence: 0, z: [0.0; PROBE_POINTS], way: [0; PROBE_POINTS], count: 0, pending: true };
                    // the copies read, each once in COPY_SOURCE for its texels
                    let mut slots: Vec<u32> = du.probes.iter().take(PROBE_POINTS).map(|q| q.4).collect();
                    slots.sort_unstable();
                    slots.dedup();
                    for s in slots {
                        let Some(t) = self.copy_textures.get(s as usize).cloned() else { continue };
                        list.ResourceBarrier(&[one(&t, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
                        for &(x, y, z, way, _) in du.probes.iter().take(PROBE_POINTS).filter(|q| q.4 == s) {
                            let k = p.count;
                            let to = D3D12_TEXTURE_COPY_LOCATION {
                                pResource: std::mem::transmute_copy(&self.readback),
                                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                                    PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                                        Offset: (pi * PROBE_POINTS + k) as u64 * PROBE_STRIDE,
                                        Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                                            Format: f,
                                            Width: 1,
                                            Height: 1,
                                            Depth: 1,
                                            RowPitch: D3D12_TEXTURE_DATA_PITCH_ALIGNMENT,
                                        },
                                    },
                                },
                            };
                            let from = D3D12_TEXTURE_COPY_LOCATION {
                                pResource: std::mem::transmute_copy(&t),
                                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
                            };
                            let b = D3D12_BOX { left: x, top: y, front: 0, right: x + 1, bottom: y + 1, back: 1 };
                            list.CopyTextureRegion(&to, 0, 0, 0, &from, Some(&b));
                            p.z[k] = z;
                            p.way[k] = way;
                            p.count += 1;
                        }
                        list.ResourceBarrier(&[one(&t, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
                    }
                    probe = Some((pi, p));
                }
            }
        }
        list.Close().map_err(|e| format!("list close: {e}"))?;
        let as_list: ID3D12CommandList = list.cast().map_err(|e| format!("list: {e}"))?;
        // a copy submitted on another queue (the game renders on one queue and presents from another, with frame
        // generation): the draw waits for that queue's fence on the GPU, so it reads the copy once it is written
        if du.on && du.wait_queue != 0 && du.wait_queue != queue.as_raw() as usize {
            depth::wait_on(queue, du.wait_queue, du.wait_fence);
        }
        queue.ExecuteCommandLists(&[Some(as_list)]);
        self.value += 1;
        queue.Signal(&self.fence, self.value).map_err(|e| format!("signal: {e}"))?;
        self.slots[slot_i].fence = self.value;
        // the copy slot this draw read: no copy into it until this fence passes (depth::before_execute)
        if let Some(slot) = du.copy {
            depth::note_read(slot, &self.fence, self.value);
        }
        if counting {
            self.count_pending[count_slot] = (self.value, depth::PRESENT_NO.load(Ordering::Relaxed));
            self.count_next = (count_slot + 1) % COUNT_SLOTS;
        }
        if let Some((pi, mut p)) = probe {
            p.fence = self.value;
            self.probes[pi] = p;
            self.probe_next = (pi + 1) % PROBE_FRAMES;
        }
        Ok(true)
    }
}

/// Why no set of copies is being made now (empty: none is held back), for the status and, when it changes, the log.
/// Returns the reason.
fn hold_back(why: &str) -> String {
    if let Ok(mut h) = depth::HELD_BACK.try_lock() {
        if *h != why {
            if !why.is_empty() {
                log_later(format!("depth copies held back: {why}"));
            }
            *h = why.to_string();
        }
    }
    why.to_string()
}

/// A committed buffer.
unsafe fn buffer(device: &ID3D12Device, kind: D3D12_HEAP_TYPE, bytes: u64, state: D3D12_RESOURCE_STATES) -> windows::core::Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut b: Option<ID3D12Resource> = None;
    device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut b)?;
    b.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

impl Drop for Gpu {
    fn drop(&mut self) {
        for s in &self.slots {
            unsafe { s.vb.Unmap(0, None) };
        }
        unsafe { self.readback.Unmap(0, None) };
    }
}

// ---------------------------------------------------------------- status

/// The overlay's state for the trainer.
pub fn status() -> Value {
    let i = info().clone();
    let mut queues: Vec<Value> = Vec::new();
    for k in 0..QUEUES {
        let p = Q_PTR[k].load(Ordering::Relaxed);
        if p != 0 {
            queues.push(json!({"queue": format!("{p:#x}"), "submits": Q_COUNT[k].load(Ordering::Relaxed)}));
        }
    }
    let queue_mode = ["auto", "swapchain", "scan", "busiest"][QUEUE_MODE.load(Ordering::Relaxed).min(3) as usize];
    json!({
        "installed": installed(),
        "install": i.install,
        "streamline": SL_PRESENT.installed(),
        "streamline_wanted": STREAMLINE.load(Ordering::Relaxed),
        "test": TEST.load(Ordering::Relaxed),
        "bars": BARS_ON.load(Ordering::Relaxed),
        "failed": FAILED.load(Ordering::Relaxed),
        "presents": PRESENTS.load(Ordering::Relaxed),
        "frames": FRAMES.load(Ordering::Relaxed),
        "skipped": SKIPPED.load(Ordering::Relaxed),
        "failures": FAILURES.load(Ordering::Relaxed),
        "error": i.error,
        "via": i.via,
        "queue": i.queue,
        "queue_mode": queue_mode,
        "format": i.format,
        "size": [i.size.0, i.size.1],
        "output": i.mode,
        "scene": i.scene,
        "timing": i.timing,
        "proxies": proxy::status(),
        "depth_state": i.depth,
        "depth": depth::status(),
        "video_memory": crate::memwatch::status(),
        "memory_guard": crate::memguard::status(),
        "alloc_trace": crate::alloctrace::status(),
        "depth_frames": i.depth_frames,
        "vertices": i.vertices,
        "landings": LANDINGS.load(Ordering::Relaxed),
        "queues_seen": queues,
        "dxgi_chain_vtable": format!("{:#x}", DXGI_CHAIN_VT.load(Ordering::Relaxed)),
        "present_slot": format!("{:#x}", DXGI_PRESENT.slot()),
        "streamline_queue_vtable": format!("{:#x}", SL_QUEUE_VT.load(Ordering::Relaxed)),
        "queue_found_at": format!("{:#x}", SCAN_AT.load(Ordering::Relaxed)),
    })
}

/// The DXGI swap chain vtable the hooks went into (0 before install), for tests.
pub fn dxgi_chain_vtable() -> usize {
    DXGI_CHAIN_VT.load(Ordering::Acquire)
}
