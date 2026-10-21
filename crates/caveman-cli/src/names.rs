//! Deck and wheel names from file names (caveman.clean_name, caveman.Deck) and the drop-folder scan
//! (caveman.read_folder), plus the wheels folder (`Name.ff0000.png`) and settings files (settings.rs).

use std::path::{Path, PathBuf};

use caveman_core::anim;
use caveman_core::effects::{self, DeckEffects};
use caveman_core::settings::{self, Settings};

use crate::report::Reporter;

/// Caveman's own folder beside the PNGs, never read as decks (caveman.PREVIEWS).
pub const PREVIEWS: &str = "previews";

/// The ASCII a character becomes under Unicode NFKD with the non-ASCII marks dropped, for the characters deck names
/// are likely to use (Latin-1, Latin Extended-A, full-width forms, a few compatibility forms). caveman.clean_name uses
/// Python's full `unicodedata.normalize('NFKD', ...)`; characters outside this table are dropped, as most of the
/// rest (other scripts) are by Python too.
fn fold(c: char) -> &'static str {
    const LATIN1: [&str; 64] = [
        "A", "A", "A", "A", "A", "A", "", "C", "E", "E", "E", "E", "I", "I", "I", "I", //
        "", "N", "O", "O", "O", "O", "O", "", "", "U", "U", "U", "U", "Y", "", "", //
        "a", "a", "a", "a", "a", "a", "", "c", "e", "e", "e", "e", "i", "i", "i", "i", //
        "", "n", "o", "o", "o", "o", "o", "", "", "u", "u", "u", "u", "y", "", "y",
    ];
    const EXT_A: [&str; 128] = [
        "A", "a", "A", "a", "A", "a", "C", "c", "C", "c", "C", "c", "C", "c", "D", "d", //
        "", "", "E", "e", "E", "e", "E", "e", "E", "e", "E", "e", "G", "g", "G", "g", //
        "G", "g", "G", "g", "H", "h", "", "", "I", "i", "I", "i", "I", "i", "I", "i", //
        "I", "", "IJ", "ij", "J", "j", "K", "k", "", "L", "l", "L", "l", "L", "l", "L", //
        "l", "", "", "N", "n", "N", "n", "N", "n", "n", "", "", "O", "o", "O", "o", //
        "O", "o", "", "", "R", "r", "R", "r", "R", "r", "S", "s", "S", "s", "S", "s", //
        "S", "s", "T", "t", "T", "t", "", "", "U", "u", "U", "u", "U", "u", "U", "u", //
        "U", "u", "U", "u", "W", "w", "Y", "y", "Y", "Z", "z", "Z", "z", "Z", "z", "s",
    ];
    const ASCII: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";
    match c as u32 {
        0..=0x7f => "",
        0xa0 | 0xa8 | 0xaf | 0xb4 | 0xb8 => " ", // NFKD: a space (and a combining mark)
        0xaa => "a",
        0xb2 => "2",
        0xb3 => "3",
        0xb9 => "1",
        0xba => "o",
        0xbc => "14", // one quarter: 1, fraction slash, 4
        0xbd => "12",
        0xbe => "34",
        0xc0..=0xff => LATIN1[(c as u32 - 0xc0) as usize],
        0x100..=0x17f => EXT_A[(c as u32 - 0x100) as usize],
        0xfb00 => "ff",
        0xfb01 => "fi",
        0xfb02 => "fl",
        0xfb03 => "ffi",
        0xfb04 => "ffl",
        0xff01..=0xff5e => {
            let i = (c as u32 - 0xff01 + 1) as usize; // full-width ! to ~
            &ASCII[i..i + 1]
        }
        0x3000 => " ", // ideographic space
        _ => "",
    }
}

