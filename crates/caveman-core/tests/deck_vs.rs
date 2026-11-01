//! The deck material's vertex programs: found through the permutations that name the six deck pixel programs, then
//! one of them disassembled, to read how the vertex buffer's tangent-space element (usage 52, four bytes) is decoded.
//! Ignored: reads the install and uses the game's dxcompiler.dll.
//! `cargo test -p caveman-core --test deck_vs -- --ignored --nocapture`

use caveman_core::game::Game;
use caveman_core::shader::dxc::Dxc;
use caveman_core::shader::{program, CONFIG_BUNDLE, DECK_PROGRAMS, GLOBALS_TOC};
use caveman_core::toc::Guid;
use std::collections::BTreeMap;
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";
const PERMUTATION_TYPE: u32 = 0xD489_FACB;

#[test]
#[ignore]
fn deck_vertex_programs() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let b = game.bundle(CONFIG_BUNDLE, GLOBALS_TOC).unwrap();
    // every permutation whose pixel program (+0x30) is a deck program: its vertex program (+0x20)
    let mut vertex_programs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (i, r) in b.manifest.res.iter().enumerate() {
        if r.rtype != PERMUTATION_TYPE {
            continue;
        }
        let Some(loc) = b.res_loc(i) else { continue };
        let Ok(data) = game.decode(&loc) else { continue };
        if data.len() < 0x40 {
            continue;
        }
        let pixel = Guid::from_le_bytes(data[0x30..0x40].try_into().unwrap()).to_string();
        if DECK_PROGRAMS.contains(&pixel.as_str()) {
            let vertex = Guid::from_le_bytes(data[0x20..0x30].try_into().unwrap()).to_string();
            vertex_programs.entry(vertex).or_default().push(format!("{} -> {pixel}", r.name));
        }
    }
    println!("{} vertex program(s) draw the deck:", vertex_programs.len());
    for (v, users) in &vertex_programs {
        println!("  {v}: {} permutation(s), e.g. {}", users.len(), users[0]);
    }
    let dxc = Dxc::open(Path::new(GAME)).unwrap();
    for (k, v) in vertex_programs.keys().enumerate() {
        let name = format!("shaders/bytecode/{v}");
        let Some(i) = b.manifest.res.iter().rposition(|r| r.name == name) else {
            println!("  {name}: not in the bundle");
            continue;
        };
        let raw = game.decode(&b.res_loc(i).unwrap()).unwrap();
        let res = program::read_bytecode_resource(&raw).expect("a bytecode resource");
        let text = dxc.disassemble(res.dxbc).unwrap();
        let out =
            format!("C:/Users/thetr/AppData/Local/Temp/claude/C--Users-thetr-Projects-ReSkate/c6d3c964-296a-4f0b-bedc-241e3cf960f6/scratchpad/deck_vs_{k}.ll");
        std::fs::write(&out, &text).unwrap();
        println!("\n== {name}: {} lines of IR written to {out}", text.lines().count());
        // the input signature and the lines that read the inputs
        let mut in_sig = false;
        for line in text.lines() {
            if line.starts_with("; Input signature") {
                in_sig = true;
            }
            if in_sig {
                println!("{line}");
                if line.trim().is_empty() && !line.starts_with(';') {
                    in_sig = false;
                }
                if line.starts_with("; Output signature") {
                    break;
                }
            }
        }
        for line in text.lines().filter(|l| l.contains("dx.op.loadInput")).take(40) {
            println!("{line}");
        }
        if k == 0 {
            break;
        }
    }
}
