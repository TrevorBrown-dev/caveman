//! Where Caveman keeps a player's things: one `Caveman` folder beside the game (`<game>\Caveman`), with `Decks`,
//! `Wheels`, `Imports`, `Packs`, `Templates` and `Archive` in it, next to the FX DLL and the trainer (Trevor,
//! 2026-10-30: "all things caveman (minus the exe) in one folder"). Until then they were `Caveman Decks`, `Caveman
//! Wheels`, `Caveman Imports`, `Caveman Packs`, `Caveman Templates` and `Caveman Archive` in the game folder itself:
//! `adopt_old_folders` moves those in on the first run of any Caveman program (a rename on the same drive, instant
//! whatever the size) and says so, and `modern` maps a path saved in settings before the move. A deck's name in game
//! comes from its path inside the drop folder, so the move renames nothing the game sees.

use std::path::{Path, PathBuf};

/// The folder beside the game.
pub const ROOT: &str = "Caveman";

/// The folders in it, each with the name it had in the game folder before 2026-10-30.
pub const FOLDERS: [(&str, &str); 6] = [
    ("Decks", "Caveman Decks"),
    ("Wheels", "Caveman Wheels"),
    ("Imports", "Caveman Imports"),
    ("Packs", "Caveman Packs"),
    ("Templates", "Caveman Templates"),
    ("Archive", "Caveman Archive"),
];

pub fn root(game: &Path) -> PathBuf {
    game.join(ROOT)
}

/// The deck PNGs (the drop folder).
pub fn decks(game: &Path) -> PathBuf {
    root(game).join("Decks")
}

/// The wheel PNGs.
pub fn wheels(game: &Path) -> PathBuf {
    root(game).join("Wheels")
}

/// Decks imported from installed mods (`caveman import`), kept apart from the drop folder.
pub fn imports(game: &Path) -> PathBuf {
    root(game).join("Imports")
}

/// Thunderstore packages the pack generator wrote.
pub fn packs(game: &Path) -> PathBuf {
    root(game).join("Packs")
}

/// The deck and wheel templates for artists.
pub fn templates(game: &Path) -> PathBuf {
    root(game).join("Templates")
}

/// Mods and decks moved out of the way (never deleted).
pub fn archive(game: &Path) -> PathBuf {
    root(game).join("Archive")
}

/// Grips: pictures printed on the grip tape and plain grip colours (names::scan_grips; 2026-10-31).
pub fn grips(game: &Path) -> PathBuf {
    root(game).join("Grips")
}

/// Grip patterns: masks that cut the grip away where the wood should show (names::scan_patterns).
pub fn patterns(game: &Path) -> PathBuf {
    root(game).join("Patterns")
}

/// Moves the old folders into the Caveman folder: each that exists while its new place does not. One line per move
/// (or failure); nothing when there is nothing to do, the usual case (six `exists` checks).
pub fn adopt_old_folders(game: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let root = root(game);
    for (new, old) in FOLDERS {
        let from = game.join(old);
        let to = root.join(new);
        if !from.is_dir() || to.exists() {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&root) {
            lines.push(format!("Could not make {}: {e}", root.display()));
            break;
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => lines.push(format!("Moved {} into {}", from.display(), to.display())),
            Err(e) => lines.push(format!("Could not move {} into {}: {e}", from.display(), to.display())),
        }
    }
    lines
}

/// A folder path from settings saved before the move: when it names an old folder of this game that is gone while
/// its new place exists, the new place; else itself.
pub fn modern(game: &Path, configured: &Path) -> PathBuf {
    if configured.exists() {
        return configured.to_path_buf();
    }
    for (new, old) in FOLDERS {
        if same_path(configured, &game.join(old)) {
            let to = root(game).join(new);
            if to.is_dir() {
                return to;
            }
        }
    }
    configured.to_path_buf()
}

/// The same path by its text: letter case and the kind of slash aside, trailing separators ignored.
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("caveman-folders-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn old_folders_move_in_once() {
        let game = scratch("adopt");
        std::fs::create_dir_all(game.join("Caveman Decks").join("000")).unwrap();
        std::fs::write(game.join("Caveman Decks").join("000").join("Cool Deck.png"), b"x").unwrap();
        std::fs::create_dir_all(game.join("Caveman Imports").join("pack")).unwrap();
        // a new folder already there is left alone, and so is the old one of the same kind
        std::fs::create_dir_all(game.join("Caveman").join("Wheels")).unwrap();
        std::fs::create_dir_all(game.join("Caveman Wheels")).unwrap();
        let lines = adopt_old_folders(&game);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| l.starts_with("Moved ")), "{lines:?}");
        assert!(decks(&game).join("000").join("Cool Deck.png").is_file());
        assert!(imports(&game).join("pack").is_dir());
        assert!(!game.join("Caveman Decks").exists());
        assert!(game.join("Caveman Wheels").is_dir(), "not moved over an existing new folder");
        assert!(adopt_old_folders(&game).is_empty(), "nothing the second time");
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn nothing_to_adopt_is_quiet() {
        let game = scratch("quiet");
        assert!(adopt_old_folders(&game).is_empty());
        assert!(!root(&game).exists(), "no folder made for nothing");
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn settings_from_before_the_move_follow_it() {
        let game = scratch("modern");
        std::fs::create_dir_all(decks(&game)).unwrap();
        let old = game.join("Caveman Decks");
        assert_eq!(modern(&game, &old), decks(&game));
        let shouty = PathBuf::from(old.to_string_lossy().to_uppercase().replace('\\', "/") + "/");
        assert_eq!(modern(&game, &shouty), decks(&game), "case and slashes aside");
        let elsewhere = game.join("My Decks");
        assert_eq!(modern(&game, &elsewhere), elsewhere, "another path stays");
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(modern(&game, &old), old, "an old folder still there stays");
        let _ = std::fs::remove_dir_all(&game);
    }

    #[test]
    fn the_layout() {
        let g = Path::new("G");
        assert_eq!(decks(g), PathBuf::from("G").join("Caveman").join("Decks"));
        assert_eq!(wheels(g), PathBuf::from("G").join("Caveman").join("Wheels"));
        assert_eq!(packs(g), PathBuf::from("G").join("Caveman").join("Packs"));
        assert_eq!(templates(g), PathBuf::from("G").join("Caveman").join("Templates"));
        assert_eq!(archive(g), PathBuf::from("G").join("Caveman").join("Archive"));
    }
}
