//! The Caveman's texture writer (board.rs), offline: a host with its own D3D12 device, queue and swap chain stands in
//! for the game. Its "deck texture" is made as the game's are (BC7 sRGB, 512 x 2048, 12 mips, placed in a heap of its
//! own, uploaded, then left in PIXEL | NON_PIXEL_SHADER_RESOURCE, the state the game's renderer uses), with fake
//! objects in the host's memory standing in for the game's chain (TextureAsset, runtime texture, RenderCore2's object;
//! `cmfx_test_board` gives the DLL their vtables). The host loads the built DLL (dormant: not Skate.exe), asks for the
//! board over the pipe, sends art, and reads its texture back after the frames: every mip byte for byte. Checked too:
//! a decoy texture (another name) is never touched, wrong fields and a texture whose own desc disagrees are refused, the
//! state the host's barriers give the texture is followed, the art goes back on a texture made again, reset, the fx
//! commands, the D3D12 debug layer's verdict (it tracks resource states: a wrong "before" state or a texture left in
//! another state is an error), and unload.

use std::ffi::{c_void, CString};
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use caveman_fx::board::{self, layout, TEXTURE_NAME};
use caveman_fx::proto::{self, BoardArt, Request};
use caveman_fx::win;
use serde_json::{json, Value};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

const SRV: u32 = 0xc0;

fn barrier(resource: &ID3D12Resource, before: u32, after: u32) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: D3D12_RESOURCE_STATES(before as i32),
                StateAfter: D3D12_RESOURCE_STATES(after as i32),
            }),
        },
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, w, l)
}

/// `CAVEMAN_HOST_SDK=618;E:\Games\ReSkate\x64\`: the game's Agility SDK, as in overlay_host.rs.
fn game_sdk() -> Option<(u32, String)> {
    let v = std::env::var("CAVEMAN_HOST_SDK").ok()?;
    let (version, path) = v.split_once(';')?;
    Some((version.trim().parse().ok()?, path.trim().to_string()))
}

/// A deck texture of the host's: placed in a heap of its own, as the game's are.
struct Tex {
    res: ID3D12Resource,
    _heap: ID3D12Heap,
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
    /// Transitions to record at the start of the next frame (in the frame's own list, which the DLL's hooks see).
    transitions: Vec<(ID3D12Resource, u32, u32)>,
}

impl Host {
    unsafe fn new() -> Host {
        use windows::Win32::UI::WindowsAndMessaging::*;
        let sdk = game_sdk();
        if sdk.is_none() && std::env::var("CAVEMAN_HOST_DEBUG_LAYER").map_or(true, |v| v != "0") {
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
                let path = CString::new(path.as_str()).unwrap();
                let factory: ID3D12DeviceFactory = config.unwrap().CreateDeviceFactory(*version, windows::core::PCSTR(path.as_ptr().cast())).unwrap();
                factory.CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap();
                println!("the host's device is from the Agility SDK {version} in {}", path.to_string_lossy());
            }
            None => D3D12CreateDevice(None::<&windows::core::IUnknown>, D3D_FEATURE_LEVEL_11_0, &mut device).unwrap(),
        }
        let device = device.unwrap();
        let queue: ID3D12CommandQueue =
            device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
        let class = windows::core::w!("CavemanFxBoardHost");
        let instance = windows::Win32::Foundation::HINSTANCE(win::GetModuleHandleW(std::ptr::null()) as isize);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR::null(), WS_OVERLAPPEDWINDOW, 0, 0, 160, 120, None, None, instance, None);
        assert!(hwnd.0 != 0);
        let factory: IDXGIFactory2 = CreateDXGIFactory2(0).unwrap();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 160,
            Height: 120,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
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
        Host { device, queue, chain, allocator, list, rtv_heap, fence, value: 0, event, transitions: Vec::new() }
    }

    unsafe fn wait_idle(&mut self) {
        self.value += 1;
        self.queue.Signal(&self.fence, self.value).unwrap();
        if self.fence.GetCompletedValue() < self.value {
            self.fence.SetEventOnCompletion(self.value, self.event).unwrap();
            win::WaitForSingleObject(self.event.0 as win::HANDLE, 5000);
        }
    }

    /// One frame: any transitions asked for, the back buffer cleared, Present (the DLL's hook uploads there).
    unsafe fn frame(&mut self) {
        self.wait_idle();
        let buffer: ID3D12Resource = self.chain.GetBuffer(self.chain.GetCurrentBackBufferIndex()).unwrap();
        let rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        self.device.CreateRenderTargetView(&buffer, None, rtv);
        self.allocator.Reset().unwrap();
        self.list.Reset(&self.allocator, None).unwrap();
        for (res, from, to) in self.transitions.drain(..) {
            self.list.ResourceBarrier(&[barrier(&res, from, to)]);
        }
        self.list.ResourceBarrier(&[barrier(&buffer, 0, 4)]);
        self.list.ClearRenderTargetView(rtv, &[0.1, 0.1, 0.1, 1.0], None);
        self.list.ResourceBarrier(&[barrier(&buffer, 4, 0)]);
        self.list.Close().unwrap();
        self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
        drop(buffer);
        let _ = self.chain.Present(0, 0);
    }

    unsafe fn run(&mut self, ms: u64) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
        self.wait_idle();
    }

    /// A BC7 sRGB texture placed in a heap of its own, filled with `chain` (the levels one after another) and left in
    /// `state`, as the game's deck textures are.
    unsafe fn texture(&mut self, width: u32, height: u32, levels: u16, chain: &[u8], state: u32) -> Tex {
        self.texture_of(width, height, levels, chain, state, DXGI_FORMAT_BC7_UNORM_SRGB)
    }

    /// The same of any block-compressed format (BC1 for the world copy).
    unsafe fn texture_of(&mut self, width: u32, height: u32, levels: u16, chain: &[u8], state: u32, format: DXGI_FORMAT) -> Tex {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: levels,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_NONE,
            ..Default::default()
        };
        let info = self.device.GetResourceAllocationInfo(0, &[desc]);
        let heap_desc = D3D12_HEAP_DESC {
            SizeInBytes: info.SizeInBytes,
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
            Alignment: info.Alignment,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
        };
        let mut heap: Option<ID3D12Heap> = None;
        self.device.CreateHeap(&heap_desc, &mut heap).unwrap();
        let heap = heap.unwrap();
        let mut res: Option<ID3D12Resource> = None;
        self.device.CreatePlacedResource(&heap, 0, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut res).unwrap();
        let res = res.unwrap();
        self.refill(&res, chain, 0x400, state);
        Tex { res, _heap: heap }
    }

    /// The whole chain copied into an existing texture (as the game re-fills the world copy at an equip): from
    /// `from` to COPY_DEST, the copies, then to `state`.
    unsafe fn refill(&mut self, res: &ID3D12Resource, chain: &[u8], from: u32, state: u32) {
        let (fp, rows, row_bytes, total) = self.footprints(res);
        let upload = self.buffer(D3D12_HEAP_TYPE_UPLOAD, total, D3D12_RESOURCE_STATE_GENERIC_READ);
        let mut p: *mut c_void = std::ptr::null_mut();
        upload.Map(0, None, Some(&mut p)).unwrap();
        let mut src = 0usize;
        for m in 0..fp.len() {
            for r in 0..rows[m] as usize {
                let n = row_bytes[m] as usize;
                std::ptr::copy_nonoverlapping(
                    chain[src + r * n..].as_ptr(),
                    (p as *mut u8).add(fp[m].Offset as usize + r * fp[m].Footprint.RowPitch as usize),
                    n,
                );
            }
            src += rows[m] as usize * row_bytes[m] as usize;
        }
        upload.Unmap(0, None);
        self.wait_idle();
        self.allocator.Reset().unwrap();
        self.list.Reset(&self.allocator, None).unwrap();
        if from != 0x400 {
            self.list.ResourceBarrier(&[barrier(res, from, 0x400)]);
        }
        for (m, f) in fp.iter().enumerate() {
            let dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(res),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: m as u32 },
            };
            let src = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&upload),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *f },
            };
            self.list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        }
        self.list.ResourceBarrier(&[barrier(res, 0x400, state)]);
        self.list.Close().unwrap();
        self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
        self.wait_idle();
    }

    /// One `CopyResource` from `src` into `dst` (both left in `state`), as the game copies an equip-time result into
    /// the ridden board's texture: for the copy tracer.
    unsafe fn copy_resource(&mut self, src: &ID3D12Resource, dst: &ID3D12Resource, state: u32) {
        self.wait_idle();
        self.allocator.Reset().unwrap();
        self.list.Reset(&self.allocator, None).unwrap();
        self.list.ResourceBarrier(&[barrier(src, state, 0x800), barrier(dst, state, 0x400)]);
        self.list.CopyResource(dst, src);
        self.list.ResourceBarrier(&[barrier(src, 0x800, state), barrier(dst, 0x400, state)]);
        self.list.Close().unwrap();
        self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
        self.wait_idle();
    }

    /// A BC7 sRGB texture array of one mip (256 x 256 slices, as the game's tile arrays), each slice filled from
    /// `slices` by a copy (traced), left in `state`.
    unsafe fn texture_array(&mut self, side: u32, slices: &[Vec<u8>], state: u32) -> Tex {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: side as u64,
            Height: side,
            DepthOrArraySize: slices.len() as u16,
            MipLevels: 1,
            Format: DXGI_FORMAT_BC7_UNORM_SRGB,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_NONE,
            ..Default::default()
        };
        let info = self.device.GetResourceAllocationInfo(0, &[desc]);
        let heap_desc = D3D12_HEAP_DESC {
            SizeInBytes: info.SizeInBytes,
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
            Alignment: info.Alignment,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
        };
        let mut heap: Option<ID3D12Heap> = None;
        self.device.CreateHeap(&heap_desc, &mut heap).unwrap();
        let heap = heap.unwrap();
        let mut res: Option<ID3D12Resource> = None;
        self.device.CreatePlacedResource(&heap, 0, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut res).unwrap();
        let res = res.unwrap();
        self.transitions.push((res.clone(), 0x400, state));
        self.run(30);
        for (k, bytes) in slices.iter().enumerate() {
            self.upload_slice(&res, k as u32, side, bytes, state);
        }
        Tex { res, _heap: heap }
    }

    /// One slice of a BC7 array written from `bytes` (rows of blocks, tightly packed), as the game's compressor
    /// output lands: a copy into subresource `slice`, between barriers from `state` and back.
    unsafe fn upload_slice(&mut self, res: &ID3D12Resource, slice: u32, side: u32, bytes: &[u8], state: u32) {
        let row = (side / 4) as usize * 16;
        let rows = (side / 4) as usize;
        let pitch = row.div_ceil(256) * 256;
        let upload = self.buffer(D3D12_HEAP_TYPE_UPLOAD, (rows * pitch) as u64, D3D12_RESOURCE_STATE_GENERIC_READ);
        let mut p: *mut c_void = std::ptr::null_mut();
        upload.Map(0, None, Some(&mut p)).unwrap();
        for r in 0..rows {
            std::ptr::copy_nonoverlapping(bytes[r * row..].as_ptr(), (p as *mut u8).add(r * pitch), row);
        }
        upload.Unmap(0, None);
        self.wait_idle();
        self.allocator.Reset().unwrap();
        self.list.Reset(&self.allocator, None).unwrap();
        self.list.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: std::mem::transmute_copy(res),
                    Subresource: slice,
                    StateBefore: D3D12_RESOURCE_STATES(state as i32),
                    StateAfter: D3D12_RESOURCE_STATE_COPY_DEST,
                }),
            },
        }]);
        let dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(res),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: slice },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::transmute_copy(&upload),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT { Format: DXGI_FORMAT_BC7_UNORM_SRGB, Width: side, Height: side, Depth: 1, RowPitch: pitch as u32 },
                },
            },
        };
        self.list.CopyTextureRegion(&dst, 0, 0, 0, &from, None);
        self.list.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: std::mem::transmute_copy(res),
                    Subresource: slice,
                    StateBefore: D3D12_RESOURCE_STATE_COPY_DEST,
                    StateAfter: D3D12_RESOURCE_STATES(state as i32),
                }),
            },
        }]);
        self.list.Close().unwrap();
        self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
        self.wait_idle();
    }

    /// A BC7 sRGB texture placed in a heap of its own, made in SRV and never written (its content undefined): a
    /// texture no barrier of the host's ever touches, so the DLL's uploads table never learns its state.
    unsafe fn blank_texture(&mut self, width: u32, height: u32, levels: u16) -> Tex {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: levels,
            Format: DXGI_FORMAT_BC7_UNORM_SRGB,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_NONE,
            ..Default::default()
        };
        let info = self.device.GetResourceAllocationInfo(0, &[desc]);
        let heap_desc = D3D12_HEAP_DESC {
            SizeInBytes: info.SizeInBytes,
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
            Alignment: info.Alignment,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
        };
        let mut heap: Option<ID3D12Heap> = None;
        self.device.CreateHeap(&heap_desc, &mut heap).unwrap();
        let heap = heap.unwrap();
        let mut res: Option<ID3D12Resource> = None;
        self.device.CreatePlacedResource(&heap, 0, &desc, D3D12_RESOURCE_STATES(SRV as i32), None, &mut res).unwrap();
        Tex { res: res.unwrap(), _heap: heap }
    }

    /// An RGBA8 render target of `levels` mips placed in a heap of its own, left in RENDER_TARGET (as the composite
    /// the game draws the deck's graphic into).
    unsafe fn rgba_target(&mut self, width: u32, height: u32, levels: u16) -> Tex {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: width as u64,
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: levels,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ..Default::default()
        };
        let info = self.device.GetResourceAllocationInfo(0, &[desc]);
        let heap_desc = D3D12_HEAP_DESC {
            SizeInBytes: info.SizeInBytes,
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
            Alignment: info.Alignment,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES,
        };
        let mut heap: Option<ID3D12Heap> = None;
        self.device.CreateHeap(&heap_desc, &mut heap).unwrap();
        let heap = heap.unwrap();
        let clear = D3D12_CLEAR_VALUE { Format: DXGI_FORMAT_R8G8B8A8_UNORM, Anonymous: D3D12_CLEAR_VALUE_0 { Color: [0.0, 0.0, 0.0, 1.0] } };
        let mut res: Option<ID3D12Resource> = None;
        self.device.CreatePlacedResource(&heap, 0, &desc, D3D12_RESOURCE_STATE_RENDER_TARGET, Some(&clear), &mut res).unwrap();
        let res = res.unwrap();
        // a placed render target must be cleared (or discarded) before anything else touches it: every mip black
        self.wait_idle();
        self.allocator.Reset().unwrap();
        self.list.Reset(&self.allocator, None).unwrap();
        let rtv = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        for m in 0..levels {
            let view = D3D12_RENDER_TARGET_VIEW_DESC {
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
                Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_RTV { MipSlice: m as u32, PlaneSlice: 0 } },
            };
            self.device.CreateRenderTargetView(&res, Some(&view), rtv);
            self.list.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0], None);
        }
        self.list.Close().unwrap();
        self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
        self.wait_idle();
        Tex { res, _heap: heap }
    }

    unsafe fn buffer(&self, kind: D3D12_HEAP_TYPE, bytes: u64, state: D3D12_RESOURCE_STATES) -> ID3D12Resource {
        let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: bytes,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut b: Option<ID3D12Resource> = None;
        if let Err(e) = self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut b) {
            let reason = self.device.GetDeviceRemovedReason();
            panic!("a buffer: {e}; device removed reason {reason:?}; debug layer: {:#?}", debug_errors(&self.device));
        }
        b.unwrap()
    }

    unsafe fn footprints(&self, res: &ID3D12Resource) -> (Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>, Vec<u32>, Vec<u64>, u64) {
        let desc = res.GetDesc();
        // every subresource (the mips, and for an array each slice's)
        let n = desc.MipLevels as usize * desc.DepthOrArraySize as usize;
        let mut fp = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
        let mut rows = vec![0u32; n];
        let mut row_bytes = vec![0u64; n];
        let mut total = 0u64;
        self.device.GetCopyableFootprints(
            &desc,
            0,
            n as u32,
            0,
            Some(fp.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            Some(row_bytes.as_mut_ptr()),
            Some(&mut total),
        );
        (fp, rows, row_bytes, total)
    }

    /// The texture's whole chain, read back (from `state` to COPY_SOURCE and back: the debug layer checks the state).
    unsafe fn read(&mut self, res: &ID3D12Resource, state: u32) -> Vec<u8> {
        let (fp, rows, row_bytes, total) = self.footprints(res);
        let readback = self.buffer(D3D12_HEAP_TYPE_READBACK, total, D3D12_RESOURCE_STATE_COPY_DEST);
        self.wait_idle();
        self.allocator.Reset().unwrap();
        self.list.Reset(&self.allocator, None).unwrap();
        self.list.ResourceBarrier(&[barrier(res, state, 0x800)]);
        for (m, f) in fp.iter().enumerate() {
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&readback),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: *f },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(res),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: m as u32 },
            };
            self.list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
        }
        self.list.ResourceBarrier(&[barrier(res, 0x800, state)]);
        self.list.Close().unwrap();
        self.queue.ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
        self.wait_idle();
        let mut p: *mut c_void = std::ptr::null_mut();
        if let Err(e) = readback.Map(0, None, Some(&mut p)) {
            let reason = self.device.GetDeviceRemovedReason();
            panic!("read-back map: {e}; device removed reason {reason:?}; debug layer: {:#?}", debug_errors(&self.device));
        }
        let mut out = Vec::new();
        for m in 0..fp.len() {
            for r in 0..rows[m] as usize {
                let at = (p as *const u8).add(fp[m].Offset as usize + r * fp[m].Footprint.RowPitch as usize);
                out.extend_from_slice(std::slice::from_raw_parts(at, row_bytes[m] as usize));
            }
        }
        readback.Unmap(0, None);
        out
    }
}

