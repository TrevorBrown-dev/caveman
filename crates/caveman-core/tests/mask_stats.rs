//! Channel statistics of the board parts' masks (`_msk` of the wood, trucks and wheels; `_osk` of the grip and the
//! default graphic), sampled where the meshes actually look, to settle what each channel means (ignored: reads the
//! install). `cargo test -p caveman-core --test mask_stats -- --ignored --nocapture`

use caveman_core::boardmodel::{self, texture_image};
use caveman_core::game::{Game, ITEMS_TOC};
use caveman_core::mesh::{LEVEL_BUNDLE, LEVEL_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn mask_channels() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let model = boardmodel::read_board_model(&game).unwrap();
    let deck_bundle_name = game.find_bundle("/deck_gen_popsicle_cas_main_bundlereftable", ITEMS_TOC).unwrap().unwrap();
    let deck_bundle = game.bundle(&deck_bundle_name, ITEMS_TOC).unwrap();
    let truck_bundle_name = {
        let toc = game.toc(ITEMS_TOC).unwrap();
        let mut names: Vec<&str> = toc
            .toc
            .bundles
            .iter()
            .map(|b| b.name.as_str())
            .filter(|n| n.starts_with("win32/characters/skateboard/unlicensed/truck/generic/default/") && n.ends_with("_ap_cas_main_bundlereftable"))
            .collect();
        names.sort();
        names[0].to_string()
    };
    let truck_bundle = game.bundle(&truck_bundle_name, ITEMS_TOC).unwrap();
    let wheel_bundle_name = {
        let toc = game.toc(ITEMS_TOC).unwrap();
        let mut names: Vec<&str> = toc
            .toc
            .bundles
            .iter()
            .map(|b| b.name.as_str())
            .filter(|n| {
                n.starts_with("win32/characters/skateboard/unlicensed/wheel/generic/classic/wheelcolor/presets/") && n.ends_with("_ap_cas_main_bundlereftable")
            })
            .collect();
        names.sort();
        names[0].to_string()
    };
    let wheel_bundle = game.bundle(&wheel_bundle_name, ITEMS_TOC).unwrap();
    let _ = (LEVEL_BUNDLE, LEVEL_TOC);
    // (mask name, the bundle, the part whose uv (set) samples it)
    let cases: Vec<(&str, &caveman_core::game::Bundle, &str, usize)> = vec![
        ("characters/skateboard/unlicensed/deck/generic/popsicle/2022/deck_gen_popsicle_msk", &deck_bundle, "Deck_Mat", 0),
        (
            "characters/skateboard/unlicensed/deck/common/generic/gripcolor/presets/deck_gen_popsiclegripcolor_00023/2025/deck_gen_popsiclegripcolor_00023_osk",
            &deck_bundle,
            "DeckTop_mat",
            1,
        ),
        ("characters/skateboard/unlicensed/truck/generic/default/2022/truck_gen_default_00001_msk", &truck_bundle, "mat_skateboard_trucks:Deck", 0),
        ("characters/skateboard/unlicensed/truck/generic/default/2022/truck_gen_default_00001_msk", &truck_bundle, "mat_skateboard_trucks:Truck_Back", 0),
        ("characters/skateboard/unlicensed/wheel/generic/classic/2022/wheel_gen_classic_msk", &wheel_bundle, "mat_skateboard_wheels:Right_Wheelback", 0),
    ];
    for (name, bundle, part_name, set) in cases {
        let img = texture_image(&game, ITEMS_TOC, bundle, name).unwrap();
        let part = model.parts.iter().find(|p| p.name == part_name).unwrap();
        // sample the mask at every vertex's uv (fractional part), per channel: min, max, mean, and a histogram of 4 bins
        let mut stats = [(255u8, 0u8, 0f64, [0usize; 4]); 4];
        let mut n = 0usize;
        for i in 0..part.positions.len() {
            let uv = if set == 0 { part.uv0[i] } else { part.uv1[i] };
            let (u, v) = (uv[0].rem_euclid(1.0), uv[1].rem_euclid(1.0));
            let (x, y) = (((u * img.width as f32) as usize).min(img.width - 1), ((v * img.height as f32) as usize).min(img.height - 1));
            let px = &img.data[(y * img.width + x) * img.channels..][..img.channels];
            for c in 0..4 {
                let val = px[c];
                let s = &mut stats[c];
                s.0 = s.0.min(val);
                s.1 = s.1.max(val);
                s.2 += val as f64;
                s.3[(val as usize * 4 / 256).min(3)] += 1;
            }
            n += 1;
        }
        println!("{part_name} (uv{set}) samples {} of {}:", n, name.rsplit('/').next().unwrap());
        for (c, cn) in ["R", "G", "B", "A"].iter().enumerate() {
            let s = &stats[c];
            println!("  {cn}: min {:>3} max {:>3} mean {:>6.1}  quarters {:?}", s.0, s.1, s.2 / n as f64, s.3);
        }
    }
    // the graphic's default osk at the bottom face's uv1 (tile 2)
    let osk = texture_image(&game, ITEMS_TOC, &deck_bundle, "characters/skateboard/defaults/deck/common/generic/graphic/deck_gen_graphic_default_osk");
    if let Ok(img) = osk {
        let part = model.parts.iter().find(|p| p.name == "Deck_Mat").unwrap();
        let mut stats = [(255u8, 0u8, 0f64); 3];
        let mut n = 0;
        for i in 0..part.positions.len() {
            let uv = part.uv1[i];
            if uv[0] < 2.0 {
                continue;
            }
            let (u, v) = (uv[0] - 2.0, uv[1]);
            let (x, y) = (((u * img.width as f32) as usize).min(img.width - 1), ((v * img.height as f32) as usize).min(img.height - 1));
            let px = &img.data[(y * img.width + x) * img.channels..][..img.channels];
            for c in 0..3 {
                stats[c].0 = stats[c].0.min(px[c]);
                stats[c].1 = stats[c].1.max(px[c]);
                stats[c].2 += px[c] as f64;
            }
            n += 1;
        }
        println!("Deck_Mat bottom (uv1 tile 2) samples {n} of deck_gen_graphic_default_osk:");
        for (c, cn) in ["R", "G", "B"].iter().enumerate() {
            println!("  {cn}: min {:>3} max {:>3} mean {:>6.1}", stats[c].0, stats[c].1, stats[c].2 / n as f64);
        }
    }
}
