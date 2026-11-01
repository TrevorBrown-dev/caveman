//! Deck effects (`mixins.py`, `vfx.py`): finishes, holo, spot, emboss and the shader marker as layer textures for
//! the deck's other slots, and the board effects (aura, nyan) as edits of cloned effect documents.
//!
//! The mask `param_graphic_osk` is R occlusion, G smoothness (gloss; stock decks about 211), B metal; the normal map
//! `param_graphic_ny` is flat unless the print is raised; the holo map `param_graphic_rgb` is red where the deck is
//! holo (and carries Caveman's shader marker in its top-left corner). All are 512 x 2048 like the art. The numbers
//! repeat numpy's float64 steps and its rounding (half to even), and glitter uses numpy's own seeded generator
//! (`np.random.default_rng(seed)`: SeedSequence and PCG64, ported below), so the textures match the Python's.

// Index loops mirror the Python's array indexing (and numpy's evaluation order) on purpose.
#![allow(clippy::needless_range_loop)]

use std::collections::HashMap;

use crate::ebx::{self, Data, Document, Guid, Object, Pointer, Value};
use crate::image::Image;

/// Width and height of a deck's art and layer textures.
pub const W: usize = 512;
pub const H: usize = 2048;
/// The default mask's mean smoothness (mixins.STOCK_GLOSS).
pub const STOCK_GLOSS: f64 = 211.0;
/// Cycle speed and glow strength written into the shader marker (mixins.FX_SPEED, FX_GLOW).
pub const FX_SPEED: u8 = 32;
pub const FX_GLOW: u8 = 128;
/// The shader marker's effect codes (shaderpatch.EFFECT_*).
pub const EFFECT_NONE: u8 = 0;
pub const EFFECT_CYCLE: u8 = 64;
pub const EFFECT_STATIC: u8 = 128;
pub const EFFECT_PULSE: u8 = 192;
pub const EFFECT_NEON: u8 = 240;
/// Animated deck (anim.rs): the holo map is a frame atlas and block 3 holds the frame count, rate and columns. Below
/// the cycle threshold (32), so the hue code leaves the colours alone.
pub const EFFECT_ANIM: u8 = 16;
/// The emboss strength (mixins.normal_map's default).
pub const EMBOSS_STRENGTH: f64 = 4.0;

// ---------------------------------------------------------------- the typed effects

/// The deck shader's holo patterns (`param_foilmode`; newdeck.FOIL_MODES).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FoilMode {
    Cellular,
    Frost,
    Pearl,
    Glitter,
    Vertical,
}

impl FoilMode {
    /// The enumerator of `common/shadercomponents/configuration/enum/foilmode` a holo preset imports.
    pub fn enumerator(self) -> Guid {
        Guid::parse(match self {
            FoilMode::Cellular => "5ecf14cd-091a-41c0-81f6-9caaff2b1c9c",
            FoilMode::Frost => "e04615d5-ea8d-416e-acc5-a3b1b9f989e8",
            FoilMode::Pearl => "b7d407a4-b44c-42e2-b91e-877d817c872d",
            FoilMode::Glitter => "77c5e80f-6036-48b5-afe4-319772124a52",
            FoilMode::Vertical => "7a67636f-9767-473b-95c7-b174736504ad",
        })
        .unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            FoilMode::Cellular => "Cellular",
            FoilMode::Frost => "Frost",
            FoilMode::Pearl => "Pearl",
            FoilMode::Glitter => "Glitter",
            FoilMode::Vertical => "Vertical",
        }
    }
}

/// One finish per deck (mixins.FINISHES and HOLO).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Finish {
    Gloss,
    Matte,
    Foil,
    Chrome,
    /// Speckled metal flakes.
    Glitter,
    /// The deck shader's holo foil in one of its patterns.
    Holo(FoilMode),
}

impl Finish {
    /// The finish a suffix names (`gloss`, `holo-frost`, ...).
    pub fn from_suffix(s: &str) -> Option<Finish> {
        Some(match s {
            "gloss" => Finish::Gloss,
            "matte" => Finish::Matte,
            "foil" => Finish::Foil,
            "chrome" => Finish::Chrome,
            "glitter" => Finish::Glitter,
            "holo" => Finish::Holo(FoilMode::Pearl),
            "holo-cellular" => Finish::Holo(FoilMode::Cellular),
            "holo-frost" => Finish::Holo(FoilMode::Frost),
            "holo-vertical" => Finish::Holo(FoilMode::Vertical),
            "holo-glitter" => Finish::Holo(FoilMode::Glitter),
            _ => return None,
        })
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Finish::Gloss => "gloss",
            Finish::Matte => "matte",
            Finish::Foil => "foil",
            Finish::Chrome => "chrome",
            Finish::Glitter => "glitter",
            Finish::Holo(FoilMode::Pearl) => "holo",
            Finish::Holo(FoilMode::Cellular) => "holo-cellular",
            Finish::Holo(FoilMode::Frost) => "holo-frost",
            Finish::Holo(FoilMode::Vertical) => "holo-vertical",
            Finish::Holo(FoilMode::Glitter) => "holo-glitter",
        }
    }

    /// (smoothness, metal) of the even finishes.
    fn even(self) -> Option<(f64, f64)> {
        match self {
            Finish::Gloss => Some((245.0, 0.0)),
            Finish::Matte => Some((70.0, 0.0)),
            Finish::Foil => Some((215.0, 255.0)),
            Finish::Chrome => Some((250.0, 255.0)),
            _ => None,
        }
    }
}

/// How the nyan ribbon's colour switches on the rainbow (vfx.nyan_colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NyanTrigger {
    /// Red exactly nyanpatch.RATIO x green: the one that works in game.
    Ratio,
    /// Red -1: showed no trail at all in game (a diagnostic).
    Negative,
    /// Plain orange, no rainbow (a diagnostic).
    Control,
}

/// One effect attached to the board (an item has one `SkaterVfx`): an aura (a light and trails) or a nyan trail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoardEffect {
    /// sRGB colour; None takes the art's most vivid colour (mixins.art_colour).
    Aura(Option<[u8; 3]>),
    Nyan {
        steps: f64,
        trigger: NyanTrigger,
    },
}

/// Everything a deck can carry besides its art: what the sidecar JSON and the filename suffixes describe.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DeckEffects {
    pub finish: Option<Finish>,
    /// The finish only on the light parts of the art.
    pub spot: bool,
    /// Raise the light parts of the art, at this strength (mixins default 4).
    pub emboss: Option<f64>,
    /// Caveman's shader effects: the hue cycle and the glow (marker in the holo map).
    pub cycle: bool,
    pub glow: bool,
    pub board: Option<BoardEffect>,
}

/// Every suffix word Caveman knows (mixins.EFFECTS), in mixins' order.
pub const SUFFIXES: [&str; 16] = [
    "gloss",
    "matte",
    "foil",
    "chrome",
    "glitter",
    "holo",
    "holo-cellular",
    "holo-frost",
    "holo-vertical",
    "holo-glitter",
    "spot",
    "emboss",
    "aura",
    "nyan",
    "cycle",
    "glow",
];

/// Whether `word` (lower case) is an effect suffix.
pub fn is_suffix(word: &str) -> bool {
    SUFFIXES.contains(&word)
}

/// (finish, modifiers, notes) from suffixes in the order written (mixins.split): one finish per deck (a second is
/// left out with a note), `spot` needs a finish, one board effect per deck (nyan wins over aura).
pub fn split(effects: &[String]) -> (Option<String>, Vec<String>, Vec<String>) {
    let (mut finish, mut mods, mut notes): (Option<String>, Vec<String>, Vec<String>) = (None, Vec::new(), Vec::new());
    for e in effects {
        if Finish::from_suffix(e).is_some() {
            match &finish {
                None => finish = Some(e.clone()),
                Some(f) => notes.push(format!("one finish per deck: kept \"{f}\", left out \"{e}\"")),
            }
        } else if !mods.contains(e) {
            mods.push(e.clone());
        }
    }
    if mods.iter().any(|m| m == "spot") && finish.is_none() {
        notes.push("\"spot\" needs a finish (such as foil or holo); left out".to_string());
        mods.retain(|m| m != "spot");
    }
    if mods.iter().any(|m| m == "nyan") && mods.iter().any(|m| m == "aura") {
        notes.push("one board effect per deck: kept \"nyan\", left out \"aura\"".to_string());
        mods.retain(|m| m != "aura");
    }
    (finish, mods, notes)
}

impl DeckEffects {
    /// The effects of filename suffixes (`Cool Deck.holo-frost.spot.png` -> ["holo-frost", "spot"]) and a hex colour
    /// among them. Returns the effects, the effect words in the order caveman.py names the deck by (finish first, then
    /// the modifiers as written), and notes for what was left out.
    pub fn from_suffixes(words: &[String], colour: Option<[u8; 3]>) -> (DeckEffects, Vec<String>, Vec<String>) {
        let (finish, mods, notes) = split(words);
        let mut e = DeckEffects { finish: finish.as_deref().and_then(Finish::from_suffix), ..DeckEffects::default() };
        for m in &mods {
            match m.as_str() {
                "spot" => e.spot = true,
                "emboss" => e.emboss = Some(EMBOSS_STRENGTH),
                "cycle" => e.cycle = true,
                "glow" => e.glow = true,
                "nyan" => e.board = Some(BoardEffect::Nyan { steps: NYAN_STEPS, trigger: NyanTrigger::Ratio }),
                "aura" if !matches!(e.board, Some(BoardEffect::Nyan { .. })) => e.board = Some(BoardEffect::Aura(colour)),
                _ => {}
            }
        }
        let mut names: Vec<String> = finish.into_iter().collect();
        names.extend(mods);
        (e, names, notes)
    }
}

