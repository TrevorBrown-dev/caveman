//! Caveman FX's data side (docs/fx-hook-plan.md): the `Caveman-FX` mod, separate from Caveman-Decks and
//! Caveman-Shaders (which it leaves alone).
//!
//! - **fx states**: `BoolAsset`s `Animation/Caveman/FX.<Name>` in the shared bundle, cloned from a game state of the
//!   same bundle (`animation/dingo/bool.state.is.wipeout.getup.end`), `Default` False. Their GUIDs are the game's style
//!   for states (one 8-byte half repeated, file GUID = instance GUID), the half taken from the name, so every build and
//!   every other mod sees the same ones. On their own they read False; `caveman_fx.dll` answers for them.
//! - **test decks**: each carries a board effect (an aura, a nyan trail, or a stock burst) whose effect item fires on
//!   one fx state (TriggerType 6), Continuous or OneOff with a cooldown.
//! - **library decks** (`LIBRARY`, "FXL <effect>"): one effect each, all on `fx.test`, so any of them can be equipped
//!   and fired from the trainer: the game's own effect blueprints of the shared bundle (knowledge/effects-catalog.md)
//!   played as they are at chosen board points, retuned copies of them (`TUNED`: bigger, brighter, longer, denser),
//!   auras at higher strengths, and the sprite prototype (`SPRITE`): a burst of Caveman's own star sprites, through a
//!   material key of its own (a texture, a depot entry, and alias rows in the global shader database and texture
//!   lookup, as ReSkate Studio adds materials; `shaderstate.rs`).

use std::path::Path;
use std::sync::Arc;

use crate::build::{self, BuildError, BuildOptions, BuildReport, DeckLayers, DeckSpec, SHARED};
use crate::ebx::{self, Document, Guid};
use crate::effects::{self, AuraTuning, BoardPoints, EffectTuning, Layers, NyanTrigger, ResolvedBoard, StateTrigger, Usage, NYAN_STEPS};
use crate::game::{Game, ITEMS_TOC};
use crate::image::Image;
use crate::item_cache::{hash64, Op};
use crate::shader::{CONFIG_BUNDLE, GLOBALS_TOC};
use crate::shaderstate::{self, Depot};

pub const MOD: &str = "Caveman-FX";
/// The game state every fx state is cloned from (a plain BoolAsset in the shared bundle).
pub const STATE_TEMPLATE: &str = "animation/dingo/bool.state.is.wipeout.getup.end";
/// The fx states' run-time names start with this; caveman_fx.dll looks for it.
pub const STATE_PREFIX: &str = "Animation/Caveman/FX.";
/// The fx states the mod carries: the default rules' (crates/caveman-fx/rules/default.json), `Test` for the trainer,
/// and spare slots for rules written later without rebuilding the mod.
pub const STATES: [&str; 22] = [
    "Test", "Kickflip", "Heelflip", "Shuvit", "Flip", "Grind", "Grind.Start", "Manual", "Landed", "Trick.Any", "Air", "Powerslide", "Grab", "Wipeout",
    "Slot1", "Slot2", "Slot3", "Slot4", "Slot5", "Slot6", "Slot7", "Slot8",
];
/// The game's one-shot effects in the shared bundle (statefx.BURSTS).
pub const IMPACT: &str = "effects/characters/effectblueprints/ebp_chr_ctd_impact_a";
pub const DUST: &str = "effects/characters/effectblueprints/ebp_chr_impactdust_a";
/// More of the shared bundle's effect blueprints (knowledge/effects-catalog.md), for the library decks.
pub const SPARKLER: &str = "effects/characters/effectblueprints/ebp_popup_dynamitecostume_sparkler";
pub const FIRE_HEAD: &str = "effects/characters/effectblueprints/eb_chr_fire_head";
pub const FIRE_SHOULDER: &str = "effects/characters/effectblueprints/eb_chr_fire_shoulder";
pub const FLAME_TRAIL: &str = "effects/characters/effectblueprints/ebp_ribbon_flame_trail_a";
pub const DARK_WISPS: &str = "effects/characters/effectblueprints/ebp_ribbon_dark_wisps_short";
pub const GHOST_TRAILS: &str = "effects/characters/effectblueprints/ebp_ribbon_ghost_skater_trails_long";
pub const HOOD_MIST: &str = "effects/characters/effectblueprints/ebp_chr_gr_hood_mist";
pub const SCYTHE_FIRE: &str = "effects/characters/effectblueprints/ebp_chr_gr_scythefire_vanim";
pub const ONI_EYES: &str = "effects/characters/effectblueprints/ebp_chr_oni_eyes";
pub const WATER_WAKE: &str = "effects/environments/effectblueprints/eb_env_water_floating_loop";
pub const CANDY: &str = "effects/characters/items/s1_pumpkincandymask/ebp_chr_jumpmotion_candy";
pub const SMOKE_PUFF: &str = "effects/characters/items/s4_dembones/ebp_chr_dembones_crash_dust_only";
pub const BONE_DEBRIS: &str = "effects/characters/items/s4_dembones/ebp_chr_dembones_crash_dust_debris_heavy";
pub const PINE_DEBRIS: &str = "effects/materialgrid/effectblueprints/ebp_mat_pineimpact_debris";
pub const SPLASH: &str = "effects/materialgrid/effectblueprints/eb_env_mgrid_water_large_with_ripple";
pub const SKULL: &str = "effects/characters/items/s4_dembones/ebp_chr_dembones_crash_head";

