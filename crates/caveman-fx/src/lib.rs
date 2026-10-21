//! Caveman FX: `caveman_fx.dll`, loaded into skate. (Skate.exe build 20260929, next to ReSkate), and what the trainer
//! shares with it. The DLL answers for Caveman's own trigger states (`Animation/Caveman/FX.<Name>` BoolAssets that
//! FX decks trigger on) from rules over the skater's real states: kickflips, grinds, manuals, landings.
//!
//! - [`game`]: the build it knows (hash, addresses, code bytes) and the checks;
//! - [`hook`]: the vtable-slot hook on `BoolAsset`'s bool view (no code patched);
//! - [`scan`]: finding the state assets in memory; [`skater`]: the skater's context and state values;
//! - [`rules`]: the rules engine; [`worker`]: the thread that runs it all; [`pipe`] and [`proto`]: the trainer's pipe.
//!
//! On load (DllMain) it only starts the worker thread. See docs/fx-hook-plan.md.

pub mod game;
pub mod hook;
pub mod pipe;
pub mod proto;
pub mod rules;
pub mod scan;
pub mod sha256;
pub mod skater;
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
            let _ = hook::uninstall();
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