/// (r, g, b) of a 6-digit hex colour such as `ff00ff`, or None (mixins.hex_colour).
pub fn hex_colour(text: &str) -> Option<[u8; 3]> {
    if text.len() != 6 || !text.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&text[i..i + 2], 16).ok();
    Some([v(0)?, v(2)?, v(4)?])
}

// ---------------------------------------------------------------- numpy's seeded generator

/// numpy's `SeedSequence(seed)` pool and `generate_state` (bit_generator.pyx), for one non-negative seed.
fn seed_state(seed: u64, words64: usize) -> Vec<u64> {
    const INIT_A: u32 = 0x43b0_d7e5;
    const MULT_A: u32 = 0x931e_8875;
    const INIT_B: u32 = 0x8b51_f9dd;
    const MULT_B: u32 = 0x58f3_8ded;
    const MIX_MULT_L: u32 = 0xca01_f9dd;
    const MIX_MULT_R: u32 = 0x4973_f715;
    const XSHIFT: u32 = 16;
    let hashmix = |value: u32, hash_const: &mut u32| {
        let mut value = value ^ *hash_const;
        *hash_const = hash_const.wrapping_mul(MULT_A);
        value = value.wrapping_mul(*hash_const);
        value ^ (value >> XSHIFT)
    };
    let mix = |x: u32, y: u32| {
        let r = MIX_MULT_L.wrapping_mul(x).wrapping_sub(MIX_MULT_R.wrapping_mul(y));
        r ^ (r >> XSHIFT)
    };
    // the seed as little-endian 32-bit words (at least one)
    let mut entropy = Vec::new();
    let mut s = seed;
    loop {
        entropy.push(s as u32);
        s >>= 32;
        if s == 0 {
            break;
        }
    }
    let mut pool = [0u32; 4];
    let mut hash_const = INIT_A;
    for (i, p) in pool.iter_mut().enumerate() {
        *p = hashmix(entropy.get(i).copied().unwrap_or(0), &mut hash_const);
    }
    for i_src in 0..4 {
        for i_dst in 0..4 {
            if i_src != i_dst {
                let h = hashmix(pool[i_src], &mut hash_const);
                pool[i_dst] = mix(pool[i_dst], h);
            }
        }
    }
    for &e in entropy.iter().skip(4) {
        for i_dst in 0..4 {
            let h = hashmix(e, &mut hash_const);
            pool[i_dst] = mix(pool[i_dst], h);
        }
    }
    let mut hash_const = INIT_B;
    let mut words = Vec::with_capacity(words64 * 2);
    for i in 0..words64 * 2 {
        let mut v = pool[i % 4] ^ hash_const;
        hash_const = hash_const.wrapping_mul(MULT_B);
        v = v.wrapping_mul(hash_const);
        words.push(v ^ (v >> XSHIFT));
    }
    words.chunks(2).map(|w| w[0] as u64 | (w[1] as u64) << 32).collect()
}

/// numpy's PCG64 (XSL-RR) seeded as `np.random.default_rng(seed)`.
pub struct NumpyRng {
    state: u128,
    inc: u128,
}

const PCG_MULT: u128 = (2_549_297_995_355_413_924u128 << 64) + 4_865_540_595_714_422_341u128;

impl NumpyRng {
    pub fn new(seed: u64) -> NumpyRng {
        let v = seed_state(seed, 4);
        let initstate = ((v[0] as u128) << 64) | v[1] as u128;
        let initseq = ((v[2] as u128) << 64) | v[3] as u128;
        let mut rng = NumpyRng { state: 0, inc: (initseq << 1) | 1 };
        rng.step();
        rng.state = rng.state.wrapping_add(initstate);
        rng.step();
        rng
    }

    fn step(&mut self) {
        self.state = self.state.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
    }

    /// The next raw 64-bit output.
    pub fn next_u64(&mut self) -> u64 {
        self.step();
        let hi = (self.state >> 64) as u64;
        let lo = self.state as u64;
        (hi ^ lo).rotate_right((hi >> 58) as u32)
    }

    /// The next double in [0, 1) (`Generator.random`).
    pub fn random(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}

/// Random values 0-1, constant over size x size squares (one value per glitter flake) (mixins._flakes): a
/// `width` x `height` row-major field.
fn flakes(width: usize, height: usize, size: usize, seed: u64) -> Vec<f64> {
    let (sw, sh) = (width.div_ceil(size), height.div_ceil(size));
    let mut rng = NumpyRng::new(seed);
    let small: Vec<f64> = (0..sw * sh).map(|_| rng.random()).collect();
    let mut out = vec![0.0; width * height];
    for y in 0..height {
        for x in 0..width {
            out[y * width + x] = small[(y / size) * sw + x / size];
        }
    }
    out
}

// ---------------------------------------------------------------- per-pixel helpers

/// numpy's `np.round` then `clip(0, 255)` to a byte.
fn byte(v: f64) -> u8 {
    v.round_ties_even().clamp(0.0, 255.0) as u8
}

/// A fused multiply-add, `a x b + c` rounded once: the CPU's FMA instruction. (`f64::mul_add` without the `fma`
/// target feature calls the C library's `fma`, which on the GNU toolchain is not fused.)
fn fma(a: f64, b: f64, c: f64) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("fma") {
            // SAFETY: the CPU supports FMA (checked above).
            return unsafe { fma_hw(a, b, c) };
        }
    }
    a.mul_add(b, c)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "fma")]
unsafe fn fma_hw(a: f64, b: f64, c: f64) -> f64 {
    use std::arch::x86_64::{_mm_cvtsd_f64, _mm_fmadd_sd, _mm_set_sd};
    _mm_cvtsd_f64(_mm_fmadd_sd(_mm_set_sd(a), _mm_set_sd(b), _mm_set_sd(c)))
}

/// 0-1 lightness of the art (Rec. 709 weights on the sRGB values) (mixins.luma): `width` x `height` row-major.
/// numpy's `a @ w` here runs through BLAS with fused multiply-adds, in this order (measured against numpy with
/// Python's math.fma: 0 differences; a plain sum differs in the last bit for a fifth of the pixels, which flips
/// emboss heights).
pub fn luma(art: &Image) -> Vec<f64> {
    let rgb = art.to_rgb();
    rgb.data
        .chunks_exact(3)
        .map(|p| {
            let (r, g, b) = (p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0);
            fma(b, 0.0722, fma(r, 0.2126, g * 0.7152))
        })
        .collect()
}

fn smoothstep(lo: f64, hi: f64, x: f64) -> f64 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 0-1 weight of the light parts of the art: where a `spot` finish goes (mixins.spot).
pub fn spot(art: &Image) -> Vec<f64> {
    luma(art).into_iter().map(|l| smoothstep(0.55, 0.75, l)).collect()
}

/// The deck's mask for a finish (mixins.mask): RGB 512 x 2048. `default`: the game's default mask; `weight`: where
/// the finish goes (0-1 per pixel, None for everywhere); elsewhere the default stays.
pub fn mask(finish: Finish, default: &Image, weight: Option<&[f64]>) -> Image {
    let d = default.to_rgb();
    let n = d.width * d.height;
    let (glitter_metal, glitter_gloss) =
        if finish == Finish::Glitter { (flakes(d.width, d.height, 2, 1), flakes(d.width, d.height, 2, 2)) } else { (Vec::new(), Vec::new()) };
    let mut out = Image::new(d.width, d.height, 3);
    for i in 0..n {
        let dv = [d.data[i * 3] as f64, d.data[i * 3 + 1] as f64, d.data[i * 3 + 2] as f64];
        let occlusion = dv[0];
        let grain = dv[1] - STOCK_GLOSS;
        let (gloss, metal) = match finish {
            Finish::Glitter => (190.0 + 60.0 * glitter_gloss[i], 180.0 + 75.0 * glitter_metal[i]),
            Finish::Holo(_) => (255.0, 0.0),
            f => {
                let (smooth, metal) = f.even().unwrap();
                (smooth + grain, metal)
            }
        };
        let mut px = [occlusion, gloss, metal];
        if let Some(w) = weight {
            for c in 0..3 {
                px[c] = dv[c] + (px[c] - dv[c]) * w[i];
            }
        }
        for c in 0..3 {
            out.data[i * 3 + c] = byte(px[c]);
        }
    }
    out
}

/// The holo map's idle red: 0, no holo. The shader multiplies its iridescence by the holo map's red (a live level
/// per texel, 0 to full), gated by the mask's metal; a deck without a holo finish ships black, `finish holo` writes
/// red. (A model of 2026-10-22 midday had the game choosing a shader variant at equip by this map's content, with a
/// re-equip between a metal and a holo finish: wrong. The material's parameters are the same either way; the holo
/// showed late because the in-world board's copies of the mask and the normal map were written raw instead of as
/// the game encodes them, see [`ridden_normal`] and [`ridden_mask`]. Every write is live, no re-equip.)
pub const HOLO_IDLE_RED: u8 = 0;

