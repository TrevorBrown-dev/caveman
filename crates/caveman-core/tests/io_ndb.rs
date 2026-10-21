//! The native DB against the Python toolkit (ndb.py): every bundle's chunk metadata and every layout.toc round trip.
mod io_common;

use caveman_core::ndb::{self, types, Node};
use caveman_core::{bundle, toc};
use io_common::*;

#[test]
fn small_tree_round_trip() {
    let mut root = Node::named(types::OBJECT, "root", Vec::new());
    root.children.push(Node::named(types::I32, "firstMip", 3i32.to_le_bytes().to_vec()));
    root.children.push(Node::named(types::STRING, "name", b"abc\0".to_vec()));
    let mut list = Node::named(types::LIST, "list", Vec::new());
    list.terminated = true;
    let mut item = Node::named(types::I64, "", 7i64.to_le_bytes().to_vec());
    item.named = false;
    list.children.push(item);
    root.children.push(list);
    let raw = ndb::write(&root);
    let (back, end) = ndb::read(&raw, 0).unwrap();
    assert_eq!(end, raw.len());
    assert_eq!(back, root);
    assert_eq!(back.field("firstMip").unwrap().as_i32(), Some(3));
    assert_eq!(back.field("name").unwrap().text(), "abc");
}

/// Every bundle's chunk metadata (game and mods) reads and writes back to the same bytes.
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and the Python fixtures"]
fn every_chunk_meta_round_trips() {
    let rows = rows("manifests.tsv");
    let game = game();
    let mut layers = Layers::default();
    let mut tocs = std::collections::HashMap::new();
    let (mut with_meta, mut ok) = (0, 0);
    for r in &rows {
        let (kind, m, rel, name) = (&r[0], &r[1], &r[2], &r[3]);
        let key = (kind.clone(), m.clone(), rel.clone());
        let t: &toc::Toc = tocs.entry(key).or_insert_with(|| toc::read_file(&toc_path(kind, m, rel)).unwrap());
        let region = t.bundles.iter().find(|b| &b.name == name).unwrap().region.clone().unwrap();
        let (inline, files) = bundle::read_region(&region).unwrap();
        let raw = match inline {
            Some(x) => x,
            None if kind == "game" => game.raw(&files[0]).unwrap(),
            None => layers.get(m).raw(&files[0]).unwrap(),
        };
        let man = caveman_core::game::read_manifest_any(&raw, game.root()).unwrap();
        if man.chunk_meta.is_empty() {
            continue;
        }
        with_meta += 1;
        let (node, end) = ndb::read(&man.chunk_meta, 0).unwrap_or_else(|e| panic!("{kind} {m} {name}: {e}"));
        let same = end == man.chunk_meta.len() && ndb::write(&node) == man.chunk_meta;
        assert_eq!(same as u8, r[7].parse::<u8>().unwrap(), "{kind} {m} {name}");
        ok += same as usize;
    }
    eprintln!("chunk metadata round trips: {ok}/{with_meta}");
}

/// The game's and the mods' layout.toc files read from the envelope on and write back to the same bytes.
#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate and the Python fixtures"]
fn every_layout_round_trips() {
    let rows = rows("layouts.tsv");
    let mut ok = 0;
    for r in &rows {
        let d = std::fs::read(&r[0]).unwrap();
        let (node, end) = ndb::read(&d, toc::ENVELOPE).unwrap_or_else(|e| panic!("{}: {e}", r[0]));
        assert_eq!(end.to_string(), r[1], "{}", r[0]);
        let same = ndb::write(&node) == d[toc::ENVELOPE..end];
        assert_eq!(same as u8, r[3].parse::<u8>().unwrap(), "{}", r[0]);
        ok += same as usize;
    }
    eprintln!("layout.toc round trips: {ok}/{}", rows.len());
}
