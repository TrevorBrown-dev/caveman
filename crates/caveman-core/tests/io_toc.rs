//! TOC reader and patch-TOC writer against the Python toolkit (fb.read_toc, fbw.write_patch_toc, fbw.perfect_hash).
mod io_common;

use caveman_core::toc::{self, perfect_hash, toc_hash, Guid, PatchBundle, PatchChunk, FNV_OFFSET};
use io_common::*;

#[test]
fn toc_hash_matches_python() {
    assert_eq!(toc_hash(b"", FNV_OFFSET), 1823410);
    assert_eq!(toc_hash(b"abc", FNV_OFFSET), 10208722);
    assert_eq!(toc_hash(&[0xff, 0x80, 1], FNV_OFFSET), 11715025);
    assert_eq!(toc_hash(b"win32/items", 7), 10561615);
}

#[test]
fn perfect_hash_small_matches_python() {
    let keys: Vec<Vec<u8>> = [&b"a"[..], b"b", b"c", b"dd", b"ee", b"win32/x"].iter().map(|k| k.to_vec()).collect();
    let (mapping, order) = perfect_hash(&keys).unwrap();
    assert_eq!(mapping, vec![-1, 1, -1, -1, 9, -5]);
    assert_eq!(order, vec![1, 3, 4, 5, 2, 0]);
    assert!(perfect_hash(&[b"x".to_vec(), b"x".to_vec()]).is_err());
}

#[test]
fn guid_text_round_trip() {
    let g: Guid = "2259d419-387f-b1ba-30fc-c5140100b012".parse().unwrap();
    assert_eq!(g.to_string(), "2259d419-387f-b1ba-30fc-c5140100b012");
    // uuid.UUID('2259d419-387f-b1ba-30fc-c5140100b012').bytes_le
    assert_eq!(&g.le_bytes()[..4], &[0x19, 0xd4, 0x59, 0x22]);
    assert_eq!(Guid::from_be_bytes(g.be_bytes()), g);
}

#[test]
fn write_then_read_small_toc() {
    let bundles =
        vec![PatchBundle { name: "win32/a/B".into(), region: vec![1, 2, 3], load: 1 }, PatchBundle { name: "win32/c".into(), region: vec![9; 10], load: 1 }];
    let chunks = vec![PatchChunk {
        guid: "a82b7e84-ed0f-54b3-eca0-250126b2aeca".parse().unwrap(),
        patch: true,
        install: 3162564249,
        archive: 1,
        offset: 64,
        size: 1000,
    }];
    let raw = toc::write_patch_toc(&bundles, &chunks, 3).unwrap();
    let t = toc::read(&raw).unwrap();
    assert_eq!(t.flags, 3);
    let (b2, c2) = t.to_patch_input();
    let mut b2s = b2.clone();
    b2s.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(b2s, bundles);
    assert_eq!(c2, chunks);
    // written from what it read, the same bytes
    assert_eq!(toc::write_patch_toc(&b2, &c2, 3).unwrap(), raw);
}

/// Every TOC of the game and the installed mods parses to what fb.read_toc gives.
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and the Python fixtures"]
fn every_toc_matches_python() {
    let rows = rows("tocs.tsv");
    let mut ok = 0;
    for r in &rows {
        let (kind, m, rel) = (&r[0], &r[1], &r[2]);
        let t = toc::read_file(&toc_path(kind, m, rel)).unwrap_or_else(|e| panic!("{kind} {m} {rel}: {e}"));
        assert_eq!(t.flags.to_string(), r[3], "{rel}: flags");
        assert_eq!(t.bundles.len().to_string(), r[4], "{rel}: bundles");
        assert_eq!(t.chunks.len().to_string(), r[5], "{rel}: chunks");
        assert_eq!(toc_summary(&t), r[6], "{kind} {m} {rel}: summary");
        ok += 1;
    }
    eprintln!("TOCs parsed identically to Python: {ok}/{}", rows.len());
}

/// Every mod TOC written back with write_patch_toc: the same bytes Python writes (and, for 118 of 119, the
/// original file).
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and the Python fixtures"]
fn mod_tocs_rewrite_like_python() {
    let rows = rows("rewrite.tsv");
    let (mut same_as_python, mut same_as_file) = (0, 0);
    for r in &rows {
        let raw = std::fs::read(toc_path("mod", &r[0], &r[1])).unwrap();
        assert_eq!(sha(&raw), r[2]);
        let t = toc::read(&raw).unwrap();
        let (b, c) = t.to_patch_input();
        let w = toc::write_patch_toc(&b, &c, t.flags).unwrap();
        assert_eq!(sha(&w), r[3], "{} {}: differs from Python's write", r[0], r[1]);
        same_as_python += 1;
        if w == raw {
            same_as_file += 1;
        }
        assert_eq!((w == raw) as u8, r[4].parse::<u8>().unwrap(), "{} {}", r[0], r[1]);
    }
    eprintln!("mod TOCs: written as Python writes {same_as_python}/{}, identical to the file {same_as_file}/{}", rows.len(), rows.len());
}

/// Perfect hashes over items.toc's real keys, against fbw.perfect_hash.
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and the Python fixtures"]
fn perfect_hash_matches_python() {
    let t = toc::read_file(&toc_path("game", "", "items.toc")).unwrap();
    for r in rows("perfect_hash.tsv") {
        let keys: Vec<Vec<u8>> = match r[0].as_str() {
            "bundles" => t.bundles.iter().map(|b| b.name.to_lowercase().into_bytes()).collect(),
            "chunks" => t.chunks.iter().map(|c| c.guid.le_bytes().to_vec()).collect(),
            "bundles100" => t.bundles.iter().take(100).map(|b| b.name.to_lowercase().into_bytes()).collect(),
            _ => vec![b"x".to_vec()],
        };
        assert_eq!(keys.len().to_string(), r[1]);
        let (mapping, order) = perfect_hash(&keys).unwrap();
        let text = format!(
            "{}|{}",
            mapping.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
            order.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")
        );
        assert_eq!(sha(text.as_bytes()), r[2], "{}", r[0]);
        eprintln!("perfect hash {} ({} keys): identical", r[0], r[1]);
    }
}
