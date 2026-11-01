//! Shader programs (`dxc.py`, `shaderpatch.py`, `nyanpatch.py`, `shadermod.py`): DXC through the game's own
//! `dxcompiler.dll` and `dxil.dll`, Caveman's patches of the six deck pixel programs (marker-gated effects and glow;
//! see knowledge/shaders.md and glow.md) and of the ribbon program (the solid rainbow trail; knowledge/trails.md),
//! and the shader mod that carries them. The programs are always built on the player's PC from their own game.

pub mod deck;
pub mod dxc;
pub mod ir;
pub mod program;
pub mod ribbon;
pub mod ribbon_banner;
pub mod ribbon_vs;
pub mod shadermod;

use std::fmt;
use std::path::PathBuf;

pub use deck::patch as patch_deck;
pub use dxc::{container_parts, Dxc, DxcOutput};
pub use program::{read_bytecode_resource, wrap, BytecodeResource};
pub use ribbon::{patch as patch_ribbon, RIBBON_PROGRAM};
pub use shadermod::{build_shader_mod, BuiltProgram, ShaderModOptions, ShaderModStats};

use crate::game::{Bundle, Game, GameError};

/// `shaderpatch.GLOBALS_TOC`: the superbundle holding the global shader database and programs.
pub const GLOBALS_TOC: &str = "globals.toc";
/// `shaderpatch.CONFIG_BUNDLE`: the bundle of the global shader database and programs.
pub const CONFIG_BUNDLE: &str = "win32/configurations/gameconfigurations/delmargameconfiguration";
/// `shaderpatch.DECK_PROGRAMS`: the deck material's six G-buffer pixel programs (variants A and B in pairs).
pub const DECK_PROGRAMS: [&str; 6] = [
    "2a8dba87-9d32-cb86-fb2c-f3b9ee93f6f8",
    "c125a7ae-b71c-5a21-3c8a-a62e86e68fe7",
    "1b6ae129-33e2-1ee7-bc84-d45555a76771",
    "4daed1c1-2a79-452d-548c-6f019ec3b59d",
    "a54c586c-3f98-f214-ab2b-32390d262d24",
    "a82b7e84-ed0f-54b3-eca0-250126b2aeca",
];
/// `shaderpatch.BYTECODE_TYPE`: the resource type of `shaders/bytecode/<guid>`.
pub const BYTECODE_TYPE: u32 = 0x0E2B_02F7;

/// The marker's texture size and block size (`shaderpatch.ART_W`, `ART_H`, `BLOCK`): three 16 x 16 blocks in the
/// top-left corner of the deck's 512 x 2048 holo map.
pub const MARKER_W: u32 = 512;
pub const MARKER_H: u32 = 2048;
pub const BLOCK: u32 = 16;
/// `shaderpatch.MARK_A`, `MARK_B`: block 0 magenta, block 1 green.
pub const MARK_A: (u8, u8, u8) = (255, 0, 255);
pub const MARK_B: (u8, u8, u8) = (0, 255, 0);
/// The effect codes of block 2's red (`shaderpatch.EFFECT_*`): none (glow only), hue cycle, a fixed half turn of the
/// hues, brightness pulse, neon (the emissive alone, unlit).
pub const EFFECT_NONE: u8 = 0;
pub const EFFECT_CYCLE: u8 = 64;
pub const EFFECT_STATIC: u8 = 128;
pub const EFFECT_PULSE: u8 = 192;
pub const EFFECT_NEON: u8 = 240;
/// Animated deck: the holo map is a frame atlas (anim.rs, knowledge/animated-decks.md). Read as a band of block 2's
/// red, 0.04 to 0.1, under the cycle threshold.
pub const EFFECT_ANIM: u8 = 16;

/// One marker block: (x0, y0, x1, y1, rgb) in holo-map texels, linear values.
pub type MarkerBlock = (u32, u32, u32, u32, (u8, u8, u8));

/// The rectangles of the marker to paint into a deck's holo map (`shaderpatch.marker_pixels`). Block 2 holds the
/// effect, the speed (G / 255 x 2 turns a second) and the glow strength (the emissive is the colour x
/// 2^(12 B / 255) - 1).
pub fn marker_pixels(effect: u8, speed: u8, glow: u8) -> [MarkerBlock; 3] {
    [(0, 0, BLOCK, BLOCK, MARK_A), (BLOCK, 0, 2 * BLOCK, BLOCK, MARK_B), (2 * BLOCK, 0, 3 * BLOCK, BLOCK, (effect, speed, glow))]
}

/// What went wrong in the shader layer.
#[derive(Debug)]
pub enum ShaderError {
    /// DXC failed (loading, a COM call, disassembly, assembly or validation).
    Dxc(String),
    /// A program did not have the shape the patch expects.
    Patch(String),
    Game(GameError),
    Io(PathBuf, std::io::Error),
}

