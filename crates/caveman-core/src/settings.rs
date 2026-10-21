//! A deck's settings file (docs/settings.md): `Cool Deck.caveman.json` beside `Cool Deck.png`. When it is there it
//! sets the deck's effects (and optionally its name) instead of the filename suffixes; the app writes it. New with the
//! Rust engine: the Python toolkit only knows suffixes (`caveman.read_folder`).

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::anim;
use crate::effects::{self, BoardEffect, DeckEffects, Finish, FoilMode, Layers, NyanTrigger};
use crate::image::Image;

/// The schema version this engine reads and writes.
pub const VERSION: u32 = 1;
/// What replaces `.png` in a deck's file name to name its settings file.
pub const EXTENSION: &str = ".caveman.json";
/// Defaults of the shader settings: the cycle speed (turns a second) and the glow strength (0 to 1) the suffixes
/// `cycle` and `glow` give (effects::FX_SPEED and FX_GLOW as fractions of the marker byte).
pub const DEFAULT_SPEED: f64 = 0.25;
pub const DEFAULT_GLOW: f64 = effects::FX_GLOW as f64 / 255.0;

/// A deck's settings. Every part is optional; an empty file (`{"version": 1}`) is a plain deck.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub version: u32,
    /// The deck's name in game (a subfolder's prefix still goes first). Absent: the file's name, with the effects
    /// named after it as for suffixes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<FinishSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emboss: Option<EmbossSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader: Option<ShaderSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<BoardSettings>,
    /// The deck's own textures, used as they are instead of the ones the finish and emboss would make (what `caveman
    /// import` writes for a deck taken from another mod). Absent: the effects make them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<LayerSettings>,
    /// An animated deck (a GIF or video file instead of a PNG): how the clip becomes frames (anim.rs,
    /// knowledge/animated-decks.md). Ignored on a PNG.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<AnimationSettings>,
}

/// How an animated deck's clip becomes its frames: `fps` (2 to 60; absent: the clip's own rate), `frames` at most
/// (2 to 254, default 64; a longer clip plays at a lower rate so the whole loop fits), `size` of a frame in the atlas
/// (`auto`: the largest that fits the budget; `full` 512 x 2048, `half`, `quarter`), `budget_mib` the atlas's first
/// mip at most (1 to 32, default 16; the mips add a third) and `seconds` of a video at most (default 30).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AnimationSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<FrameSizeSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_mib: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameSizeSetting {
    Auto,
    Full,
    Half,
    Quarter,
}

impl FrameSizeSetting {
    fn frame_size(self) -> anim::FrameSize {
        match self {
            FrameSizeSetting::Auto => anim::FrameSize::Auto,
            FrameSizeSetting::Full => anim::FrameSize::Full,
            FrameSizeSetting::Half => anim::FrameSize::Half,
            FrameSizeSetting::Quarter => anim::FrameSize::Quarter,
        }
    }
}

/// Texture files for a deck's slots, relative to the deck PNG's folder: `osk` the mask (R occlusion, G gloss, B metal),
/// `ny` the normal map, `rgb` the holo map (red where holo) and `foilmode` its pattern (default pearl). Any size; they
/// are resized to the deck's 512 x 2048.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LayerSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub osk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ny: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foilmode: Option<HoloMode>,
}

