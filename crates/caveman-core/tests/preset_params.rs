//! Prints the boxed parameters of the three host decks' appearance presets (ParamKey GUID and value), to find the
//! keys of `param_graphic_metalmaskvalue` (1.0 on the metal host), `param_glass_iridescenceamount` and the like.
//! Needs the game: `cargo test --release -p caveman-core --test preset_params -- --ignored --nocapture`.

use std::path::Path;

use caveman_core::build::{preset_params, Host};
use caveman_core::game::Game;

#[test]
#[ignore]
fn dump_host_preset_params() {
    let game = Game::open(Path::new(r"E:\Games\ReSkate")).unwrap();
    let mut all: Vec<(String, Vec<(Host, Vec<u8>)>)> = Vec::new();
    for host in [Host::Plain, Host::Metal, Host::Holo] {
        for (k, raw) in preset_params(&game, host).unwrap() {
            match all.iter_mut().find(|(key, _)| *key == k) {
                Some((_, v)) => v.push((host, raw)),
                None => all.push((k, vec![(host, raw)])),
            }
        }
    }
    // the two stock holo decks with the strong iridescence (1.0 per knowledge/cosmetics.md), to tell the amount's key
    // from the holo host's other 0.25
    for (tag, stock) in [
        ("CarpetCo", "characters/skateboard/licensed/deck/common/carpetco/graphic/deck_carpetco_graphic_00005/2026/deck_carpetco_graphic_00005"),
        ("Deathwish", "characters/skateboard/licensed/deck/common/deathwish/graphic/deck_deathwish_graphic_00013/2026/deck_deathwish_graphic_00013"),
    ] {
        match caveman_core::build::preset_params_of(&game, stock) {
            Ok(params) => {
                for (k, raw) in params {
                    let shown = if raw.len() % 4 == 0 && !raw.is_empty() && raw.len() <= 16 {
                        raw.chunks(4).map(|c| format!("{}", f32::from_le_bytes(c.try_into().unwrap()))).collect::<Vec<_>>().join(", ")
                    } else {
                        format!("{} bytes", raw.len())
                    };
                    println!("{k}  {tag}=[{shown}]");
                }
            }
            Err(e) => println!("{tag}: {e}"),
        }
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, values) in &all {
        let show = |raw: &[u8]| -> String {
            if raw.len().is_multiple_of(4) && !raw.is_empty() && raw.len() <= 16 {
                raw.chunks(4).map(|c| format!("{}", f32::from_le_bytes(c.try_into().unwrap()))).collect::<Vec<_>>().join(", ")
            } else {
                format!("{} bytes {:02x?}", raw.len(), &raw[..raw.len().min(8)])
            }
        };
        let per: Vec<String> = values.iter().map(|(h, raw)| format!("{h:?}=[{}]", show(raw))).collect();
        println!("{k}  {}", per.join("  "));
    }
}
