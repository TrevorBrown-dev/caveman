//! Prints the skateboard's skeleton, pose, mesh and lod-group assets (ignored: reads the install).
//! `cargo test -p caveman-core --test ebx_dump -- --ignored --nocapture`

use caveman_core::ebx::{self, Data, Document, Object};
use caveman_core::game::{Game, ITEMS_TOC};
use caveman_core::mesh::{DECK_BUNDLE_SUFFIX, LEVEL_BUNDLE, LEVEL_TOC};
use std::collections::HashMap;
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

fn show(doc: &Document, names: &HashMap<ebx::Guid, String>, obj: &Object, indent: usize, max_array: usize, out: &mut String) {
    for f in &obj.fields {
        show_value(doc, names, &f.name, &f.value.data, indent, max_array, out);
    }
}

fn show_value(doc: &Document, names: &HashMap<ebx::Guid, String>, name: &str, data: &Data, indent: usize, max_array: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match data {
        Data::Object(o) => {
            out.push_str(&format!("{pad}{name}: {{ ({})\n", doc.type_name(o.descriptor)));
            show(doc, names, o, indent + 2, max_array, out);
            out.push_str(&format!("{pad}}}\n"));
        }
        Data::Array(items) => {
            out.push_str(&format!("{pad}{name}: [{}]\n", items.len()));
            for (i, v) in items.iter().enumerate().take(max_array) {
                show_value(doc, names, &format!("[{i}]"), &v.data, indent + 2, max_array, out);
            }
            if items.len() > max_array {
                out.push_str(&format!("{pad}  ... {} more\n", items.len() - max_array));
            }
        }
        Data::Pointer(p) => {
            let text = format!("{p:?}");
            let mut resolved = String::new();
            for (g, n) in names {
                if text.contains(&g.to_string()) {
                    resolved = format!(" = {n}");
                }
            }
            out.push_str(&format!("{pad}{name}: {text}{resolved}\n"));
        }
        other => out.push_str(&format!("{pad}{name}: {other:?}\n")),
    }
}

fn dump(game: &Game, toc: &str, bundle: &str, name: &str, names: &HashMap<ebx::Guid, String>, max_array: usize) {
    let b = game.bundle(bundle, toc).unwrap();
    let Some(i) = b.manifest.ebx_index(name) else {
        println!("\n==== {name}: not in {bundle}");
        return;
    };
    let bytes = game.decode(&b.ebx_loc(i).unwrap()).unwrap();
    let doc = ebx::read_document(&bytes).unwrap();
    println!("\n==== {name} ({} instances, {} imports, file {})", doc.instances.len(), doc.imports.len(), doc.file_guid);
    for (k, (f, inst)) in doc.imports.iter().enumerate() {
        println!("  import {k}: file {f} instance {inst}{}", names.get(f).map(|n| format!(" = {n}")).unwrap_or_default());
    }
    let mut out = String::new();
    for k in 0..doc.instances.len() {
        out.push_str(&format!(" instance {k}: {} ({})\n", doc.instance_type(k), doc.instances[k].instance_guid));
        if let Ok(o) = doc.object(k) {
            show(&doc, names, o, 4, max_array, &mut out);
        }
    }
    println!("{out}");
}

/// The file GUIDs of the EBX whose names contain any of `parts`, for resolving imports.
fn name_map(game: &Game, toc: &str, bundle: &str, parts: &[&str]) -> HashMap<ebx::Guid, String> {
    let b = game.bundle(bundle, toc).unwrap();
    let mut out = HashMap::new();
    for (i, e) in b.manifest.ebx.iter().enumerate() {
        let lower = e.name.to_lowercase();
        if parts.iter().any(|p| lower.contains(p)) {
            if let Some(loc) = b.ebx_loc(i) {
                if let Ok(bytes) = game.decode(&loc) {
                    if let Ok(g) = ebx::read_file_guid(&bytes) {
                        out.insert(g, e.name.clone());
                    }
                }
            }
        }
    }
    out
}

#[test]
#[ignore]
fn dump_board_assets() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let deck_bundle = game.find_bundle(DECK_BUNDLE_SUFFIX, ITEMS_TOC).unwrap().unwrap();
    let parts = ["skateboard", "deck_gen_popsicle", "gripcolor", "wheel_gen_classic", "skeleton", "char_skateboard", "default_pose"];
    let mut names = name_map(&game, ITEMS_TOC, &deck_bundle, &parts);
    names.extend(name_map(&game, LEVEL_TOC, LEVEL_BUNDLE, &parts));
    println!("{} names mapped", names.len());
    dump(&game, ITEMS_TOC, &deck_bundle, "characters/skateboard/unlicensed/deck/generic/popsicle/2022/deck_gen_popsicle_mesh", &names, 12);
    dump(&game, ITEMS_TOC, &deck_bundle, "characters/skateboard/unlicensed/deck/generic/popsicle/2022/deck_gen_popsicle", &names, 12);
    dump(&game, ITEMS_TOC, &deck_bundle, "characters/maincharacters/common/lodgroups/char_skateboard", &names, 12);
    dump(&game, LEVEL_TOC, LEVEL_BUNDLE, "characters/_prototype/skateboard/skateboard_default_skeleton", &names, 64);
    dump(&game, LEVEL_TOC, LEVEL_BUNDLE, "characters/_prototype/skateboardv2/skateboardv2_default_skeleton", &names, 64);
    dump(&game, LEVEL_TOC, LEVEL_BUNDLE, "animation/dingo/skateboard_default_pose", &names, 64);
    dump(&game, LEVEL_TOC, LEVEL_BUNDLE, "characters/_prototype/skateboard/trucks/skateboard_trucks_default_mesh", &names, 12);
    dump(&game, LEVEL_TOC, LEVEL_BUNDLE, "characters/_prototype/skateboard/wheels/skateboard_wheels_default_mesh", &names, 12);
}