/// A test deck's board effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FxLook {
    Aura([u8; 3]),
    Nyan,
    Burst(&'static str),
    /// One of the shared bundle's effect blueprints, played as it is at chosen board points (library decks).
    Stock(&'static str, BoardPoints),
    /// A blueprint the mod ships on its own, by its short name (`TUNED`, `SPRITE`), at chosen board points.
    Own(&'static str, BoardPoints),
    /// An aura at a chosen strength.
    AuraTuned([u8; 3], AuraTuning),
}

/// A retuned copy of one of the shared bundle's blueprints, shipped by the mod as
/// `characters/vfx/caveman/fxlib/ebp_<name>` (the copy works as the aura's copy of the oni blueprint does).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TunedFx {
    pub name: &'static str,
    pub source: &'static str,
    pub tuning: EffectTuning,
}

pub const TUNED: [TunedFx; 2] = [
    // the crash test dummy's impact: three times the particles, four times their size, twice their life, three times
    // as bright, the flash light four times as long and three times as wide
    TunedFx {
        name: "impact_big",
        source: IMPACT,
        tuning: EffectTuning { count: 3.0, size: 4.0, life: 2.0, brightness: 3.0, light_intensity: 1.0, light_radius: 3.0, light_life: 4.0 },
    },
    // the dynamite sparkler: twice the sparks, three times their size, half again their life, twice as bright
    TunedFx {
        name: "sparkler_big",
        source: SPARKLER,
        tuning: EffectTuning { count: 2.0, size: 3.0, life: 1.5, brightness: 2.0, light_intensity: 3.0, light_radius: 1.0, light_life: 1.0 },
    },
];

/// Where the mod's own blueprints and textures go.
pub const OWN_PREFIX: &str = "characters/vfx/caveman/fxlib/";

/// The aura at four and at sixteen times the plain aura's light (and brighter, wider, longer trails).
pub const AURA_X4: AuraTuning = AuraTuning { light_intensity: 160.0, light_radius: 1.5, trail_boost: 4.0, trail_dim: 0.75, trail_width: Some(2.0), trail_life: Some(0.3) };
pub const AURA_X16: AuraTuning = AuraTuning { light_intensity: 640.0, light_radius: 2.5, trail_boost: 10.0, trail_dim: 2.0, trail_width: Some(3.0), trail_life: Some(0.5) };

/// The sprite prototype: the crash test dummy impact's "bouncing bits" emitter (debris sprites from a 2 x 2 sheet,
/// thrown out and falling with gravity), drawn with Caveman's own sheet of cartoon stars through a material key of its
/// own (knowledge/effects-catalog.md, "Our own textures").
pub struct SpriteFx {
    /// The blueprint's short name (`OWN_PREFIX` + `ebp_<name>`).
    pub name: &'static str,
    pub source: &'static str,
    /// The source emitter's material key (`t_fx_ctd_bouncingbits_a` in the shared bundle's depot).
    pub source_key: i64,
    /// The texture that key binds, swapped for ours in the copy of its block.
    pub source_texture: &'static str,
    /// A BC7 sRGB texture of the shared bundle to clone ours from (same size: 1024 x 1024, 11 mips, whole).
    pub template: &'static str,
    /// Our texture's name.
    pub texture: &'static str,
    /// Our depot resource's name (ends in the key, as the game's and Studio's do).
    pub depot_prefix: &'static str,
}

pub const SPRITE: SpriteFx = SpriteFx {
    name: "star_burst",
    source: IMPACT,
    source_key: 2_833_993_466_531_726_840,
    source_texture: "effects/characters/textures/crashtestdummy/t_fx_ctd_bouncingbits_a",
    template: "effects/_global/textures/masks/t_fx_rgb_gradient_masks",
    texture: "characters/vfx/caveman/fxlib/t_caveman_star_sprites",
    depot_prefix: "characters/vfx/caveman/fxlib/caveman_fx_win32_shaderstate/shaderblockdepot_",
};
/// The global shader database and texture-set lookup the effect material keys live in (game configuration bundle).
pub const PROGRAM_LOOKUP: &str = "configurations/gameconfigurations/delmargameconfiguration_win32_shaderstate_db";
pub const TEXTURE_LOOKUP: &str = "configurations/gameconfigurations/delmargameconfiguration_win32_shaderstate_streamabletexturesetlookuptable";
/// The flipbook graphs' initial speed range (min, max): the sprite stars fly out slower than the debris they replace.
pub const SPRITE_SPEED: i64 = -111_169_611;

/// A test deck: its name, the fx state it fires on, the effect and how it plays, and the words on its art.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxDeck {
    pub name: &'static str,
    pub state: &'static str,
    pub look: FxLook,
    pub usage: Usage,
    /// Seconds between two one-off firings (above the DLL's 150 ms pulse, so a pulse fires once).
    pub cooldown: f64,
    pub label: [&'static str; 3],
    pub colour: [u8; 3],
}

pub const DECKS: [FxDeck; 8] = [
    FxDeck { name: "FX Test Aura", state: "Test", look: FxLook::Aura([255, 255, 255]), usage: Usage::Continuous, cooldown: 0.0, label: ["TEST", "WHITE AURA", "BY HAND"], colour: [90, 90, 110] },
    FxDeck { name: "FX Kickflip Burst", state: "Kickflip", look: FxLook::Burst(IMPACT), usage: Usage::OneOff, cooldown: 0.3, label: ["KICKFLIP", "BURST", "ONE SHOT"], colour: [230, 60, 40] },
    FxDeck { name: "FX Flip Aura", state: "Flip", look: FxLook::Aura([40, 255, 40]), usage: Usage::Continuous, cooldown: 0.0, label: ["ANY FLIP", "GREEN AURA", "WHILE FLIPPING"], colour: [30, 160, 50] },
    FxDeck { name: "FX Grind Nyan", state: "Grind", look: FxLook::Nyan, usage: Usage::Continuous, cooldown: 0.0, label: ["GRIND", "RAINBOW", "WHILE GRINDING"], colour: [200, 40, 200] },
    FxDeck { name: "FX Grind Start Burst", state: "Grind.Start", look: FxLook::Burst(IMPACT), usage: Usage::OneOff, cooldown: 0.3, label: ["GRIND ON", "BURST", "ONE SHOT"], colour: [240, 140, 0] },
    FxDeck { name: "FX Manual Aura", state: "Manual", look: FxLook::Aura([0, 255, 255]), usage: Usage::Continuous, cooldown: 0.0, label: ["MANUAL", "CYAN AURA", "WHILE MANUAL"], colour: [0, 150, 170] },
    FxDeck { name: "FX Landed Burst", state: "Landed", look: FxLook::Burst(DUST), usage: Usage::OneOff, cooldown: 0.3, label: ["LANDED", "DUST BURST", "ONE SHOT"], colour: [140, 110, 80] },
    FxDeck { name: "FX Air Aura", state: "Air", look: FxLook::Aura([255, 220, 0]), usage: Usage::Continuous, cooldown: 0.0, label: ["AIR", "YELLOW AURA", "IN THE AIR"], colour: [200, 170, 0] },
];

/// One effect of a combo deck: its look, the fx state it fires on and how it plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComboSpec {
    pub look: FxLook,
    pub state: &'static str,
    pub usage: Usage,
    pub cooldown: f64,
}

