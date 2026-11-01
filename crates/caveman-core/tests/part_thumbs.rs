//! The menu thumbnails of the equipped wheel colours and grip, from the shared bundle: the mean colour of their
//! opaque pixels against the presets' colours, to see how the game applies a region colour and the grip tint
//! (ignored: reads the install). `cargo test -p caveman-core --test part_thumbs -- --ignored --nocapture`

use caveman_core::boardmodel::texture_image;
use caveman_core::build::SHARED;
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
#[ignore]
fn thumbnails_against_colours() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    std::fs::create_dir_all(&dir).unwrap();
    for (thumb, colour_linear) in [
        ("thumbnail/tool/own_wheelcolor_gen_classic00001_lrg", Some([0.7388f64, 0.5526, 0.3814])),
        ("thumbnail/tool/own_wheelcolor_gen_classic00005_lrg", Some([0.9734, 0.0648, 0.1683])),
        ("thumbnail/tool/own_deckgripcolor_gen_popsicle00001_lrg", Some([0.0481, 0.0440, 0.0537])),
        ("thumbnail/tool/own_deckgripcolor_gen_popsicle00023_lrg", None),
        ("thumbnail/tool/own_truck_gen_default_00006_lrg", None),
    ] {
        let img = match texture_image(&game, ITEMS_TOC, &shared, thumb) {
            Ok(i) => i,
            Err(e) => {
                println!("{thumb}: {e}");
                continue;
            }
        };
        let out = dir.join(format!("{}.png", thumb.rsplit('/').next().unwrap()));
        img.write_png(&out).unwrap();
        // the opaque pixels: mean and median per channel (sRGB), and the brightest decile
        let mut chans: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for px in img.data.as_chunks::<4>().0 {
            if px[3] > 250 {
                for c in 0..3 {
                    chans[c].push(px[c]);
                }
            }
        }
        let n = chans[0].len();
        if n == 0 {
            println!("{thumb}: no opaque pixels");
            continue;
        }
        let mut line = format!("{thumb} ({} x {}, {n} opaque px):", img.width, img.height);
        for c in 0..3 {
            chans[c].sort_unstable();
            let mean = chans[c].iter().map(|&v| v as f64).sum::<f64>() / n as f64;
            let median = chans[c][n / 2];
            let p90 = chans[c][n * 9 / 10];
            line.push_str(&format!(" [{}: mean {mean:.0} median {median} p90 {p90}]", ["R", "G", "B"][c]));
        }
        println!("{line}");
        if let Some(c) = colour_linear {
            let srgb: Vec<String> =
                c.iter().map(|v| format!("{:.0}", 255.0 * if *v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 })).collect();
            let med_lin: Vec<String> = (0..3).map(|k| format!("{:.3}", srgb_to_linear(chans[k][n / 2] as f64 / 255.0))).collect();
            println!("    preset colour linear {c:?} = sRGB ({}); the median's linear ({})", srgb.join(", "), med_lin.join(", "));
        }
        println!("    wrote {}", out.display());
    }
}
