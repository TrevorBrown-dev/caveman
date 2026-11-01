//! The overlay, offline: a host with its own D3D12 device, queue and swap chain on a hidden window, standing in for
//! the game. The host hooks its swap chain's `Present` slot first (as ReSkate and the Steam overlay sit on the
//! game's), then loads the built `caveman_fx.dll` (dormant: not Skate.exe), turns drawing on over the pipe, and reads
//! back what reaches `Present`: the DLL's hook runs first, draws, and calls the host's hook, which copies the back
//! buffer to a readback buffer before calling DXGI. Checked: the test rectangle, the bars over a given scene (and a
//! landing spike that settles), three back buffer formats through `ResizeBuffers` (never blocked by the DLL), each
//! queue strategy, the D3D12 debug layer's verdict, and that unload puts the host's hook back.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use caveman_fx::bars::{self, Camera};
use caveman_fx::pip::{PipMesh, PipPart};
use caveman_fx::proto::{self, Request};
use caveman_fx::win;
use serde_json::{json, Value};
use windows::core::{IUnknown, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{BOOL, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

const W: u32 = 320;
const H: u32 = 240;

type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT;
static HOST_ORIGINAL: AtomicUsize = AtomicUsize::new(0);

/// What the host's Present hook needs to copy the back buffer.
struct Capture {
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    readback: ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    fence: ID3D12Fence,
    value: u64,
    event: HANDLE,
    captured: u64,
}

unsafe impl Send for Capture {}

static CAPTURE: Mutex<Option<Capture>> = Mutex::new(None);

fn barrier(resource: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

unsafe fn wait(fence: &ID3D12Fence, value: u64, event: HANDLE) {
    if fence.GetCompletedValue() < value {
        fence.SetEventOnCompletion(value, event).unwrap();
        win::WaitForSingleObject(event.0 as win::HANDLE, 5000);
    }
}

/// The host's own Present hook: copies the back buffer (after anything hooked above it drew), then calls DXGI.
unsafe extern "system" fn host_present(this: *mut c_void, sync: u32, flags: u32) -> HRESULT {
    if let Ok(mut g) = CAPTURE.lock() {
        if let Some(c) = g.as_mut() {
            let chain = IDXGISwapChain::from_raw_borrowed(&this).unwrap().cast::<IDXGISwapChain3>().unwrap();
            let buffer: ID3D12Resource = chain.GetBuffer(chain.GetCurrentBackBufferIndex()).unwrap();
            wait(&c.fence, c.value, c.event);
            c.allocator.Reset().unwrap();
            c.list.Reset(&c.allocator, None).unwrap();
            c.list.ResourceBarrier(&[barrier(&buffer, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            let dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&c.readback),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: c.footprint },
            };
            let src = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&buffer),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            c.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
            c.list.ResourceBarrier(&[barrier(&buffer, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_PRESENT)]);
            c.list.Close().unwrap();
            c.queue.ExecuteCommandLists(&[Some(c.list.cast().unwrap())]);
            c.value += 1;
            c.queue.Signal(&c.fence, c.value).unwrap();
            c.captured += 1;
        }
    }
    let f: PresentFn = std::mem::transmute::<usize, PresentFn>(HOST_ORIGINAL.load(Ordering::Acquire));
    f(this, sync, flags)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, w, l)
}

struct Host {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    chain: IDXGISwapChain3,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    rtv_heap: ID3D12DescriptorHeap,
    fence: ID3D12Fence,
    value: u64,
    event: HANDLE,
    format: DXGI_FORMAT,
    /// Present through this fake Streamline wrapper instead of the swap chain itself.
    via_wrapper: Option<*mut c_void>,
    /// The colour each frame is cleared to.
    clear: [f32; 4],
    /// A depth pass each frame, as the game's G-buffer pass ends: the overlay's hooks copy the depth there.
    depth: Option<HostDepth>,
}

/// The host's depth buffer (R32_TYPELESS, a D32_FLOAT view, reversed Z) at half the back buffer's size, as an
/// upscaler renders: each frame cleared to `base`, then each rect to its value, then moved out of DEPTH_WRITE.
struct HostDepth {
    res: ID3D12Resource,
    /// The main depth's view, then a quarter-size depth's (a half-resolution pass after the main one, as the game has).
    dsv_heap: ID3D12DescriptorHeap,
    _half: ID3D12Resource,
    w: u32,
    h: u32,
    first: bool,
    base: f32,
    /// Rects of the full-resolution picture (in texels of the whole depth) and their values: drawn scaled into the
    /// frame's viewport.
    rects: Vec<(RECT, f32)>,
    /// The dynamic resolution: each frame renders into this share of the depth (cycled).
    scales: Vec<f32>,
    frame: usize,
    /// Every this many frames (0: never) the window's only pass end of the main depth is another pass's: the whole
    /// depth cleared to `wrong_value` and left in DEPTH_READ (where the G-buffer's leaves it in PIXEL_SHADER_RESOURCE),
    /// as when the next frame's recording overlaps the present. Every other frame has such a later pass after the
    /// G-buffer's (writing nothing).
    wrong_every: usize,
    wrong_value: f32,
}

impl Host {
    unsafe fn new(format: DXGI_FORMAT) -> Host {
        use windows::Win32::UI::WindowsAndMessaging::*;
        // the debug layer, when the Graphics Tools are installed: our API use is checked (not with the game's SDK:
        // there the point is D3D12Core's own path, where each command list has a vtable of its own, as in the game)
        let sdk = game_sdk();
        let debug_layer = sdk.is_none() && std::env::var("CAVEMAN_HOST_DEBUG_LAYER").map_or(true, |v| v != "0");
        if debug_layer {
            if let Ok(debug) = {
                let mut d: Option<ID3D12Debug> = None;
                D3D12GetDebugInterface(&mut d).map(|_| d.unwrap())
            } {
                debug.EnableDebugLayer();
            }
        }
        let mut device: Option<ID3D12Device> = None;
        match &sdk {
            Some((version, path)) => {
                let mut config: Option<ID3D12SDKConfiguration1> = None;
                D3D12GetInterface(&CLSID_D3D12SDKConfiguration, &mut config).unwrap();
                let path = std::ffi::CString::new(path.as_str()).unwrap();
                let factory: ID3D12DeviceFactory = config.unwrap().CreateDeviceFactory(*version, windows::core::PCSTR(path.as_ptr().cast())).unwrap();
                factory.CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap();
                println!("the host's device is from the Agility SDK {version} in {}", path.to_string_lossy());
            }
            None => D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap(),
        }
        let device = device.unwrap();
        // the host's own depth clears use other values than the resource's optimized one, on purpose (the occluder):
        // not stored, so the queue (1024 messages at most) keeps room for anything the DLL does wrong
        if let Ok(q) = device.cast::<ID3D12InfoQueue>() {
            let mut ids = [D3D12_MESSAGE_ID_CLEARDEPTHSTENCILVIEW_MISMATCHINGCLEARVALUE];
            let filter = D3D12_INFO_QUEUE_FILTER {
                DenyList: D3D12_INFO_QUEUE_FILTER_DESC { NumIDs: 1, pIDList: ids.as_mut_ptr(), ..Default::default() },
                ..Default::default()
            };
            q.AddStorageFilterEntries(&filter).unwrap();
        }
        let queue: ID3D12CommandQueue =
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
        let class = windows::core::w!("CavemanFxOverlayHost");
        let instance = windows::Win32::Foundation::HINSTANCE(win::GetModuleHandleW(std::ptr::null()) as isize);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, W as i32, H as i32, None, None, instance, None);
        assert!(hwnd.0 != 0);
        let factory: IDXGIFactory2 = CreateDXGIFactory2(0).unwrap();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: W,
            Height: H,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        let chain: IDXGISwapChain3 = factory.CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None::<&IDXGIOutput>).unwrap().cast().unwrap();
        let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).unwrap();
        list.Close().unwrap();
        let rtv_heap: ID3D12DescriptorHeap =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() }).unwrap();
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        let event = HANDLE(win::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) as isize);
        let host = Host {
            device,
            queue,
            chain,
            allocator,
            list,
            rtv_heap,
            fence,
            value: 0,
            event,
            format,
            via_wrapper: None,
            clear: [0.0, 0.0, 0.0, 1.0],
            depth: None,
        };
        host.set_capture();
        host
    }

    /// The readback buffer for the current format.
    unsafe fn set_capture(&self) {
        let buffer: ID3D12Resource = self.chain.GetBuffer(0).unwrap();
        let desc = buffer.GetDesc();
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0u64;
        self.device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
        let rdesc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: total,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut readback: Option<ID3D12Resource> = None;
        self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &rdesc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut readback).unwrap();
        let allocator: ID3D12CommandAllocator = self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let list: ID3D12GraphicsCommandList =
            self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None::<&ID3D12PipelineState>).unwrap();
        list.Close().unwrap();
        let fence: ID3D12Fence = self.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        let event = HANDLE(win::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null()) as isize);
        *CAPTURE.lock().unwrap() =
            Some(Capture { queue: self.queue.clone(), allocator, list, readback: readback.unwrap(), footprint, fence, value: 0, event, captured: 0 });
    }

    /// Clears the back buffer to black and presents it.
    unsafe fn frame(&mut self) {
        wait(&self.fence, self.value, self.event);
        let buffer: ID3D12Resource = self.chain.GetBuffer(self.chain.GetCurrentBackBufferIndex()).unwrap();
        let rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(&buffer, None, rtv);
        self.allocator.Reset().unwrap();
        self.list.Reset(&self.allocator, None).unwrap();
        let wrong = self.depth.as_ref().is_some_and(|d| d.wrong_every > 0 && d.frame % d.wrong_every == d.wrong_every - 1);
        if let Some(d) = self.depth.as_mut().filter(|_| wrong) {
            // another pass instead of the G-buffer's: the whole depth near, ended to DEPTH_READ
            if !d.first {
                self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_DEPTH_WRITE)]);
            }
            d.first = false;
            d.frame += 1;
            let dsv = d.dsv_heap.GetCPUDescriptorHandleForHeapStart();
            self.list.OMSetRenderTargets(0, None, BOOL::from(false), Some(&dsv));
            self.list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, d.wrong_value, 0, &[]);
            self.list.OMSetRenderTargets(0, None, BOOL::from(false), None);
            self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_DEPTH_READ)]);
            self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_DEPTH_READ, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
        }
        if let Some(d) = self.depth.as_mut().filter(|_| !wrong) {
            if !d.first {
                self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_DEPTH_WRITE)]);
            }
            d.first = false;
            let scale = d.scales[d.frame % d.scales.len()];
            d.frame += 1;
            let (vw, vh) = ((d.w as f32 * scale).round(), (d.h as f32 * scale).round());
            let dsv = d.dsv_heap.GetCPUDescriptorHandleForHeapStart();
            let mut half_dsv = dsv;
            half_dsv.ptr += self.device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_DSV) as usize;
            // the scene pass: the frame's viewport, the main depth bound, its picture scaled into the viewport
            self.list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: 0.0, TopLeftY: 0.0, Width: vw, Height: vh, MinDepth: 0.0, MaxDepth: 1.0 }]);
            self.list.OMSetRenderTargets(0, None, BOOL::from(false), Some(&dsv));
            self.list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, d.base, 0, &[]);
            for (r, v) in &d.rects {
                let s = |x: i32| (x as f32 * scale).round() as i32;
                let r = RECT { left: s(r.left), top: s(r.top), right: s(r.right).max(s(r.left) + 1), bottom: s(r.bottom).max(s(r.top) + 1) };
                self.list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, *v, 0, &[r]);
            }
            // a half-resolution pass of its own depth before the main depth's barrier (its viewport is the last one
            // set when that barrier comes, as in the game)
            self.list.OMSetRenderTargets(0, None, BOOL::from(false), Some(&half_dsv));
            self.list.RSSetViewports(&[D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: (vw / 2.0).floor(),
                Height: (vh / 2.0).floor(),
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]);
            self.list.ClearDepthStencilView(half_dsv, D3D12_CLEAR_FLAG_DEPTH, d.base, 0, &[]);
            self.list.OMSetRenderTargets(0, None, BOOL::from(false), None);
            // the end of the depth pass: the overlay's hook records its copy right after this barrier
            self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
            if d.wrong_every > 0 {
                // a later pass of the main depth (writing nothing): another pass end, left alone
                self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_DEPTH_WRITE)]);
                self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_DEPTH_READ)]);
                self.list.ResourceBarrier(&[barrier(&d.res, D3D12_RESOURCE_STATE_DEPTH_READ, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
            }
        }
        self.list.ResourceBarrier(&[barrier(&buffer, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
        self.list.ClearRenderTargetView(rtv, &self.clear, None);
        self.list.ResourceBarrier(&[barrier(&buffer, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT)]);
        self.list.Close().unwrap();
        self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
        drop(buffer);
        match self.via_wrapper {
            // as the game calls Streamline's wrapper: through its vtable's Present slot
            Some(w) => {
                let f: PresentFn = std::mem::transmute::<usize, PresentFn>(*(*(w as *const *const usize)).add(8));
                let _ = f(w, 0, 0);
            }
            None => {
                let _ = self.chain.Present(0, 0);
            }
        }
        self.value += 1;
        self.queue.Signal(&self.fence, self.value).unwrap();
    }

    /// Frames for `ms` milliseconds (the DLL's clock is real time).
    unsafe fn run(&mut self, ms: u64) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            self.frame();
            std::thread::sleep(Duration::from_millis(8));
        }
    }

    /// The last captured frame as linear-ish 0..1 RGB (each format decoded as it is stored).
    unsafe fn pixels(&self) -> Vec<[f32; 3]> {
        let g = CAPTURE.lock().unwrap();
        let c = g.as_ref().unwrap();
        wait(&c.fence, c.value, c.event);
        let mut p: *mut c_void = std::ptr::null_mut();
        c.readback.Map(0, None, Some(&mut p)).unwrap();
        let pitch = c.footprint.Footprint.RowPitch as usize;
        let mut out = Vec::with_capacity((W * H) as usize);
        for y in 0..H as usize {
            for x in 0..W as usize {
                let at = (p as *const u8).add(y * pitch);
                out.push(match self.format {
                    DXGI_FORMAT_R10G10B10A2_UNORM => {
                        let v = std::ptr::read_unaligned(at.add(x * 4) as *const u32);
                        [(v & 0x3FF) as f32 / 1023.0, ((v >> 10) & 0x3FF) as f32 / 1023.0, ((v >> 20) & 0x3FF) as f32 / 1023.0]
                    }
                    DXGI_FORMAT_R16G16B16A16_FLOAT => {
                        let h = |k: usize| half(std::ptr::read_unaligned(at.add(x * 8 + k * 2) as *const u16));
                        [h(0), h(1), h(2)]
                    }
                    _ => {
                        let b = std::slice::from_raw_parts(at.add(x * 4), 4);
                        [b[0] as f32 / 255.0, b[1] as f32 / 255.0, b[2] as f32 / 255.0]
                    }
                });
            }
        }
        c.readback.Unmap(0, None);
        out
    }

    unsafe fn wait_idle(&mut self) {
        self.value += 1;
        self.queue.Signal(&self.fence, self.value).unwrap();
        wait(&self.fence, self.value, self.event);
    }

    /// Gives the host a depth pass (see [`HostDepth`]), at half the back buffer's size.
    unsafe fn enable_depth(&mut self, base: f32, typeless: DXGI_FORMAT, view_format: DXGI_FORMAT) {
        self.enable_depth_sized(base, typeless, view_format, W / 2, H / 2);
    }

    /// The same, at a size of its own (a game making its main depth anew at another render size).
    unsafe fn enable_depth_sized(&mut self, base: f32, typeless: DXGI_FORMAT, view_format: DXGI_FORMAT, w: u32, h: u32) {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: w as u64,
            Height: h,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: typeless,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            ..Default::default()
        };
        let clear =
            D3D12_CLEAR_VALUE { Format: view_format, Anonymous: D3D12_CLEAR_VALUE_0 { DepthStencil: D3D12_DEPTH_STENCIL_VALUE { Depth: 0.0, Stencil: 0 } } };
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let mut res: Option<ID3D12Resource> = None;
        self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_DEPTH_WRITE, Some(&clear), &mut res).unwrap();
        let res = res.unwrap();
        // the half-resolution pass's own depth (stays in DEPTH_WRITE)
        let half_desc = D3D12_RESOURCE_DESC { Width: (w / 2) as u64, Height: h / 2, ..desc };
        let mut half: Option<ID3D12Resource> = None;
        self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &half_desc, D3D12_RESOURCE_STATE_DEPTH_WRITE, Some(&clear), &mut half).unwrap();
        let half = half.unwrap();
        let dsv_heap: ID3D12DescriptorHeap = self
            .device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV, NumDescriptors: 2, ..Default::default() })
            .unwrap();
        let view = D3D12_DEPTH_STENCIL_VIEW_DESC {
            Format: view_format,
            ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
            Flags: D3D12_DSV_FLAG_NONE,
            Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 } },
        };
        let first = dsv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateDepthStencilView(&res, Some(&view), first);
        let mut second = first;
        second.ptr += self.device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_DSV) as usize;
        self.device.CreateDepthStencilView(&half, Some(&view), second);
        self.depth = Some(HostDepth {
            res,
            dsv_heap,
            _half: half,
            w,
            h,
            first: true,
            base,
            rects: Vec::new(),
            scales: vec![1.0],
            frame: 0,
            wrong_every: 0,
            wrong_value: 0.0,
        });
    }
}

