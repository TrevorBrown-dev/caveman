//! Item ids already taken (caveman.existing_items): the game's and every installed mod's, so a new deck or wheel
//! never shares an id with another (ReSkate would leave one of the two mods out whole).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use caveman_core::build::SHARED;
use caveman_core::game::{Game, Layer, ITEMS_TOC};

use crate::report::Reporter;

/// Lower-case item ids already taken ("own_..."), with who has them: "the game" or the mod's folder name. Every mod
/// in `mods_dir` counts except `skip` (Caveman's own) and dot-folders; a mod that cannot be read cannot collide
/// either, so it gets a note and is passed over.
pub fn existing_items(game: &Arc<Game>, mods_dir: &Path, skip: &str, report: &mut Reporter) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut taken = BTreeMap::new();
    let shared = game.bundle(SHARED, ITEMS_TOC)?;
    for e in &shared.manifest.ebx {
        if e.name.starts_with("items/") {
            taken.insert(leaf(&e.name), "the game".to_string());
        }
    }
    let mut mods: Vec<String> = match std::fs::read_dir(mods_dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect(),
        Err(_) => Vec::new(),
    };
    mods.sort();
    for m in mods {
        let root = mods_dir.join(&m);
        if m.starts_with('.') || m == skip || !root.join("Win32").join(ITEMS_TOC).is_file() {
            continue;
        }
        if let Err(error) = mod_items(game, &root, &m, &mut taken) {
            report.log(&format!("  note: could not read {m} ({error})"));
        }
    }
    Ok(taken)
}

fn mod_items(game: &Arc<Game>, root: &Path, name: &str, taken: &mut BTreeMap<String, String>) -> Result<(), Box<dyn std::error::Error>> {
    let layer = Layer::open(root, game.clone());
    let toc = layer.toc(ITEMS_TOC)?;
    for b in &toc.bundles {
        let Some(region) = &b.region else { continue };
        if !b.name.ends_with("cas_main_sharedbundle") {
            continue;
        }
        let (m, files) = layer.bundle(region)?;
        for (e, f) in m.ebx.iter().zip(files.iter()) {
            if f.patch && e.name.starts_with("items/") {
                taken.entry(leaf(&e.name)).or_insert_with(|| name.to_string());
            }
        }
    }
    Ok(())
}

fn leaf(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_string()
}
