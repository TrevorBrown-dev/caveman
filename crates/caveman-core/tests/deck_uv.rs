//! The deck body's UV layout: where the faces looking down, up and sideways sit in each UV set (histograms), and the
//! sizes of other decks' colour textures (ignored: reads the install).
//! `cargo test -p caveman-core --test deck_uv -- --ignored --nocapture`

use caveman_core::bcdec::BcFormat;
use caveman_core::boardmodel;
use caveman_core::game::{Game, ITEMS_TOC};
use caveman_core::texture::TextureHeader;
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";
const TEXTURE_TYPE: u32 = 0x6bde_20ba;

#[test]
#[ignore]
fn deck_uv_layout() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let model = boardmodel::read_board_model(&game).unwrap();
    for part in model.parts.iter().filter(|p| p.name.starts_with("Deck")) {
        println!("\n== {} ({} vertices)", part.name, part.positions.len());
        for (label, f) in [("down", 0usize), ("up", 1), ("side", 2), ("other", 3)] {
            let idx: Vec<usize> = (0..part.positions.len())
                .filter(|&i| {
                    let ny = part.normals[i][1];
                    match f {
                        0 => ny < -0.7,
                        1 => ny > 0.7,
                        2 => ny.abs() < 0.3,
                        _ => !(!(-0.7..=0.7).contains(&ny) || ny.abs() < 0.3),
                    }
                })
                .collect();
            if idx.is_empty() {
                continue;
            }
            let mut bins0 = [0usize; 10];
            let mut tiles1 = [0usize; 4];
            let mut bins1v = [0usize; 10];
            let (mut zlo, mut zhi) = (f32::INFINITY, f32::NEG_INFINITY);
            for &i in &idx {
                let u = part.uv0[i][0].clamp(0.0, 0.9999);
                bins0[(u * 10.0) as usize] += 1;
                let t = part.uv1[i][0].floor().clamp(0.0, 3.0) as usize;
                tiles1[t] += 1;
                let v = part.uv1[i][1].clamp(0.0, 0.9999);
                bins1v[(v * 10.0) as usize] += 1;
                zlo = zlo.min(part.positions[i][2]);
                zhi = zhi.max(part.positions[i][2]);
            }
            println!(
                "  {label:<5} {:>4} vertices, z {zlo:+.3}..{zhi:+.3}: uv0 u tenths {bins0:?}; uv1 tile counts {tiles1:?}; uv1 v tenths {bins1v:?}",
                idx.len()
            );
        }
        // the vertices looking down in the middle of the board: their uv0 and uv1
        println!("  a few down-looking vertices near the middle (position, uv0, uv1):");
        let mut shown = 0;
        for i in 0..part.positions.len() {
            if part.normals[i][1] < -0.9 && part.positions[i][2].abs() < 0.05 && shown < 6 {
                println!("    {:?} uv0 {:?} uv1 {:?}", part.positions[i], part.uv0[i], part.uv1[i]);
                shown += 1;
            }
        }
        println!("  a few up-looking vertices near the middle:");
        shown = 0;
        for i in 0..part.positions.len() {
            if part.normals[i][1] > 0.9 && part.positions[i][2].abs() < 0.05 && shown < 6 {
                println!("    {:?} uv0 {:?} uv1 {:?}", part.positions[i], part.uv0[i], part.uv1[i]);
                shown += 1;
            }
        }
    }
    // other decks' colour textures: sizes
    let toc = game.toc(ITEMS_TOC).unwrap();
    let mut names: Vec<&str> =
        toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.contains("/deck/") && n.ends_with("_ap_cas_main_bundlereftable")).collect();
    names.sort();
    println!("\n{} deck bundles; colour textures of a sample:", names.len());
    let sample: Vec<&&str> = names.iter().step_by((names.len() / 14).max(1)).take(14).collect();
    for n in sample {
        let b = game.bundle(n, ITEMS_TOC).unwrap();
        for (ri, r) in b.manifest.res.iter().enumerate() {
            if r.rtype == TEXTURE_TYPE && (r.name.ends_with("_c") || r.name.ends_with("_osk")) && r.name.contains("deck") {
                let payload = game.decode(&b.res_loc(ri).unwrap()).unwrap();
                let h = TextureHeader::parse(&payload, &r.meta).unwrap();
                println!("  {} x {} fmt {} {:?}  {}", h.width, h.height, h.format, BcFormat::from_frostbite(h.format), r.name);
            }
        }
    }
}