fn half(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1F) as i32;
    let m = (h & 0x3FF) as f32;
    s * match e {
        0 => m / 1024.0 * 2f32.powi(-14),
        31 => f32::INFINITY,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

/// `CAVEMAN_HOST_SDK=618;E:\Games\ReSkate\x64\`: the host (and the DLL's dummy device) use that Agility SDK's
/// D3D12Core, the game's, read in place (D3D12Core.dll is Microsoft's redistributable; nothing is written there).
/// Its command lists then have vtables of their own, as in the game, and the depth hooks must find them.
fn game_sdk() -> Option<(u32, String)> {
    let v = std::env::var("CAVEMAN_HOST_SDK").ok()?;
    let (version, path) = v.split_once(';')?;
    Some((version.trim().parse().ok()?, path.trim().to_string()))
}

fn dll_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().join("caveman_fx.dll")
}

fn send(pipe: &str, req: &Request) -> Value {
    let v = proto::request(pipe, req, 3000).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    v
}

fn status(pipe: &str) -> Value {
    send(pipe, &Request::Status)
}

fn lit(p: [f32; 3]) -> bool {
    p[0] + p[1] + p[2] > 0.15
}

fn at(px: &[[f32; 3]], x: f32, y: f32) -> [f32; 3] {
    let (x, y) = ((x as u32).min(W - 1), (y as u32).min(H - 1));
    px[(y * W + x) as usize]
}

/// Where a world point lands on the host's screen, in pixels.
fn screen(cam: &Camera, p: [f32; 3]) -> (f32, f32) {
    let c = bars::project(cam, W as f32 / H as f32, p);
    ((c[0] / c[3] + 1.0) * 0.5 * W as f32, (1.0 - c[1] / c[3]) * 0.5 * H as f32)
}

unsafe fn debug_errors(device: &ID3D12Device) -> Vec<String> {
    let Ok(q) = device.cast::<ID3D12InfoQueue>() else {
        println!("(no D3D12 debug layer: install the Graphics Tools to have the API use checked)");
        return Vec::new();
    };
    println!("D3D12 debug layer on: {} message(s) stored", q.GetNumStoredMessages());
    let mut out = Vec::new();
    let (mut host_clears, mut other) = (0, 0);
    for i in 0..q.GetNumStoredMessages() {
        let mut len = 0usize;
        if q.GetMessage(i, None, &mut len).is_err() || len == 0 {
            continue;
        }
        let mut buf = vec![0u64; len / 8 + 1];
        let m = buf.as_mut_ptr() as *mut D3D12_MESSAGE;
        if q.GetMessage(i, Some(m), &mut len).is_ok() {
            let m = &*m;
            let text = String::from_utf8_lossy(std::slice::from_raw_parts(m.pDescription, m.DescriptionByteLength.saturating_sub(1))).into_owned();
            // the host's own depth clears use other values than the resource's optimized one, on purpose (the occluder)
            // (and the live-object report `live_objects` asks for is not an error)
            let host_own = text.contains("ClearDepthStencilView: The clear values do not match")
                || text.contains("Live ID3D12")
                || text.starts_with("Live ")
                || text.contains("Refcount");
            if host_own {
                host_clears += 1;
            } else if m.Severity == D3D12_MESSAGE_SEVERITY_ERROR
                || m.Severity == D3D12_MESSAGE_SEVERITY_CORRUPTION
                || m.Severity == D3D12_MESSAGE_SEVERITY_WARNING
            {
                out.push(text);
            } else {
                other += 1;
                if other <= 3 {
                    println!("  (debug layer, severity {}: {text})", m.Severity.0);
                }
            }
        }
    }
    println!("  ({host_clears} warning(s) about the host's own depth clears, {other} message(s) below warning)");
    out
}

#[test]
fn overlay_draws_through_a_hooked_present_and_unloads() {
    // CI runners have only the software adapter, which reports other copy traffic than a real GPU and fails the
    // checks below; the test runs wherever there is a GPU, and on CI when asked.
    if std::env::var_os("CI").is_some() && std::env::var_os("CAVEMAN_GPU_TESTS").is_none() {
        eprintln!("skipped: a real GPU is needed (set CAVEMAN_GPU_TESTS=1 to run it on CI anyway)");
        return;
    }
    let dll = dll_path();
    if !dll.is_file() {
        println!("skipped: {} is not built yet (cargo build -p caveman-fx, then test again)", dll.display());
        return;
    }
    unsafe { run_host(&dll) };
}

