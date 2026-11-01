//! The hook: slot 3 of `BoolAsset`'s bool-view vtable, swapped for [`getter`]. No code is patched; one 8-byte data
//! word changes (with `VirtualProtect` around the write), and it is put back on unload.
//!
//! Every `BoolAsset` read the game makes through that vtable comes here, from any thread, so the hot path is a few
//! instructions: hash the view pointer, probe a small table of the views of Caveman's own assets, and either return
//! the DLL's value for that fx state or call the game's getter with the same arguments. It reads nothing but the
//! pointer it is given and this module's statics, so it cannot fault.

use std::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};

use crate::win;

/// The getter's shape. The game's takes (view, ctx) and returns a bool in al; all four register arguments and the
/// whole of rax are passed through, so a wider real signature would survive too.
pub type Getter = unsafe extern "C" fn(usize, usize, usize, usize) -> usize;

/// How many fx states the DLL can answer for.
pub const MAX_FX: usize = 64;
const SLOTS: usize = 256;
const MASK: usize = SLOTS - 1;
/// An entry is always found within this many probes of its bucket (insertion refuses otherwise).
const PROBES: usize = 8;

struct Table {
    views: [AtomicUsize; SLOTS],
    fx: [AtomicU8; SLOTS],
}

#[allow(clippy::declare_interior_mutable_const)]
const ZERO_USIZE: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_U8: AtomicU8 = AtomicU8::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_U32: AtomicU32 = AtomicU32::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const EMPTY_TABLE: Table = Table { views: [ZERO_USIZE; SLOTS], fx: [ZERO_U8; SLOTS] };

/// Two tables: the worker fills the idle one and then switches, so a reader never sees a half-built table.
static TABLES: [Table; 2] = [EMPTY_TABLE, EMPTY_TABLE];
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// The value of each fx state (0 or 1), set by the worker every tick.
pub static FX_VALUE: [AtomicU8; MAX_FX] = [ZERO_U8; MAX_FX];
/// How often the game asked for each fx state (since load): proves a deck's trigger reads it.
pub static FX_READS: [AtomicU32; MAX_FX] = [ZERO_U32; MAX_FX];
/// The game's getter (set before the slot is swapped, never cleared).
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
/// The vtable slot swapped (0: not installed).
static SLOT: AtomicUsize = AtomicUsize::new(0);

#[inline(always)]
fn bucket(view: usize) -> usize {
    ((view >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) & MASK
}

/// The replacement getter.
///
/// # Safety
/// Called by the game through the vtable with the game's own arguments; only forwards them.
pub unsafe extern "C" fn getter(view: usize, ctx: usize, a3: usize, a4: usize) -> usize {
    let t = &TABLES[ACTIVE.load(Ordering::Acquire) & 1];
    let mut i = bucket(view);
    for _ in 0..PROBES {
        let k = t.views[i].load(Ordering::Acquire);
        if k == 0 {
            break;
        }
        if k == view {
            let fx = t.fx[i].load(Ordering::Relaxed) as usize & (MAX_FX - 1);
            FX_READS[fx].fetch_add(1, Ordering::Relaxed);
            return FX_VALUE[fx].load(Ordering::Relaxed) as usize;
        }
        i = (i + 1) & MASK;
    }
    let original: Getter = std::mem::transmute::<usize, Getter>(ORIGINAL.load(Ordering::Relaxed));
    original(view, ctx, a3, a4)
}

/// Replaces the table of Caveman's views: (view pointer, fx index). Returns how many fitted (an entry that would need
/// more than [`PROBES`] probes is left out; with at most 64 entries in 256 slots that does not happen in practice).
pub fn set_views(entries: &[(usize, u8)]) -> usize {
    let idle = (ACTIVE.load(Ordering::Acquire) & 1) ^ 1;
    let t = &TABLES[idle];
    for k in 0..SLOTS {
        t.views[k].store(0, Ordering::Relaxed);
    }
    let mut placed = 0;
    'next: for &(view, fx) in entries {
        if view == 0 || fx as usize >= MAX_FX {
            continue;
        }
        let mut i = bucket(view);
        for _ in 0..PROBES {
            let k = t.views[i].load(Ordering::Relaxed);
            if k == view {
                continue 'next; // listed twice
            }
            if k == 0 {
                t.fx[i].store(fx, Ordering::Relaxed);
                t.views[i].store(view, Ordering::Release);
                placed += 1;
                continue 'next;
            }
            i = (i + 1) & MASK;
        }
    }
    ACTIVE.store(idle, Ordering::Release);
    placed
}

