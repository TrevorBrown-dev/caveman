//! 8-bit images: PNG read and write, and the Pillow operations the Python toolkit relies on, reproduced
//! exactly (Pillow 12): `Image.resize(..., LANCZOS)` (fixed-point separable resampling, premultiplied alpha
//! for RGBA), `Image.alpha_composite`, `ImageFilter.GaussianBlur` (three box blurs), `ImageFilter.MinFilter`,
//! `crop`, `rotate(-90, expand=True)` and the mode conversions between L, RGB and RGBA.

use std::fmt;
use std::path::Path;

/// An error reading, writing or converting an image.
#[derive(Debug)]
pub enum ImageError {
    Png(String),
    Io(std::io::Error),
    Invalid(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Png(e) => write!(f, "PNG: {e}"),
            ImageError::Io(e) => write!(f, "{e}"),
            ImageError::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<std::io::Error> for ImageError {
    fn from(e: std::io::Error) -> Self {
        ImageError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, ImageError>;

/// An 8-bit image, row-major, interleaved: 1 channel (Pillow "L"), 3 ("RGB") or 4 ("RGBA").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: Vec<u8>,
}

impl Image {
    /// A black (all zero) image.
    pub fn new(width: usize, height: usize, channels: usize) -> Image {
        assert!(matches!(channels, 1 | 3 | 4), "channels must be 1, 3 or 4");
        Image { width, height, channels, data: vec![0; width * height * channels] }
    }

    /// An image filled with one pixel value (`value.len()` = channels).
    pub fn filled(width: usize, height: usize, value: &[u8]) -> Image {
        let mut img = Image::new(width, height, value.len());
        for px in img.data.chunks_exact_mut(value.len()) {
            px.copy_from_slice(value);
        }
        img
    }

    /// Wraps raw interleaved pixels.
    pub fn from_raw(width: usize, height: usize, channels: usize, data: Vec<u8>) -> Result<Image> {
        if !matches!(channels, 1 | 3 | 4) || data.len() != width * height * channels {
            return Err(ImageError::Invalid(format!(
                "{} bytes do not make a {width} x {height} image of {channels} channels",
                data.len()
            )));
        }
        Ok(Image { width, height, channels, data })
    }

    pub fn pixel(&self, x: usize, y: usize) -> &[u8] {
        let at = (y * self.width + x) * self.channels;
        &self.data[at..at + self.channels]
    }

    pub fn pixel_mut(&mut self, x: usize, y: usize) -> &mut [u8] {
        let at = (y * self.width + x) * self.channels;
        &mut self.data[at..at + self.channels]
    }

    // ------------------------------------------------------------ PNG

    /// Reads a PNG as Pillow opens and converts it: grey as L (RGBA when it has transparency), grey with
    /// alpha as RGBA, palette and RGB as RGB (RGBA with transparency), 16-bit samples cut to their high byte.
    pub fn read_png(path: impl AsRef<Path>) -> Result<Image> {
        let bytes = std::fs::read(path)?;
        Image::decode_png(&bytes)
    }

    pub fn decode_png(bytes: &[u8]) -> Result<Image> {
        let mut decoder = png::Decoder::new(bytes);
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|e| ImageError::Png(e.to_string()))?;
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).map_err(|e| ImageError::Png(e.to_string()))?;
        buf.truncate(info.buffer_size());
        let (w, h) = (info.width as usize, info.height as usize);
        let (channels, data) = match info.color_type {
            png::ColorType::Grayscale => (1, buf),
            png::ColorType::GrayscaleAlpha => {
                let mut out = Vec::with_capacity(w * h * 4);
                for px in buf.chunks_exact(2) {
                    out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
                }
                (4, out)
            }
            png::ColorType::Rgb => (3, buf),
            png::ColorType::Rgba => (4, buf),
            png::ColorType::Indexed => {
                return Err(ImageError::Png("palette not expanded".into()));
            }
        };
        Image::from_raw(w, h, channels, data)
    }

    pub fn write_png(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.encode_png()?)?;
        Ok(())
    }

