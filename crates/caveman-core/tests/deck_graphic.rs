//! The deck graphic layer's stock textures (the Natural Blank graphic and the default osk mask) and the grip's osk,
//! written as PNGs with the mask's channels split, to look at (ignored: reads the install).
//! `cargo test -p caveman-core --test deck_graphic -- --ignored --nocapture`

use caveman_core::boardmodel;
use caveman_core::game::{Game, ITEMS_TOC};
use caveman_core::image::Image;
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";
const TEXTURE_TYPE: u32 = 0x6bde_20ba;

fn channel(img: &Image, c: usize) -> Image {
    let mut out = Image::new(img.width, img.height, 1);
    for (i, px) in img.data.chunks_exact(img.channels).enumerate() {
        out.data[i] = px[c];
    }
    out
}

#[test]
#[ignore]
fn dump_graphic_layer() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    std::fs::create_dir_all(&dir).unwrap();
    let toc = game.toc(ITEMS_TOC).unwrap();
    let wood = toc
        .toc
        .bundles
        .iter()
        .map(|b| b.name.as_str())
        .find(|n| n.ends_with("/deck_gen_graphic_00001_ap_cas_main_bundlereftable"))
        .expect("the Natural Blank bundle");
    let b = game.bundle(wood, ITEMS_TOC).unwrap();
    for r in b.manifest.res.iter().filter(|r| r.rtype == TEXTURE_TYPE) {
        println!("texture in the Natural Blank bundle: {}", r.name);
    }
    for (name, stem) in [
        ("characters/skateboard/unlicensed/deck/common/generic/graphic/deck_gen_graphic_00001/2022/deck_gen_graphic_00001_c", "graphic_blank_c"),
        ("characters/skateboard/defaults/deck/common/generic/graphic/deck_gen_graphic_default_osk", "graphic_default_osk"),
    ] {
        let img = boardmodel::texture_image(&game, ITEMS_TOC, &b, name).unwrap();
        let out = dir.join(format!("{stem}.png"));
        img.write_png(&out).unwrap();
        println!("wrote {} ({} x {} x {})", out.display(), img.width, img.height, img.channels);
        if stem.ends_with("osk") {
            for (c, cn) in ["r", "g", "b", "a"].iter().enumerate() {
                let ch = channel(&img, c);
                let (mut lo, mut hi, mut sum) = (255u8, 0u8, 0u64);
                for &v in &ch.data {
                    lo = lo.min(v);
                    hi = hi.max(v);
                    sum += v as u64;
                }
                println!("  channel {cn}: min {lo} max {hi} mean {:.1}", sum as f64 / ch.data.len() as f64);
                ch.write_png(dir.join(format!("{stem}_{cn}.png"))).unwrap();
            }
        }
    }
    // the grip preset's osk, raw, channels split
    let model = boardmodel::read_board_model(&game).unwrap();
    let grip = model.materials.iter().find(|m| m.role == "grip").unwrap();
    let deck_bundle_name = game.find_bundle("/deck_gen_popsicle_cas_main_bundlereftable", ITEMS_TOC).unwrap().unwrap();
    let deck_bundle = game.bundle(&deck_bundle_name, ITEMS_TOC).unwrap();
    for src in &grip.sources {
        if src.ends_with("_osk") {
            let img = boardmodel::texture_image(&game, ITEMS_TOC, &deck_bundle, src).unwrap();
            println!("grip osk {src}: {} x {} x {}", img.width, img.height, img.channels);
            for (c, cn) in ["r", "g", "b", "a"].iter().enumerate() {
                let ch = channel(&img, c);
                let (mut lo, mut hi, mut sum) = (255u8, 0u8, 0u64);
                for &v in &ch.data {
                    lo = lo.min(v);
                    hi = hi.max(v);
                    sum += v as u64;
                }
                println!("  channel {cn}: min {lo} max {hi} mean {:.1}", sum as f64 / ch.data.len() as f64);
                ch.write_png(dir.join(format!("grip_osk_{cn}.png"))).unwrap();
            }
        }
    }
    // the deck base mask too
    let deck = model.materials.iter().find(|m| m.role == "deck").unwrap();
    for src in &deck.sources {
        if src.ends_with("_msk") {
            let img = boardmodel::texture_image(&game, ITEMS_TOC, &deck_bundle, src).unwrap();
            println!("deck base msk {src}: {} x {} x {}", img.width, img.height, img.channels);
            for (c, cn) in ["r", "g", "b", "a"].iter().enumerate() {
                let ch = channel(&img, c);
                let (mut lo, mut hi, mut sum) = (255u8, 0u8, 0u64);
                for &v in &ch.data {
                    lo = lo.min(v);
                    hi = hi.max(v);
                    sum += v as u64;
                }
                println!("  channel {cn}: min {lo} max {hi} mean {:.1}", sum as f64 / ch.data.len() as f64);
                ch.write_png(dir.join(format!("deck_msk_{cn}.png"))).unwrap();
            }
        }
    }
}