unsafe fn run_host(dll: &std::path::Path) {
    let mut host = Host::new(DXGI_FORMAT_R8G8B8A8_UNORM);
    let chain_vt = win::read_ptr(host.chain.as_raw() as usize).unwrap();
    let slot = chain_vt + 8 * 8;
    // the host's hook first, as ReSkate's and Steam's sit on the game's Present
    let dxgi_present = win::read_ptr(slot).unwrap();
    HOST_ORIGINAL.store(dxgi_present, Ordering::Release);
    let mut old = 0u32;
    assert!(win::VirtualProtect(slot as *mut _, 8, win::PAGE_READWRITE, &mut old) != 0);
    *(slot as *mut usize) = host_present as *const () as usize;
    win::VirtualProtect(slot as *mut _, 8, old, &mut old);
    host.run(50);
    assert!(CAPTURE.lock().unwrap().as_ref().unwrap().captured > 0, "the host's own hook runs");
    assert!(!host.pixels().iter().any(|&p| lit(p)), "a black frame before the DLL");

    let pipe = proto::pipe_for(false, std::process::id());
    let module = win::LoadLibraryW(win::wide(&dll.to_string_lossy()).as_ptr());
    assert!(!module.is_null());
    let t = Instant::now();
    while status_phase(&pipe) != "dormant" {
        assert!(t.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some((version, path)) = game_sdk() {
        let set_sdk: unsafe extern "C" fn(u32, *const std::ffi::c_char) -> i32 =
            std::mem::transmute(win::GetProcAddress(module, c"cmfx_test_sdk".as_ptr().cast()));
        let path = std::ffi::CString::new(path).unwrap();
        assert_eq!(set_sdk(version, path.as_ptr()), 0);
    }
    let (dll_lo, dll_hi) = win::module_range(module).unwrap();

    // the test rectangle: top left, 20 % wide, in the bars' colour
    send(&pipe, &Request::Draw { test: Some(true), queue: None, reset: None, streamline: None });
    let t = Instant::now();
    while status(&pipe)["render"]["installed"] != true {
        assert!(t.elapsed() < Duration::from_secs(10), "{}", status(&pipe)["render"]);
        host.run(20);
    }
    let r = status(&pipe)["render"].clone();
    println!("render after install: {r}");
    assert_eq!(r["dxgi_chain_vtable"], format!("{chain_vt:#x}"), "the dummy (composition) swap chain shares the game's vtable");
    let hooked = win::read_ptr(slot).unwrap();
    let (lo, hi) = win::module_range(module).unwrap();
    assert!((lo..hi).contains(&hooked), "the slot now holds the DLL's hook ({hooked:#x})");
    host.run(150);
    let px = host.pixels();
    let inside = at(&px, 0.15 * W as f32, 0.12 * H as f32);
    assert!(lit(inside) && inside[0] > 0.5 && inside[1] > 0.4 && inside[2] < 0.2, "yellow test rectangle: {inside:?}");
    assert!(!lit(at(&px, 0.5 * W as f32, 0.5 * H as f32)), "nothing in the middle");
    let r = status(&pipe)["render"].clone();
    assert!(r["frames"].as_u64().unwrap() > 3, "{r}");
    let given = host.chain.GetDevice::<ID3D12CommandQueue>().map(|q| q.as_raw() as usize);
    println!(
        "the swap chain's GetDevice(queue): {given:x?} (vtable {:x?}); the host's queue {:#x} (vtable {:x?})",
        given.as_ref().ok().and_then(|&p| win::read_ptr(p)),
        host.queue.as_raw() as usize,
        win::read_ptr(host.queue.as_raw() as usize)
    );
    assert!(r["queue"] == "the swap chain's" || r["queue"] == "found in the swap chain", "{r}");
    assert_eq!(r["output"], "sdr");

    // the staging ring, first: its heap needs the memory watch's first sample (a second after the install), and a
    // trace running puts the resource hooks in, so the ring also sees the game's releases
    let t = Instant::now();
    while status(&pipe)["render"]["video_memory"]["measured"] != true && t.elapsed() < Duration::from_secs(5) {
        host.run(100);
    }
    let v = send(&pipe, &Request::Mem { trace: Some(20.0), min_kb: Some(16), stop: None, report: None, ring: None, ring_mb: None, ring_any: None });
    assert_eq!(v["mem"]["hooks"], json!({"release": true, "map": true}), "{v}");
    // with the depth test OFF: the ring must keep the command list hooks it reads the copies through
    send(&pipe, &Request::Depth { on: Some(false), near: None, copies: None });
    staging_ring(&mut host, &pipe);
    send(&pipe, &Request::Depth { on: Some(true), near: None, copies: None });
    send(&pipe, &Request::Mem { trace: None, min_kb: None, stop: Some(true), report: None, ring: None, ring_mb: None, ring_any: None });

    // the bars over a given scene: the camera 1.5 m above the board, looking straight down; the board along +z
    send(&pipe, &Request::Draw { test: Some(false), queue: None, reset: None, streamline: None });
    let cam_m: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 10.0, 1.5, 20.0, 1.0];
    let board_m: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 0.0, 20.0, 1.0];
    let set_scene: unsafe extern "C" fn(*const f32, f32, *const f32) -> i32 =
        std::mem::transmute(win::GetProcAddress(module, c"cmfx_test_scene".as_ptr().cast()));
    assert_eq!(set_scene(cam_m.as_ptr(), 60.0, board_m.as_ptr()), 0);
    let cam = Camera::from_matrix(&cam_m, 60.0).unwrap();
    // the bars in the board's own plane (height 0): the checks below are at the board's height
    let v = send(&pipe, &Request::Bars { set: Some(json!({"enabled": true, "count": 8, "land": 3.0, "colour": "#ffd21f", "height": 0.0})), land: None });
    assert_eq!(v["bars"]["enabled"], true);
    host.run(300);
    let r = render_status_when(&mut host, &pipe, "scene", &json!("ok (test scene)"));
    assert_eq!(r["scene"], "ok (test scene)", "{r}");
    let px = host.pixels();
    let s = caveman_fx::overlay::settings(); // the defaults, for the geometry
    let bar0_z = 20.0 - 0.5 * s.span;
    let near_edge = 0.105 + 0.012;
    for side in [1.0f32, -1.0] {
        let (x, y) = screen(&cam, [10.0 + side * near_edge, 0.0, bar0_z]);
        assert!(lit(at(&px, x, y)), "bar 0 on side {side} at ({x}, {y}): {:?}", at(&px, x, y));
    }
    let (cx, cy) = screen(&cam, [10.0, 0.0, 20.0]);
    assert!(!lit(at(&px, cx, cy)), "the deck itself is not covered");
    let (fx, fy) = screen(&cam, [10.0 + 0.62, 0.0, bar0_z]);
    assert!(!lit(at(&px, fx, fy)), "nothing beyond the bars' reach before a landing: {:?}", at(&px, fx, fy));
    // a landing: they shoot out past 0.6 m, then settle back within the second
    send(&pipe, &Request::Bars { set: None, land: Some(true) });
    host.run(130);
    let px = host.pixels();
    assert!(lit(at(&px, fx, fy)), "the landing spike reaches 0.62 m: {:?}", at(&px, fx, fy));
    host.run(1400);
    let px = host.pixels();
    assert!(!lit(at(&px, fx, fy)), "settled after the spike: {:?}", at(&px, fx, fy));
    assert!(status(&pipe)["render"]["landings"].as_u64().unwrap() >= 1);

    // the other queue strategies draw too
    for mode in ["busiest", "scan", "swapchain", "auto"] {
        send(&pipe, &Request::Draw { test: Some(true), queue: Some(mode.into()), reset: None, streamline: None });
        let before = status(&pipe)["render"]["frames"].as_u64().unwrap();
        host.run(2300); // the queue choice is kept for 2 s
        let r = status(&pipe)["render"].clone();
        let drawn = r["frames"].as_u64().unwrap() - before;
        println!("queue {mode}: {drawn} frames drawn ({}), error {:?}", r["queue"], r["error"]);
        if mode == "swapchain" && r["error"] == "no queue to draw with yet" {
            // DXGI's own swap chains answer GetDevice(ID3D12CommandQueue) with E_NOINTERFACE (seen here): the scan
            // and the busiest queue are what find it
            continue;
        }
        assert!(drawn > 10, "queue {mode}: {r}");
        let px = host.pixels();
        assert!(lit(at(&px, 0.15 * W as f32, 0.12 * H as f32)), "queue {mode}");
    }

    // HDR back buffers through ResizeBuffers (the DLL holds no back buffer between frames, so it never blocks one)
    for (format, mode, check) in
        [(DXGI_FORMAT_R10G10B10A2_UNORM, "hdr10", 0.6f32), (DXGI_FORMAT_R16G16B16A16_FLOAT, "scrgb", 4.0), (DXGI_FORMAT_R10G10B10A2_UNORM, "sdr", 0.9)]
    {
        host.wait_idle();
        host.chain.ResizeBuffers(2, W, H, format, 0).unwrap_or_else(|e| panic!("ResizeBuffers to {}: {e}", format.0));
        host.format = format;
        host.set_capture();
        send(&pipe, &Request::Bars { set: Some(json!({"hdr": mode})), land: None });
        host.run(300);
        let r = render_status_when(&mut host, &pipe, "output", &json!(mode));
        assert_eq!(r["output"], mode, "{r}");
        assert_eq!(r["format"], format.0, "{r}");
        let p = at(&host.pixels(), 0.15 * W as f32, 0.12 * H as f32);
        println!("format {} as {mode}: test rectangle {p:?}", format.0);
        // the rectangle's alpha pulses between 0.5 and 1 (additive on black)
        assert!(p[0] > check * 0.5 && p[2] < p[0], "{mode}: {p:?}");
        match mode {
            // PQ at 400 nits: about 0.62 x alpha for red, and the dim blue (34 nits) lifted to about 0.42 x alpha
            "hdr10" => assert!(p[0] > 0.3 && p[0] < 0.8 && p[2] > 0.15, "{p:?}"),
            // scRGB: 400 nits is 5.0 x alpha
            "scrgb" => assert!(p[0] > 2.0 && p[0] < 5.1, "{p:?}"),
            // SDR values written as they are: blue stays near 0.12 x alpha
            _ => assert!(p[0] > 0.45 && p[2] < 0.15, "{p:?}"),
        }
    }
    host.wait_idle();
    host.chain.ResizeBuffers(2, W, H, DXGI_FORMAT_R8G8B8A8_UNORM, 0).unwrap();
    host.format = DXGI_FORMAT_R8G8B8A8_UNORM;
    host.set_capture();
    send(&pipe, &Request::Bars { set: Some(json!({"hdr": "auto"})), land: None });

    solid_and_depth(&mut host, &pipe, &cam);
    ring_layout(&mut host, &pipe, &cam);
    rim_layout(&mut host, &pipe, set_scene, &cam_m, &board_m);
    board_cam(&mut host, &pipe);

    // a fake Streamline wrapper over the host's swap chain (the game presents through Streamline's): its Present is
    // hooked like the real wrapper's; frames are drawn there (via streamline, the queue found through the wrapper's
    // base interface), once each: the DXGI hook below stays quiet while the wrapper's draws
    send(&pipe, &Request::Draw { test: Some(true), queue: Some("auto".into()), reset: None, streamline: None });
    let vtbl: &'static mut [usize; 40] = Box::leak(Box::new([fw_notimpl as *const () as usize; 40]));
    vtbl[0] = fw_qi as *const () as usize;
    vtbl[1] = fw_addref as *const () as usize;
    vtbl[2] = fw_release as *const () as usize;
    vtbl[8] = fw_present as *const () as usize;
    vtbl[22] = fw_present1 as *const () as usize;
    let wrapper: &'static mut FakeWrapper = Box::leak(Box::new(FakeWrapper { vtbl: vtbl.as_ptr(), inner: host.chain.as_raw() }));
    let wrapper_ptr = wrapper as *mut FakeWrapper as *mut c_void;
    let hook_wrapper: unsafe extern "C" fn(usize) -> i32 = std::mem::transmute(win::GetProcAddress(module, c"cmfx_test_wrapper".as_ptr().cast()));
    assert_eq!(hook_wrapper(vtbl.as_ptr() as usize), 0);
    assert_ne!(vtbl[8], fw_present as *const () as usize, "the wrapper's Present slot is hooked");
    host.via_wrapper = Some(wrapper_ptr);
    host.run(300);
    let before = status(&pipe)["render"].clone();
    host.run(500);
    let r = status(&pipe)["render"].clone();
    let frames = r["frames"].as_u64().unwrap() - before["frames"].as_u64().unwrap();
    let presents = r["presents"].as_u64().unwrap() - before["presents"].as_u64().unwrap();
    println!("through the wrapper: {frames} frames drawn for {presents} presents seen; {r}");
    assert_eq!(r["via"], "streamline", "{r}");
    assert_eq!(r["streamline"], true);
    assert_eq!(r["queue"], "found in the swap chain", "{r}");
    // the nested DXGI Present (inside the wrapper's) is not even counted: one draw per frame
    assert!(frames > 10 && frames <= presents, "one draw per frame: {frames} frames, {presents} presents");
    assert_eq!(r["queue_found_at"], "0x140", "the queue's place in DXGI's swap chain (also 0x140 in the game)");
    assert!(lit(at(&host.pixels(), 0.15 * W as f32, 0.12 * H as f32)), "drawn through the wrapper");
    // presenting the DXGI swap chain directly: quiet for 250 ms after the wrapper's last draw, then drawn there
    host.via_wrapper = None;
    let before = status(&pipe)["render"]["frames"].as_u64().unwrap();
    host.run(120);
    let quiet = status(&pipe)["render"]["frames"].as_u64().unwrap() - before;
    host.run(600);
    let after = status(&pipe)["render"]["frames"].as_u64().unwrap() - before;
    println!("direct presents after the wrapper: {quiet} drawn in the first 120 ms, {after} by 720 ms");
    assert!(quiet <= 1 && after > 10, "{quiet} {after}");
    host.via_wrapper = Some(wrapper_ptr);
    host.run(100);

    // unload: the slot goes back to the host's hook (not DXGI's), the DLL leaves, the host keeps presenting
    let errors = debug_errors(&host.device);
    send(&pipe, &Request::Unload);
    let name = win::wide("caveman_fx.dll");
    let t = Instant::now();
    while !win::GetModuleHandleW(name.as_ptr()).is_null() {
        host.run(20);
        assert!(t.elapsed() < Duration::from_secs(15), "the DLL did not unload");
    }
    assert_eq!(win::read_ptr(slot), Some(host_present as *const () as usize), "the host's hook is back in the slot");
    assert_eq!(vtbl[8], fw_present as *const () as usize, "the wrapper's Present is back");
    assert_eq!(vtbl[22], fw_present1 as *const () as usize, "the wrapper's Present1 is back");
    // no command list of the host's still calls into the DLL's old code (the lists' own tables were put back; the frames
    // just run would have crashed otherwise)
    for list in [host.list.as_raw() as usize, CAPTURE.lock().unwrap().as_ref().unwrap().list.as_raw() as usize] {
        let table = win::read_ptr(list).unwrap();
        for slot in [21usize, 26, 80] {
            let f = win::read_ptr(table + slot * 8).unwrap_or(0);
            assert!(!(dll_lo..dll_hi).contains(&f), "list {list:#x} slot {slot} still points into the unloaded DLL ({f:#x})");
        }
    }
    host.via_wrapper = None;
    host.run(100);
    assert!(!host.pixels().iter().any(|&p| lit(p)), "nothing drawn after unload");
    assert!(errors.is_empty(), "D3D12 debug layer errors: {errors:#?}");
    let errors = debug_errors(&host.device);
    assert!(errors.is_empty(), "D3D12 debug layer errors after unload: {errors:#?}");
    // put DXGI's Present back
    let mut old = 0u32;
    win::VirtualProtect(slot as *mut _, 8, win::PAGE_READWRITE, &mut old);
    *(slot as *mut usize) = dxgi_present;
    win::VirtualProtect(slot as *mut _, 8, old, &mut old);
    *CAPTURE.lock().unwrap() = None;
}

fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
}

/// Frames until the depth test is on with the main depth of `format` chosen (at most `secs`).
unsafe fn wait_for_depth(host: &mut Host, pipe: &str, format: DXGI_FORMAT, size: Option<(u32, u32)>, secs: u64) {
    let t = Instant::now();
    loop {
        host.run(100);
        let r = status(pipe)["render"].clone();
        let chosen = r["depth"]["candidates"].as_array().and_then(|c| c.iter().find(|x| x["chosen"] == true).cloned()).unwrap_or(Value::Null);
        let size_ok = size.is_none_or(|(w, h)| chosen["size"] == json!([w, h]) && r["depth"]["copies"]["size"] == json!([w, h]));
        if chosen["format"] == json!(format.0) && size_ok && r["depth_state"].as_str().unwrap_or("").starts_with("on:") && r["depth"]["copies_held_back"] == ""
        {
            println!("depth format {}: {}", format.0, r["depth_state"]);
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(secs), "depth format {}: {} / {}", format.0, r["depth_state"], r["depth"]);
    }
}

/// How many live objects the debug device reports (None without the debug layer, or when its report does not reach
/// the info queue).
unsafe fn live_objects(device: &ID3D12Device) -> Option<usize> {
    let debug = device.cast::<ID3D12DebugDevice>().ok()?;
    let q = device.cast::<ID3D12InfoQueue>().ok()?;
    let before = q.GetNumStoredMessages();
    debug.ReportLiveDeviceObjects(D3D12_RLDO_DETAIL | D3D12_RLDO_IGNORE_INTERNAL).ok()?;
    let after = q.GetNumStoredMessages();
    let mut live = 0;
    for i in before..after {
        let mut len = 0usize;
        if q.GetMessage(i, None, &mut len).is_err() || len == 0 {
            continue;
        }
        let mut buf = vec![0u64; len / 8 + 1];
        let m = buf.as_mut_ptr() as *mut D3D12_MESSAGE;
        if q.GetMessage(i, Some(m), &mut len).is_ok() {
            let m = &*m;
            let text = String::from_utf8_lossy(std::slice::from_raw_parts(m.pDescription, m.DescriptionByteLength.saturating_sub(1)));
            if text.contains("Live ID3D12") {
                live += 1;
            }
        }
    }
    (after > before).then_some(live)
}

/// A texel rect of the host's half-size depth covering the world box (x0..x1, z0..z1) at the board's height.
fn depth_rect(cam: &Camera, x0: f32, x1: f32, z0: f32, z1: f32) -> RECT {
    let (ax, ay) = screen(cam, [x0, 0.0, z0]);
    let (bx, by) = screen(cam, [x1, 0.0, z1]);
    RECT { left: (ax.min(bx) / 2.0) as i32, top: (ay.min(by) / 2.0) as i32, right: (ax.max(bx) / 2.0).ceil() as i32, bottom: (ay.max(by) / 2.0).ceil() as i32 }
}

