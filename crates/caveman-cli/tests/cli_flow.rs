//! The whole CLI run on scratch folders: settings files overriding suffixes, wheels, `--if-changed`, the shader mod
//! built and removed (only Caveman's own). Leaves a copy of the first build in scratchpad/cli/flow-built for Python's
//! verifiers. Needs the game (read only) and Caveman Decks\000\Dan Da Dan.png; run with `--ignored`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use caveman_cli::report::Reporter;
use caveman_cli::{run, Options, Outcome, Shaders, SHADER_MOD};
use caveman_core::image::Image;

fn scratch() -> PathBuf {
    std::env::var_os("CAVEMAN_SCRATCH").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad")
    })
}

#[derive(Clone)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Runs the CLI; returns the outcome and what it said.
fn go(o: &Options) -> (Outcome, String) {
    let buf = Buf(Arc::new(Mutex::new(Vec::new())));
    let mut report = Reporter::to(Box::new(buf.clone()), false);
    let outcome = run(o, &mut report).unwrap();
    let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    print!("{text}");
    (outcome, text)
}

fn touch(path: &Path) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(5)).unwrap();
}

fn donut() -> Image {
    let mut img = Image::new(512, 512, 4);
    for y in 0..512 {
        for x in 0..512 {
            let (dx, dy) = (x as f64 - 255.5, y as f64 - 255.5);
            let r = (dx * dx + dy * dy).sqrt() / 256.0;
            if (0.35..0.9).contains(&r) {
                let a = dy.atan2(dx);
                img.pixel_mut(x, y).copy_from_slice(&[(128.0 + 127.0 * a.cos()) as u8, (128.0 + 127.0 * a.sin()) as u8, 200, 255]);
            }
        }
    }
    img
}

#[test]
#[ignore]
fn flow() {
    let root = scratch().join("cli").join("flow");
    let _ = fs::remove_dir_all(&root);
    let (decks, wheels, mods) = (root.join("decks"), root.join("wheels"), root.join("mods"));
    fs::create_dir_all(&decks).unwrap();
    fs::create_dir_all(&wheels).unwrap();
    fs::create_dir_all(&mods).unwrap();
    let art = Path::new(r"E:\Games\ReSkate\Caveman\Decks\000\Dan Da Dan.png");
    let deck = |name: &str, json: Option<&str>| {
        fs::copy(art, decks.join(format!("{name}.png"))).unwrap();
        if let Some(j) = json {
            let stem = name.to_string();
            fs::write(decks.join(format!("{stem}.caveman.json")), j).unwrap();
        }
    };
    deck("Plain JSON", Some(r#"{"version": 1, "finish": {"kind": "holo", "mode": "frost"}}"#));
    deck("Aura Colour", Some(r##"{"version": 1, "board": {"effect": "aura", "colour": "#ff8800"}}"##));
    deck("Nyan Named.foil", Some(r#"{"version": 1, "name": "Rainbow Ride", "board": {"effect": "nyan"}}"#));
    deck("Fast Cycle", Some(r#"{"version": 1, "shader": {"effect": "cycle", "speed": 1.0, "glow": 0.25}}"#));
    deck("Bad", Some(r#"{"version": 1, "finsh": {"kind": "foil"}}"#));
    deck("Suffix Only.chrome", None);
    donut().write_png(wheels.join("Red Donut.ff0000.png")).unwrap();
    donut().write_png(wheels.join("Plain Wheel.png")).unwrap();

    let mut o = Options::new(r"E:\Games\ReSkate");
    o.decks = Some(decks.clone());
    o.wheels = Some(wheels.clone());
    o.out = Some(mods.clone());
    o.prefix = "000 - ".into();
    o.if_changed = true;
    o.cache = Some(root.join("cache"));

    // 1. the first build: settings files override suffixes; wheels; the shader mod (cycle, nyan)
    let (outcome, text) = go(&o);
    assert_eq!(outcome, Outcome::Built { decks: 5, wheels: 2, grips: 0, cutouts: 0, shaders: Shaders::Built });
    for line in [
        "  000 - Plain JSON Holo Frost",
        "  000 - Aura Colour Aura FF8800",
        "  000 - Rainbow Ride",
        "  000 - Fast Cycle Cycle Glow",
        "  000 - Suffix Only Chrome",
        "  skipped Bad.png: its settings file Bad.caveman.json: not valid settings: unknown field `finsh`",
        "  000 - Red Donut",
        "  000 - Plain Wheel",
        "Built 5 deck(s) and 2 wheel(s)",
    ] {
        assert!(text.contains(line), "missing: {line}");
    }
    assert!(mods.join(SHADER_MOD).join("Win32").join("globals.toc").is_file());
    assert!(wheels.join("previews").join("000 - Red Donut.png").is_file());
    let keep = scratch().join("cli").join("flow-built");
    let _ = fs::remove_dir_all(&keep);
    copy_dir(&mods.join("Caveman-Decks"), &keep.join("Caveman-Decks"));

    // 2. nothing changed: skipped
    assert_eq!(go(&o).0, Outcome::UpToDate);
    // 3. a PNG touched: rebuilt; then a settings file touched: rebuilt
    touch(&decks.join("Plain JSON.png"));
    assert!(matches!(go(&o).0, Outcome::Built { .. }));
    assert_eq!(go(&o).0, Outcome::UpToDate);
    touch(&decks.join("Aura Colour.caveman.json"));
    assert!(matches!(go(&o).0, Outcome::Built { .. }));

    // 4. no deck needs shaders any more: Caveman's shader mod removed
    for f in ["Nyan Named.foil.png", "Nyan Named.foil.caveman.json", "Fast Cycle.png", "Fast Cycle.caveman.json"] {
        fs::remove_file(decks.join(f)).unwrap();
    }
    assert_eq!(go(&o).0, Outcome::Built { decks: 3, wheels: 2, grips: 0, cutouts: 0, shaders: Shaders::Removed });
    assert!(!mods.join(SHADER_MOD).exists());

    // 5. a Caveman-Shaders that Caveman did not build is never removed
    fs::create_dir_all(mods.join(SHADER_MOD)).unwrap();
    fs::write(mods.join(SHADER_MOD).join("reskate-build.json"), r#"{"tool": "ReSkateStudio"}"#).unwrap();
    touch(&decks.join("Plain JSON.png"));
    assert_eq!(go(&o).0, Outcome::Built { decks: 3, wheels: 2, grips: 0, cutouts: 0, shaders: Shaders::None });
    assert!(mods.join(SHADER_MOD).join("reskate-build.json").is_file());

    // 6. nothing left to build: Caveman's mod removed, the other left
    for e in fs::read_dir(&decks).unwrap().chain(fs::read_dir(&wheels).unwrap()) {
        let p = e.unwrap().path();
        if p.is_file() {
            fs::remove_file(p).unwrap();
        }
    }
    let (outcome, text) = go(&o);
    assert_eq!(outcome, Outcome::Nothing);
    assert!(text.contains("No decks to build: removed"));
    assert!(!mods.join("Caveman-Decks").exists());
    assert!(mods.join(SHADER_MOD).exists());
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let p = e.unwrap().path();
        let t = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_dir(&p, &t);
        } else {
            fs::copy(&p, &t).unwrap();
        }
    }
}