/// A combo deck: several effects on one board, each on its own fx state, and the words on its art.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxCombo {
    pub name: &'static str,
    pub parts: &'static [ComboSpec],
    pub label: [&'static str; 3],
    pub colour: [u8; 3],
}

pub const COMBOS: [FxCombo; 1] = [FxCombo {
    name: "FX Combo",
    parts: &[
        ComboSpec { look: FxLook::Aura([0, 255, 255]), state: "Manual", usage: Usage::Continuous, cooldown: 0.0 },
        ComboSpec { look: FxLook::Aura([40, 255, 40]), state: "Flip", usage: Usage::Continuous, cooldown: 0.0 },
        // fx.air's default rule excludes flip tricks, so this and the flip aura never show together
        ComboSpec { look: FxLook::Aura([255, 220, 0]), state: "Air", usage: Usage::Continuous, cooldown: 0.0 },
        ComboSpec { look: FxLook::Nyan, state: "Grind", usage: Usage::Continuous, cooldown: 0.0 },
        ComboSpec { look: FxLook::Burst(IMPACT), state: "Grind.Start", usage: Usage::OneOff, cooldown: 0.3 },
        ComboSpec { look: FxLook::Burst(IMPACT), state: "Kickflip", usage: Usage::OneOff, cooldown: 0.3 },
        ComboSpec { look: FxLook::Burst(DUST), state: "Landed", usage: Usage::OneOff, cooldown: 0.3 },
    ],
    label: ["COMBO", "MANUAL FLIP AIR", "GRIND LAND"],
    colour: [120, 40, 200],
}];

/// The fx state every library deck fires on: the trainer's `fire fx.test <ms>` plays the equipped one.
pub const LIBRARY_STATE: &str = "Test";

const fn lib(name: &'static str, look: FxLook, usage: Usage, cooldown: f64, label: [&'static str; 3], colour: [u8; 3]) -> FxDeck {
    FxDeck { name, state: LIBRARY_STATE, look, usage, cooldown, label, colour }
}

/// The FX Library ("FXL <effect>"): one effect per deck, all on `fx.test`. Loops are Continuous (they play while
/// fx.test is held); one-shots are OneOff with a cooldown, so holding fx.test for 5 s fires a few.
pub const LIBRARY: [FxDeck; 24] = [
    // the game's looping costume effects
    lib("FXL Sparkler", FxLook::Stock(SPARKLER, BoardPoints::Trucks), Usage::Continuous, 0.0, ["SPARKLER", "LOOP", "AT THE TRUCKS"], [230, 110, 20]),
    lib("FXL Fire Head", FxLook::Stock(FIRE_HEAD, BoardPoints::Centre), Usage::Continuous, 0.0, ["FIRE", "LOOP", "STUNT RIDER"], [220, 50, 10]),
    lib("FXL Fire Shoulder", FxLook::Stock(FIRE_SHOULDER, BoardPoints::Trucks), Usage::Continuous, 0.0, ["FIRE SMALL", "LOOP", "AT THE TRUCKS"], [200, 80, 10]),
    lib("FXL Flame Trail", FxLook::Stock(FLAME_TRAIL, BoardPoints::Centre), Usage::Continuous, 0.0, ["FLAME TRAIL", "LOOP", "REAPER"], [180, 60, 0]),
    lib("FXL Dark Wisps", FxLook::Stock(DARK_WISPS, BoardPoints::Wheels), Usage::Continuous, 0.0, ["DARK WISPS", "LOOP", "AT THE WHEELS"], [50, 40, 70]),
    lib("FXL Ghost Trails", FxLook::Stock(GHOST_TRAILS, BoardPoints::NoseTail), Usage::Continuous, 0.0, ["GHOST TRAILS", "LOOP", "NOSE AND TAIL"], [120, 170, 200]),
    lib("FXL Reaper Mist", FxLook::Stock(HOOD_MIST, BoardPoints::Centre), Usage::Continuous, 0.0, ["MIST", "LOOP", "REAPER HOOD"], [70, 90, 110]),
    lib("FXL Scythe Fire", FxLook::Stock(SCYTHE_FIRE, BoardPoints::Centre), Usage::Continuous, 0.0, ["GEO FLAME", "LOOP", "REAPER SCYTHE"], [150, 30, 120]),
    lib("FXL Oni Eyes", FxLook::Stock(ONI_EYES, BoardPoints::NoseTail), Usage::Continuous, 0.0, ["EYE GLOW", "LOOP", "NOSE AND TAIL"], [140, 40, 220]),
    lib("FXL Water Wake", FxLook::Stock(WATER_WAKE, BoardPoints::Ground), Usage::Continuous, 0.0, ["WATER WAKE", "LOOP", "ON THE GROUND"], [30, 110, 200]),
    lib("FXL Candy", FxLook::Stock(CANDY, BoardPoints::Centre), Usage::Continuous, 0.0, ["CANDY", "LOOP", "PUMPKIN MASK"], [240, 120, 30]),
    // the game's one-shots
    lib("FXL Impact", FxLook::Stock(IMPACT, BoardPoints::Centre), Usage::OneOff, 1.0, ["IMPACT", "BURST", "CRASH DUMMY"], [230, 60, 40]),
    lib("FXL Dust Burst", FxLook::Stock(DUST, BoardPoints::Ground), Usage::OneOff, 1.0, ["DUST", "BURST", "ON THE GROUND"], [150, 120, 90]),
    lib("FXL Smoke Puff", FxLook::Stock(SMOKE_PUFF, BoardPoints::Ground), Usage::OneOff, 1.0, ["SMOKE PUFF", "BURST", "DEM BONES"], [120, 120, 120]),
    lib("FXL Bone Debris", FxLook::Stock(BONE_DEBRIS, BoardPoints::Centre), Usage::OneOff, 1.5, ["BONE DEBRIS", "BURST", "DEM BONES"], [210, 200, 170]),
    lib("FXL Pine Debris", FxLook::Stock(PINE_DEBRIS, BoardPoints::Centre), Usage::OneOff, 1.0, ["PINE NEEDLES", "BURST", "DEBRIS"], [60, 120, 50]),
    lib("FXL Splash", FxLook::Stock(SPLASH, BoardPoints::Ground), Usage::OneOff, 1.5, ["WATER SPLASH", "BURST", "ON THE GROUND"], [40, 140, 220]),
    lib("FXL Skull Drop", FxLook::Stock(SKULL, BoardPoints::Centre), Usage::OneOff, 3.0, ["SKULL", "BURST", "DEM BONES"], [235, 230, 210]),
    // stronger versions
    lib("FXL Impact Big", FxLook::Own("impact_big", BoardPoints::Centre), Usage::OneOff, 1.0, ["IMPACT X3", "BURST", "RETUNED"], [255, 40, 40]),
    lib("FXL Sparkler Big", FxLook::Own("sparkler_big", BoardPoints::Trucks), Usage::Continuous, 0.0, ["SPARKLER X3", "LOOP", "RETUNED"], [255, 140, 0]),
    lib("FXL Aura X4", FxLook::AuraTuned([0, 255, 255], AURA_X4), Usage::Continuous, 0.0, ["AURA X4", "LOOP", "BRIGHTER"], [0, 160, 180]),
    lib("FXL Aura X16", FxLook::AuraTuned([0, 255, 255], AURA_X16), Usage::Continuous, 0.0, ["AURA X16", "LOOP", "BRIGHTEST"], [0, 200, 220]),
    // Caveman's own sprites
    lib("FXL Star Burst", FxLook::Own("star_burst", BoardPoints::Centre), Usage::OneOff, 1.5, ["STAR BURST", "OUR SPRITES", "PROTOTYPE"], [250, 210, 0]),
    // the plain aura on fx.test, the yardstick the stronger ones are compared with
    lib("FXL Aura X1", FxLook::AuraTuned([0, 255, 255], AuraTuning::STOCK), Usage::Continuous, 0.0, ["AURA X1", "LOOP", "AS BEFORE"], [0, 110, 130]),
];