/// The metal (B) the game's compositor puts into a holo deck's composited mask (read back live 2026-10-22: 25 on and
/// off the deck, where our source mask had 0), and which the world shader multiplies the iridescence by: a mask copy
/// written with B 0 switches the holo off whatever the holo map says. A live holo mask carries at least this.
pub const HOLO_METAL_FLOOR: u8 = 25;

/// The holo map of a deck with no holo: the idle red everywhere (see [`HOLO_IDLE_RED`]).
pub fn holo_idle() -> Image {
    Image::filled(W, H, &[HOLO_IDLE_RED, 0, 0])
}

/// The `param_graphic_rgb` texture: red 255 where the deck is holo (mixins.holo_map).
pub fn holo_map(weight: Option<&[f64]>) -> Image {
    let mut out = Image::new(W, H, 3);
    for i in 0..W * H {
        out.data[i * 3] = byte(255.0 * weight.map_or(1.0, |w| w[i]));
    }
    out
}

/// The holo map with Caveman's shader marker painted into its top-left corner, off the deck (mixins.mark,
/// shaderpatch.marker_pixels): magenta, green, then (effect, speed, glow), 16 x 16 blocks.
pub fn mark(holo: &Image, effect: u8, speed: u8, glow: u8) -> Image {
    let mut out = holo.to_rgb();
    let blocks = [(0usize, [255u8, 0, 255]), (16, [0, 255, 0]), (32, [effect, speed, glow])];
    for (x0, colour) in blocks {
        for y in 0..16 {
            for x in x0..x0 + 16 {
                out.pixel_mut(x, y).copy_from_slice(&colour);
            }
        }
    }
    out
}

/// numpy's `np.gradient` of a row-major field: (d/dy, d/dx), central differences inside, one-sided at the edges.
fn gradient(f: &[f64], width: usize, height: usize) -> (Vec<f64>, Vec<f64>) {
    let (mut gy, mut gx) = (vec![0.0; f.len()], vec![0.0; f.len()]);
    let at = |x: usize, y: usize| f[y * width + x];
    for y in 0..height {
        for x in 0..width {
            gy[y * width + x] = if height < 2 {
                0.0
            } else if y == 0 {
                (at(x, 1) - at(x, 0)) / 1.0
            } else if y == height - 1 {
                (at(x, y) - at(x, y - 1)) / 1.0
            } else {
                (at(x, y + 1) - at(x, y - 1)) / 2.0
            };
            gx[y * width + x] = if width < 2 {
                0.0
            } else if x == 0 {
                (at(1, y) - at(0, y)) / 1.0
            } else if x == width - 1 {
                (at(x, y) - at(x - 1, y)) / 1.0
            } else {
                (at(x + 1, y) - at(x - 1, y)) / 2.0
            };
        }
    }
    (gy, gx)
}

/// Tangent-space X and Y slopes as a unit normal in RGB, flat (127.5, 127.5, 255) (mixins.encode_normal).
pub fn encode_normal(nx: &[f64], ny: &[f64], width: usize, height: usize) -> Image {
    let mut out = Image::new(width, height, 3);
    for i in 0..width * height {
        let norm = (nx[i] * nx[i] + ny[i] * ny[i] + 1.0).sqrt();
        let v = [nx[i] / norm, ny[i] / norm, 1.0 / norm];
        for c in 0..3 {
            out.data[i * 3 + c] = byte((v[c] + 1.0) * 127.5);
        }
    }
    out
}

/// The in-world board's normal map as the game builds it from a deck's normal map (the nameless RGBA8 UNORM copy
/// two ids after the ridden art copy): per texel, the source RGB decoded as a signed vector and normalised, biased
/// back to 0..1 and normalised AGAIN, stored as plain UNORM (so a flat 128,128,255 lands as 104,104,208, and any
/// texel with blue under 128 as 180,180,0). Measured 2026-10-22 by writing gradients into The Caveman's `_ny` and
/// peeking the copy the game rebuilt at the next equip: 52 texels, every one within 1.1 of this formula, the same
/// value at every position (no spatial term). A copy written with the raw bytes instead gives the in-world board a
/// mis-encoded normal map: chrome and metal then look flat in game while the pause menu, which samples the source
/// itself, shows them right.
pub fn ridden_normal(src: &Image) -> Image {
    let mut out = src.clone();
    let ch = out.channels;
    if ch < 3 {
        return out;
    }
    for px in out.data.chunks_mut(ch) {
        let mut n = [0f64; 3];
        for c in 0..3 {
            n[c] = px[c] as f64 / 255.0 * 2.0 - 1.0;
        }
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            for v in &mut n {
                *v /= len;
            }
        }
        let w = [n[0] * 0.5 + 0.5, n[1] * 0.5 + 0.5, n[2] * 0.5 + 0.5];
        let len = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
        for c in 0..3 {
            px[c] = byte(if len > 0.0 { w[c] / len } else { w[c] } * 255.0);
        }
    }
    out
}

/// The in-world board's mask as the game builds it from a deck's mask (the copy one id after the ridden art copy):
/// occlusion and smoothness as they are, the metal channel lifted a tenth of the way to full (blue 4 lands as 29, 128
/// as 141: never under 25). Measured 2026-10-22 as [`ridden_normal`] was, on the same equip.
pub fn ridden_mask(src: &Image) -> Image {
    let mut out = src.clone();
    let ch = out.channels;
    if ch >= 3 {
        for px in out.data.chunks_mut(ch) {
            px[2] = byte(px[2] as f64 * 0.9 + 25.5);
        }
    }
    out
}

/// A layer as the in-world board's copy of it must be written: the normal map and the mask re-encoded as the game's
/// compositor does ([`ridden_normal`], [`ridden_mask`]); the art and the holo map as they are.
pub fn ridden_layer(slot: &str, img: &Image) -> Image {
    match slot {
        "ny" => ridden_normal(img),
        "osk" => ridden_mask(img),
        _ => img.clone(),
    }
}

/// The deck's normal map, or None when nothing raises the surface (mixins.normal_map): `emboss` raises the light
/// parts of the art at its strength; glitter tilts each flake a little (where `weight`).
pub fn normal_map(emboss: Option<f64>, glitter: bool, art: &Image, default: &Image, weight: Option<&[f64]>) -> Option<Image> {
    if emboss.is_none() && !glitter {
        return None;
    }
    let d = default.to_rgb();
    let (w, h) = (d.width, d.height);
    let mut nx: Vec<f64> = d.data.chunks_exact(3).map(|p| p[0] as f64 / 127.5 - 1.0).collect();
    let mut ny: Vec<f64> = d.data.chunks_exact(3).map(|p| p[1] as f64 / 127.5 - 1.0).collect();
    if let Some(strength) = emboss {
        let l = luma(art);
        let height = Image { width: w, height: h, channels: 1, data: l.iter().map(|v| (v * 255.0) as u8).collect() };
        let height = height.gaussian_blur(2.0);
        let hf: Vec<f64> = height.data.iter().map(|&v| v as f64 / 255.0).collect();
        let (gy, gx) = gradient(&hf, w, h);
        for i in 0..w * h {
            nx[i] -= strength * gx[i];
            ny[i] -= strength * gy[i];
        }
    }
    if glitter {
        let (fx, fy) = (flakes(w, h, 2, 3), flakes(w, h, 2, 4));
        for i in 0..w * h {
            let k = weight.map_or(1.0, |wt| wt[i]);
            nx[i] += 0.6 * (2.0 * fx[i] - 1.0) * k;
            ny[i] += 0.6 * (2.0 * fy[i] - 1.0) * k;
        }
    }
    Some(encode_normal(&nx, &ny, w, h))
}

// ---------------------------------------------------------------- the art's colour

/// Pillow's bicubic filter (a = -0.5), support 2.
fn bicubic(x: f64) -> f64 {
    let a = -0.5;
    let x = x.abs();
    if x < 1.0 {
        ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
    } else if x < 2.0 {
        (((x - 5.0) * x + 8.0) * x - 4.0) * a
    } else {
        0.0
    }
}

