//! Caveman.exe: put it beside ReSkateLauncher.exe and launch the game through it. It builds the decks in
//! `Caveman\Decks` and the wheels in `Caveman\Wheels` (the one Caveman folder beside the game, caveman_cli::folders;
//! the folders from before 2026-10-30 move into it on the first run; only when something changed; the build runs in
//! this process, no Python), then starts ReSkate's own launcher with the same arguments and exits.
//!
//! Once the game is up it also puts Caveman's FX DLL in with the staging ring on (`Caveman\caveman-trainer.exe
//! attach`, detached, when that folder is installed beside this exe): the ring stands in for the skater build's
//! committed staging buffers (`docs/skater-build-memory.md`). The ring's size comes from `--caveman-ring MB` (or
//! `off`), else `%LOCALAPPDATA%\Caveman\launcher.json` (`{"ring_mb": 12288}` or `{"ring_mb": 0}` for off, and
//! `"attach": false` for no DLL at all), else [`DEFAULT_RING_MB`].
//!
//! Options for testing, taken out before ReSkate's launcher sees the rest: `--caveman-no-launch` (build, do not start
//! ReSkate), `--caveman-no-attach`, `--caveman-ring MB|off`, `--caveman-game DIR`, `--caveman-decks DIR`,
//! `--caveman-wheels DIR`, `--caveman-out DIR`, `--caveman-cache DIR` (the item cache; default
//! `%LOCALAPPDATA%\Caveman\cache`).

use std::env;
use std::io::{self, BufRead, Write};
use std::os::windows::process::CommandExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use caveman_cli::report::Reporter;
use caveman_cli::{run, Options};

const LAUNCHER: &str = "ReSkateLauncher.exe";
/// The FX DLL's companion, installed in `Caveman\` beside this exe with `caveman_fx.dll`.
const TRAINER: &str = "caveman-trainer.exe";
/// The staging ring as shipped (MB): a build's batch of staging buffers and most of the next one.
const DEFAULT_RING_MB: u64 = 12288;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;

fn main() {
    let exe = env::current_exe().expect("cannot find Caveman.exe's own path");
    let mut game = exe.parent().expect("Caveman.exe has no folder").to_path_buf();
    let (mut no_launch, mut decks, mut wheels, mut out, mut cache) = (false, None, None, None, None);
    let (mut no_attach, mut ring_arg) = (false, None);
    let mut args = Vec::new();
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--caveman-no-launch" => no_launch = true,
            "--caveman-no-attach" => no_attach = true,
            "--caveman-ring" => ring_arg = it.next(),
            "--caveman-game" => game = it.next().map(PathBuf::from).unwrap_or(game),
            "--caveman-decks" => decks = it.next().map(PathBuf::from),
            "--caveman-wheels" => wheels = it.next().map(PathBuf::from),
            "--caveman-out" => out = it.next().map(PathBuf::from),
            "--caveman-cache" => cache = it.next().map(PathBuf::from),
            _ => args.push(a),
        }
    }

    if !game.join(LAUNCHER).is_file() {
        fail(&format!("{} is not beside Caveman.exe (in {}).", LAUNCHER, game.display()), false);
        return;
    }
    if game_running() {
        println!("Caveman: skate. is already running, so the decks are left as they are.");
    } else {
        let mut o = Options::new(&game);
        o.decks = decks;
        o.wheels = wheels;
        o.out = out;
        o.cache = cache;
        o.if_changed = true;
        o.author = launcher_json().and_then(|s| s["author"].as_str().map(str::trim).filter(|a| !a.is_empty()).map(str::to_string));
        build(&o);
    }
    if no_launch {
        println!("Caveman: not starting ReSkate (--caveman-no-launch).");
        return;
    }
    match Command::new(game.join(LAUNCHER)).args(&args).current_dir(&game).spawn() {
        Ok(_) => {}
        Err(e) => fail(&format!("could not start {}: {}", LAUNCHER, e), false),
    }
    let (attach_wanted, ring) = attach_settings(ring_arg.as_deref());
    if !no_attach && attach_wanted {
        attach(&game, ring);
    }
}

/// `%LOCALAPPDATA%\Caveman\launcher.json`, the settings the Caveman app shares with this launcher (`ring_mb`,
/// `attach`, `author`), when it is there and reads.
fn launcher_json() -> Option<serde_json::Value> {
    env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("Caveman").join("launcher.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
}

/// Whether the FX DLL goes in, and the staging ring's size (None: the ring off): the `--caveman-ring` argument, else
/// `%LOCALAPPDATA%\Caveman\launcher.json`, else the default.
fn attach_settings(ring_arg: Option<&str>) -> (bool, Option<u64>) {
    let parse = |s: &str| -> Option<Option<u64>> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(None),
            t => t.parse::<u64>().ok().map(Some),
        }
    };
    if let Some(v) = ring_arg.and_then(parse) {
        return (true, v);
    }
    if let Some(s) = launcher_json() {
        let attach = s["attach"].as_bool().unwrap_or(true);
        let ring = match &s["ring_mb"] {
            serde_json::Value::Number(n) => n.as_u64().filter(|&mb| mb > 0),
            serde_json::Value::String(t) => parse(t).flatten(),
            _ => Some(DEFAULT_RING_MB),
        };
        return (attach, ring);
    }
    (true, Some(DEFAULT_RING_MB))
}

/// Starts `caveman-trainer attach` detached: it waits for the game, puts the FX DLL in and switches the staging ring
/// on, logging to `%LOCALAPPDATA%\Caveman\fx\attach.log`. Nothing happens when the trainer is not installed.
fn attach(game: &Path, ring: Option<u64>) {
    let trainer = game.join("Caveman").join(TRAINER);
    if !trainer.is_file() {
        return;
    }
    let ring_arg = ring.map(|mb| mb.to_string()).unwrap_or_else(|| "off".into());
    let spawned = Command::new(&trainer)
        .args(["attach", "--ring", &ring_arg])
        .current_dir(game)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn();
    match spawned {
        Ok(_) => println!(
            "Caveman: the FX DLL goes in once the game is up ({}; %LOCALAPPDATA%\\Caveman\\fx\\attach.log says how it went).",
            match ring {
                Some(mb) => format!("staging ring {} MB", mb),
                None => "staging ring off".to_string(),
            }
        ),
        Err(e) => println!("Caveman: could not start the attach helper ({}): {}", trainer.display(), e),
    }
}

/// Builds the decks and wheels; on any failure, says so and waits before ReSkate starts, so a deck that did not
/// build is never a surprise in game.
fn build(o: &Options) {
    println!("Caveman: checking your decks...");
    let mut report = Reporter::text();
    match panic::catch_unwind(AssertUnwindSafe(|| run(o, &mut report))) {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => fail(&format!("the deck build failed: {e}"), true),
        Err(_) => fail("the deck build stopped unexpectedly; see the lines above.", true),
    }
}

fn game_running() -> bool {
    Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Skate.exe", "/NH"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).to_lowercase().contains("skate.exe"))
        .unwrap_or(false)
}

/// Reports a problem. With `then_launch`, ReSkate still starts after Enter; otherwise this only waits so the message
/// can be read.
fn fail(message: &str, then_launch: bool) {
    println!("\nCaveman: {}", message);
    if then_launch {
        print!("Press Enter to start ReSkate anyway, or close this window. ");
    } else {
        print!("Press Enter to close. ");
    }
    let _ = io::stdout().flush();
    let _ = io::stdin().lock().lines().next();
}