/// An fx state's run-time name: `Animation/Caveman/FX.Kickflip`.
pub fn state_name(short: &str) -> String {
    format!("{STATE_PREFIX}{short}")
}

/// An fx state's (file, instance) GUIDs: the game's style for states (one 8-byte half repeated, file = instance),
/// the half from the lower-case name.
pub fn state_ref(short: &str) -> (Guid, Guid) {
    let h = crate::item_cache::hash64(&[b"caveman-fx-state-1", state_name(short).to_ascii_lowercase().as_bytes()]).to_be_bytes();
    let mut be = [0u8; 16];
    be[..8].copy_from_slice(&h);
    be[8..].copy_from_slice(&h);
    let g = Guid::from_be(be);
    (g, g)
}

/// An fx state asset cloned from the template's EBX: (bundle-manifest name, EBX bytes).
pub fn state_asset(template: &[u8], short: &str) -> ebx::Result<(String, Vec<u8>)> {
    let mut doc = ebx::read_document(template)?;
    if doc.root_type != "BoolAsset" || doc.instances.len() != 1 || !doc.imports.is_empty() {
        return Err(ebx::Error(format!("{STATE_TEMPLATE} is not a plain BoolAsset ({}, {} instances)", doc.root_type, doc.instances.len())));
    }
    let name = state_name(short);
    let (file, instance) = state_ref(short);
    doc.set_file_guid(file);
    doc.set_instance_guid(0, instance);
    doc.rename(&name)?;
    doc.set("Default", false, 0)?;
    Ok((name.to_ascii_lowercase(), ebx::write_document_with(&doc, false, false)?))
}

// ---------------------------------------------------------------- the test decks' art

/// A 5 x 7 font: capitals, digits and a little punctuation (rows top to bottom, bit 4 the left column).
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
        '-' => [0, 0, 0, 0x1F, 0, 0, 0],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0, 0x04],
        _ => [0; 7],
    }
}

/// Draws `text` centred on row `y` (its top), as large as fits in `max_width` px up to `max_scale`; returns the height.
fn draw_line(img: &mut Image, text: &str, y: usize, max_scale: usize, colour: [u8; 3]) -> usize {
    let n = text.chars().count().max(1);
    // the deck is narrower than the texture, and rounded at the ends: keep 90 px off each side
    let scale = ((img.width - 180) / (n * 6 - 1)).clamp(1, max_scale);
    let width = (n * 6 - 1) * scale;
    let x0 = (img.width - width) / 2;
    for (i, c) in text.chars().enumerate() {
        for (row, bits) in glyph(c).iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (x0 + (i * 6 + col) * scale + dx, y + row * scale + dy);
                        if px < img.width && py < img.height {
                            img.pixel_mut(px, py).copy_from_slice(&colour);
                        }
                    }
                }
            }
        }
    }
    7 * scale
}

