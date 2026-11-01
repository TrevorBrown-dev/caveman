//! An item record's inner data (`DelMarCustomizationItemData`: the preset it names), the stock deck graphics' item
//! records, and a printed grip's preset parameters (ignored: reads the install).
//! `cargo test -p caveman-core --test item_records -- --ignored --nocapture`

use caveman_core::build::{preset_params_of, SHARED};
use caveman_core::ebx;
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn item_records() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    for leaf in [
        "own_wheelcolor_gen_classic_00005",
        "own_deckgripcolor_gen_popsicle_00001",
        "own_deckgripcutout_gen_popsicle_00001",
        "own_truck_gen_default_00006",
        "own_deckgraphic_gen_popsicle_00001",
        "own_deckgraphic_gen_popsicle_00002",
    ] {
        let found = shared
            .manifest
            .ebx
            .iter()
            .find(|e| e.name.to_lowercase().ends_with(&format!("/{leaf}")) && !e.name.starts_with("thumbnail/"))
            .map(|e| e.name.clone());
        let Some(name) = found else {
            let similar: Vec<&str> = shared
                .manifest
                .ebx
                .iter()
                .filter(|e| e.name.to_lowercase().contains(&leaf[4..].to_lowercase()) && !e.name.starts_with("thumbnail/"))
                .map(|e| e.name.as_str())
                .take(5)
                .collect();
            println!("== {leaf}: no item record; similar: {similar:?}");
            continue;
        };
        let i = shared.manifest.ebx_index(&name).unwrap();
        let doc = ebx::read_document(&game.decode(&shared.ebx_loc(i).unwrap()).unwrap()).unwrap();
        println!("== {name}:");
        for obj in 0..6 {
            for field in ["AssetPaths", "AssetName", "AssetTypeId", "BaseColor", "ThumbnailTexture", "AppearanceTemplateParameters", "Key"] {
                if let Ok(v) = doc.get(field, obj) {
                    let text = format!("{:?}", v);
                    println!("   object {obj} {field}: {}", &text[..text.len().min(500)]);
                }
            }
        }
    }
    // how many item records in all, by category folder
    let mut cats: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for e in shared.manifest.ebx.iter().filter(|e| e.name.starts_with("items/")) {
        let cat = e.name.split('/').nth(1).unwrap_or("").to_string();
        *cats.entry(cat).or_default() += 1;
    }
    println!("== item categories: {cats:?}");
    // a printed grip's preset parameters
    for stock in ["characters/skateboard/unlicensed/deck/common/generic/gripcolor/presets/deck_gen_popsiclegripcolor_00023"] {
        println!("== {stock}");
        match preset_params_of(&game, stock) {
            Ok(params) => {
                for (k, raw) in params {
                    let floats: Vec<String> = raw.chunks_exact(4).take(4).map(|b| format!("{:.4}", f32::from_le_bytes(b.try_into().unwrap()))).collect();
                    println!("  {k}: {} bytes as floats {}", raw.len(), floats.join(" "));
                }
            }
            Err(e) => println!("  no preset params: {e}"),
        }
    }
}
