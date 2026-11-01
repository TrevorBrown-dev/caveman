//! Which decks and wheels go into the game: a drop folder's `.caveman-off.json` lists the files switched OFF (the
//! Caveman app's switch per item), by their path relative to the folder. Everything not listed is on, so a folder
//! without the file builds as it always did. The build skips what is off (and says so), the fingerprint counts the
//! file, and the app reads it to show each item's switch.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The file, at the top of a drop folder (a dot file: the scans never take it for a deck).
pub const OFF_FILE: &str = ".caveman-off.json";

/// The files switched off in one drop folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OffList {
    /// Relative paths, lower-cased, with `/` between folders.
    off: BTreeSet<String>,
}

/// A file's key in the list: its path relative to the folder, lower-cased, `/` between folders. None when the file
/// is not under the folder.
pub fn key(folder: &Path, file: &Path) -> Option<String> {
    let (f, p) = (absolute(folder), absolute(file));
    let rel = p.strip_prefix(&f).ok()?;
    let text = rel.to_string_lossy().replace('\\', "/").to_lowercase();
    (!text.is_empty()).then_some(text)
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

impl OffList {
    pub fn path(folder: &Path) -> PathBuf {
        folder.join(OFF_FILE)
    }

    /// The folder's list; empty when there is no file. A file that cannot be read counts as empty too (nothing is
    /// switched off by a broken file), with the reason returned beside the list.
    pub fn load(folder: &Path) -> (OffList, Option<String>) {
        let path = OffList::path(folder);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (OffList::default(), None),
            Err(e) => return (OffList::default(), Some(format!("{}: {e}", path.display()))),
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return (OffList::default(), Some(format!("{}: {e}", path.display()))),
        };
        let off = value["off"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).map(|s| s.replace('\\', "/").to_lowercase()).collect();
        (OffList { off }, None)
    }

    pub fn is_off(&self, folder: &Path, file: &Path) -> bool {
        key(folder, file).is_some_and(|k| self.off.contains(&k))
    }

    pub fn is_empty(&self) -> bool {
        self.off.is_empty()
    }

    pub fn len(&self) -> usize {
        self.off.len()
    }

    /// Switches one file off or on; returns whether the list changed.
    pub fn set(&mut self, folder: &Path, file: &Path, off: bool) -> bool {
        let Some(k) = key(folder, file) else { return false };
        if off {
            self.off.insert(k)
        } else {
            self.off.remove(&k)
        }
    }

    /// Writes the list (removes the file when nothing is off).
    pub fn save(&self, folder: &Path) -> std::io::Result<()> {
        let path = OffList::path(folder);
        if self.off.is_empty() {
            match std::fs::remove_file(&path) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        let value = json!({ "version": 1, "off": self.off.iter().collect::<Vec<_>>() });
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&value).unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("caveman-off-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        let a = dir.join("Cool Deck.png");
        let b = dir.join("Sub").join("Other.gif");
        assert_eq!(key(&dir, &a).as_deref(), Some("cool deck.png"));
        assert_eq!(key(&dir, &b).as_deref(), Some("sub/other.gif"));
        assert_eq!(key(&dir, Path::new(r"C:\elsewhere\x.png")), None);
        let mut list = OffList::default();
        assert!(list.set(&dir, &b, true));
        assert!(!list.set(&dir, &b, true), "already off");
        assert!(list.is_off(&dir, &b) && !list.is_off(&dir, &a));
        list.save(&dir).unwrap();
        let (again, problem) = OffList::load(&dir);
        assert_eq!((again, problem), (list.clone(), None));
        assert!(list.set(&dir, &b, false));
        list.save(&dir).unwrap();
        assert!(!OffList::path(&dir).exists(), "an empty list leaves no file");
        std::fs::write(OffList::path(&dir), "not json").unwrap();
        let (broken, problem) = OffList::load(&dir);
        assert!(broken.is_empty() && problem.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
