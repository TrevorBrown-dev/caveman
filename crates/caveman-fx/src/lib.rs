//! Caveman FX: `caveman_fx.dll`, loaded into skate. (Skate.exe build 20260929, next to ReSkate), and what the trainer
//! shares with it. The DLL answers for Caveman's own trigger states (`Animation/Caveman/FX.<Name>` BoolAssets that
//! FX decks trigger on) from rules over the skater's real states: kickflips, grinds, manuals, landings.
//!
//! - [`game`]: the build it knows (hash, addresses, code bytes) and the checks;
//! - [`hook`]: the vtable-slot hook on `BoolAsset`'s bool view (no code patched);
//! - [`scan`]: finding the state assets in memory; [`skater`]: the skater's context and state values;
//! - [`rules`]: the rules engine; [`worker`]: the thread that runs it all; [`pipe`] and [`proto`]: the trainer's pipe;
//! - the DLL's own drawing: [`overlay`] (hooks on the swap chain's Present, D3D12 quads into the back buffer),
//!   [`scene`] (the camera and the board from game memory), [`bars`] (the equaliser bars on the deck), [`audio`]
//!   (WASAPI loopback bands), [`vhook`] (vtable-slot hooks) and [`rtti`] (vtables by class name);
//! - [`board`]: The Caveman, one deck whose graphic the DLL rewrites on the GPU (and whose fx.cave.* effects the trainer
//!   switches).
//!
//! On load (DllMain) it only starts the worker thread. See docs/fx-hook-plan.md.

#![recursion_limit = "512"]

pub mod alloctrace;
pub mod audio;
pub mod bars;
pub mod bc7;
pub mod bc7_tables;
pub mod board;
pub mod copies;
pub mod depth;
pub mod diag;
pub mod game;
pub mod hook;
pub mod material;
pub mod memguard;
pub mod memwatch;
pub mod overlay;
pub mod pip;
pub mod pipe;
pub mod proto;
pub mod proxy;
pub mod rim;
pub mod rtti;
pub mod rules;
pub mod scan;
pub mod scene;
pub mod sha256;
pub mod skater;
pub mod stagering;
pub mod tick;
pub mod vhook;
pub mod video;
pub mod win;
pub mod worker;

use std::ffi::c_void;
use std::sync::atomic::Ordering;

/// The DLL entry point: starts the worker thread on attach (nothing else under the loader lock); on a FreeLibrary
/// that did not come from the worker's own unload, puts the vtable slot back.
///
/// # Safety
/// Called by the Windows loader.
#[no_mangle]
pub unsafe extern "system" fn DllMain(module: win::HANDLE, reason: u32, reserved: *mut c_void) -> win::BOOL {
    const PROCESS_DETACH: u32 = 0;
    const PROCESS_ATTACH: u32 = 1;
    match reason {
        PROCESS_ATTACH => {
            worker::MODULE.store(module as usize, Ordering::Release);
            let thread = win::CreateThread(std::ptr::null_mut(), 0, worker::thread_main, std::ptr::null_mut(), 0, std::ptr::null_mut());
            if !thread.is_null() {
                win::CloseHandle(thread);
            }
        }
        PROCESS_DETACH if reserved.is_null() => {
            let _ = tick::uninstall();
            let _ = hook::uninstall();
            overlay::emergency_uninstall();
        }
        _ => {}
    }
    1
}

/// Test support (the offline host test): installs the hook on a vtable slot the caller owns. Refused unless the DLL
/// is dormant (never in the game). 0 ok, -1 refused, -2 the install failed.
///
/// # Safety
/// `slot` must point at a pointer-sized, 8-byte aligned slot holding a function of the [`hook::Getter`] shape.
#[no_mangle]
pub unsafe extern "C" fn cmfx_test_install(slot: *mut usize) -> i32 {
    if worker::PHASE.load(Ordering::Acquire) != worker::Phase::Dormant as u8 || slot.is_null() {
        return -1;
    }
    let Some(original) = win::read_ptr(slot as usize) else { return -2 };
    match hook::install(slot as usize, original) {
        Ok(()) => 0,
        Err(_) => -2,
    }
}

