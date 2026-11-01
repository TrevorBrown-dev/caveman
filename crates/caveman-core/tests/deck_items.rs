//! The deck item records' names in the shared bundle and the truck item's full asset paths (ignored: reads the
//! install). `cargo test -p caveman-core --test deck_items -- --ignored --nocapture`

use caveman_core::build::SHARED;
use caveman_core::ebx;
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn deck_items() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    let boards: Vec<&str> = shared.manifest.ebx.iter().filter(|e| e.name.starts_with("items/cust_board/")).map(|e| e.name.as_str()).collect();
    println!("== {} items/cust_board records, e.g.:", boards.len());
    for n in boards.iter().take(8) {
        println!("  {n}");
    }
    for word in ["graphic_00001", "popsicle_00001", "gen_graphic", "caveman", "natural"] {
        let hits: Vec<&&str> = boards.iter().filter(|n| n.to_lowercase().contains(word)).take(4).collect();
        println!("  with {word:?}: {hits:?}");
    }
    // the first deck record's inner data
    if let Some(name) = boards.iter().find(|n| n.to_lowercase().contains("gen_graphic_00001") || n.to_lowercase().contains("popsicle_00001")).or(boards.first())
    {
        let i = shared.manifest.ebx_index(name).unwrap();
        let doc = ebx::read_document(&game.decode(&shared.ebx_loc(i).unwrap()).unwrap()).unwrap();
        println!("== {name}:");
        if let Ok(v) = doc.get("Key", 0) {
            println!("   Key: {:?}", v.as_str());
        }
        if let Ok(v) = doc.get("AssetPaths", 1) {
            println!("   AssetPaths: {:?}", v);
        }
    }
    // the truck item's asset paths, whole
    if let Some(e) = shared.manifest.ebx.iter().find(|e| e.name == "items/truck/own_truck_gen_default_00006") {
        let i = shared.manifest.ebx_index(&e.name).unwrap();
        let doc = ebx::read_document(&game.decode(&shared.ebx_loc(i).unwrap()).unwrap()).unwrap();
        println!("== {}: AssetPaths {:?}", e.name, doc.get("AssetPaths", 1).ok());
    }
}