impl LayerSettings {
    /// The files named, in slot order (osk, ny, rgb).
    pub fn files(&self) -> Vec<(&'static str, &str)> {
        [("osk", &self.osk), ("ny", &self.ny), ("rgb", &self.rgb)].into_iter().filter_map(|(s, f)| f.as_deref().map(|f| (s, f))).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FinishKind {
    Gloss,
    Matte,
    Foil,
    Chrome,
    Glitter,
    Holo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HoloMode {
    Pearl,
    Cellular,
    Frost,
    Vertical,
    Glitter,
}

/// The deck's finish: one of the even finishes, glitter, or holo foil in one of the game's five patterns; `spot` puts
/// it on the light parts of the art only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinishSettings {
    pub kind: FinishKind,
    /// Holo only (default pearl).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<HoloMode>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub spot: bool,
}

/// Raised print: the light parts of the art stand up from the deck, at this strength (default 4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbossSettings {
    #[serde(default = "default_emboss")]
    pub strength: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ShaderEffect {
    #[default]
    None,
    Cycle,
}

/// Caveman's shader effects (they need the Caveman-Shaders mod): the colours cycling through the hues at `speed`
/// turns a second, and the print glowing at `glow` (0 off to 1 the strongest).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShaderSettings {
    #[serde(default)]
    pub effect: ShaderEffect,
    #[serde(default = "default_speed")]
    pub speed: f64,
    #[serde(default)]
    pub glow: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BoardKind {
    #[default]
    None,
    Aura,
    Nyan,
}

/// An effect attached to the board: an aura (a light and trails, `colour` "#rrggbb" or absent for the art's most vivid
/// colour) or the solid rainbow trail (`steps`, default 8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardSettings {
    #[serde(default)]
    pub effect: BoardKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<f64>,
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn default_emboss() -> f64 {
    effects::EMBOSS_STRENGTH
}
fn default_speed() -> f64 {
    DEFAULT_SPEED
}

/// A settings file that cannot be used, and why.
#[derive(Debug)]
pub struct SettingsError(pub String);

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SettingsError {}

fn err<T>(message: impl Into<String>) -> Result<T, SettingsError> {
    Err(SettingsError(message.into()))
}

/// A deck file's name without its image or clip extension (`.png`, `.gif`, a video's), any case.
pub fn art_stem(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    for ext in [".png", ".gif"].into_iter().chain(anim::VIDEO_EXTENSIONS.iter().map(|e| match *e {
        "mp4" => ".mp4",
        "webm" => ".webm",
        "mov" => ".mov",
        "mkv" => ".mkv",
        "avi" => ".avi",
        _ => ".m4v",
    })) {
        if lower.ends_with(ext) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// `Cool Deck.png` -> `Cool Deck.caveman.json` (any `.png` case; a GIF or video deck likewise).
pub fn sidecar_path(png: &Path) -> PathBuf {
    let name = png.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = art_stem(&name);
    png.with_file_name(format!("{stem}{EXTENSION}"))
}

/// Whether a file name is a settings file.
pub fn is_sidecar(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().ends_with(EXTENSION)
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { version: VERSION, name: None, finish: None, emboss: None, shader: None, board: None, layers: None, animation: None }
    }
}

impl Settings {
    /// Reads and checks a settings file's text.
    pub fn parse(text: &str) -> Result<Settings, SettingsError> {
        let s: Settings = serde_json::from_str(text).map_err(|e| SettingsError(format!("not valid settings: {e}")))?;
        s.validate()?;
        Ok(s)
    }

    /// The settings file beside a deck PNG, if there is one.
    pub fn read_sidecar(png: &Path) -> Result<Option<Settings>, SettingsError> {
        let path = sidecar_path(png);
        match std::fs::read_to_string(&path) {
            Ok(text) => Settings::parse(&text).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// The settings as pretty JSON (what the app writes).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Checks the version and the ranges.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.version != VERSION {
            return err(format!("settings version {} (this Caveman reads version {VERSION})", self.version));
        }
        if let Some(f) = &self.finish {
            if f.mode.is_some() && f.kind != FinishKind::Holo {
                return err("finish.mode is for holo only");
            }
        }
        if let Some(e) = &self.emboss {
            if !(e.strength > 0.0 && e.strength <= 20.0) {
                return err(format!("emboss.strength {} (use more than 0, at most 20)", e.strength));
            }
        }
        if let Some(s) = &self.shader {
            if !(0.0..=2.0).contains(&s.speed) {
                return err(format!("shader.speed {} (turns a second, 0 to 2)", s.speed));
            }
            if !(0.0..=1.0).contains(&s.glow) {
                return err(format!("shader.glow {} (0 to 1)", s.glow));
            }
        }
        if let Some(l) = &self.layers {
            if self.finish.is_some() || self.emboss.is_some() {
                return err("layers replaces finish and emboss (use one or the other)");
            }
            for (slot, file) in l.files() {
                let p = Path::new(file);
                if file.trim().is_empty() || p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_))) {
                    return err(format!("layers.{slot} {file:?} (a file name relative to the deck's folder)"));
                }
            }
            if l.foilmode.is_some() && l.rgb.is_none() {
                return err("layers.foilmode needs layers.rgb (the holo map)");
            }
        }
        if let Some(a) = &self.animation {
            if let Some(fps) = a.fps {
                if !(fps >= anim::MIN_FPS as f64 && fps <= anim::MAX_FPS as f64) {
                    return err(format!("animation.fps {fps} ({} to {} a second)", anim::MIN_FPS, anim::MAX_FPS));
                }
            }
            if let Some(n) = a.frames {
                if !(2..=anim::MAX_FRAMES).contains(&n) {
                    return err(format!("animation.frames {n} (2 to {})", anim::MAX_FRAMES));
                }
            }
            if let Some(m) = a.budget_mib {
                if !(1..=(anim::MAX_BUDGET >> 20) as u32).contains(&m) {
                    return err(format!("animation.budget_mib {m} (1 to {})", anim::MAX_BUDGET >> 20));
                }
            }
            if let Some(s) = a.seconds {
                if !(s >= 0.5 && s <= 600.0) {
                    return err(format!("animation.seconds {s} (0.5 to 600)"));
                }
            }
        }
        if let Some(b) = &self.board {
            if let Some(c) = &b.colour {
                if parse_colour(c).is_none() {
                    return err(format!("board.colour {c:?} (use \"#rrggbb\")"));
                }
                if b.effect != BoardKind::Aura {
                    return err("board.colour is for the aura only");
                }
            }
            if let Some(steps) = b.steps {
                if b.effect != BoardKind::Nyan {
                    return err("board.steps is for nyan only");
                }
                if !(steps > 0.0 && steps <= 256.0) {
                    return err(format!("board.steps {steps} (more than 0, at most 256)"));
                }
            }
        }
        Ok(())
    }

