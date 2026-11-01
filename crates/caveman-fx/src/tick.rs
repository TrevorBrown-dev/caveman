//! A hook on the game's client tick, so that work which must run on the engine thread (the material publish of
//! material.rs) runs there, right after the game's own tick: where ReSkate runs its own board colouring.
//!
//! Why: the publish reaches a per-thread engine context (a pool page table at +0x400 of a per-slot object) that
//! only the game's own threads have; called from the DLL's worker thread it read through a null pointer and took
//! the game down (2026-10-22, `Skate.exe+0x3ec494e`, the dump's faulting thread was ours).
//!
//! The tick (`Skate.exe+0x2e77190`, ReSkate's `runtime::client_tick`) is no vtable slot, and nothing in the image
//! calls it or takes its address (it is reached through a table of RVAs), so the DLL's usual slot swap cannot catch
//! it. ReSkate detours its code: its first five bytes are a `jmp rel32` into ReSkate's hook, which runs the game's
//! tick and then ReSkate's work. This hook changes only that jump's target. A relay page of ours within reach of the
//! site holds a 14-byte absolute jump to [`our_tick`]; our tick calls what the jump went to before (ReSkate's hook,
//! so the game's tick runs as it did), then does the queued engine-thread work. One aligned 8-byte word at the site
//! changes (a compare-and-swap with `VirtualProtect` around it), and it is swapped back on unload when it still
//! holds ours. Without ReSkate's detour in place (the game's own prologue at the site) the hook refuses: there is
//! nothing to retarget, and relocating the prologue is not done here.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use serde_json::{json, Value};

use crate::win;

/// RVA of the client tick (build 20260929, ReSkate's `runtime::client_tick`).
pub const CLIENT_TICK: usize = 0x2e77190;
/// The game's own first bytes of the tick (what ReSkate checks before it detours).
pub const CLIENT_TICK_PREFIX: [u8; 17] = [0x40, 0x53, 0x57, 0x41, 0x55, 0x41, 0x56, 0x48, 0x83, 0xec, 0x38, 0x8b, 0x81, 0xc4, 0, 0, 0];

/// The tick's shape: `(client, update)`.
pub type Tick = unsafe extern "system" fn(usize, usize);

static SITE: AtomicUsize = AtomicUsize::new(0);
/// Where the site's jump went before us (ReSkate's hook). Set before the swap and never cleared: a thread may be
/// inside [`our_tick`] after an uninstall.
static PREVIOUS: AtomicUsize = AtomicUsize::new(0);
static RELAY: AtomicUsize = AtomicUsize::new(0);
static PREV_WORD: AtomicU64 = AtomicU64::new(0);
static OUR_WORD: AtomicU64 = AtomicU64::new(0);
static INSIDE: AtomicU32 = AtomicU32::new(0);
pub static CALLS: AtomicU64 = AtomicU64::new(0);
pub static THREAD: AtomicU32 = AtomicU32::new(0);
pub static LAST_MS: AtomicU64 = AtomicU64::new(0);

/// The hook: the previous target first (the game's tick runs in there), then the queued engine-thread work.
///
/// # Safety
/// Called by the game as the client tick, with its arguments.
pub unsafe extern "system" fn our_tick(client: usize, update: usize) {
    INSIDE.fetch_add(1, Ordering::AcqRel);
    let prev = PREVIOUS.load(Ordering::Acquire);
    if prev != 0 {
        let f: Tick = std::mem::transmute(prev);
        f(client, update);
    }
    CALLS.fetch_add(1, Ordering::Relaxed);
    THREAD.store(win::GetCurrentThreadId(), Ordering::Relaxed);
    LAST_MS.store(win::now_ms(), Ordering::Relaxed);
    crate::material::engine_tick();
    INSIDE.fetch_sub(1, Ordering::AcqRel);
}

pub fn installed() -> bool {
    SITE.load(Ordering::Acquire) != 0
}

