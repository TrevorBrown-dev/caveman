//! The grip colour, wheel colour and truck presets in items.toc: names, and for the first grips the mean colour of
//! their texture, to pick the plain defaults (ignored: reads the install).
//! `cargo test -p caveman-core --test part_presets -- --ignored --nocapture`

use caveman_core::boardmodel::texture_image;
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";
const TEXTURE_TYPE: u32 = 0x6bde_20ba;

#[test]
#[ignore]
fn list_presets() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let toc = game.toc(ITEMS_TOC).unwrap();
    for (label, needle) in [("grip colour presets", "gripcolor/presets/"), ("wheel colour presets", "wheelcolor/presets/"), ("trucks", "/truck/")] {
        let mut names: Vec<&str> =
            toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.contains(needle) && n.ends_with("_ap_cas_main_bundlereftable")).collect();
        names.sort();
        println!("\n== {} {label}; the first 12:", names.len());
        for n in names.iter().take(12) {
            println!("  {n}");
        }
        if label.starts_with("grip") {
            for n in names.iter().take(4) {
                let b = game.bundle(n, ITEMS_TOC).unwrap();
                for r in b.manifest.res.iter().filter(|r| r.rtype == TEXTURE_TYPE && r.name.ends_with("_c") && r.name.contains("gripcolor")) {
                    let img = texture_image(&game, ITEMS_TOC, &b, &r.name).unwrap();
                    let mut sum = [0u64; 3];
                    for px in img.data.as_chunks::<4>().0 {
                        for c in 0..3 {
                            sum[c] += px[c] as u64;
                        }
                    }
                    let n = (img.width * img.height) as u64;
                    println!("    {} mean rgb ({}, {}, {})", r.name.rsplit('/').next().unwrap(), sum[0] / n, sum[1] / n, sum[2] / n);
                }
            }
        }
    }
}
