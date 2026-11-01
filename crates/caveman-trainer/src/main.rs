//! `caveman-trainer`: Caveman FX's companion (docs/fx-hook-plan.md). Injects `caveman_fx.dll` into skate. and talks
//! to it over `\\.\pipe\caveman-fx`: live skater states and fx states, firing fx states by hand, loading rules,
//! unloading. `probe` runs the DLL's own asset scan and skater reads from outside, read-only, without injecting.
//! `draw` and `bars` drive the DLL's own drawing (the overlay: the equaliser bars on the deck); `bars meter` tries
//! the audio capture here, without the game. `board` drives The Caveman: its graphic and its effects, live.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use caveman_fx::proto::{self, Request};
use caveman_fx::rules::{self, Engine, FxNames, RulesFile};
use caveman_fx::win::{self, Handle};
use caveman_fx::{game, scan, sha256, skater};
use serde_json::Value;

mod board;
mod mem;
mod params;
mod pip;
mod rim;
mod timing;
mod video;

const USAGE: &str = "usage: caveman-trainer <command>

  inject [--yes] [--dll FILE] [--pid N]
                                 load caveman_fx.dll into the running Skate.exe (checks the build, ReSkate, no
                                 anti-cheat, not loaded yet; injects a copy from %LOCALAPPDATA%\\Caveman\\fx\\dll).
                                 Without --yes it only runs the checks and says what it would do.
  status [--json]                the DLL's state: phase, hook, scan, skater, states, fx states, log
  watch [SECONDS]                print skater-state and fx-state changes as they happen (default 60 s)
  fire FX [MS]                   force an fx state on (default 2000 ms), e.g. fire fx.test 5000
  stop [FX]                      end an override (all without FX)
  rules load FILE | get | check FILE | default
                                 load rules into the DLL, print the loaded ones, check a file offline, print defaults
  rescan                         look for the assets again
  draw test on|off               the overlay's screen-space test rectangle (top left, the bars' colour)
  draw status | reset | queue auto|swapchain|scan|busiest
                                 the overlay's state; re-arm drawing after failures; force a queue strategy (tests)
  draw streamline on|off         also hook Streamline's swap chain wrapper: the bars go under ReSkate's menus, but a
                                 swap chain the game makes later (a display mode change) then fails ReSkate's own hook
                                 setup; off (the default) draws in DXGI's Present, over ReSkate's menus
  draw depth on|off | near M | status
                                 hide the bars behind the game's scene (skater, deck, wheels; on by default): off, the
                                 near plane by hand (metres; 0: the game's own, 0.06), or the depth test's details
  draw depth copies picture|window
                                 copy every G-buffer pass end of the main depth (picture, the default) or only the
                                 first of each present window (window: the earlier way)
  draw log dump [FILE] | key KEY | auto on|off | status
                                 the depth test's record of the last 2048 presents as JSON (default: a new file in
                                 %TEMP%\\caveman-fx\\marks); the key that saves the 3 s before it and 1 s after (F8);
                                 saving the moments around each blink the DLL finds by itself (on)
  pip on|off|load|status|...     the board cam: the skateboard drawn in a corner, turned as the board under the
                                 skater is (caveman-trainer pip for the words: size, corner, view, elevation, ...)
  bars [on|off]                  the equaliser bars on the deck (no argument: the settings and the overlay's state)
  bars colour RRGGBB | count N | gain X | source random|audio | land
                                 colour (hex, no # needed), bars per side, how far the levels push them, the levels' source,
                                 a landing spike by hand
  bars set KEY VALUE             any setting. The audio's feel: window DB (the bands' window under their reference,
                                 42; narrower spreads the bars out), track all|band (measured against the loudest
                                 band, or each band's own recent peak: band makes every bar show its own dynamics),
                                 floor 0..0.95 (levels under it show as nothing, the rest stretches up: the bars fall
                                 between beats), release DB/s (how fast a reference falls after a peak, 8), rise MS
                                 and fall MS (the meter's attack and decay, 35 and 120). The shape: length base width glow span edge height shift axis (z|x) facing
                                 (board|camera) mirror land land_ms brightness hdr (auto|sdr|hdr10|scrgb) tempo
                                 style (solid|glow) outline (metres) timing (frame|tick) depth_bias (metres)
                                 layout (sides|ring: all round the deck's outline, count = bars in the ring|rim: the ring
                                 on the deck's real edge, kicks included; bars rim load first)
                                 tip_colour accent_colour (RRGGBB; accent empty: none) soft (metres)
                                 deck_width deck_length deck_end (the ring's outline, metres) rim_out rim_lift (metres)
  bars reset                     every setting back to its default (bars stay on or off)
  bars meter [SECONDS]           no game needed: capture the PC's audio here and print the bands (default 10 s)
  unload                         put the game's getter back, stop the DLL's threads and free it
  probe [SECONDS] [--pid N]      read-only, no injection: the DLL's checks, scan and skater reads run from outside
                                 (ReadProcessMemory); then watch the states for SECONDS (default 0)
  probe-scene [SECONDS] [--pid N]
                                 read-only, no injection: what the overlay would read each frame (the camera, the
                                 board's transform), from outside, four times a second for SECONDS (default 0)
  pose [--rate HZ] [--seconds S] [--bones] [--pid N]
                                 read-only, no injection: the camera and the board as one JSON line per tick (default
                                 60 a second) for the app's board cam window, until the game or the reader goes;
                                 --bones adds the board rig's 16 bones (scale, rotation, position in the parent's frame)
  probe-bones [SECONDS] [--pid N]
                                 read-only, no injection: the board rig's bones as the animation system holds them,
                                 four times a second for SECONDS (default 0): the skeleton's order (Reference,
                                 AITrajectory, Trajectory, Connect, Connect2, Ground, Skateboard_Root, Deck, Truck_Back,
                                 eff_truckBack, Right_WheelBack, Left_WheelBack, Truck_Front, eff_truckFront,
                                 Right_WheelFront, Left_WheelFront)
  attach [--ring MB|off] [--timeout S]
                                 for Caveman.exe at launch: waits for the game (ReSkate loaded), injects the DLL beside
                                 this exe unless it already answers, then `mem ring on --mb MB` (default 12288);
                                 lines to %LOCALAPPDATA%\\Caveman\\fx\\attach.log
  probe-timing [SECONDS] [--wait-moving] [--copies [--tol M] [--wc]] [--pid N]
                                 read-only, no injection: when the camera and the board's transform change, against
                                 the presents the injected DLL counts; finds the renderer's per-frame transforms as the
                                 DLL does and compares the bars' jitter with each pair; --copies also hunts every copy
                                 of both matrices. Polls for SECONDS (default 10): ride meanwhile";

type Result<T> = std::result::Result<T, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("inject") => inject(&args[1..]),
        Some("status") => status(args.iter().any(|a| a == "--json")),
        Some("watch") => watch(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(60.0)),
        Some("fire") => fire(&args[1..]),
        Some("stop") => send_print(&Request::Stop { fx: args.get(1).cloned() }),
        Some("rules") => rules_cmd(&args[1..]),
        Some("rescan") => send_print(&Request::Rescan),
        Some("unload") => unload(),
        Some("probe") => probe(&args[1..]),
        Some("probe-scene") => probe_scene(&args[1..]),
        Some("pose") => pose(&args[1..]),
        Some("probe-bones") => probe_bones(&args[1..]),
        Some("attach") => attach(&args[1..]),
        Some("probe-timing") => timing::probe_timing(&args[1..]),
        Some("probe-projection") => timing::probe_projection(&args[1..]),
        Some("draw") => draw_cmd(&args[1..]),
        Some("bars") => bars_cmd(&args[1..]),
        Some("board") => board::board_cmd(&args[1..]),
        Some("pip") => pip::pip_cmd(&args[1..]),
        Some("mem") => mem::mem_cmd(&args[1..]),
        Some("-h" | "--help") | None => {
            println!(
                "{USAGE}
{}
{}
{}",
                rim::USAGE,
                board::USAGE,
                mem::USAGE
            );
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other}\n{USAGE}")),
    };
    if let Err(e) = result {
        eprintln!("caveman-trainer: {e}");
        std::process::exit(1);
    }
}

fn send(req: &Request) -> Result<Value> {
    let v = proto::request(proto::PIPE, req, 3000)?;
    if v["ok"] == false {
        return Err(v["error"].as_str().unwrap_or("failed").to_string());
    }
    Ok(v)
}

fn send_print(req: &Request) -> Result<()> {
    let v = send(req)?;
    println!("{v}");
    Ok(())
}

// ---------------------------------------------------------------- processes

struct Proc {
    pid: u32,
}

fn processes() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let snap = Handle(unsafe { win::CreateToolhelp32Snapshot(win::TH32CS_SNAPPROCESS, 0) });
    if !snap.valid() {
        return out;
    }
    let mut pe: win::PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    pe.dwSize = std::mem::size_of::<win::PROCESSENTRY32W>() as u32;
    if unsafe { win::Process32FirstW(snap.0, &mut pe) } != 0 {
        loop {
            out.push((pe.th32ProcessID, win::from_wide(&pe.szExeFile)));
            if unsafe { win::Process32NextW(snap.0, &mut pe) } == 0 {
                break;
            }
        }
    }
    out
}

fn find_game(pid: Option<u32>) -> Result<Proc> {
    let games: Vec<u32> = processes().into_iter().filter(|(_, n)| n.eq_ignore_ascii_case("skate.exe")).map(|(p, _)| p).collect();
    match pid {
        Some(p) if games.contains(&p) => Ok(Proc { pid: p }),
        Some(p) => Err(format!("process {p} is not Skate.exe")),
        None => match games.as_slice() {
            [] => Err("Skate.exe is not running".into()),
            [p] => Ok(Proc { pid: *p }),
            many => Err(format!("{} Skate.exe processes ({many:?}): pick one with --pid", many.len())),
        },
    }
}

/// (name, base, size) of a process's modules.
fn modules(pid: u32) -> Vec<(String, usize, usize)> {
    for _ in 0..5 {
        let snap = Handle(unsafe { win::CreateToolhelp32Snapshot(win::TH32CS_SNAPMODULE | win::TH32CS_SNAPMODULE32, pid) });
        if !snap.valid() {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let mut out = Vec::new();
        let mut me: win::MODULEENTRY32W = unsafe { std::mem::zeroed() };
        me.dwSize = std::mem::size_of::<win::MODULEENTRY32W>() as u32;
        if unsafe { win::Module32FirstW(snap.0, &mut me) } != 0 {
            loop {
                out.push((win::from_wide(&me.szModule), me.modBaseAddr as usize, me.modBaseSize as usize));
                if unsafe { win::Module32NextW(snap.0, &mut me) } == 0 {
                    break;
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }
    Vec::new()
}

fn image_path(process: win::HANDLE) -> Option<String> {
    let mut buf = vec![0u16; 32768];
    let mut size = buf.len() as u32;
    (unsafe { win::QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut size) } != 0).then(|| win::from_wide(&buf[..size as usize]))
}

/// Checks the game is the known build: Ok(exe path).
fn check_build(process: win::HANDLE) -> Result<String> {
    let path = image_path(process).ok_or("cannot read Skate.exe's path")?;
    let digest = sha256::file_sha256(Path::new(&path)).map_err(|e| format!("{path}: {e}"))?;
    if digest != game::EXE_SHA256 {
        return Err(format!("{path} is not build {} (sha256 {}...): the DLL would stay dormant; not injecting", game::BUILD, &digest[..16]));
    }
    Ok(path)
}

// ---------------------------------------------------------------- inject

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn inject(args: &[String]) -> Result<()> {
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let source = match arg_value(args, "--dll") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("caveman_fx.dll"),
    };
    if !source.is_file() {
        return Err(format!("{} not found (build it: cargo build --release -p caveman-fx)", source.display()));
    }
    let query = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_LIMITED_INFORMATION | win::PROCESS_VM_READ, 0, game.pid) });
    if !query.valid() {
        return Err(format!("cannot open Skate.exe (pid {}): error {}", game.pid, unsafe { win::GetLastError() }));
    }
    let exe = check_build(query.0)?;
    println!("Skate.exe pid {} is build {} ({exe})", game.pid, game::BUILD);
    let anticheat: Vec<String> = processes().into_iter().map(|(_, n)| n).filter(|n| n.to_ascii_lowercase().starts_with("eaanticheat")).collect();
    if !anticheat.is_empty() {
        return Err(format!("EA anti-cheat is running ({}): not injecting", anticheat.join(", ")));
    }
    let mods = modules(game.pid);
    if mods.is_empty() {
        return Err("cannot list Skate.exe's modules".into());
    }
    if !mods.iter().any(|(n, _, _)| n.eq_ignore_ascii_case("ReSkate.dll")) {
        return Err("ReSkate.dll is not loaded in Skate.exe: start the game through ReSkate (or wait until it is up)".into());
    }
    let loaded: Vec<String> = mods.iter().filter(|(n, _, _)| n.to_ascii_lowercase().starts_with("caveman_fx")).map(|(n, _, _)| n.clone()).collect();
    if proto::request(proto::PIPE, &Request::Ping, 200).is_ok() {
        return Err(format!("{} already answers ({} loaded): `caveman-trainer unload` first", proto::PIPE, loaded.join(", ")));
    }
    if !loaded.is_empty() {
        // an unload refused (another hook chained into ours: it stays loaded, inert and silent): a fresh copy goes in
        // beside it under a name of its own
        println!("{} stay(s) loaded inert (its unload was refused); injecting a fresh copy beside it", loaded.join(", "));
    }
    if !args.iter().any(|a| a == "--yes") {
        println!("checks passed: would inject {} into Skate.exe pid {} (ReSkate loaded, no anti-cheat).", source.display(), game.pid);
        println!("nothing done: run `caveman-trainer inject --yes` to inject.");
        return Ok(());
    }
    // a copy per build, so the build output is never locked by the game
    let bytes = std::fs::read(&source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut h = sha256::Sha256::new();
    h.update(&bytes);
    let tag = &sha256::hex(&h.finish())[..12];
    let dir = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).ok_or("no LOCALAPPDATA")?.join("Caveman").join("fx").join("dll");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // a name no module of the game's already has (the same build again beside an inert copy of itself)
    let mut dll = dir.join(format!("caveman_fx-{tag}.dll"));
    let mut k = 1;
    while loaded.iter().any(|n| dll.file_name().is_some_and(|f| f.eq_ignore_ascii_case(n))) {
        dll = dir.join(format!("caveman_fx-{tag}-{k}.dll"));
        k += 1;
    }
    if !dll.is_file() {
        std::fs::write(&dll, &bytes).map_err(|e| format!("{}: {e}", dll.display()))?;
    }
    println!("injecting {}", dll.display());
    let access = win::PROCESS_CREATE_THREAD
        | win::PROCESS_QUERY_INFORMATION
        | win::PROCESS_VM_OPERATION
        | win::PROCESS_VM_WRITE
        | win::PROCESS_VM_READ
        | win::SYNCHRONIZE;
    let process = Handle(unsafe { win::OpenProcess(access, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe for injection: error {}", unsafe { win::GetLastError() }));
    }
    let path = win::wide(&dll.to_string_lossy());
    let size = path.len() * 2;
    let remote = unsafe { win::VirtualAllocEx(process.0, std::ptr::null_mut(), size, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE) };
    if remote.is_null() {
        return Err(format!("VirtualAllocEx failed: error {}", unsafe { win::GetLastError() }));
    }
    let result = (|| -> Result<u32> {
        let mut written = 0usize;
        if unsafe { win::WriteProcessMemory(process.0, remote, path.as_ptr().cast(), size, &mut written) } == 0 || written != size {
            return Err(format!("WriteProcessMemory failed: error {}", unsafe { win::GetLastError() }));
        }
        let k32 = unsafe { win::GetModuleHandleW(win::wide("kernel32.dll").as_ptr()) };
        let load = unsafe { win::GetProcAddress(k32, c"LoadLibraryW".as_ptr().cast()) };
        if load.is_null() {
            return Err("no LoadLibraryW".into());
        }
        let thread = Handle(unsafe { win::CreateRemoteThread(process.0, std::ptr::null_mut(), 0, load, remote, 0, std::ptr::null_mut()) });
        if !thread.valid() {
            return Err(format!("CreateRemoteThread failed: error {}", unsafe { win::GetLastError() }));
        }
        if unsafe { win::WaitForSingleObject(thread.0, 20_000) } != win::WAIT_OBJECT_0 {
            return Err("LoadLibraryW did not return within 20 s".into());
        }
        let mut code = 0u32;
        unsafe { win::GetExitCodeThread(thread.0, &mut code) };
        Ok(code)
    })();
    unsafe { win::VirtualFreeEx(process.0, remote, 0, win::MEM_RELEASE) };
    if result? == 0 {
        return Err("LoadLibraryW returned NULL in the game (see %LOCALAPPDATA%\\Caveman\\fx\\caveman-fx.log)".into());
    }
    println!("loaded; waiting for the DLL to check the game...");
    let t = Instant::now();
    loop {
        if let Ok(v) = proto::request(proto::PIPE, &Request::Status, 1000) {
            if v["phase"] != "starting" {
                print_status(&v);
                return Ok(());
            }
        }
        if t.elapsed() > Duration::from_secs(30) {
            return Err("the DLL did not answer within 30 s".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ---------------------------------------------------------------- status, watch

fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(x) if x.fract() == 0.0 => format!("{}", x as i64),
        Some(x) => format!("{x:.2}"),
        None => "-".into(),
    }
}

fn input_label(i: &Value) -> String {
    i["short"].as_str().map(str::to_string).unwrap_or_else(|| i["name"].as_str().unwrap_or("?").to_string())
}

fn print_status(v: &Value) {
    println!(
        "caveman_fx {} in pid {} ({}): {} ({})",
        v["version"].as_str().unwrap_or("?"),
        v["pid"],
        v["pipe"].as_str().unwrap_or(""),
        v["phase"].as_str().unwrap_or("?").to_uppercase(),
        v["reason"].as_str().unwrap_or("")
    );
    let s = &v["scan"];
    println!(
        "  hook: {}; exe base {}; scans {} (last {} ms, {} MB, {} regions, {} candidates{}); fx assets {}, state assets {}{}",
        if v["hooked"] == true { "installed" } else { "not installed" },
        v["exe_base"].as_str().unwrap_or("-"),
        s["scans"],
        s["last_ms"],
        s["last_mb"],
        s["regions"],
        s["candidates"],
        if s["running"] == true { "; scanning now" } else { "" },
        s["ours"],
        s["states"],
        s["next_in_ms"].as_u64().map(|ms| format!("; next scan in {:.1} s", ms as f64 / 1000.0)).unwrap_or_default()
    );
    match v["skater"].as_object() {
        Some(k) => println!(
            "  skater: record {} ({} pages, ctx {}); {} record(s) in {} world(s)",
            k["id"],
            k["pages"],
            k["ctx"].as_str().unwrap_or(""),
            v["records"].as_array().map_or(0, Vec::len),
            v["worlds"]
        ),
        None => println!("  skater: none (menu or loading); {} world(s)", v["worlds"]),
    }
    if let Some(inputs) = v["inputs"].as_array() {
        let parts: Vec<String> = inputs
            .iter()
            .map(|i| {
                let mark = if i["assets"] == 0 {
                    "?"
                } else if i["live"] == true {
                    ""
                } else {
                    "*"
                };
                format!("{}={}{mark}", input_label(i), num(&i["value"]))
            })
            .collect();
        println!("  states (? not found, * not live: Default): {}", parts.join(" "));
    }
    if let Some(fx) = v["fx"].as_array() {
        println!("  fx states (name: value, assets in game, reads by the game, override):");
        for f in fx {
            let ov = f["override_ms"].as_u64().unwrap_or(0);
            println!(
                "    {:<18} {:<3} assets {:<2} reads {:<8}{}{}",
                f["name"].as_str().unwrap_or("?"),
                if f["value"] == true { "ON" } else { "off" },
                f["assets"],
                f["reads"],
                if f["ruled"] == true { " ruled" } else { "" },
                if ov > 0 { format!(" forced {:.1} s", ov as f64 / 1000.0) } else { String::new() }
            );
        }
    }
    println!("  rules: {}", v["rules"]);
    print_render(v);
    if let Some(log) = v["log"].as_array() {
        for l in log.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
            println!("  | {}", l.as_str().unwrap_or(""));
        }
    }
}

fn status(json: bool) -> Result<()> {
    let v = send(&Request::Status)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    } else {
        print_status(&v);
    }
    Ok(())
}

fn clock() -> String {
    let mut t = win::SYSTEMTIME::default();
    unsafe { win::GetLocalTime(&mut t) };
    format!("{:02}:{:02}:{:02}.{:03}", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds)
}

fn watch(seconds: f64) -> Result<()> {
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last: std::collections::BTreeMap<String, String> = Default::default();
    let mut last_log: Option<String> = None;
    println!("watching for {seconds} s (Ctrl+C to stop)");
    while Instant::now() < end {
        let v = match proto::request(proto::PIPE, &Request::Status, 1000) {
            Ok(v) => v,
            Err(e) => {
                println!("{} {e}", clock());
                std::thread::sleep(Duration::from_millis(1000));
                continue;
            }
        };
        let mut now: std::collections::BTreeMap<String, String> = Default::default();
        now.insert("phase".into(), v["phase"].as_str().unwrap_or("?").to_string());
        now.insert("skater".into(), v["skater"]["id"].as_u64().map(|i| format!("record {i}")).unwrap_or_else(|| "none".into()));
        for i in v["inputs"].as_array().into_iter().flatten() {
            now.insert(input_label(i), num(&i["value"]));
        }
        for f in v["fx"].as_array().into_iter().flatten() {
            now.insert(f["name"].as_str().unwrap_or("?").to_string(), if f["value"] == true { "ON".into() } else { "off".into() });
        }
        let changed: Vec<String> = now.iter().filter(|(k, val)| last.get(*k) != Some(val)).map(|(k, val)| format!("{k}={val}")).collect();
        if !changed.is_empty() {
            println!("{} {}", clock(), changed.join(" "));
        }
        if let Some(lines) = v["log"].as_array() {
            let lines: Vec<&str> = lines.iter().filter_map(|l| l.as_str()).collect();
            let start =
                last_log.as_deref().and_then(|l| lines.iter().position(|x| *x == l)).map_or(if last_log.is_none() { lines.len() } else { 0 }, |p| p + 1);
            for l in &lines[start.min(lines.len())..] {
                println!("{} | {l}", clock());
            }
            if let Some(l) = lines.last() {
                last_log = Some(l.to_string());
            }
        }
        last = now;
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn fire(args: &[String]) -> Result<()> {
    let fx = args.first().ok_or("fire needs an fx state, e.g. fire fx.test 3000")?.clone();
    let ms = match args.get(1) {
        Some(s) => Some(s.parse::<u64>().map_err(|_| format!("{s} is not milliseconds"))?),
        None => None,
    };
    let v = send(&Request::Fire { fx, ms })?;
    println!("{} on for {} ms ({} asset(s) in the game)", v["fx"].as_str().unwrap_or("?"), v["ms"], v["assets"]);
    if v["assets"] == 0 {
        println!("note: no Animation/Caveman/FX asset of that name was found in the game: no deck can show it (is Caveman-FX installed and the scan done?)");
    }
    Ok(())
}

fn read_rules(path: &str) -> Result<RulesFile> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

fn rules_cmd(args: &[String]) -> Result<()> {
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("load"), Some(path)) => {
            let file = read_rules(path)?;
            let v = send(&Request::Rules { rules: file })?;
            println!("loaded {} rule(s); states read: {}", v["rules"], v["inputs"]);
            Ok(())
        }
        (Some("check"), Some(path)) => {
            let file = read_rules(path)?;
            let mut fx = FxNames::default();
            let e = Engine::compile(&file, &mut fx)?;
            println!("{} rule(s) OK", file.rules.len());
            println!("fx states: {}", fx.names().join(", "));
            println!("states read: {}", e.inputs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", "));
            Ok(())
        }
        (Some("get"), _) => {
            let v = send(&Request::RulesGet)?;
            println!("{}", serde_json::to_string_pretty(&v["rules"]).unwrap_or_default());
            Ok(())
        }
        (Some("default"), _) => {
            println!("{}", rules::DEFAULT_RULES.trim_end());
            Ok(())
        }
        _ => Err("rules load FILE | rules get | rules check FILE | rules default".into()),
    }
}

fn unload() -> Result<()> {
    let v = send(&Request::Unload)?;
    if v["unloading"] != true {
        return Err(format!("unexpected answer {v}"));
    }
    println!("unloading...");
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(250));
        if proto::request(proto::PIPE, &Request::Ping, 200).is_err() {
            if let Ok(g) = find_game(None) {
                let still: Vec<String> = modules(g.pid).into_iter().map(|(n, _, _)| n).filter(|n| n.to_ascii_lowercase().starts_with("caveman_fx")).collect();
                if !still.is_empty() {
                    continue;
                }
            }
            println!("unloaded: the game's getter is back and caveman_fx is out of the game");
            return Ok(());
        }
    }
    Err("the DLL did not unload within 10 s (see %LOCALAPPDATA%\\Caveman\\fx\\caveman-fx.log)".into())
}

