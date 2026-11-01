//! Shared helpers for the io port's tests: the Python reference digests (written by the session's
//! `scratchpad/io/fixtures.py` from the Python toolkit) and the same summaries computed from the Rust readers.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use caveman_core::bundle::{Endian, Manifest};
use caveman_core::game::{Game, Layer};
use caveman_core::toc::Toc;

pub const GAME_ROOT: &str = r"E:\Games\ReSkate";

/// Where the Python reference digests are (`CAVEMAN_IO_FIXTURES`, else the session scratchpad).
pub fn fixtures() -> PathBuf {
    std::env::var_os("CAVEMAN_IO_FIXTURES").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\io")
    })
}

/// The rows of a fixture file, split on tabs.
pub fn rows(name: &str) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(fixtures().join(name)).unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    text.lines().filter(|l| !l.is_empty()).map(|l| l.split('\t').map(str::to_string).collect()).collect()
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sha(b: &[u8]) -> String {
    hex(&sha1_smol::Sha1::from(b).digest().bytes())
}

/// `fixtures.py`'s `toc_summary`.
pub fn toc_summary(t: &Toc) -> String {
    let mut lines = Vec::new();
    for b in &t.bundles {
        let region = b.region.as_ref().map(|r| sha(r)).unwrap_or_else(|| "-".into());
        lines.push(format!("B|{}|{}|{}|{}|{}", b.name, b.load, b.size, b.region_off, region));
    }
    for c in &t.chunks {
        if c.removed {
            lines.push(format!("C|{}|removed", c.guid));
        } else {
            lines.push(format!("C|{}|{}|{}|{}|{}|{}", c.guid, c.patch as u8, c.install, c.archive, c.offset, c.size));
        }
    }
    sha(lines.join("\n").as_bytes())
}

/// `fixtures.py`'s `manifest_summary`.
pub fn manifest_summary(m: &Manifest) -> String {
    let mut lines = vec![match m.endian {
        Endian::Big => ">".to_string(),
        Endian::Little => "<".to_string(),
    }];
    for e in &m.ebx {
        lines.push(format!("E|{}|{}|{}", e.name, e.size, hex(&e.sha1)));
    }
    for r in &m.res {
        lines.push(format!("R|{}|{}|{}|{}|{}|{}", r.name, r.size, hex(&r.sha1), r.rtype, hex(&r.meta), r.rid));
    }
    for c in &m.chunks {
        lines.push(format!("K|{}|{}|{}|{}", c.guid, c.logical_offset, c.logical_size, hex(&c.sha1)));
    }
    lines.push(format!("M|{}", sha(&m.chunk_meta)));
    sha(lines.join("\n").as_bytes())
}

/// The game, opened once per test binary.
pub fn game() -> Arc<Game> {
    static GAME: std::sync::OnceLock<Arc<Game>> = std::sync::OnceLock::new();
    GAME.get_or_init(|| Arc::new(Game::open(std::path::Path::new(GAME_ROOT)).expect("open the game"))).clone()
}

/// Mod layers by folder name, opened on demand.
#[derive(Default)]
pub struct Layers(HashMap<String, Layer>);

impl Layers {
    pub fn get(&mut self, m: &str) -> &Layer {
        self.0.entry(m.to_string()).or_insert_with(|| Layer::open(&std::path::Path::new(GAME_ROOT).join("Mods").join(m), game()))
    }
}

/// A TOC file of the game (`kind` "game") or of a mod folder.
pub fn toc_path(kind: &str, m: &str, rel: &str) -> PathBuf {
    let base = if kind == "game" { PathBuf::from(GAME_ROOT).join("Data").join("Win32") } else { PathBuf::from(GAME_ROOT).join("Mods").join(m).join("Win32") };
    rel.split('/').fold(base, |p, part| p.join(part))
}
