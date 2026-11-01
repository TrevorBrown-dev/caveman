//! Draws where the wheel mesh's faces land on the wheel texture: the +x face's vertices red, the -x face's green, the
//! tread's blue, over `wheel_gen_classic_c` scaled up 4x (ignored: reads the install).
//! `cargo test -p caveman-core --test wheel_uv_overlay -- --ignored --nocapture`

use caveman_core::boardmodel::{self, texture_image};
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn wheel_uv_overlay() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    let model = boardmodel::read_board_model(&game).unwrap();
    let toc = game.toc(ITEMS_TOC).unwrap();
    let mut names: Vec<&str> =
        toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.contains("wheelcolor/presets/") && n.ends_with("_ap_cas_main_bundlereftable")).collect();
    names.sort();
    let b = game.bundle(names[0], ITEMS_TOC).unwrap();
    let tex = texture_image(&game, ITEMS_TOC, &b, "characters/skateboard/unlicensed/wheel/generic/classic/2022/wheel_gen_classic_c").unwrap();
    let scale = 4;
    let mut img = tex.to_rgb().resize_lanczos(tex.width * scale, tex.height * scale);
    let (w, h) = (img.width, img.height);
    for name in ["Right_Wheelback"] {
        let wheel = model.parts.iter().find(|p| p.name.contains(name)).unwrap();
        for i in 0..wheel.positions.len() {
            let nx = wheel.normals[i][0];
            let colour: [u8; 3] = if nx > 0.7 {
                [255, 0, 0]
            } else if nx < -0.7 {
                [0, 200, 0]
            } else if nx.abs() < 0.3 {
                [0, 90, 255]
            } else {
                [255, 255, 0]
            };
            let uv = wheel.uv0[i];
            let (x, y) = (((uv[0].rem_euclid(1.0)) * w as f32) as usize, ((uv[1].rem_euclid(1.0)) * h as f32) as usize);
            for dy in 0..3usize {
                for dx in 0..3usize {
                    let (px, py) = ((x + dx).min(w - 1), (y + dy).min(h - 1));
                    let k = (py * w + px) * 3;
                    img.data[k..k + 3].copy_from_slice(&colour);
                }
            }
        }
    }
    let out = dir.join("wheel_uv_overlay.png");
    img.write_png(&out).unwrap();
    println!("wrote {} ({} x {}): red +x face, green -x face, blue tread, yellow in between", out.display(), w, h);
    // a few vertices of the +x face with their uv and position, to see the mapping's sense
    let wheel = model.parts.iter().find(|p| p.name.contains("Right_Wheelback")).unwrap();
    let mut shown = 0;
    for i in 0..wheel.positions.len() {
        if wheel.normals[i][0] > 0.9 && shown < 8 {
            let p = wheel.positions[i];
            let r = (p[1] * p[1] + p[2] * p[2]).sqrt();
            let ang = p[2].atan2(p[1]).to_degrees();
            println!("  +x face vertex: radius {r:.4} angle {ang:7.1} uv0 {:?}", wheel.uv0[i]);
            shown += 1;
        }
    }
}
