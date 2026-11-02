//! Colour helpers shared by the item build (grip colours) and the board model (wheel and grip colours). They live
//! apart from `boardmodel` because they decide what the build makes: `tests/item_cache_version.rs` watches this file
//! and not the board model's.

use crate::image::Image;

/// sRGB 0..1 to linear 0..1.
pub fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear 0..1 to sRGB 0..1.
pub fn linear_to_srgb(v: f64) -> f64 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// A colour texture as the game's region colouring shows it: the colour where the texture is at its typical value
/// and the texture's variation around it. (The menu thumbnails of the wheel colours and of black grip show the
/// preset's colour itself at about the renders' light level, not the colour times the grey texture.) `colour` is
/// linear rgb; the typical value is the median linear luminance of the texels that are not nearly black (the
/// bearing seal, cut-outs).
pub fn coloured(tex: &Image, colour: [f32; 3]) -> Image {
    let src = tex.to_rgba();
    let lum = |p: &[u8]| -> f64 {
        0.2126 * srgb_to_linear(p[0] as f64 / 255.0) + 0.7152 * srgb_to_linear(p[1] as f64 / 255.0) + 0.0722 * srgb_to_linear(p[2] as f64 / 255.0)
    };
    let mut lums: Vec<f64> = src.data.chunks_exact(4).map(lum).filter(|l| *l > 0.02).collect();
    if lums.is_empty() {
        return src;
    }
    lums.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = lums[lums.len() / 2].max(1e-3);
    let mut out = src.clone();
    for px in out.data.chunks_exact_mut(4) {
        for c in 0..3 {
            let lin = srgb_to_linear(px[c] as f64 / 255.0) / median * colour[c] as f64;
            px[c] = (linear_to_srgb(lin.clamp(0.0, 1.0)) * 255.0).round() as u8;
        }
    }
    out
}
