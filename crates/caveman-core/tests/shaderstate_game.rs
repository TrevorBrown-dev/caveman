//! shaderstate.rs against real data: the global shader database and texture lookup rewrite byte for byte with nothing
//! added; the shared bundle's depot reads (every key); a depot written by ReSkate Studio (the installed FPV Drone mod's,
//! when present) is rebuilt byte for byte by `write_depot`. Needs the game (read only); run with `--ignored`.

use std::path::Path;
use std::sync::Arc;

use caveman_core::build::SHARED;
use caveman_core::game::{Game, Layer, ITEMS_TOC};
use caveman_core::{fxmod, shader, shaderstate};

const GAME: &str = r"E:\Games\ReSkate";

#[test]
#[ignore]
fn tables_round_trip() {
    let game = Arc::new(Game::open(Path::new(GAME)).unwrap());
    let config = game.bundle(shader::CONFIG_BUNDLE, shader::GLOBALS_TOC).unwrap();
    let res = |name: &str| {
        let i = config.manifest.res_index(name).unwrap();
        (game.decode(&config.files[config.manifest.res_file(i)]).unwrap(), config.manifest.res[i].meta)
    };
    let (db, meta) = res(fxmod::PROGRAM_LOOKUP);
    let (rewritten, m2) = shaderstate::add_program_rows(&db, &meta, &[]).unwrap();
    assert!(rewritten == db && m2 == meta, "the database rewrites byte for byte");
    let (_, rows) = shaderstate::program_rows(&db, &meta).unwrap();
    assert!(rows.windows(2).all(|w| w[0].0 < w[1].0), "the game's program lookup is sorted by key");
    let (tl, _) = res(fxmod::TEXTURE_LOOKUP);
    assert_eq!(shaderstate::add_texture_rows(&tl, &[]).unwrap(), tl, "the texture lookup rewrites byte for byte");
    println!("{} program rows, {} texture rows, {} records", rows.len(), tl.len() / 16, shaderstate::record_count(&db).unwrap());

    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    let di = shared.manifest.res.iter().position(|r| r.rtype == shaderstate::DEPOT_TYPE).unwrap();
    let depot = shaderstate::read_depot(&game.decode(&shared.files[shared.manifest.res_file(di)]).unwrap(), &shared.manifest.res[di].meta).unwrap();
    assert!(depot.entries.iter().any(|(k, _, _)| *k == fxmod::SPRITE.source_key), "the sprite's source key");
    println!("shared depot: {} keys", depot.entries.len());

    // a depot ReSkate Studio wrote (each key its own block): rebuilt byte for byte
    let drone = Path::new(GAME).join("Mods").join("zeex64-ReSkate_FPV_Drone");
    if !drone.is_dir() {
        println!("the FPV Drone mod is not installed: Studio's depot not checked");
        return;
    }
    let layer = Layer::open(&drone, game.clone());
    let toc = layer.toc("levels/game/dingolevel_root/dingolevel_root.toc").unwrap();
    let mut checked = 0;
    for b in &toc.bundles {
        let Some(region) = b.region.as_ref() else { continue };
        let (m, files) = layer.bundle(region).unwrap();
        for (i, r) in m.res.iter().enumerate() {
            let loc = files[m.res_file(i) - 1];
            if r.rtype != shaderstate::DEPOT_TYPE || !loc.patch {
                continue;
            }
            let data = layer.decode(&loc).unwrap();
            let d = shaderstate::read_depot(&data, &r.meta).unwrap();
            let (out, meta) = shaderstate::write_depot(&d);
            assert!(out == data && meta == r.meta, "{}: rebuilt byte for byte", r.name);
            println!("{}: {} keys rebuilt byte for byte", r.name, d.entries.len());
            checked += 1;
        }
    }
    assert!(checked > 0, "the drone mod's depot");
}
