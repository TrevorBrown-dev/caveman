//! Writes the stock gold deck's textures (the metal host's: its art, its own mask, and the default normal map) as
//! PNGs, to put the stock gold look on The Caveman live and compare it with the game's own in the same lighting.
//! Needs the game: `cargo test --release -p caveman-core --test dump_gold -- --ignored --nocapture`.
//! Output: %TEMP%\caveman-gold\{c,osk,ny}.png (the folder is printed).

use std::path::{Path, PathBuf};

use caveman_core::build::{texture_image, Host};
use caveman_core::game::{Game, ITEMS_TOC};

#[test]
#[ignore]
fn dump_gold_deck_layers() {
    let game = Game::open(Path::new(r"E:\Games\ReSkate")).unwrap();
    let out = PathBuf::from(std::env::var("TEMP").unwrap()).join("caveman-gold");
    std::fs::create_dir_all(&out).unwrap();
    let host = Host::Metal;
    let b = game.bundle(&host.bundle(), ITEMS_TOC).unwrap();
    let stock = host.stock();
    let defaults = "characters/skateboard/defaults/deck/common/generic/graphic/deck_gen_graphic_default";
    for (slot, name) in [("c", format!("{stock}_c")), ("osk", format!("{stock}_osk")), ("ny", format!("{defaults}_ny"))] {
        let img = texture_image(&game, &b, &name).unwrap();
        let path = out.join(format!("{slot}.png"));
        img.write_png(&path).unwrap();
        println!("{slot}: {} {}x{} x{} -> {}", name, img.width, img.height, img.channels, path.display());
    }
    println!("folder: {}", out.display());
}
