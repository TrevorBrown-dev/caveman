//! The name-clash scan (items::existing_items) against Python's caveman.existing_items: dumps the taken set for
//! scratchpad/cli/compare_items.py. Needs the game (read only); run with `--ignored`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use caveman_cli::items::existing_items;
use caveman_cli::report::Reporter;
use caveman_core::game::Game;

fn scratch() -> PathBuf {
    std::env::var_os("CAVEMAN_SCRATCH").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad")
    })
}

#[test]
#[ignore]
fn taken_items_dump() {
    let game = Arc::new(Game::open(Path::new(r"E:\Games\ReSkate")).unwrap());
    let mut report = Reporter::to(Box::new(std::io::sink()), false);
    let taken = existing_items(&game, Path::new(r"E:\Games\ReSkate\Mods"), "Caveman-Decks", &mut report).unwrap();
    let lines: Vec<String> = taken.iter().map(|(k, v)| format!("{k}\t{v}")).collect();
    let out = scratch().join("cli").join("taken-rust.tsv");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, lines.join("\n")).unwrap();
    println!("{} taken item ids -> {}", taken.len(), out.display());
    assert!(taken.len() > 1000);
}