    /// The effects these settings give the deck.
    pub fn effects(&self) -> DeckEffects {
        let mut e = DeckEffects::default();
        if let Some(f) = &self.finish {
            e.finish = Some(match f.kind {
                FinishKind::Gloss => Finish::Gloss,
                FinishKind::Matte => Finish::Matte,
                FinishKind::Foil => Finish::Foil,
                FinishKind::Chrome => Finish::Chrome,
                FinishKind::Glitter => Finish::Glitter,
                FinishKind::Holo => Finish::Holo(match f.mode.unwrap_or(HoloMode::Pearl) {
                    HoloMode::Pearl => FoilMode::Pearl,
                    HoloMode::Cellular => FoilMode::Cellular,
                    HoloMode::Frost => FoilMode::Frost,
                    HoloMode::Vertical => FoilMode::Vertical,
                    HoloMode::Glitter => FoilMode::Glitter,
                }),
            });
            e.spot = f.spot;
        }
        e.emboss = self.emboss.as_ref().map(|x| x.strength);
        if let Some(s) = &self.shader {
            e.cycle = s.effect == ShaderEffect::Cycle;
            e.glow = s.glow > 0.0;
        }
        if let Some(b) = &self.board {
            e.board = match b.effect {
                BoardKind::None => None,
                BoardKind::Aura => Some(BoardEffect::Aura(b.colour.as_deref().and_then(parse_colour))),
                BoardKind::Nyan => Some(BoardEffect::Nyan { steps: b.steps.unwrap_or(effects::NYAN_STEPS), trigger: NyanTrigger::Ratio }),
            };
        }
        e
    }

