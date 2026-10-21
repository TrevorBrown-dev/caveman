//! The app's settings: the game folder and the drop folders (and, for testing, where mods and the item cache go). Kept
//! as JSON in the app's config folder (`%APPDATA%\dev.caveman.app\config.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// The ReSkate game folder (Skate.exe, ReSkateLauncher.exe, Mods).
    pub game: String,
    /// The deck PNGs; empty for `<game>\Caveman Decks`.
    pub decks: String,
    /// The wheel PNGs; empty for `<game>\Caveman Wheels`.
    pub wheels: String,
    /// Where mods are written; empty for `<game>\Mods` (set elsewhere only to test builds).
    pub out: String,
    /// The item cache; empty for `%LOCALAPPDATA%\Caveman\cache`.
    pub cache: String,
}

impl Default for Config {
    fn default() -> Config {
        Config { game: caveman_cli::DEFAULT_GAME.into(), decks: String::new(), wheels: String::new(), out: String::new(), cache: String::new() }
    }
}

fn some(text: &str) -> Option<PathBuf> {
    let t = text.trim();
    if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    }
}

impl Config {
    /// The build's options for this configuration.
    pub fn options(&self) -> caveman_cli::Options {
        let mut o = caveman_cli::Options::new(self.game.trim());
        o.decks = some(&self.decks);
        o.wheels = some(&self.wheels);
        o.out = some(&self.out);
        o.cache = some(&self.cache);
        o.json = true;
        o
    }

    pub fn decks_dir(&self) -> PathBuf {
        self.options().decks_dir()
    }

    pub fn wheels_dir(&self) -> PathBuf {
        self.options().wheels_dir()
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.options().mods_dir()
    }

    pub fn game_dir(&self) -> PathBuf {
        PathBuf::from(self.game.trim())
    }

    pub fn load(file: &Path) -> Config {
        std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, file: &Path) -> Result<(), String> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(file, text).map_err(|e| format!("{}: {e}", file.display()))
    }
}
