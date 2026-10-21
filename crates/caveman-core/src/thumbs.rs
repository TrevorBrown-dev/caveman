//! Menu thumbnails: art "rendered" onto the board the way the game's own thumbnails are (`thumbs.py`), and
//! the wheel model (`newwheel.map_wheel`, `fit_wheel_light`, `render_thumbnail`).
//!
//! A stock deck thumbnail is the deck seen from below, standing up, with a transparent background. Columns of
//! the art map linearly onto it, rows follow a fitted curve (the nose and tail bend away from the camera:
//! `assets/thumbnail_mapping.json`). Light (multiplied) and sheen (added) per pixel are fitted over stock decks;
//! new art times the light plus the sheen looks like a vanilla thumbnail. Floating point follows numpy's
//! float64 order so results match the Python closely.

use crate::image::Image;

/// A float64 image (numpy arrays in the Python): row-major, `channels` per pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: Vec<f64>,
}

impl Field {
    pub fn new(width: usize, height: usize, channels: usize) -> Field {
        Field { width, height, channels, data: vec![0.0; width * height * channels] }
    }

    /// An 8-bit image as floats (`np.asarray(img).astype(np.float64)`).
    pub fn from_image(img: &Image) -> Field {
        Field { width: img.width, height: img.height, channels: img.channels, data: img.data.iter().map(|&v| v as f64).collect() }
    }

    fn at(&self, x: usize, y: usize, c: usize) -> f64 {
        self.data[(y * self.width + x) * self.channels + c]
    }
}

/// Where deck art lands on a thumbnail (assets/thumbnail_mapping.json).
#[derive(Clone, Debug)]
pub struct Mapping {
    pub thumb_size: f64,
    pub thumb_cols: [f64; 2],
    pub art_cols: [usize; 2],
    /// Polynomial (highest power first) from a thumbnail row (at thumb_size) to an art row.
    pub poly: Vec<f64>,
}

const MAPPING_JSON: &str = include_str!("../../../assets/thumbnail_mapping.json");

impl Default for Mapping {
    /// Caveman's fitted mapping (assets/thumbnail_mapping.json, built in).
    fn default() -> Mapping {
        Mapping::from_json(MAPPING_JSON).expect("the built-in thumbnail mapping parses")
    }
}

impl Mapping {
    /// Reads the mapping JSON (only the keys it needs).
    pub fn from_json(text: &str) -> Option<Mapping> {
        let thumb_size = json_numbers(text, "thumb_size")?.first().copied()?;
        let cols = json_numbers(text, "thumb_cols")?;
        let art = json_numbers(text, "art_cols")?;
        let poly = json_numbers(text, "poly")?;
        Some(Mapping {
            thumb_size,
            thumb_cols: [*cols.first()?, *cols.get(1)?],
            art_cols: [*art.first()? as usize, *art.get(1)? as usize],
            poly,
        })
    }
}

/// The number or numbers after `"key":` in a small JSON document.
fn json_numbers(text: &str, key: &str) -> Option<Vec<f64>> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
    let body = if let Some(list) = rest.strip_prefix('[') { &list[..list.find(']')?] } else { rest.split([',', '}', '\n']).next()? };
    body.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| s.parse().ok()).collect()
}