    /// The settings that give these effects (to seed a settings file from a deck's suffixes).
    pub fn from_effects(e: &DeckEffects) -> Settings {
        let finish = e.finish.map(|f| {
            let (kind, mode) = match f {
                Finish::Gloss => (FinishKind::Gloss, None),
                Finish::Matte => (FinishKind::Matte, None),
                Finish::Foil => (FinishKind::Foil, None),
                Finish::Chrome => (FinishKind::Chrome, None),
                Finish::Glitter => (FinishKind::Glitter, None),
                Finish::Holo(m) => (
                    FinishKind::Holo,
                    Some(match m {
                        FoilMode::Pearl => HoloMode::Pearl,
                        FoilMode::Cellular => HoloMode::Cellular,
                        FoilMode::Frost => HoloMode::Frost,
                        FoilMode::Vertical => HoloMode::Vertical,
                        FoilMode::Glitter => HoloMode::Glitter,
                    }),
                ),
            };
            FinishSettings { kind, mode, spot: e.spot }
        });
        let shader = (e.cycle || e.glow).then_some(ShaderSettings {
            effect: if e.cycle { ShaderEffect::Cycle } else { ShaderEffect::None },
            speed: DEFAULT_SPEED,
            glow: if e.glow { DEFAULT_GLOW } else { 0.0 },
        });
        let board = e.board.map(|b| match b {
            BoardEffect::Aura(c) => BoardSettings { effect: BoardKind::Aura, colour: c.map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])), steps: None },
            BoardEffect::Nyan { steps, .. } => BoardSettings { effect: BoardKind::Nyan, colour: None, steps: Some(steps) },
        });
        Settings { version: VERSION, name: None, finish, emboss: e.emboss.map(|strength| EmbossSettings { strength }), shader, board, layers: None, animation: None }
    }

    /// How an animated deck's clip becomes frames: the `animation` settings over anim's defaults.
    pub fn policy(&self) -> anim::Policy {
        let mut p = anim::Policy::default();
        if let Some(a) = &self.animation {
            p.fps = a.fps;
            if let Some(n) = a.frames {
                p.max_frames = n;
            }
            if let Some(s) = a.size {
                p.size = s.frame_size();
            }
            if let Some(m) = a.budget_mib {
                p.budget = (m as usize) << 20;
            }
            if let Some(s) = a.seconds {
                p.max_seconds = s;
            }
        }
        p
    }

    /// The effect words a deck with these settings is named by when the settings give no name, in the order the
    /// suffixes are listed (mixins.EFFECTS): the finish, then spot, emboss, aura, nyan, cycle, glow. Lower case,
    /// hyphenated, as suffixes are.
    pub fn effect_words(&self) -> Vec<String> {
        let e = self.effects();
        let mut words: Vec<String> = e.finish.map(|f| f.suffix().to_string()).into_iter().collect();
        if e.spot && e.finish.is_some() {
            words.push("spot".into());
        }
        if e.emboss.is_some() {
            words.push("emboss".into());
        }
        match e.board {
            Some(BoardEffect::Aura(_)) => words.push("aura".into()),
            Some(BoardEffect::Nyan { .. }) => words.push("nyan".into()),
            None => {}
        }
        if e.cycle {
            words.push("cycle".into());
        }
        if e.glow {
            words.push("glow".into());
        }
        words
    }

    /// The aura's colour when the settings name one.
    pub fn colour(&self) -> Option<[u8; 3]> {
        self.board.as_ref().and_then(|b| b.colour.as_deref()).and_then(parse_colour)
    }

    /// The shader marker's speed and glow bytes (shaderpatch.marker_pixels block 2: G turns at 2 G / 255 a second;
    /// B the glow strength), or None for no shader effect.
    pub fn marker_bytes(&self) -> Option<(u8, u8)> {
        let s = self.shader.as_ref()?;
        if s.effect == ShaderEffect::None && s.glow <= 0.0 {
            return None;
        }
        Some(((s.speed * 255.0 / 2.0).round().clamp(0.0, 255.0) as u8, (s.glow * 255.0).round().clamp(0.0, 255.0) as u8))
    }

    /// Whether the marker differs from the one the `cycle` and `glow` suffixes give (then the layers must be made
    /// here, with [`Settings::layers`], rather than from the effects alone).
    pub fn custom_marker(&self) -> bool {
        let e = self.effects();
        match self.marker_bytes() {
            None => false,
            Some((speed, glow)) => speed != effects::FX_SPEED || glow != if e.glow { effects::FX_GLOW } else { 0 },
        }
    }

    /// The deck's layers (effects::layers), with these settings' cycle speed and glow strength in the marker.
    pub fn layers(&self, art: &Image, default_mask: &Image, default_normal: &Image) -> Layers {
        let e = self.effects();
        let mut out = effects::layers(&e, art, default_mask, default_normal);
        if let (Some((speed, glow)), Some(rgb)) = (self.marker_bytes(), out.rgb.as_ref()) {
            let effect = if e.cycle { effects::EFFECT_CYCLE } else { effects::EFFECT_NONE };
            out.rgb = Some(effects::mark(rgb, effect, speed, glow));
        }
        out
    }

    /// Whether the build must be given this deck's layers (made here) rather than its effects: a custom shader
    /// marker, or texture files of its own.
    pub fn needs_layers(&self) -> bool {
        self.custom_marker() || self.layers.is_some()
    }

    /// The texture files these settings name, beside the deck PNG `png` (to hash them with the deck: a changed file
    /// rebuilds it).
    pub fn layer_paths(&self, png: &Path) -> Vec<PathBuf> {
        let dir = png.parent().unwrap_or(Path::new("."));
        self.layers.as_ref().map(|l| l.files().into_iter().map(|(_, f)| dir.join(f)).collect()).unwrap_or_default()
    }

    /// The deck's layers: [`Settings::layers`], with the texture files `layers` names (beside the deck PNG `png`) in
    /// place of made ones. A holo map without a mask gets the default mask (the holo host's own is its graphic's);
    /// the shader marker, when the settings ask for one, goes into the given holo map.
    pub fn custom_layers(&self, png: &Path, art: &Image, default_mask: &Image, default_normal: &Image) -> Result<Layers, SettingsError> {
        let mut out = self.layers(art, default_mask, default_normal);
        let Some(l) = &self.layers else { return Ok(out) };
        let dir = png.parent().unwrap_or(Path::new("."));
        let load = |file: &str, normal: bool| -> Result<Image, SettingsError> {
            let path = dir.join(file);
            let img = Image::read_png(&path).map_err(|e| SettingsError(format!("layer {}: {e}", path.display())))?.to_rgb();
            Ok(fit_layer(&img, normal))
        };
        if let Some(f) = &l.osk {
            out.osk = Some(load(f, false)?);
        }
        if let Some(f) = &l.ny {
            out.ny = Some(load(f, true)?);
        }
        if let Some(f) = &l.rgb {
            let mut rgb = load(f, false)?;
            if let Some((speed, glow)) = self.marker_bytes() {
                let effect = if self.effects().cycle { effects::EFFECT_CYCLE } else { effects::EFFECT_NONE };
                rgb = effects::mark(&rgb, effect, speed, glow);
            }
            out.rgb = Some(rgb);
            out.foilmode = Some(match l.foilmode.unwrap_or(HoloMode::Pearl) {
                HoloMode::Pearl => FoilMode::Pearl,
                HoloMode::Cellular => FoilMode::Cellular,
                HoloMode::Frost => FoilMode::Frost,
                HoloMode::Vertical => FoilMode::Vertical,
                HoloMode::Glitter => FoilMode::Glitter,
            });
            if out.osk.is_none() {
                out.osk = Some(default_mask.to_rgb());
            }
        }
        Ok(out)
    }
}

