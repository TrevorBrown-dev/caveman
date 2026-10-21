//! The hook on a fake vtable in a read-only page of this process: install, the fast path (ours answered, everything
//! else forwarded with its arguments), switching tables while other threads call through the slot, uninstall.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use caveman_fx::hook::{self, Getter};
use caveman_fx::win;

static CALLS: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn fake_getter(view: usize, ctx: usize, a3: usize, a4: usize) -> usize {
    CALLS.fetch_add(1, Ordering::Relaxed);
    (view ^ ctx ^ a3 ^ a4) & 0xFFFF | 0x1_0000
}

unsafe extern "C" fn other(_: usize, _: usize, _: usize, _: usize) -> usize {
    0xBAD
}

fn protection(addr: usize) -> u32 {
    let mut mbi: win::MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    unsafe { win::VirtualQuery(addr as *const _, &mut mbi, std::mem::size_of::<win::MEMORY_BASIC_INFORMATION>()) };
    mbi.Protect
}

#[test]
fn hook_on_a_fake_vtable() {
    // a read-only page holding a vtable, as the game's .rodata
    let page = unsafe { win::VirtualAlloc(std::ptr::null_mut(), 0x1000, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE) } as *mut usize;
    assert!(!page.is_null());
    let fake = fake_getter as *const () as usize;
    unsafe {
        for k in 0..6 {
            *page.add(k) = other as *const () as usize;
        }
        *page.add(3) = fake;
        let mut old = 0;
        win::VirtualProtect(page.cast(), 0x1000, win::PAGE_READONLY, &mut old);
    }
    let slot = page as usize + 3 * 8;
    assert_eq!(protection(slot), win::PAGE_READONLY);
    let call = |view: usize, ctx: usize| -> usize {
        let f: Getter = unsafe { std::mem::transmute::<usize, Getter>(std::ptr::read_volatile(slot as *const usize)) };
        unsafe { f(view, ctx, 0x30, 0x400) }
    };

    // a wrong original is refused and changes nothing
    assert!(unsafe { hook::install(slot, other as *const () as usize) }.is_err());
    assert_eq!(unsafe { *(slot as *const usize) }, fake);

    unsafe { hook::install(slot, fake) }.unwrap();
    assert!(hook::installed());
    assert_eq!(unsafe { *(slot as *const usize) }, hook::hook_address());
    assert_eq!(protection(slot), win::PAGE_READONLY, "the page's protection is put back");
    assert!(unsafe { hook::install(slot, fake) }.is_err(), "installed twice");

    // nothing mapped: everything forwarded, with all four arguments
    let before = CALLS.load(Ordering::Relaxed);
    assert_eq!(call(0x1000, 0x2), (0x1000 ^ 0x2 ^ 0x30 ^ 0x400) | 0x1_0000);
    assert_eq!(CALLS.load(Ordering::Relaxed), before + 1);

    // two of ours
    let (ours_a, ours_b) = (0x7FF6_1234_5660usize, 0x0000_0234_0000_0060usize);
    assert_eq!(hook::set_views(&[(ours_a, 5), (ours_b, 6)]), 2);
    hook::set_value(5, true);
    hook::set_value(6, false);
    let before = CALLS.load(Ordering::Relaxed);
    assert_eq!(call(ours_a, 0xC7C), 1);
    assert_eq!(call(ours_b, 0xC7C), 0);
    assert_eq!(CALLS.load(Ordering::Relaxed), before, "ours never reach the game's getter");
    assert_eq!(hook::reads(5), 1);
    assert_eq!(call(ours_a + 8, 0), ((ours_a + 8) ^ 0x30 ^ 0x400) & 0xFFFF | 0x1_0000);

    // speed of the hot path through the slot
    let n = 5_000_000usize;
    let t = Instant::now();
    let mut acc = 0usize;
    for k in 0..n {
        acc = acc.wrapping_add(call(0x10_0000 + (k & 0xFFF) * 0x70, k));
    }
    let forwarded = t.elapsed().as_nanos() as f64 / n as f64;
    let t = Instant::now();
    for _ in 0..n {
        acc = acc.wrapping_add(call(ours_a, 0));
    }
    let answered = t.elapsed().as_nanos() as f64 / n as f64;
    println!("hot path: {forwarded:.1} ns per forwarded call, {answered:.1} ns per answered call (acc {acc})");
    assert!(forwarded < 200.0 && answered < 200.0);

    // tables switched while four threads call through the slot: every answer is 0/1 (ours) or the fake's
    static RUN: AtomicBool = AtomicBool::new(true);
    let threads: Vec<_> = (0..4)
        .map(|t| {
            std::thread::spawn(move || {
                let mut bad = 0u64;
                let mut k = 0usize;
                while RUN.load(Ordering::Relaxed) {
                    let view = if k % 2 == 0 { ours_a } else { 0x5000 + t * 0x100 };
                    let f: Getter = unsafe { std::mem::transmute::<usize, Getter>(std::ptr::read_volatile(slot as *const usize)) };
                    let r = unsafe { f(view, 0, 0x30, 0x400) };
                    if r > 1 && r & 0x1_0000 == 0 {
                        bad += 1;
                    }
                    k += 1;
                }
                bad
            })
        })
        .collect();
    for round in 0..2000 {
        if round % 2 == 0 {
            hook::set_views(&[(ours_a, 5)]);
        } else {
            hook::set_views(&[(ours_b, 6), (ours_a, 7)]);
        }
        hook::set_value(5, round % 3 == 0);
        hook::set_value(7, round % 5 == 0);
    }
    RUN.store(false, Ordering::Relaxed);
    for t in threads {
        assert_eq!(t.join().unwrap(), 0);
    }

    // uninstall: the game's getter is back, the protection too
    unsafe { hook::uninstall() }.unwrap();
    assert!(!hook::installed());
    assert_eq!(unsafe { *(slot as *const usize) }, fake);
    assert_eq!(protection(slot), win::PAGE_READONLY);
    unsafe { hook::uninstall() }.unwrap(); // twice: nothing to do
    assert_eq!(call(ours_a, 0), (ours_a ^ 0x30 ^ 0x400) & 0xFFFF | 0x1_0000);

    // installed again after an uninstall (a re-injected DLL does this)
    unsafe { hook::install(slot, fake) }.unwrap();
    unsafe { hook::uninstall() }.unwrap();
    assert_eq!(unsafe { *(slot as *const usize) }, fake);
}
