//! The shared bundle's item records and thumbnails for the equipped parts: what they are named, what an item record
//! says (its preset), and the thumbnails' colours against the presets' (ignored: reads the install).
//! `cargo test -p caveman-core --test shared_lookup -- --ignored --nocapture`

use caveman_core::boardmodel::texture_image;
use caveman_core::build::SHARED;
use caveman_core::ebx;
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn shared_items_and_thumbs() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    for word in ["wheelgraphic", "wheelcolor_gen_classic_0001"] {
        let ebx: Vec<&str> = shared
            .manifest
            .ebx
            .iter()
            .filter(|e| e.name.to_lowercase().contains(word) && e.name.starts_with("items/"))
            .map(|e| e.name.as_str())
            .take(5)
            .collect();
        let n = shared.manifest.ebx.iter().filter(|e| e.name.to_lowercase().contains(word) && e.name.starts_with("items/")).count();
        println!("== {n} item record(s) with {word:?}, e.g. {ebx:?}");
    }
    // the equipped items' records
    for leaf in [
        "own_wheelcolor_gen_classic_00005",
        "own_deckgripcolor_gen_popsicle_00001",
        "own_deckgripcutout_gen_popsicle_00001",
        "own_truck_gen_default_00006",
        "own_deckgraphic_gen_popsicle_00001",
    ] {
        let found =
            shared.manifest.ebx.iter().find(|e| e.name.starts_with("items/") && e.name.to_lowercase().ends_with(&format!("/{leaf}"))).map(|e| e.name.clone());
        match found {
            Some(name) => {
                let i = shared.manifest.ebx_index(&name).unwrap();
                let doc = ebx::read_document(&game.decode(&shared.ebx_loc(i).unwrap()).unwrap()).unwrap();
                println!("== item {name}:");
                for field in ["Name", "Key", "Categories", "AssetPaths", "BaseColor", "ItemData", "AppearanceTemplateParameters"] {
                    match doc.get(field, 0) {
                        Ok(v) => {
                            let text = format!("{:?}", v);
                            println!("   {field}: {}", &text[..text.len().min(700)]);
                        }
                        Err(e) => println!("   {field}: ({e})"),
                    }
                }
            }
            None => println!("== item {leaf}: not in the shared bundle"),
        }
    }
    // thumbnails against the presets' colours
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    std::fs::create_dir_all(&dir).unwrap();
    for (thumb, colour_linear) in [
        ("thumbnail/tool/own_wheelcolor_gen_classic_00001_lrg", Some([0.7388f64, 0.5526, 0.3814])),
        ("thumbnail/tool/own_wheelcolor_gen_classic_00005_lrg", Some([0.9734, 0.0648, 0.1683])),
        ("thumbnail/tool/own_deckgripcolor_gen_popsicle_00001_lrg", Some([0.0481, 0.0440, 0.0537])),
        ("thumbnail/tool/own_deckgripcolor_gen_popsicle_00023_lrg", None),
    ] {
        let img = match texture_image(&game, ITEMS_TOC, &shared, thumb) {
            Ok(i) => i,
            Err(e) => {
                println!("{thumb}: {e}");
                continue;
            }
        };
        let out = dir.join(format!("{}.png", thumb.rsplit('/').next().unwrap()));
        img.write_png(&out).unwrap();
        let mut chans: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for px in img.data.as_chunks::<4>().0 {
            if px[3] > 250 {
                for c in 0..3 {
                    chans[c].push(px[c]);
                }
            }
        }
        let n = chans[0].len();
        if n == 0 {
            println!("{thumb}: no opaque pixels");
            continue;
        }
        let mut line = format!("{thumb} ({n} opaque px):",);
        for c in 0..3 {
            chans[c].sort_unstable();
            let mean = chans[c].iter().map(|&v| v as f64).sum::<f64>() / n as f64;
            line.push_str(&format!(
                " [{}: mean {mean:.0} median {} p10 {} p90 {}]",
                ["R", "G", "B"][c],
                chans[c][n / 2],
                chans[c][n / 10],
                chans[c][n * 9 / 10]
            ));
        }
        println!("{line}");
        if let Some(c) = colour_linear {
            let srgb: Vec<String> =
                c.iter().map(|v| format!("{:.0}", 255.0 * if *v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 })).collect();
            println!("    preset colour as sRGB ({})", srgb.join(", "));
        }
    }
}