/// The solid style (crisp, opaque, square ends; the glow style still added), then the depth test: the host's own
/// depth pass (reversed Z, near 0.1 m, half the back buffer's size, as an upscaler's) with an occluder over part of a
/// bar; the overlay copies it from the host's command list, calibrates on the deck, and hides the bar there.
unsafe fn solid_and_depth(host: &mut Host, pipe: &str, cam: &Camera) {
    send(pipe, &Request::Draw { test: Some(false), queue: None, reset: None, streamline: None });
    // fixed bars: no levels, no spike, each exactly `base` long; 4 a side (0.133 m apart), 6 cm wide
    send(
        pipe,
        &Request::Bars {
            set: Some(json!({"style": "solid", "count": 4, "gain": 0.0, "land": 0.0, "base": 0.2, "width": 0.06, "colour": "#ff0000", "outline": 0.0})),
            land: None,
        },
    );
    let (r, len, edge, bar_z) = (0.03f32, 0.2f32, 0.105f32, 19.8f32);
    // bar 0 on the +x side: u along it from the deck's edge, v across it (along z)
    let world = |side: f32, u: f32, v: f32| [10.0 + side * (edge + u), 0.0, bar_z + v];
    let grey = [64.0 / 255.0; 3];
    let red = [1.0, 0.0, 0.0];
    host.clear = [0.25, 0.25, 0.25, 1.0];
    host.run(250);
    let px = host.pixels();
    let pix = |p: [f32; 3]| {
        let (x, y) = screen(cam, p);
        at(&px, x, y)
    };
    assert!(close(pix(world(1.0, 0.1, 0.0)), red, 0.02), "opaque: the bar's own colour, none of the grey: {:?}", pix(world(1.0, 0.1, 0.0)));
    for v in [r - 0.008, -(r - 0.008)] {
        assert!(
            close(pix(world(1.0, len - 0.008, v)), red, 0.02),
            "square end: full colour just inside the far corner {v}: {:?}",
            pix(world(1.0, len - 0.008, v))
        );
    }
    assert!(
        close(pix(world(1.0, len + 0.015, 0.0)), grey, 0.01),
        "square end: nothing past it (a capsule would reach): {:?}",
        pix(world(1.0, len + 0.015, 0.0))
    );
    assert!(close(pix(world(1.0, 0.1, r + 0.015)), grey, 0.01), "nothing beside it (no glow): {:?}", pix(world(1.0, 0.1, r + 0.015)));
    // crisp: across the bar, only the two edge pixels may be partly covered
    let (x, y0) = screen(cam, world(1.0, 0.1, -0.06));
    let (_, y1) = screen(cam, world(1.0, 0.1, 0.06));
    let (mut solid, mut mixed) = (0, 0);
    for y in (y0.min(y1) as u32)..=(y0.max(y1) as u32) {
        let p = at(&px, x, y as f32);
        if close(p, red, 0.02) {
            solid += 1;
        } else if !close(p, grey, 0.01) {
            mixed += 1;
        }
    }
    println!("solid bar across: {solid} full pixels, {mixed} edge pixels");
    assert!(solid >= 6 && mixed <= 2, "crisp edges: {solid} full, {mixed} partly covered");
    // the glow style is still there, and still added: the grey shows through, a halo beside the bar
    send(pipe, &Request::Bars { set: Some(json!({"style": "glow", "glow": 0.03})), land: None });
    host.run(150);
    let px = host.pixels();
    let (gx, gy) = screen(cam, world(1.0, 0.1, 0.0));
    let (hx, hy) = screen(cam, world(1.0, 0.1, r + 0.015));
    println!("glow style: inside {:?}, beside {:?}", at(&px, gx, gy), at(&px, hx, hy));
    assert!(at(&px, gx, gy)[1] > 0.3, "added: {:?}", at(&px, gx, gy));
    assert!(at(&px, hx, hy)[0] > grey[0] + 0.05, "a glow beside the bar: {:?}", at(&px, hx, hy));
    send(pipe, &Request::Bars { set: Some(json!({"style": "solid"})), land: None });

    // opaque in HDR10 too: the same stored value over two different backgrounds
    host.wait_idle();
    host.chain.ResizeBuffers(2, W, H, DXGI_FORMAT_R10G10B10A2_UNORM, 0).unwrap();
    host.format = DXGI_FORMAT_R10G10B10A2_UNORM;
    host.set_capture();
    send(pipe, &Request::Bars { set: Some(json!({"hdr": "hdr10"})), land: None });
    let mut seen = Vec::new();
    for g in [0.25f32, 0.6] {
        host.clear = [g, g, g, 1.0];
        host.run(150);
        let (x, y) = screen(cam, world(1.0, 0.1, 0.0));
        seen.push(at(&host.pixels(), x, y));
    }
    println!("solid in hdr10 over two greys: {seen:?}");
    assert!(close(seen[0], seen[1], 1.5 / 1023.0) && seen[0][0] > 0.4, "{seen:?}");
    host.wait_idle();
    host.chain.ResizeBuffers(2, W, H, DXGI_FORMAT_R8G8B8A8_UNORM, 0).unwrap();
    host.format = DXGI_FORMAT_R8G8B8A8_UNORM;
    host.set_capture();
    send(pipe, &Request::Bars { set: Some(json!({"hdr": "auto"})), land: None });
    host.clear = [0.25, 0.25, 0.25, 1.0];

    // the depth test: the game's projection (reversed, near 0.06 m, stored 0.06 / z - 0.000005); the ground 1 m under
    // the board (2.5 m from the camera), the deck at the board (1.5 m: the deck check's points), and an occluder 30 cm
    // above the outer part of bar 0 on the +x side (1.2 m). The depth is half the back buffer's size (an upscaler's
    // render size), and the frames render into a viewport of it that changes every frame (dynamic resolution: 100, 80
    // and 90 %), with a half-resolution pass of another depth before the barrier
    let n = caveman_fx::depth::GAME_NEAR;
    let stored = |z: f32| n / z - caveman_fx::depth::GAME_FAR_TERM;
    host.enable_depth(stored(2.5), DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_D32_FLOAT);
    let deck = depth_rect(cam, 9.9, 10.1, 19.75, 20.25);
    let occluder = depth_rect(cam, 10.0 + edge + 0.1, 10.0 + edge + 0.25, bar_z - 0.06, bar_z + 0.06);
    host.depth.as_mut().unwrap().rects = vec![(deck, stored(1.5)), (occluder, stored(1.2))];
    host.depth.as_mut().unwrap().scales = vec![1.0, 0.8, 0.9];
    let t = Instant::now();
    let mut r_status = status(pipe)["render"].clone();
    while !r_status["depth_state"].as_str().unwrap_or("").starts_with("on:") {
        assert!(t.elapsed() < Duration::from_secs(10), "the depth test did not come on: {}", r_status["depth_state"]);
        host.run(100);
        r_status = status(pipe)["render"].clone();
    }
    println!("depth: {} ({})", r_status["depth_state"], r_status["depth"]);
    assert!(r_status["depth_state"].as_str().unwrap().starts_with("on: reversed depth, near 0.0600 m (the game's projection)"), "{}", r_status["depth_state"]);
    let d = &r_status["depth"];
    if game_sdk().is_some() {
        // the game's D3D12Core: the lists' own tables were found and hooked, the shared table put back
        println!(
            "lists' own tables: {} adopted, offset {}, {} slots patched, driver functions {}, shared table {}",
            d["lists_adopted"], d["own_table_offset"], d["slots_patched"], d["driver_functions"], d["shared_table"]
        );
        assert!(d["lists_adopted"].as_u64().unwrap() > 0 && d["own_table_offset"] != "0x0", "{d}");
        assert!(d["shared_table"].as_str().unwrap().starts_with("put back"), "{d}");
    }
    assert_eq!(d["candidates"][0]["size"], json!([W / 2, H / 2]));
    assert_eq!(d["candidates"][0]["chosen"], true);
    assert!(d["copies_submitted"].as_u64().unwrap() > 0);
    // every frame right, whatever its viewport: several captures, each a different frame of the 100 / 80 / 90 % cycle
    for k in 0..6 {
        host.run(30 + k * 7);
        let px = host.pixels();
        let pix = |p: [f32; 3]| {
            let (x, y) = screen(cam, p);
            at(&px, x, y)
        };
        assert!(close(pix(world(1.0, 0.05, 0.0)), red, 0.02), "capture {k}: the bar in front of the ground: drawn: {:?}", pix(world(1.0, 0.05, 0.0)));
        assert!(close(pix(world(1.0, 0.17, 0.0)), grey, 0.01), "capture {k}: the bar behind the occluder: hidden: {:?}", pix(world(1.0, 0.17, 0.0)));
        assert!(close(pix(world(-1.0, 0.17, 0.0)), red, 0.02), "capture {k}: the other side has no occluder: {:?}", pix(world(-1.0, 0.17, 0.0)));
    }
    // the deck check's read-back lands a few frames later and the status line refreshes every 30 frames
    let t = Instant::now();
    let mut r_status = status(pipe)["render"].clone();
    while !r_status["depth_state"].as_str().unwrap_or("").contains("(the deck says: viewport 0.06") && t.elapsed() < Duration::from_secs(4) {
        host.run(100);
        r_status = status(pipe)["render"].clone();
    }
    println!("with a dynamic resolution: {} (the frame's viewport {})", r_status["depth_state"], r_status["depth"]["scene_viewport"]);
    assert!(
        r_status["depth_state"].as_str().unwrap().contains("(the deck says: viewport 0.06"),
        "the deck check agrees through the frame's viewport: {}",
        r_status["depth_state"]
    );
    let every_frame_right = |host: &mut Host, frames: usize, what: &str| {
        for k in 0..frames {
            host.frame();
            let px = host.pixels();
            let pix = |p: [f32; 3]| {
                let (x, y) = screen(cam, p);
                at(&px, x, y)
            };
            assert!(close(pix(world(1.0, 0.05, 0.0)), red, 0.02), "{what}, frame {k}: the bar in front of the ground drawn: {:?}", pix(world(1.0, 0.05, 0.0)));
            assert!(close(pix(world(1.0, 0.17, 0.0)), grey, 0.01), "{what}, frame {k}: the bar behind the occluder hidden: {:?}", pix(world(1.0, 0.17, 0.0)));
        }
    };
    // a window whose only pass end of the main depth is another pass's (the whole depth cleared 0.5 m from the camera,
    // left in DEPTH_READ, not the G-buffer's PIXEL_SHADER_RESOURCE), every 5th frame, and a later pass end in the
    // others: those windows get no copy, and the frame uses the one before in its own camera; every frame right
    host.depth.as_mut().unwrap().wrong_every = 5;
    host.depth.as_mut().unwrap().wrong_value = stored(0.5);
    every_frame_right(host, 16, "another pass's end first");
    host.run(500); // the status is refreshed every 30 frames
    let r = status(pipe)["render"].clone();
    println!(
        "other passes' ends: G-buffer state {}, {} ends of another state, frames {}, pass ends {}",
        r["depth"]["gbuffer_state"], r["depth"]["other_state_ends"], r["depth_frames"]["frames"], r["depth"]["pass_ends_by_window"]
    );
    assert_eq!(r["depth"]["gbuffer_state"], "0x80");
    assert!(r["depth"]["other_state_ends"].as_u64().unwrap() >= 3, "{}", r["depth"]);
    assert!(
        r["depth_frames"]["frames"]["late"].as_u64().unwrap() >= 3 && r["depth_frames"]["frames"]["blinks"].as_u64().unwrap() >= 3,
        "{}",
        r["depth_frames"]
    );
    assert!(r["depth_state"].as_str().unwrap().starts_with("on:"), "{}", r["depth_state"]);
    // the game makes its main depth anew (another resource, the same shape): the new one is taken within a few
    // presents, the last good copy used meanwhile; the test is never off
    host.depth.as_mut().unwrap().wrong_every = 0;
    let old_chosen = r["depth"]["chosen"].clone();
    let swap_at = status(pipe)["render"]["presents"].as_u64().unwrap();
    let offs_since = |changes: &Value| {
        changes
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c.as_str())
            .filter(|c| c.contains(": off"))
            .filter(|c| c.trim_start_matches("present ").split(':').next().and_then(|n| n.parse::<u64>().ok()).is_some_and(|n| n >= swap_at))
            .count()
    };
    host.wait_idle();
    let old = host.depth.take().unwrap(); // kept alive: the new one is not made at its address
    host.enable_depth(stored(2.5), DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_D32_FLOAT);
    host.depth.as_mut().unwrap().rects = old.rects.clone();
    host.depth.as_mut().unwrap().scales = old.scales.clone();
    every_frame_right(host, 12, "a new main depth");
    host.run(500);
    let r = status(pipe)["render"].clone();
    println!("a new main depth: chosen {} (was {old_chosen}), changes {}", r["depth"]["chosen"], r["depth"]["changes"]);
    assert_ne!(r["depth"]["chosen"], old_chosen);
    assert!(r["depth"]["changes"].as_array().unwrap().iter().any(|c| c.as_str().unwrap_or("").contains("main depth")), "{}", r["depth"]["changes"]);
    assert_eq!(offs_since(&r["depth"]["changes"]), 0, "never off: {}", r["depth"]["changes"]);
    host.wait_idle();
    drop(old);
    // off: drawn over everything again
    send(pipe, &Request::Depth { on: Some(false), near: None, copies: None });
    host.run(100);
    let (x, y) = screen(cam, world(1.0, 0.17, 0.0));
    assert!(close(at(&host.pixels(), x, y), red, 0.02), "depth off: {:?}", at(&host.pixels(), x, y));
    // (the status is refreshed every 30 frames)
    let t = Instant::now();
    while !status(pipe)["render"]["depth_state"].as_str().unwrap().starts_with("off") {
        assert!(t.elapsed() < Duration::from_secs(5), "{}", status(pipe)["render"]["depth_state"]);
        host.run(50);
    }
    send(pipe, &Request::Depth { on: Some(true), near: None, copies: None });
    // the near plane by hand: 0.03 m makes the ground (stored 0.024) read 1.25 m away, in front of the bars (1.5 m)
    send(pipe, &Request::Depth { on: None, near: Some(0.03), copies: None });
    host.run(150);
    let (x, y) = screen(cam, world(1.0, 0.05, 0.0));
    assert!(close(at(&host.pixels(), x, y), grey, 0.01), "near set by hand is used: {:?}", at(&host.pixels(), x, y));
    send(pipe, &Request::Depth { on: None, near: Some(0.0), copies: None });
    host.run(150);
    let px = host.pixels();
    let (ax, ay) = screen(cam, world(1.0, 0.05, 0.0));
    let (bx, by) = screen(cam, world(1.0, 0.17, 0.0));
    assert!(
        close(at(&px, ax, ay), red, 0.02) && close(at(&px, bx, by), grey, 0.01),
        "the game's near plane again: {:?} {:?}",
        at(&px, ax, ay),
        at(&px, bx, by)
    );
    // no depth pass any more: the last copy is used (in its own camera) for 30 presents, then none: the bars draw
    // unhidden
    host.wait_idle();
    host.depth = None;
    let t = Instant::now();
    let mut r_status = status(pipe)["render"].clone();
    while !r_status["depth_state"].as_str().unwrap_or("").starts_with("no copy") && t.elapsed() < Duration::from_secs(5) {
        host.run(100); // the status is refreshed every 30 frames
        r_status = status(pipe)["render"].clone();
    }
    println!("without a depth pass: {}", r_status["depth_state"]);
    assert!(r_status["depth_state"].as_str().unwrap().starts_with("no copy"), "{}", r_status["depth_state"]);
    let (x, y) = screen(cam, world(1.0, 0.17, 0.0));
    assert!(close(at(&host.pixels(), x, y), red, 0.02));
    // the other depth formats a game may use: 32-bit depth with stencil (two planes: the copy takes the depth plane;
    // the same R32_FLOAT copies serve, as the game's main depth is this format: audit H1, the debug layer's verdict
    // printed) and 16-bit (R16_UNORM copies: the set made again, once, under the five-second rule)
    let sets_before = status(pipe)["render"]["depth"]["copy_sets_made"].as_u64().unwrap();
    for (typeless, view) in [(DXGI_FORMAT_R32G8X24_TYPELESS, DXGI_FORMAT_D32_FLOAT_S8X24_UINT), (DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_D16_UNORM)] {
        host.wait_idle();
        host.enable_depth(stored(2.5), typeless, view);
        host.depth.as_mut().unwrap().rects = vec![(deck, stored(1.5)), (occluder, stored(1.2))];
        wait_for_depth(host, pipe, typeless, None, 12);
        host.run(150);
        let px = host.pixels();
        let (ax, ay) = screen(cam, world(1.0, 0.05, 0.0));
        let (bx, by) = screen(cam, world(1.0, 0.17, 0.0));
        assert!(
            close(at(&px, ax, ay), red, 0.02) && close(at(&px, bx, by), grey, 0.01),
            "depth format {}: {:?} {:?}",
            typeless.0,
            at(&px, ax, ay),
            at(&px, bx, by)
        );
        if typeless == DXGI_FORMAT_R32G8X24_TYPELESS {
            let errors = debug_errors(&host.device);
            println!(
                "H1 verdict (D32_FLOAT_S8X24_UINT depth, plane 0 copied into R32_FLOAT, exact size): {}",
                if errors.is_empty() { "the debug layer stores no error or warning" } else { "ERRORS" }
            );
            assert!(errors.is_empty(), "H1: {errors:#?}");
        }
    }
    let r = status(pipe)["render"]["depth"].clone();
    assert_eq!(r["copy_sets_made"].as_u64().unwrap(), sets_before + 1, "one set made again (for R16_UNORM), none for D32S8: {r}");
    assert_eq!(r["copies"]["format"], json!(DXGI_FORMAT_R16_UNORM.0), "{r}");

    // size flapping (a pause or a menu making and dropping depths at other sizes, 2026-10-21 18:29): the main depth's
    // size changes every third present for 100 presents; the set is never made again (none of them is steady), one
    // set alive, the bytes held flat, no copy taken while the size mismatches, the debug layer quiet, and the DLL's
    // live objects the same at the end as at the start
    host.wait_idle();
    let live_before = live_objects(&host.device);
    let mb_before = r["copies_mb"].clone();
    let made_before = r["copy_sets_made"].as_u64().unwrap();
    let recorded_before = status(pipe)["render"]["depth"]["copies_recorded"].as_u64().unwrap();
    let sizes = [(W / 2, H / 2), (W / 2 - 8, H / 2 - 6), (W / 2 + 8, H / 2 + 6)];
    for k in 0..100 {
        if k % 3 == 0 {
            host.wait_idle();
            let (w, h) = sizes[(k / 3) % 3];
            host.enable_depth_sized(stored(2.5), DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_D16_UNORM, w, h);
            host.depth.as_mut().unwrap().rects = vec![(deck, stored(1.5)), (occluder, stored(1.2))];
        }
        host.frame();
        std::thread::sleep(Duration::from_millis(4));
    }
    host.wait_idle();
    let during = status(pipe)["render"]["depth"]["copies_recorded"].as_u64().unwrap() - recorded_before;
    host.run(300);
    let r = status(pipe)["render"]["depth"].clone();
    println!("size flapping: {} MB held, {} set(s) alive, {} made (was {made_before}), {during} copies recorded in the 100 presents (34 of them at the set's size), held back: {}", r["copies_mb"], r["copy_sets_alive"], r["copy_sets_made"], r["copies_held_back"]);
    assert_eq!(r["copy_sets_made"].as_u64().unwrap(), made_before, "no set made again while flapping: {r}");
    assert_eq!(r["copy_sets_alive"], 1, "{r}");
    assert_eq!(r["copies_mb"], mb_before, "{r}");
    // (the sizes cycle through the set's own; a depth at that size is copied only once it is the chosen one again)
    assert!(during <= 40, "copies only at the set's own size while flapping: {during} of 100 presents: {r}");
    println!("  changes while flapping: {}", r["changes"]);
    println!("  candidates after flapping: {}", r["candidates"]);
    let errors = debug_errors(&host.device);
    assert!(errors.is_empty(), "size flapping: {errors:#?}");
    let live_after = live_objects(&host.device);
    println!("live objects: {live_before:?} before the flapping, {live_after:?} after");
    if let (Some(a), Some(b)) = (live_before, live_after) {
        assert!(b <= a, "live objects grew while flapping: {a} -> {b}");
    }
    // steady at one of the sizes again: the set made again for it (within the five-second rule), the test on again
    host.wait_idle();
    host.enable_depth_sized(stored(2.5), DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_D16_UNORM, W / 2 + 8, H / 2 + 6);
    host.depth.as_mut().unwrap().rects = vec![(deck, stored(1.5)), (occluder, stored(1.2))];
    wait_for_depth(host, pipe, DXGI_FORMAT_R16_TYPELESS, Some((W / 2 + 8, H / 2 + 6)), 12);
    let r = status(pipe)["render"]["depth"].clone();
    assert_eq!(r["copies"]["size"], json!([W / 2 + 8, H / 2 + 6]), "{r}");
    assert_eq!(r["copy_sets_made"].as_u64().unwrap(), made_before + 1);

    // C2: a list recorded with a copy and held unsubmitted while the set is replaced: the set stays alive until the
    // list is submitted; another one reset instead of submitted: its copy is forgotten and the set freed
    let held_allocator: ID3D12CommandAllocator = host.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
    let held: ID3D12GraphicsCommandList =
        host.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &held_allocator, None::<&ID3D12PipelineState>).unwrap();
    held.Close().unwrap();
    let record_pass_end = |list: &ID3D12GraphicsCommandList, allocator: &ID3D12CommandAllocator, depth: &ID3D12Resource| {
        allocator.Reset().unwrap();
        list.Reset(allocator, None).unwrap();
        list.ResourceBarrier(&[barrier(depth, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_DEPTH_WRITE)]);
        list.ResourceBarrier(&[barrier(depth, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
        list.Close().unwrap();
    };
    let pending = |pipe: &str| status(pipe)["render"]["depth"]["pending"].as_array().map_or(0, Vec::len);
    let depth_res = host.depth.as_ref().unwrap().res.clone();
    record_pass_end(&held, &held_allocator, &depth_res);
    if pending(pipe) == 0 {
        println!(
            "C2: a never-submitted list carries no hook under this D3D12 (its own table is patched at its first submission): the held-list check is skipped"
        );
    } else {
        let gen_before = status(pipe)["render"]["depth"]["copies"]["generation"].clone();
        // the set replaced (another size, steady): the old one must stay alive for the held list
        host.wait_idle();
        host.enable_depth_sized(stored(2.5), DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_D16_UNORM, W / 2, H / 2);
        host.depth.as_mut().unwrap().rects = vec![(deck, stored(1.5)), (occluder, stored(1.2))];
        wait_for_depth(host, pipe, DXGI_FORMAT_R16_TYPELESS, Some((W / 2, H / 2)), 12);
        host.run(400);
        let r = status(pipe)["render"]["depth"].clone();
        assert_ne!(r["copies"]["generation"], gen_before, "{r}");
        assert_eq!(r["copy_sets_alive"], 2, "the replaced set stays alive while a list recorded with a copy into it is unsubmitted: {r}");
        assert_eq!(pending(pipe), 1, "{r}");
        // submitted at last (20 presents later): the copy runs on the old depth (kept alive by the host for it) and
        // the old set is freed after
        host.run(200);
        host.queue.ExecuteCommandLists(&[Some(held.cast().unwrap())]);
        let t = Instant::now();
        while status(pipe)["render"]["depth"]["copy_sets_alive"] != json!(1) {
            assert!(t.elapsed() < Duration::from_secs(5), "the replaced set was not freed after the held list ran: {}", status(pipe)["render"]["depth"]);
            host.run(50);
        }
        assert_eq!(pending(pipe), 0);
        let errors = debug_errors(&host.device);
        assert!(errors.is_empty(), "C2 (held list submitted late): {errors:#?}");
        println!("C2: the replaced set lived until the held list ran, then was freed");
        // the same, reset instead of submitted
        let depth_res2 = host.depth.as_ref().unwrap().res.clone();
        record_pass_end(&held, &held_allocator, &depth_res2);
        assert_eq!(pending(pipe), 1);
        host.wait_idle();
        host.enable_depth_sized(stored(2.5), DXGI_FORMAT_R16_TYPELESS, DXGI_FORMAT_D16_UNORM, W / 2 + 8, H / 2 + 6);
        host.depth.as_mut().unwrap().rects = vec![(deck, stored(1.5)), (occluder, stored(1.2))];
        wait_for_depth(host, pipe, DXGI_FORMAT_R16_TYPELESS, Some((W / 2 + 8, H / 2 + 6)), 12);
        host.run(400);
        assert_eq!(status(pipe)["render"]["depth"]["copy_sets_alive"], 2);
        held_allocator.Reset().unwrap();
        held.Reset(&held_allocator, None).unwrap(); // the recording discarded: the DLL drops its pending copy
        held.Close().unwrap();
        let t = Instant::now();
        while status(pipe)["render"]["depth"]["copy_sets_alive"] != json!(1) {
            assert!(t.elapsed() < Duration::from_secs(5), "the replaced set was not freed after the held list was reset: {}", status(pipe)["render"]["depth"]);
            host.run(50);
        }
        let r = status(pipe)["render"]["depth"].clone();
        assert_eq!(pending(pipe), 0);
        assert!(r["pending_dropped_at_reset"].as_u64().unwrap() >= 1, "{r}");
        println!("C2: the pending copy of a reset list was dropped and the replaced set freed");
    }
    drop(held);
    drop(held_allocator);

    // C3: a copy submitted on another queue than the one the overlay draws on is never read, only counted
    let other_queue: ID3D12CommandQueue =
        host.device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
    let oq_allocator: ID3D12CommandAllocator = host.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
    let oq_list: ID3D12GraphicsCommandList =
        host.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &oq_allocator, None::<&ID3D12PipelineState>).unwrap();
    oq_list.Close().unwrap();
    let other_before = status(pipe)["render"]["depth"]["copies_other_queue"].as_u64().unwrap();
    for _ in 0..3 {
        host.wait_idle();
        let depth_res = host.depth.as_ref().unwrap().res.clone();
        record_pass_end(&oq_list, &oq_allocator, &depth_res);
        other_queue.ExecuteCommandLists(&[Some(oq_list.cast().unwrap())]);
        let fence: ID3D12Fence = host.device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        other_queue.Signal(&fence, 1).unwrap();
        wait(&fence, 1, host.event);
        host.run(60);
    }
    let r = status(pipe)["render"]["depth"].clone();
    println!(
        "C3: {} copies submitted on another queue (never read); the test {}",
        r["copies_other_queue"].as_u64().unwrap() - other_before,
        status(pipe)["render"]["depth_state"]
    );
    if r["copies_other_queue"].as_u64().unwrap() > other_before {
        assert!(status(pipe)["render"]["depth_state"].as_str().unwrap().starts_with("on:"));
    }
    let errors = debug_errors(&host.device);
    assert!(errors.is_empty(), "C3: {errors:#?}");
    host.wait_idle();
    drop(oq_list);
    drop(oq_allocator);
    drop(other_queue);
    // no depth pass from here: the last copy serves for 30 presents (late), then the test is off; the checks after
    // this one expect it off
    host.depth = None;
    let t = Instant::now();
    while !status(pipe)["render"]["depth_state"].as_str().unwrap_or("").starts_with("no copy") {
        assert!(t.elapsed() < Duration::from_secs(8), "{}", status(pipe)["render"]["depth_state"]);
        host.run(100);
    }
    host.wait_idle();
    host.depth = None;
    // back to the defaults the rest of the test expects
    host.clear = [0.0, 0.0, 0.0, 1.0];
    send(pipe, &Request::Bars { set: Some(json!({"count": 8, "gain": 1.0, "land": 1.5, "base": 0.025, "width": 0.032, "colour": "#ffd21f"})), land: None });
}

/// The ring layout: bars all the way round the deck's outline, each on the outline's outward normal.
unsafe fn ring_layout(host: &mut Host, pipe: &str, cam: &Camera) {
    let n = 64usize;
    send(
        pipe,
        &Request::Bars {
            set: Some(json!({"layout": "ring", "count": n, "gain": 0.0, "land": 0.0, "base": 0.1, "width": 0.02, "height": 0.0, "colour": "#00e5ff"})),
            land: None,
        },
    );
    host.clear = [0.25, 0.25, 0.25, 1.0];
    host.run(250);
    let px = host.pixels();
    let s = caveman_fx::bars::BarSettings::default();
    let p = caveman_fx::bars::outline_length(s.deck_width, s.deck_length, s.deck_end);
    let cyan = [0.0, 229.0 / 255.0, 1.0];
    let (mut on, mut off) = (0, 0);
    for k in 0..n {
        let (x, y, nx, ny) = caveman_fx::bars::outline_point(s.deck_width, s.deck_length, s.deck_end, (k as f32 + 0.5) * p / n as f32);
        // the board: right = +x (across), forward = +z (along)
        let at_u = |u: f32| {
            let (sx, sy) = screen(cam, [10.0 + x + nx * u, 0.0, 20.0 + y + ny * u]);
            at(&px, sx, sy)
        };
        if close(at_u(0.05), cyan, 0.02) {
            on += 1;
        }
        // past the bar's end: the background
        if close(at_u(0.13), [64.0 / 255.0; 3], 0.01) {
            off += 1;
        }
    }
    println!("ring: {on} of {n} bars drawn on their outline normals, {off} clear past their ends");
    assert!(on == n && off == n, "{on} {off}");
    // nothing on the deck itself
    let (cx, cy) = screen(cam, [10.0, 0.0, 20.0]);
    assert!(close(at(&px, cx, cy), [64.0 / 255.0; 3], 0.01));
    host.clear = [0.0, 0.0, 0.0, 1.0];
    send(
        pipe,
        &Request::Bars {
            set: Some(json!({"layout": "sides", "count": 8, "gain": 1.0, "land": 1.5, "base": 0.025, "width": 0.032, "colour": "#ffd21f"})),
            land: None,
        },
    );
}

/// The rim layout: a made-up kicked rim sent over the pipe (30 degree kicks); seen from the side, the bars at the
/// nose and tail stand on the raised ends and point out and up along them, not level. Cleared, the layout draws the
/// ring (flat, at `height`).
unsafe fn rim_layout(host: &mut Host, pipe: &str, set_scene: unsafe extern "C" fn(*const f32, f32, *const f32) -> i32, cam_m: &[f32; 16], board_m: &[f32; 16]) {
    let path = caveman_fx::rim::synthetic(256, 0.21, 0.81, 0.0975, 30.0, 0.15);
    let v = send(pipe, &Request::Rim { path: Some(path.clone()), clear: None });
    assert_eq!((v["rim"]["loaded"].clone(), v["rim"]["points"].clone()), (json!(true), json!(256)), "{v}");
    assert_eq!(status(pipe)["rim"]["points"], 256);
    let n = 16usize;
    let v = send(
        pipe,
        &Request::Bars {
            set: Some(
                json!({"layout": "rim", "count": n, "gain": 0.0, "land": 0.0, "base": 0.12, "width": 0.02, "height": 0.0, "facing": "camera", "colour": "#00e5ff"}),
            ),
            land: None,
        },
    );
    let s: caveman_fx::bars::BarSettings = serde_json::from_value(v["bars"].clone()).unwrap();
    // the camera 0.9 m to the board's right at the deck's height, looking across it (-x): the nose (+z) on the left
    let side_m: [f32; 16] = [0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 10.9, 0.15, 20.0, 1.0];
    assert_eq!(set_scene(side_m.as_ptr(), 60.0, board_m.as_ptr()), 0);
    let side = Camera::from_matrix(&side_m, 60.0).unwrap();
    host.clear = [0.25, 0.25, 0.25, 1.0];
    host.run(250);
    let px = host.pixels();
    let cyan = [0.0, 229.0 / 255.0, 1.0];
    let grey = [64.0 / 255.0; 3];
    let pix = |p: [f32; 3]| {
        let (x, y) = screen(&side, p);
        at(&px, x, y)
    };
    let board = bars::Board::from_matrix(board_m).unwrap();
    let rim = caveman_fx::rim::Rim::new(path).unwrap();
    let places = bars::rim_bars(&s, &board, &rim, n);
    for k in [0, n / 2] {
        let (b, o, _) = places[k];
        let level = bars::normalize([o[0], 0.0, o[2]]).unwrap();
        let along = |d: [f32; 3], u: f32| bars::add(b, bars::mul(d, u));
        println!("rim bar {k}: base {b:?}, out {o:?}; along it {:?}, level from its base {:?}", pix(along(o, 0.06)), pix(along(level, 0.06)));
        assert!(o[1] > 0.3, "bar {k} points up the kick: {o:?}");
        assert!(close(pix(along(o, 0.03)), cyan, 0.02) && close(pix(along(o, 0.09)), cyan, 0.02), "bar {k} stands on the rim, along its way out");
        assert!(close(pix(along(level, 0.09)), grey, 0.01), "bar {k} is tilted, not level: {:?}", pix(along(level, 0.09)));
        assert!(close(pix(along(o, 0.16)), grey, 0.01), "nothing past bar {k}'s end");
    }
    // cleared: the ring again (level, at height 0)
    let v = send(pipe, &Request::Rim { path: None, clear: Some(true) });
    assert_eq!(v["rim"]["loaded"], false);
    host.run(150);
    let px = host.pixels();
    let pix = |p: [f32; 3]| {
        let (x, y) = screen(&side, p);
        at(&px, x, y)
    };
    let per = bars::outline_length(s.deck_width, s.deck_length, s.deck_end);
    let (x, z, nx, nz) = bars::outline_point(s.deck_width, s.deck_length, s.deck_end, 0.5 * per / n as f32);
    assert!(close(pix([10.0 + x + nx * 0.06, 0.0, 20.0 + z + nz * 0.06]), cyan, 0.02), "the ring's bar 0 without a rim");
    let (b, o, _) = places[0];
    assert!(close(pix(bars::add(b, bars::mul(o, 0.09))), grey, 0.01), "no rim bar without a rim");
    println!("rim: the nose and tail bars stand on the kicks and tilt up with them; cleared, the ring");
    // back to the camera and the settings the rest of the test expects
    assert_eq!(set_scene(cam_m.as_ptr(), 60.0, board_m.as_ptr()), 0);
    host.clear = [0.0, 0.0, 0.0, 1.0];
    send(
        pipe,
        &Request::Bars {
            set: Some(json!({"layout": "sides", "facing": "board", "count": 8, "gain": 1.0, "land": 1.5, "base": 0.025, "width": 0.032, "colour": "#ffd21f"})),
            land: None,
        },
    );
}

/// A flat slab in the deck's place (x across, y up, z along): six faces, each with its outward normal, as the board
/// cam's deck.
fn slab() -> PipMesh {
    let (lo, hi) = ([-0.108f32, 0.08, -0.4], [0.108f32, 0.10, 0.4]);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut n = [0.0f32; 3];
            n[axis] = sign;
            let base = (vertices.len() / 6) as u32;
            for (ua, ub) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
                let mut p = [0.0f32; 3];
                p[axis] = if sign > 0.0 { hi[axis] } else { lo[axis] };
                p[a] = if ua == 1 { hi[a] } else { lo[a] };
                p[b] = if ub == 1 { hi[b] } else { lo[b] };
                vertices.extend_from_slice(&[p[0], p[1], p[2], n[0], n[1], n[2]]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    PipMesh { label: "slab".into(), vertices, indices, parts: vec![PipPart { kind: "deck".into(), first_index: 0, index_count: 36 }], bounds: [lo, hi] }
}

/// A 512 x 2048 sRGB texture in the deck art's shape (the nose third green, the middle red, the tail third blue),
/// uploaded and left in the pixel shader resource state.
unsafe fn art_texture(host: &mut Host) -> ID3D12Resource {
    let (w, h) = (512u32, 2048u32);
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: w as u64,
        Height: h,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        ..Default::default()
    };
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let mut tex: Option<ID3D12Resource> = None;
    host.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut tex).unwrap();
    let tex = tex.unwrap();
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0u64;
    host.device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
    let up_heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
    let up_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut up: Option<ID3D12Resource> = None;
    host.device.CreateCommittedResource(&up_heap, D3D12_HEAP_FLAG_NONE, &up_desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut up).unwrap();
    let up = up.unwrap();
    let mut p: *mut c_void = std::ptr::null_mut();
    up.Map(0, None, Some(&mut p)).unwrap();
    let pitch = footprint.Footprint.RowPitch as usize;
    for y in 0..h as usize {
        let row = (p as *mut u8).add(footprint.Offset as usize + y * pitch);
        let c: [u8; 4] = if y < 683 {
            [0, 255, 0, 255]
        } else if y < 1366 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        };
        for x in 0..w as usize {
            std::ptr::copy_nonoverlapping(c.as_ptr(), row.add(x * 4), 4);
        }
    }
    up.Unmap(0, None);
    host.wait_idle();
    host.allocator.Reset().unwrap();
    host.list.Reset(&host.allocator, None).unwrap();
    let dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: std::mem::transmute_copy(&tex),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    };
    let src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: std::mem::transmute_copy(&up),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
    };
    host.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
    host.list.ResourceBarrier(&[barrier(&tex, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)]);
    host.list.Close().unwrap();
    host.queue.ExecuteCommandLists(&[Some(host.list.cast().unwrap())]);
    host.wait_idle();
    drop(up);
    tex
}

