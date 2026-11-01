//! Lists items.toc bundles whose names contain the needles in `NEEDLES` (comma separated), and the EBX assets of the
//! bundle named by `BUNDLE_SUFFIX` (ignored: reads the install).
//! `NEEDLES=truck_gen_default,items cargo test -p caveman-core --test list_bundles -- --ignored --nocapture`

use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn list_bundles() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let toc = game.toc(ITEMS_TOC).unwrap();
    let needles = std::env::var("NEEDLES").unwrap_or_default();
    for needle in needles.split(',').filter(|n| !n.is_empty()) {
        let mut names: Vec<&str> = toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.to_lowercase().contains(&needle.to_lowercase())).collect();
        names.sort();
        println!("\n== {} bundle(s) with {needle:?}:", names.len());
        for n in names.iter().take(40) {
            println!("  {n}");
        }
    }
    if let Ok(suffix) = std::env::var("BUNDLE_SUFFIX") {
        let suffix = format!("/{}", suffix.trim_start_matches('/'));
        match game.find_bundle(&suffix, ITEMS_TOC).unwrap() {
            Some(name) => {
                let b = game.bundle(&name, ITEMS_TOC).unwrap();
                println!("\n== EBX of {name}:");
                for e in &b.manifest.ebx {
                    println!("  {}", e.name);
                }
                println!("== resources:");
                for r in &b.manifest.res {
                    println!("  {:#x} {}", r.rtype, r.name);
                }
            }
            None => println!("no bundle ending {suffix}"),
        }
    }
}