/// Test support: makes `view` one of Caveman's views, for fx index `fx`, with `value`. Same refusal as above.
#[no_mangle]
pub extern "C" fn cmfx_test_map(view: usize, fx: u32, value: u32) -> i32 {
    if worker::PHASE.load(Ordering::Acquire) != worker::Phase::Dormant as u8 || !hook::installed() || fx as usize >= hook::MAX_FX {
        return -1;
    }
    hook::set_views(&[(view, fx as u8)]);
    hook::set_value(fx as usize, value != 0);
    0
}

/// Whether this process is a test host (not the game): the overlay test exports work only there.
fn test_host() -> bool {
    let exe = win::module_path(std::ptr::null_mut()).unwrap_or_default();
    !std::path::Path::new(&exe).file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case("skate.exe"))
}

/// Test support (the offline host test): gives the overlay a scene instead of the game's: the camera's world matrix
/// (16 floats: right, up, back, position rows) and vertical field of view in degrees, and the board's (right, up,
/// forward, position; null: no board). A null camera clears it. Refused in Skate.exe: 0 ok, -1 refused, -2 the
/// matrices are not usable.
///
/// # Safety
/// `camera` and `board` must each be null or point at 16 floats.
#[no_mangle]
pub unsafe extern "C" fn cmfx_test_scene(camera: *const f32, fov: f32, board: *const f32) -> i32 {
    if !test_host() {
        return -1;
    }
    let mut slot = scene::TEST_SCENE.lock().unwrap_or_else(|e| e.into_inner());
    if camera.is_null() {
        *slot = None;
        return 0;
    }
    let m = |p: *const f32| -> [f32; 16] { std::array::from_fn(|i| *p.add(i)) };
    let Some(cam) = bars::Camera::from_matrix(&m(camera), fov) else { return -2 };
    let board = if board.is_null() {
        None
    } else {
        match bars::Board::from_matrix(&m(board)) {
            Some(b) => Some(b),
            None => return -2,
        }
    };
    *slot = Some(bars::Scene { camera: cam, board });
    0
}

/// Test support: the overlay makes its dummy device from this Agility SDK (`version`, `path`: a NUL-terminated
/// folder), as the test host's own device is, so its vtables are the host's. Refused in Skate.exe and once the
/// overlay is installed: 0 ok, -1 refused.
///
/// # Safety
/// `path` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn cmfx_test_sdk(version: u32, path: *const std::ffi::c_char) -> i32 {
    if !test_host() || overlay::installed() || path.is_null() {
        return -1;
    }
    let path = std::ffi::CStr::from_ptr(path).to_string_lossy().into_owned();
    *overlay::TEST_SDK.lock().unwrap_or_else(|e| e.into_inner()) = Some((version, path));
    0
}

/// Test support (the offline board test): the values The Caveman's texture chain is checked against, standing in for
/// the game's (`board::Ids`: the TextureAsset vtable, its TypeInfo and descriptor, the runtime texture's vtable,
/// RenderCore2's resource vtable), so a host's fake objects can be found as the game's are. Refused in Skate.exe: 0 ok,
/// -1 refused.
#[no_mangle]
pub extern "C" fn cmfx_test_board(asset_vtable: usize, type_info: usize, descriptor: usize, texture_vtable: usize, rc2_vtable: usize) -> i32 {
    if !test_host() || type_info == 0 || descriptor == 0 || texture_vtable == 0 {
        return -1;
    }
    board::set_test_ids(board::Ids { asset_vtable, type_info, descriptor, texture_vtable, rc2_vtable });
    0
}

/// Test support: treats `vtable` as Streamline's swap chain wrapper vtable and hooks its Present slots (a test host's
/// fake wrapper). Refused in Skate.exe and before the overlay is installed: 0 ok, -1 refused, -2 the hook failed.
///
/// # Safety
/// `vtable` must be a vtable whose slots 8 and 22 are an `IDXGISwapChain1`'s `Present` and `Present1`.
#[no_mangle]
pub unsafe extern "C" fn cmfx_test_wrapper(vtable: usize) -> i32 {
    if !test_host() || !overlay::installed() {
        return -1;
    }
    match overlay::hook_wrapper_for_test(vtable) {
        Ok(()) => 0,
        Err(_) => -2,
    }
}