/// The board cam: a slab for the deck with a test texture handed to the cam for its underside, drawn in a square at
/// the top left. From below the art shows (its three bands), from above the grip side is plain grey, and with the
/// art off the underside is too.
unsafe fn board_cam(host: &mut Host, pipe: &str) {
    // the allocation trace sees the host's own allocations below: the texture (kept) and its upload buffer (released
    // after the copy, mapped and fully written, so its lifetime and touched pages are measured)
    let v = send(pipe, &Request::Mem { trace: Some(8.0), min_kb: Some(16), stop: None, report: None, ring: None, ring_mb: None, ring_any: None });
    assert_eq!(v["mem"]["tracing"], true, "{v}");
    assert_eq!(v["mem"]["hooks"], json!({"release": true, "map": true}), "{v}");
    let tex = art_texture(host);
    host.run(200);
    let v = send(pipe, &Request::Mem { trace: None, min_kb: None, stop: None, report: Some(true), ring: None, ring_mb: None, ring_any: None });
    let m = &v["mem"];
    println!(
        "allocation trace in the host: {} allocation(s), {:.1} MB, {} held; sites {}",
        m["allocations"],
        m["total_mb"].as_f64().unwrap_or(0.0),
        m["held_at_report"],
        m["sites"]
    );
    assert!(m["allocations"].as_u64().unwrap_or(0) >= 2, "the texture and its upload buffer: {m}");
    let sites = m["sites"].as_array().cloned().unwrap_or_default();
    let freed: u64 = sites.iter().map(|s| s["freed"].as_u64().unwrap_or(0)).sum();
    assert!(freed >= 1, "the upload buffer was released and seen going: {m}");
    let upload = sites.iter().find(|s| s["heaps_mb"]["upload (system memory)"].as_f64().unwrap_or(0.0) > 3.0).expect("the 4 MB upload buffer's site");
    assert!(upload["mean_life_ms"].as_f64().unwrap_or(-1.0) >= 0.0, "its lifetime was measured: {upload}");
    assert!(upload["touched_frac"].as_f64().unwrap_or(-1.0) > 0.9, "its pages were all written (mapped, then measured at release): {upload}");
    assert!(m["file"].as_str().is_some_and(|p| std::path::Path::new(p).is_file()), "the raw file was written: {m}");
    let v = send(pipe, &Request::Pip { mesh: Some(slab()), clear: None, art: Some(format!("{:#x}", tex.as_raw() as usize)) });
    assert_eq!(v["pip"]["loaded"], true, "{v}");
    let set = |host: &mut Host, elevation: f32, art: bool, ms: u64| {
        let v = send(
            pipe,
            &Request::Bars {
                set: Some(
                    json!({"pip": true, "pip_size": 0.5, "pip_margin": 0.02, "pip_corner": "tl", "pip_view": "world", "pip_elevation": elevation, "pip_azimuth": 0.0, "pip_fov": 30.0, "pip_zoom": 1.0, "pip_art": art, "pip_backdrop": 0.55}),
                ),
                land: None,
            },
        );
        assert_eq!(v["bars"]["pip"], true, "{v}");
        host.run(ms);
    };
    // the square: 120 px a side, 5 px from the corner
    let count = |px: &[[f32; 3]]| {
        let (mut r, mut g, mut b, mut grey) = (0, 0, 0, 0);
        for y in 5..125 {
            for x in 5..125 {
                let p = at(px, x as f32, y as f32);
                let hi = |k: usize| p[k] > 0.4 && p[(k + 1) % 3] < 0.2 && p[(k + 2) % 3] < 0.2;
                if hi(0) {
                    r += 1;
                } else if hi(1) {
                    g += 1;
                } else if hi(2) {
                    b += 1;
                } else if p[0] > 0.15 && (p[0] - p[1]).abs() < 0.05 && (p[1] - p[2]).abs() < 0.06 {
                    grey += 1;
                }
            }
        }
        (r, g, b, grey)
    };
    set(host, -60.0, true, 900);
    let below = count(&host.pixels());
    let st = send(pipe, &Request::Pip { mesh: None, clear: None, art: None });
    println!("board cam from below: red {}, green {}, blue {}, grey {} pixels; {}", below.0, below.1, below.2, below.3, st["pip"]);
    assert!(st["pip"]["frames_drawn"].as_u64().unwrap_or(0) > 5 && st["pip"]["art_bound"] == true, "{}", st["pip"]);
    assert!(below.0 > 100 && below.1 > 30 && below.2 > 30, "the art's three bands on the underside from below: {below:?}");
    set(host, 60.0, true, 400);
    let above = count(&host.pixels());
    println!("board cam from above: red {}, green {}, blue {}, grey {} pixels", above.0, above.1, above.2, above.3);
    assert!(above.0 + above.1 + above.2 < 10 && above.3 > 300, "the grip side from above is plain grey: {above:?}");
    set(host, -60.0, false, 400);
    let off = count(&host.pixels());
    println!("board cam from below, art off: red {}, green {}, blue {}, grey {} pixels", off.0, off.1, off.2, off.3);
    assert!(off.0 + off.1 + off.2 < 10 && off.3 > 300, "art off: plain grey: {off:?}");
    send(pipe, &Request::Bars { set: Some(json!({"pip": false})), land: None });
    send(pipe, &Request::Pip { mesh: None, clear: Some(true), art: Some("0".into()) });
    host.run(150);
    // the texture stays alive for the frames in flight that read it (the DLL holds its own reference until unload)
    std::mem::forget(tex);
}