/// A file name as a deck name (caveman.clean_name): plain ASCII letters, digits, hyphens and single spaces (the
/// game's own item keys use hyphens, as in `sci-fi`).
pub fn clean_name(text: &str) -> String {
    let mut ascii = String::new();
    for c in text.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else {
            ascii.push_str(fold(c));
        }
    }
    let mut out = String::new();
    for c in ascii.chars() {
        let c = if c == '_' || c == '.' { ' ' } else { c };
        if !(c.is_ascii_alphanumeric() || c == ' ' || c == '-') {
            continue;
        }
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches(|c| c == ' ' || c == '-').to_string()
}

/// Python's `str.title()` for Caveman's effect words (ASCII letters and spaces).
pub fn title(word: &str) -> String {
    let mut out = String::new();
    let mut start = true;
    for c in word.chars() {
        if c.is_ascii_alphabetic() {
            out.push(if start { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() });
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

/// The display name of a named deck: the name, its effect words titled (`holo-frost` -> "Holo Frost"), then a hex
/// colour in capitals when one was given (caveman.Deck).
pub fn display_name(name: &str, words: &[String], colour: Option<[u8; 3]>) -> String {
    let mut parts = vec![name.to_string()];
    parts.extend(words.iter().map(|w| title(&w.replace('-', " "))));
    if let Some(c) = colour {
        parts.push(format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2]));
    }
    parts.join(" ")
}

/// An item key from a display name: `Own_` and the name with underscores for spaces (caveman.Deck.item_id).
pub fn item_key(display: &str) -> String {
    format!("Own_{}", display.replace(' ', "_"))
}

/// What the scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Deck,
    Wheel,
}

/// One image to build.
#[derive(Clone, Debug)]
pub struct Found {
    pub kind: Kind,
    pub path: PathBuf,
    pub display: String,
    pub key: String,
    /// A deck's effects (from its settings file, else its suffixes).
    pub effects: DeckEffects,
    /// A deck's settings file, when it has one.
    pub settings: Option<Settings>,
    /// A wheel's colour (white when the name has none).
    pub colour: [u8; 3],
    /// What was left out of the name's effects, and later what fitting the art did.
    pub notes: Vec<String>,
    /// An animated deck: the file is a clip ("gif" or "video", anim::clip_kind), not a PNG.
    pub clip: Option<&'static str>,
}

/// The entries of a folder, sorted as Python's `sorted(os.listdir(...))` (by code point).
fn sorted_entries(folder: &Path) -> Vec<String> {
    let mut out: Vec<String> = match std::fs::read_dir(folder) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => Vec::new(),
    };
    out.sort();
    out
}

/// Every deck PNG in the folder and its subfolders (caveman.read_folder): a subfolder's name goes before its decks'
/// names (`000\Cool Deck.png` is "000 - Cool Deck"); suffixes and a hex colour come off the end of the name; a
/// settings file beside the PNG overrides the suffixes.
pub fn scan_decks(folder: &Path, prefix: &str, top: bool, report: &mut Reporter, out: &mut Vec<Found>) {
    for entry in sorted_entries(folder) {
        let path = folder.join(&entry);
        if path.is_dir() {
            if entry.starts_with('.') || (top && entry.to_lowercase() == PREVIEWS) {
                continue;
            }
            scan_decks(&path, &format!("{prefix}{entry} - "), false, report, out);
            continue;
        }
        let clip = anim::clip_kind(&entry);
        if (!entry.to_lowercase().ends_with(".png") && clip.is_none()) || !path.is_file() {
            continue;
        }
        let (stem, words, colour) = peel(settings::art_stem(&entry), true);
        let found = match Settings::read_sidecar(&path) {
            Err(e) => {
                report.skipped(Kind::Deck, &path, &format!("its settings file {}: {e}", file_name(&settings::sidecar_path(&path))));
                continue;
            }
            Ok(Some(s)) => {
                let name = clean_name(&format!("{prefix}{}", s.name.as_deref().unwrap_or(&stem)));
                let display = if s.name.is_some() { name.clone() } else { display_name(&name, &s.effect_words(), s.colour()) };
                (name, display, s.effects(), Some(s), Vec::new())
            }
            Ok(None) => {
                let name = clean_name(&format!("{prefix}{stem}"));
                let (e, names, notes) = DeckEffects::from_suffixes(&words, colour);
                let display = display_name(&name, &names, colour);
                (name, display, e, None, notes)
            }
        };
        let (name, display, effects, settings, notes) = found;
        if name.is_empty() {
            report.skipped(Kind::Deck, &path, "no usable letters or digits in its name");
            continue;
        }
        let key = item_key(&display);
        out.push(Found { kind: Kind::Deck, path, display, key, effects, settings, colour: [255; 3], notes, clip });
    }
}