    pub fn encode_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            enc.set_color(match self.channels {
                1 => png::ColorType::Grayscale,
                3 => png::ColorType::Rgb,
                _ => png::ColorType::Rgba,
            });
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().map_err(|e| ImageError::Png(e.to_string()))?;
            writer.write_image_data(&self.data).map_err(|e| ImageError::Png(e.to_string()))?;
        }
        Ok(out)
    }

    // ------------------------------------------------------------ conversions (Pillow's convert)

    /// Pillow `convert('RGB')`: drops alpha, repeats grey.
    pub fn to_rgb(&self) -> Image {
        match self.channels {
            3 => self.clone(),
            1 => Image { width: self.width, height: self.height, channels: 3, data: self.data.iter().flat_map(|&v| [v, v, v]).collect() },
            _ => Image {
                width: self.width,
                height: self.height,
                channels: 3,
                data: self.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
            },
        }
    }

    /// Pillow `convert('RGBA')`: opaque alpha for L and RGB.
    pub fn to_rgba(&self) -> Image {
        match self.channels {
            4 => self.clone(),
            1 => Image { width: self.width, height: self.height, channels: 4, data: self.data.iter().flat_map(|&v| [v, v, v, 255]).collect() },
            _ => Image {
                width: self.width,
                height: self.height,
                channels: 4,
                data: self.data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            },
        }
    }

    /// Pillow `convert('L')`: ITU-R 601-2 luma in fixed point, (r 19595 + g 38470 + b 7471 + 0x8000) >> 16
    /// (alpha ignored).
    pub fn to_l(&self) -> Image {
        if self.channels == 1 {
            return self.clone();
        }
        Image {
            width: self.width,
            height: self.height,
            channels: 1,
            data: self
                .data
                .chunks_exact(self.channels)
                .map(|p| ((p[0] as u32 * 19595 + p[1] as u32 * 38470 + p[2] as u32 * 7471 + 0x8000) >> 16) as u8)
                .collect(),
        }
    }

    /// One channel as an L image.
    pub fn channel(&self, c: usize) -> Image {
        Image {
            width: self.width,
            height: self.height,
            channels: 1,
            data: self.data.chunks_exact(self.channels).map(|p| p[c]).collect(),
        }
    }

    // ------------------------------------------------------------ geometry

    /// Pillow `crop((x0, y0, x1, y1))` within the image.
    pub fn crop(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Image {
        assert!(x0 <= x1 && x1 <= self.width && y0 <= y1 && y1 <= self.height, "crop outside the image");
        let (w, h, c) = (x1 - x0, y1 - y0, self.channels);
        let mut data = Vec::with_capacity(w * h * c);
        for y in y0..y1 {
            let row = (y * self.width + x0) * c;
            data.extend_from_slice(&self.data[row..row + w * c]);
        }
        Image { width: w, height: h, channels: c, data }
    }

    /// Pillow `rotate(-90, expand=True)`: a quarter turn clockwise (the left edge becomes the top).
    pub fn rotate_cw(&self) -> Image {
        let (w, h, c) = (self.width, self.height, self.channels);
        let mut out = Image::new(h, w, c);
        for oy in 0..w {
            for ox in 0..h {
                let src = self.pixel(oy, h - 1 - ox).to_vec();
                out.pixel_mut(ox, oy).copy_from_slice(&src);
            }
        }
        out
    }

    // ------------------------------------------------------------ resampling (Pillow Resample.c)

    /// Pillow `resize((width, height), Image.LANCZOS)`: the same size is a copy; RGBA is resampled
    /// premultiplied (Pillow converts to "RGBa" and back).
    pub fn resize_lanczos(&self, width: usize, height: usize) -> Image {
        if width == self.width && height == self.height {
            return self.clone();
        }
        if self.channels == 4 {
            let pre = self.premultiply();
            return resample(&pre, width, height).unpremultiply();
        }
        resample(self, width, height)
    }

    /// Pillow's RGBA to RGBa: each colour times alpha / 255, rounded (MULDIV255).
    fn premultiply(&self) -> Image {
        let mut out = self.clone();
        for px in out.data.chunks_exact_mut(4) {
            let a = px[3] as u32;
            for v in &mut px[..3] {
                let tmp = *v as u32 * a + 128;
                *v = (((tmp >> 8) + tmp) >> 8) as u8;
            }
        }
        out
    }

    /// Pillow's RGBa to RGBA: each colour times 255 / alpha (integer division, clipped) unless alpha is 0 or 255.
    fn unpremultiply(&self) -> Image {
        let mut out = self.clone();
        for px in out.data.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a != 0 && a != 255 {
                for v in &mut px[..3] {
                    *v = ((255 * *v as u32) / a).min(255) as u8;
                }
            }
        }
        out
    }

    // ------------------------------------------------------------ compositing

    /// Pillow `Image.alpha_composite(self, over)`: `over` drawn over `self` (both RGBA, same size).
    pub fn alpha_composite(&self, over: &Image) -> Image {
        assert!(self.channels == 4 && over.channels == 4, "alpha_composite needs RGBA images");
        assert!(self.width == over.width && self.height == over.height, "alpha_composite needs equal sizes");
        const PRECISION_BITS: u32 = 7;
        let shift_div255 = |a: u32| ((a >> 8) + a) >> 8;
        let mut out = self.clone();
        for (dst, src) in out.data.chunks_exact_mut(4).zip(over.data.chunks_exact(4)) {
            let (sa, da) = (src[3] as u32, dst[3] as u32);
            if sa == 0 {
                continue;
            }
            let blend = da * (255 - sa);
            let outa255 = sa * 255 + blend;
            let coef1 = sa * 255 * 255 * (1 << PRECISION_BITS) / outa255;
            let coef2 = 255 * (1 << PRECISION_BITS) - coef1;
            for c in 0..3 {
                let tmp = src[c] as u32 * coef1 + dst[c] as u32 * coef2;
                dst[c] = (shift_div255(tmp + (0x80 << PRECISION_BITS)) >> PRECISION_BITS) as u8;
            }
            dst[3] = shift_div255(outa255 + 0x80) as u8;
        }
        out
    }

    // ------------------------------------------------------------ filters (L images)

    /// Pillow `filter(ImageFilter.MinFilter(size))` on an L image: the minimum over a size x size window,
    /// edges repeated.
    pub fn min_filter(&self, size: usize) -> Image {
        assert_eq!(self.channels, 1, "min_filter works on L images");
        let r = (size / 2) as isize;
        let (w, h) = (self.width as isize, self.height as isize);
        // separable: a window minimum is the minimum of row minima
        let mut rows = vec![0u8; self.data.len()];
        for y in 0..h {
            for x in 0..w {
                let mut m = 255u8;
                for dx in -r..=r {
                    let xx = (x + dx).clamp(0, w - 1);
                    m = m.min(self.data[(y * w + xx) as usize]);
                }
                rows[(y * w + x) as usize] = m;
            }
        }
        let mut out = Image::new(self.width, self.height, 1);
        for y in 0..h {
            for x in 0..w {
                let mut m = 255u8;
                for dy in -r..=r {
                    let yy = (y + dy).clamp(0, h - 1);
                    m = m.min(rows[(yy * w + x) as usize]);
                }
                out.data[(y * w + x) as usize] = m;
            }
        }
        out
    }

    /// Pillow `filter(ImageFilter.GaussianBlur(radius))` on an L image: three passes of a box blur with a
    /// fractional radius in each direction (BoxBlur.c).
    pub fn gaussian_blur(&self, radius: f32) -> Image {
        assert_eq!(self.channels, 1, "gaussian_blur works on L images");
        if radius == 0.0 {
            return self.clone();
        }
        let box_r = gaussian_blur_radius(radius, 3);
        let mut out = self.clone();
        for _ in 0..3 {
            box_blur_rows(&mut out, box_r);
        }
        let mut t = out.transpose();
        for _ in 0..3 {
            box_blur_rows(&mut t, box_r);
        }
        t.transpose()
    }

    fn transpose(&self) -> Image {
        let mut out = Image::new(self.height, self.width, self.channels);
        for y in 0..self.height {
            for x in 0..self.width {
                let src = self.pixel(x, y).to_vec();
                out.pixel_mut(y, x).copy_from_slice(&src);
            }
        }
        out
    }
}

