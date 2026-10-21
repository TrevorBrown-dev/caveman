//! Bake and play: the build (caveman_cli::run, the same build Caveman.exe runs at launch) with its JSON events sent to
//! the window as they happen, and starting ReSkate. Both refuse while the game is running (a build would change the
//! mods under it).

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use caveman_cli::report::Reporter;
use caveman_cli::{Outcome, Shaders};
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;

/// A console program run without flashing a console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Whether Skate.exe is running (as launcher/ checks it).
pub fn game_running() -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Skate.exe", "/NH", "/FO", "CSV"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("\"skate.exe\""),
        Err(_) => false,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    /// "built", "up_to_date" or "nothing".
    pub outcome: &'static str,
    pub decks: usize,
    pub wheels: usize,
    /// "built", "removed" or "none".
    pub shaders: &'static str,
    pub seconds: f64,
    pub mods_dir: String,
}

/// The build's JSON lines, each passed on as soon as it is complete.
struct Events<F: FnMut(Value) + Send> {
    pending: Vec<u8>,
    send: F,
}

impl<F: FnMut(Value) + Send> Write for Events<F> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let value = serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({"event": "log", "message": text}));
            (self.send)(value);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs the build for this configuration; `send` gets every event (`{"event": ...}`, as `caveman --json` prints).
/// `force` builds even when nothing changed (no `--if-changed`).
pub fn build(config: &Config, force: bool, send: impl FnMut(Value) + Send + 'static) -> Result<BuildResult, String> {
    let mods_dir = config.mods_dir();
    let writes_game_mods = mods_dir == config.game_dir().join("Mods");
    if writes_game_mods && game_running() {
        return Err("The game is running: close skate. before building (a build changes the mods it has loaded).".into());
    }
    if !config.game_dir().join("Skate.exe").is_file() {
        return Err(format!("{} is not a skate. game folder (no Skate.exe)", config.game_dir().display()));
    }
    let mut o = config.options();
    o.if_changed = !force;
    let mut report = Reporter::to(Box::new(Events { pending: Vec::new(), send }), true);
    let start = Instant::now();
    let outcome = caveman_cli::run(&o, &mut report).map_err(|e| e.to_string())?;
    let seconds = start.elapsed().as_secs_f64();
    let mods_dir = mods_dir.display().to_string();
    Ok(match outcome {
        Outcome::UpToDate => BuildResult { outcome: "up_to_date", decks: 0, wheels: 0, shaders: "none", seconds, mods_dir },
        Outcome::Nothing => BuildResult { outcome: "nothing", decks: 0, wheels: 0, shaders: "none", seconds, mods_dir },
        Outcome::Built { decks, wheels, shaders } => BuildResult {
            outcome: "built",
            decks,
            wheels,
            shaders: match shaders {
                Shaders::Built => "built",
                Shaders::Removed => "removed",
                Shaders::None => "none",
            },
            seconds,
            mods_dir,
        },
    })
}

/// Starts ReSkate (ReSkateLauncher.exe in the game folder, as Caveman.exe does after its build).
pub fn launch(game_dir: &Path) -> Result<(), String> {
    if game_running() {
        return Err("The game is already running.".into());
    }
    let exe = game_dir.join("ReSkateLauncher.exe");
    if !exe.is_file() {
        return Err(format!("no ReSkateLauncher.exe in {}", game_dir.display()));
    }
    Command::new(&exe).current_dir(game_dir).spawn().map(|_| ()).map_err(|e| format!("{}: {e}", exe.display()))
}

/// Opens a folder (or shows a file) in Explorer.
pub fn reveal(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("explorer.exe");
    if path.is_file() {
        cmd.arg(format!("/select,{}", path.display()));
    } else {
        cmd.arg(path);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}
