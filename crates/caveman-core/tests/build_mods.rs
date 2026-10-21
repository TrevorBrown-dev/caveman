//! Builds real mods with the Rust engine into the session scratchpad, for Python's verifiers
//! (`tools/re/verify_newdeck.py`, `verify_newwheel.py`) and a comparison with Python's builds of the same input
//! (`scratchpad/buildport/compare_mods.py`). Needs the game (read only); run with `--ignored`.
//!
//! Inputs (prepared in the scratchpad): `buildport/decks` (a copy of the drop folder), `buildport/finishes-art`
//! (build_finishes.py's art PNGs), `wheels/testfaces` (the wheels agent's three faces).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use caveman_core::build::{self, BuildOptions, DeckLayers, DeckSpec, WheelSpec};
use caveman_core::effects::{self, BoardEffect, DeckEffects, Finish, FoilMode, Layers, NyanTrigger, ResolvedBoard};
use caveman_core::game::Game;
use caveman_core::image::Image;

const GAME: &str = r"E:\Games\ReSkate";

fn scratch() -> PathBuf {
    std::env::var_os("CAVEMAN_SCRATCH").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad")
    })
}

fn game() -> Arc<Game> {
    Arc::new(Game::open(Path::new(GAME)).unwrap())
}

fn log(line: &str) {
    println!("{line}");
}

/// caveman.clean_name for ASCII names: `_` and `.` to spaces, keep letters, digits, spaces and hyphens.
fn clean_name(text: &str) -> String {
    let spaced: String = text.chars().map(|c| if c == '_' || c == '.' { ' ' } else { c }).collect();
    let kept: String = spaced.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '-').collect();
    let mut out = String::new();
    for c in kept.chars() {
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches(|c| c == ' ' || c == '-').to_string()
}

/// Python's str.title() for the ASCII words Caveman names effects with.
fn title(word: &str) -> String {
    word.split(' ')
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + &cs.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// caveman.read_folder + caveman.Deck: (display, item key, path, effects, colour) of every PNG, subfolders as prefixes.
fn read_folder(folder: &Path, prefix: &str, top: bool, out: &mut Vec<(String, String, PathBuf, DeckEffects)>) {
    let mut entries: Vec<String> = std::fs::read_dir(folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    entries.sort();
    for entry in entries {
        let path = folder.join(&entry);
        if path.is_dir() {
            if entry.starts_with('.') || (top && entry.eq_ignore_ascii_case("previews")) {
                continue;
            }
            read_folder(&path, &format!("{prefix}{entry} - "), false, out);
            continue;
        }
        if !entry.to_lowercase().ends_with(".png") {
            continue;
        }
        let mut stem = entry[..entry.len() - 4].to_string();
        let (mut words, mut colour) = (Vec::new(), None);
        while let Some((rest, last)) = stem.rsplit_once('.') {
            if effects::is_suffix(&last.to_lowercase()) {
                words.insert(0, last.to_lowercase());
            } else if colour.is_none() && effects::hex_colour(last).is_some() {
                colour = effects::hex_colour(last);
            } else {
                break;
            }
            stem = rest.to_string();
        }
        let name = clean_name(&format!("{prefix}{stem}"));
        let (e, names, _) = DeckEffects::from_suffixes(&words, colour);
        let mut parts = vec![name];
        parts.extend(names.iter().map(|n| title(&n.replace('-', " "))));
        if let Some(c) = colour {
            parts.push(format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2]));
        }
        let display = parts.join(" ");
        let key = format!("Own_{}", display.replace(' ', "_"));
        out.push((display, key, path, e));
    }
}

fn decks_from_folder(game: &Game, folder: &Path) -> Vec<DeckSpec> {
    let wood = build::natural_wood(game).unwrap();
    let mut found = Vec::new();
    read_folder(folder, "", true, &mut found);
    found
        .into_iter()
        .map(|(display, key, path, e)| {
            let (art, _) = build::fit_art(&Image::read_png(&path).unwrap(), &wood).unwrap();
            DeckSpec::new(display, key, art, DeckLayers::Effects(e))
        })
        .collect()
}