// ---------------------------------------------------------------- probe (read-only)

fn probe(args: &[String]) -> Result<()> {
    let seconds: f64 = args.iter().find(|a| !a.starts_with("--") && a.parse::<f64>().is_ok()).and_then(|a| a.parse().ok()).unwrap_or(0.0);
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let process = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_READ, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe (pid {}) for reading: error {}", game.pid, unsafe { win::GetLastError() }));
    }
    let exe = check_build(process.0)?;
    let base = modules(game.pid).into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case("skate.exe")).map(|(_, b, _)| b).ok_or("cannot find Skate.exe's base")?;
    println!("Skate.exe pid {} build {} at {base:#x} ({exe})", game.pid, game::BUILD);
    win::set_target(process.0, game.pid);
    match game::check_image(base) {
        Ok(()) => println!("code and vtables: as in build {} (the DLL would hook)", game::BUILD),
        Err(e) => println!("code and vtables: {e} (the DLL would stay dormant)"),
    }
    let rt = skater::Runtime::new(base).ok_or("the game's globals are not readable")?;
    let worlds = rt.worlds();
    println!("worlds: {}", worlds.iter().map(|(w, t)| format!("{w:#x} (thread {t})")).collect::<Vec<_>>().join(", "));
    let mut records = Vec::new();
    for (w, _) in &worlds {
        if let Some(m) = rt.manager(*w) {
            for r in rt.records(m) {
                if !records.contains(&r) {
                    records.push(r);
                }
            }
        }
    }
    for r in &records {
        println!("  record {} ctx {:#x} pages {}", r.id, r.ctx, r.pages);
    }
    let skater = records.iter().copied().max_by(|a, b| a.pages.cmp(&b.pages).then(b.id.cmp(&a.id))).filter(|r| r.pages > 0);
    match skater {
        Some(r) => println!("skater: record {} ({} pages)", r.id, r.pages),
        None => println!("skater: none (in a menu or loading?)"),
    }
    let spec = scan::ScanSpec {
        classes: vec![
            (base + game::rva::BOOL_ASSET_VTABLE, rules::Kind::Bool),
            (base + game::rva::ENUM_ASSET_VTABLE, rules::Kind::Int),
            (base + game::rva::FLOAT_ASSET_VTABLE, rules::Kind::Float),
        ],
        bool_view_vtable: Some(base + game::rva::BOOL_VIEW_VTABLE),
        wanted: rules::INPUTS.iter().map(|(_, n, _)| n.to_string()).collect(),
        prefix: caveman_fx::worker::OUR_PREFIX.into(),
        mapped: false,
    };
    let t = Instant::now();
    let mut s = scan::Scanner::new(spec)?;
    while !s.step(256 << 20, 1000) {}
    println!(
        "scan: {} MB of resident private memory in {} regions, {} candidates, {:.1} s",
        s.bytes >> 20,
        s.regions,
        s.candidates_seen,
        t.elapsed().as_secs_f64()
    );
    let mut hits = s.hits.clone();
    hits.sort_by_key(|a| a.name.to_ascii_lowercase());
    let ours: Vec<&scan::Hit> = hits.iter().filter(|h| h.name.to_ascii_lowercase().starts_with(caveman_fx::worker::OUR_PREFIX)).collect();
    println!(
        "Caveman fx assets: {}",
        if ours.is_empty() { "none (is Caveman-FX installed?)".to_string() } else { ours.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(", ") }
    );
    let read_all = |ctx: Option<usize>| -> Vec<(String, String)> {
        rules::INPUTS
            .iter()
            .map(|(short, name, kind)| {
                let found: Vec<&scan::Hit> = hits.iter().filter(|h| h.name.eq_ignore_ascii_case(name)).collect();
                let text = if found.is_empty() {
                    "?".to_string()
                } else {
                    let rs: Vec<skater::Reading> = found.iter().map(|h| skater::read_state(h.asset, *kind, ctx)).collect();
                    let r = rs.iter().find(|r| r.live).or(rs.first()).copied().unwrap();
                    format!(
                        "{}{}",
                        r.value.map(|v| if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v:.2}") }).unwrap_or("-".into()),
                        if r.live { "" } else { "*" }
                    )
                };
                (short.to_string(), text)
            })
            .collect()
    };
    for (short, name, _) in rules::INPUTS.iter() {
        let n = hits.iter().filter(|h| h.name.eq_ignore_ascii_case(name)).count();
        println!("  {short:<13} {name}: {n} asset(s)");
    }
    let ctx = skater.map(|r| r.ctx);
    let line = |v: &[(String, String)]| v.iter().map(|(k, x)| format!("{k}={x}")).collect::<Vec<_>>().join(" ");
    let mut last = read_all(ctx);
    println!("{} {}   (* not live: Default; ? not found)", clock(), line(&last));
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
        let now = read_all(ctx);
        let changed: Vec<(String, String)> = now.iter().zip(&last).filter(|(a, b)| a != b).map(|(a, _)| a.clone()).collect();
        if !changed.is_empty() {
            println!("{} {}", clock(), line(&changed));
        }
        last = now;
    }
    Ok(())
}