/// Pillow `resize(size)` (its default BICUBIC filter, 8 bits per channel, Resample.c): horizontal then vertical.
pub fn resize_bicubic(img: &Image, width: usize, height: usize) -> Image {
    const PRECISION_BITS: u32 = 32 - 8 - 2;
    fn coefficients(in_size: usize, out_size: usize) -> (Vec<(usize, usize)>, Vec<i32>, usize) {
        let scale = in_size as f32 as f64 / out_size as f64;
        let filterscale = scale.max(1.0);
        let support = 2.0 * filterscale;
        let ksize = support.ceil() as usize * 2 + 1;
        let mut bounds = Vec::with_capacity(out_size);
        let mut kk = vec![0f64; out_size * ksize];
        for xx in 0..out_size {
            let center = (xx as f64 + 0.5) * scale;
            let ss = 1.0 / filterscale;
            let xmin = ((center - support + 0.5) as i64).max(0);
            let xmax = ((center + support + 0.5) as i64).min(in_size as i64) - xmin;
            let k = &mut kk[xx * ksize..(xx + 1) * ksize];
            let mut ww = 0.0;
            for x in 0..xmax as usize {
                let w = bicubic((x as f64 + xmin as f64 - center + 0.5) * ss);
                k[x] = w;
                ww += w;
            }
            for v in k.iter_mut().take(xmax as usize) {
                if ww != 0.0 {
                    *v /= ww;
                }
            }
            bounds.push((xmin as usize, xmax as usize));
        }
        let scale_fixed = (1u32 << PRECISION_BITS) as f64;
        let fixed = kk.iter().map(|&v| if v < 0.0 { (-0.5 + v * scale_fixed) as i32 } else { (0.5 + v * scale_fixed) as i32 }).collect();
        (bounds, fixed, ksize)
    }
    let clip8 = |v: i32| (v >> PRECISION_BITS).clamp(0, 255) as u8;
    let c = img.channels;
    let mut cur = img.clone();
    if width != img.width {
        let (bounds, k, ksize) = coefficients(img.width, width);
        let mut out = Image::new(width, cur.height, c);
        for y in 0..cur.height {
            for (xx, &(xmin, n)) in bounds.iter().enumerate() {
                for ch in 0..c {
                    let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                    for x in 0..n {
                        ss += cur.data[(y * cur.width + x + xmin) * c + ch] as i32 * k[xx * ksize + x];
                    }
                    out.data[(y * width + xx) * c + ch] = clip8(ss);
                }
            }
        }
        cur = out;
    }
    if height != img.height {
        let (bounds, k, ksize) = coefficients(img.height, height);
        let w = cur.width;
        let mut out = Image::new(w, height, c);
        for (yy, &(ymin, n)) in bounds.iter().enumerate() {
            for x in 0..w * c {
                let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                for y in 0..n {
                    ss += cur.data[(y + ymin) * w * c + x] as i32 * k[yy * ksize + y];
                }
                out.data[yy * w * c + x] = clip8(ss);
            }
        }
        cur = out;
    }
    cur
}

fn rgb_to_hue(r: f64, g: f64, b: f64) -> f64 {
    let (maxc, minc) = (r.max(g).max(b), r.min(g).min(b));
    if minc == maxc {
        return 0.0;
    }
    let rangec = maxc - minc;
    let (rc, gc, bc) = ((maxc - r) / rangec, (maxc - g) / rangec, (maxc - b) / rangec);
    let h = if r == maxc {
        bc - gc
    } else if g == maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    (h / 6.0).rem_euclid(1.0)
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    if s == 0.0 {
        return [v, v, v];
    }
    let i = (h * 6.0) as i64; // int() truncates
    let f = h * 6.0 - i as f64;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i.rem_euclid(6) {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// The art's most vivid colour (the mean of its most saturated, bright pixels) at full brightness: what an aura
/// glows in when no colour is given; white for grey art (mixins.art_colour).
pub fn art_colour(art: &Image) -> [u8; 3] {
    let small = resize_bicubic(&art.to_rgb(), 64, 256);
    let px: Vec<[f64; 3]> = small.data.chunks_exact(3).map(|p| [p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0]).collect();
    let vivid: Vec<f64> = px
        .iter()
        .map(|p| {
            let (mx, mn) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
            (mx - mn) * mx
        })
        .collect();
    if vivid.iter().cloned().fold(f64::NEG_INFINITY, f64::max) < 0.15 {
        return [255, 255, 255];
    }
    // np.quantile(vivid, 0.9), the linear method with numpy's _lerp
    let mut sorted = vivid.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let virtual_index = (sorted.len() - 1) as f64 * 0.9;
    let prev = virtual_index.floor();
    let gamma = virtual_index - prev;
    let (a, b) = (sorted[prev as usize], sorted[(prev as usize + 1).min(sorted.len() - 1)]);
    let diff = b - a;
    let threshold = if gamma >= 0.5 { b - diff * (1.0 - gamma) } else { a + diff * gamma };
    let (mut sum, mut count) = ([0.0f64; 3], 0usize);
    for (p, v) in px.iter().zip(&vivid) {
        if *v >= threshold {
            for c in 0..3 {
                sum[c] += p[c];
            }
            count += 1;
        }
    }
    let top = sum.map(|s| s / count as f64);
    let h = rgb_to_hue(top[0], top[1], top[2]);
    hsv_to_rgb(h, 0.85, 1.0).map(|x| (255.0 * x).round_ties_even() as u8)
}

// ---------------------------------------------------------------- the layers a deck's effects make

/// What a deck's effects change (mixins.layers): its own mask, normal map and holo map (RGB 512 x 2048), the foil
/// mode of a holo deck, and a board effect with its colour resolved.
#[derive(Clone, Debug, Default)]
pub struct Layers {
    pub osk: Option<Image>,
    pub ny: Option<Image>,
    pub rgb: Option<Image>,
    pub foilmode: Option<FoilMode>,
    pub board: Option<ResolvedBoard>,
    /// The board effect fires on a game state instead of always (Caveman FX's decks; None for every other deck).
    pub trigger: Option<StateTrigger>,
    /// Several state-fired board effects on one deck (Caveman FX's combo decks; empty for every other deck). Used
    /// instead of `board` and `trigger`.
    pub combo: Vec<(ResolvedBoard, StateTrigger)>,
    /// Float parameters of the appearance preset set by ParamKey GUID (`build::PARAM_*`): The Caveman's iridescence
    /// amount at 1.0 (the holo host's is 0.25). A key the host's preset lacks fails the build.
    pub preset_floats: Vec<(String, f32)>,
}

/// A board effect ready to build: an aura's colour is known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolvedBoard {
    Aura([u8; 3]),
    Nyan {
        steps: f64,
        trigger: NyanTrigger,
    },
    /// One of the game's own effect blueprints, (file, instance) GUIDs, played from the middle of the board (a stock
    /// burst such as `ebp_chr_ctd_impact_a`; not copied).
    Burst((Guid, Guid)),
    /// An effect blueprint played as it is at chosen points on the board: one of the game's (shared bundle) or one the
    /// mod ships on its own (a retuned copy, a sprite effect: Caveman FX's library decks). Imported by GUID, not copied.
    Stock {
        blueprint: (Guid, Guid),
        at: BoardPoints,
    },
    /// An aura with its strength set (light, trails); [`AuraTuning::STOCK`] is the plain aura.
    AuraTuned {
        colour: [u8; 3],
        tuning: AuraTuning,
    },
}

/// Where on the board an effect plays: attach points in board space (x across, y up, z along, metres).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoardPoints {
    /// The middle of the board, 7 cm up (the nyan and burst point).
    Centre,
    /// Both trucks (the aura's two points).
    Trucks,
    /// The four wheels (Kichi Aura Wheels' offsets).
    Wheels,
    /// Nose and tail, at deck height.
    NoseTail,
    /// The middle of the board on the ground.
    Ground,
}

/// Kichi Aura Wheels' wheel offsets (knowledge/glow.md), at the aura's height.
pub const WHEEL_POINTS: [[f64; 3]; 4] = [[0.1, 0.03, 0.205], [-0.1, 0.03, 0.205], [0.1, 0.03, -0.205], [-0.1, 0.03, -0.205]];
/// Nose and tail: the deck is about 0.8 m long, its top about 10 cm up.
pub const NOSE_TAIL_POINTS: [[f64; 3]; 2] = [[0.0, 0.1, 0.38], [0.0, 0.1, -0.38]];
pub const GROUND_POINTS: [[f64; 3]; 1] = [[0.0, 0.0, 0.0]];

impl BoardPoints {
    pub fn points(self) -> &'static [[f64; 3]] {
        match self {
            BoardPoints::Centre => &NYAN_POINTS,
            BoardPoints::Trucks => &POINTS,
            BoardPoints::Wheels => &WHEEL_POINTS,
            BoardPoints::NoseTail => &NOSE_TAIL_POINTS,
            BoardPoints::Ground => &GROUND_POINTS,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            BoardPoints::Centre => "centre",
            BoardPoints::Trucks => "trucks",
            BoardPoints::Wheels => "wheels",
            BoardPoints::NoseTail => "nose and tail",
            BoardPoints::Ground => "ground",
        }
    }
}

/// An aura's strength (knowledge/effects-catalog.md, "Intensity knobs"): the light's intensity and reach, the trails'
/// colour scale (HDR: above 1 is brighter), width and length in time. [`AuraTuning::STOCK`] is the plain aura's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuraTuning {
    /// `PbrSphereLightEntityData.Intensity` (the oni sword light 150, Kichi 13, the plain aura 40).
    pub light_intensity: f64,
    /// `AttenuationRadius`, metres: how far the light pool reaches.
    pub light_radius: f64,
    /// The trail colour is the aura colour times this (its brightness).
    pub trail_boost: f64,
    /// The trail's second colour is the aura colour times this.
    pub trail_dim: f64,
    /// Scales each trail's width (PropertyId 226981187; the template's are 0.1 and 0.05 m); None leaves them.
    pub trail_width: Option<f64>,
    /// Each trail's `ParticleLifeSpan`, seconds (the trail's length in time; the template's are 0.15 and 0.1 s); None
    /// leaves them.
    pub trail_life: Option<f64>,
}