/// A deterministic chain of bytes (any 16 bytes are a BC7 block; the test compares bytes, not pixels).
fn pattern(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// The fake objects standing in for the game's chain (board.rs's layout), in the host's heap.
struct Fakes {
    vtables: Box<[u64; 4]>,
    descriptor: Box<[u64; 2]>,
    type_info: Box<[u64; 2]>,
    name: CString,
    texture: Box<[u64; 0x24]>,
    rc2: Box<[u64; 0x28]>,
    asset: Box<[u64; 8]>,
}

impl Fakes {
    fn new(name: &str, resource: usize) -> Fakes {
        let vtables = Box::new([0u64; 4]);
        let descriptor = Box::new([board::TEXTURE_ASSET_HASH as u64, 0]);
        let type_info = Box::new([&*descriptor as *const _ as u64, 0]);
        let mut f = Fakes {
            vtables,
            descriptor,
            type_info,
            name: CString::new(name).unwrap(),
            texture: Box::new([0u64; 0x24]),
            rc2: Box::new([0u64; 0x28]),
            asset: Box::new([0u64; 8]),
        };
        f.texture[0] = f.texture_vtable() as u64;
        for (off, v) in [
            (layout::TEX_FORMAT, 67),
            (layout::TEX_WIDTH, 512),
            (layout::TEX_HEIGHT, 2048),
            (layout::TEX_DEPTH, 1),
            (layout::TEX_ARRAY, 1),
            (layout::TEX_MIPS, 12),
        ] {
            f.set_tex_u32(off, v);
        }
        f.texture[layout::TEX_NAME_HASH / 8] = board::owner_hash(name);
        f.texture[layout::TEX_RC2 / 8] = &*f.rc2 as *const _ as u64;
        f.rc2[0] = f.rc2_vtable() as u64;
        f.rc2[layout::RC2_BIND / 8] = 8 << 32;
        f.rc2[layout::RC2_STATE / 8] = 8;
        f.set_resource(resource);
        f.asset[0] = f.asset_vtable() as u64;
        f.asset[1] = &*f.type_info as *const _ as u64;
        f.asset[3] = f.name.as_ptr() as u64;
        f.asset[4] = &*f.texture as *const _ as u64;
        f
    }

    /// Shares another fake's vtables and type (a second asset of the same class).
    fn same_class(mut self, other: &Fakes) -> Fakes {
        self.texture[0] = other.texture_vtable() as u64;
        self.rc2[0] = other.rc2_vtable() as u64;
        self.asset[0] = other.asset_vtable() as u64;
        self.asset[1] = &*other.type_info as *const _ as u64;
        self
    }

    fn asset_vtable(&self) -> usize {
        &self.vtables[0] as *const _ as usize
    }
    fn texture_vtable(&self) -> usize {
        &self.vtables[1] as *const _ as usize
    }
    fn rc2_vtable(&self) -> usize {
        &self.vtables[2] as *const _ as usize
    }
    fn asset_addr(&self) -> usize {
        &*self.asset as *const _ as usize
    }

    fn set_tex_u32(&mut self, off: usize, v: u32) {
        let w = &mut self.texture[off / 8];
        let shift = (off % 8) * 8;
        *w = (*w & !(0xffff_ffffu64 << shift)) | ((v as u64) << shift);
    }

    fn set_resource(&mut self, resource: usize) {
        self.texture[layout::TEX_D3D12 / 8] = resource as u64;
        self.rc2[layout::RC2_D3D12 / 8] = resource as u64;
    }
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

fn board_status(pipe: &str) -> Value {
    send(pipe, &Request::board())["board"].clone()
}

/// The art with an RGBA8 chain of it along (for the composite).
fn art_with(pipe: &str, label: &str, chain: &[u8], rgba: Option<&[u8]>, keep: Option<bool>) -> Value {
    let arts = rgba.map(|r| vec![BoardArt::with_format(label, "rgba8", 12, r)]);
    send(
        pipe,
        &Request::Board {
            art: Some(BoardArt::new(label, chain)),
            keep,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    )
}

fn art(pipe: &str, label: &str, chain: &[u8], keep: Option<bool>) -> Value {
    send(
        pipe,
        &Request::Board {
            art: Some(BoardArt::new(label, chain)),
            keep,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    )
}

/// Frames until `done` says so (or `secs` pass: a failure with the board's state).
unsafe fn until(host: &mut Host, pipe: &str, secs: u64, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let t = Instant::now();
    loop {
        let b = board_status(pipe);
        if done(&b) {
            return b;
        }
        if t.elapsed() >= Duration::from_secs(secs) {
            let errors = debug_errors(&host.device);
            panic!(
                "{what}: {}
D3D12 debug layer: {errors:#?}",
                serde_json::to_string_pretty(&b).unwrap()
            );
        }
        host.run(40);
    }
}

unsafe fn debug_errors(device: &ID3D12Device) -> Vec<String> {
    let Ok(q) = device.cast::<ID3D12InfoQueue>() else {
        println!("(no D3D12 debug layer: install the Graphics Tools to have the API use and the states checked)");
        return Vec::new();
    };
    println!("D3D12 debug layer on: {} message(s) stored", q.GetNumStoredMessages());
    let mut out = Vec::new();
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
            // the host's own clears of its render target with values other than its creation's (a performance
            // warning about the host, not about the DLL)
            if text.contains("The clear values do not match those passed to resource creation") {
                continue;
            }
            if m.Severity == D3D12_MESSAGE_SEVERITY_ERROR || m.Severity == D3D12_MESSAGE_SEVERITY_CORRUPTION || m.Severity == D3D12_MESSAGE_SEVERITY_WARNING {
                out.push(text);
            }
        }
    }
    out
}

#[test]
fn board_writes_its_texture_and_only_its_texture() {
    let dll = dll_path();
    if !dll.is_file() {
        println!("skipped: {} is not built yet (cargo build -p caveman-fx, then test again)", dll.display());
        return;
    }
    unsafe { run_host(&dll) };
}

unsafe fn run_host(dll: &std::path::Path) {
    let mut host = Host::new();
    let len = board::chain_len();
    let white = pattern(1, len);
    // the deck texture and a decoy (another deck's texture: never to be touched), both as the game leaves them
    let mut deck = host.texture(512, 2048, 12, &white, SRV);
    let decoy_bytes = pattern(2, len);
    let decoy = host.texture(512, 2048, 12, &decoy_bytes, SRV);
    let mut fakes = Fakes::new(TEXTURE_NAME, deck.res.as_raw() as usize);
    let decoy_fakes = Fakes::new(&TEXTURE_NAME.replace("the_caveman", "the_cavewoman"), decoy.res.as_raw() as usize).same_class(&fakes);
    // a sibling: an equip-time copy of the texture as the game makes them (a runtime texture of the same shape under
    // another name, no asset of The Caveman's), its pointer next to the main texture's in a material's texture table
    // (entries of 0x30 bytes, the pointer at +0x18)
    let sibling_bytes = pattern(9, len);
    let sibling = host.texture(512, 2048, 12, &sibling_bytes, SRV);
    let sibling_fakes =
        Fakes::new("characters/customization/reskate/board_bottomart/sibling_copy_8f6992cc49", sibling.res.as_raw() as usize).same_class(&fakes);
    let mut table = Box::new([0u64; 24]);
    table[2 * 6 + 3] = &*fakes.texture as *const _ as u64;
    table[3 * 6 + 3] = &*sibling_fakes.texture as *const _ as u64;
    // (observable, or a release build drops the stores into a box nothing reads)
    let table = std::hint::black_box(table);
    host.run(30);

    let pipe = proto::pipe_for(false, std::process::id());
    let module = win::LoadLibraryW(win::wide(&dll.to_string_lossy()).as_ptr());
    assert!(!module.is_null());
    let t = Instant::now();
    while proto::request(&pipe, &Request::Status, 2000).ok().and_then(|v| v["phase"].as_str().map(str::to_string)).as_deref() != Some("dormant") {
        assert!(t.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some((version, path)) = game_sdk() {
        let set_sdk: unsafe extern "C" fn(u32, *const std::ffi::c_char) -> i32 =
            std::mem::transmute(win::GetProcAddress(module, c"cmfx_test_sdk".as_ptr().cast()));
        let path = CString::new(path).unwrap();
        assert_eq!(set_sdk(version, path.as_ptr()), 0);
    }
    let set_board: unsafe extern "C" fn(usize, usize, usize, usize, usize) -> i32 =
        std::mem::transmute(win::GetProcAddress(module, c"cmfx_test_board".as_ptr().cast()));
    assert_eq!(
        set_board(
            fakes.asset_vtable(),
            &*fakes.type_info as *const _ as usize,
            &*fakes.descriptor as *const _ as usize,
            fakes.texture_vtable(),
            fakes.rc2_vtable()
        ),
        0
    );

    // found by the scan: the asset, its texture, its resource, the state inferred from the RenderCore2 object
    let b = until(&mut host, &pipe, 60, "The Caveman found", |b| b["found"] == true);
    println!("found: {}", b["last_scan"]);
    let target = &b["targets"][0];
    assert_eq!(target["kind"], "main");
    assert_eq!(target["asset"], format!("{:#x}", fakes.asset_addr()));
    assert_eq!(target["resource"], format!("{:#x}", deck.res.as_raw() as usize));
    assert_eq!(target["state"]["ok"], true, "{target}");
    assert!(target["state"]["source"].as_str().unwrap().starts_with("inferred"), "{target}");
    assert!(target["state"]["states"].as_str().unwrap().starts_with("0xc0"), "{target}");
    // the sibling found through the table (the decoy, whose pointer sits nowhere near the main texture's, not), once
    // siblings are turned on (off by default: in the game they turned out to be other decks' graphics)
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: Some(true),
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 60, "the sibling found", |b| b["siblings"] == 1);
    println!("siblings: {}", b["last_sibling_scan"]);
    assert_eq!(b["targets"].as_array().unwrap().len(), 2, "The Caveman's texture and its sibling (not the decoy): {b}");
    let sib = &b["targets"][1];
    assert_eq!(sib["kind"], "sibling");
    assert_eq!(sib["resource"], format!("{:#x}", sibling.res.as_raw() as usize));
    assert!(sib["state"]["states"].as_str().unwrap().starts_with("0xc0"), "{sib}");
    let installed = proto::request(&pipe, &Request::Status, 2000).unwrap()["render"]["installed"].clone();
    assert_eq!(installed, true, "the overlay installs for the board (its Present hook uploads)");

    // art: every mip byte for byte, the decoy untouched, the texture left in the state it had (the host's read-back
    // barrier starts from 0xc0: the debug layer checks it)
    let a = pattern(3, len);
    art(&pipe, "pattern a", &a, None);
    let b = until(&mut host, &pipe, 20, "art a written", |b| b["uploads"] == 1);
    println!("upload: {}", b["last_upload"]);
    assert!(b["pending"].is_null());
    let got = host.read(&deck.res, SRV);
    assert_eq!(got.len(), len);
    assert!(got == a, "every mip of the texture is the art (first difference at byte {:?})", got.iter().zip(&a).position(|(x, y)| x != y));
    assert!(host.read(&decoy.res, SRV) == decoy_bytes, "the decoy is untouched");
    assert!(host.read(&sibling.res, SRV) == a, "the sibling holds the art too");
    assert_eq!(b["last_upload"]["resources"].as_array().map_or(0, Vec::len), 2, "written into 2 textures: {}", b["last_upload"]);
    let frame_us = b["last_upload"]["frame_us"].as_u64().unwrap();
    println!("the upload took {frame_us} us in the frame ({} us filling the upload buffer)", b["last_upload"]["fill_us"]);
    // the read-back check (board verify): a 4 x 4 block of mips 0 and 2 read back in the upload's own list, and again
    // 2 s later, both matching the art sent
    let b = until(&mut host, &pipe, 10, "the read-back right after the upload", |b| b["verify"]["targets"][0]["verified_now"] != "not yet");
    println!("read-back after the upload: {}", b["verify"]);
    assert_eq!(b["verify"]["targets"][0]["verified_now"], "match", "{}", b["verify"]);
    assert_eq!(b["verify"]["targets"].as_array().map_or(0, Vec::len), 2, "two targets written (the texture and its sibling): {}", b["verify"]);
    assert_eq!(b["verify"]["targets"][1]["verified_now"], "match", "{}", b["verify"]);
    let b = until(&mut host, &pipe, 10, "the read-back 2 s later", |b| b["verify"]["targets"][0]["verified_later"] != "not yet");
    println!("read-back 2 s later: {}", b["verify"]);
    assert_eq!(b["verify"]["targets"][0]["verified_later"], "match", "{}", b["verify"]);
    assert!(b["verify"]["later_at_ms"].as_u64().unwrap() >= b["verify"]["now_at_ms"].as_u64().unwrap() + 1900, "{}", b["verify"]);
    // and on request
    let later_at = b["verify"]["later_at_ms"].clone();
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: Some(true),
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 10, "the read-back on request", |b| {
        b["verify"]["targets"][0]["verified_later"] != "not yet" && b["verify"]["later_at_ms"] != later_at
    });
    assert_eq!(b["verify"]["targets"][0]["verified_later"], "match", "{}", b["verify"]);
    if !cfg!(debug_assertions) {
        assert!(frame_us < 2000, "within the 2 ms budget: {frame_us} us");
    }

    // the host moves its texture to COMMON (as a game keeping textures in COMMON would): the DLL follows its barriers
    host.transitions.push((deck.res.clone(), SRV, 0));
    host.run(50);
    let b = until(&mut host, &pipe, 10, "the host's barrier seen", |b| b["targets"][0]["seen"]["transitions"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(b["targets"][0]["state"]["source"], "the game's own barriers on it", "{b}");
    assert!(b["targets"][0]["state"]["states"].as_str().unwrap().starts_with("0x0"), "{b}");
    let c = pattern(4, len);
    art(&pipe, "pattern c", &c, None);
    until(&mut host, &pipe, 20, "art c written", |b| b["uploads"] == 2);
    assert!(host.read(&deck.res, 0) == c, "written from COMMON and left in COMMON");
    host.transitions.push((deck.res.clone(), 0, SRV));
    host.run(50);
    until(&mut host, &pipe, 10, "back to 0xc0 seen", |b| b["targets"][0]["state"]["states"].as_str().unwrap_or("").starts_with("0xc0"));

    // by hand
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: Some("srv".into()),
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = board_status(&pipe);
    assert_eq!(b["targets"][0]["state"]["source"], "set by hand");
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: Some("auto".into()),
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let refused = proto::request(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: Some("0x400".into()),
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
        3000,
    )
    .unwrap();
    assert_eq!(refused["ok"], false, "COPY_DEST is not a state to leave it in: {refused}");

    // a wrong field: refused (the art waits), then found once right, and the waiting art goes on by itself
    fakes.set_tex_u32(layout::TEX_FORMAT, 66);
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: Some(true),
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 30, "refused for its format", |b| b["found"] == false && b["refused"].as_str().unwrap_or("").contains("format"));
    println!("refused: {}", b["refused"]);
    let d = pattern(5, len);
    art(&pipe, "pattern d", &d, None);
    host.run(300);
    assert!(host.read(&deck.res, SRV) == c, "nothing written while it is refused");
    fakes.set_tex_u32(layout::TEX_FORMAT, 67);
    until(&mut host, &pipe, 30, "found again and the waiting art written", |b| b["found"] == true && b["uploads"].as_u64().unwrap_or(0) >= 3);
    assert!(host.read(&deck.res, SRV) == d);

    // a texture whose own desc disagrees with the runtime texture's fields: refused at the GPU (GetDesc)
    let small_bytes = pattern(6, (0..9).map(|m| ((256usize >> m).max(1).div_ceil(4)).pow(2) * 16).sum());
    let small = host.texture(256, 256, 9, &small_bytes, SRV);
    fakes.set_resource(small.res.as_raw() as usize);
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: Some(true),
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    until(&mut host, &pipe, 30, "the small texture taken as a target", |b| b["targets"][0]["resource"] == format!("{:#x}", small.res.as_raw() as usize));
    art(&pipe, "pattern e", &pattern(7, len), Some(false));
    // (the main target refused; the sibling, still right, is written: the refusal is in the upload's skipped list)
    let b = until(&mut host, &pipe, 20, "refused by GetDesc", |b| {
        b["last_error"].as_str().unwrap_or("").contains("GetDesc") || b["last_upload"]["skipped"].to_string().contains("GetDesc")
    });
    println!("refused: {} {}", b["last_error"], b["last_upload"]["skipped"]);
    assert!(host.read(&small.res, SRV) == small_bytes, "the small texture is untouched");
    fakes.set_resource(deck.res.as_raw() as usize);
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: Some(true),
            state: None,
            find: Some(true),
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    until(&mut host, &pipe, 30, "back on the deck texture", |b| b["targets"][0]["resource"] == format!("{:#x}", deck.res.as_raw() as usize));

    // the texture made again (the deck loaded again): the kept art goes on the new one by itself
    let f = pattern(8, len);
    art(&pipe, "pattern f", &f, Some(true));
    until(&mut host, &pipe, 20, "art f written", |b| b["last_upload"]["label"] == "pattern f");
    assert!(host.read(&deck.res, SRV) == f);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    host.wait_idle();
    let remade = host.texture(512, 2048, 12, &white, SRV);
    fakes.set_resource(remade.res.as_raw() as usize);
    let old = std::mem::replace(&mut deck, remade);
    let b = until(&mut host, &pipe, 20, "the kept art on the texture made again", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    assert_eq!(b["targets"][0]["resource"], format!("{:#x}", deck.res.as_raw() as usize));
    assert!(host.read(&deck.res, SRV) == f, "the new texture has the kept art");
    host.wait_idle();
    drop(old);

    // reset: The Caveman's own bytes again, not kept
    art(&pipe, "reset", &white, Some(false));
    until(&mut host, &pipe, 20, "reset written", |b| b["last_upload"]["label"] == "reset");
    assert!(host.read(&deck.res, SRV) == white);
    assert_eq!(board_status(&pipe)["art"]["keep"], false);

    // the composite (the nameless RGBA8 render target the game draws the deck's graphic into at equip, which the ridden
    // board samples; audit of 2026-10-21 night): found by the runtime texture vtable scan (no name hash, the main's
    // size, a RenderCore2 object with flags 128 and 256), described by GetDesc, refused until the game's own barrier
    // on it is seen (never inferred), then written with the art's RGBA8 chain (its 10 levels) and read back
    let comp = host.rgba_target(512, 2048, 10);
    let mut comp_fakes = Fakes::new("characters/customization/reskate/board_bottomart/nameless", comp.res.as_raw() as usize).same_class(&fakes);
    comp_fakes.texture[layout::TEX_NAME_HASH / 8] = 0;
    comp_fakes.set_tex_u32(layout::TEX_FORMAT, 18);
    comp_fakes.set_tex_u32(layout::TEX_MIPS, 10);
    comp_fakes.rc2[layout::RC2_BIND / 8] = 128 << 32;
    comp_fakes.rc2[layout::RC2_STATE / 8] = 256;
    let comp_fakes = std::hint::black_box(comp_fakes);
    // (a look again: in the game the composite search runs again by itself while the deck is equipped)
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: Some(true),
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 60, "the composite found", |b| b["composites"] == 1);
    println!("composite: {}", b["last_composite_scan"]);
    let ct = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "composite").cloned().unwrap();
    assert_eq!(ct["resource"], format!("{:#x}", comp.res.as_raw() as usize));
    assert_eq!((ct["rc2_bind"].clone(), ct["rc2_state"].clone()), (json!(128), json!(256)), "{ct}");
    assert_eq!(ct["state"]["ok"], false, "never inferred: {ct}");
    assert!(ct["state"]["error"].as_str().unwrap().contains("re-equip"), "{ct}");
    let b = until(&mut host, &pipe, 10, "the composite described", |b| {
        b["targets"].as_array().unwrap().iter().any(|t| t["kind"] == "composite" && !t["dxgi_format"].is_null())
    });
    let ct = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "composite").cloned().unwrap();
    assert_eq!((ct["dxgi_format"].clone(), ct["mips"].clone()), (json!(DXGI_FORMAT_R8G8B8A8_UNORM.0), json!(10)), "{ct}");
    // written: the main (and the sibling) with the BC7 chain, the composite refused with a note, untouched
    let rgba_len = board::chain_len_for(board::ChainFormat::Rgba8, 512, 2048, 12);
    let g = pattern(12, len);
    let g_rgba = pattern(13, rgba_len);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    art_with(&pipe, "pattern g", &g, Some(&g_rgba), Some(true));
    let b = until(&mut host, &pipe, 20, "art g written", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    println!("with the composite refused: {}", b["last_upload"]["skipped"]);
    assert!(host.read(&deck.res, SRV) == g);
    assert!(b["last_upload"]["skipped"].to_string().contains("re-equip"), "{}", b["last_upload"]);
    let ct = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "composite").cloned().unwrap();
    assert!(ct["refused"].as_str().unwrap().contains("re-equip"), "{ct}");
    assert!(host.read(&comp.res, 0x4).iter().all(|&x| x == 0 || x == 255), "the composite untouched (black)");
    // the game's barrier on it (RENDER_TARGET -> PIXEL_SHADER_RESOURCE, as at equip): seen, logged, the state known;
    // the kept art goes on it by itself
    host.transitions.push((comp.res.clone(), 0x4, SRV));
    host.run(60);
    let b = until(&mut host, &pipe, 20, "the composite written after the barrier", |b| b["uploads"].as_u64().unwrap_or(0) > uploads + 1);
    let ct = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "composite").cloned().unwrap();
    assert_eq!(ct["state"]["source"], "the game's own barriers on it", "{ct}");
    assert!(ct["state"]["states"].as_str().unwrap().starts_with("0xc0"), "{ct}");
    assert_eq!(ct["refused"], "", "{ct}");
    let written = host.read(&comp.res, SRV);
    let want = &g_rgba[..board::chain_len_for(board::ChainFormat::Rgba8, 512, 2048, 10)];
    assert_eq!(written.len(), want.len());
    assert!(
        written == want,
        "the composite holds the RGBA8 chain's 10 levels (first difference at byte {:?})",
        written.iter().zip(want).position(|(x, y)| x != y)
    );
    assert!(host.read(&deck.res, SRV) == g, "the main still holds the art");
    println!("composite written: {}", b["last_upload"]["states"]);
    let b = until(&mut host, &pipe, 10, "the composite's read-back", |b| {
        b["verify"]["targets"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x["resource"] == format!("{:#x}", comp.res.as_raw() as usize) && x["verified_now"] != "not yet"))
    });
    let vt = b["verify"]["targets"].as_array().unwrap().iter().find(|x| x["resource"] == format!("{:#x}", comp.res.as_raw() as usize)).cloned().unwrap();
    assert_eq!(vt["verified_now"], "match", "{}", b["verify"]);
    // the game renders it again before the later read: the check says so
    host.transitions.push((comp.res.clone(), SRV, 0x4));
    host.run(40);
    host.wait_idle();
    host.allocator.Reset().unwrap();
    host.list.Reset(&host.allocator, None).unwrap();
    let rtv = host.rtv_heap.GetCPUDescriptorHandleForHeapStart();
    let view = D3D12_RENDER_TARGET_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_RTV { MipSlice: 0, PlaneSlice: 0 } },
    };
    host.device.CreateRenderTargetView(&comp.res, Some(&view), rtv);
    host.list.ClearRenderTargetView(rtv, &[1.0, 0.0, 0.0, 1.0], None);
    host.list.Close().unwrap();
    host.queue.ExecuteCommandLists(&[Some(host.list.cast().unwrap())]);
    host.wait_idle();
    host.transitions.push((comp.res.clone(), 0x4, SRV));
    host.run(40);
    let b = until(&mut host, &pipe, 10, "the composite's later read-back", |b| {
        b["verify"]["targets"]
            .as_array()
            .is_some_and(|t| t.iter().any(|x| x["resource"] == format!("{:#x}", comp.res.as_raw() as usize) && x["verified_later"] != "not yet"))
    });
    let vt = b["verify"]["targets"].as_array().unwrap().iter().find(|x| x["resource"] == format!("{:#x}", comp.res.as_raw() as usize)).cloned().unwrap();
    println!("composite later: {}", vt["verified_later"]);
    assert!(vt["verified_later"].as_str().unwrap().starts_with("changed later (the game rendered it again)"), "{}", b["verify"]);
    // a state set by hand for the composite only (0xc0, which it is in): the main keeps its own source
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: Some("srv".into()),
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: Some("kind:composite".into()),
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = board_status(&pipe);
    let ct = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "composite").cloned().unwrap();
    let mt = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "main").cloned().unwrap();
    assert_eq!((ct["state"]["source"].clone(), ct["manual_state"].clone()), (json!("set by hand"), json!("0xc0")), "{ct}");
    assert_eq!(mt["manual_state"], Value::Null, "{mt}");
    assert_eq!(b["manual_per"][0]["for"], "kind:composite");
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: Some("auto".into()),
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: Some("kind:composite".into()),
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = board_status(&pipe);
    assert!(b["manual_per"].as_array().unwrap().is_empty());
    assert_eq!(b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "composite").unwrap()["state"]["source"], "the game's own barriers on it");
    // composite-common: a flag (the host's composite showed barriers, so it is never assumed COMMON)
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: Some(true),
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    assert_eq!(board_status(&pipe)["composite_common"], true);
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: Some(false),
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let errors = debug_errors(&host.device);
    assert!(errors.is_empty(), "the composite's write: {errors:#?}");
    host.wait_idle();

    // the uploads ring (`board uploads`): the host's RENDER_TARGET -> SRV barrier on the composite above (the game's
    // render of it) is in it, with the present it happened at and, once the render thread described it, its shape
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: Some(true),
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let comp_hex = format!("{:#x}", comp.res.as_raw() as usize);
    let b = until(&mut host, &pipe, 10, "the uploads ring described", |b| {
        b["uploads_ring"].as_array().is_some_and(|r| r.iter().any(|e| e["resource"] == comp_hex && e["desc"].is_object()))
    });
    let ring = b["uploads_ring"].as_array().unwrap();
    let e = ring.iter().find(|e| e["resource"] == comp_hex).cloned().unwrap();
    assert_eq!(
        (e["after"].clone(), e["target"].clone(), e["desc"]["width"].clone(), e["desc"]["height"].clone(), e["desc"]["mips"].clone()),
        (json!("0xc0"), json!("composite"), json!(512), json!(2048), json!(10)),
        "{e}"
    );
    assert!(e["presents_ago"].as_u64().is_some() && e["count"].as_u64().unwrap() >= 1 && e["regular"] == false, "{e}");
    // the host's back buffers, written every other frame, keep one slot each and are the regulars
    let regulars = ring.iter().filter(|e| e["regular"] == true).count();
    assert!((1..=3).contains(&regulars), "the back buffers are the frame regulars: {ring:#?}");
    println!("uploads table: {} entries ({regulars} regular), the composite's: {e}", ring.len());

    // a target added by hand (`board target 0x...`): a 256 x 256 BC7 texture of 4 mips (any 2D shape goes), with a
    // state set for it; written with the chain of its shape among the art's, read back; then removed
    let hand_len = board::chain_len_for(board::ChainFormat::Bc7, 256, 256, 4);
    let hand_bytes = pattern(21, hand_len);
    let hand = host.texture(256, 256, 4, &hand_bytes, SRV);
    let mut hand_fakes = Fakes::new("characters/customization/reskate/board_bottomart/by_hand", hand.res.as_raw() as usize).same_class(&fakes);
    hand_fakes.set_tex_u32(layout::TEX_WIDTH, 256);
    hand_fakes.set_tex_u32(layout::TEX_HEIGHT, 256);
    hand_fakes.set_tex_u32(layout::TEX_MIPS, 4);
    let hand_fakes = std::hint::black_box(hand_fakes);
    let hand_tex = format!("{:#x}", &*hand_fakes.texture as *const _ as usize);
    let target = |what: &str, state: Option<&str>| {
        proto::request(
            &pipe,
            &Request::Board {
                art: None,
                keep: None,
                state: state.map(str::to_string),
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: Some(what.to_string()),
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            },
            3000,
        )
        .unwrap()
    };
    assert!(target("0x10", None)["error"].as_str().unwrap().contains("not a pointer"));
    assert!(target(&format!("{:#x}", &*hand_fakes.rc2 as *const _ as usize), None)["error"].as_str().unwrap().contains("vtable"));
    assert!(target("zzz", None)["error"].as_str().unwrap().contains("0x..."));
    let v = target(&hand_tex, Some("srv"));
    assert_eq!(v["ok"], true, "{v}");
    assert!(target(&hand_tex, None)["error"].as_str().unwrap().contains("already a target"));
    let hand_hex = format!("{:#x}", hand.res.as_raw() as usize);
    let b = until(&mut host, &pipe, 10, "the target by hand described", |b| {
        b["targets"].as_array().unwrap().iter().any(|t| t["kind"] == "manual" && !t["dxgi_format"].is_null())
    });
    let ht = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "manual").cloned().unwrap();
    assert_eq!(
        (ht["resource"].clone(), ht["width"].clone(), ht["height"].clone(), ht["mips"].clone(), ht["dxgi_format"].clone()),
        (json!(hand_hex), json!(256), json!(256), json!(4), json!(DXGI_FORMAT_BC7_UNORM_SRGB.0)),
        "{ht}"
    );
    assert_eq!((ht["state"]["source"].clone(), ht["manual_state"].clone()), (json!("set by hand"), json!("0xc0")), "{ht}");
    assert_eq!(b["manual_targets"], 1);
    // the art with no chain of its shape: the others written, it refused with the reason
    let h = pattern(22, len);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    art_with(&pipe, "pattern h", &h, Some(&pattern(23, rgba_len)), Some(true));
    let b = until(&mut host, &pipe, 20, "art h written around the target by hand", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    assert!(host.read(&deck.res, SRV) == h);
    assert!(host.read(&hand.res, SRV) == hand_bytes, "no chain of its shape: untouched");
    let ht = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "manual").cloned().unwrap();
    assert!(ht["refused"].as_str().unwrap().contains("no bc7 chain of 256x256"), "{ht}");
    // with one: written byte for byte and read back
    let hand_art = pattern(24, hand_len);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    send(
        &pipe,
        &Request::Board {
            art: Some(BoardArt::new("pattern h2", &h)),
            keep: Some(true),
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: Some(vec![
                BoardArt::with_format("pattern h2", "rgba8", 12, &pattern(23, rgba_len)),
                BoardArt::with_shape("pattern h2", "bc7", 256, 256, 4, &hand_art),
            ]),
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 20, "the target by hand written", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    assert!(host.read(&hand.res, SRV) == hand_art, "the target by hand holds the chain of its shape");
    assert!(host.read(&deck.res, SRV) == h);
    assert_eq!(b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "manual").unwrap()["refused"], "", "{}", b["last_upload"]);
    let b = until(&mut host, &pipe, 10, "the target by hand's read-back", |b| {
        b["verify"]["targets"].as_array().is_some_and(|t| t.iter().any(|x| x["resource"] == hand_hex && x["verified_now"] != "not yet"))
    });
    assert_eq!(b["verify"]["targets"].as_array().unwrap().iter().find(|x| x["resource"] == hand_hex).unwrap()["verified_now"], "match", "{}", b["verify"]);
    // removed: its state by hand goes with it
    assert_eq!(target("clear", None)["ok"], true);
    let b = board_status(&pipe);
    assert_eq!(b["manual_targets"], 0);
    assert!(b["manual_per"].as_array().unwrap().is_empty(), "{}", b["manual_per"]);
    assert!(b["targets"].as_array().unwrap().iter().all(|t| t["kind"] != "manual"));
    host.wait_idle();

    // the finals (the run-time-made copies of The Caveman's shape with the known name hashes): found by the same
    // census as the composite, written with the BC7 chain as the main is (inferred from RenderCore2), off on request
    let fin_bytes = pattern(25, len);
    let fin = host.texture(512, 2048, 12, &fin_bytes, SRV);
    let mut fin_fakes = Fakes::new("characters/customization/reskate/board_bottomart/final_a", fin.res.as_raw() as usize).same_class(&fakes);
    fin_fakes.texture[layout::TEX_NAME_HASH / 8] = board::FINAL_HASHES[0];
    fin_fakes.set_tex_u32(layout::TEX_ID, 0x4242);
    let fin_fakes = std::hint::black_box(fin_fakes);
    let fin_hex = format!("{:#x}", fin.res.as_raw() as usize);
    assert_eq!(board_status(&pipe)["finals_on"], true, "on by default");
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: Some(true),
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 60, "the final found and written", |b| {
        b["finals"]["found"] == 1 && b["targets"].as_array().unwrap().iter().any(|t| t["kind"] == "final" && t["art"].is_object())
    });
    let ft = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "final").cloned().unwrap();
    assert_eq!(
        (ft["resource"].clone(), ft["hash"].clone(), ft["id"].clone()),
        (json!(fin_hex), json!(format!("{:#x}", board::FINAL_HASHES[0])), json!("0x4242")),
        "{ft}"
    );
    // (the host's upload of its bytes, COPY_DEST -> SRV, was seen: the game's own barrier; without one, a final takes
    // the main's rule, inferred from RenderCore2)
    assert!(ft["state"]["states"].as_str().unwrap().starts_with("0xc0"), "{ft}");
    assert!(
        ft["state"]["source"] == "the game's own barriers on it" || ft["state"]["source"] == "inferred from RenderCore2 (shader resource on the direct queue)",
        "{ft}"
    );
    assert!(host.read(&fin.res, SRV) == h, "the final holds the kept art");
    println!("final: {}", b["last_composite_scan"]);
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: Some(false),
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = board_status(&pipe);
    assert_eq!((b["finals_on"].clone(), b["finals"]["found"].clone()), (json!(false), json!(0)), "{}", b["finals"]);
    assert!(b["targets"].as_array().unwrap().iter().all(|t| t["kind"] != "final"));
    host.wait_idle();

    // the copy tracer: a CopyResource of the host's from one texture into another is in the ring with both shapes,
    // on a direct list submitted on the direct queue (the host's uploads from buffers are there too)
    let rawt = host.texture(512, 2048, 12, &pattern(27, len), SRV);
    let raw_hex = format!("{:#x}", rawt.res.as_raw() as usize);
    let copy_dst = host.texture(512, 2048, 12, &pattern(28, len), SRV);
    let dst_hex = format!("{:#x}", copy_dst.res.as_raw() as usize);
    host.copy_resource(&rawt.res, &copy_dst.res, SRV);
    assert!(host.read(&copy_dst.res, SRV) == pattern(27, len), "the host's copy landed");
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: Some(true),
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 10, "the copy traced and described", |b| {
        b["copies_ring"].as_array().is_some_and(|r| {
            r.iter().any(|c| c["src"]["resource"] == raw_hex && c["dst"]["resource"] == dst_hex && c["dst"]["desc"].is_object() && !c["queue"].is_null())
        })
    });
    let c = b["copies_ring"].as_array().unwrap().iter().find(|c| c["src"]["resource"] == raw_hex && c["dst"]["resource"] == dst_hex).cloned().unwrap();
    assert_eq!(
        (
            c["kind"].clone(),
            c["box"].clone(),
            c["list_type"].clone(),
            c["queue_type"].clone(),
            c["src"]["subresource"].clone(),
            c["dst"]["subresource"].clone()
        ),
        (json!("CopyResource"), json!(false), json!("direct"), json!("direct"), json!("all"), json!("all")),
        "{c}"
    );
    assert_eq!(
        (c["src"]["desc"]["width"].clone(), c["src"]["desc"]["mips"].clone(), c["dst"]["desc"]["height"].clone(), c["dst"]["desc"]["dxgi_format"].clone()),
        (json!(512), json!(12), json!(2048), json!(DXGI_FORMAT_BC7_UNORM_SRGB.0)),
        "{c}"
    );
    assert!(b["copies_seen"].as_u64().unwrap() >= 1);
    // (and a buffer-to-texture upload of the host's, as CopyTextureRegion with a buffer source)
    assert!(
        b["copies_ring"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "CopyTextureRegion" && c["src"]["subresource"] == "buffer" && c["dst"]["resource"] == dst_hex),
        "{}",
        b["copies_ring"]
    );
    println!("copies ring: {} entries, the host's CopyResource: {c}", b["copies_ring"].as_array().unwrap().len());
    // the deck ring (`board copies --deck`): the same copy is there (both sides deck-shaped), with its present
    let dc = b["copies_deck"].as_array().unwrap().iter().find(|c| c["src"]["resource"] == raw_hex && c["dst"]["resource"] == dst_hex).cloned().unwrap();
    assert_eq!((dc["kind"].clone(), dc["present"].as_u64().is_some()), (json!("CopyResource"), true), "{dc}");
    assert!(b["deck_seen"].as_u64().unwrap() >= 1);
    assert_eq!(b["deck_shape"], json!([512, 2048]));
    // the deck-shaped uploads subset (`board uploads --deck`): the composite (described as 512 x 2048) is in it with
    // its presents, the host's back buffers (160 x 120) are not
    let ud = b["uploads_deck"].as_array().unwrap();
    let e = ud.iter().find(|e| e["resource"] == comp_hex).cloned().unwrap();
    assert!(e["first_present"].as_u64().unwrap() <= e["last_present"].as_u64().unwrap(), "{e}");
    assert!(ud.iter().all(|e| e["desc"].is_null() || (e["desc"]["width"] == 512 && e["desc"]["height"] == 2048)), "{ud:#?}");
    assert!(b["main_made_at"].as_u64().is_some() && b["present"].as_u64().unwrap() >= b["main_made_at"].as_u64().unwrap());

    // a target from the raw resource pointer (`board target --resource 0x...`: the equip-time results the game fills by
    // a copy have no runtime texture object): accepted only with a live owner, a RenderCore2 object or a runtime
    // texture naming the resource, found by a scan for the pointer value. The host's texture with no fakes at all:
    // refused; with a RenderCore2-like fake naming it: a target, its state the one the uploads table saw the host's
    // upload leave it in (COPY_DEST -> SRV) or the game's own barrier, written and read back; errors for what is not
    // a resource
    host.run(40);
    assert!(target("resource:0x10", None)["error"].as_str().unwrap().contains("not a pointer"));
    assert!(target(&format!("resource:{:#x}", &*fin_fakes.rc2 as *const _ as usize), None)["error"].as_str().unwrap().contains("not a D3D12 resource"));
    assert!(target("resource:zzz", None)["error"].as_str().unwrap().contains("resource:0x..."));
    let v = target(&format!("resource:{raw_hex}"), None);
    assert_eq!(v["ok"], true, "{v}");
    assert!(target(&format!("resource:{raw_hex}"), None)["error"].as_str().unwrap().contains("being checked already"));
    let b = until(&mut host, &pipe, 60, "the raw target without an owner refused", |b| b["raw_pending"].is_null());
    assert!(b["raw_last"].as_str().unwrap().contains("no live owner: the game may have released it"), "{}", b["raw_last"]);
    assert_eq!(b["manual_targets"], 0, "{}", b["raw_last"]);
    println!("raw without an owner: {}", b["raw_last"]);
    // an owner: a RenderCore2-like object naming the resource at +0x118 (the texture fake beside it names nothing)
    let mut own_fakes = Fakes::new("characters/customization/reskate/board_bottomart/raw_owner", rawt.res.as_raw() as usize).same_class(&fakes);
    own_fakes.texture[layout::TEX_D3D12 / 8] = 0;
    let own_fakes = std::hint::black_box(own_fakes);
    let v = target(&format!("resource:{raw_hex}"), None);
    assert_eq!(v["ok"], true, "{v}");
    let b = until(&mut host, &pipe, 60, "the raw target with an owner accepted", |b| b["raw_pending"].is_null() && b["manual_targets"] == 1);
    println!("raw with an owner: {}", b["raw_last"]);
    let b = until(&mut host, &pipe, 10, "the raw target described", |b| {
        b["targets"].as_array().unwrap().iter().any(|t| t["kind"] == "manual" && !t["dxgi_format"].is_null())
    });
    let rt = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "manual").cloned().unwrap();
    assert_eq!(
        (rt["resource"].clone(), rt["raw"].clone(), rt["texture"].clone(), rt["width"].clone(), rt["height"].clone(), rt["mips"].clone()),
        (json!(raw_hex), json!(true), Value::Null, json!(512), json!(2048), json!(12)),
        "{rt}"
    );
    assert_eq!(
        (rt["owner"]["kind"].clone(), rt["owner"]["address"].clone(), rt["owner"]["alive"].clone()),
        (json!("RenderCore2"), json!(format!("{:#x}", &*own_fakes.rc2 as *const _ as usize)), json!(true)),
        "{rt}"
    );
    assert_eq!((rt["rc2_bind"].clone(), rt["rc2_state"].clone()), (json!(8), json!(8)), "{rt}");
    // (the host's upload of its bytes was just now: the COPY_DEST ring merged into its watch slot says the same as the
    // uploads table would, as the game's own barrier; the table's state carries once that upload is older than the
    // ring's merge window, as in the game when the target is added a while after the equip)
    assert!(rt["state"]["source"] == "the game's own barriers on it" || rt["state"]["source"] == "the last write the game ended on it (board uploads)", "{rt}");
    assert!(rt["state"]["states"].as_str().unwrap().starts_with("0xc0"), "{rt}");
    assert_eq!(rt["refused"], "", "{rt}");
    let r = pattern(26, len);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    art(&pipe, "pattern r", &r, Some(true));
    let b = until(&mut host, &pipe, 20, "the raw target written", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    assert!(host.read(&rawt.res, SRV) == r, "the raw target holds the art");
    assert!(host.read(&deck.res, SRV) == r);
    assert_eq!(b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "manual").unwrap()["refused"], "", "{}", b["last_upload"]);
    let b = until(&mut host, &pipe, 10, "the raw target's read-back", |b| {
        b["verify"]["targets"].as_array().is_some_and(|t| t.iter().any(|x| x["resource"] == raw_hex && x["verified_now"] != "not yet"))
    });
    assert_eq!(b["verify"]["targets"].as_array().unwrap().iter().find(|x| x["resource"] == raw_hex).unwrap()["verified_now"], "match", "{}", b["verify"]);
    // the owner gone (the game released the texture): the upload refuses, the worker drops it
    let mut own_fakes = own_fakes;
    own_fakes.rc2[layout::RC2_D3D12 / 8] = 0;
    let own_fakes = std::hint::black_box(own_fakes);
    let before = host.read(&rawt.res, SRV);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    art(&pipe, "pattern r2", &pattern(29, len), Some(true));
    until(&mut host, &pipe, 20, "art r2 written around the orphaned raw target", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    assert!(host.read(&rawt.res, SRV) == before, "the orphaned raw target was not written");
    assert!(host.read(&deck.res, SRV) == pattern(29, len), "the main was written meanwhile");
    let b = until(&mut host, &pipe, 10, "the orphaned raw target dropped", |b| b["targets"].as_array().unwrap().iter().all(|t| t["kind"] != "manual"));
    assert_eq!(b["desc_leaks"], 0, "every reference the hooks took for the describe queue was released");
    println!(
        "orphaned raw target: {}",
        b["log"].as_array().and_then(|l| l.iter().rev().find(|x| x.as_str().is_some_and(|s| s.contains("no live owner")))).cloned().unwrap_or_default()
    );
    // the uploads table's owners (`board uploads` looks them up): the composite's runtime texture fake names it
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: Some(true),
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    host.run(100);
    assert_eq!(board_status(&pipe)["owners_pending"], false, "no owner scan without --owners");
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: Some(true),
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: Some(true),
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 60, "the uploads' owners looked up", |b| b["owners_pending"] == false && !b["owners_at_ms_ago"].is_null());
    let e = b["uploads_ring"].as_array().unwrap().iter().find(|e| e["resource"] == comp_hex).cloned().unwrap();
    assert_eq!(
        (e["owner"]["kind"].clone(), e["owner"]["address"].clone()),
        (json!("runtime texture"), json!(format!("{:#x}", &*comp_fakes.texture as *const _ as usize))),
        "{e}"
    );
    let e = b["uploads_ring"].as_array().unwrap().iter().find(|e| e["resource"] == dst_hex).cloned().unwrap();
    assert_eq!(e["owner"], json!("none"), "{e}");
    assert_eq!(target("clear", None)["ok"], true);
    assert_eq!(board_status(&pipe)["manual_targets"], 0);
    host.wait_idle();

    // the world copy (the 512 x 2048 BC1 texture of format code 54 and the known name hash the game re-fills at every
    // equip): found by the census, described as BC1 (DXGI 71), written with the art's BC1 chain (read back as match
    // on 8-byte blocks), peeked and decoded, and written again when the host re-fills it (the game's re-fill seen as
    // COPY_DEST -> SRV by the uploads table); `worldcopy off` drops it
    let bc1_len = board::chain_len_for(board::ChainFormat::Bc1, 512, 2048, 12);
    let wc_bytes = pattern(41, bc1_len);
    let wc = host.texture_of(512, 2048, 12, &wc_bytes, SRV, DXGI_FORMAT_BC1_UNORM);
    let wc_hex = format!("{:#x}", wc.res.as_raw() as usize);
    let mut wc_fakes = Fakes::new("characters/customization/reskate/board_bottomart/worldcopy", wc.res.as_raw() as usize).same_class(&fakes);
    wc_fakes.texture[layout::TEX_NAME_HASH / 8] = board::DEFAULT_WORLDCOPY_HASHES[0];
    wc_fakes.set_tex_u32(layout::TEX_FORMAT, board::WORLDCOPY_FORMAT);
    wc_fakes.set_tex_u32(layout::TEX_ID, 0x5151);
    let wc_fakes = std::hint::black_box(wc_fakes);
    assert_eq!(board_status(&pipe)["worldcopy_on"], true, "on by default");
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: Some(true),
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 60, "the world copy found", |b| b["worldcopies"]["found"] == 1);
    let wt = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "worldcopy").cloned().unwrap();
    assert_eq!(
        (wt["resource"].clone(), wt["hash"].clone(), wt["id"].clone()),
        (json!(wc_hex), json!(format!("{:#x}", board::DEFAULT_WORLDCOPY_HASHES[0])), json!("0x5151")),
        "{wt}"
    );
    let b = until(&mut host, &pipe, 10, "the world copy described", |b| {
        b["targets"].as_array().unwrap().iter().any(|t| t["kind"] == "worldcopy" && !t["dxgi_format"].is_null())
    });
    let wt = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "worldcopy").cloned().unwrap();
    assert_eq!((wt["dxgi_format"].clone(), wt["mips"].clone()), (json!(DXGI_FORMAT_BC1_UNORM.0), json!(12)), "{wt}");
    assert!(wt["state"]["states"].as_str().unwrap().starts_with("0xc0"), "{wt}");
    // the art with a BC1 chain: the world copy written with it, read back as match; without one: refused with why
    let w_art = pattern(42, len);
    let w_bc1 = pattern(43, bc1_len);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    art_with(&pipe, "pattern w (no bc1)", &w_art, Some(&pattern(44, rgba_len)), Some(true));
    let b = until(&mut host, &pipe, 20, "art w written", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    let wt = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "worldcopy").cloned().unwrap();
    assert!(wt["refused"].as_str().unwrap().contains("no bc1 chain"), "{wt}");
    assert!(host.read(&wc.res, SRV) == wc_bytes, "the world copy untouched without a BC1 chain");
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    send(
        &pipe,
        &Request::Board {
            art: Some(BoardArt::new("pattern w", &w_art)),
            keep: Some(true),
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: Some(vec![BoardArt::with_format("pattern w", "rgba8", 12, &pattern(44, rgba_len)), BoardArt::with_format("pattern w", "bc1", 12, &w_bc1)]),
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 20, "the world copy written", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    assert!(host.read(&wc.res, SRV) == w_bc1, "the world copy holds the BC1 chain");
    assert!(host.read(&deck.res, SRV) == w_art);
    assert_eq!(b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "worldcopy").unwrap()["refused"], "", "{}", b["last_upload"]);
    let b = until(&mut host, &pipe, 10, "the world copy's read-back", |b| {
        b["verify"]["targets"].as_array().is_some_and(|t| t.iter().any(|x| x["resource"] == wc_hex && x["verified_now"] != "not yet"))
    });
    assert_eq!(b["verify"]["targets"].as_array().unwrap().iter().find(|x| x["resource"] == wc_hex).unwrap()["verified_now"], "match", "{}", b["verify"]);
    // a peek of the world copy: the first 8-byte block, decoded as BC1
    let peek_wc = proto::request(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: Some(proto::PeekReq { resource: wc_hex.clone(), x: 0, y: 0, mip: 0, n: 4 }),
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
        3000,
    )
    .unwrap();
    assert_eq!(peek_wc["ok"], true, "{peek_wc}");
    let b = until(&mut host, &pipe, 10, "the BC1 peek", |b| b["peek"]["done"] == true && b["peek"]["resource"] == wc_hex);
    let r = &b["peek"]["result"];
    let want: String = w_bc1[..8].iter().map(|x| format!("{x:02x}")).collect();
    assert_eq!((r["family"].clone(), r["texels"][0][0].clone(), r["bytes_per_texel_or_block"].clone()), (json!("bc1"), json!(want), json!(8)), "{r}");
    let dec = caveman_fx::bc7::decode_bc1(w_bc1[..8].try_into().unwrap());
    let want_px: String = dec[5].iter().map(|x| format!("{x:02x}")).collect();
    assert_eq!(r["decoded"][1][1], json!(want_px), "{r}");
    // the game re-fills it: written again, and the time logged
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    host.refill(&wc.res, &pattern(45, bc1_len), SRV, SRV);
    let b = until(&mut host, &pipe, 20, "the world copy written again after the re-fill", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    host.run(40);
    assert!(host.read(&wc.res, SRV) == w_bc1, "the world copy holds our chain again");
    let wt = b["targets"].as_array().unwrap().iter().find(|t| t["kind"] == "worldcopy").cloned().unwrap();
    assert!(wt["refill_present"].as_u64().is_some() && wt["written_present"].as_u64().unwrap() > wt["refill_present"].as_u64().unwrap(), "{wt}");
    let b = until(&mut host, &pipe, 10, "the re-write logged", |b| {
        b["log"].as_array().is_some_and(|l| l.iter().any(|x| x.as_str().is_some_and(|s| s.contains("written again") && s.contains("after the game's re-fill"))))
    });
    println!(
        "world copy: {}",
        b["log"].as_array().unwrap().iter().rev().find(|x| x.as_str().is_some_and(|s| s.contains("after the game's re-fill"))).cloned().unwrap_or_default()
    );
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: Some(false),
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = board_status(&pipe);
    assert_eq!((b["worldcopy_on"].clone(), b["worldcopies"]["found"].clone()), (json!(false), json!(0)), "{}", b["worldcopies"]);
    host.wait_idle();

    // the slices (the tile arrays the world samples): a 256 x 256 BC7 array of 4 slices filled by copies (traced into
    // the deck ring: a tile array's slices), two of them holding tiles of our art (mip 0 tile (1, 3) and mip 1 tile
    // (0, 2)), two random; `board slices` lists the array, `board slices match` finds the two, `board tiles` writes
    // the next art's tiles into them (read back as match, the other slices untouched), and a slice the host writes
    // again is written again by the DLL (kept)
    let t_art = pattern(31, len);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    art(&pipe, "pattern t", &t_art, Some(true));
    until(&mut host, &pipe, 20, "art t written", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    let sizes = board::chain_sizes(board::ChainFormat::Bc7, 512, 2048, 12);
    let tile_of = |chain: &[u8], mip: usize, tx: usize, ty: usize| -> Vec<u8> {
        let start: usize = sizes[..mip].iter().sum();
        let full = ((512usize >> mip).max(1)).div_ceil(4);
        let mut out = Vec::new();
        for r in 0..64 {
            let at = start + ((ty * 64 + r) * full + tx * 64) * 16;
            out.extend_from_slice(&chain[at..at + 64 * 16]);
        }
        out
    };
    let slices = vec![pattern(32, 65536), pattern(33, 65536), tile_of(&t_art, 0, 1, 3), tile_of(&t_art, 1, 0, 2)];
    let arr = host.texture_array(256, &slices, SRV);
    let arr_hex = format!("{:#x}", arr.res.as_raw() as usize);
    host.run(80);
    let b = until(&mut host, &pipe, 10, "the tile array's slices in the report", |b| {
        b["slices"]["arrays"].as_array().is_some_and(|a| a.iter().any(|x| x["resource"] == arr_hex && x["slices"].as_array().is_some_and(|s| s.len() == 4)))
    });
    let sa = b["slices"]["arrays"].as_array().unwrap().iter().find(|x| x["resource"] == arr_hex).cloned().unwrap();
    assert_eq!((sa["dxgi_format"].clone(), sa["array"].clone(), sa["state"].clone()), (json!(DXGI_FORMAT_BC7_UNORM_SRGB.0), json!(4), json!("0xc0")), "{sa}");
    println!("slices report: {sa}");
    let slices_req = |what: &str, state: Option<&str>| {
        send(
            &pipe,
            &Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: Some(what.to_string()),
                slices_state: state.map(str::to_string),
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            },
        )
    };
    slices_req("match", None);
    let b = until(&mut host, &pipe, 20, "the slices matched", |b| b["slice_job"].is_null() && b["slices_result"]["kind"] == "match");
    println!("slices match: {}", b["slices_result"]);
    let m = b["slice_matches"].as_array().unwrap();
    assert_eq!(m.len(), 2, "{}", b["slices_result"]);
    let find = |slice: u64| m.iter().find(|x| x["resource"] == arr_hex && x["slice"] == slice).cloned().unwrap();
    assert_eq!((find(2)["mip"].clone(), find(2)["tx"].clone(), find(2)["ty"].clone(), find(2)["error"].clone()), (json!(0), json!(1), json!(3), json!(0.0)));
    assert_eq!((find(3)["mip"].clone(), find(3)["tx"].clone(), find(3)["ty"].clone(), find(3)["error"].clone()), (json!(1), json!(0), json!(2), json!(0.0)));
    assert!(b["slices_result"]["items"].as_array().unwrap().iter().filter(|i| i["matched"] == false).count() == 2);
    // the tiles of the next art written into the matched slices, read back, the other slices untouched
    let u_art = pattern(34, len);
    let uploads = board_status(&pipe)["uploads"].as_u64().unwrap();
    art(&pipe, "pattern u", &u_art, Some(true));
    until(&mut host, &pipe, 20, "art u written", |b| b["uploads"].as_u64().unwrap_or(0) > uploads);
    slices_req("write", None);
    let b = until(&mut host, &pipe, 20, "the tiles written", |b| b["slice_job"].is_null() && b["slices_result"]["kind"] == "write");
    println!("tiles: {}", b["slices_result"]);
    assert_eq!(b["slices_result"]["written"], 2, "{}", b["slices_result"]);
    assert!(b["slice_matches"].as_array().unwrap().iter().all(|x| x["verdict"] == "match"), "{}", b["slice_matches"]);
    let all = host.read(&arr.res, SRV);
    assert!(all[2 * 65536..3 * 65536] == tile_of(&u_art, 0, 1, 3)[..], "slice 2 holds the new art's tile");
    assert!(all[3 * 65536..4 * 65536] == tile_of(&u_art, 1, 0, 2)[..], "slice 3 holds the new art's tile");
    assert!(all[..65536] == slices[0][..] && all[65536..2 * 65536] == slices[1][..], "the other slices are untouched");
    assert_eq!(b["tiles_keep"], true);
    // the game writes slice 2 again: the DLL writes it again
    host.upload_slice(&arr.res, 2, 256, &pattern(35, 65536), SRV);
    let b = until(&mut host, &pipe, 20, "the slice written again", |b| b["tiles_rewrites"].as_u64().unwrap_or(0) >= 1 && b["slice_job"].is_null());
    host.run(60);
    assert!(host.read(&arr.res, SRV)[2 * 65536..3 * 65536] == tile_of(&u_art, 0, 1, 3)[..], "slice 2 holds our tile again");
    println!("tiles kept: rewrites {}", b["tiles_rewrites"]);
    slices_req("clear", None);
    assert_eq!(board_status(&pipe)["tiles_keep"], false);
    // a state by hand for the arrays
    slices_req("match", Some("srv"));
    let b = until(&mut host, &pipe, 20, "the slices matched with a state by hand", |b| b["slice_job"].is_null() && b["slices_result"]["kind"] == "match");
    assert_eq!((b["slices_state"].clone(), b["slice_matches"].as_array().unwrap().len()), (json!("0xc0"), 2));
    slices_req("clear", Some("auto"));
    assert!(board_status(&pipe)["slices_state"].is_null());

    // the peek (`board peek`): a 4 x 4 block of the main's mip 0 at (4, 0): the chain's second block; and
    // 2 x 2 texels of the composite's mip 1 at (1, 0): what the host reads back there
    let peek = |resource: &str, x: u32, y: u32, mip: u32, n: u32| {
        let v = proto::request(
            &pipe,
            &Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: Some(proto::PeekReq { resource: resource.to_string(), x, y, mip, n }),
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            },
            3000,
        )
        .unwrap();
        assert_eq!(v["ok"], true, "{v}");
    };
    let main_hex = format!("{:#x}", deck.res.as_raw() as usize);
    let small_hex = format!("{:#x}", small.res.as_raw() as usize);
    peek(&main_hex, 5, 1, 0, 4);
    let b = until(&mut host, &pipe, 10, "the BC7 peek", |b| b["peek"]["done"] == true);
    let r = &b["peek"]["result"];
    assert_eq!(
        (r["left"].clone(), r["top"].clone(), r["width"].clone(), r["height"].clone(), r["family"].clone()),
        (json!(4), json!(0), json!(4), json!(4), json!("bc7")),
        "{r}"
    );
    let want: String = u_art[16..32].iter().map(|x| format!("{x:02x}")).collect();
    assert_eq!(r["texels"][0][0], json!(want), "{r}");
    let comp_now = host.read(&comp.res, SRV);
    peek(&comp_hex, 1, 0, 1, 2);
    let b = until(&mut host, &pipe, 10, "the RGBA8 peek", |b| b["peek"]["done"] == true && b["peek"]["mip"] == 1);
    let r = &b["peek"]["result"];
    assert_eq!(
        (r["left"].clone(), r["top"].clone(), r["width"].clone(), r["height"].clone(), r["family"].clone()),
        (json!(1), json!(0), json!(2), json!(2), json!("rgba8")),
        "{r}"
    );
    // mip 1 is 256 x 1024: it starts after mip 0's bytes, texel (x, y) at (y * 256 + x) * 4
    let mip1 = 512 * 2048 * 4;
    for y in 0..2u32 {
        for x in 0..2u32 {
            let at = mip1 + ((y * 256 + 1 + x) * 4) as usize;
            let want: String = comp_now[at..at + 4].iter().map(|v| format!("{v:02x}")).collect();
            assert_eq!(r["texels"][y as usize][x as usize], json!(want), "{r}");
        }
    }
    let v = proto::request(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: Some(proto::PeekReq { resource: small_hex.clone(), x: 0, y: 0, mip: 0, n: 4 }),
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
        3000,
    )
    .unwrap();
    assert!(v["error"].as_str().unwrap().contains("not a target"), "{v}");

    // the colour hunt (`board hunt`): a 512 x 2048 RGBA8 render target of the host's cleared to mint green, its
    // RENDER_TARGET -> SRV barrier seen (so its state is known); the hunt over every texture the DLL knows finds it
    // (all three blocks, the mean the colour) and not the main (a BC7 pattern); a hit's owner looked up on request
    let hue = host.rgba_target(512, 2048, 1);
    let hue_hex = format!("{:#x}", hue.res.as_raw() as usize);
    host.wait_idle();
    host.allocator.Reset().unwrap();
    host.list.Reset(&host.allocator, None).unwrap();
    let rtv = host.rtv_heap.GetCPUDescriptorHandleForHeapStart();
    let view = D3D12_RENDER_TARGET_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_RTV { MipSlice: 0, PlaneSlice: 0 } },
    };
    host.device.CreateRenderTargetView(&hue.res, Some(&view), rtv);
    host.list.ClearRenderTargetView(rtv, &[60.0 / 255.0, 240.0 / 255.0, 138.0 / 255.0, 1.0], None);
    host.list.Close().unwrap();
    host.queue.ExecuteCommandLists(&[Some(host.list.cast().unwrap())]);
    host.wait_idle();
    host.transitions.push((hue.res.clone(), 0x4, SRV));
    host.run(80);
    let hunt = |rgb: [u8; 3], tolerance: Option<u8>, everything: bool| {
        send(
            &pipe,
            &Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: Some(proto::HuntReq { rgb, tolerance, everything, assume: None, census: false, targets: false }),
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            },
        )
    };
    hunt([60, 240, 138], None, false);
    let b = until(&mut host, &pipe, 30, "the hunt done", |b| b["hunt"]["done"] == true);
    println!(
        "hunt: {} items, {} read, {} unknown, {} ms, hits {}",
        b["hunt"]["items"], b["hunt"]["read"], b["hunt"]["unknown_state"], b["hunt"]["ms"], b["hunt"]["hits"]
    );
    let hits = b["hunt"]["hits"].as_array().unwrap();
    let hit = hits.iter().find(|h| h["resource"] == hue_hex).cloned().unwrap();
    assert_eq!(
        (hit["hits"].clone(), hit["width"].clone(), hit["height"].clone(), hit["dxgi_format"].clone(), hit["deck_shaped"].clone(), hit["state"].clone()),
        (json!([true, true, true]), json!(512), json!(2048), json!(DXGI_FORMAT_R8G8B8A8_UNORM.0), json!(true), json!("0xc0")),
        "{hit}"
    );
    assert_eq!((hit["encodings"].clone(), hit["tolerance"].clone()), (json!(["srgb"]), json!(24)), "{hit}");
    let mean = hit["mean"].as_array().unwrap();
    assert!(
        (mean[0].as_i64().unwrap() - 60).abs() <= 1 && (mean[1].as_i64().unwrap() - 240).abs() <= 1 && (mean[2].as_i64().unwrap() - 138).abs() <= 1,
        "{hit}"
    );
    assert!(!hits.iter().any(|h| h["resource"] == main_hex), "the main's pattern is not the colour: {hits:?}");
    assert!(b["hunt"]["read"].as_u64().unwrap() >= 2 && b["hunt"]["items"].as_u64().unwrap() >= b["hunt"]["read"].as_u64().unwrap());
    // (the random BC7 patterns may hit as the colour premultiplied by some texel's alpha: listed apart, never hits)
    assert!(hits.iter().all(|h| h["resource"] == hue_hex), "only the mint texture is a hit: {hits:?}");
    // `--assume srv`: a texture of unknown state (one the DLL saw only as a copy's source: no write of it ever ended
    // with a barrier, so the uploads table never learnt its state; it is in SRV, as assumed) is read too, except
    // render targets; more read, the same hit
    let blank = host.blank_texture(512, 2048, 12);
    let blank_hex = format!("{:#x}", blank.res.as_raw() as usize);
    host.copy_resource(&blank.res, &copy_dst.res, SRV);
    host.run(60);
    hunt([60, 240, 138], None, false);
    let b = until(&mut host, &pipe, 30, "the hunt with the blank texture done", |b| {
        b["hunt"]["done"] == true && b["hunt"]["assume"].is_null() && b["hunt"]["items"].as_u64().unwrap_or(0) > 0
    });
    let unknown = b["hunt"]["unknown_state"].as_u64().unwrap();
    assert!(unknown >= 1, "{}", b["hunt"]);
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: Some(proto::HuntReq { rgb: [60, 240, 138], tolerance: None, everything: false, assume: Some("srv".into()), census: false, targets: false }),
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 30, "the assumed hunt done", |b| b["hunt"]["done"] == true && b["hunt"]["assume"] == "0xc0");
    println!("hunt --assume srv: {} read, {} assumed, {} unknown", b["hunt"]["read"], b["hunt"]["assumed_read"], b["hunt"]["unknown_state"]);
    assert!(b["hunt"]["assumed_read"].as_u64().unwrap() >= 1 && b["hunt"]["unknown_state"].as_u64().unwrap() < unknown, "{}", b["hunt"]);
    assert!(b["hunt"]["stuck"].is_null() && b["hunt"]["stage"] == 2 && b["hunt"]["lock_misses"].is_u64(), "{}", b["hunt"]);
    let hits = b["hunt"]["hits"].as_array().unwrap();
    assert!(hits.iter().any(|h| h["resource"] == hue_hex && h["assumed"] == false), "{hits:?}");
    assert!(hits.iter().all(|h| h["resource"] == hue_hex), "only the mint texture is a hit: {hits:?}");
    let _ = blank_hex;
    // a colour nothing holds: no hits, the deck-shaped misses listed with their means
    hunt([201, 33, 177], Some(4), false);
    let b = until(&mut host, &pipe, 30, "the second hunt done", |b| b["hunt"]["done"] == true && b["hunt"]["rgb"] == json!([201, 33, 177]));
    assert!(b["hunt"]["hits"].as_array().unwrap().is_empty(), "{}", b["hunt"]["hits"]);
    assert!(b["hunt"]["deck_misses"].as_array().unwrap().iter().any(|m| m["resource"] == hue_hex), "{}", b["hunt"]["deck_misses"]);
    // the owners of the hits (none: a bare texture of the host's)
    // the linearised encoding: the texture holds 11/222/64 when the colour is looked for as 3cf08a
    host.wait_idle();
    host.allocator.Reset().unwrap();
    host.list.Reset(&host.allocator, None).unwrap();
    // (the host's frames put the back buffer's view in the heap's one slot: the mint texture's view again)
    host.device.CreateRenderTargetView(&hue.res, Some(&view), rtv);
    host.list.ResourceBarrier(&[barrier(&hue.res, SRV, 0x4)]);
    host.list.ClearRenderTargetView(rtv, &[11.0 / 255.0, 222.0 / 255.0, 64.0 / 255.0, 1.0], None);
    host.list.ResourceBarrier(&[barrier(&hue.res, 0x4, SRV)]);
    host.list.Close().unwrap();
    host.queue.ExecuteCommandLists(&[Some(host.list.cast().unwrap())]);
    host.wait_idle();
    let now_hue = host.read(&hue.res, SRV);
    assert_eq!(&now_hue[..4], &[11, 222, 64, 255], "the host's clear to the linearised bytes landed");
    host.run(40);
    hunt([60, 240, 138], None, true);
    until(&mut host, &pipe, 30, "the third hunt done", |b| b["hunt"]["done"] == true && b["hunt"]["rgb"] == json!([60, 240, 138]));
    send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: None,
            on: None,
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: Some(true),
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: Some("hunt".into()),
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    let b = until(&mut host, &pipe, 60, "the hits' owners", |b| {
        b["owners_pending"] == false && b["hunt"]["hits"].as_array().is_some_and(|h| h.iter().any(|x| x["resource"] == hue_hex && !x["owner"].is_null()))
    });
    let hit = b["hunt"]["hits"].as_array().unwrap().iter().find(|x| x["resource"] == hue_hex).cloned().unwrap();
    assert_eq!((hit["owner"].clone(), hit["encodings"].clone()), (json!("none"), json!(["linear"])), "{hit}");
    assert_eq!(target("clear", None)["ok"], true);
    host.wait_idle();
    let errors = debug_errors(&host.device);
    assert!(errors.is_empty(), "the writes by hand, of the final and of the tiles: {errors:#?}");

    // the effects: on until off, a pulse
    let v = send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: Some("aura".into()),
            on: Some(true),
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    assert_eq!(v["changed"]["fx"], "fx.cave.aura");
    let aura = v["fx"].as_array().unwrap().iter().find(|f| f["name"] == "fx.cave.aura").cloned().unwrap();
    assert!(aura["override_ms"].as_u64().unwrap() > 3_600_000, "on until turned off: {aura}");
    let v = send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: Some("fx.cave.aura".into()),
            on: Some(false),
            ms: None,
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    assert_eq!(v["fx"].as_array().unwrap().iter().find(|f| f["name"] == "fx.cave.aura").unwrap()["override_ms"], 0);
    let v = send(
        &pipe,
        &Request::Board {
            art: None,
            keep: None,
            state: None,
            find: None,
            fx: Some("impact".into()),
            on: None,
            ms: Some(150),
            verify: None,
            arts: None,
            siblings: None,
            state_for: None,
            composite_common: None,
            target: None,
            uploads: None,
            finals: None,
            copies: None,
            peek: None,
            force: None,
            owners: None,
            slices: None,
            slices_state: None,
            hunt: None,
            owners_of: None,
            worldcopy: None,
            worldcopy_hashes: None,
            hunt_abort: None,
        },
    );
    assert_eq!(v["changed"]["ms"], 150);
    assert!(
        proto::request(
            &pipe,
            &Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: Some("impact".into()),
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None
            },
            3000
        )
        .unwrap()["ok"]
            == false
    );

    // nothing of the DLL's in the host's state: the debug layer's verdict, then unload
    assert!(host.read(&decoy.res, SRV) == decoy_bytes, "the decoy is still untouched");
    let errors = debug_errors(&host.device);
    // the verdict means something: a wrong state set by hand (COMMON, while the texture is in 0xc0) is reported
    if let Ok(q) = host.device.cast::<ID3D12InfoQueue>() {
        assert!(errors.is_empty(), "D3D12 debug layer errors: {errors:#?}");
        send(
            &pipe,
            &Request::Board {
                art: None,
                keep: None,
                state: Some("common".into()),
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            },
        );
        let n = board_status(&pipe)["uploads"].as_u64().unwrap();
        art(&pipe, "wrong state on purpose", &pattern(9, len), Some(false));
        until(&mut host, &pipe, 20, "the wrong-state upload", |b| b["uploads"].as_u64() == Some(n + 1));
        host.run(50);
        let caught = debug_errors(&host.device);
        println!("a wrong state by hand: {} debug layer message(s), e.g. {:?}", caught.len(), caught.first());
        assert!(!caught.is_empty(), "the debug layer checks the states the DLL's barriers claim");
        // put things right: the texture is in COMMON now (the DLL's barriers ended there); back to 0xc0, automatic
        host.transitions.push((deck.res.clone(), 0, SRV));
        host.run(50);
        send(
            &pipe,
            &Request::Board {
                art: None,
                keep: None,
                state: Some("auto".into()),
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            },
        );
        q.ClearStoredMessages();
    }
    let errors = debug_errors(&host.device);
    send(&pipe, &Request::Unload);
    let name = win::wide("caveman_fx.dll");
    let t = Instant::now();
    while !win::GetModuleHandleW(name.as_ptr()).is_null() {
        host.run(20);
        assert!(t.elapsed() < Duration::from_secs(15), "the DLL did not unload");
    }
    host.run(100);
    assert!(errors.is_empty(), "D3D12 debug layer errors: {errors:#?}");
    let errors = debug_errors(&host.device);
    assert!(errors.is_empty(), "D3D12 debug layer errors after unload: {errors:#?}");
    host.wait_idle();
    drop((
        decoy_fakes,
        fakes,
        small,
        decoy,
        deck,
        comp,
        comp_fakes,
        sibling,
        sibling_fakes,
        table,
        hand,
        hand_fakes,
        fin,
        fin_fakes,
        rawt,
        copy_dst,
        own_fakes,
        arr,
        hue,
        blank,
        wc,
        wc_fakes,
    ));
}