/// Every wheel PNG in the folder and its subfolders: `Name.ff0000.png` is the wheel "Name" in red (white without a
/// colour); subfolders prefix names as for decks.
pub fn scan_wheels(folder: &Path, prefix: &str, top: bool, report: &mut Reporter, out: &mut Vec<Found>) {
    for entry in sorted_entries(folder) {
        let path = folder.join(&entry);
        if path.is_dir() {
            if entry.starts_with('.') || (top && entry.to_lowercase() == PREVIEWS) {
                continue;
            }
            scan_wheels(&path, &format!("{prefix}{entry} - "), false, report, out);
            continue;
        }
        if !entry.to_lowercase().ends_with(".png") || !path.is_file() {
            continue;
        }
        let (stem, _, colour) = peel(&entry[..entry.len() - 4], false);
        let display = clean_name(&format!("{prefix}{stem}"));
        if display.is_empty() {
            report.skipped(Kind::Wheel, &path, "no usable letters or digits in its name");
            continue;
        }
        let key = item_key(&display);
        let colour = colour.unwrap_or([255, 255, 255]);
        out.push(Found { kind: Kind::Wheel, path, display, key, effects: DeckEffects::default(), settings: None, colour, notes: Vec::new(), clip: None });
    }
}

/// Peels effect suffixes (decks only) and one hex colour off the end of a file stem: `Deck.aura.00ffff` ->
/// ("Deck", ["aura"], 00ffff) (caveman.read_folder).
fn peel(stem: &str, effect_words: bool) -> (String, Vec<String>, Option<[u8; 3]>) {
    let mut stem = stem.to_string();
    let (mut words, mut colour) = (Vec::new(), None);
    while let Some((rest, last)) = stem.rsplit_once('.') {
        let lower = last.to_lowercase();
        if effect_words && effects::is_suffix(&lower) {
            words.insert(0, lower);
        } else if colour.is_none() && effects::hex_colour(last).is_some() {
            colour = effects::hex_colour(last);
        } else {
            break;
        }
        stem = rest.to_string();
    }
    (stem, words, colour)
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(clean_name("000 - Dan_Da.Dan"), "000 - Dan Da Dan");
        assert_eq!(clean_name("  --Café Déjà Vu--  "), "Cafe Deja Vu");
        assert_eq!(clean_name("ＡＢＣ ½"), "ABC 12");
        assert_eq!(clean_name("日本"), "");
        assert_eq!(title("holo cellular"), "Holo Cellular");
        let words: Vec<String> = vec!["holo-frost".into(), "spot".into()];
        assert_eq!(display_name("Deck", &words, Some([0, 255, 255])), "Deck Holo Frost Spot 00FFFF");
        assert_eq!(item_key("000 - Deck"), "Own_000_-_Deck");
        assert_eq!(peel("Deck.aura.00ffff", true), ("Deck".into(), vec!["aura".into()], Some([0, 255, 255])));
        assert_eq!(peel("Wheel.ff0000", false), ("Wheel".into(), vec![], Some([255, 0, 0])));
        assert_eq!(peel("Wheel.foil", false).0, "Wheel.foil");
    }
}
