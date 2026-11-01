//! The wheel's textures and UV regions (side faces against the tread, by the axle axis), a graphic wheel's face and
//! mask, and the grip template's textures, as PNGs and numbers (ignored: reads the install).
//! `cargo test -p caveman-core --test wheel_textures -- --ignored --nocapture`

use caveman_core::boardmodel::{self, texture_image};
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

fn range(uv: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for u in uv {
        for k in 0..2 {
            lo[k] = lo[k].min(u[k]);
            hi[k] = hi[k].max(u[k]);
        }
    }
    (lo, hi)
}

#[test]
#[ignore]
fn wheel_textures_and_regions() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    std::fs::create_dir_all(&dir).unwrap();
    let model = boardmodel::read_board_model(&game).unwrap();
    // the wheel mesh's regions by the axle (x in the wheel bone's space)
    let wheel = model.parts.iter().find(|p| p.name.contains("Right_Wheelback")).unwrap();
    for (label, f) in [("+x face", 0usize), ("-x face", 1), ("tread", 2)] {
        let idx: Vec<usize> = (0..wheel.positions.len())
            .filter(|&i| {
                let nx = wheel.normals[i][0];
                match f {
                    0 => nx > 0.7,
                    1 => nx < -0.7,
                    _ => nx.abs() < 0.3,
                }
            })
            .collect();
        let uv: Vec<[f32; 2]> = idx.iter().map(|&i| wheel.uv0[i]).collect();
        let mut bins = [0usize; 8];
        for u in &uv {
            bins[((u[1].clamp(0.0, 0.9999)) * 8.0) as usize] += 1;
        }
        println!(
            "{label}: {} vertices, uv0 range {:?}, v eighths {bins:?}, x of positions {:.4}..{:.4}",
            idx.len(),
            range(&uv),
            idx.iter().map(|&i| wheel.positions[i][0]).fold(f32::INFINITY, f32::min),
            idx.iter().map(|&i| wheel.positions[i][0]).fold(f32::NEG_INFINITY, f32::max)
        );
    }
    // the wheel textures and their channels
    let toc = game.toc(ITEMS_TOC).unwrap();
    let mut names: Vec<&str> =
        toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.contains("wheelcolor/presets/") && n.ends_with("_ap_cas_main_bundlereftable")).collect();
    names.sort();
    let b = game.bundle(names[0], ITEMS_TOC).unwrap();
    for r in b.manifest.res.iter().filter(|r| r.rtype == TEXTURE_TYPE) {
        let img = texture_image(&game, ITEMS_TOC, &b, &r.name).unwrap();
        let leaf = r.name.rsplit('/').next().unwrap();
        img.write_png(dir.join(format!("{leaf}.png"))).unwrap();
        if leaf.ends_with("_msk") {
            for (c, cn) in ["r", "g", "b"].iter().enumerate() {
                channel(&img, c).write_png(dir.join(format!("{leaf}_{cn}.png"))).unwrap();
            }
        }
        println!("wrote {leaf} ({} x {})", img.width, img.height);
    }
    // a graphic wheel: the donut
    if let Some(n) =
        toc.toc.bundles.iter().map(|b| b.name.as_str()).find(|n| n.contains("wheel_gen_graphic_00006") && n.ends_with("_ap_cas_main_bundlereftable"))
    {
        let gb = game.bundle(n, ITEMS_TOC).unwrap();
        for r in gb.manifest.res.iter().filter(|r| r.rtype == TEXTURE_TYPE && r.name.contains("graphic_00006")) {
            let img = texture_image(&game, ITEMS_TOC, &gb, &r.name).unwrap();
            let leaf = r.name.rsplit('/').next().unwrap();
            img.write_png(dir.join(format!("{leaf}.png"))).unwrap();
            if leaf.ends_with("_osk") {
                channel(&img, 0).write_png(dir.join(format!("{leaf}_r.png"))).unwrap();
            }
            println!("wrote {leaf} ({} x {})", img.width, img.height);
        }
    }
    // the grip template's textures
    let gbundle = game.find_bundle("/deck_gen_popsiclegripcolor_00001_ap_cas_main_bundlereftable", ITEMS_TOC).unwrap().unwrap();
    let gb = game.bundle(&gbundle, ITEMS_TOC).unwrap();
    for r in gb.manifest.res.iter().filter(|r| r.rtype == TEXTURE_TYPE && r.name.contains("gripcolor")) {
        let img = texture_image(&game, ITEMS_TOC, &gb, &r.name).unwrap();
        let leaf = r.name.rsplit('/').next().unwrap();
        img.write_png(dir.join(format!("{leaf}.png"))).unwrap();
        println!("wrote {leaf} ({} x {})", img.width, img.height);
    }
    // the wood mask's R at the top face (the bolt heads): sample the grip's uv0 positions
    let wood_msk = texture_image(
        &game,
        ITEMS_TOC,
        &game.bundle(&game.find_bundle("/deck_gen_popsicle_cas_main_bundlereftable", ITEMS_TOC).unwrap().unwrap(), ITEMS_TOC).unwrap(),
        "characters/skateboard/unlicensed/deck/generic/popsicle/2022/deck_gen_popsicle_msk",
    )
    .unwrap();
    let (mut r_at_b, mut n) = (0u64, 0u64);
    let (mut bmax, mut rmax) = (0u8, 0u8);
    for px in wood_msk.data.as_chunks::<4>().0.iter().enumerate().filter(|(i, _)| (i % wood_msk.width) < wood_msk.width / 2).map(|(_, p)| p) {
        if px[2] > 128 {
            r_at_b += px[0] as u64;
            n += 1;
            rmax = rmax.max(px[0]);
        }
        bmax = bmax.max(px[2]);
    }
    println!(
        "wood msk, left half (the top face): texels with B > 128: {n}, their mean R {:.1}, max R {rmax}",
        if n > 0 { r_at_b as f64 / n as f64 } else { 0.0 }
    );
}