/// Whether `view` is in the active table (for tests and status).
pub fn lookup(view: usize) -> Option<u8> {
    let t = &TABLES[ACTIVE.load(Ordering::Acquire) & 1];
    let mut i = bucket(view);
    for _ in 0..PROBES {
        let k = t.views[i].load(Ordering::Acquire);
        if k == 0 {
            return None;
        }
        if k == view {
            return Some(t.fx[i].load(Ordering::Relaxed));
        }
        i = (i + 1) & MASK;
    }
    None
}

pub fn set_value(fx: usize, on: bool) {
    if fx < MAX_FX {
        FX_VALUE[fx].store(on as u8, Ordering::Relaxed);
    }
}

pub fn reads(fx: usize) -> u32 {
    if fx < MAX_FX {
        FX_READS[fx].load(Ordering::Relaxed)
    } else {
        0
    }
}

pub fn installed() -> bool {
    SLOT.load(Ordering::Acquire) != 0
}

/// The slot swapped, when installed.
pub fn slot() -> usize {
    SLOT.load(Ordering::Acquire)
}

/// Swaps 8 bytes at `slot` from `from` to `to`, making the page writable around the write. Err when the slot did not
/// hold `from` (left unchanged) or the page could not be made writable.
///
/// # Safety
/// `slot` must be an 8-byte aligned pointer slot of this process.
unsafe fn swap(slot: usize, from: usize, to: usize) -> Result<(), String> {
    if !slot.is_multiple_of(8) || win::read_ptr(slot).is_none() {
        return Err(format!("slot {slot:#x} is not readable"));
    }
    let mut old = 0u32;
    if win::VirtualProtect(slot as *mut _, 8, win::PAGE_READWRITE, &mut old) == 0 {
        return Err(format!("VirtualProtect failed (error {})", win::GetLastError()));
    }
    let cell = AtomicUsize::from_ptr(slot as *mut usize);
    let result = cell.compare_exchange(from, to, Ordering::SeqCst, Ordering::SeqCst);
    let mut ignored = 0u32;
    win::VirtualProtect(slot as *mut _, 8, old, &mut ignored);
    match result {
        Ok(_) => Ok(()),
        Err(now) => Err(format!("the slot holds {now:#x}, expected {from:#x}")),
    }
}

/// Installs the hook into the vtable slot at `slot`, which must hold `original` (the game's getter).
///
/// # Safety
/// `slot` must be a vtable slot of this process whose callers use the [`Getter`] convention.
pub unsafe fn install(slot: usize, original: usize) -> Result<(), String> {
    if installed() {
        return Err("already installed".into());
    }
    if original == 0 {
        return Err("no original getter".into());
    }
    ORIGINAL.store(original, Ordering::SeqCst);
    swap(slot, original, getter as *const () as usize)?;
    SLOT.store(slot, Ordering::Release);
    Ok(())
}

/// Puts the game's getter back. Err (and the hook stays, forwarding everything once the table is emptied) when the
/// slot no longer holds the hook: something else swapped it after us, and taking ours out would break theirs.
///
/// # Safety
/// No other code may be swapping the same slot concurrently.
pub unsafe fn uninstall() -> Result<(), String> {
    let slot = SLOT.load(Ordering::Acquire);
    if slot == 0 {
        return Ok(());
    }
    set_views(&[]);
    swap(slot, getter as *const () as usize, ORIGINAL.load(Ordering::SeqCst))?;
    SLOT.store(0, Ordering::Release);
    Ok(())
}

/// The hook's own address (what the slot holds while installed).
pub fn hook_address() -> usize {
    getter as *const () as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_spread() {
        let mut used = std::collections::HashSet::new();
        for k in 0..64usize {
            used.insert(bucket(0x2_0000_0000 + k * 0x90));
        }
        assert!(used.len() > 40, "{} buckets for 64 views", used.len());
    }
}
