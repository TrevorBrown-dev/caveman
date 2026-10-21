//! The bundle reference table (`brt.rs`) against the game's own table and every installed mod's copy, as
//! `tools/re/test_brt.py` does: each copy is rebuilt from the base by inserting the presets it adds in the order they
//! sit in its pool; a byte-identical rebuild means the mod's tool writes tables exactly as ReSkate's merge does.
//! Needs the game (read only); run with `--ignored`.

use std::path::Path;
use std::sync::Arc;

use caveman_core::brt::{self, Builder, Table};
use caveman_core::build::{BRT, SHARED};
use caveman_core::game::{Game, Layer, ITEMS_TOC};

const GAME: &str = r"E:\Games\ReSkate";

/// The table resource and meta a layer ships, if it ships a changed one.
fn mod_table(layer: &Layer) -> Option<(Vec<u8>, Vec<u8>)> {
    let toc = layer.toc(ITEMS_TOC).ok()?;
    let b = toc.bundles.iter().find(|b| b.name == SHARED)?;
    let (m, files) = layer.bundle(b.region.as_ref()?).ok()?;
    let ri = m.res_index(BRT)?;
    let f = files[m.ebx.len() + ri];
    if !f.patch {
        return None;
    }
    Some((layer.decode(&f).ok()?, m.res[ri].meta.to_vec()))
}

#[test]
#[ignore]
fn tables_match_python() {
    let game = Arc::new(Game::open(Path::new(GAME)).unwrap());
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    let ri = shared.manifest.res_index(BRT).unwrap();
    let data = game.decode(&shared.files[shared.manifest.res_file(ri)]).unwrap();
    let base = Table::parse(&data, &shared.manifest.res[ri].meta).unwrap();
    println!("base: {} bytes, {} rows, {} presets, {} bundles", data.len(), base.rows.len(), base.presets.len(), base.bundle_count);
    assert_eq!(Builder::new(&base).unwrap().write().0, base.data, "unchanged write");
    let (mut identical, mut equivalent, mut different) = (0, 0, 0);
    let mut copies = Vec::new();
    let mut mods: Vec<_> = std::fs::read_dir(Path::new(GAME).join("Mods")).unwrap().map(|e| e.unwrap().path()).collect();
    mods.sort();
    for dir in mods {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with('.') || !dir.join("Win32").join(ITEMS_TOC).is_file() {
            continue;
        }
        let Some((cdata, cmeta)) = mod_table(&Layer::open(&dir, game.clone())) else { continue };
        let copy = Table::parse(&cdata, &cmeta).unwrap();
        let added = match brt::added_presets(&base, &copy) {
            Ok(a) => a,
            Err(e) => {
                println!("  {name}: refused ({e})");
                different += 1;
                continue;
            }
        };
        let mut builder = Builder::new(&base).unwrap();
        for (path, b) in &added {
            builder.insert(path, *b).unwrap();
        }
        let (rdata, rmeta) = builder.write();
        if rdata == cdata && rmeta.to_vec() == cmeta {
            identical += 1;
        } else {
            let rebuilt = Table::parse(&rdata, &rmeta).unwrap();
            let rows = |t: &Table| {
                let mut v: Vec<(u64, u32, String)> = t.rows.iter().map(|r| (r.0, r.1, t.text(r.2).unwrap())).collect();
                v.sort();
                v
            };
            if rows(&rebuilt) == rows(&copy) {
                equivalent += 1;
            } else {
                different += 1;
                println!("  {name}: lookups differ");
            }
        }
        copies.push(copy);
    }
    let merged = brt::merge(&base, &copies).unwrap();
    let mt = Table::parse(&merged.data, &merged.meta).unwrap();
    println!(
        "mods with a changed table: {}; rebuilt byte-identical: {identical}; equivalent (same lookups): {equivalent}; different: {different}",
        identical + equivalent + different
    );
    println!(
        "merge of all copies: {} presets added, {} conflicts, {} under their full path only; merged table parses with {} rows",
        merged.added,
        merged.conflicts,
        merged.shadowed.len(),
        mt.rows.len()
    );
    assert_eq!(different, 0);
}
