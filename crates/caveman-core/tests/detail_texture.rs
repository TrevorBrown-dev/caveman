//! The tiled detail texture the truck, wheel and (maybe) deck bundles carry (`texeldetail_special_nk_ta`), and the
//! grip and deck bundles' full resource lists, to see what detail maps the shaders tile (ignored: reads the install).
//! `cargo test -p caveman-core --test detail_texture -- --ignored --nocapture`

use caveman_core::boardmodel::texture_image;
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";
const TEXTURE_TYPE: u32 = 0x6bde_20ba;

#[test]
#[ignore]
fn detail_textures() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    for suffix in [
        "/truck_gen_default_cas_main_bundlereftable",
        "/deck_gen_popsicle_cas_main_bundlereftable",
        "/deck_gen_popsiclegripcolor_00001_ap_cas_main_bundlereftable",
        "/deck_gen_graphic_00001_ap_cas_main_bundlereftable",
    ] {
        let name = game.find_bundle(suffix, ITEMS_TOC).unwrap().unwrap();
        let b = game.bundle(&name, ITEMS_TOC).unwrap();
        println!("== {name}");
        for r in &b.manifest.res {
            if r.rtype == TEXTURE_TYPE {
                match texture_image(&game, ITEMS_TOC, &b, &r.name) {
                    Ok(img) => {
                        let leaf = r.name.rsplit('/').next().unwrap();
                        println!("   texture {} ({} x {})", r.name, img.width, img.height);
                        if leaf.contains("texeldetail") || leaf.contains("detail") {
                            img.write_png(dir.join(format!("{leaf}.png"))).unwrap();
                            let mut sum = [0u64; 4];
                            for px in img.data.as_chunks::<4>().0 {
                                for c in 0..4 {
                                    sum[c] += px[c] as u64;
                                }
                            }
                            let n = (img.width * img.height) as u64;
                            println!("      mean rgba ({}, {}, {}, {}); wrote {leaf}.png", sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n);
                        }
                    }
                    Err(e) => println!("   texture {}: {e}", r.name),
                }
            } else {
                println!("   {:#x} {}", r.rtype, r.name);
            }
        }
    }
}