/// The staging ring (stagering.rs), with any call site taken since a test host has no Skate.exe: eight 16 MB upload
/// buffers asked of the device as the game asks, in a ring of four slots; each is filled with its own byte, copied
/// into a texture of its own on a list of its own, and released. The fifth waits for the first copy's fence and takes
/// its slot; every texture ends up holding its own byte. A second copy out of one buffer, on another list, switches
/// the ring off.
unsafe fn staging_ring(host: &mut Host, pipe: &str) {
    const N: usize = 8;
    const BYTES: u64 = 16 << 20;
    let none = || Request::Mem { trace: None, min_kb: None, stop: None, report: None, ring: None, ring_mb: None, ring_any: None };
    // a 64 MB ring: four 16 MB buffers fill it, the fifth needs the first's region back
    let v = send(pipe, &Request::Mem { trace: None, min_kb: None, stop: None, report: None, ring: Some(true), ring_mb: Some(64), ring_any: Some(true) });
    assert_eq!(v["ring"]["enabled"], true, "{v}");
    assert_eq!(v["copy_hooks"], true, "the copy hooks are in for the ring: {v}");
    // the heap is made on the worker thread (never in a hook): wait for it
    let mut v = v;
    for _ in 0..40 {
        if v["ring"]["heap_mb"].as_u64().unwrap_or(0) > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        v = send(pipe, &none());
    }
    assert_eq!(v["ring"]["heap_mb"], 64, "the ring's heap was made by the worker: {v}");
    let device = host.device.clone();
    let queue = host.queue.clone();
    let tex_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: 2048,
        Height: 2048,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        ..Default::default()
    };
    let default_heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let upload_heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
    let buf_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: BYTES,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
        Offset: 0,
        Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_R8G8B8A8_UNORM, Width: 2048, Height: 2048, Depth: 1, RowPitch: 8192 },
    };
    let mut textures: Vec<ID3D12Resource> = Vec::new();
    let mut lists: Vec<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)> = Vec::new();
    for _ in 0..N {
        let mut tex: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&default_heap, D3D12_HEAP_FLAG_NONE, &tex_desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut tex).unwrap();
        textures.push(tex.unwrap());
        let a: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
        let l: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &a, None::<&ID3D12PipelineState>).unwrap();
        l.Close().unwrap();
        lists.push((a, l));
    }
    let make_buffer = |fill: u8| -> ID3D12Resource {
        let mut buf: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&upload_heap, D3D12_HEAP_FLAG_NONE, &buf_desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buf).unwrap();
        let buf = buf.unwrap();
        let mut p: *mut c_void = std::ptr::null_mut();
        buf.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut p)).unwrap();
        std::ptr::write_bytes(p as *mut u8, fill, BYTES as usize);
        buf.Unmap(0, None);
        buf
    };
    let copy = |l: &ID3D12GraphicsCommandList, buf: &ID3D12Resource, tex: &ID3D12Resource| {
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(tex),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(buf),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
        };
        l.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
    };
    // as the game does it: every buffer stays held until long after its copy (the slots are recycled under them)
    let t0 = Instant::now();
    let mut held = Vec::new();
    for i in 0..N {
        let buf = make_buffer((i + 1) as u8);
        let (a, l) = &lists[i];
        a.Reset().unwrap();
        l.Reset(a, None).unwrap();
        copy(l, &buf, &textures[i]);
        l.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(l.cast().unwrap())]);
        // the ring never waits for a slot (r6r), so each copy runs before the next buffer is asked for, as the GPU
        // would be ahead of a job thread that fills 30 MB between requests
        host.wait_idle();
        held.push(buf);
    }
    host.wait_idle();
    let st = send(pipe, &none())["ring"].clone();
    println!("staging ring in the host: {N} buffers of 16 MB in {} ms; {st}", t0.elapsed().as_millis());
    assert_eq!(st["handed"].as_u64(), Some(N as u64), "every buffer came from the ring: {st}");
    assert!(st["recycled"].as_u64().unwrap_or(0) >= (N - 4) as u64, "the slots were recycled once their copies ran: {st}");
    assert_eq!(st["fallback_full"], 0, "{st}");
    assert_eq!(st["rereads"], 0, "{st}");
    // "live" counts the buffers the ring still tracks: regions retire from the oldest as soon as their copies have
    // run (every earlier buffer's copy ran before the next was asked for), so only the last buffer's region is left,
    // while the game still holds all eight resources
    assert_eq!(st["live"], 1, "only the newest buffer's region is still tracked: {st}");
    assert_eq!(st["recycled"], (N - 1) as u64, "every earlier region was recycled under a live resource: {st}");
    // the textures hold their own bytes: the first (its slot was recycled under it) and the last
    let rb_desc = D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ..buf_desc };
    let readback_heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
    for &i in &[0usize, N - 1] {
        let mut rb: Option<ID3D12Resource> = None;
        device.CreateCommittedResource(&readback_heap, D3D12_HEAP_FLAG_NONE, &rb_desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut rb).unwrap();
        let rb = rb.unwrap();
        let (a, l) = &lists[i];
        a.Reset().unwrap();
        l.Reset(a, None).unwrap();
        l.ResourceBarrier(&[barrier(&textures[i], D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&rb),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
        };
        let src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&textures[i]),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        l.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        l.ResourceBarrier(&[barrier(&textures[i], D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST)]);
        l.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(l.cast().unwrap())]);
        host.wait_idle();
        let mut p: *mut c_void = std::ptr::null_mut();
        rb.Map(0, None, Some(&mut p)).unwrap();
        let bytes = std::slice::from_raw_parts(p as *const u8, BYTES as usize);
        let want = (i + 1) as u8;
        let wrong = [0usize, 8191, BYTES as usize / 2, BYTES as usize - 1].iter().filter(|&&k| bytes[k] != want).count();
        rb.Unmap(0, None);
        assert_eq!(wrong, 0, "texture {i} holds byte {want} everywhere (slot recycling waited for its copy)");
    }
    // the game lets go of all of them at once
    drop(held);
    let st = send(pipe, &none())["ring"].clone();
    assert_eq!(st["live"], 0, "every buffer was released: {st}");
    // a second copy out of one buffer, on another list: caught, and the ring switches itself off
    let buf = make_buffer(0xEE);
    for k in 0..2 {
        let (a, l) = &lists[k];
        a.Reset().unwrap();
        l.Reset(a, None).unwrap();
        copy(l, &buf, &textures[k]);
        l.Close().unwrap();
        queue.ExecuteCommandLists(&[Some(l.cast().unwrap())]);
    }
    host.wait_idle();
    drop(buf);
    let st = send(pipe, &none())["ring"].clone();
    assert_eq!(st["rereads"], 1, "{st}");
    assert_eq!(st["enabled"], false, "a second copy switches the ring off: {st}");
    send(pipe, &Request::Mem { trace: None, min_kb: None, stop: None, report: None, ring: Some(false), ring_mb: None, ring_any: Some(false) });
    host.wait_idle();
    println!("staging ring: recycled under live resources, every texture right, a second copy caught");
}