// ---------------------------------------------------------------- draw, bars

fn print_render(v: &Value) {
    let r = &v["render"];
    if r.is_null() {
        return;
    }
    let size = r["size"].as_array().map(|a| format!("{}x{}", a[0], a[1])).unwrap_or_default();
    println!(
        "  overlay: {}{}; test {}, bars {}; frames {} drawn of {} presents ({} skipped, {} failing){}",
        if r["installed"] == true { "installed" } else { "not installed" },
        if r["streamline"] == true { " (Streamline's Present hooked too)" } else { "" },
        if r["test"] == true { "on" } else { "off" },
        if r["bars"] == true { "on" } else { "off" },
        r["frames"],
        r["presents"],
        r["skipped"],
        r["failures"],
        if r["failed"] == true { "; DRAWING TURNED OFF after failures (draw reset)" } else { "" }
    );
    if r["frames"].as_u64().unwrap_or(0) > 0 || r["queue"] != "" {
        println!(
            "    via {} Present, queue: {} ({}), back buffer {size} format {} ({}), scene: {}, {} vertices, landings {}",
            r["via"].as_str().unwrap_or("-"),
            r["queue"].as_str().unwrap_or("-"),
            r["queue_mode"].as_str().unwrap_or("auto"),
            r["format"],
            r["output"].as_str().unwrap_or("-"),
            r["scene"].as_str().unwrap_or("-"),
            r["vertices"],
            r["landings"]
        );
    }
    if let Some(t) = r["timing"].as_str().filter(|s| !s.is_empty()) {
        let p = &r["proxies"];
        println!(
            "    transforms: {t} (frame proxies: camera {}, board {}; {} search(es){}){}",
            p["camera"].as_str().unwrap_or("-"),
            p["board"].as_str().unwrap_or("-"),
            p["searches"],
            if p["searching"] == true { ", searching" } else { "" },
            p["last"].as_str().filter(|s| !s.is_empty()).map(|s| format!("; last: {s}")).unwrap_or_default()
        );
    }
    let d = &r["depth"];
    if d.is_object() {
        let chosen = d["candidates"]
            .as_array()
            .and_then(|c| c.iter().find(|x| x["chosen"] == true))
            .map(|c| format!("{}x{} format {}", c["size"][0], c["size"][1], c["format"]));
        println!(
            "    depth: {}; main depth {}; {} candidate(s), {} pass end(s) seen, copies {} recorded / {} submitted; lists hooked {} (shared table {}){}",
            r["depth_state"].as_str().filter(|s| !s.is_empty()).unwrap_or(if d["enabled"] == true { "-" } else { "off" }),
            chosen.unwrap_or_else(|| "not chosen".into()),
            d["candidates"].as_array().map_or(0, Vec::len),
            d["passes_ended"],
            d["copies_recorded"],
            d["copies_submitted"],
            d["lists_adopted"],
            d["shared_table"].as_str().unwrap_or("?"),
            if d["enhanced_barriers"].as_u64().unwrap_or(0) > 0 {
                format!("; {} enhanced barrier(s) on depth (not supported yet)", d["enhanced_barriers"])
            } else {
                String::new()
            }
        );
    }
    let f = &r["depth_frames"];
    if f.is_object() {
        let p = &f["paired"];
        let b = &f["blinks"];
        println!(
            "    depth frames: {} fresh, {} late (an older copy, in its own camera), {} off; lapses {}{}; copies paired with the frame: {} exact, {} one old, {} ahead, {} unpaired",
            f["frames"]["fresh"],
            f["frames"]["late"],
            f["frames"]["off"],
            f["frames"]["blinks"],
            f["frames"]["last_lapses"].as_array().and_then(|l| l.last()).and_then(|l| l.as_str()).map(|l| format!(" (last: {l})")).unwrap_or_default(),
            p["exact"],
            p["one_old"],
            p["ahead"],
            p["none"]
        );
        println!(
            "    blinks found by the GPU's count: {} ({} unhidden, {} overhidden) in {} presents counted; mark key {}, auto-save {}, {} mark(s) saved to {}{}",
            b["blinks"],
            b["unhidden"],
            b["overhidden"],
            b["presents_counted"],
            b["key"].as_str().unwrap_or("?"),
            if b["auto_save"] == true { "on" } else { "off" },
            b["marks_saved"],
            b["marks_dir"].as_str().unwrap_or("?"),
            b["last_blinks"].as_array().and_then(|l| l.last()).map(|l| format!("; last: {l}")).unwrap_or_default()
        );
    }
    let d = &r["depth"];
    if d.is_object() {
        println!(
            "    depth copies: {} MB held ({} set(s) alive, {} made, {} kept for good){}; per {}; {} on another queue; {} pass end(s) in a write state; {} pending; {} dropped at a reset",
            d["copies_mb"],
            d["copy_sets_alive"],
            d["copy_sets_made"],
            d["copy_sets_leaked"],
            d["copies_held_back"].as_str().filter(|s| !s.is_empty()).map(|s| format!("; held back: {s}")).unwrap_or_default(),
            d["copies_per"].as_str().unwrap_or("?"),
            d["copies_other_queue"],
            d["write_state_ends"],
            d["pending"].as_array().map_or(0, Vec::len),
            d["pending_dropped_at_reset"]
        );
    }
    let m = &r["video_memory"];
    if m.is_object() {
        println!(
            "    memory watch: video {} of {} MB, system for the GPU {} of {} MB ({} samples, {} jump(s) logged){}",
            m["local_mb"]["used"],
            m["local_mb"]["budget"],
            m["non_local_mb"]["used"],
            m["non_local_mb"]["budget"],
            m["samples"],
            m["jumps_logged"],
            m["error"].as_str().filter(|s| !s.is_empty()).map(|s| format!("; {s}")).unwrap_or_default()
        );
    }
    let g = &r["memory_guard"];
    if g.is_object() {
        let hooked = g["hooked"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        println!(
            "    memory guard: {}; {} out-of-memory episode(s) in {} allocation(s) of the game's: {} rescued, {} given up (an allocation is tried again for up to {} s){}{}",
            if hooked.is_empty() { "not hooked".to_string() } else { format!("on {hooked}") },
            g["episodes"],
            g["calls"],
            g["rescued"],
            g["gave_up"],
            g["retry_for_ms"].as_u64().unwrap_or(0) / 1000,
            if g["squeezed"] == true { "; squeezed now: nothing of ours allocates" } else { "" },
            g["last"].as_str().filter(|s| !s.is_empty()).map(|s| format!("\n      last: {s}")).unwrap_or_default()
        );
    }
    if let Some(i) = r["install"].as_str().filter(|s| !s.is_empty()) {
        println!("    {i}");
    }
    if let Some(e) = r["error"].as_str().filter(|s| !s.is_empty()) {
        println!("    last error: {e}");
    }
    let b = &v["bars"];
    if b.is_object() {
        println!(
            "  bars: {}, {} {} (tips {}), {} {}, gain {}, source {}{}, length {} m, landing spike {}",
            if b["enabled"] == true { "ON" } else { "off" },
            b["style"].as_str().unwrap_or("?"),
            b["colour"].as_str().unwrap_or("?"),
            b["tip_colour"].as_str().unwrap_or("-"),
            b["count"],
            if b["layout"] == "ring" {
                "in a ring round the deck"
            } else if b["layout"] == "rim" {
                if v["rim"]["loaded"] == false {
                    "on the deck's rim (none loaded: a ring)"
                } else {
                    "on the deck's rim"
                }
            } else {
                "per side"
            },
            num(&b["gain"]),
            b["source"].as_str().unwrap_or("?"),
            if b["source"] == "audio" { format!(" ({})", v["audio"]["status"].as_str().unwrap_or("")) } else { String::new() },
            num(&b["length"]),
            num(&b["land"])
        );
    }
}

fn wait_installed(want_draw: bool) {
    // the DLL installs the overlay on its next tick: show the result
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(1500) {
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(v) = send(&Request::Status) {
            if !want_draw
                || v["render"]["installed"] == true
                || v["log"].as_array().is_some_and(|l| l.iter().any(|x| x.as_str().is_some_and(|x| x.contains("overlay not installed"))))
            {
                print_render(&v);
                return;
            }
        }
    }
    if let Ok(v) = send(&Request::Status) {
        print_render(&v);
    }
}

pub(crate) fn on_off(s: Option<&String>) -> Result<bool> {
    match s.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("on" | "true" | "1" | "yes") => Ok(true),
        Some("off" | "false" | "0" | "no") => Ok(false),
        _ => Err("on or off".into()),
    }
}

