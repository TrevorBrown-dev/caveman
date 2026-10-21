//! Bundle regions and manifests against the Python toolkit (fb.read_bundle_region, fb.read_binary_bundle,
//! fbw.write_bundle_region, fbw.write_binary_bundle), over every bundle of the game and the installed mods.
mod io_common;

use std::collections::HashMap;

use caveman_core::bundle::{self, FileLoc};
use caveman_core::game::read_manifest_any;
use caveman_core::toc;
use io_common::*;

#[test]
fn region_matches_python_small() {
    let files = [
        FileLoc { patch: true, install: 3162564249, archive: 1, offset: 0, size: 10 },
        FileLoc { patch: true, install: 3162564249, archive: 1, offset: 10, size: 5 },
        FileLoc { patch: false, install: 3162564249, archive: 8, offset: 100, size: 7 },
    ];
    let r = bundle::write_region(&files);
    assert_eq!(
        hex(&r),
        "00000000000000000000004c0000000300000024000000240000002400000000000000030001bc80e6990001000000000000000a0000000a000000050000bc80e69900080000006400000007840080"
    );
    let (inline, back) = bundle::read_region(&r).unwrap();
    assert!(inline.is_none());
    assert_eq!(back, files);
}

#[test]
fn manifest_write_read_small() {
    let m = bundle::Manifest {
        endian: bundle::Endian::Little,
        ebx: vec![bundle::ManifestEbx { name: "a/b".into(), size: 10, sha1: [1; 20] }],
        res: vec![bundle::ManifestRes { name: "a/t".into(), size: 180, sha1: [2; 20], rtype: 0x6BDE20BA, meta: [3; 16], rid: 0x1235 }],
        chunks: vec![bundle::ManifestChunk {
            guid: "a82b7e84-ed0f-54b3-eca0-250126b2aeca".parse().unwrap(),
            logical_offset: 0,
            logical_size: 99,
            sha1: [4; 20],
        }],
        chunk_meta: vec![2, 0x00],
    };
    let raw = bundle::write_manifest(&m).unwrap();
    assert_eq!(bundle::read_manifest(&raw).unwrap(), m);
}

/// Every bundle manifest of the game and the mods: parsed as Python parses it, written back to the same bytes, and its
/// region rewritten exactly when Python's is.
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and the Python fixtures"]
fn every_manifest_matches_python() {
    let rows = rows("manifests.tsv");
    let game = game();
    let mut layers = Layers::default();
    let mut tocs: HashMap<(String, String, String), toc::Toc> = HashMap::new();
    let (mut parsed, mut rt, mut region_rt, mut region_rt_python) = (0, 0, 0, 0);
    for r in &rows {
        let (kind, m, rel, name) = (&r[0], &r[1], &r[2], &r[3]);
        assert_ne!(r[4], "error", "Python could not read {name}");
        let key = (kind.clone(), m.clone(), rel.clone());
        if !tocs.contains_key(&key) {
            tocs.insert(key.clone(), toc::read_file(&toc_path(kind, m, rel)).unwrap());
        }
        let t = &tocs[&key];
        let b = t.bundles.iter().find(|b| &b.name == name).unwrap();
        let region = b.region.as_ref().unwrap();
        let (inline, files) = bundle::read_region(region).unwrap();
        let raw = match inline {
            Some(x) => x,
            None if kind == "game" => game.raw(&files[0]).unwrap(),
            None => layers.get(m).raw(&files[0]).unwrap(),
        };
        let man = read_manifest_any(&raw, game.root()).unwrap_or_else(|e| panic!("{kind} {m} {name}: {e}"));
        assert_eq!(manifest_summary(&man), r[4], "{kind} {m} {name}: manifest differs from Python's");
        parsed += 1;
        // the bytes the manifest was parsed from (decoded when the file was CAS-encoded)
        let plain = if r[5] == "1" { caveman_core::cas::decode_bytes(&raw, game.root()).unwrap() } else { raw };
        let w = bundle::write_manifest(&man).unwrap();
        assert_eq!((w == plain) as u8, r[6].parse::<u8>().unwrap(), "{kind} {m} {name}: manifest round trip");
        if w == plain {
            rt += 1;
        }
        let rw = bundle::write_region(&files) == *region;
        assert_eq!(rw as u8, r[8].parse::<u8>().unwrap(), "{kind} {m} {name}: region round trip");
        region_rt += rw as usize;
        region_rt_python += (r[8] == "1") as usize;
    }
    eprintln!(
        "manifests parsed as Python: {parsed}/{n}; written back identically: {rt}/{n}; regions rewritten identically: {region_rt}/{n} (Python: {region_rt_python})",
        n = rows.len()
    );
}
