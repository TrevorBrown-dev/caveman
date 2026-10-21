//! The shader mod the CLI builds (Caveman-Shaders) is shader::build_shader_mod's output with the CLI's options:
//! builds it directly into the scratchpad for scratchpad/cli/compare_shaders.py (which also compares Python's
//! shadermod.build_mod). Needs the game (read only); run with `--ignored`.

use std::path::{Path, PathBuf};

use caveman_core::game::Game;
use caveman_core::shader::{build_shader_mod, ShaderModOptions};

fn scratch() -> PathBuf {
    std::env::var_os("CAVEMAN_SCRATCH").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad")
    })
}

#[test]
#[ignore]
fn shader_mod_direct() {
    let game = Game::open(Path::new(r"E:\Games\ReSkate")).unwrap();
    let dir = scratch().join("cli").join("shaders-direct").join(caveman_cli::SHADER_MOD);
    let stats = build_shader_mod(&game, &dir, &ShaderModOptions { title: "Caveman Shaders".into(), built: None, animated: false }).unwrap();
    println!("direct shader mod: {} programs, cas {} -> {}", stats.programs.len(), stats.cas, dir.display());
    // the six deck programs, the ribbon's pixel and vertex programs (globals.toc), the banner's spawn and simulate
    // programs (items.toc)
    assert_eq!(stats.programs.len(), 10);
}
