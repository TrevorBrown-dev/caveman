//! Builds the board model from the install into the scratch folder and checks its shape, for the stock board and
//! for an equipped loadout (ignored: reads the install).
//! `cargo test -p caveman-core --test board_model -- --ignored --nocapture`

use caveman_core::boardmodel::{self, Loadout, OwnWheel, LANES};
use caveman_core::game::Game;
use caveman_core::image::Image;
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

fn describe(model: &boardmodel::BoardModel) {
    println!("skeleton: {} bones", model.skeleton.names.len());
    for p in &model.parts {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for q in &p.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        println!(
            "  part {:<36} lane {:>2} {:<16} material {} ({}): {} vertices {} triangles",
            p.name,
            p.lane,
            LANES[p.lane],
            p.material,
            model.materials[p.material].role,
            p.positions.len(),
            p.indices.len() / 3
        );
    }
    for m in &model.materials {
        println!(
            "  material {} ({}): base {:?} normal {:?} orm {:?} from {:?}",
            m.name,
            m.role,
            m.base.as_ref().map(|i| (i.width, i.height)),
            m.normal.as_ref().map(|i| (i.width, i.height)),
            m.orm.as_ref().map(|i| (i.width, i.height)),
            m.sources
        );
    }
    for n in &model.notes {
        println!("  note: {n}");
    }
}

#[test]
#[ignore]
fn build_board_glb() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let model = boardmodel::read_board_model(&game).unwrap();
    describe(&model);
    println!("  {} triangle(s) across tiles", model.mixed_triangles);
    assert_eq!(model.mixed_triangles, 0);
    assert!(model.parts.iter().any(|p| model.materials[p.material].role == "graphic"), "no graphic part");
    // a wheel's vertices sit around its bone (the bind pose puts the bone at the wheel's centre)
    let wheel = model.parts.iter().find(|p| LANES[p.lane].contains("Wheel")).unwrap();
    let n = wheel.positions.len() as f32;
    let centre = wheel.positions.iter().fold([0f32; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
    println!("  a wheel's centre in its bone's space: {centre:?}");
    assert!(centre.iter().all(|c| c.abs() < 0.01), "the wheel is not around its bone: {centre:?}");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-test.glb");
    let facts = boardmodel::build(&game, &out).unwrap();
    println!("wrote {} ({} bytes): {facts}", out.display(), std::fs::metadata(&out).unwrap().len());
}

/// Trevor's loadout of 2026-10-30 (the profile's board recipe): The Caveman deck (not a game item: the graphic
/// falls back), black grip, no cutout, the checkered default trucks 00006, red wheels 00005.
#[test]
#[ignore]
fn build_equipped_glb() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let loadout = Loadout {
        deck: "Own_000_-_The_Caveman".into(),
        grip: "Own_DeckGripColor_Gen_Popsicle_00001".into(),
        cutout: "Own_DeckGripCutout_Gen_Popsicle_00001".into(),
        trucks: "Own_Truck_Gen_Default_00006".into(),
        wheels: "Own_WheelColor_Gen_Classic_00005".into(),
    };
    let model = boardmodel::read_board_model_for(&game, &loadout).unwrap();
    describe(&model);
    let trucks = model.materials.iter().find(|m| m.role == "trucks").unwrap();
    assert!(trucks.sources.iter().any(|s| s.contains("truck_gen_default_00006_c")), "{:?}", trucks.sources);
    let wheels = model.materials.iter().find(|m| m.role == "wheels").unwrap();
    let base = wheels.base.as_ref().unwrap();
    // red wheels: the base's mean is red
    let (mut r, mut g) = (0u64, 0u64);
    for px in base.data.chunks_exact(base.channels) {
        r += px[0] as u64;
        g += px[1] as u64;
    }
    assert!(r > g * 2, "the wheels are not red: r {r} g {g}");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-equipped.glb");
    let facts = boardmodel::build_for(&game, &loadout, &out).unwrap();
    println!("wrote {} ({} bytes): {facts}", out.display(), std::fs::metadata(&out).unwrap().len());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    if let Some(g) = model.materials.iter().find(|m| m.role == "grip").and_then(|m| m.base.as_ref()) {
        g.write_png(dir.join("grip_black_bolts_baked.png")).unwrap();
    }
    if let Some(g) = model.materials.iter().find(|m| m.role == "wheels").and_then(|m| m.base.as_ref()) {
        g.write_png(dir.join("wheels_red_baked.png")).unwrap();
    }
    // a stock graphic deck, a cutout with holes and a graphic wheel (the donut), to see those paths work
    let other = Loadout {
        deck: "Own_DeckGraphic_Real_Popsicle_00001".into(),
        cutout: "Own_DeckGripCutout_Gen_Popsicle_00010".into(),
        grip: "Own_DeckGripColor_Gen_Popsicle_00003".into(),
        wheels: "Own_WheelGraphic_Gen_Classic_00006".into(),
        ..loadout
    };
    let model = boardmodel::read_board_model_for(&game, &other).unwrap();
    for n in &model.notes {
        println!("  other: {n}");
    }
    if let Some(g) = model.materials.iter().find(|m| m.role == "grip").and_then(|m| m.base.as_ref()) {
        g.write_png(dir.join("grip_cutout_baked.png")).unwrap();
    }
    if let Some(g) = model.materials.iter().find(|m| m.role == "graphic").and_then(|m| m.base.as_ref()) {
        g.write_png(dir.join("graphic_real_00001.png")).unwrap();
    }
    if let Some(g) = model.materials.iter().find(|m| m.role == "wheels").and_then(|m| m.base.as_ref()) {
        g.write_png(dir.join("wheels_donut_baked.png")).unwrap();
    }
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-other.glb");
    boardmodel::write_glb(&model, &out).unwrap();
}

/// One of the player's own wheels on the stock board (the Studio's preview): a yellow disc on a transparent face, in
/// blue. The urethane takes the blue as its region colour, and each wheel's outer face shows the disc.
#[test]
#[ignore]
fn build_own_wheel_glb() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let mut face = Image::filled(256, 256, &[0, 0, 0, 0]);
    for y in 0..256 {
        for x in 0..256 {
            let (dx, dy) = (x as f32 - 127.5, y as f32 - 127.5);
            if dx * dx + dy * dy < 80.0 * 80.0 {
                face.data[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4].copy_from_slice(&[255, 220, 0, 255]);
            }
        }
    }
    let own = OwnWheel { face, colour: [0, 110, 255] };
    let model = boardmodel::read_board_model_with(&game, &Loadout::stock(), Some(&own)).unwrap();
    describe(&model);
    assert!(model.notes.iter().any(|n| n.contains("graphic the wheel's own face") && n.contains("over 4 wheel(s)")), "{:?}", model.notes);
    let wheels = model.materials.iter().find(|m| m.role == "wheels").unwrap();
    let base = wheels.base.as_ref().unwrap();
    let (mut r, mut b) = (0u64, 0u64);
    for px in base.data.chunks_exact(base.channels) {
        r += px[0] as u64;
        b += px[2] as u64;
    }
    assert!(b > r * 2, "the wheels are not blue: r {r} b {b}");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-textures");
    for m in model.materials.iter().filter(|m| m.name.starts_with("Wheel_Mat:")) {
        m.base.as_ref().unwrap().write_png(dir.join(format!("own_{}.png", m.name.replace(':', "_")))).unwrap();
    }
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/board-own-wheel.glb");
    let facts = boardmodel::build_with(&game, &Loadout::stock(), Some(&own), &out).unwrap();
    println!("wrote {} ({} bytes): {facts}", out.display(), std::fs::metadata(&out).unwrap().len());
}