impl AuraTuning {
    /// The plain aura (Caveman's decks and the FX decks): exactly what [`aura_blueprint`] has always written.
    pub const STOCK: AuraTuning = AuraTuning {
        light_intensity: LIGHT_INTENSITY,
        light_radius: LIGHT_RADIUS,
        trail_boost: TRAIL_BOOST,
        trail_dim: TRAIL_DIM,
        trail_width: None,
        trail_life: None,
    };
}

/// How to make a stock effect bigger, brighter, longer or denser: a retuned copy of a blueprint ([`retune_blueprint`]).
/// Every field multiplies the blueprint's own values; 1 leaves them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectTuning {
    /// Spawn rate, initial and maximum particle count of each emitter (where the blueprint overrides them).
    pub count: f64,
    /// Particle size (PropertyId -711019869, a min and max in metres) and ribbon width (226981187).
    pub size: f64,
    /// A particle size the scaling does not go past, metres (0: no limit): flash sprites already a metre wide stay
    /// near that while sparks a centimetre wide grow. A size already above it is left as it is.
    pub max_size: f64,
    /// Particle and emitter life (seconds; an emitter life of 0, endless, stays 0).
    pub life: f64,
    /// The colour parameters (-1752990270, -1752990271: HDR rgb, so above 1 glows brighter).
    pub brightness: f64,
    /// Each light's intensity.
    pub light_intensity: f64,
    /// Each light's reach (`AttenuationRadius`).
    pub light_radius: f64,
    /// Each light's lifetime (a lifetime of 0, endless, stays 0).
    pub light_life: f64,
}

impl EffectTuning {
    pub const NONE: EffectTuning =
        EffectTuning { count: 1.0, size: 1.0, max_size: 0.0, life: 1.0, brightness: 1.0, light_intensity: 1.0, light_radius: 1.0, light_life: 1.0 };

    /// A size scaled as this tuning says (`max_size` applied).
    pub fn scale_size(&self, v: f64) -> f64 {
        let scaled = v * self.size;
        if self.max_size > 0.0 && scaled > self.max_size {
            v.max(self.max_size)
        } else {
            scaled
        }
    }
}

/// How a state-fired board effect plays (`TriggerOutputUsage`, knowledge/state-effects-runtime.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    /// The effect lives while the state is true.
    Continuous,
    /// The effect starts when the state is true and the cooldown has run out.
    OneOff,
}

/// A board effect fired by a game state (TriggerType 6; statefx.trigger): the state's (file, instance) GUIDs, how it
/// plays, and the seconds between two one-off firings (`CooldownType` Time; above 0, since 0 fires every frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StateTrigger {
    pub state: (Guid, Guid),
    pub usage: Usage,
    pub cooldown: f64,
}

/// `SkaterVfxEffectItemTrigger_Gamestate`.
pub const GAME_STATE: i64 = 6;

/// Makes the effect item of a board-effect asset clone fire on a game state (statefx.trigger): TriggerType 6, the
/// usage, a time cooldown, and an import of the state (by GUID: the state must live where the shared bundle can reach
/// it, never the root level).
pub fn trigger(doc: &mut Document, t: &StateTrigger) -> ebx::Result<()> {
    if t.usage == Usage::OneOff && t.cooldown <= 0.0 {
        return Err(err("a one-off trigger needs a cooldown above 0 (with 0 it fires every frame)"));
    }
    let item = *doc.instances_of("SkaterVfxEffectItem").first().ok_or_else(|| err("no SkaterVfxEffectItem"))?;
    let state = doc.add_import(t.state);
    let o = doc.object_mut(item)?;
    o.set("TriggerType", Data::Int(GAME_STATE))?;
    o.set("TriggerOutputUsage", Data::Int(if t.usage == Usage::OneOff { 1 } else { 0 }))?;
    o.set("CooldownType", Data::Int(0))?;
    o.set("CooldownTime", Data::F64(t.cooldown))?;
    o.set("TriggerEffectsGameState.AssetRef", state)?;
    Ok(())
}

/// The layers of a deck's effects over `art` (512 x 2048 RGB); `default_mask` and `default_normal` are the game's
/// default deck textures (newdeck.default_layers) (mixins.layers).
pub fn layers(effects: &DeckEffects, art: &Image, default_mask: &Image, default_normal: &Image) -> Layers {
    let board = match effects.board {
        Some(BoardEffect::Nyan { steps, trigger }) => Some(ResolvedBoard::Nyan { steps, trigger }),
        Some(BoardEffect::Aura(colour)) => Some(ResolvedBoard::Aura(colour.unwrap_or_else(|| art_colour(art)))),
        None => None,
    };
    // `trigger` stays None: only Caveman FX's decks (fxmod.rs) carry one
    let mut out = Layers { board, ..Layers::default() };
    let weight = if effects.spot && effects.finish.is_some() { Some(spot(art)) } else { None };
    if let Some(finish) = effects.finish {
        out.osk = Some(mask(finish, default_mask, weight.as_deref()));
        if let Finish::Holo(mode) = finish {
            out.rgb = Some(holo_map(weight.as_deref()));
            out.foilmode = Some(mode);
        }
    }
    if effects.cycle || effects.glow {
        let holo = out.rgb.take().unwrap_or_else(|| Image::new(W, H, 3));
        let effect = if effects.cycle { EFFECT_CYCLE } else { EFFECT_NONE };
        out.rgb = Some(mark(&holo, effect, FX_SPEED, if effects.glow { FX_GLOW } else { 0 }));
        if out.osk.is_none() {
            out.osk = Some(default_mask.to_rgb()); // the holo host's own mask is its graphic's
        }
    }
    let glitter = effects.finish == Some(Finish::Glitter);
    out.ny = normal_map(effects.emboss, glitter, art, default_normal, weight.as_deref());
    out
}

// ---------------------------------------------------------------- board effects (vfx.py)

/// The game's oni costume effects every board effect is cloned from (vfx.BLUEPRINT, vfx.ASSET).
pub const BLUEPRINT: &str = "effects/characters/effectblueprints/ebp_chr_oni_sword_light";
pub const ASSET: &str = "characters/vfx/skfx_cst_oni2026vfx";
/// Where Caveman's effect documents go (vfx.PREFIX).
pub const VFX_PREFIX: &str = "characters/vfx/caveman/";
const TRAIL_GRAPH: &str = "b48e76d9-4a88-46c6-a49f-26b2b1ae3d70";
const HELPER_GRAPH: &str = "bf1101c3-b8cc-11ef-b925-bbfa60fddb5c";
/// Emitter PropertyIds: the trail colour and second colour (HDR rgb), the ribbon width. The colour ids are shared by
/// the flipbook sprite graphs too (knowledge/effects-catalog.md).
pub const TRAIL_COLOUR: i64 = -1_752_990_270;
pub const TRAIL_COLOUR2: i64 = -1_752_990_271;
pub const WIDTH: i64 = 226_981_187;
/// The flipbook sprite graphs' particle size: (min, max) in metres (x, y), shared by every flipbook graph.
pub const SPRITE_SIZE: i64 = -711_019_869;
const TRAIL_BOOST: f64 = 1.5;
const TRAIL_DIM: f64 = 0.25;
const LIGHT_INTENSITY: f64 = 40.0;
const LIGHT_RADIUS: f64 = 0.9;
/// Attach points in board space (x across, y up, z along, metres): the trucks for an aura, the middle for nyan.
pub const POINTS: [[f64; 3]; 2] = [[0.0, 0.03, 0.205], [0.0, 0.03, -0.205]];
/// 7 cm up: at 3 cm the wide band dipped into the ground (Trevor, 2026-10-20); the deck's underside is about 10 cm up.
pub const NYAN_POINTS: [[f64; 3]; 1] = [[0.0, 0.07, 0.0]];
const BOARD: i64 = 1;
const NO_BONE: u64 = 0xFFFF_FFFF;
const NYAN_WIDTH: f64 = 0.28;
const NYAN_LIFE: f64 = 1.2;
/// The nyan step count (vfx.NYAN_STEPS) and the ratio trigger (nyanpatch.RATIO, RATIO_STEPS).
pub const NYAN_STEPS: f64 = 8.0;
pub const RATIO: f64 = 0.0371;
pub const RATIO_STEPS: f64 = 64.0;

/// 0-255 sRGB to 0-1 linear (vfx.linear).
pub fn linear(rgb: [u8; 3]) -> [f64; 3] {
    rgb.map(|c| {
        let c = c as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    })
}

fn err(message: impl Into<String>) -> ebx::Error {
    ebx::Error(message.into())
}

/// vfx._set_vec: a Vec3/Vec4 field's x, y, z(, w) set to floats.
fn set_vec(obj: &mut Object, field: &str, values: &[f64]) -> ebx::Result<()> {
    let vec = obj.find_mut(field).and_then(|f| f.value.data.as_object_mut()).ok_or_else(|| err(format!("no vector {field}")))?;
    for (name, v) in ["x", "y", "z", "w"].iter().zip(values) {
        vec.find_mut(name).ok_or_else(|| err(format!("no {field}.{name}")))?.value.data = Data::F64(*v);
    }
    Ok(())
}