/// A test deck's art (512 x 2048 RGB): its colour, dark at the ends; CAVEMAN FX, the three label lines and the fx
/// state's name, so each deck is told apart in the menu and on the board.
pub fn deck_art(deck: &FxDeck) -> Image {
    let (w, h) = (512usize, 2048usize);
    let mut img = Image::new(w, h, 3);
    for y in 0..h {
        let t = 1.0 - ((y as f64 / h as f64) - 0.5).abs() * 1.4; // 0.3 at the ends, 1 in the middle
        let px: [u8; 3] = deck.colour.map(|c| (c as f64 * t).round().clamp(0.0, 255.0) as u8);
        for x in 0..w {
            let stripe = (x + y) / 48 % 2 == 0;
            let v = if stripe { px } else { px.map(|c| (c as f64 * 0.85) as u8) };
            img.pixel_mut(x, y).copy_from_slice(&v);
        }
    }
    let bright = deck.colour.iter().map(|&c| c as u32).sum::<u32>() > 420;
    let ink = if bright { [10, 10, 10] } else { [255, 255, 255] };
    let mut y = 330;
    y += draw_line(&mut img, "CAVEMAN", y, 12, [255, 255, 255]) + 24;
    draw_line(&mut img, "FX", y, 20, [255, 255, 255]);
    let mut y = 760;
    for line in deck.label {
        y += draw_line(&mut img, line, y, 10, ink) + 40;
    }
    let state = format!("FX.{}", deck.state.to_ascii_uppercase());
    draw_line(&mut img, &state, 1560, 6, [255, 255, 255]);
    img
}

// ---------------------------------------------------------------- the mod's own blueprints

/// A GUID for one of the mod's own assets, from its name and a part ("file", "instance 3"): the same in every build.
fn own_guid(name: &str, part: &str) -> Guid {
    let half = |s: &[u8]| hash64(&[b"caveman-fx-own-1", name.as_bytes(), part.as_bytes(), s]).to_be_bytes();
    let mut be = [0u8; 16];
    be[..8].copy_from_slice(&half(b"a"));
    be[8..].copy_from_slice(&half(b"b"));
    be[6] = (be[6] & 0x0F) | 0x40;
    be[8] = (be[8] & 0x3F) | 0x80;
    Guid::from_be(be)
}

/// A game EBX as one of the mod's own documents: GUIDs from `name` (own_guid), renamed `name`.
fn clone_own(template: &[u8], name: &str) -> ebx::Result<Document> {
    let mut doc = ebx::read_document(template)?;
    doc.set_file_guid(own_guid(name, "file"));
    for i in 0..doc.instances.len() {
        doc.set_instance_guid(i, own_guid(name, &format!("instance {i}")));
    }
    doc.rename(name)?;
    Ok(doc)
}

/// The asset name of one of the mod's own blueprints.
pub fn own_blueprint_name(short: &str) -> String {
    format!("{OWN_PREFIX}ebp_{short}")
}

/// A retuned copy of a stock blueprint (`TUNED`): (asset name, document). `source` is the stock blueprint's EBX.
pub fn tuned_blueprint(source: &[u8], t: &TunedFx) -> Result<(String, Document), BuildError> {
    let name = own_blueprint_name(t.name);
    let mut doc = clone_own(source, &name)?;
    let changed = effects::retune_blueprint(&mut doc, &t.tuning)?;
    if changed == 0 {
        return Err(BuildError(format!("{}: retuning {} changed nothing", t.name, t.source)));
    }
    Ok((name, doc))
}

/// A sheet of four cartoon sprites, `side` x `side` RGBA, 2 x 2 cells (as the debris sheet it replaces): a yellow star,
/// an orange star, a cyan arrow pointing up, a pink star, each with a dark outline. Outside the shapes the colour is
/// the outline's, so mips and filtering fringe dark, not white.
pub fn star_sheet(side: usize) -> Image {
    let cell = side / 2;
    let mut img = Image::new(side, side, 4);
    let star = |points: usize, outer: f64, inner: f64| -> Vec<(f64, f64)> {
        (0..points * 2)
            .map(|k| {
                let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * k as f64 / points as f64;
                let r = if k % 2 == 0 { outer } else { inner };
                (r * a.cos(), r * a.sin())
            })
            .collect()
    };
    // y grows downwards in the image: the arrow's tip at the top (y -0.9)
    let arrow: Vec<(f64, f64)> = vec![(0.0, -0.9), (0.75, -0.05), (0.3, -0.05), (0.3, 0.85), (-0.3, 0.85), (-0.3, -0.05), (-0.75, -0.05)];
    let shapes: [(Vec<(f64, f64)>, [u8; 3]); 4] =
        [(star(5, 0.92, 0.42), [255, 220, 30]), (star(5, 0.92, 0.42), [255, 110, 20]), (arrow, [40, 230, 255]), (star(5, 0.92, 0.42), [255, 70, 200])];
    let outline = [20u8, 12, 30];
    let border = 0.09; // the outline's width, in cell units (the cell spans -1..1)
    let aa = 2.0 / cell as f64; // about one texel
    for (n, (poly, fill)) in shapes.iter().enumerate() {
        let (ox, oy) = ((n % 2) * cell, (n / 2) * cell);
        for y in 0..cell {
            for x in 0..cell {
                let (u, v) = ((x as f64 + 0.5) / cell as f64 * 2.0 - 1.0, (y as f64 + 0.5) / cell as f64 * 2.0 - 1.0);
                let d = signed_distance(poly, u, v); // negative inside
                let alpha = (0.5 - d / aa).clamp(0.0, 1.0);
                let inner = (0.5 - (d + border) / aa).clamp(0.0, 1.0); // 1 well inside the outline
                let mut px = [0u8; 4];
                for c in 0..3 {
                    px[c] = (outline[c] as f64 + (fill[c] as f64 - outline[c] as f64) * inner).round() as u8;
                }
                px[3] = (255.0 * alpha).round() as u8;
                img.pixel_mut(ox + x, oy + y).copy_from_slice(&px);
            }
        }
    }
    img
}