impl fmt::Display for ShaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShaderError::Dxc(m) => write!(f, "DXC: {m}"),
            ShaderError::Patch(m) => write!(f, "shader patch: {m}"),
            ShaderError::Game(e) => write!(f, "{e}"),
            ShaderError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for ShaderError {}

impl From<GameError> for ShaderError {
    fn from(e: GameError) -> ShaderError {
        ShaderError::Game(e)
    }
}

impl From<crate::cas::CasError> for ShaderError {
    fn from(e: crate::cas::CasError) -> ShaderError {
        ShaderError::Game(GameError::Cas(e))
    }
}

impl From<crate::bundle::BundleError> for ShaderError {
    fn from(e: crate::bundle::BundleError) -> ShaderError {
        ShaderError::Game(GameError::Bundle(e))
    }
}

impl From<crate::toc::TocError> for ShaderError {
    fn from(e: crate::toc::TocError) -> ShaderError {
        ShaderError::Game(GameError::Toc(e))
    }
}

/// Which patch a program gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Patch {
    Deck,
    /// The deck patch with the flipbook for animated decks (deck::patch_anim, knowledge/animated-decks.md).
    DeckAnim,
    Ribbon,
    /// The ribbon's vertex program (`nyanvspatch.patch`): nyan ribbons flat and pulled toward the camera.
    RibbonVs,
}

/// Every program Caveman replaces, in the order the mod writes them: the six deck programs, then the ribbon's pixel
/// and vertex programs.
pub fn programs() -> Vec<(&'static str, Patch)> {
    programs_with(false)
}

/// [`programs`], the deck programs with the flipbook when `animated`.
pub fn programs_with(animated: bool) -> Vec<(&'static str, Patch)> {
    let deck = if animated { Patch::DeckAnim } else { Patch::Deck };
    DECK_PROGRAMS.iter().map(|g| (*g, deck)).chain([(RIBBON_PROGRAM, Patch::Ribbon), (ribbon_vs::RIBBON_VS_PROGRAM, Patch::RibbonVs)]).collect()
}

/// A program's patched disassembly and note (`shaderpatch.patch` / `nyanpatch.patch`).
pub fn patch_ir(which: Patch, ir_text: &str) -> Result<(String, String), ShaderError> {
    match which {
        Patch::Deck => patch_deck(ir_text).map_err(ShaderError::Patch),
        Patch::DeckAnim => deck::patch_anim(ir_text).map_err(ShaderError::Patch),
        Patch::Ribbon => patch_ribbon(ir_text).map(|t| (t, "nyan ribbon".to_string())).map_err(ShaderError::Patch),
        // the banner (ribbon_banner: the width follows the board's axis as laid) on top of the flat patch
        Patch::RibbonVs => {
            ribbon_vs::patch(ir_text).and_then(|flat| ribbon_banner::patch_vs(&flat)).map(|t| (t, "nyan banner".to_string())).map_err(ShaderError::Patch)
        }
    }
}

/// One program end to end (`shaderpatch.build`'s loop body / `nyanpatch.build`): disassemble the game's program,
/// patch, assemble, validate and sign, wrap.
pub fn build_program(dxc: &Dxc, original: &[u8], which: Patch, name: &str) -> Result<BuiltProgram, ShaderError> {
    let res = read_bytecode_resource(original).ok_or_else(|| ShaderError::Patch(format!("{name}: no DXBC")))?;
    let ir_text = dxc.disassemble(res.dxbc)?;
    let (patched, notes) = patch_ir(which, &ir_text)?;
    let out = dxc.assemble(&patched, true)?;
    if !out.ok {
        return Err(ShaderError::Dxc(format!("{name}: {}", out.messages)));
    }
    let (payload, meta) = wrap(original, &out.bytes).map_err(ShaderError::Patch)?;
    Ok(BuiltProgram { name: name.to_string(), payload, meta, notes })
}

/// Every replaced program, built from the game's `delmargameconfiguration` bundle (`shaderpatch.build` plus
/// `nyanpatch.build`).
pub fn build_programs(game: &Game, bundle: &Bundle, dxc: &Dxc) -> Result<Vec<BuiltProgram>, ShaderError> {
    build_programs_with(game, bundle, dxc, false)
}

/// [`build_programs`], the deck programs with the flipbook when `animated`.
pub fn build_programs_with(game: &Game, bundle: &Bundle, dxc: &Dxc, animated: bool) -> Result<Vec<BuiltProgram>, ShaderError> {
    let m = &bundle.manifest;
    let mut out = Vec::new();
    for (guid, which) in programs_with(animated) {
        let name = format!("shaders/bytecode/{guid}");
        let i = m.res.iter().rposition(|r| r.name == name).ok_or_else(|| ShaderError::Patch(format!("no {name}")))?;
        let loc = bundle.res_loc(i).ok_or_else(|| ShaderError::Patch(format!("{name}: no file")))?;
        let original = game.decode(&loc)?;
        out.push(build_program(dxc, &original, which, &name)?);
    }
    Ok(out)
}