fn graph_file(doc: &Document, obj: &Object) -> Option<Guid> {
    match obj.find("EmitterGraph")?.value.data.as_pointer()? {
        Pointer::External(i) => doc.imports.get(i).map(|r| r.0),
        _ => None,
    }
}

fn internal(v: &Value) -> ebx::Result<usize> {
    match v.data.as_pointer() {
        Some(Pointer::Internal(i)) => Ok(i),
        other => Err(err(format!("not an internal pointer: {other:?}"))),
    }
}

/// Rewrites a blueprint's EffectEntityData Components: `decide(doc, instance, kept so far)` returns whether to keep a
/// component, after editing it.
fn edit_components(doc: &mut Document, mut decide: impl FnMut(&mut Document, usize, usize) -> ebx::Result<bool>) -> ebx::Result<()> {
    let effect = *doc.instances_of("EffectEntityData").first().ok_or_else(|| err("no EffectEntityData"))?;
    let components: Vec<Value> = doc.object(effect)?.find("Components").and_then(|f| f.value.data.as_array()).ok_or_else(|| err("no Components"))?.clone();
    let mut keep = Vec::new();
    for v in components {
        let i = internal(&v)?;
        if decide(doc, i, keep.len())? {
            keep.push(v);
        }
    }
    *doc.object_mut(effect)?.find_mut("Components").unwrap().value.data.as_array_mut().unwrap() = keep;
    Ok(())
}

/// Edits a clone of BLUEPRINT for an aura (vfx.blueprint): light and trails in `colour` (sRGB), trails at the attach
/// point, the helper emitter dropped.
pub fn aura_blueprint(doc: &mut Document, colour: [u8; 3]) -> ebx::Result<()> {
    aura_blueprint_tuned(doc, colour, &AuraTuning::STOCK)
}

/// [`aura_blueprint`] at a chosen strength: the light's intensity and reach, the trails' brightness, and optionally
/// their width and length. With [`AuraTuning::STOCK`] it writes exactly what `aura_blueprint` always has.
pub fn aura_blueprint_tuned(doc: &mut Document, colour: [u8; 3], tuning: &AuraTuning) -> ebx::Result<()> {
    let c = linear(colour);
    let (trail, helper) = (Guid::parse(TRAIL_GRAPH)?, Guid::parse(HELPER_GRAPH)?);
    let (boost, dim) = (tuning.trail_boost, tuning.trail_dim);
    edit_components(doc, |doc, i, _| {
        if doc.instance_type(i) == "EmitterGraphEntityData" {
            let gf = graph_file(doc, doc.object(i)?);
            if gf == Some(helper) {
                return Ok(false);
            }
            if gf == Some(trail) {
                let o = doc.object_mut(i)?;
                set_vec(o.find_mut("Transform").and_then(|f| f.value.data.as_object_mut()).ok_or_else(|| err("no Transform"))?, "trans", &[0.0, 0.0, 0.0])?;
                for prm in o.find_mut("EmitterGraphParams").and_then(|f| f.value.data.as_array_mut()).ok_or_else(|| err("no EmitterGraphParams"))? {
                    let po = prm.data.as_object_mut().ok_or_else(|| err("a parameter is not an object"))?;
                    let pid = po.find("PropertyId").and_then(|f| f.value.data.as_i64());
                    if pid == Some(TRAIL_COLOUR) {
                        set_vec(po, "Value", &[c[0] * boost, c[1] * boost, c[2] * boost, 0.0])?;
                    } else if pid == Some(TRAIL_COLOUR2) {
                        set_vec(po, "Value", &[c[0] * dim, c[1] * dim, c[2] * dim, 0.0])?;
                    } else if let (Some(WIDTH), Some(scale)) = (pid, tuning.trail_width) {
                        let w = po.get("Value.x")?.as_f64().ok_or_else(|| err("a width that is not a number"))?;
                        po.set("Value.x", Data::F64(w * scale))?;
                    }
                }
                if let Some(life) = tuning.trail_life {
                    for q in ["Low", "Medium", "High", "Ultra"] {
                        o.set(&format!("EmitterGraphOverrides.ParticleLifeSpan.{q}"), Data::F64(life))?;
                    }
                }
            }
        }
        Ok(true)
    })?;
    let light = *doc.instances_of("PbrSphereLightEntityData").first().ok_or_else(|| err("no PbrSphereLightEntityData"))?;
    let lo = doc.object_mut(light)?;
    set_vec(lo, "Color", &c)?;
    lo.set("Intensity", Data::F64(tuning.light_intensity))?;
    lo.set("AttenuationRadius", Data::F64(tuning.light_radius))?;
    Ok(())
}

const QUALITY: [&str; 4] = ["Low", "Medium", "High", "Ultra"];

/// Multiplies a QualityScalableFloat or QualityScalableInt field at `path` (each quality level) by `k`; ints round
/// (and stay at least 1 when they were). Returns whether anything changed.
fn scale_quality(o: &mut Object, path: &str, k: f64) -> ebx::Result<bool> {
    if k == 1.0 {
        return Ok(false);
    }
    let mut changed = false;
    for q in QUALITY {
        let p = format!("{path}.{q}");
        let v = o.get(&p)?.clone();
        let new = match &v {
            Data::Int(n) => Data::Int(if *n == 0 { 0 } else { ((*n as f64 * k).round() as i64).max(1) }),
            Data::UInt(n) => Data::UInt(if *n == 0 { 0 } else { ((*n as f64 * k).round() as u64).max(1) }),
            other => Data::F64(other.as_f64().ok_or_else(|| err(format!("{p} is not a number")))? * k),
        };
        if new != v {
            changed = true;
        }
        o.set(&p, new)?;
    }
    Ok(changed)
}

/// The blueprint's live components of type `type_name`: the instances its EffectEntityData lists (a component left
/// out by [`keep_components`] or [`aura_blueprint`] stays in the document, unreferenced).
pub fn components_of(doc: &Document, type_name: &str) -> Vec<usize> {
    let listed: Vec<usize> = doc
        .instances_of("EffectEntityData")
        .first()
        .and_then(|&e| doc.object(e).ok())
        .and_then(|o| o.find("Components"))
        .and_then(|f| f.value.data.as_array())
        .map(|a| a.iter().filter_map(|v| internal(v).ok()).collect())
        .unwrap_or_default();
    listed.into_iter().filter(|&i| doc.instance_type(i).eq_ignore_ascii_case(type_name)).collect()
}

/// The light data (`PbrSphereLightEntityData` and the like) of the live light components.
fn live_lights(doc: &Document) -> Vec<usize> {
    components_of(doc, "LightEffectEntityData")
        .into_iter()
        .filter_map(|i| doc.object(i).ok().and_then(|o| o.find("Light")).and_then(|f| internal(&f.value).ok()))
        .collect()
}

/// Retunes a copy of an effect blueprint (bigger, brighter, longer, denser; knowledge/effects-catalog.md): in every
/// emitter the spawn rate and particle counts it overrides, particle and emitter life, particle size, ribbon width and
/// colour; in every light its intensity, reach and lifetime. Returns how many values changed.
pub fn retune_blueprint(doc: &mut Document, t: &EffectTuning) -> ebx::Result<usize> {
    let mut changed = 0usize;
    for i in components_of(doc, "EmitterGraphEntityData") {
        let o = doc.object_mut(i)?;
        let set = |o: &Object, flag: &str| -> bool { o.get(&format!("EmitterGraphOverrides.{flag}")).ok().and_then(|d| d.as_bool()).unwrap_or(false) };
        let (rate, max, init, life, elife) = (
            set(o, "IsSpawnRateOverrideSet"),
            set(o, "IsParticleMaxCountOverrideSet"),
            set(o, "IsInitialParticleCountOverrideSet"),
            set(o, "IsParticleLifeSpanOverrideSet"),
            set(o, "IsEmitterLifeSpanOverrideSet"),
        );
        for (on, field, k) in [
            (rate, "SpawnRate", t.count),
            (max, "ParticleMaxCount", t.count),
            (init, "InitialParticleCount", t.count),
            (life, "ParticleLifeSpan", t.life),
            (elife, "EmitterLifeSpan", t.life),
        ] {
            if on && scale_quality(o, &format!("EmitterGraphOverrides.{field}"), k)? {
                changed += 1;
            }
        }
        for prm in o.find_mut("EmitterGraphParams").and_then(|f| f.value.data.as_array_mut()).ok_or_else(|| err("no EmitterGraphParams"))? {
            let po = prm.data.as_object_mut().ok_or_else(|| err("a parameter is not an object"))?;
            let pid = po.find("PropertyId").and_then(|f| f.value.data.as_i64());
            let (axes, k, sized): (&[&str], f64, bool) = match pid {
                Some(SPRITE_SIZE) => (&["x", "y"], t.size, true),
                Some(WIDTH) => (&["x"], t.size, true),
                Some(TRAIL_COLOUR) | Some(TRAIL_COLOUR2) => (&["x", "y", "z"], t.brightness, false),
                _ => continue,
            };
            if k == 1.0 {
                continue;
            }
            for a in axes {
                let v = po.get(&format!("Value.{a}"))?.as_f64().ok_or_else(|| err("a parameter value that is not a number"))?;
                po.set(&format!("Value.{a}"), Data::F64(if sized { t.scale_size(v) } else { v * k }))?;
            }
            changed += 1;
        }
    }
    for i in components_of(doc, "LightEffectEntityData") {
        let o = doc.object_mut(i)?;
        let life = o.get("Lifetime")?.as_f64().unwrap_or(0.0);
        if life > 0.0 && t.light_life != 1.0 {
            o.set("Lifetime", Data::F64(life * t.light_life))?;
            changed += 1;
        }
    }
    for i in live_lights(doc) {
        let o = doc.object_mut(i)?;
        for (field, k) in [("Intensity", t.light_intensity), ("AttenuationRadius", t.light_radius)] {
            if k != 1.0 {
                let v = o.get(field)?.as_f64().ok_or_else(|| err(format!("{field} is not a number")))?;
                o.set(field, Data::F64(v * k))?;
                changed += 1;
            }
        }
    }
    Ok(changed)
}

