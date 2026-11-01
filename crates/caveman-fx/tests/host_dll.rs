//! A host for the built `caveman_fx.dll`, as the game would load it (LoadLibraryW), in a process that is not
//! Skate.exe: the DLL must stay dormant and say why over its pipe (here `\\.\pipe\caveman-fx-<pid>`, never the
//! game's), its test exports drive the real hook code on a fake vtable, and `unload` must put the slot back, stop
//! its threads and free the DLL; then it loads again.

use std::time::{Duration, Instant};

use caveman_fx::hook::Getter;
use caveman_fx::proto::{self, Request};
use caveman_fx::win;

unsafe extern "C" fn fake_getter(view: usize, ctx: usize, a3: usize, a4: usize) -> usize {
    (view ^ ctx ^ a3 ^ a4) & 0xFFFF | 0x1_0000
}

fn dll_path() -> std::path::PathBuf {
    // target/<profile>/deps/host_dll-<hash>.exe -> target/<profile>/caveman_fx.dll
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().join("caveman_fx.dll")
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn loaded(name: &[u16]) -> bool {
    !unsafe { win::GetModuleHandleW(name.as_ptr()) }.is_null()
}

#[test]
fn dormant_selftest_and_unload() {
    let dll = dll_path();
    if !dll.is_file() {
        // `cargo test` builds the rlib only; the DLL comes from `cargo build -p caveman-fx` (same profile)
        println!("skipped: {} is not built yet (cargo build -p caveman-fx, then test again)", dll.display());
        return;
    }
    let pipe = proto::pipe_for(false, std::process::id());
    let module_name = win::wide("caveman_fx.dll");

    // a read-only fake vtable, slot 3 the fake getter
    let page = unsafe { win::VirtualAlloc(std::ptr::null_mut(), 0x1000, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE) } as *mut usize;
    let fake = fake_getter as *const () as usize;
    unsafe {
        *page.add(3) = fake;
        let mut old = 0;
        win::VirtualProtect(page.cast(), 0x1000, win::PAGE_READONLY, &mut old);
    }
    let slot = page as usize + 24;
    let call = |view: usize| -> usize {
        let f: Getter = unsafe { std::mem::transmute::<usize, Getter>(std::ptr::read_volatile(slot as *const usize)) };
        unsafe { f(view, 0x2, 0x30, 0x400) }
    };

    for round in 0..2 {
        let wpath = win::wide(&dll.to_string_lossy());
        let module = unsafe { win::LoadLibraryW(wpath.as_ptr()) };
        assert!(!module.is_null(), "LoadLibraryW failed: {}", unsafe { win::GetLastError() });

        // dormant, and saying why
        let mut status = serde_json::Value::Null;
        wait_for("the dormant status", || {
            status = proto::request(&pipe, &Request::Status, 2000).unwrap_or_default();
            status["phase"] == "dormant"
        });
        assert!(status["reason"].as_str().unwrap().contains("not Skate.exe"), "{status}");
        assert_eq!(status["hooked"], false);
        assert_eq!(status["pid"], std::process::id());
        assert!(status["fx"].as_array().unwrap().iter().any(|f| f["name"] == "fx.kickflip" && f["ruled"] == true));

        // the trainer's commands work while dormant
        let fired = proto::request(&pipe, &Request::Fire { fx: "Test".into(), ms: Some(5000) }, 2000).unwrap();
        assert_eq!(fired["fx"], "fx.test");
        let status = proto::request(&pipe, &Request::Status, 2000).unwrap();
        let test = status["fx"].as_array().unwrap().iter().find(|f| f["name"] == "fx.test").unwrap().clone();
        assert!(test["override_ms"].as_u64().unwrap() > 4000, "{test}");
        let bad: caveman_fx::rules::RulesFile = serde_json::from_str(r#"{"rules": [{"fx": "fx.a", "when": "grind &&"}]}"#).unwrap();
        let answer = proto::request(&pipe, &Request::Rules { rules: bad }, 2000).unwrap();
        assert_eq!(answer["ok"], false);
        assert!(answer["error"].as_str().unwrap().contains("rule 1 (fx.a)"), "{answer}");
        let good: caveman_fx::rules::RulesFile =
            serde_json::from_str(r#"{"rules": [{"fx": "fx.a", "when": "grind && Animation/Dingo/Bool.State.Is.Push"}]}"#).unwrap();
        let answer = proto::request(&pipe, &Request::Rules { rules: good }, 2000).unwrap();
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["inputs"][1], "Animation/Dingo/Bool.State.Is.Push");
        let back = proto::request(&pipe, &Request::RulesGet, 2000).unwrap();
        assert_eq!(back["rules"]["rules"][0]["fx"], "fx.a");
        assert_eq!(proto::request_line(&pipe, "not json", 2000).unwrap()["ok"], false);

        // the real hook code, on the fake vtable, through the test exports
        let install: unsafe extern "C" fn(*mut usize) -> i32 =
            unsafe { std::mem::transmute(win::GetProcAddress(module, c"cmfx_test_install".as_ptr().cast())) };
        let map: extern "C" fn(usize, u32, u32) -> i32 = unsafe { std::mem::transmute(win::GetProcAddress(module, c"cmfx_test_map".as_ptr().cast())) };
        assert_eq!(map(0x1234_5670, 3, 1), -1, "nothing to map before the install");
        assert_eq!(unsafe { install(slot as *mut usize) }, 0);
        let hooked = unsafe { std::ptr::read_volatile(slot as *const usize) };
        let (lo, hi) = win::module_range(module).unwrap();
        assert!(hooked != fake && (lo..hi).contains(&hooked), "the slot points into the DLL");
        assert_eq!(map(0x1234_5670, 3, 1), 0);
        assert_eq!(call(0x1234_5670), 1);
        assert_eq!(call(0x1234_5678), (0x1234_5678 ^ 0x2 ^ 0x30 ^ 0x400) & 0xFFFF | 0x1_0000);
        assert_eq!(map(0x1234_5670, 3, 0), 0);
        assert_eq!(call(0x1234_5670), 0);

        // unload: slot back, threads gone, DLL freed
        let answer = proto::request(&pipe, &Request::Unload, 2000).unwrap();
        assert_eq!(answer["unloading"], true);
        wait_for("the DLL to unload", || !loaded(&module_name));
        assert_eq!(unsafe { std::ptr::read_volatile(slot as *const usize) }, fake, "round {round}: the slot is restored");
        assert_eq!(call(0x1234_5670), (0x1234_5670 ^ 0x2 ^ 0x30 ^ 0x400) & 0xFFFF | 0x1_0000);
        assert!(proto::request(&pipe, &Request::Ping, 300).is_err(), "the pipe is gone");
    }
}
