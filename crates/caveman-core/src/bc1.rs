//! A BC1 encoder (opaque four-colour blocks): a port of `bc1.py` that writes the same blocks byte for byte.
//! Each 4 x 4 block's endpoints are the ends of its colours along their main axis, quantized to RGB 565;
//! each texel takes the nearest of the four palette colours. Wheel masks (`_osk`) are BC1 (format 54).
//!
//! Exactness: everything is float32 as in numpy (`np.ones(..., float32)` keeps the axis float32 here); the
//! covariance and all dot products are sequential float32 sums (measured on numpy 2.4), rounding is numpy's
//! half-to-even.

use crate::bc7::blocks;
use crate::image::Image;

/// BC1 blocks (8 bytes each, row-major by block) of an RGB or RGBA image; alpha is ignored (bc1.encode).
pub fn encode(img: &Image) -> Vec<u8> {
    assert!(matches!(img.channels, 3 | 4), "BC1 encodes RGB or RGBA images");
    let blocks = blocks(img);
    let mut out = vec![0u8; blocks.len() * 8];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
    let per = blocks.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (chunk_in, chunk_out) in blocks.chunks(per).zip(out.chunks_mut(per * 8)) {
            s.spawn(move || {
                for (b, o) in chunk_in.iter().zip(chunk_out.chunks_exact_mut(8)) {
                    o.copy_from_slice(&encode_block(b));
                }
            });
        }
    });
    out
}

fn to565(c: [f32; 3]) -> u32 {
    let c = c.map(|v| v.round_ties_even().clamp(0.0, 255.0));
    let r = (c[0] * 31.0 / 255.0).round_ties_even() as u32;
    let g = (c[1] * 63.0 / 255.0).round_ties_even() as u32;
    let b = (c[2] * 31.0 / 255.0).round_ties_even() as u32;
    (r << 11) | (g << 5) | b
}

fn from565(v: u32) -> [f32; 3] {
    let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
    [((r << 3) | (r >> 2)) as f32, ((g << 2) | (g >> 4)) as f32, ((b << 3) | (b >> 2)) as f32]
}

fn encode_block(px: &[[f32; 4]; 16]) -> [u8; 8] {
    let mut mean = [0f32; 3];
    for c in 0..3 {
        let mut s = 0f32;
        for p in px {
            s += p[c];
        }
        mean[c] = s / 16.0;
    }
    let mut d = [[0f32; 3]; 16];
    for p in 0..16 {
        for c in 0..3 {
            d[p][c] = px[p][c] - mean[c];
        }
    }
    let mut cov = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut s = 0f32;
            for p in &d {
                s += p[i] * p[j];
            }
            cov[i][j] = s;
        }
    }
    let mut axis = [1f32; 3];
    for _ in 0..8 {
        let mut next = [0f32; 3];
        for i in 0..3 {
            let mut s = 0f32;
            for j in 0..3 {
                s += cov[i][j] * axis[j];
            }
            next[i] = s;
        }
        let mut n = 0f32;
        for v in next {
            n += v * v;
        }
        let norm = n.sqrt().max(1e-6);
        axis = next.map(|v| v / norm);
    }
    let mut tmin = f32::INFINITY;
    let mut tmax = f32::NEG_INFINITY;
    for p in &d {
        let mut s = 0f32;
        for i in 0..3 {
            s += p[i] * axis[i];
        }
        tmin = tmin.min(s);
        tmax = tmax.max(s);
    }
    let e0 = [0, 1, 2].map(|c| mean[c] + axis[c] * tmax);
    let e1 = [0, 1, 2].map(|c| mean[c] + axis[c] * tmin);
    let (mut c0, mut c1) = (to565(e0), to565(e1));
    if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1); // four-colour mode needs c0 > c1
    }
    let (p0, p1) = (from565(c0), from565(c1));
    let pal = [
        p0,
        p1,
        [0, 1, 2].map(|c| (2.0 * p0[c] + p1[c]) / 3.0),
        [0, 1, 2].map(|c| (p0[c] + 2.0 * p1[c]) / 3.0),
    ];
    let mut bits = 0u32;
    if c0 != c1 {
        for (p, colour) in px.iter().enumerate() {
            let mut best = (f32::INFINITY, 0u32);
            for (k, entry) in pal.iter().enumerate() {
                let mut s = 0f32;
                for c in 0..3 {
                    let diff = colour[c] - entry[c];
                    s += diff * diff;
                }
                if s < best.0 {
                    best = (s, k as u32);
                }
            }
            bits |= best.1 << (2 * p);
        }
    } // a flat block: c0 == c1 is three-colour mode, index 0 is c0
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(c0 | (c1 << 16)).to_le_bytes());
    out[4..].copy_from_slice(&bits.to_le_bytes());
    out
}