/// Keeps only the blueprint components (lights, emitters) `keep(doc, instance)` accepts; returns how many are left.
pub fn keep_components(doc: &mut Document, mut keep: impl FnMut(&Document, usize) -> bool) -> ebx::Result<usize> {
    let mut kept = 0;
    edit_components(doc, |doc, i, _| {
        let k = keep(doc, i);
        kept += k as usize;
        Ok(k)
    })?;
    Ok(kept)
}

/// The material keys (`ExpressionShaderInstanceKeys`) of an emitter.
pub fn material_keys(doc: &Document, emitter: usize) -> Vec<i64> {
    doc.object(emitter)
        .ok()
        .and_then(|o| o.find("ExpressionShaderInstanceKeys"))
        .and_then(|f| f.value.data.as_array())
        .map(|a| a.iter().filter_map(|v| v.data.as_i64()).collect())
        .unwrap_or_default()
}

/// Points every emitter that draws with material key `from` at key `to` instead; returns how many keys changed.
pub fn replace_material_key(doc: &mut Document, from: i64, to: i64) -> ebx::Result<usize> {
    let mut n = 0;
    for i in components_of(doc, "EmitterGraphEntityData") {
        let o = doc.object_mut(i)?;
        if let Some(keys) = o.find_mut("ExpressionShaderInstanceKeys").and_then(|f| f.value.data.as_array_mut()) {
            for k in keys.iter_mut() {
                if k.data.as_i64() == Some(from) {
                    k.data = Data::Int(to);
                    n += 1;
                }
            }
        }
    }
    Ok(n)
}

/// Sets one emitter parameter's value (x, y, z, w as given; the rest left) on every emitter that has it; returns how
/// many emitters had it.
pub fn set_emitter_param(doc: &mut Document, property: i64, values: &[f64]) -> ebx::Result<usize> {
    let mut n = 0;
    for i in components_of(doc, "EmitterGraphEntityData") {
        let o = doc.object_mut(i)?;
        for prm in o.find_mut("EmitterGraphParams").and_then(|f| f.value.data.as_array_mut()).ok_or_else(|| err("no EmitterGraphParams"))? {
            let po = prm.data.as_object_mut().ok_or_else(|| err("a parameter is not an object"))?;
            if po.find("PropertyId").and_then(|f| f.value.data.as_i64()) == Some(property) {
                for (a, v) in ["x", "y", "z", "w"].iter().zip(values) {
                    po.set(&format!("Value.{a}"), Data::F64(*v))?;
                }
                n += 1;
            }
        }
    }
    Ok(n)
}

/// Moves every component (emitters and lights) of a blueprint to `trans` (metres, in the effect's own space).
pub fn place_components(doc: &mut Document, trans: [f64; 3]) -> ebx::Result<()> {
    for t in ["EmitterGraphEntityData", "LightEffectEntityData"] {
        for i in components_of(doc, t) {
            let o = doc.object_mut(i)?;
            set_vec(o.find_mut("Transform").and_then(|f| f.value.data.as_object_mut()).ok_or_else(|| err("no Transform"))?, "trans", &trans)?;
        }
    }
    Ok(())
}

/// The ribbon colour that switches on nyanpatch's rainbow (vfx.nyan_colour).
pub fn nyan_colour(steps: f64, trigger: NyanTrigger) -> [f64; 4] {
    match trigger {
        NyanTrigger::Negative => [-1.0, steps, 1.0, 0.0],
        NyanTrigger::Control => [1.0, 0.4, 0.0, 0.0],
        NyanTrigger::Ratio => [RATIO, 1.0, steps / RATIO_STEPS, 0.0],
    }
}

/// Edits a clone of BLUEPRINT for a nyan trail (vfx.nyan_blueprint): only the first ribbon trail, in nyan mode, wide
/// and long-lived; the light, the other emitters and the helper dropped.
pub fn nyan_blueprint(doc: &mut Document, steps: f64, trigger: NyanTrigger) -> ebx::Result<()> {
    let trail = Guid::parse(TRAIL_GRAPH)?;
    let colour = nyan_colour(steps, trigger);
    let mut kept = 0;
    edit_components(doc, |doc, i, keep| {
        if doc.instance_type(i) != "EmitterGraphEntityData" || keep > 0 {
            return Ok(false);
        }
        if graph_file(doc, doc.object(i)?) != Some(trail) {
            return Ok(false);
        }
        let o = doc.object_mut(i)?;
        set_vec(o.find_mut("Transform").and_then(|f| f.value.data.as_object_mut()).ok_or_else(|| err("no Transform"))?, "trans", &[0.0, 0.0, 0.0])?;
        for prm in o.find_mut("EmitterGraphParams").and_then(|f| f.value.data.as_array_mut()).ok_or_else(|| err("no EmitterGraphParams"))? {
            let po = prm.data.as_object_mut().ok_or_else(|| err("a parameter is not an object"))?;
            let pid = po.find("PropertyId").and_then(|f| f.value.data.as_i64());
            if pid == Some(TRAIL_COLOUR) || pid == Some(TRAIL_COLOUR2) {
                set_vec(po, "Value", &colour)?;
            } else if pid == Some(WIDTH) {
                po.set("Value.x", Data::F64(NYAN_WIDTH))?;
            }
        }
        for q in ["Low", "Medium", "High", "Ultra"] {
            o.set(&format!("EmitterGraphOverrides.ParticleLifeSpan.{q}"), Data::F64(NYAN_LIFE))?;
        }
        kept += 1;
        Ok(true)
    })?;
    if kept == 0 {
        return Err(err(format!("no ribbon trail in {BLUEPRINT}")));
    }
    Ok(())
}

/// Edits a clone of ASSET (vfx.asset): attachments on the board at `points`, each playing `blueprint`
/// ((file, instance) GUIDs); with fewer points than the template's attachments the rest are left out. The effect
/// item's InstanceGuidHash is set to `identity` (random in a build; the oni costume's own must not be shared).
pub fn effect_asset(doc: &mut Document, blueprint: (Guid, Guid), points: &[[f64; 3]], identity: i64) -> ebx::Result<()> {
    let items = doc.instances_of("SkaterVfxTriggerableItem");
    if items.len() < points.len() {
        return Err(err(format!("{ASSET} has {} attachments, {} wanted", items.len(), points.len())));
    }
    let used: Vec<usize> = items[..points.len()].to_vec();
    let effect_item = *doc.instances_of("SkaterVfxEffectItem").first().ok_or_else(|| err("no SkaterVfxEffectItem"))?;
    {
        let effects = doc.object_mut(effect_item)?.find_mut("Effects").and_then(|f| f.value.data.as_array_mut()).ok_or_else(|| err("no Effects"))?;
        let mut kept = Vec::new();
        for v in effects.drain(..) {
            if used.contains(&internal(&v)?) {
                kept.push(v);
            }
        }
        *effects = kept;
    }
    for (&i, point) in used.iter().zip(points) {
        let effect_import = match doc.object(i)?.get("Effect")?.as_pointer() {
            Some(Pointer::External(k)) => k,
            other => return Err(err(format!("an attachment's Effect is not an import: {other:?}"))),
        };
        doc.imports[effect_import] = blueprint;
        let o = doc.object_mut(i)?;
        set_vec(o, "Offset", point)?;
        set_vec(o, "Rotation", &[0.0, 0.0, 0.0])?;
        o.set("AttachType", Data::Int(BOARD))?;
        o.set("OffsetType", Data::Int(1))?;
        o.set("TargetBoneHash", Data::UInt(NO_BONE))?;
        o.set("MirroredTargetBoneHash", Data::UInt(NO_BONE))?;
    }
    for i in doc.instances_of("SkaterVfxEffectItem") {
        doc.object_mut(i)?.set("InstanceGuidHash", Data::Int(identity))?;
    }
    Ok(())
}

