//! The app's settings: the game folder and the drop folders (and, for testing, where mods and the item cache go), the
//! author name the built mods carry, the FX DLL's launch settings and the plugins' own settings (a free-form object the
//! front end owns). Kept as JSON in the app's config folder (`%APPDATA%\dev.caveman.app\config.json`). The parts
//! Caveman.exe (launcher/) shares are mirrored into `%LOCALAPPDATA%\Caveman\launcher.json` at every save, so both
//! launch paths agree.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The staging ring as shipped (MB), as launcher/ has it.
pub const DEFAULT_RING_MB: u64 = 12288;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// The ReSkate game folder (Skate.exe, ReSkateLauncher.exe, Mods).
    pub game: String,
    /// The deck PNGs; empty for `<game>\Caveman\Decks` (caveman_cli::folders).
    pub decks: String,
    /// The wheel PNGs; empty for `<game>\Caveman\Wheels`.
    pub wheels: String,
    /// Where mods are written; empty for `<game>\Mods` (set elsewhere only to test builds).
    pub out: String,
    /// The item cache; empty for `%LOCALAPPDATA%\Caveman\cache`.
    pub cache: String,
    /// `caveman-trainer.exe` for the plugins; empty for the one beside the app.
    pub trainer: String,
    /// The author in the built mods' manifest.json (empty: "Caveman").
    pub author: String,
    /// Put the FX DLL in when the game starts (whichever way it was started, while Caveman is open).
    pub attach: bool,
    /// The staging ring's size in MB (0: off).
    pub ring_mb: u64,
    /// The plugins' settings (which start with the game, the board cam's view, the OBS link's source): the front end's.
    pub plugins: Value,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            game: caveman_cli::DEFAULT_GAME.into(),
            decks: String::new(),
            wheels: String::new(),
            out: String::new(),
            cache: String::new(),
            trainer: String::new(),
            author: String::new(),
            attach: true,
            ring_mb: DEFAULT_RING_MB,
            plugins: json!({}),
        }
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
        o.author = Some(self.author.trim()).filter(|a| !a.is_empty()).map(str::to_string);
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
        std::fs::write(file, text).map_err(|e| format!("{}: {e}", file.display()))?;
        self.save_launcher_json()
    }

    /// `%LOCALAPPDATA%\Caveman\launcher.json`: what Caveman.exe reads at launch (the ring's size, whether the DLL goes
    /// in, the author).
    pub fn launcher_json_path() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Caveman").join("launcher.json"))
    }

    pub fn save_launcher_json(&self) -> Result<(), String> {
        let Some(path) = Config::launcher_json_path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let value = json!({ "ring_mb": self.ring_mb, "attach": self.attach, "author": self.author.trim() });
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?)).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_config_files_still_read() {
        let c: Config = serde_json::from_str(r#"{"game": "G", "decks": "", "wheels": "", "out": "", "cache": "", "trainer": ""}"#).unwrap();
        assert_eq!((c.game.as_str(), c.attach, c.ring_mb, c.author.as_str()), ("G", true, DEFAULT_RING_MB, ""));
        assert!(c.plugins.is_object());
        assert!(c.options().author.is_none());
        let mut c2 = c.clone();
        c2.author = " Trevor ".into();
        assert_eq!(c2.options().author.as_deref(), Some("Trevor"));
    }
}
