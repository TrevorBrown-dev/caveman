//! The boxed parameters of the equipped grip colour, wheel colour and truck presets (ParamKey GUID and value), and
//! the textures and EBX assets in their bundles, to see how their colours are carried (ignored: reads the install).
//! `cargo test -p caveman-core --test equipped_presets -- --ignored --nocapture`

use caveman_core::build::preset_params_of;
use caveman_core::game::{Game, ITEMS_TOC};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";
const TEXTURE_TYPE: u32 = 0x6bde_20ba;

#[test]
#[ignore]
fn equipped_preset_params() {
    let game = Game::open(Path::new(GAME)).unwrap();
    for stock in [
        "characters/skateboard/unlicensed/deck/common/generic/gripcolor/presets/deck_gen_popsiclegripcolor_00001",
        "characters/skateboard/unlicensed/wheel/generic/classic/wheelcolor/presets/wheel_gen_classicwheelcolor_00005",
        "characters/skateboard/unlicensed/wheel/generic/classic/wheelcolor/presets/wheel_gen_classicwheelcolor_00001",
        "characters/skateboard/unlicensed/truck/generic/default/2022/truck_gen_default_00006",
        "characters/skateboard/unlicensed/truck/generic/default/2022/truck_gen_default_00001",
    ] {
        println!("\n== {stock}");
        match preset_params_of(&game, stock) {
            Ok(params) => {
                for (k, raw) in params {
                    let floats: Vec<String> = raw.chunks_exact(4).take(4).map(|b| format!("{:.4}", f32::from_le_bytes(b.try_into().unwrap()))).collect();
                    println!("  {k}: {} bytes {:02x?} as floats {}", raw.len(), &raw[..raw.len().min(16)], floats.join(" "));
                }
            }
            Err(e) => println!("  no preset params: {e}"),
        }
        match game.bundle(&format!("win32/{stock}_ap_cas_main_bundlereftable"), ITEMS_TOC) {
            Ok(b) => {
                for r in b.manifest.res.iter().filter(|r| r.rtype == TEXTURE_TYPE) {
                    println!("  texture {}", r.name);
                }
                for e in &b.manifest.ebx {
                    println!("  ebx {}", e.name);
                }
            }
            Err(e) => println!("  no bundle: {e}"),
        }
    }
}