pub fn status() -> Value {
    let last = LAST_MS.load(Ordering::Relaxed);
    json!({
        "installed": installed(),
        "calls": CALLS.load(Ordering::Relaxed),
        "thread": THREAD.load(Ordering::Relaxed),
        "last_ms_ago": if last == 0 { Value::Null } else { json!(win::now_ms().saturating_sub(last)) },
        "previous": format!("{:#x}", PREVIOUS.load(Ordering::Acquire)),
        "relay": format!("{:#x}", RELAY.load(Ordering::Acquire)),
    })
}

/// A committed page within rel32 reach of `near` (for the relay), or None.
fn alloc_near(near: usize) -> Option<usize> {
    const REACH: usize = 0x7000_0000;
    let lo = near.saturating_sub(REACH).max(0x10000);
    let hi = near.saturating_add(REACH);
    let mut addr = lo;
    let mut steps = 0;
    while addr < hi && steps < 100_000 {
        steps += 1;
        let mbi = win::query(addr)?;
        let region = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        if mbi.State == win::MEM_FREE {
            let start = (addr + 0xFFFF) & !0xFFFF;
            if start >= lo && start + 0x1000 <= region.saturating_add(size) && start + 0x1000 <= hi {
                // SAFETY: a fresh private page of our own
                let p = unsafe { win::VirtualAlloc(start as *mut _, 0x1000, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE) };
                if !p.is_null() {
                    return Some(p as usize);
                }
            }
        }
        addr = region.checked_add(size)?;
    }
    None
}

/// Retargets the jump at the client tick to our relay. What it did, or why not.
///
/// # Safety
/// `base` must be the game's image base of this process.
pub unsafe fn install(base: usize) -> Result<String, String> {
    if installed() {
        return Ok("already installed".into());
    }
    if base == 0 {
        return Err("no game".into());
    }
    let site = base + CLIENT_TICK;
    let mut head = [0u8; 17];
    if !win::read_into(site, &mut head) {
        return Err("the client tick is unreadable".into());
    }
    if head == CLIENT_TICK_PREFIX {
        return Err("the client tick holds the game's own prologue (ReSkate's detour is not in place): nothing to retarget, not hooked".into());
    }
    if head[0] != 0xE9 {
        return Err(format!("the client tick starts with {:02x?}, neither a jmp rel32 (ReSkate's detour) nor the game's prologue: not hooked", &head[..8]));
    }
    let rel = i32::from_le_bytes(head[1..5].try_into().unwrap());
    let previous = (site as i64 + 5 + rel as i64) as usize;
    if !crate::vhook::is_code(previous) {
        return Err(format!("the jump at the client tick goes to {previous:#x}, which is not code: not hooked"));
    }
    let relay = match RELAY.load(Ordering::Acquire) {
        0 => alloc_near(site).ok_or("no free page within reach of the client tick for the relay")?,
        r => r,
    };
    // jmp [rip+0], then the 8-byte target
    let mut stub = [0u8; 14];
    stub[..6].copy_from_slice(&[0xFF, 0x25, 0, 0, 0, 0]);
    stub[6..].copy_from_slice(&(our_tick as *const () as usize as u64).to_le_bytes());
    let mut old = 0u32;
    if win::VirtualProtect(relay as *mut _, 0x1000, win::PAGE_READWRITE, &mut old) == 0 || !win::write_into(relay, &stub) {
        return Err("the relay page could not be written".into());
    }
    if win::VirtualProtect(relay as *mut _, 0x1000, win::PAGE_EXECUTE_READ, &mut old) == 0 {
        return Err(format!("the relay page could not be made executable (error {})", win::GetLastError()));
    }
    win::FlushInstructionCache(win::GetCurrentProcess(), relay as *const _, stub.len());
    RELAY.store(relay, Ordering::Release);
    let new_rel = relay as i64 - (site as i64 + 5);
    if new_rel < i32::MIN as i64 || new_rel > i32::MAX as i64 {
        return Err("the relay is out of the jump's reach".into());
    }
    let mut ours = head;
    ours[1..5].copy_from_slice(&(new_rel as i32).to_le_bytes());
    let prev_word = u64::from_le_bytes(head[..8].try_into().unwrap());
    let our_word = u64::from_le_bytes(ours[..8].try_into().unwrap());
    if !site.is_multiple_of(8) {
        return Err("the client tick is not 8-byte aligned: not hooked".into());
    }
    // the previous target is in place before any thread can reach our relay
    PREVIOUS.store(previous, Ordering::SeqCst);
    PREV_WORD.store(prev_word, Ordering::SeqCst);
    OUR_WORD.store(our_word, Ordering::SeqCst);
    let mut oldp = 0u32;
    if win::VirtualProtect(site as *mut _, 8, win::PAGE_EXECUTE_READWRITE, &mut oldp) == 0 {
        return Err(format!("VirtualProtect on the client tick failed (error {})", win::GetLastError()));
    }
    let cell = AtomicU64::from_ptr(site as *mut u64);
    let swapped = cell.compare_exchange(prev_word, our_word, Ordering::SeqCst, Ordering::SeqCst);
    let mut ignored = 0u32;
    win::VirtualProtect(site as *mut _, 8, oldp, &mut ignored);
    win::FlushInstructionCache(win::GetCurrentProcess(), site as *const _, 8);
    if let Err(now) = swapped {
        return Err(format!("the client tick changed under us ({now:#018x}): not hooked"));
    }
    SITE.store(site, Ordering::Release);
    Ok(format!("the client tick's jump retargeted from {previous:#x} to our relay {relay:#x}"))
}