fn draw_cmd(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("test") => {
            let on = on_off(args.get(1))?;
            send(&Request::Draw { test: Some(on), queue: None, reset: None, streamline: None })?;
            println!("test rectangle {}", if on { "on" } else { "off" });
            wait_installed(on);
            Ok(())
        }
        Some("reset") => {
            send(&Request::Draw { test: None, queue: None, reset: Some(true), streamline: None })?;
            println!("drawing re-armed");
            Ok(())
        }
        Some("streamline") => {
            let on = on_off(args.get(1))?;
            let v = send(&Request::Draw { test: None, queue: None, reset: None, streamline: Some(on) })?;
            println!("{}", v["note"].as_str().unwrap_or(""));
            Ok(())
        }
        Some("queue") => {
            let q = args.get(1).ok_or("draw queue auto|swapchain|scan|busiest")?;
            send(&Request::Draw { test: None, queue: Some(q.clone()), reset: None, streamline: None })?;
            println!("queue strategy: {q}");
            Ok(())
        }
        Some("depth") => match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("near") => {
                let n: f32 = args.get(2).and_then(|s| s.parse().ok()).ok_or("draw depth near METRES (0: the game's own, 0.06)")?;
                let v = send(&Request::Depth { on: None, near: Some(n), copies: None })?;
                print_render(&v);
                Ok(())
            }
            Some("copies") => {
                let c = args.get(2).ok_or("draw depth copies picture|window")?;
                let v = send(&Request::Depth { on: None, near: None, copies: Some(c.clone()) })?;
                println!("depth copies per {}", v["render"]["depth"]["copies_per"].as_str().unwrap_or("?"));
                Ok(())
            }
            Some("status") | None => {
                let v = send(&Request::Depth { on: None, near: None, copies: None })?;
                print_render(&v);
                println!("  depth details: {}", serde_json::to_string_pretty(&v["render"]["depth"]).unwrap_or_default());
                Ok(())
            }
            _ => {
                let on = on_off(args.get(1))?;
                let v = send(&Request::Depth { on: Some(on), near: None, copies: None })?;
                println!("depth test {}", if on { "on" } else { "off" });
                print_render(&v);
                Ok(())
            }
        },
        Some("log") => match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("dump") => {
                let path = args.get(2).filter(|a| !a.starts_with("--")).cloned();
                let v = send(&Request::Diag { dump: Some(path.map(Value::String).unwrap_or(Value::Bool(true))), key: None, auto: None })?;
                println!("{} presents written to {}", v["dumped"]["presents"], v["dumped"]["path"].as_str().unwrap_or("?"));
                Ok(())
            }
            Some("key") => {
                let k = args.get(2).ok_or("draw log key F8|0xNN|off")?;
                let v = send(&Request::Diag { dump: None, key: Some(k.clone()), auto: None })?;
                println!("the mark key: {}", v["diag"]["key"].as_str().unwrap_or("?"));
                Ok(())
            }
            Some("auto") => {
                let on = on_off(args.get(2))?;
                send(&Request::Diag { dump: None, key: None, auto: Some(on) })?;
                println!("saving the moments around each blink found: {}", if on { "on" } else { "off" });
                Ok(())
            }
            Some("status") | None => {
                let v = send(&Request::Diag { dump: None, key: None, auto: None })?;
                println!("{}", serde_json::to_string_pretty(&v["diag"]).unwrap_or_default());
                Ok(())
            }
            Some(other) => Err(format!("draw log {other}? draw log dump [FILE] | key KEY | auto on|off | status")),
        },
        Some("status") | None => {
            let v = send(&Request::Status)?;
            print_render(&v);
            Ok(())
        }
        Some(other) => {
            Err(format!("draw {other}? draw test on|off | status | reset | queue MODE | streamline on|off | depth on|off|near M|copies C|status | log ..."))
        }
    }
}

