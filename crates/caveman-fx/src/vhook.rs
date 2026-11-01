//! Vtable-slot hooks for COM objects (the swap chain's `Present`, the command queue's `ExecuteCommandLists`): one
//! 8-byte data word swapped with `VirtualProtect` around the write, exactly as [`crate::hook`] does for the game's
//! getter. No code is patched, so this chains with code detours (ReSkate's, Steam's, which patch the function the slot
//! points at) and with other slot hooks: we call whatever the slot held before us.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::win;

/// One hooked slot: where it is and what it held before us.
pub struct VSlot {
    slot: AtomicUsize,
    original: AtomicUsize,
}

impl VSlot {
    pub const fn new() -> VSlot {
        VSlot { slot: AtomicUsize::new(0), original: AtomicUsize::new(0) }
    }

    /// What the slot held before us (the function to call through). Kept after an uninstall: a thread may still be
    /// inside our hook.
    #[inline(always)]
    pub fn original(&self) -> usize {
        self.original.load(Ordering::Acquire)
    }

    pub fn installed(&self) -> bool {
        self.slot.load(Ordering::Acquire) != 0
    }

    /// The slot's address while installed.
    pub fn slot(&self) -> usize {
        self.slot.load(Ordering::Acquire)
    }

    /// Points `slot` at `ours`, remembering what it held. Refused when already installed, when the slot already holds
    /// `ours` (it would call itself), or when the slot is not a readable, aligned pointer to code.
    ///
    /// # Safety
    /// `slot` must be a vtable slot of this process whose callers use the calling convention `ours` has.
    pub unsafe fn install(&self, slot: usize, ours: usize) -> Result<(), String> {
        if self.installed() {
            return Err("already installed".into());
        }
        let current = win::read_ptr(slot).ok_or_else(|| format!("slot {slot:#x} is not readable"))?;
        if current == ours {
            return Err(format!("slot {slot:#x} already holds this hook"));
        }
        if !is_code(current) {
            return Err(format!("slot {slot:#x} holds {current:#x}, which is not code"));
        }
        self.original.store(current, Ordering::SeqCst);
        swap(slot, current, ours)?;
        self.slot.store(slot, Ordering::Release);
        Ok(())
    }

    /// Puts the original back. Err (and nothing changes) when the slot no longer holds `ours`: something hooked it
    /// after us, and taking ours out would cut theirs off.
    ///
    /// # Safety
    /// No other code may be swapping the same slot concurrently.
    pub unsafe fn uninstall(&self, ours: usize) -> Result<(), String> {
        let slot = self.slot.load(Ordering::Acquire);
        if slot == 0 {
            return Ok(());
        }
        swap(slot, ours, self.original())?;
        self.slot.store(0, Ordering::Release);
        Ok(())
    }
}

impl Default for VSlot {
    fn default() -> Self {
        VSlot::new()
    }
}

/// Whether `addr` is in committed executable memory.
pub fn is_code(addr: usize) -> bool {
    let Some(mbi) = win::query(addr) else { return false };
    let exec = win::PAGE_EXECUTE_READ | win::PAGE_EXECUTE_READWRITE | win::PAGE_EXECUTE_WRITECOPY | 0x10;
    mbi.State == win::MEM_COMMIT && mbi.Protect & exec != 0 && mbi.Protect & win::PAGE_GUARD == 0
}

/// Swaps 8 bytes at `slot` from `from` to `to`, making the page writable around the write. Err when the slot did not
/// hold `from` (left unchanged) or the page could not be made writable.
///
/// # Safety
/// `slot` must be an 8-byte aligned pointer slot of this process.
pub unsafe fn swap(slot: usize, from: usize, to: usize) -> Result<(), String> {
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

/// The vtable of a COM object (its first word).
pub fn vtable_of(object: usize) -> Option<usize> {
    win::read_ptr(object).filter(|&v| v >= win::LOWEST)
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn a() -> usize {
        1
    }
    unsafe extern "C" fn b() -> usize {
        2
    }

    #[test]
    fn swaps_and_restores_on_a_read_only_page() {
        let page = unsafe { win::VirtualAlloc(std::ptr::null_mut(), 0x1000, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE) } as *mut usize;
        unsafe {
            *page.add(2) = a as *const () as usize;
            let mut old = 0;
            win::VirtualProtect(page.cast(), 0x1000, win::PAGE_READONLY, &mut old);
        }
        let slot = page as usize + 16;
        let s = VSlot::new();
        unsafe { s.install(slot, b as *const () as usize) }.unwrap();
        assert_eq!(unsafe { *(slot as *const usize) }, b as *const () as usize);
        assert_eq!(s.original(), a as *const () as usize);
        assert!(unsafe { s.install(slot, b as *const () as usize) }.is_err());
        unsafe { s.uninstall(b as *const () as usize) }.unwrap();
        assert_eq!(unsafe { *(slot as *const usize) }, a as *const () as usize);
        assert!(!s.installed());
        // a slot holding data, not code, is refused
        let data = Box::new(0usize);
        let mut fake = [&*data as *const usize as usize];
        assert!(unsafe { VSlot::new().install(fake.as_mut_ptr() as usize, b as *const () as usize) }.is_err());
    }
}
