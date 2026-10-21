//! The installed game and mod layers (gamedata.Game, fb.Layer, caveman.ModLayer).
mod io_common;

use caveman_core::bundle::MOD_INSTALL_CHUNK;
use caveman_core::game::{chunk_loc, Layer, ITEMS_TOC};
use io_common::*;

const SHARED: &str = "win32/characters/customization/configs/cas_main_sharedbundle";

#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate"]
fn game_reads_bundles_and_chunks() {
    let game = game();
    // the install chunks layout.toc names: the mods' own (initialinstallpackage) among them
    let pkg = game.package(MOD_INSTALL_CHUNK).expect("initialinstallpackage");
    assert!(pkg.ends_with("initialinstallpackage"), "{}", pkg.display());
    assert!(game.packages().len() >= 2);
    // the host deck's bundle, found by suffix as newdeck does
    let host = game.find_bundle("/deck_gen_graphic_00003_ap_cas_main_bundlereftable", ITEMS_TOC).unwrap().unwrap();
    let b = game.bundle(&host, ITEMS_TOC).unwrap();
    assert_eq!(b.files.len(), 1 + b.manifest.ebx.len() + b.manifest.res.len() + b.manifest.chunks.len());
    let ap = b.manifest.ebx.iter().position(|e| e.name.ends_with("deck_gen_graphic_00003_ap")).unwrap();
    let ebx = game.decode(&b.ebx_loc(ap).unwrap()).unwrap();
    assert_eq!(&ebx[..4], b"RIFF");
    assert_eq!(ebx.len() as u32, b.manifest.ebx[ap].size);
    // a texture resource header and its pixels (the chunk the header names, from the TOC)
    let tex = b.manifest.res.iter().position(|r| r.name.ends_with("deck_gen_graphic_00003_c")).unwrap();
    let header = game.decode(&b.res_loc(tex).unwrap()).unwrap();
    assert_eq!(header.len(), 180);
    let mut g = [0u8; 16];
    g.copy_from_slice(&header[40..56]);
    let guid = caveman_core::toc::Guid::from_le_bytes(g);
    let chunk = game.chunk(&guid, ITEMS_TOC).unwrap().expect("the texture's chunk");
    let pixels = game.decode(&chunk_loc(&chunk)).unwrap();
    let chunk_size = u32::from_le_bytes(header[116..120].try_into().unwrap());
    assert_eq!(pixels.len() as u32, chunk_size);
    assert_eq!(game.chunk_data(&guid, ITEMS_TOC).unwrap().len(), pixels.len());
    // the shared customization bundle: about 25,000 entries
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    assert!(shared.manifest.ebx.len() > 10_000);
    eprintln!(
        "host {} ({} ebx, {} res, {} chunks); shared bundle {} ebx; texture {} bytes",
        host,
        b.manifest.ebx.len(),
        b.manifest.res.len(),
        b.manifest.chunks.len(),
        shared.manifest.ebx.len(),
        pixels.len()
    );
}

#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and its Mods"]
fn layers_read_mod_files() {
    let game = game();
    let mods = std::path::Path::new(GAME_ROOT).join("Mods");
    let mut read = 0;
    for entry in std::fs::read_dir(&mods).unwrap().filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        let toc_file = entry.path().join("Win32").join("items.toc");
        if name.starts_with('.') || !toc_file.is_file() {
            continue;
        }
        let layer = Layer::open(&entry.path(), game.clone());
        let t = layer.toc("items.toc").unwrap();
        for b in t.bundles.iter().filter(|b| b.region.is_some()) {
            let (m, files) = layer.bundle(b.region.as_ref().unwrap()).unwrap();
            assert_eq!(files.len(), m.ebx.len() + m.res.len() + m.chunks.len(), "{name} {}", b.name);
            // every EBX the mod ships itself decodes to an EBX document
            for (i, f) in files.iter().enumerate().take(m.ebx.len()) {
                if f.patch {
                    let d = layer.decode(f).unwrap();
                    assert_eq!(&d[..4], b"RIFF", "{name} {}", m.ebx[i].name);
                    assert_eq!(d.len() as u32, m.ebx[i].size);
                    read += 1;
                }
            }
        }
    }
    eprintln!("decoded {read} EBX files from the installed mods' own archives");
}