/// A setting's value as typed: a number, true/false, or a string.
fn setting_value(text: &str) -> Value {
    serde_json::from_str::<Value>(text).ok().filter(|v| !v.is_string()).unwrap_or_else(|| Value::String(text.to_string()))
}

fn bars_set(patch: Value) -> Result<()> {
    let v = send(&Request::Bars { set: Some(patch), land: None })?;
    print_render(&v);
    Ok(())
}

fn bars_cmd(args: &[String]) -> Result<()> {
    let arg = |i: usize, what: &str| args.get(i).cloned().ok_or_else(|| format!("bars {} needs {what}", args[0]));
    match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("status") => {
            let v = send(&Request::Bars { set: None, land: None })?;
            print_render(&v);
            println!("  settings: {}", v["bars"]);
            Ok(())
        }
        Some("on" | "off") => {
            let on = args[0].eq_ignore_ascii_case("on");
            bars_set(serde_json::json!({ "enabled": on }))?;
            if on {
                wait_installed(true);
            }
            Ok(())
        }
        Some("colour" | "color") => bars_set(serde_json::json!({ "colour": arg(1, "#rrggbb")? })),
        Some("count") => {
            let n: u32 = arg(1, "a number")?.parse().map_err(|_| "count: a whole number".to_string())?;
            bars_set(serde_json::json!({ "count": n }))
        }
        Some("gain") => {
            let g: f64 = arg(1, "a number")?.parse().map_err(|_| "gain: a number".to_string())?;
            bars_set(serde_json::json!({ "gain": g }))
        }
        Some("source") => {
            let src = arg(1, "random or audio")?.to_ascii_lowercase();
            bars_set(serde_json::json!({ "source": src }))
        }
        Some("land") => {
            let v = send(&Request::Bars { set: None, land: Some(true) })?;
            println!("landing spike fired (landings {})", v["render"]["landings"]);
            Ok(())
        }
        Some("set") => {
            let key = arg(1, "a setting name")?;
            let value = arg(2, "a value")?;
            let mut patch = serde_json::Map::new();
            patch.insert(key, setting_value(&value));
            bars_set(Value::Object(patch))
        }
        Some("reset") => {
            let current = send(&Request::Bars { set: None, land: None })?;
            let mut defaults = serde_json::to_value(caveman_fx::bars::BarSettings::default()).map_err(|e| e.to_string())?;
            defaults["enabled"] = current["bars"]["enabled"].clone();
            bars_set(defaults)
        }
        Some("meter") => meter(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0)),
        Some("rim") => rim::rim_cmd(&args[1..]),
        Some(other) => Err(format!("bars {other}? see caveman-trainer --help")),
    }
}