/// Pillow's `_gaussian_blur_radius`: the box radius whose `passes` box blurs approximate a Gaussian.
fn gaussian_blur_radius(radius: f32, passes: i32) -> f32 {
    let sigma2: f32 = radius * radius / passes as f32;
    let big_l: f32 = (12.0f64 * sigma2 as f64 + 1.0).sqrt() as f32;
    let l: f32 = ((big_l as f64 - 1.0) / 2.0).floor() as f32;
    let mut a: f32 = ((2.0 * l + 1.0) as f64 * (l * (l + 1.0) - 3.0 * sigma2) as f64) as f32;
    a = (a as f64 / (6.0 * (sigma2 - (l + 1.0) * (l + 1.0)) as f64)) as f32;
    l + a
}

/// Pillow's ImagingHorizontalBoxBlur on an L image, in place (each row through a line buffer).
fn box_blur_rows(img: &mut Image, float_radius: f32) {
    let w = img.width;
    let radius = float_radius as i32;
    let ww: u32 = ((1u32 << 24) as f32 / (float_radius * 2.0 + 1.0)) as u32;
    let fw: u32 = ((1u32 << 24) - (radius as u32 * 2 + 1) * ww) / 2;
    let edge_a = (radius + 1).min(w as i32);
    let edge_b = (w as i32 - radius - 1).max(0);
    let lastx = w as i32 - 1;
    let mut line = vec![0u8; w];
    for y in 0..img.height {
        let row = &img.data[y * w..(y + 1) * w];
        line_box_blur8(&mut line, row, lastx, radius, edge_a, edge_b, ww, fw);
        img.data[y * w..(y + 1) * w].copy_from_slice(&line);
    }
}