/// Signed distance from (x, y) to a polygon's outline: negative inside.
fn signed_distance(poly: &[(f64, f64)], x: f64, y: f64) -> f64 {
    let mut best = f64::INFINITY;
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((x - a.0) * dx + (y - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let (px, py) = (a.0 + t * dx - x, a.1 + t * dy - y);
        best = best.min((px * px + py * py).sqrt());
        if (a.1 > y) != (b.1 > y) && x < a.0 + (y - a.1) * dx / dy {
            inside = !inside;
        }
    }
    if inside {
        -best
    } else {
        best
    }
}

/// Everything the sprite prototype adds: its blueprint (an extra EBX), the texture and depot (extra pieces in the
/// shared bundle) and the configuration rows (the global shader database and texture lookup, replaced whole).
pub struct SpritePieces {
    pub blueprint: (String, Vec<u8>),
    pub reference: (Guid, Guid),
    pub ops: Vec<Op>,
    pub config_res: Vec<(String, Vec<u8>, [u8; 16])>,
    /// The new material key, the texture's resource id, the depot's.
    pub key: i64,
    pub texture_rid: u64,
    pub depot_rid: u64,
    /// The source key's records and texture set the new key aliases.
    pub records: Vec<u32>,
    pub texture_set: u64,
}

/// The depot resource's name for a key.
pub fn sprite_depot_name(key: i64) -> String {
    format!("{}{}", SPRITE.depot_prefix, key as u64)
}

/// Builds the sprite prototype's pieces from the player's own game: the star sheet as a texture of the shared bundle
/// (cloned from `SPRITE.template`, made as deck textures are), a material key of its own whose depot block is the
/// source key's with the texture swapped, alias rows for the key in the global shader database and texture lookup,
/// and the blueprint: the impact's bouncing-bits emitter and its flash, drawing with the new key, more and bigger.
pub fn sprite_pieces(game: &Game, shared: &crate::game::Bundle, mod_name: &str) -> Result<SpritePieces, BuildError> {
    let ebx_of = |name: &str| -> Result<Vec<u8>, BuildError> {
        let i = shared.manifest.ebx_index(name).ok_or_else(|| BuildError(format!("the shared bundle has no {name}")))?;
        Ok(game.decode(&shared.files[shared.manifest.ebx_file(i)])?)
    };
    let shared_rids: std::collections::HashSet<u64> = shared.manifest.res.iter().map(|r| r.rid).collect();
    // the texture
    let (mut ops, tex, texture_rid, chunk) = build::mod_texture(game, SHARED, SPRITE.template, SPRITE.texture, &star_sheet(1024), mod_name)?;
    if shared_rids.contains(&texture_rid) || game.chunk(&chunk, ITEMS_TOC)?.is_some() || shared.manifest.chunks.iter().any(|c| c.guid == chunk) {
        return Err(BuildError(format!("{}: its resource id or chunk is already the game's", SPRITE.texture)));
    }
    // the source key's block, its texture swapped
    let di = shared.manifest.res.iter().position(|r| r.rtype == shaderstate::DEPOT_TYPE).ok_or_else(|| BuildError("the shared bundle has no shader block depot".into()))?;
    let depot_res = &shared.manifest.res[di];
    let depot = shaderstate::read_depot(&game.decode(&shared.files[shared.manifest.res_file(di)])?, &depot_res.meta).map_err(|e| BuildError(e.0))?;
    let list = depot
        .entries
        .iter()
        .find(|(k, _, _)| *k == SPRITE.source_key)
        .map(|(_, l, _)| l.clone())
        .ok_or_else(|| BuildError(format!("the shared depot has no key {}", SPRITE.source_key)))?;
    let source_tex = ebx::read_document(&ebx_of(SPRITE.source_texture)?)?.reference();
    let (list, swapped) = shaderstate::swap_guids(&list, &[(source_tex.0 .0, tex.0 .0), (source_tex.1 .0, tex.1 .0)]);
    if swapped != 2 {
        return Err(BuildError(format!("the source block names {} {swapped} time(s), not twice (instance and file GUID)", SPRITE.source_texture)));
    }
    // the configuration rows: the source key's records and texture set, aliased
    let config = game.bundle(CONFIG_BUNDLE, GLOBALS_TOC)?;
    let res_of = |name: &str| -> Result<(Vec<u8>, [u8; 16]), BuildError> {
        let i = config.manifest.res_index(name).ok_or_else(|| BuildError(format!("{CONFIG_BUNDLE} has no {name}")))?;
        Ok((game.decode(&config.files[config.manifest.res_file(i)])?, config.manifest.res[i].meta))
    };
    let (db, db_meta) = res_of(PROGRAM_LOOKUP)?;
    let (tl, tl_meta) = res_of(TEXTURE_LOOKUP)?;
    let st = |e: shaderstate::StateError| BuildError(e.0);
    let (_, program) = shaderstate::program_rows(&db, &db_meta).map_err(st)?;
    let textures = shaderstate::texture_rows(&tl).map_err(st)?;
    let src = SPRITE.source_key as u64;
    let records = program.iter().find(|(k, _)| *k == src).map(|(_, r)| r.clone()).ok_or_else(|| BuildError("the source key has no shader records".into()))?;
    let texture_set = textures.iter().find(|(k, _)| *k == src).map(|(_, s)| *s).ok_or_else(|| BuildError("the source key has no texture set".into()))?;
    // a key nobody has: from the texture's name, top bit clear
    let mut key = (hash64(&[b"caveman-fx-sprite-1", SPRITE.texture.as_bytes()]) & 0x7FFF_FFFF_FFFF_FFFF) as i64;
    while program.iter().any(|(k, _)| *k == key as u64) || textures.iter().any(|(k, _)| *k == key as u64) || depot.entries.iter().any(|(k, _, _)| *k == key) {
        key += 2;
    }
    let (db2, db2_meta) = shaderstate::add_program_rows(&db, &db_meta, &[(key as u64, records.clone())]).map_err(st)?;
    let tl2 = shaderstate::add_texture_rows(&tl, &[(key as u64, texture_set)]).map_err(st)?;
    // the depot of our own: one entry
    let hash = shaderstate::fnv1a64(&list);
    if depot.entries.iter().any(|(_, _, h)| *h == hash) {
        return Err(BuildError("the sprite block's hash is already a stock block's".into()));
    }
    let (depot_data, depot_meta) = shaderstate::write_depot(&Depot { entries: vec![(key, list, hash)] });
    let mut depot_rid = hash64(&[b"caveman-fx-sprite-1", b"depot rid", mod_name.to_lowercase().as_bytes()]) | 1;
    while shared_rids.contains(&depot_rid) || depot_rid == texture_rid {
        depot_rid = depot_rid.wrapping_add(2);
    }
    ops.push(Op::Res {
        bundle: SHARED.to_string(),
        name: sprite_depot_name(key),
        rtype: shaderstate::DEPOT_TYPE,
        meta: depot_meta,
        rid: depot_rid,
        size: depot_data.len() as u32,
        enc: crate::cas::encode(&depot_data, game.root())?,
    });
    // the blueprint: the bouncing-bits emitter and the flash, on the new key, more stars, bigger, slower
    let name = own_blueprint_name(SPRITE.name);
    let mut bp = clone_own(&ebx_of(SPRITE.source)?, &name)?;
    let kept = effects::keep_components(&mut bp, |doc, i| match doc.instance_type(i) {
        "EmitterGraphEntityData" => effects::material_keys(doc, i).contains(&SPRITE.source_key),
        "LightEffectEntityData" => true,
        _ => false,
    })?;
    if kept != 2 || effects::replace_material_key(&mut bp, SPRITE.source_key, key)? != 1 {
        return Err(BuildError(format!("{}: expected one bouncing-bits emitter and one light", SPRITE.source)));
    }
    effects::retune_blueprint(&mut bp, &EffectTuning { count: 2.5, size: 5.0, life: 1.0, brightness: 1.0, light_intensity: 1.0, light_radius: 2.0, light_life: 3.0 })?;
    if effects::set_emitter_param(&mut bp, SPRITE_SPEED, &[4.0, 10.0])? != 1 {
        return Err(BuildError("the bouncing-bits emitter has no speed range".into()));
    }
    effects::place_components(&mut bp, [0.0, 0.15, 0.0])?;
    let reference = bp.reference();
    let bytes = ebx::write_document_with(&bp, false, false)?;
    Ok(SpritePieces {
        blueprint: (name, bytes),
        reference,
        ops,
        config_res: vec![(PROGRAM_LOOKUP.to_string(), db2, db2_meta), (TEXTURE_LOOKUP.to_string(), tl2, tl_meta)],
        key,
        texture_rid,
        depot_rid,
        records,
        texture_set,
    })
}

// ---------------------------------------------------------------- the build

/// What the mod holds: the fx states and the decks, the mod's own blueprints (short name, asset name, GUIDs) and the
/// sprite prototype's material key.
#[derive(Clone, Debug)]
pub struct FxBuild {
    pub report: BuildReport,
    pub states: Vec<(String, (Guid, Guid))>,
    pub own: Vec<(String, String, (Guid, Guid))>,
    pub sprite_key: i64,
}

/// The item key of a test deck.
pub fn deck_key(display: &str) -> String {
    format!("Own_{}", display.replace(' ', "_"))
}

/// Builds the Caveman-FX mod into `mods_dir/mod_name`: the fx states and the test decks (`prefix` goes before each
/// deck's name). Every template comes from the player's own game.
pub fn build_fx_mod(game: Arc<Game>, mods_dir: &Path, mod_name: &str, prefix: &str, log: &mut dyn FnMut(&str)) -> Result<FxBuild, BuildError> {
    let shared = game.bundle(SHARED, ITEMS_TOC)?;
    let ebx_of = |name: &str| -> Result<Vec<u8>, BuildError> {
        let i = shared.manifest.ebx_index(name).ok_or_else(|| BuildError(format!("the shared bundle has no {name}")))?;
        Ok(game.decode(&shared.files[shared.manifest.ebx_file(i)])?)
    };
    let template = ebx_of(STATE_TEMPLATE)?;
    let mut extra = Vec::new();
    let mut states = Vec::new();
    for short in STATES {
        let (name, bytes) = state_asset(&template, short)?;
        if shared.manifest.ebx_index(&name).is_some() {
            return Err(BuildError(format!("the game already has {name}")));
        }
        states.push((state_name(short), state_ref(short)));
        extra.push((SHARED.to_string(), name, bytes));
    }
    log(&format!("  {} fx states ({}...)", states.len(), states.iter().take(3).map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")));
    // the mod's own blueprints: retuned copies, then the sprite prototype with its texture, depot and material rows
    let mut own: Vec<(String, String, (Guid, Guid))> = Vec::new();
    for t in TUNED {
        let (name, doc) = tuned_blueprint(&ebx_of(t.source)?, &t)?;
        own.push((t.name.to_string(), name.clone(), doc.reference()));
        extra.push((SHARED.to_string(), name, ebx::write_document_with(&doc, false, false)?));
    }
    let sprite = sprite_pieces(&game, &shared, mod_name)?;
    log(&format!(
        "  sprite prototype: material key {} aliasing {} (records {:?}, texture set {:#x}), texture {}, depot {}",
        sprite.key,
        SPRITE.source_key,
        sprite.records,
        sprite.texture_set,
        SPRITE.texture,
        sprite_depot_name(sprite.key)
    ));
    own.push((SPRITE.name.to_string(), sprite.blueprint.0.clone(), sprite.reference));
    extra.push((SHARED.to_string(), sprite.blueprint.0.clone(), sprite.blueprint.1.clone()));
    let own_ref = |short: &str| -> Result<(Guid, Guid), BuildError> {
        own.iter().find(|(s, _, _)| s == short).map(|(_, _, r)| *r).ok_or_else(|| BuildError(format!("no blueprint of the mod's own named {short}")))
    };
    let resolve = |look: FxLook| -> Result<ResolvedBoard, BuildError> {
        Ok(match look {
            FxLook::Aura(c) => ResolvedBoard::Aura(c),
            FxLook::Nyan => ResolvedBoard::Nyan { steps: NYAN_STEPS, trigger: NyanTrigger::Ratio },
            FxLook::Burst(bp) => ResolvedBoard::Burst(ebx::read_document(&ebx_of(bp)?)?.reference()),
            FxLook::Stock(bp, at) => ResolvedBoard::Stock { blueprint: ebx::read_document(&ebx_of(bp)?)?.reference(), at },
            FxLook::Own(short, at) => ResolvedBoard::Stock { blueprint: own_ref(short)?, at },
            FxLook::AuraTuned(colour, tuning) => ResolvedBoard::AuraTuned { colour, tuning },
        })
    };
    let mut decks = Vec::new();
    for d in DECKS {
        let trigger = StateTrigger { state: state_ref(d.state), usage: d.usage, cooldown: d.cooldown };
        let layers = Layers { board: Some(resolve(d.look)?), trigger: Some(trigger), ..Layers::default() };
        let display = format!("{prefix}{}", d.name);
        decks.push(DeckSpec::new(display.clone(), deck_key(&display), deck_art(&d), DeckLayers::Layers(layers)));
    }
    for c in COMBOS {
        let mut combo = Vec::new();
        for p in c.parts {
            combo.push((resolve(p.look)?, StateTrigger { state: state_ref(p.state), usage: p.usage, cooldown: p.cooldown }));
        }
        let layers = Layers { combo, ..Layers::default() };
        let display = format!("{prefix}{}", c.name);
        // the art of a deck that names its effects (the fx state line reads FX.COMBO)
        let art = deck_art(&FxDeck { name: c.name, state: "Combo", look: FxLook::Nyan, usage: Usage::Continuous, cooldown: 0.0, label: c.label, colour: c.colour });
        decks.push(DeckSpec::new(display.clone(), deck_key(&display), art, DeckLayers::Layers(layers)));
    }
    // the library, after the test and combo decks (theirs stay exactly as they were)
    for d in LIBRARY {
        let trigger = StateTrigger { state: state_ref(d.state), usage: d.usage, cooldown: d.cooldown };
        let layers = Layers { board: Some(resolve(d.look)?), trigger: Some(trigger), ..Layers::default() };
        let display = format!("{prefix}{}", d.name);
        decks.push(DeckSpec::new(display.clone(), deck_key(&display), deck_art(&d), DeckLayers::Layers(layers)));
    }
    let opts = BuildOptions { item_cache: false, extra_ebx: extra, extra_ops: sprite.ops, config_res: sprite.config_res, ..BuildOptions::default() };
    let report = build::build(game, &decks, &[], mods_dir, mod_name, &opts, log)?;
    Ok(FxBuild { report, states, own, sprite_key: sprite.key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_guids_in_the_games_style() {
        let (f, i) = state_ref("Kickflip");
        assert_eq!(f, i);
        let text = f.to_string();
        let hex: String = text.chars().filter(|c| *c != '-').collect();
        assert_eq!(&hex[..16], &hex[16..], "{text}");
        assert_ne!(state_ref("Kickflip"), state_ref("Heelflip"));
        assert_eq!(state_ref("Kickflip"), state_ref("Kickflip"));
        assert_eq!(state_name("Trick.Any"), "Animation/Caveman/FX.Trick.Any");
    }

    #[test]
    fn decks_fire_on_known_states() {
        for d in DECKS {
            assert!(STATES.contains(&d.state), "{}", d.name);
            assert!(d.usage == Usage::Continuous || d.cooldown > 0.15, "{}: a one-shot needs a cooldown above the pulse", d.name);
            let art = deck_art(&d);
            assert_eq!((art.width, art.height, art.channels), (512, 2048, 3));
        }
        assert_eq!(deck_key("000 - FX Test Aura"), "Own_000_-_FX_Test_Aura");
    }

    #[test]
    fn library_decks() {
        let mut keys = std::collections::HashSet::new();
        for d in DECKS.iter().chain(LIBRARY.iter()) {
            assert!(keys.insert(deck_key(&d.name.to_lowercase())), "{}: a name used twice", d.name);
        }
        for c in COMBOS {
            assert!(keys.insert(deck_key(&c.name.to_lowercase())));
        }
        for d in LIBRARY {
            assert!(d.name.starts_with("FXL "), "{}", d.name);
            assert_eq!(d.state, LIBRARY_STATE, "{}: fired by fx.test", d.name);
            assert!(STATES.contains(&d.state));
            assert!(d.usage == Usage::Continuous || d.cooldown > 0.15, "{}: a one-shot needs a cooldown above the pulse", d.name);
            if let FxLook::Own(short, _) = d.look {
                assert!(TUNED.iter().any(|t| t.name == short) || SPRITE.name == short, "{}: no blueprint {short}", d.name);
            }
            // every character of the art's lines is in the font
            for line in d.label {
                assert!(line.chars().all(|c| c == ' ' || glyph(c) != [0; 7]), "{}: {line:?} has a character the font lacks", d.name);
            }
        }
        let stock = LIBRARY.iter().filter(|d| matches!(d.look, FxLook::Stock(..))).count();
        assert!((10..=20).contains(&stock), "{stock} stock effects");
        assert_ne!(own_guid("a", "file"), own_guid("b", "file"));
        assert_eq!(own_guid("a", "file"), own_guid("a", "file"));
    }

    #[test]
    fn star_sheet_cells() {
        let s = star_sheet(256);
        assert_eq!((s.width, s.height, s.channels), (256, 256, 4));
        // each cell's middle is opaque fill, its corners transparent
        for (cx, cy) in [(64usize, 64usize), (192, 64), (64, 192), (192, 192)] {
            assert_eq!(s.pixel(cx, cy + 10)[3], 255, "cell at {cx},{cy}");
            assert_eq!(s.pixel(cx - 63, cy - 63)[3], 0);
        }
        assert_eq!(&s.pixel(64, 74)[..3], &[255, 220, 30], "the yellow star's fill");
    }
}