/// [`effect_asset`] for any number of points: with more points than the template's attachments, the further ones are
/// copies of the first attachment (`next_guid` gives their GUIDs; it is not called otherwise, so up to the template's
/// count the result is exactly `effect_asset`'s).
pub fn effect_asset_at(
    doc: &mut Document,
    blueprint: (Guid, Guid),
    points: &[[f64; 3]],
    identity: i64,
    next_guid: &mut dyn FnMut() -> Guid,
) -> ebx::Result<()> {
    let have = doc.instances_of("SkaterVfxTriggerableItem").len();
    if points.len() <= have {
        return effect_asset(doc, blueprint, points, identity);
    }
    effect_asset(doc, blueprint, &points[..have], identity)?;
    let item = *doc.instances_of("SkaterVfxEffectItem").first().ok_or_else(|| err("no SkaterVfxEffectItem"))?;
    let first = match doc.object(item)?.find("Effects").map(|f| &f.value.data) {
        Some(Data::Array(a)) if !a.is_empty() => internal(&a[0])?,
        _ => return Err(err("the effect item has no attachment")),
    };
    for point in &points[have..] {
        // the copy goes after the exported instances: the effect item (exported) keeps its index
        let copy = doc.clone_instance(first, next_guid())?;
        set_vec(doc.object_mut(copy)?, "Offset", point)?;
        let item = *doc.instances_of("SkaterVfxEffectItem").first().ok_or_else(|| err("no SkaterVfxEffectItem"))?;
        array_mut(doc.object_mut(item)?, "Effects")?.push(Value::new(Pointer::Internal(copy)));
    }
    Ok(())
}

/// One part of a combo board effect (Caveman FX): what plays, a blueprint at `points` on the board, and the state it
/// fires on.
#[derive(Clone, Debug, PartialEq)]
pub struct ComboPart {
    pub blueprint: (Guid, Guid),
    pub points: Vec<[f64; 3]>,
    pub trigger: StateTrigger,
}

/// Makes a clone of ASSET carry several board effects, each its own effect item with its own attachments, blueprint
/// and state, all listed in the asset's `VFX` (the game's own assets carry several items; Dem Bones' two). The first
/// part goes on the template's effect item ([`effect_asset`], [`trigger`]); each further part on a copy of it, with
/// copies of its first attachment. `next_guid` gives the new instances' GUIDs; each item gets its own
/// `InstanceGuidHash` from the same stream.
pub fn combo_asset(doc: &mut Document, parts: &[ComboPart], next_guid: &mut dyn FnMut() -> Guid) -> ebx::Result<()> {
    let (first, rest) = parts.split_first().ok_or_else(|| err("a combo needs at least one part"))?;
    let identity = |g: Guid| i64::from_le_bytes(g.0[..8].try_into().unwrap());
    let id0 = identity(next_guid());
    effect_asset_at(doc, first.blueprint, &first.points, id0, next_guid)?;
    trigger(doc, &first.trigger)?;
    let item0 = *doc.instances_of("SkaterVfxEffectItem").first().ok_or_else(|| err("no SkaterVfxEffectItem"))?;
    let attachment0 = match doc.object(item0)?.find("Effects").map(|f| &f.value.data) {
        Some(Data::Array(a)) if !a.is_empty() => internal(&a[0])?,
        _ => return Err(err("the effect item has no attachment")),
    };
    for part in rest {
        if part.trigger.usage == Usage::OneOff && part.trigger.cooldown <= 0.0 {
            return Err(err("a one-off trigger needs a cooldown above 0 (with 0 it fires every frame)"));
        }
        // exported copies go after the exported instances (item0, attachment0 and the root stay where they are), and
        // each pointer is put into the document at once, so a later insertion moves it along
        let item = doc.clone_instance(item0, next_guid())?;
        array_mut(doc.object_mut(0)?, "VFX")?.push(Value::new(Pointer::Internal(item)));
        array_mut(doc.object_mut(item)?, "Effects")?.clear();
        for point in &part.points {
            let attachment = doc.clone_instance(attachment0, next_guid())?;
            let effect = doc.add_import(part.blueprint);
            let o = doc.object_mut(attachment)?;
            o.set("Effect", effect)?;
            set_vec(o, "Offset", point)?;
            array_mut(doc.object_mut(item)?, "Effects")?.push(Value::new(Pointer::Internal(attachment)));
        }
        let state = doc.add_import(part.trigger.state);
        let id = identity(next_guid());
        let o = doc.object_mut(item)?;
        o.set("TriggerType", Data::Int(GAME_STATE))?;
        o.set("TriggerOutputUsage", Data::Int(if part.trigger.usage == Usage::OneOff { 1 } else { 0 }))?;
        o.set("CooldownType", Data::Int(0))?;
        o.set("CooldownTime", Data::F64(part.trigger.cooldown))?;
        o.set("TriggerEffectsGameState.AssetRef", state)?;
        o.set("InstanceGuidHash", Data::Int(id))?;
    }
    Ok(())
}

fn array_mut<'a>(obj: &'a mut Object, field: &str) -> ebx::Result<&'a mut Vec<Value>> {
    obj.find_mut(field).and_then(|f| f.value.data.as_array_mut()).ok_or_else(|| err(format!("no {field} array")))
}

/// Points an item's SkaterVfx at `asset` ((file, instance) GUIDs) and shows it in the menu preview (vfx.attach).
pub fn attach(item: &mut Document, item_data: usize, asset: (Guid, Guid)) -> ebx::Result<()> {
    let ptr = item.add_import(asset);
    item.set("SkaterVfx", ptr, item_data)?;
    item.set("PreviewVFX", true, item_data)?;
    Ok(())
}

/// The foil-mode swap of a holo preset: the host's enumerator (Pearl) to `mode`'s (newdeck.build).
pub fn foil_mode_remap(mode: FoilMode) -> HashMap<Guid, Guid> {
    HashMap::from([(FoilMode::Pearl.enumerator(), mode.enumerator())])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texels peeked from the copies the game rebuilt on 2026-10-22 (source RGB -> the copy's), one per band of the
    /// calibration gradients: the formulas must land within 1 of each.
    #[test]
    fn ridden_copies_match_the_games_own() {
        let normal = [
            ([8u8, 127, 255], [40u8, 125, 218]),
            ([104, 127, 255], [89, 107, 214]),
            ([127, 127, 255], [104, 104, 208]),
            ([248, 127, 255], [165, 97, 168]),
            ([127, 40, 255], [119, 52, 219]),
            ([127, 232, 255], [98, 159, 173]),
            ([127, 127, 8], [180, 180, 1]),
            ([127, 127, 136], [100, 100, 212]),
            ([127, 127, 152], [103, 103, 210]),
        ];
        for (src, want) in normal {
            let got = ridden_normal(&Image::filled(2, 2, &src));
            for c in 0..3 {
                assert!((got.data[c] as i32 - want[c] as i32).abs() <= 1, "normal {src:?}: got {:?}, the game wrote {want:?}", &got.data[..3]);
            }
        }
        let mask = [([255u8, 16, 4], 29u8), ([255, 128, 128], 141), ([255, 240, 249], 250), ([255, 255, 255], 255), ([0, 0, 0], 26)];
        for (src, want) in mask {
            let got = ridden_mask(&Image::filled(1, 1, &src));
            assert_eq!(&got.data[..2], &src[..2], "mask {src:?}: red and green pass as they are");
            assert!((got.data[2] as i32 - want as i32).abs() <= 1, "mask {src:?}: blue {} , the game wrote {want}", got.data[2]);
        }
        // four channels: the alpha stays
        let got = ridden_normal(&Image::filled(1, 1, &[127, 127, 255, 77]));
        assert_eq!(got.data, vec![104, 104, 208, 77]);
        assert_eq!(ridden_layer("rgb", &Image::filled(1, 1, &[9, 8, 7])).data, vec![9, 8, 7]);
    }

    #[test]
    fn numpy_default_rng_matches() {
        // np.random.default_rng(1).random(4), as hex floats (numpy 2.4.6)
        let mut rng = NumpyRng::new(1);
        let want = [0x3FE060D7BE6F245Cu64, 0x3FEE6A32D7782A55, 0x3FC273D27B04760C, 0x3FEE5B5615DA558D];
        for w in want {
            assert_eq!(rng.random().to_bits(), w);
        }
        assert_eq!(seed_state(1, 4), vec![7434755675892716031, 10007452063617845036, 5266248848659119178, 15045448681910192573]);
        assert_eq!(NumpyRng::new(2).random(), 0.2616121342493164);
        assert_eq!(NumpyRng::new(3).random(), 0.08564916714362436);
        assert_eq!(NumpyRng::new(4).random(), 0.9430561055723676);
    }

    #[test]
    fn suffixes_split_as_mixins() {
        let words: Vec<String> = ["holo-frost", "foil", "spot", "aura", "nyan"].iter().map(|s| s.to_string()).collect();
        let (e, names, notes) = DeckEffects::from_suffixes(&words, None);
        assert_eq!(e.finish, Some(Finish::Holo(FoilMode::Frost)));
        assert!(e.spot);
        assert!(matches!(e.board, Some(BoardEffect::Nyan { .. })));
        assert_eq!(names, vec!["holo-frost", "spot", "nyan"]);
        assert_eq!(notes.len(), 2);
        assert_eq!(hex_colour("00ffFF"), Some([0, 255, 255]));
        assert_eq!(hex_colour("00ffF"), None);
    }
}