/// The audio capture, here in the trainer (no game): a text meter of the bands.
fn meter(seconds: f64) -> Result<()> {
    use caveman_fx::audio;
    audio::start();
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    let mut bands = [0.0f32; 16];
    println!("capturing the default output device for {seconds} s (play something)");
    let mut shown = false;
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        audio::bands(&mut bands);
        let st = audio::status();
        if !audio::running() {
            if st["status"].as_str().is_some_and(|s| s.starts_with("stopped")) {
                return Err(format!("audio capture: {}", st["status"].as_str().unwrap_or("")));
            }
            continue;
        }
        if !shown {
            println!("{}", st["status"].as_str().unwrap_or(""));
            shown = true;
        }
        let line: String = bands.iter().map(|&b| [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'][((b * 9.0).round() as usize).min(9)]).collect();
        let peak = bands.iter().copied().fold(0.0f32, f32::max);
        println!("[{line}] peak {peak:.2}, {} analyses", st["analyses"]);
    }
    audio::stop();
    Ok(())
}

// ---------------------------------------------------------------- attach (Caveman.exe at launch)

/// The staging ring's size as shipped (MB), the launcher's default too (`docs/skater-build-memory.md`).
const DEFAULT_RING_MB: u64 = 12288;

/// `attach [--ring MB|off] [--timeout S]`: waits for the game (Skate.exe with ReSkate loaded, no anti-cheat), puts the
/// DLL beside this exe in unless it already answers, then switches the staging ring on at the size given. Run detached
/// by Caveman.exe, so every line also goes to `%LOCALAPPDATA%\Caveman\fx\attach.log`.
fn attach(args: &[String]) -> Result<()> {
    let timeout = arg_value(args, "--timeout").and_then(|s| s.parse::<u64>().ok()).unwrap_or(600);
    let ring: Option<u64> = match arg_value(args, "--ring").as_deref().map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) if s == "off" || s == "0" || s == "none" => None,
        Some(s) => Some(s.parse().map_err(|_| format!("--ring {s}: a size in MB, or off"))?),
        None => Some(DEFAULT_RING_MB),
    };
    let log_path = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Caveman").join("fx").join("attach.log"));
    let note = |line: &str| {
        println!("{line}");
        if let Some(p) = &log_path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                use std::io::Write;
                let _ = writeln!(f, "{} {line}", clock());
            }
        }
    };
    note(&format!(
        "attach: waiting up to {timeout} s for skate. with ReSkate loaded{}",
        ring.map(|mb| format!(", then the staging ring at {mb} MB")).unwrap_or_else(|| ", the staging ring off".into())
    ));
    let t0 = Instant::now();
    loop {
        if t0.elapsed() > Duration::from_secs(timeout) {
            note("attach: no game within the time: giving up");
            return Err("no game within the time".into());
        }
        if proto::request(proto::PIPE, &Request::Ping, 300).is_ok() {
            note("attach: the DLL already answers");
            break;
        }
        if let Ok(game) = find_game(None) {
            if modules(game.pid).iter().any(|(n, _, _)| n.eq_ignore_ascii_case("ReSkate.dll")) {
                match inject(&["--yes".to_string()]) {
                    Ok(()) => {
                        note(&format!("attach: the DLL is in Skate.exe pid {}", game.pid));
                        break;
                    }
                    Err(e) if e.contains("anti-cheat") => {
                        note(&format!("attach: {e}"));
                        return Err(e);
                    }
                    Err(e) => note(&format!("attach: inject: {e}; trying again")),
                }
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    if let Some(mb) = ring {
        let req = Request::Mem { trace: None, min_kb: None, stop: None, report: None, ring: Some(true), ring_mb: Some(mb), ring_any: None };
        match proto::request(proto::PIPE, &req, 10_000) {
            Ok(v) => note(&format!(
                "attach: staging ring {} at {} MB (heap {} MB{})",
                if v["ring"]["enabled"] == true { "on" } else { "off" },
                v["ring"]["mb"],
                v["ring"]["heap_mb"],
                v["ring"]["last"].as_str().filter(|s| !s.is_empty()).map(|s| format!("; {s}")).unwrap_or_default()
            )),
            Err(e) => note(&format!("attach: the ring could not be set: {e}")),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- pose (read-only, for the board cam window)

/// `pose [--rate HZ] [--seconds S] [--pid N]`: the camera and the board as one JSON line per tick, read from outside the
/// game (nothing injected), until the seconds run out, the game goes (`{"gone": true}`), or the reader of this output
/// goes away. A line: `{"t": seconds, "camera": {"pos", "right", "up", "back", "fov"}, "board": {"pos", "right",
/// "up", "forward"} or null, "why": "ok" or why there is no board}`; without a camera only `t` and `why`.
fn pose(args: &[String]) -> Result<()> {
    use caveman_fx::scene;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let rate: f64 = arg_value(args, "--rate").and_then(|s| s.parse::<f64>().ok()).unwrap_or(60.0_f64).clamp(1.0, 240.0);
    let seconds: f64 = arg_value(args, "--seconds").and_then(|s| s.parse().ok()).unwrap_or(f64::INFINITY);
    let with_bones = args.iter().any(|a| a == "--bones");
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let process = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_READ | win::SYNCHRONIZE, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe (pid {}) for reading: error {}", game.pid, unsafe { win::GetLastError() }));
    }
    check_build(process.0)?;
    let base = modules(game.pid).into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case("skate.exe")).map(|(_, b, _)| b).ok_or("cannot find Skate.exe's base")?;
    win::set_target(process.0, game.pid);
    let rt = skater::Runtime::new(base).ok_or("the game's globals are not readable")?;
    scene::EXE_BASE.store(base, std::sync::atomic::Ordering::Release);
    let mut reader = scene::Reader::default();
    let mut out = std::io::stdout().lock();
    let period = Duration::from_secs_f64(1.0 / rate);
    let t0 = Instant::now();
    // The worlds: a thread of the game shows its world only while it works in it, so they are looked for again and
    // again (often until the board is found, then now and then) and what was seen is kept. Each look opens every
    // thread of the game, 65-78 ms here; done on the stream's own thread it stalled the pose on that beat (2026-11-01:
    // the wheels, and the tricks, froze in the board cam every two seconds), so the looks after the first are a
    // thread's own and the stream merges what it finds.
    let mut worlds: Vec<(usize, u32)> = rt.worlds();
    scene::publish_worlds(base, &worlds);
    let board_found = Arc::new(AtomicBool::new(false));
    let (worlds_tx, worlds_rx) = std::sync::mpsc::channel::<Vec<(usize, u32)>>();
    {
        let board_found = board_found.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(if board_found.load(Ordering::Relaxed) { 2000 } else { 500 }));
            if worlds_tx.send(rt.worlds()).is_err() {
                break;
            }
        });
    }
    loop {
        let now = Instant::now();
        if (now - t0).as_secs_f64() > seconds {
            break;
        }
        if unsafe { win::WaitForSingleObject(process.0, 0) } == win::WAIT_OBJECT_0 {
            let _ = writeln!(out, "{}", serde_json::json!({"gone": true}));
            break;
        }
        let mut looked = false;
        while let Ok(seen) = worlds_rx.try_recv() {
            looked = true;
            for w in seen {
                if !worlds.iter().any(|x| x.0 == w.0) {
                    worlds.push(w);
                }
            }
        }
        if looked {
            scene::publish_worlds(base, &worlds);
        }
        let t = (now - t0).as_secs_f64();
        let line = match reader.read(t) {
            Some(s) => {
                let c = &s.camera;
                let mut line = serde_json::json!({
                    "t": t,
                    "camera": {"pos": c.pos, "right": c.right, "up": c.up, "back": c.back, "fov": c.fov_y},
                    "board": s.board.map(|b| serde_json::json!({"pos": b.pos, "right": b.right, "up": b.up, "forward": b.forward})),
                    "why": reader.why,
                });
                if with_bones && s.board.is_some() {
                    match reader.bones(base, 64) {
                        Ok(lanes) => {
                            line["bones"] = serde_json::json!(lanes
                                .iter()
                                .map(|l| serde_json::json!([[l.scale[0], l.scale[1], l.scale[2]], l.rotation, [l.position[0], l.position[1], l.position[2]]]))
                                .collect::<Vec<_>>());
                        }
                        Err(why) => line["bones_why"] = serde_json::json!(why),
                    }
                }
                line
            }
            None => serde_json::json!({"t": t, "why": reader.why}),
        };
        board_found.store(reader.why == "ok", Ordering::Relaxed);
        if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
            // whoever read this went away
            break;
        }
        let spent = now.elapsed();
        if spent < period {
            std::thread::sleep(period - spent);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- probe-scene (read-only)

fn v3(v: [f32; 3]) -> String {
    format!("({:.3}, {:.3}, {:.3})", v[0], v[1], v[2])
}

/// The board rig's bone names, in the skeleton asset's order (`skateboard_default_skeleton`).
pub const BOARD_BONES: [&str; 16] = [
    "Reference",
    "AITrajectory",
    "Trajectory",
    "Connect",
    "Connect2",
    "Ground",
    "Skateboard_Root",
    "Deck",
    "Truck_Back",
    "eff_truckBack",
    "Right_WheelBack",
    "Left_WheelBack",
    "Truck_Front",
    "eff_truckFront",
    "Right_WheelFront",
    "Left_WheelFront",
];

/// `probe-bones`: the board rig's lanes from outside the game, as the animation system holds them.
fn probe_bones(args: &[String]) -> Result<()> {
    use caveman_fx::scene;
    let seconds: f64 = args.iter().find(|a| !a.starts_with("--") && a.parse::<f64>().is_ok()).and_then(|a| a.parse().ok()).unwrap_or(0.0);
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let process = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_READ, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe (pid {}) for reading: error {}", game.pid, unsafe { win::GetLastError() }));
    }
    check_build(process.0)?;
    let base = modules(game.pid).into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case("skate.exe")).map(|(_, b, _)| b).ok_or("cannot find Skate.exe's base")?;
    win::set_target(process.0, game.pid);
    let rt = skater::Runtime::new(base).ok_or("the game's globals are not readable")?;
    scene::EXE_BASE.store(base, std::sync::atomic::Ordering::Release);
    let mut reader = scene::Reader::default();
    let t0 = Instant::now();
    let end = t0 + Duration::from_secs_f64(seconds);
    let mut worlds: Vec<(usize, u32)> = Vec::new();
    loop {
        for w in rt.worlds() {
            if !worlds.iter().any(|x| x.0 == w.0) {
                worlds.push(w);
            }
        }
        scene::publish_worlds(base, &worlds);
        match reader.read(t0.elapsed().as_secs_f64()) {
            Some(s) if s.board.is_some() => {
                let b = s.board.unwrap();
                println!("{} board at {} right {} up {} forward {}", clock(), v3(b.pos), v3(b.right), v3(b.up), v3(b.forward));
                if args.iter().any(|a| a == "--skeleton") {
                    if let Some(entity) = reader.board_entity() {
                        match scene::read_skeleton_resource(base, entity, 0x800) {
                            Ok((at, raw)) => {
                                println!(
                                    "  skeleton resource at {at:#x}: count {} at +0xc; the first 0x80 bytes:",
                                    u32::from_le_bytes(raw[0xc..0x10].try_into().unwrap())
                                );
                                for row in raw[..0x80].chunks(32) {
                                    println!("    {}", row.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
                                }
                                // the skeleton asset's bone name hashes (skateboard_default_skeleton), looked for in the resource
                                const HASHES: [(u32, &str); 16] = [
                                    (3124388676, "Reference"),
                                    (1180344196, "AITrajectory"),
                                    (915436042, "Trajectory"),
                                    (161123025, "Connect"),
                                    (4014507825, "Connect2"),
                                    (1332089008, "Ground"),
                                    (3829058982, "Skateboard_Root"),
                                    (3730854838, "Deck"),
                                    (334595000, "Truck_Back"),
                                    (2240827619, "eff_truckBack"),
                                    (1910859892, "Right_WheelBack"),
                                    (1084298995, "Left_WheelBack"),
                                    (217051778, "Truck_Front"),
                                    (2797823535, "eff_truckFront"),
                                    (4196472086, "Right_WheelFront"),
                                    (2844544415, "Left_WheelFront"),
                                ];
                                let mut found = Vec::new();
                                for o in (0..raw.len() - 4).step_by(4) {
                                    let v = u32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
                                    if let Some((_, name)) = HASHES.iter().find(|(h, _)| *h == v) {
                                        found.push((o, *name));
                                    }
                                }
                                println!("  name hashes in the resource (offset: name): {found:?}");
                                // pointers inside the resource: followed for the hashes too
                                for o in (0..0x100).step_by(8) {
                                    let p = u64::from_le_bytes(raw[o..o + 8].try_into().unwrap()) as usize;
                                    if p > 0x10000 && p < 0x7fff_ffff_ffff {
                                        let mut sub = vec![0u8; 0x100];
                                        if win::read_into(p, &mut sub) {
                                            let mut hits = Vec::new();
                                            for so in (0..sub.len() - 4).step_by(4) {
                                                let v = u32::from_le_bytes(sub[so..so + 4].try_into().unwrap());
                                                if let Some((_, name)) = HASHES.iter().find(|(h, _)| *h == v) {
                                                    hits.push((so / 4, *name));
                                                }
                                            }
                                            if !hits.is_empty() {
                                                println!("  pointer at +{o:#x} -> {p:#x}: hashes {hits:?}");
                                            }
                                            let shorts: Vec<i16> = sub[..64].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                                            let ints: Vec<i32> = sub[..64].chunks(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect();
                                            if shorts.iter().take(16).all(|&v| (-1..16).contains(&v)) && shorts[0] == -1 {
                                                println!("  pointer at +{o:#x} -> {p:#x}: looks like parents (i16): {:?}", &shorts[..16]);
                                            }
                                            if ints.iter().take(16).all(|&v| (-1..16).contains(&v)) && ints[0] == -1 {
                                                println!("  pointer at +{o:#x} -> {p:#x}: looks like parents (i32): {:?}", &ints[..16]);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(why) => println!("  no skeleton resource: {why}"),
                        }
                    }
                }
                match reader.bones(base, 64) {
                    Ok(lanes) => {
                        println!("  {} bone(s): index name | position (parent frame) | rotation x y z w | scale", lanes.len());
                        for (i, l) in lanes.iter().enumerate() {
                            let name = BOARD_BONES.get(i).copied().unwrap_or("?");
                            println!(
                                "  {i:>2} {name:<16} | {:+.4} {:+.4} {:+.4} | {:+.4} {:+.4} {:+.4} {:+.4} | {:.3} {:.3} {:.3}",
                                l.position[0],
                                l.position[1],
                                l.position[2],
                                l.rotation[0],
                                l.rotation[1],
                                l.rotation[2],
                                l.rotation[3],
                                l.scale[0],
                                l.scale[1],
                                l.scale[2]
                            );
                        }
                    }
                    Err(why) => println!("  no bones: {why}"),
                }
            }
            Some(_) => println!("{} no board: {}", clock(), reader.why),
            None => println!("{} no scene: {}", clock(), reader.why),
        }
        if Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// The overlay's scene reads (`caveman_fx::scene`), run from outside the game with ReadProcessMemory.
fn probe_scene(args: &[String]) -> Result<()> {
    use caveman_fx::scene;
    let seconds: f64 = args.iter().find(|a| !a.starts_with("--") && a.parse::<f64>().is_ok()).and_then(|a| a.parse().ok()).unwrap_or(0.0);
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let process = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_READ, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe (pid {}) for reading: error {}", game.pid, unsafe { win::GetLastError() }));
    }
    check_build(process.0)?;
    let base = modules(game.pid).into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case("skate.exe")).map(|(_, b, _)| b).ok_or("cannot find Skate.exe's base")?;
    win::set_target(process.0, game.pid);
    let rt = skater::Runtime::new(base).ok_or("the game's globals are not readable")?;
    scene::EXE_BASE.store(base, std::sync::atomic::Ordering::Release);
    let mut reader = scene::Reader::default();
    let t0 = Instant::now();
    let end = t0 + Duration::from_secs_f64(seconds);
    let mut worlds: Vec<(usize, u32)> = Vec::new();
    loop {
        // a thread shows its world only while it works in it: keep what was seen
        for w in rt.worlds() {
            if !worlds.iter().any(|x| x.0 == w.0) {
                worlds.push(w);
            }
        }
        scene::publish_worlds(base, &worlds);
        let ctx = scene::CLIENT_CTX.load(std::sync::atomic::Ordering::Acquire);
        match reader.read(t0.elapsed().as_secs_f64()) {
            Some(s) => {
                let c = &s.camera;
                print!("{} camera {} fov {:.1}, looking {}", clock(), v3(c.pos), c.fov_y, v3(c.back.map(|x| -x)));
                match s.board {
                    Some(b) => println!(
                        "; board {} ({:.2} m from the camera), right {} up {} forward {}",
                        v3(b.pos),
                        caveman_fx::bars::length(caveman_fx::bars::sub(b.pos, c.pos)),
                        v3(b.right),
                        v3(b.up),
                        v3(b.forward)
                    ),
                    None => println!("; no board: {} (client world {ctx:#x})", reader.why),
                }
            }
            None => println!("{} no scene: {} (client world {ctx:#x}, {} world(s) seen)", clock(), reader.why, worlds.len()),
        }
        if Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}