/// A layer texture at the deck's 512 x 2048 (Lanczos when it is another size; a normal map made unit length again, as
/// build::mip_chain does for its smaller mips).
fn fit_layer(img: &Image, normal: bool) -> Image {
    if (img.width, img.height) == (effects::W, effects::H) {
        return img.clone();
    }
    let mut out = img.resize_lanczos(effects::W, effects::H);
    if normal {
        for px in out.data.chunks_exact_mut(out.channels) {
            let n = [px[0] as f64 / 127.5 - 1.0, px[1] as f64 / 127.5 - 1.0, px[2] as f64 / 127.5 - 1.0];
            let norm = ((n[0] * n[0] + n[1] * n[1]) + n[2] * n[2]).sqrt().max(1e-6);
            for c in 0..3 {
                px[c] = ((n[c] / norm + 1.0) * 127.5).round_ties_even().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// "#rrggbb" or "rrggbb".
pub fn parse_colour(text: &str) -> Option<[u8; 3]> {
    effects::hex_colour(text.strip_prefix('#').unwrap_or(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_effects_round_trip() {
        let words: Vec<String> = ["holo-frost", "spot", "emboss", "cycle", "glow"].iter().map(|s| s.to_string()).collect();
        let (e, _, _) = DeckEffects::from_suffixes(&words, None);
        let s = Settings::from_effects(&e);
        assert_eq!(s.effects(), e);
        assert!(!s.custom_marker());
        assert_eq!(Settings::parse(&s.to_json()).unwrap(), s);
        assert_eq!(s.effect_words(), words);
    }

    #[test]
    fn checks() {
        assert!(Settings::parse(r#"{"version": 2}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "finsh": {}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "finish": {"kind": "foil", "mode": "frost"}}"#).is_err());
        assert!(Settings::parse(r##"{"version": 1, "board": {"effect": "aura", "colour": "#00ffff"}}"##).is_ok());
        assert!(Settings::parse(r##"{"version": 1, "board": {"effect": "aura", "colour": "teal"}}"##).is_err());
        let s = Settings::parse(r#"{"version": 1, "shader": {"effect": "cycle", "speed": 1.0, "glow": 0.25}}"#).unwrap();
        assert_eq!(s.marker_bytes(), Some((128, 64)));
        assert!(s.custom_marker());
    }

    #[test]
    fn layers_schema() {
        let s = Settings::parse(r#"{"version": 1, "name": "D", "layers": {"osk": ".layers/D.mask.png", "rgb": ".layers/D.holo.png", "foilmode": "frost"}}"#).unwrap();
        assert!(s.needs_layers());
        assert_eq!(s.layer_paths(Path::new(r"x\D.png")), vec![PathBuf::from(r"x\.layers/D.mask.png"), PathBuf::from(r"x\.layers/D.holo.png")]);
        assert_eq!(Settings::parse(&s.to_json()).unwrap(), s);
        // replaces finish and emboss; relative paths only; a foil mode needs a holo map; no unknown fields
        assert!(Settings::parse(r#"{"version": 1, "finish": {"kind": "foil"}, "layers": {"osk": "m.png"}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "emboss": {}, "layers": {"ny": "n.png"}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "layers": {"osk": "C:\\m.png"}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "layers": {"osk": "../m.png"}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "layers": {"foilmode": "frost"}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "layers": {"mask": "m.png"}}"#).is_err());
        // shader and board still apply with layers
        assert!(Settings::parse(r##"{"version": 1, "board": {"effect": "aura", "colour": "#ff0000"}, "layers": {"osk": "m.png"}}"##).is_ok());
        // no layers: nothing changes for decks without them
        assert!(!Settings::parse(r#"{"version": 1}"#).unwrap().needs_layers());
    }

    #[test]
    fn custom_layers_load() {
        let dir = std::env::temp_dir().join(format!("caveman-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".layers")).unwrap();
        // a 256 x 1024 mask (resized to the deck's 512 x 2048) and a full-size holo map
        Image::filled(256, 1024, &[200, 100, 50]).write_png(dir.join(".layers/D.mask.png")).unwrap();
        Image::filled(512, 2048, &[255, 0, 0]).write_png(dir.join(".layers/D.holo.png")).unwrap();
        let png = dir.join("D.png");
        let art = Image::filled(512, 2048, &[10, 20, 30]);
        let (mask, normal) = (Image::filled(512, 2048, &[255, 211, 0, 255]), Image::filled(512, 2048, &[127, 127, 255, 255]));
        let s = Settings::parse(r#"{"version": 1, "layers": {"osk": ".layers/D.mask.png", "rgb": ".layers/D.holo.png", "foilmode": "frost"}}"#).unwrap();
        let l = s.custom_layers(&png, &art, &mask, &normal).unwrap();
        let osk = l.osk.unwrap();
        assert_eq!((osk.width, osk.height, osk.channels), (512, 2048, 3));
        assert_eq!(osk.pixel(100, 100), &[200, 100, 50]);
        assert_eq!(l.rgb.unwrap().pixel(0, 0), &[255, 0, 0]);
        assert_eq!(l.foilmode, Some(FoilMode::Frost));
        assert!(l.ny.is_none());
        // a holo map alone takes the default mask; the foil mode defaults to pearl
        let s = Settings::parse(r#"{"version": 1, "layers": {"rgb": ".layers/D.holo.png"}}"#).unwrap();
        let l = s.custom_layers(&png, &art, &mask, &normal).unwrap();
        assert_eq!(l.osk.unwrap().pixel(0, 0), &[255, 211, 0]);
        assert_eq!(l.foilmode, Some(FoilMode::Pearl));
        // a missing file is an error naming it
        let s = Settings::parse(r#"{"version": 1, "layers": {"ny": ".layers/none.png"}}"#).unwrap();
        assert!(s.custom_layers(&png, &art, &mask, &normal).unwrap_err().0.contains("none.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecar_names() {
        assert_eq!(sidecar_path(Path::new(r"a\Cool Deck.foil.PNG")), PathBuf::from(r"a\Cool Deck.foil.caveman.json"));
        assert_eq!(sidecar_path(Path::new(r"a\Cool.gif")), PathBuf::from(r"a\Cool.caveman.json"));
        assert_eq!(sidecar_path(Path::new(r"a\Cool.MP4")), PathBuf::from(r"a\Cool.caveman.json"));
        assert_eq!(art_stem("Cool.webm"), "Cool");
        assert!(is_sidecar("Cool Deck.Caveman.JSON"));
    }

    #[test]
    fn animation_settings() {
        let s = Settings::parse(r#"{"version": 1, "animation": {"fps": 12, "frames": 32, "size": "half", "budget_mib": 8}}"#).unwrap();
        let p = s.policy();
        assert_eq!((p.fps, p.max_frames, p.size, p.budget), (Some(12.0), 32, anim::FrameSize::Half, 8 << 20));
        assert_eq!(Settings::parse(&s.to_json()).unwrap(), s);
        assert_eq!(Settings::parse(r#"{"version": 1}"#).unwrap().policy().max_frames, anim::DEFAULT_MAX_FRAMES);
        assert!(Settings::parse(r#"{"version": 1, "animation": {"fps": 1}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "animation": {"frames": 300}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "animation": {"size": "huge"}}"#).is_err());
        assert!(Settings::parse(r#"{"version": 1, "animation": {"budget_mib": 64}}"#).is_err());
    }
}
