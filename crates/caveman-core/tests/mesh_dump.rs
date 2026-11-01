//! A look at the skateboard meshes' vertex declarations and sections (ignored: reads the install). Run with
//! `cargo test -p caveman-core --test mesh_dump -- --ignored --nocapture`.

use caveman_core::game::{Game, ITEMS_TOC};
use caveman_core::mesh::{self, DECK_BUNDLE_SUFFIX, DECK_MESH, LEVEL_BUNDLE, LEVEL_TOC, MESHSET_TYPE, TRUCKS_MESH, WHEELS_MESH};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn ptr_at(b: &[u8], o: usize) -> usize {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize + 16
}

fn dump(game: &Game, toc: &str, bundle: &str, name: &str) {
    let b = game.bundle(bundle, toc).unwrap();
    let i = b.manifest.res.iter().position(|r| r.rtype == MESHSET_TYPE && r.name == name).unwrap();
    let res = game.decode(&b.res_loc(i).unwrap()).unwrap();
    let l0 = mesh::lod(&res, 0).unwrap();
    let geometry: Option<Vec<u8>> = match game.chunk(&l0.chunk, toc).unwrap() {
        Some(c) if !c.removed => Some(game.decode(&caveman_core::game::chunk_loc(&c)).unwrap()),
        _ => b.manifest.chunk_index(&l0.chunk).and_then(|ci| b.chunk_loc(ci)).map(|loc| game.decode(&loc).unwrap()),
    };
    let geometry = geometry.as_deref();
    println!("\n==== {name} ({} bytes; layout {} {} {}; lods {})", res.len(), u32_at(&res, 0), u32_at(&res, 4), u32_at(&res, 8), u16_at(&res, 180));
    // the EBX beside it: names that may tell the materials and skeleton
    for e in &b.manifest.ebx {
        let leaf = e.name.rsplit('/').next().unwrap_or(&e.name);
        if e.name.contains("_prototype/skateboard")
            || leaf.contains("deck_gen_popsicle")
            || leaf.contains("skeleton")
            || leaf.contains("default_pose")
            || leaf.contains("wheel_gen_classic")
            || leaf.contains("char_skateboard")
            || leaf.contains("skateboardwear")
        {
            println!("  ebx: {}", e.name);
        }
    }
    let lod_count = u16_at(&res, 180) as usize;
    for lod_i in 0..lod_count.min(1) {
        let at = ptr_at(&res, 48 + lod_i * 8);
        let geometry = if lod_i == 0 { geometry } else { None };
        let sections = u32_at(&res, at + 8) as usize;
        println!(
            "  lod {lod_i}: kind {} sections {} index format {} vb {} ib {}",
            u32_at(&res, at),
            sections,
            u32_at(&res, at + 84),
            u32_at(&res, at + 92),
            u32_at(&res, at + 88)
        );
        let table = ptr_at(&res, at + 12);
        for s in 0..sections {
            let sec = table + s * 384;
            let d = sec + 112;
            let (elements, streams) = (res[d + 96] as usize, res[d + 97] as usize);
            let palette_count = u32_at(&res, sec + 24);
            let mut palette = Vec::new();
            if palette_count > 0 && palette_count < 256 {
                let p = ptr_at(&res, sec + 16);
                for k in 0..palette_count as usize {
                    palette.push(u16_at(&res, p + k * 2));
                }
            }
            // a name pointer? print the first bytes of the section for a look
            println!(
                "    section {s}: triangles {} first index {} vertex start {} count {} primitive {} bones {:?}",
                u32_at(&res, sec + 32),
                u32_at(&res, sec + 36),
                u32_at(&res, sec + 40),
                u32_at(&res, sec + 44),
                res[sec + 31],
                palette
            );
            // any pointer in the first 16 bytes may be the material or name
            for o in (0..16).step_by(8) {
                let v = u64::from_le_bytes(res[sec + o..sec + o + 8].try_into().unwrap());
                if v != 0 && (v as usize) < res.len() {
                    let p = v as usize + 16;
                    let text: String = res[p..(p + 48).min(res.len())]
                        .iter()
                        .take_while(|&&c| c != 0)
                        .map(|&c| if c.is_ascii_graphic() || c == b' ' { c as char } else { '.' })
                        .collect();
                    println!("      +{o}: pointer to {p} {text:?}");
                }
            }
            for k in 0..streams {
                println!("      stream {k}: stride {} class {}", res[d + 64 + k * 2], res[d + 64 + k * 2 + 1]);
            }
            for e in 0..elements {
                let el = d + e * 4;
                println!("      element {e}: usage {} format {} offset {} stream {}", res[el], res[el + 1], res[el + 2], res[el + 3]);
            }
            if let Some(geo) = geometry {
                let count = u32_at(&res, sec + 44) as usize;
                let mut base = u32_at(&res, sec + 40) as usize;
                let mut bases = Vec::new();
                for k in 0..streams {
                    bases.push((base, res[d + 64 + k * 2] as usize));
                    base += res[d + 64 + k * 2] as usize * count;
                }
                for v in [0usize, 1, 2, count / 2, count - 1] {
                    let mut line = format!("      vertex {v}:");
                    for (k, (b, stride)) in bases.iter().enumerate() {
                        let o = b + v * stride;
                        let bytes = &geo[o..o + stride];
                        line.push_str(&format!(" s{k}={}", bytes.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join("")));
                    }
                    println!("{line}");
                }
                // bone indices seen in the whole section (stream 1 as u16 x4) and the weights' sums
                let (b1, st1) = bases[1];
                let (b2, st2) = bases[2];
                let mut seen = std::collections::BTreeMap::new();
                let mut sums = std::collections::BTreeSet::new();
                for v in 0..count {
                    let o = b1 + v * st1;
                    let w = &geo[b2 + v * st2..b2 + v * st2 + st2];
                    sums.insert(w.iter().map(|&x| x as u32).sum::<u32>());
                    for k in 0..st1 / 2 {
                        let idx = u16_at(geo, o + k * 2);
                        if w[k] > 0 {
                            *seen.entry(idx).or_insert(0usize) += 1;
                        }
                    }
                }
                println!("      weighted bone indices (index: vertices): {seen:?}; weight sums {sums:?}");
            }
        }
    }
}

#[test]
#[ignore]
fn dump_board_meshes() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let deck_bundle = game.find_bundle(DECK_BUNDLE_SUFFIX, ITEMS_TOC).unwrap().unwrap();
    dump(&game, ITEMS_TOC, &deck_bundle, DECK_MESH);
    dump(&game, LEVEL_TOC, LEVEL_BUNDLE, TRUCKS_MESH);
    dump(&game, LEVEL_TOC, LEVEL_BUNDLE, WHEELS_MESH);
    let m = mesh::deck_mesh(&game).unwrap();
    println!("deck mesh as read: {} vertices {} triangles {} sections", m.positions.len(), m.triangles.len(), m.sections.len());
}