/// numpy's pairwise summation (`np.sum` of a contiguous float64 array).
pub fn pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut res = 0.0;
        for &v in a {
            res += v;
        }
        res
    } else if n <= 128 {
        let mut r = [0f64; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - (n % 8) {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

/// thumbs.map_art: the art (RGB) as it lands on a `size` x `size` thumbnail's deck, extended past the deck edge.
pub fn map_art(art: &Image, size: usize, m: &Mapping) -> Field {
    let k = size as f64 / m.thumb_size;
    let tx0 = (m.thumb_cols[0] * k).round_ties_even() as i64;
    let tx1 = (m.thumb_cols[1] * k).round_ties_even() as i64;
    let (ax0, ax1) = (m.art_cols[0], m.art_cols[1]);
    let cols = art.to_rgb().crop(ax0, 0, ax1, 2048).resize_lanczos((tx1 - tx0) as usize, 2048);
    const PAD: usize = 8;
    let pw = cols.width + 2 * PAD;
    // cumulative sums down the columns (edge-padded by PAD on each side)
    let mut csum = vec![0f64; 2049 * pw * 3];
    for y in 0..2048 {
        for x in 0..pw {
            let sx = x.saturating_sub(PAD).min(cols.width - 1);
            for c in 0..3 {
                let v = cols.data[(y * cols.width + sx) * 3 + c] as f64;
                csum[((y + 1) * pw + x) * 3 + c] = csum[(y * pw + x) * 3 + c] + v;
            }
        }
    }
    let edges: Vec<f64> = (0..=size)
        .map(|i| {
            let x = i as f64 / k;
            let mut y = 0.0;
            for &p in &m.poly {
                y = y * x + p;
            }
            y.clamp(0.0, 2047.0)
        })
        .collect();
    let mut out = Field::new(size, size, 3);
    let x0 = tx0 - PAD as i64;
    for y in 0..size {
        let (a0, a1) = (edges[y], edges[y + 1]);
        let i0 = a0.floor() as usize;
        let i1 = ((i0 + 1) as f64).max(a1.ceil()) as usize;
        let i1c = i1.min(2048);
        let count = (i1c - i0) as f64;
        let lo = x0.max(0);
        let hi = (size as i64).min(x0 + pw as i64);
        for x in lo..hi {
            let bx = (x - x0) as usize;
            for c in 0..3 {
                let band = (csum[(i1c * pw + bx) * 3 + c] - csum[(i0 * pw + bx) * 3 + c]) / count;
                out.data[(y * size + x as usize) * 3 + c] = band;
            }
        }
    }
    out
}

/// thumbs._gauss: separable Gaussian blur of a 2-D float array, edges repeated.
pub fn gauss(a: &[f64], width: usize, height: usize, sigma: f64) -> Vec<f64> {
    let r = (3.0 * sigma) as usize;
    let mut k: Vec<f64> = (0..=2 * r)
        .map(|i| {
            let t = (i as f64 - r as f64) / sigma;
            (-0.5 * (t * t)).exp()
        })
        .collect();
    let s = pairwise_sum(&k);
    for v in &mut k {
        *v /= s;
    }
    let ph = height + 2 * r;
    let padded = |x: usize, y: usize| -> f64 {
        let sx = x.saturating_sub(r).min(width - 1);
        let sy = y.saturating_sub(r).min(height - 1);
        a[sy * width + sx]
    };
    // horizontal: rows of the padded array, columns i..i + width
    let mut h = vec![0f64; ph * width];
    for y in 0..ph {
        for x in 0..width {
            let mut acc = 0.0;
            for (i, &ki) in k.iter().enumerate() {
                acc += ki * padded(x + i, y);
            }
            h[y * width + x] = acc;
        }
    }
    let mut out = vec![0f64; height * width];
    for y in 0..height {
        for x in 0..width {
            let mut acc = 0.0;
            for (i, &ki) in k.iter().enumerate() {
                acc += ki * h[(y + i) * width + x];
            }
            out[y * width + x] = acc;
        }
    }
    out
}

/// thumbs._smooth_inside: a Gaussian blur of `img` within the deck (alpha > 250; normalized, so outside pixels
/// do not bleed in), with the original kept within `keep_edge` px of the deck's edge.
pub fn smooth_inside(img: &Field, alpha: &[f64], radius: f64, keep_edge: usize) -> Field {
    if radius <= 0.0 {
        return img.clone();
    }
    let (w, h, ch) = (img.width, img.height, img.channels);
    let inside: Vec<f64> = alpha.iter().map(|&a| if a > 250.0 { 1.0 } else { 0.0 }).collect();
    let weight = gauss(&inside, w, h, radius);
    let mut out = Field::new(w, h, ch);
    for c in 0..ch {
        let plane: Vec<f64> = (0..w * h).map(|i| img.data[i * ch + c] * inside[i]).collect();
        let blurred = gauss(&plane, w, h, radius);
        for i in 0..w * h {
            out.data[i * ch + c] = blurred[i] / weight[i].max(1e-6);
        }
    }
    let mask = Image::from_raw(w, h, 1, inside.iter().map(|&v| (v * 255.0) as u8).collect()).unwrap();
    let core = mask.min_filter(2 * keep_edge + 1).gaussian_blur(2.0);
    let mut res = Field::new(w, h, ch);
    for i in 0..w * h {
        let k = core.data[i] as f64 / 255.0;
        for c in 0..ch {
            let j = i * ch + c;
            res.data[j] = k * out.data[j] + (1.0 - k) * img.data[j];
        }
    }
    res
}

/// numpy's median over the first axis of stacked equal-size planes.
fn median_planes(planes: &[Vec<f64>]) -> Vec<f64> {
    let n = planes.len();
    (0..planes[0].len())
        .map(|i| {
            let mut v: Vec<f64> = planes.iter().map(|p| p[i]).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if n % 2 == 1 {
                v[n / 2]
            } else {
                (v[n / 2 - 1] + v[n / 2]) / 2.0
            }
        })
        .collect()
}

/// The fitted light of a thumbnail model: light (multiplied) and sheen (added) per pixel and channel, and the
/// median alpha of the stock thumbnails.
#[derive(Clone, Debug)]
pub struct Lighting {
    pub light: Field,
    pub sheen: Field,
    pub alpha: Vec<f64>,
}

/// The least-squares light and sheen of `mapped` (each stock item's art as it lands on its thumbnail) against
/// the thumbnails: real = art x light + sheen, with a ridge; light smoothed inside (thumbs.fit_light).
fn fit(mapped: &[Field], thumbs: &[Image], ridge: f64, smooth: f64, edge: usize) -> Lighting {
    let (w, h) = (mapped[0].width, mapped[0].height);
    let len = w * h * 3;
    let (mut sa, mut sr, mut saa, mut sar) = (vec![0f64; len], vec![0f64; len], vec![0f64; len], vec![0f64; len]);
    let mut alphas = Vec::new();
    for (a, thumb) in mapped.iter().zip(thumbs) {
        let rgba = thumb.to_rgba();
        for i in 0..w * h {
            for c in 0..3 {
                let j = i * 3 + c;
                let (av, rv) = (a.data[j], rgba.data[i * 4 + c] as f64);
                sa[j] += av;
                sr[j] += rv;
                saa[j] += av * av;
                sar[j] += av * rv;
            }
        }
        alphas.push(rgba.data.chunks_exact(4).map(|p| p[3] as f64).collect::<Vec<_>>());
    }
    let n = mapped.len() as f64;
    let mut light = Field::new(w, h, 3);
    let (mut ma, mut mr) = (vec![0f64; len], vec![0f64; len]);
    for j in 0..len {
        ma[j] = sa[j] / n;
        mr[j] = sr[j] / n;
        let var = saa[j] / n - ma[j] * ma[j];
        let cov = sar[j] / n - ma[j] * mr[j];
        light.data[j] = cov / (var + ridge);
    }
    let alpha = median_planes(&alphas);
    let light = smooth_inside(&light, &alpha, smooth, edge);
    let mut sheen = Field::new(w, h, 3);
    for j in 0..len {
        sheen.data[j] = mr[j] - light.data[j] * ma[j];
    }
    Lighting { light, sheen, alpha }
}

/// thumbs.fit_light: deck thumbnail lighting from `pairs` [(stock art, its thumbnail of `size`)].
pub fn fit_light(pairs: &[(Image, Image)], size: usize, m: &Mapping) -> Lighting {
    let mapped: Vec<Field> = pairs.iter().map(|(art, _)| map_art(art, size, m)).collect();
    let thumbs: Vec<Image> = pairs.iter().map(|(_, t)| t.clone()).collect();
    fit(&mapped, &thumbs, 60.0, 6.0, 6)
}

/// thumbs.fit_metal: the light of an all-metal deck (one grey light per pixel: art x light, least squares
/// through zero over the stock metal decks). Returns a 1-channel field.
pub fn fit_metal(pairs: &[(Image, Image)], size: usize, m: &Mapping) -> Field {
    let (mut num, mut den) = (vec![0f64; size * size], vec![0f64; size * size]);
    let mut alphas = Vec::new();
    for (art, thumb) in pairs {
        let a = map_art(art, size, m);
        let rgba = thumb.to_rgba();
        for i in 0..size * size {
            let (mut sn, mut sd) = (0.0, 0.0);
            for c in 0..3 {
                let av = a.data[i * 3 + c];
                sn += av * rgba.data[i * 4 + c] as f64;
                sd += av * av;
            }
            num[i] += sn;
            den[i] += sd;
        }
        alphas.push(rgba.data.chunks_exact(4).map(|p| p[3] as f64).collect::<Vec<_>>());
    }
    let alpha = median_planes(&alphas);
    let light = Field { width: size, height: size, channels: 1, data: num.iter().zip(&den).map(|(n, d)| n / d.max(1.0)).collect() };
    smooth_inside(&light, &alpha, 6.0, 6)
}

/// thumbs.render: new deck art on the board with the fitted light and sheen. `metal`: the deck's metal mask
/// (an image the size of the art, 255 metal) and the metal light (fit_metal). Returns RGBA.
pub fn render(art: &Image, lighting: &Lighting, metal: Option<(&Image, &Field)>, m: &Mapping) -> Image {
    let size = lighting.light.width;
    let mapped = map_art(art, size, m);
    let mut rgb: Vec<f64> = (0..size * size * 3).map(|j| mapped.data[j] * lighting.light.data[j] + lighting.sheen.data[j]).collect();
    if let Some((mask, metal_light)) = metal {
        let mm = map_art(mask, size, m);
        for i in 0..size * size {
            let k = mm.data[i * 3] / 255.0;
            for c in 0..3 {
                let j = i * 3 + c;
                rgb[j] = (1.0 - k) * rgb[j] + k * mapped.data[j] * metal_light.data[i];
            }
        }
    }
    to_rgba(&rgb, &lighting.alpha, size)
}

/// Clip to 0-255, black where the alpha is under 1 (the game's thumbnails are), truncate to 8 bits.
fn to_rgba(rgb: &[f64], alpha: &[f64], size: usize) -> Image {
    let mut out = Image::new(size, size, 4);
    for i in 0..size * size {
        let a = alpha[i];
        for c in 0..3 {
            let v = if a < 1.0 { 0.0 } else { rgb[i * 3 + c].clamp(0.0, 255.0) };
            out.data[i * 4 + c] = v as u8;
        }
        out.data[i * 4 + 3] = a as u8;
    }
    out
}

// ---------------------------------------------------------------- wheels (newwheel.py)

/// The wheel disc in a 768 px thumbnail (centre x, centre y, radius) and the face scale (newwheel.DISC,
/// FACE_SCALE): the face ellipse (the top half of the 512 x 512 texture) maps onto the disc with its outer
/// 18% wrapping over the wheel's rounded edge.
pub const WHEEL_DISC: (f64, f64, f64) = (378.5, 383.5, 335.5);
pub const WHEEL_FACE_SCALE: f64 = 1.22;

/// newwheel.linear_to_srgb for one colour channel (0-1 linear to a rounded 0-255 value).
pub fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    (255.0 * if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }).round_ties_even()
}

/// newwheel.srgb_to_linear for one 0-255 value.
pub fn srgb_to_linear(c: f64) -> f64 {
    let c = c / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A stock wheel as seen in game (newwheel._stock_pairs): its art where its mask's R shows the graphic, its
/// colour (linear) elsewhere.
pub fn wheel_seen_texture(art: &Image, mask: &Image, colour_linear: [f64; 3]) -> Field {
    let (art, mask) = (art.to_rgb(), mask.to_rgb());
    let base = colour_linear.map(linear_to_srgb);
    let mut out = Field::new(art.width, art.height, 3);
    for i in 0..art.width * art.height {
        let k = mask.data[i * 3] as f64 / 255.0;
        for c in 0..3 {
            out.data[i * 3 + c] = art.data[i * 3 + c] as f64 * k + base[c] * (1.0 - k);
        }
    }
    out
}

/// newwheel.map_wheel: a 512 x 512 wheel texture (RGB floats) as it lands on a `size` x `size` thumbnail: the
/// top half's ellipse on the disc, sampled bilinearly at 768 px and averaged down for smaller sizes.
pub fn map_wheel(tex: &Field, size: usize) -> Field {
    let (cx, cy, r) = WHEEL_DISC;
    let mut full = Field::new(768, 768, 3);
    for y in 0..768 {
        let ay = (128.0 + (y as f64 - cy) / r / WHEEL_FACE_SCALE * 128.0).clamp(0.0, 255.0);
        let y0 = ay.floor() as usize;
        let y1 = (y0 + 1).min(256);
        let fy = ay - y0 as f64;
        for x in 0..768 {
            let ax = (256.0 + (x as f64 - cx) / r / WHEEL_FACE_SCALE * 256.0).clamp(0.0, 511.0);
            let x0 = ax.floor() as usize;
            let x1 = (x0 + 1).min(511);
            let fx = ax - x0 as f64;
            for c in 0..3 {
                let v = (tex.at(x0, y0, c) * (1.0 - fx) + tex.at(x1, y0, c) * fx) * (1.0 - fy)
                    + (tex.at(x0, y1, c) * (1.0 - fx) + tex.at(x1, y1, c) * fx) * fy;
                full.data[(y * 768 + x) * 3 + c] = v;
            }
        }
    }
    if size == 768 {
        return full;
    }
    let k = 768 / size;
    let mut out = Field::new(size, size, 3);
    for y in 0..size {
        for x in 0..size {
            for c in 0..3 {
                let mut s = 0.0;
                for dy in 0..k {
                    for dx in 0..k {
                        s += full.data[((y * k + dy) * 768 + x * k + dx) * 3 + c];
                    }
                }
                out.data[(y * size + x) * 3 + c] = s / (k * k) as f64;
            }
        }
    }
    out
}

/// newwheel.fit_wheel_light: wheel thumbnail lighting from `pairs` [(texture as seen in game, thumbnail of `size`)].
pub fn fit_wheel_light(pairs: &[(Field, Image)], size: usize) -> Lighting {
    let mapped: Vec<Field> = pairs.iter().map(|(t, _)| map_wheel(t, size)).collect();
    let thumbs: Vec<Image> = pairs.iter().map(|(_, t)| t.clone()).collect();
    let smooth = 6.0 * size as f64 / 768.0;
    let edge = ((6.0 * size as f64 / 768.0).round_ties_even() as usize).max(2);
    fit(&mapped, &thumbs, 60.0, smooth, edge)
}

/// newwheel.render_thumbnail: a wheel texture on the disc with the fitted light and sheen. Returns RGBA.
pub fn render_wheel(tex: &Field, lighting: &Lighting) -> Image {
    let size = lighting.light.width;
    let mapped = map_wheel(tex, size);
    let rgb: Vec<f64> = (0..size * size * 3).map(|j| mapped.data[j] * lighting.light.data[j] + lighting.sheen.data[j]).collect();
    to_rgba(&rgb, &lighting.alpha, size)
}
