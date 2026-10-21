//! The library: every deck and wheel in the drop folders, as the build sees them (caveman-cli's scan: subfolder
//! prefixes, suffixes, settings files), with each one's effective settings, its folder group and its preview images.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use caveman_cli::names::{self, Found, Kind, PREVIEWS};
use caveman_cli::report::Reporter;
use caveman_core::settings::{self, Settings};
use serde::Serialize;
use serde_json::Value;

use crate::config::Config;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    pub kind: &'static str,
    /// The name in game.
    pub display: String,
    pub key: String,
    /// The PNG.
    pub source: String,
    /// The file name without `.png`.
    pub stem: String,
    /// The subfolder it is in, relative to its drop folder ("" at the top, "000", "Imported/Supreme").
    pub group: String,
    /// What the build will use: its settings file, else its suffixes as settings (decks).
    pub settings: Value,
    /// Whether it has a settings file.
    pub sidecar: bool,
    pub sidecar_path: String,
    pub notes: Vec<String>,
    /// Caveman's preview from the last build (decks: the art fitted to the deck; wheels: the menu thumbnail).
    pub preview: Option<String>,
    /// A wheel's colour ("#rrggbb").
    pub colour: Option<String>,
    /// The PNG's last change, in milliseconds (cache-busting for images).
    pub modified: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub kind: String,
    pub source: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub decks: Vec<LibraryItem>,
    pub wheels: Vec<LibraryItem>,
    /// Files the build would leave out, and why (a broken settings file, an unusable name).
    pub problems: Vec<Problem>,
    pub decks_dir: String,
    pub wheels_dir: String,
}

/// A writer the scan's JSON events go into.
#[derive(Clone, Default)]
pub struct Lines(pub Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn modified(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn group(root: &Path, path: &Path) -> String {
    let parent = path.parent().unwrap_or(root);
    parent.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default()
}

fn item(f: &Found, root: &Path, previews: &Path) -> LibraryItem {
    let stem = f.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let preview = previews.join(format!("{}.png", f.display));
    let (kind, settings, colour) = match f.kind {
        Kind::Deck => {
            let s = f.settings.clone().unwrap_or_else(|| Settings::from_effects(&f.effects));
            ("deck", serde_json::to_value(&s).unwrap_or(Value::Null), None)
        }
        Kind::Wheel => ("wheel", Value::Null, Some(format!("#{:02x}{:02x}{:02x}", f.colour[0], f.colour[1], f.colour[2]))),
    };
    LibraryItem {
        kind,
        display: f.display.clone(),
        key: f.key.clone(),
        source: f.path.display().to_string(),
        stem,
        group: group(root, &f.path),
        settings,
        sidecar: f.settings.is_some(),
        sidecar_path: settings::sidecar_path(&f.path).display().to_string(),
        notes: f.notes.clone(),
        preview: if preview.is_file() { Some(preview.display().to_string()) } else { None },
        colour,
        modified: modified(&f.path),
    }
}

/// Scans both drop folders (without changing anything: missing folders are reported empty).
pub fn scan(config: &Config) -> Library {
    let (decks_dir, wheels_dir) = (config.decks_dir(), config.wheels_dir());
    let lines = Lines::default();
    let mut report = Reporter::to(Box::new(lines.clone()), true);
    let mut found = Vec::new();
    if decks_dir.is_dir() {
        names::scan_decks(&decks_dir, "", true, &mut report, &mut found);
    }
    let n_decks = found.len();
    if wheels_dir.is_dir() {
        names::scan_wheels(&wheels_dir, "", true, &mut report, &mut found);
    }
    let (deck_previews, wheel_previews) = (decks_dir.join(PREVIEWS), wheels_dir.join(PREVIEWS));
    let decks = found[..n_decks].iter().map(|f| item(f, &decks_dir, &deck_previews)).collect();
    let wheels = found[n_decks..].iter().map(|f| item(f, &wheels_dir, &wheel_previews)).collect();
    let text = String::from_utf8_lossy(&lines.0.lock().unwrap()).into_owned();
    let problems = text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["event"] == "skipped")
        .map(|e| Problem {
            kind: e["kind"].as_str().unwrap_or("").into(),
            source: e["source"].as_str().unwrap_or("").into(),
            reason: e["reason"].as_str().unwrap_or("").into(),
        })
        .collect();
    Library { decks, wheels, problems, decks_dir: decks_dir.display().to_string(), wheels_dir: wheels_dir.display().to_string() }
}

/// Writes a deck's settings file after checking it as the build will (unknown fields, ranges).
pub fn save_settings(source: &Path, value: Value) -> Result<String, String> {
    let s: Settings = serde_json::from_value(value).map_err(|e| format!("settings: {e}"))?;
    s.validate().map_err(|e| e.to_string())?;
    let path = settings::sidecar_path(source);
    std::fs::write(&path, s.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Removes a deck's settings file (its suffixes apply again).
pub fn clear_settings(source: &Path) -> Result<bool, String> {
    let path: PathBuf = settings::sidecar_path(source);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(true);
    }
    Ok(false)
}
