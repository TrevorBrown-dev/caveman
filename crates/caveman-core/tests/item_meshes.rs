//! The truck and wheel items' own meshes (`truck_gen_default_mesh`, `wheel_gen_classic_mesh`, in the model bundles
//! the item records name) against the level root's prototypes: sections, bones, UV layout, and the wheel's UVs
//! drawn over its texture (ignored: reads the install).
//! `cargo test -p caveman-core --test item_meshes -- --ignored --nocapture`

use caveman_core::boardmodel::texture_image;
use caveman_core::game::{Game, ITEMS_TOC};
use caveman_core::mesh;
use std::collections::BTreeMap;
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

fn describe(label: &str, sections: &[mesh::FullSection]) {
    println!("== {label}: {} section(s)", sections.len());
    for s in sections {
        let mut per_bone: BTreeMap<u16, usize> = BTreeMap::new();
        for v in &s.vertices {
            *per_bone.entry(v.bone).or_default() += 1;
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        let (mut plo, mut phi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in &s.vertices {
            for k in 0..2 {
                lo[k] = lo[k].min(v.uv0[k]);
                hi[k] = hi[k].max(v.uv0[k]);
            }
            for k in 0..3 {
                plo[k] = plo[k].min(v.position[k]);
                phi[k] = phi[k].max(v.position[k]);
            }
        }
        println!(
            "   {:?}: {} vertices {} triangles, bones {per_bone:?}, uv1 {}, normals {}, uv0 {lo:?}..{hi:?}, bounds {plo:?}..{phi:?}",
            s.material,
            s.vertices.len(),
            s.triangles.len(),
            s.has_uv1,
            s.has_normals
        );
    }
}

#[test]
#[ignore]
fn item_meshes() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    let truck_bundle = game.find_bundle("/truck_gen_default_cas_main_bundlereftable", ITEMS_TOC).unwrap().unwrap();
    let trucks =
        mesh::bundle_mesh_full(&game, ITEMS_TOC, &truck_bundle, "characters/skateboard/unlicensed/truck/generic/default/2022/truck_gen_default_mesh", 0)
            .unwrap();
    describe("truck_gen_default_mesh", &trucks);
    let wheel_bundle = game.find_bundle("/wheel_gen_classic_cas_main_bundlereftable", ITEMS_TOC).unwrap().unwrap();
    let wheels =
        mesh::bundle_mesh_full(&game, ITEMS_TOC, &wheel_bundle, "characters/skateboard/unlicensed/wheel/generic/classic/2022/wheel_gen_classic_mesh", 0)
            .unwrap();
    describe("wheel_gen_classic_mesh", &wheels);
    // the prototypes, for the record
    let proto_t = mesh::bundle_mesh_full(&game, mesh::LEVEL_TOC, mesh::LEVEL_BUNDLE, mesh::TRUCKS_MESH, 0).unwrap();
    describe("prototype trucks", &proto_t);
    let proto_w = mesh::bundle_mesh_full(&game, mesh::LEVEL_TOC, mesh::LEVEL_BUNDLE, mesh::WHEELS_MESH, 0).unwrap();
    describe("prototype wheels", &proto_w);
    // how many levels of detail each has
    for (label, bundle, toc, name) in [
        ("truck_gen_default_mesh", truck_bundle.as_str(), ITEMS_TOC, "characters/skateboard/unlicensed/truck/generic/default/2022/truck_gen_default_mesh"),
        ("wheel_gen_classic_mesh", wheel_bundle.as_str(), ITEMS_TOC, "characters/skateboard/unlicensed/wheel/generic/classic/2022/wheel_gen_classic_mesh"),
    ] {
        let mut lods = 0;
        while mesh::bundle_mesh_full(&game, toc, bundle, name, lods).is_ok() && lods < 8 {
            lods += 1;
        }
        println!("   {label}: {lods} level(s) of detail");
    }
    // the wheel's UVs over its texture: the first wheel bone's vertices, by face
    let b = game.bundle(&wheel_bundle, ITEMS_TOC).unwrap();
    let tex = texture_image(&game, ITEMS_TOC, &b, "characters/skateboard/unlicensed/wheel/generic/classic/2022/wheel_gen_classic_c").unwrap();
    let scale = 4;
    let mut img = tex.to_rgb().resize_lanczos(tex.width * scale, tex.height * scale);
    let (w, h) = (img.width, img.height);
    if let Some(s) = wheels.first() {
        let first_bone = s.vertices.iter().map(|v| v.bone).min().unwrap_or(0);
        // the wheel's centre and axle: the bone's vertices' mean, the axle x in mesh space
        let verts: Vec<&mesh::FullVertex> = s.vertices.iter().filter(|v| v.bone == first_bone).collect();
        let n = verts.len() as f32;
        let centre = verts.iter().fold([0f32; 3], |a, v| [a[0] + v.position[0] / n, a[1] + v.position[1] / n, a[2] + v.position[2] / n]);
        println!("   wheel bone {first_bone}: {} vertices, centre {centre:?}", verts.len());
        for v in &verts {
            let nx = v.normal[0];
            let colour: [u8; 3] = if nx > 0.7 {
                [255, 0, 0]
            } else if nx < -0.7 {
                [0, 200, 0]
            } else if nx.abs() < 0.3 {
                [0, 90, 255]
            } else {
                [255, 255, 0]
            };
            let (x, y) = (((v.uv0[0].rem_euclid(1.0)) * w as f32) as usize, ((v.uv0[1].rem_euclid(1.0)) * h as f32) as usize);
            for dy in 0..3usize {
                for dx in 0..3usize {
                    let (px, py) = ((x + dx).min(w - 1), (y + dy).min(h - 1));
                    let k = (py * w + px) * 3;
                    img.data[k..k + 3].copy_from_slice(&colour);
                }
            }
        }
    }
    let out = dir.join("wheel_item_uv_overlay.png");
    img.write_png(&out).unwrap();
    println!("wrote {}", out.display());
}
