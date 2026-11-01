//! The skeleton asset's bind pose as quaternions, the parts' UV ranges per face, the wheel colour bundles of items.toc,
//! and the parts' textures as PNG files to look at (ignored: reads the install).
//! `cargo test -p caveman-core --test board_frames -- --ignored --nocapture`

use caveman_core::boardmodel::{self, LANES};
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";
const TEXTURE_TYPE: u32 = 0x6bde_20ba;

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
fn frames_uvs_and_wheel_bundles() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let sk = boardmodel::read_skeleton(&game).unwrap();
    println!("asset bind pose (local trans | local quat xyzw | model trans | model quat xyzw):");
    for k in 0..sk.names.len() {
        let (lt, lq, mt, mq) = (sk.local[k].trans, sk.local[k].quaternion(), sk.model[k].trans, sk.model[k].quaternion());
        let lane = LANES.iter().position(|l| l.eq_ignore_ascii_case(&sk.names[k]));
        println!(
            "{k:>2} {:<18} parent {:>2} lane {:>2} | {:+.4} {:+.4} {:+.4} | {:+.4} {:+.4} {:+.4} {:+.4} | {:+.4} {:+.4} {:+.4} | {:+.4} {:+.4} {:+.4} {:+.4}",
            sk.names[k],
            sk.parents[k],
            lane.map(|l| l as i32).unwrap_or(-1),
            lt[0],
            lt[1],
            lt[2],
            lq[0],
            lq[1],
            lq[2],
            lq[3],
            mt[0],
            mt[1],
            mt[2],
            mq[0],
            mq[1],
            mq[2],
            mq[3]
        );
    }
    let model = boardmodel::read_board_model(&game).unwrap();
    println!("\nparts' UV ranges (whole; the faces looking down, up and sideways; the nose end's v):");
    for p in &model.parts {
        let pick = |f: &dyn Fn(usize) -> bool| -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
            let idx: Vec<usize> = (0..p.positions.len()).filter(|&i| f(i)).collect();
            (idx.iter().map(|&i| p.uv0[i]).collect(), idx.iter().map(|&i| p.uv1[i]).collect())
        };
        let (d0, d1) = pick(&|i| p.normals[i][1] < -0.7);
        let (u0, u1) = pick(&|i| p.normals[i][1] > 0.7);
        let (s0, s1) = pick(&|i| p.normals[i][1].abs() < 0.3);
        println!("  {:<40} uv0 {:?} uv1 {:?}", p.name, range(&p.uv0), range(&p.uv1));
        println!("      down {:>5}: uv0 {:?} uv1 {:?}", d0.len(), range(&d0), range(&d1));
        println!("      up   {:>5}: uv0 {:?} uv1 {:?}", u0.len(), range(&u0), range(&u1));
        println!("      side {:>5}: uv0 {:?} uv1 {:?}", s0.len(), range(&s0), range(&s1));
        if p.name.starts_with("Deck") {
            // the vertex furthest along +z (the nose end) and -z: where in the texture they are
            let (mut zmax, mut zmin) = ((f32::NEG_INFINITY, 0usize), (f32::INFINITY, 0usize));
            for (i, q) in p.positions.iter().enumerate() {
                if q[2] > zmax.0 {
                    zmax = (q[2], i);
                }
                if q[2] < zmin.0 {
                    zmin = (q[2], i);
                }
            }
            println!(
                "      +z end at {:?}: uv0 {:?} uv1 {:?}; -z end at {:?}: uv0 {:?} uv1 {:?}",
                p.positions[zmax.1], p.uv0[zmax.1], p.uv1[zmax.1], p.positions[zmin.1], p.uv0[zmin.1], p.uv1[zmin.1]
            );
        }
    }
    let toc = game.toc(ITEMS_TOC).unwrap();
    let mut names: Vec<&str> = toc.toc.bundles.iter().map(|b| b.name.as_str()).filter(|n| n.to_lowercase().contains("wheel")).collect();
    names.sort();
    println!("\n{} wheel bundles", names.len());
    for n in names.iter().filter(|n| n.contains("wheelcolor")).take(1) {
        let b = game.bundle(n, ITEMS_TOC).unwrap();
        println!("  textures of {n}:");
        for r in b.manifest.res.iter().filter(|r| r.rtype == TEXTURE_TYPE) {
            println!("    {}", r.name);
        }
    }
}

/// Writes the model's textures as PNGs under target/board-textures, to look at.
#[test]
#[ignore]
fn dump_textures() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let model = boardmodel::read_board_model(&game).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    std::fs::create_dir_all(&dir).unwrap();
    for m in &model.materials {
        for (kind, img) in [("c", &m.base), ("n", &m.normal), ("orm", &m.orm)] {
            if let Some(img) = img {
                let out = dir.join(format!("{}_{kind}.png", m.role));
                img.write_png(&out).unwrap();
                println!("wrote {} ({} x {})", out.display(), img.width, img.height);
            }
        }
    }
}