fn wheels_from_faces() -> Vec<WheelSpec> {
    let faces = scratch().join("wheels").join("testfaces");
    [("Caveman Donut", [255u8, 0, 0]), ("Caveman Star", [0, 204, 255]), ("Caveman Checker", [255, 255, 255])]
        .iter()
        .map(|(n, c)| {
            WheelSpec::new(format!("000 - {n}"), format!("Own_000_-_{}", n.replace(' ', "_")), Image::read_png(faces.join(format!("{n}.png"))).unwrap(), *c)
        })
        .collect()
}

fn out_dir(name: &str) -> PathBuf {
    let d = scratch().join("buildport").join("rust").join(name);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
#[ignore]
fn build_drop_folder() {
    let g = game();
    let decks = decks_from_folder(&g, &scratch().join("buildport").join("decks"));
    assert!(!decks.is_empty(), "no PNGs in buildport/decks");
    let start = Instant::now();
    let report = build::build(g, &decks, &[], &out_dir("decks"), "Caveman-Decks", &BuildOptions::default(), &mut log).unwrap();
    println!("built {} decks in {:.2} s ({} KB of cas)", report.decks.len(), start.elapsed().as_secs_f64(), report.cas_bytes / 1024);
}

/// build_finishes.py's halves: the finish everywhere on the left half, `spot` on the right half.
fn halves(art: &Image) -> Vec<f64> {
    let s = effects::spot(art);
    (0..512 * 2048).map(|i| if i % 512 < 256 { 1.0 } else { s[i] }).collect()
}

fn band_rows(count: usize) -> Vec<(usize, usize)> {
    // np.linspace(79, 1970, count + 1).round() (numpy: k x step + start, the last exactly the stop), ends at 0 and 2048
    let step = (1970.0 - 79.0) / count as f64;
    let mut edges: Vec<usize> = (0..=count).map(|k| if k == count { 1970 } else { (k as f64 * step + 79.0).round_ties_even() as usize }).collect();
    edges[0] = 0;
    edges[count] = 2048;
    edges.windows(2).map(|w| (w[0], w[1])).collect()
}

/// build_finishes.py's decks from its saved art: the finishes bands, the holo modes, the FX and nyan diagnostics.
fn calibration_decks(game: &Game) -> Vec<DeckSpec> {
    let (dmask, dnormal) = build::default_layers(game).unwrap();
    let art_dir = scratch().join("buildport").join("finishes-art");
    let art = |name: &str| Image::read_png(art_dir.join(format!("art-{}.png", name.replace(' ', "-")))).unwrap().to_rgb();
    let mut decks = Vec::new();
    let mut push = |name: &str, art: Image, layers: Layers| {
        let display = format!("000 - {name}");
        decks.push(DeckSpec::new(display.clone(), format!("Own_{}", display.replace(' ', "_")), art, DeckLayers::Layers(layers)));
    };
    // Caveman Finishes: one band per finish (default first), finish layers per band
    let finishes = [Finish::Gloss, Finish::Matte, Finish::Foil, Finish::Chrome, Finish::Glitter];
    let a = art("Caveman Finishes");
    let w = halves(&a);
    let mut osk = dmask.to_rgb();
    let mut ny = dnormal.to_rgb();
    let rows = band_rows(1 + finishes.len());
    for (k, &(y0, y1)) in rows.iter().enumerate().skip(1) {
        let f = finishes[k - 1];
        let m = effects::mask(f, &dmask, Some(&w));
        osk.data[y0 * 512 * 3..y1 * 512 * 3].copy_from_slice(&m.data[y0 * 512 * 3..y1 * 512 * 3]);
        if let Some(n) = effects::normal_map(None, f == Finish::Glitter, &a, &dnormal, Some(&w)) {
            ny.data[y0 * 512 * 3..y1 * 512 * 3].copy_from_slice(&n.data[y0 * 512 * 3..y1 * 512 * 3]);
        }
    }
    push("Caveman Finishes", a, Layers { osk: Some(osk), ny: Some(ny), ..Layers::default() });
    for mode in [FoilMode::Pearl, FoilMode::Cellular, FoilMode::Frost, FoilMode::Vertical, FoilMode::Glitter] {
        let name = format!("Caveman Holo {}", mode.name());
        let a = art(&name);
        let w = halves(&a);
        let layers = Layers {
            osk: Some(effects::mask(Finish::Holo(mode), &dmask, Some(&w))),
            rgb: Some(effects::holo_map(Some(&w))),
            foilmode: Some(mode),
            ..Layers::default()
        };
        push(&name, a, layers);
    }
    for (name, effect, glow) in [
        ("FX Glow 7", effects::EFFECT_NONE, 64u8),
        ("FX Glow 63", effects::EFFECT_NONE, 128),
        ("FX Glow 511", effects::EFFECT_NONE, 192),
        ("FX Glow 4095", effects::EFFECT_NONE, 255),
        ("FX Neon", effects::EFFECT_NEON, 64),
    ] {
        let full = format!("Caveman {name}");
        let layers = Layers {
            osk: Some(dmask.to_rgb()),
            rgb: Some(effects::mark(&Image::new(512, 2048, 3), effect, effects::FX_SPEED, glow)),
            ..Layers::default()
        };
        push(&full, art(&full), layers);
    }
    for (label, steps, trigger) in [
        ("Nyan Ctrl", 8.0, NyanTrigger::Control),
        ("Nyan Neg S8", 8.0, NyanTrigger::Negative),
        ("Nyan Ratio S2", 2.0, NyanTrigger::Ratio),
        ("Nyan Ratio S8", 8.0, NyanTrigger::Ratio),
        ("Nyan Ratio S32", 32.0, NyanTrigger::Ratio),
    ] {
        let full = format!("Caveman {label}");
        push(&full, art(&full), Layers { board: Some(ResolvedBoard::Nyan { steps, trigger }), ..Layers::default() });
    }
    decks
}

#[test]
#[ignore]
fn build_calibration() {
    let g = game();
    let decks = calibration_decks(&g);
    let start = Instant::now();
    let report = build::build(g, &decks, &[], &out_dir("finishes"), "Caveman-Finishes-Test", &BuildOptions::default(), &mut log).unwrap();
    println!("built {} calibration decks in {:.2} s", report.decks.len(), start.elapsed().as_secs_f64());
}

#[test]
#[ignore]
fn build_wheels() {
    let g = game();
    let start = Instant::now();
    let report = build::build(g, &[], &wheels_from_faces(), &out_dir("wheels"), "Caveman-Wheels-Test", &BuildOptions::default(), &mut log).unwrap();
    println!("built {} wheels in {:.2} s", report.wheels.len(), start.elapsed().as_secs_f64());
    for w in &report.wheels {
        let mut bg = Image::filled(768, 768, &[60, 60, 70, 255]);
        bg = bg.alpha_composite(w.thumbnail.as_ref().expect("a wheel keeps its thumbnail"));
        bg.to_rgb().write_png(out_dir("wheels").join(format!("preview-{}.png", w.base))).unwrap();
    }
}

#[test]
#[ignore]
fn build_decks_and_wheels() {
    let g = game();
    let wood = build::natural_wood(&g).unwrap();
    let src = scratch().join("buildport").join("decks").join("000");
    let art = |file: &str| build::fit_art(&Image::read_png(src.join(file)).unwrap(), &wood).unwrap().0;
    let decks = vec![
        DeckSpec::new("000 - Combo Plain", "Own_000_-_Combo_Plain", art("Dan Da Dan.png"), DeckLayers::Effects(DeckEffects::default())),
        DeckSpec::new(
            "000 - Combo Holo Frost Aura",
            "Own_000_-_Combo_Holo_Frost_Aura",
            art("Dan Da Dan.png"),
            DeckLayers::Effects(DeckEffects {
                finish: Some(Finish::Holo(FoilMode::Frost)),
                emboss: Some(effects::EMBOSS_STRENGTH),
                board: Some(BoardEffect::Aura(Some([0, 255, 128]))),
                ..DeckEffects::default()
            }),
        ),
    ];
    let start = Instant::now();
    let report = build::build(g, &decks, &wheels_from_faces()[..2], &out_dir("combined"), "Caveman-Combined-Test", &BuildOptions::default(), &mut log).unwrap();
    println!("built {} decks and {} wheels in {:.2} s", report.decks.len(), report.wheels.len(), start.elapsed().as_secs_f64());
}