#[allow(clippy::too_many_arguments)]
fn line_box_blur8(out: &mut [u8], inp: &[u8], lastx: i32, radius: i32, edge_a: i32, edge_b: i32, ww: u32, fw: u32) {
    let at = |i: i32| inp[i as usize] as u32;
    let mut acc: u32 = at(0) * (radius as u32 + 1);
    for x in 0..edge_a - 1 {
        acc += at(x);
    }
    acc = acc.wrapping_add(at(lastx).wrapping_mul((radius - edge_a + 1) as u32));
    let mut step = |acc: &mut u32, sub: i32, add: i32, left: i32, right: i32, x: i32| {
        *acc = acc.wrapping_add(at(add)).wrapping_sub(at(sub));
        let bulk = acc.wrapping_mul(ww).wrapping_add((at(left) + at(right)).wrapping_mul(fw));
        out[x as usize] = (bulk.wrapping_add(1 << 23) >> 24) as u8;
    };
    if edge_a <= edge_b {
        for x in 0..edge_a {
            step(&mut acc, 0, x + radius, 0, x + radius + 1, x);
        }
        for x in edge_a..edge_b {
            step(&mut acc, x - radius - 1, x + radius, x - radius - 1, x + radius + 1, x);
        }
        for x in edge_b..=lastx {
            step(&mut acc, x - radius - 1, lastx, x - radius - 1, lastx, x);
        }
    } else {
        for x in 0..edge_b {
            step(&mut acc, 0, x + radius, 0, x + radius + 1, x);
        }
        for x in edge_b..edge_a {
            step(&mut acc, 0, lastx, 0, lastx, x);
        }
        for x in edge_a..=lastx {
            step(&mut acc, x - radius - 1, lastx, x - radius - 1, lastx, x);
        }
    }
}

// ---------------------------------------------------------------- Pillow Resample.c, 8 bits per channel

const PRECISION_BITS: u32 = 32 - 8 - 2;

fn lanczos(x: f64) -> f64 {
    fn sinc(x: f64) -> f64 {
        if x == 0.0 {
            1.0
        } else {
            let x = x * std::f64::consts::PI;
            x.sin() / x
        }
    }
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

/// Pillow's precompute_coeffs + normalize_coeffs_8bpc: (bounds [(min, count)], fixed-point weights, ksize).
fn coefficients(in_size: usize, out_size: usize) -> (Vec<(usize, usize)>, Vec<i32>, usize) {
    let (in0, in1) = (0.0f32, in_size as f32);
    let scale = (in1 - in0) as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = 3.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0f64; out_size * ksize];
    for xx in 0..out_size {
        let center = in0 as f64 + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let mut xmin = (center - support + 0.5) as i64;
        if xmin < 0 {
            xmin = 0;
        }
        let mut xmax = (center + support + 0.5) as i64;
        if xmax > in_size as i64 {
            xmax = in_size as i64;
        }
        xmax -= xmin;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        let mut ww = 0.0;
        for x in 0..xmax as usize {
            let w = lanczos((x as f64 + xmin as f64 - center + 0.5) * ss);
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
    let fixed = kk
        .iter()
        .map(|&v| if v < 0.0 { (-0.5 + v * scale_fixed) as i32 } else { (0.5 + v * scale_fixed) as i32 })
        .collect();
    (bounds, fixed, ksize)
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// Pillow ImagingResample with the Lanczos filter: a horizontal pass (when the width changes), then a
/// vertical pass (when the height changes), each rounding to 8 bits.
fn resample(img: &Image, width: usize, height: usize) -> Image {
    let c = img.channels;
    let mut cur = img.clone();
    if width != img.width {
        let (bounds, k, ksize) = coefficients(img.width, width);
        let mut out = Image::new(width, cur.height, c);
        for y in 0..cur.height {
            let row = &cur.data[y * cur.width * c..(y + 1) * cur.width * c];
            for (xx, &(xmin, n)) in bounds.iter().enumerate() {
                let kw = &k[xx * ksize..xx * ksize + n];
                for ch in 0..c {
                    let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                    for (x, &w) in kw.iter().enumerate() {
                        ss += row[(x + xmin) * c + ch] as i32 * w;
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
            let kw = &k[yy * ksize..yy * ksize + n];
            for x in 0..w * c {
                let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                for (y, &wt) in kw.iter().enumerate() {
                    ss += cur.data[(y + ymin) * w * c + x] as i32 * wt;
                }
                out.data[yy * w * c + x] = clip8(ss);
            }
        }
        cur = out;
    }
    cur
}