/// The render status once `key` reads `want` (its lines refresh every 30 frames, and a busy machine renders the host
/// slowly), or as it stands after four seconds.
unsafe fn render_status_when(host: &mut Host, pipe: &str, key: &str, want: &Value) -> Value {
    let t = Instant::now();
    loop {
        let r = status(pipe)["render"].clone();
        if &r[key] == want || t.elapsed() > Duration::from_secs(4) {
            return r;
        }
        host.run(100);
    }
}

fn status_phase(pipe: &str) -> String {
    proto::request(pipe, &Request::Status, 2000).ok().and_then(|v| v["phase"].as_str().map(str::to_string)).unwrap_or_default()
}

// ---------------------------------------------------------------- a fake Streamline wrapper

/// Streamline's "the object you wrap" interface (`StreamlineRetrieveBaseInterface`).
const SL_BASE: GUID = GUID::from_u128(0xadec44e2_61f0_45c3_ad9f_1b37379284ff);

/// A COM object standing in for Streamline's swap chain wrapper: Present and Present1 forward to the DXGI swap chain
/// it wraps (through that swap chain's vtable, as the real wrapper does), the base interface gives it, any other
/// interface is the wrapped swap chain's.
#[repr(C)]
struct FakeWrapper {
    vtbl: *const usize,
    inner: *mut c_void,
}

type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const DXGI_PRESENT_PARAMETERS) -> HRESULT;

unsafe extern "system" fn fw_qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let w = &*(this as *const FakeWrapper);
    let inner = IUnknown::from_raw_borrowed(&w.inner).unwrap();
    if *iid == SL_BASE {
        inner.query(&IUnknown::IID, out)
    } else {
        inner.query(iid, out)
    }
}

unsafe extern "system" fn fw_addref(_: *mut c_void) -> u32 {
    2
}

unsafe extern "system" fn fw_release(_: *mut c_void) -> u32 {
    1
}

unsafe extern "system" fn fw_notimpl() -> HRESULT {
    HRESULT(0x8000_4001u32 as i32)
}

unsafe extern "system" fn fw_present(this: *mut c_void, sync: u32, flags: u32) -> HRESULT {
    let w = &*(this as *const FakeWrapper);
    let f: PresentFn = std::mem::transmute::<usize, PresentFn>(*(*(w.inner as *const *const usize)).add(8));
    f(w.inner, sync, flags)
}

unsafe extern "system" fn fw_present1(this: *mut c_void, sync: u32, flags: u32, params: *const DXGI_PRESENT_PARAMETERS) -> HRESULT {
    let w = &*(this as *const FakeWrapper);
    let f: Present1Fn = std::mem::transmute::<usize, Present1Fn>(*(*(w.inner as *const *const usize)).add(22));
    f(w.inner, sync, flags, params)
}
