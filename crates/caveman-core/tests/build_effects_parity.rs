//! Dumps the stages of the deck effects for one art (fit as caveman.py does) so a Python script
//! (`scratchpad/buildport/effects_parity.py`) can compare them with mixins.py stage by stage. Needs the game; run with
//! `--ignored`.

use std::path::{Path, PathBuf};

use caveman_core::build;
use caveman_core::effects::{self, Finish};
use caveman_core::game::Game;
use caveman_core::image::Image;

fn scratch() -> PathBuf {
    PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\buildport")
}

fn f64s(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[test]
#[ignore]
fn dump_effect_stages() {
    let game = Game::open(Path::new(r"E:\Games\ReSkate")).unwrap();
    let wood = build::natural_wood(&game).unwrap();
    let png = scratch().join("decks").join("000").join("Dan Da Dan.png");
    let (art, _) = build::fit_art(&Image::read_png(&png).unwrap(), &wood).unwrap();
    let (dmask, dnormal) = build::default_layers(&game).unwrap();
    let out = scratch().join("parity");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("art.rgb"), &art.data).unwrap();
    let l = effects::luma(&art);
    std::fs::write(out.join("luma.f64"), f64s(&l)).unwrap();
    let height = Image { width: 512, height: 2048, channels: 1, data: l.iter().map(|v| (v * 255.0) as u8).collect() };
    std::fs::write(out.join("height.u8"), &height.data).unwrap();
    std::fs::write(out.join("blurred.u8"), &height.gaussian_blur(2.0).data).unwrap();
    let n = effects::normal_map(Some(effects::EMBOSS_STRENGTH), false, &art, &dnormal, None).unwrap();
    std::fs::write(out.join("normal.rgb"), &n.data).unwrap();
    let s = effects::spot(&art);
    std::fs::write(out.join("spot.f64"), f64s(&s)).unwrap();
    std::fs::write(out.join("mask_foil_spot.rgb"), &effects::mask(Finish::Foil, &dmask, Some(&s)).data).unwrap();
    std::fs::write(out.join("default_normal.rgb"), &dnormal.to_rgb().data).unwrap();
    println!("stages written to {}", out.display());
}
