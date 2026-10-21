//! `caveman-trainer`: Caveman FX's companion (docs/fx-hook-plan.md). Injects `caveman_fx.dll` into skate. and talks
//! to it over `\\.\pipe\caveman-fx`: live skater states and fx states, firing fx states by hand, loading rules,
//! unloading. `probe` runs the DLL's own asset scan and skater reads from outside, read-only, without injecting.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use caveman_fx::proto::{self, Request};
use caveman_fx::rules::{self, Engine, FxNames, RulesFile};
use caveman_fx::win::{self, Handle};
use caveman_fx::{game, scan, sha256, skater};
use serde_json::Value;

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
  unload                         put the game's getter back, stop the DLL's threads and free it
  probe [SECONDS] [--pid N]      read-only, no injection: the DLL's checks, scan and skater reads run from outside
                                 (ReadProcessMemory); then watch the states for SECONDS (default 0)";

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
        Some("-h" | "--help") | None => {
            println!("{USAGE}");
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
    if let Some((n, _, _)) = mods.iter().find(|(n, _, _)| n.to_ascii_lowercase().starts_with("caveman_fx")) {
        return Err(format!("{n} is already loaded: `caveman-trainer unload` first"));
    }
    if proto::request(proto::PIPE, &Request::Ping, 200).is_ok() {
        return Err(format!("{} already answers: another caveman_fx is loaded", proto::PIPE));
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
    let dll = dir.join(format!("caveman_fx-{tag}.dll"));
    if !dll.is_file() {
        std::fs::write(&dll, &bytes).map_err(|e| format!("{}: {e}", dll.display()))?;
    }
    println!("injecting {}", dll.display());
    let access = win::PROCESS_CREATE_THREAD | win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_OPERATION | win::PROCESS_VM_WRITE | win::PROCESS_VM_READ | win::SYNCHRONIZE;
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
        Some(k) => println!("  skater: record {} ({} pages, ctx {}); {} record(s) in {} world(s)", k["id"], k["pages"], k["ctx"].as_str().unwrap_or(""), v["records"].as_array().map_or(0, Vec::len), v["worlds"]),
        None => println!("  skater: none (menu or loading); {} world(s)", v["worlds"]),
    }
    if let Some(inputs) = v["inputs"].as_array() {
        let parts: Vec<String> = inputs
            .iter()
            .map(|i| {
                let mark = if i["assets"] == 0 { "?" } else if i["live"] == true { "" } else { "*" };
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
            let start = last_log.as_deref().and_then(|l| lines.iter().position(|x| *x == l)).map_or(if last_log.is_none() { lines.len() } else { 0 }, |p| p + 1);
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
    println!("scan: {} MB of resident private memory in {} regions, {} candidates, {:.1} s", s.bytes >> 20, s.regions, s.candidates_seen, t.elapsed().as_secs_f64());
    let mut hits = s.hits.clone();
    hits.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    let ours: Vec<&scan::Hit> = hits.iter().filter(|h| h.name.to_ascii_lowercase().starts_with(caveman_fx::worker::OUR_PREFIX)).collect();
    println!("Caveman fx assets: {}", if ours.is_empty() { "none (is Caveman-FX installed?)".to_string() } else { ours.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(", ") });
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
                    format!("{}{}", r.value.map(|v| if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v:.2}") }).unwrap_or("-".into()), if r.live { "" } else { "*" })
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
