//! Where the skateboard parts' textures live: the bundles of items.toc named for trucks, wheels and grip, and the
//! texture assets in the first of each (ignored: reads the install).
//! `cargo test -p caveman-core --test board_textures -- --ignored --nocapture`

use caveman_core::game::{Game, ITEMS_TOC};
use caveman_core::mesh::{LEVEL_BUNDLE, LEVEL_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn list_part_bundles() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let toc = game.toc(ITEMS_TOC).unwrap();
    for word in ["truck", "wheel", "grip"] {
        let names: Vec<&str> = toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.to_lowercase().contains(word)).collect();
        println!("\n== {} bundle(s) with {word:?} in items.toc; the first 12:", names.len());
        for n in names.iter().take(12) {
            println!("  {n}");
        }
        if let Some(first) = names.first() {
            let b = game.bundle(first, ITEMS_TOC).unwrap();
            println!("  textures and meshes of {first}:");
            for r in &b.manifest.res {
                let leaf = r.name.rsplit('/').next().unwrap_or(&r.name);
                if leaf.ends_with("_c")
                    || leaf.ends_with("_ny")
                    || leaf.ends_with("_osk")
                    || leaf.ends_with("_msk")
                    || leaf.ends_with("_n")
                    || leaf.ends_with("_mesh")
                    || leaf.ends_with("_rgb")
                {
                    println!("    res {:#x} {}", r.rtype, r.name);
                }
            }
            for e in &b.manifest.ebx {
                if e.name.contains("items/") || e.name.contains("presets") {
                    println!("    ebx {}", e.name);
                }
            }
        }
    }
    // the level root: the prototype skateboard's own textures, if any
    let b = game.bundle(LEVEL_BUNDLE, LEVEL_TOC).unwrap();
    println!("\n== level root resources named for the skateboard prototype or generic wheel:");
    for r in &b.manifest.res {
        let lower = r.name.to_lowercase();
        if lower.contains("_prototype/skateboard") || lower.contains("wheel_gen_classic") || lower.contains("skateboard/defaults") {
            println!("  res {:#x} {}", r.rtype, r.name);
        }
    }
}
