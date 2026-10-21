//! Caveman.exe: put it beside ReSkateLauncher.exe and launch the game through it. It builds the decks in
//! `Caveman Decks` and the wheels in `Caveman Wheels` (only when something changed; the build runs in this process,
//! no Python), then starts ReSkate's own launcher with the same arguments and exits.
//!
//! Options for testing, taken out before ReSkate's launcher sees the rest: `--caveman-no-launch` (build, do not start
//! ReSkate), `--caveman-game DIR`, `--caveman-decks DIR`, `--caveman-wheels DIR`, `--caveman-out DIR`,
//! `--caveman-cache DIR` (the item cache; default `%LOCALAPPDATA%\Caveman\cache`).

use std::env;
use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::Command;

use caveman_cli::report::Reporter;
use caveman_cli::{run, Options};

const LAUNCHER: &str = "ReSkateLauncher.exe";

fn main() {
    let exe = env::current_exe().expect("cannot find Caveman.exe's own path");
    let mut game = exe.parent().expect("Caveman.exe has no folder").to_path_buf();
    let (mut no_launch, mut decks, mut wheels, mut out, mut cache) = (false, None, None, None, None);
    let mut args = Vec::new();
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--caveman-no-launch" => no_launch = true,
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
