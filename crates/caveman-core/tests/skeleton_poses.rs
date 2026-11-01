//! The skateboard skeleton asset's bind pose per bone (local and model), as translations and quaternions, to match the
//! runtime pose lanes against (ignored: reads the install).
//! `cargo test -p caveman-core --test skeleton_poses -- --ignored --nocapture`

use caveman_core::ebx::{self, Data};
use caveman_core::game::Game;
use caveman_core::mesh::{self, LEVEL_BUNDLE, LEVEL_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

fn vec3(d: &Data) -> [f32; 3] {
    let o = d.as_object().unwrap();
    let f = |n: &str| match o.get(n).unwrap() {
        Data::F32(v) => *v,
        other => panic!("{n}: {other:?}"),
    };
    [f("x"), f("y"), f("z")]
}

/// (right, up, forward, trans) of a LinearTransform.
fn transform(d: &Data) -> [[f32; 3]; 4] {
    let o = d.as_object().unwrap();
    [vec3(o.get("right").unwrap()), vec3(o.get("up").unwrap()), vec3(o.get("forward").unwrap()), vec3(o.get("trans").unwrap())]
}

/// A quaternion (x, y, z, w) from the basis vectors right, up, forward (the columns of the rotation).
fn quat(t: &[[f32; 3]; 4]) -> [f32; 4] {
    let (r, u, f) = (t[0], t[1], t[2]);
    // m[row][col] with columns r, u, f
    let m = [[r[0], u[0], f[0]], [r[1], u[1], f[1]], [r[2], u[2], f[2]]];
    let trace = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m[2][1] - m[1][2]) / s;
        y = (m[0][2] - m[2][0]) / s;
        z = (m[1][0] - m[0][1]) / s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    [x, y, z, w]
}

#[test]
#[ignore]
fn skeleton_bind_pose() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let b = game.bundle(LEVEL_BUNDLE, LEVEL_TOC).unwrap();
    for name in ["characters/_prototype/skateboard/skateboard_default_skeleton", "characters/_prototype/skateboardv2/skateboardv2_default_skeleton"] {
        let i = b.manifest.ebx_index(name).unwrap();
        let doc = ebx::read_document(&game.decode(&b.ebx_loc(i).unwrap()).unwrap()).unwrap();
        let names: Vec<String> = doc.get("BoneNames", 0).unwrap().as_array().unwrap().iter().map(|v| v.data.as_str().unwrap()).collect();
        let parents: Vec<i64> = doc.get("Hierarchy", 0).unwrap().as_array().unwrap().iter().map(|v| v.data.as_i64().unwrap()).collect();
        let local: Vec<[[f32; 3]; 4]> = doc.get("LocalPose", 0).unwrap().as_array().unwrap().iter().map(|v| transform(&v.data)).collect();
        let model: Vec<[[f32; 3]; 4]> = doc.get("ModelPose", 0).unwrap().as_array().unwrap().iter().map(|v| transform(&v.data)).collect();
        println!("\n==== {name}");
        println!(" bone name             parent | local trans              | local quat x y z w            | model trans");
        for k in 0..names.len() {
            let (lt, lq, mt) = (local[k][3], quat(&local[k]), model[k][3]);
            println!(
                "{k:>2} {:<20} {:>3}  | {:+.4} {:+.4} {:+.4} | {:+.4} {:+.4} {:+.4} {:+.4} | {:+.4} {:+.4} {:+.4}",
                names[k], parents[k], lt[0], lt[1], lt[2], lq[0], lq[1], lq[2], lq[3], mt[0], mt[1], mt[2]
            );
        }
    }
    // the deck mesh's sections and bones, with the full reader
    let deck_bundle = game.find_bundle(mesh::DECK_BUNDLE_SUFFIX, caveman_core::game::ITEMS_TOC).unwrap().unwrap();
    let sections = mesh::bundle_mesh_full(&game, caveman_core::game::ITEMS_TOC, &deck_bundle, mesh::DECK_MESH, 0).unwrap();
    for s in &sections {
        let mut bones = std::collections::BTreeSet::new();
        for v in &s.vertices {
            bones.insert(v.bone);
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in &s.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.position[k]);
                hi[k] = hi[k].max(v.position[k]);
            }
        }
        println!(
            "deck section {:?}: {} vertices, bones {bones:?}, uv1 {}, normals {}, bounds {lo:?}..{hi:?}",
            s.material,
            s.vertices.len(),
            s.has_uv1,
            s.has_normals
        );
    }
    let trucks = mesh::bundle_mesh_full(&game, LEVEL_TOC, LEVEL_BUNDLE, mesh::TRUCKS_MESH, 0).unwrap();
    for s in &trucks {
        let mut per_bone: std::collections::BTreeMap<u16, ([f32; 3], [f32; 3], usize)> = std::collections::BTreeMap::new();
        for v in &s.vertices {
            let e = per_bone.entry(v.bone).or_insert(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3], 0));
            for k in 0..3 {
                e.0[k] = e.0[k].min(v.position[k]);
                e.1[k] = e.1[k].max(v.position[k]);
            }
            e.2 += 1;
        }
        println!("trucks section {:?}: per bone (min..max, count): {per_bone:?}", s.material);
    }
    let wheels = mesh::bundle_mesh_full(&game, LEVEL_TOC, LEVEL_BUNDLE, mesh::WHEELS_MESH, 0).unwrap();
    for s in &wheels {
        let mut per_bone: std::collections::BTreeMap<u16, ([f32; 3], usize)> = std::collections::BTreeMap::new();
        for v in &s.vertices {
            let e = per_bone.entry(v.bone).or_insert(([0.0; 3], 0));
            for k in 0..3 {
                e.0[k] += v.position[k];
            }
            e.1 += 1;
        }
        let centres: Vec<(u16, [f32; 3])> = per_bone.iter().map(|(b, (sum, n))| (*b, [sum[0] / *n as f32, sum[1] / *n as f32, sum[2] / *n as f32])).collect();
        println!("wheels section {:?}: centres per bone: {centres:?}", s.material);
    }
}