/// Puts the previous jump back. Err (and nothing changes) when the site no longer holds ours: something hooked it
/// after us, and the DLL must then stay loaded.
///
/// # Safety
/// No other code may be swapping the same word concurrently.
pub unsafe fn uninstall() -> Result<(), String> {
    let site = SITE.load(Ordering::Acquire);
    if site == 0 {
        return Ok(());
    }
    let mut oldp = 0u32;
    if win::VirtualProtect(site as *mut _, 8, win::PAGE_EXECUTE_READWRITE, &mut oldp) == 0 {
        return Err(format!("VirtualProtect on the client tick failed (error {})", win::GetLastError()));
    }
    let cell = AtomicU64::from_ptr(site as *mut u64);
    let swapped = cell.compare_exchange(OUR_WORD.load(Ordering::SeqCst), PREV_WORD.load(Ordering::SeqCst), Ordering::SeqCst, Ordering::SeqCst);
    let mut ignored = 0u32;
    win::VirtualProtect(site as *mut _, 8, oldp, &mut ignored);
    win::FlushInstructionCache(win::GetCurrentProcess(), site as *const _, 8);
    if let Err(now) = swapped {
        return Err(format!("the client tick no longer holds our jump ({now:#018x}): left as it is"));
    }
    SITE.store(0, Ordering::Release);
    // a thread inside our tick (the game's whole tick runs in there): wait for it, up to five seconds
    let t0 = win::now_ms();
    while INSIDE.load(Ordering::Acquire) != 0 && win::now_ms().saturating_sub(t0) < 5000 {
        win::Sleep(5);
    }
    // the relay page stays (4 KiB): a thread that read the word just before the swap back may still jump through it
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_page_lands_within_reach() {
        let here = alloc_near as *const () as usize;
        let page = alloc_near(here).expect("a page near our own code");
        let distance = (page as i64 - here as i64).abs();
        assert!(distance < 0x7fff_0000, "{distance:#x}");
        assert!(win::query(page).is_some_and(|m| m.State == win::MEM_COMMIT));
        unsafe { win::VirtualFree(page as *mut _, 0, win::MEM_RELEASE) };
    }

    #[test]
    fn refuses_without_a_game() {
        assert!(unsafe { install(0) }.is_err());
        assert!(!installed());
    }
}
