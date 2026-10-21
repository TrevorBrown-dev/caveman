//! The app's back end against a scratch copy of the drop folder (never the game's own Mods). Needs the game (read only)
//! and the drop folder at E:\Games\ReSkate: `cargo test -- --include-ignored`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use caveman_app::{art, bake, config::Config, library};
use serde_json::json;

const GAME: &str = r"E:\Games\ReSkate";

fn scratch(name: &str) -> PathBuf {
    let root = std::env::var("CAVEMAN_APP_SCRATCH").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("caveman-app-tests"));
    let dir = root.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A copy of the real drop folder's `000` decks plus one wheel, and a config building into the scratch folder.
fn setup(name: &str) -> (Config, PathBuf) {
    let root = scratch(name);
    let decks = root.join("decks").join("000");
    std::fs::create_dir_all(&decks).unwrap();
    for e in std::fs::read_dir(Path::new(GAME).join("Caveman Decks").join("000")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map(|x| x == "png").unwrap_or(false) {
            std::fs::copy(&p, decks.join(p.file_name().unwrap())).unwrap();
        }
    }
    let wheels = root.join("wheels");
    std::fs::create_dir_all(&wheels).unwrap();
    let template = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/wheel_template_1024.png");
    std::fs::copy(template, wheels.join("Test Wheel.ff0000.png")).unwrap();
    let config = Config {
        game: GAME.into(),
        decks: root.join("decks").display().to_string(),
        wheels: wheels.display().to_string(),
        out: root.join("mods").display().to_string(),
        cache: root.join("cache").display().to_string(),
    };
    (config, root)
}

#[test]
#[ignore]
fn scan_settings_and_fit() {
    let (config, root) = setup("scan");
    let lib = library::scan(&config);
    let pngs = std::fs::read_dir(root.join("decks").join("000")).unwrap().count();
    assert_eq!(lib.decks.len() + lib.problems.len(), pngs, "every PNG is a deck or a problem");
    assert!(lib.decks.iter().all(|d| d.group == "000" && d.display.starts_with("000 - ")));
    assert_eq!(lib.wheels.len(), 1);
    assert_eq!(lib.wheels[0].colour.as_deref(), Some("#ff0000"));
    let plain = lib.decks.iter().find(|d| d.display == "000 - Dan Da Dan").expect("the plain deck");
    assert!(!plain.sidecar);

    // A settings file turns the plain deck into a holo frost deck, as the build reads it.
    let saved = library::save_settings(Path::new(&plain.source), json!({"version": 1, "finish": {"kind": "holo", "mode": "frost"}})).unwrap();
    assert!(Path::new(&saved).is_file());
    let lib2 = library::scan(&config);
    let d = lib2.decks.iter().find(|d| d.source == plain.source).unwrap();
    assert!(d.sidecar);
    assert_eq!(d.display, "000 - Dan Da Dan Holo Frost");
    assert_eq!(d.settings["finish"]["mode"], "frost");
    // Bad settings are refused before anything is written.
    assert!(library::save_settings(Path::new(&plain.source), json!({"version": 1, "finish": {"kind": "sparkly"}})).is_err());
    assert!(library::clear_settings(Path::new(&plain.source)).unwrap());
    let lib3 = library::scan(&config);
    assert!(!lib3.decks.iter().find(|d| d.source == plain.source).unwrap().sidecar);

    // The art as the build fits it: 512 x 2048.
    let state = art::ArtState::default();
    let (path, _) = art::fitted(&state, Path::new(GAME), Path::new(&plain.source), &root.join("art")).unwrap();
    let img = caveman_core::image::Image::read_png(&path).unwrap();
    assert_eq!((img.width, img.height), (512, 2048));
    // Cached the second time.
    let (again, _) = art::fitted(&state, Path::new(GAME), Path::new(&plain.source), &root.join("art")).unwrap();
    assert_eq!(path, again);
}

/// Writes app/mock/library.json (for `pnpm dev:mock`): the library of a scratch drop folder built into the scratch
/// folder (so it has previews), each deck's fitted art, and the folders the mock may serve files from.
#[test]
#[ignore]
fn dump_mock_fixture() {
    let (config, root) = setup("mock");
    // the orientation pattern (TOP up, L left, R right on the bottom as the game shows it), in its own group
    let test = root.join("decks").join("Test");
    std::fs::create_dir_all(&test).unwrap();
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test_pattern.png"), test.join("Test Pattern.png")).unwrap();
    // a few settings files, so the fixture shows the editor's range
    let lib0 = library::scan(&config);
    let pick = |name: &str| lib0.decks.iter().find(|d| d.display == name).map(|d| d.source.clone());
    if let Some(src) = pick("000 - Dan Da Dan") {
        library::save_settings(Path::new(&src), json!({"version": 1, "finish": {"kind": "holo", "mode": "cellular", "spot": true}, "shader": {"effect": "cycle", "speed": 0.4, "glow": 0.0}})).unwrap();
    }
    bake::build(&config, true, |_| {}).unwrap();
    let lib = library::scan(&config);
    let state = art::ArtState::default();
    let mut fitted = serde_json::Map::new();
    for d in &lib.decks {
        let (path, notes) = art::fitted(&state, Path::new(GAME), Path::new(&d.source), &root.join("art")).unwrap();
        fitted.insert(d.source.clone(), json!({"path": path.display().to_string(), "notes": notes}));
    }
    let fixture = json!({
        "library": lib,
        "config": config,
        "fitted": fitted,
        "roots": [root.display().to_string(), std::path::PathBuf::from(GAME).join("Caveman Decks").display().to_string()],
    });
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mock/library.json");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, serde_json::to_string_pretty(&fixture).unwrap()).unwrap();
    println!("wrote {} ({} decks, {} wheels)", out.display(), lib.decks.len(), lib.wheels.len());
}

#[test]
#[ignore]
fn build_into_scratch() {
    let (config, root) = setup("build");
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let r = bake::build(&config, true, move |e| sink.lock().unwrap().push(e)).unwrap();
    assert_eq!(r.outcome, "built");
    let pngs = std::fs::read_dir(root.join("decks").join("000")).unwrap().filter(|e| e.as_ref().unwrap().path().is_file()).count();
    assert_eq!(r.decks, pngs);
    assert_eq!(r.wheels, 1);
    assert_eq!(r.shaders, "built", "the drop folder has cycle, glow and nyan decks");
    let ev = events.lock().unwrap();
    assert!(ev.iter().filter(|e| e["event"] == "item").count() >= pngs);
    assert!(ev.iter().any(|e| e["event"] == "built"));
    assert!(root.join("mods").join("Caveman-Decks").join("Win32").join("items.toc").is_file());
    drop(ev);
    // Nothing changed: the fingerprint skip.
    let r2 = bake::build(&config, false, |_| {}).unwrap();
    assert_eq!(r2.outcome, "up_to_date");
    println!("built {} decks + {} wheel in {:.1} s; up to date in {:.2} s", r.decks, r.wheels, r.seconds, r2.seconds);
}
