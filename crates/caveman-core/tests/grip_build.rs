//! Builds a test mod with a plain grip colour, a printed grip and a grip pattern into `target/mods/Caveman-Grip-Test`
//! (ignored: reads the install; the mod is for installing by hand into the game's Mods and looking at in the menu).
//! Also writes the thumbnails and the textures' previews into `target/grip/`.
//! `cargo test -p caveman-core --release --test grip_build -- --ignored --nocapture`

use caveman_core::build::{self, BuildOptions, CutoutSpec, GripSpec, GRIP_SIZE};
use caveman_core::game::Game;
use caveman_core::image::Image;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const GAME: &str = r"E:\Games\ReSkate";
const PRINT: &str = r"E:\Games\ReSkate\Caveman\Decks\000\Biggie Cheese.png";

fn target(sub: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").join(sub);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Polka dots: a 512 x 2048 mask, 255 everywhere but round holes on a grid (soft-edged).
fn dots() -> Image {
    let (w, h) = GRIP_SIZE;
    let mut m = Image::filled(w, h, &[255]);
    let (step, r) = (96.0f64, 30.0f64);
    for y in 0..h {
        for x in 0..w {
            let row = (y as f64 / step).floor();
            let offset = if row as i64 % 2 == 0 { 0.0 } else { step / 2.0 };
            let cx = ((x as f64 - offset) / step).round() * step + offset;
            let cy = row * step + step / 2.0;
            let d = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
            let v = ((d - r + 1.0) * 127.0).clamp(0.0, 255.0);
            m.pixel_mut(x, y)[0] = v as u8;
        }
    }
    m
}

#[test]
#[ignore]
fn grip_test_mod() {
    let game = Arc::new(Game::open(Path::new(GAME)).unwrap());
    let out = target("mods");
    let print = Image::read_png(PRINT).ok();
    let mut grips = vec![GripSpec {
        display: "Caveman Hot Pink".into(),
        key: "Own_Grip_Caveman_Hot_Pink".into(),
        colour: Some([255, 105, 180]),
        print: None,
        input: None,
        notes: Vec::new(),
    }];
    if let Some(p) = print {
        grips.push(GripSpec {
            display: "Caveman Print".into(),
            key: "Own_Grip_Caveman_Print".into(),
            colour: None,
            print: Some(p.clone()),
            input: None,
            notes: Vec::new(),
        });
        grips.push(GripSpec {
            display: "Caveman Print Blue".into(),
            key: "Own_Grip_Caveman_Print_Blue".into(),
            colour: Some([40, 90, 220]),
            print: Some(p),
            input: None,
            notes: Vec::new(),
        });
    }
    let cutouts = vec![CutoutSpec { display: "Caveman Dots".into(), key: "Own_GripPattern_Caveman_Dots".into(), mask: dots(), input: None, notes: Vec::new() }];
    let mut log = |l: &str| println!("{l}");
    let report = build::build(game, &[], &[], &grips, &cutouts, &out, "Caveman-Grip-Test", &BuildOptions::default(), &mut log).unwrap();
    println!(
        "built {} grip(s) and {} pattern(s) into {} ({} KB, {} bundles, {} chunks)",
        report.grips.len(),
        report.cutouts.len(),
        report.mod_dir.display(),
        report.cas_bytes / 1024,
        report.bundles,
        report.chunks
    );
    let look = target("grip");
    for b in report.grips.iter().chain(&report.cutouts) {
        if let Some(t) = &b.thumbnail {
            t.write_png(look.join(format!("thumb_{}.png", b.display.replace(' ', "_")))).unwrap();
        }
    }
    dots().write_png(look.join("mask_dots.png")).unwrap();
    assert_eq!(report.cutouts.len(), 1);
    assert!(!report.grips.is_empty());
    for name in ["manifest.json", "README.md"] {
        assert!(report.mod_dir.join(name).is_file(), "{name}");
    }
}

#[test]
fn fit_mask_shapes() {
    // a 1:4 picture is fitted whole; another is repeated as a tile
    let tall = Image::filled(128, 512, &[200, 200, 200]);
    let (m, notes) = build::fit_mask(&tall);
    assert_eq!((m.width, m.height, m.channels), (512, 2048, 1));
    assert!(notes.iter().any(|n| n.contains("lightness")), "{notes:?}");
    assert_eq!(m.pixel(10, 10)[0], 200);
    let mut tile = Image::filled(64, 64, &[255, 255, 255, 255]);
    tile.pixel_mut(3, 5)[3] = 0;
    let (m, notes) = build::fit_mask(&tile);
    assert!(notes.iter().any(|n| n.contains("tile")), "{notes:?}");
    assert_eq!(m.pixel(3, 5)[0], 0, "the clear pixel is a hole");
    assert_eq!(m.pixel(3 + 64, 5 + 64 * 3)[0], 0, "repeated");
    assert_eq!(m.pixel(4, 5)[0], 255);
}
