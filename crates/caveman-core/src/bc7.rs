//! A BC7 encoder, mode 6 only (one subset, RGBA endpoints with per-endpoint p-bits, 4-bit indices): a port of
//! `bc7.py` that writes the same blocks byte for byte. RGB input is encoded opaque (alpha 254 or 255: deck
//! textures do not use it); RGBA input keeps its alpha (menu thumbnails are cut out).
//!
//! Exactness: the Python encoder is numpy, so every floating-point step here repeats numpy's precision and
//! order (measured on numpy 2.4): the means, differences, palettes and distances are integer-exact; the
//! covariance (an einsum over the 16 pixels) is a sequential float32 sum; the axis is float64 (numpy promotes
//! `ones(float32) / np.sqrt(ch)`), its dot products use numpy's two-lane SIMD order (even products, odd
//! products, then the two lanes); the least-squares refit is float32.

use crate::image::Image;

const WEIGHTS: [f32; 16] = [0., 4., 9., 13., 17., 21., 26., 30., 34., 38., 43., 47., 51., 55., 60., 64.];

/// BC7 blocks (16 bytes each, row-major by block) of an RGB or RGBA image (bc7.encode). Sizes that are
/// not multiples of 4 are padded by repeating the edge, as numpy's `pad(mode='edge')`.
pub fn encode(img: &Image) -> Vec<u8> {
    assert!(matches!(img.channels, 3 | 4), "BC7 encodes RGB or RGBA images");
    let blocks = blocks(img);
    let mut out = vec![0u8; blocks.len() * 16];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(16);
    let per = blocks.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (chunk_in, chunk_out) in blocks.chunks(per).zip(out.chunks_mut(per * 16)) {
            s.spawn(move || {
                for (b, o) in chunk_in.iter().zip(chunk_out.chunks_exact_mut(16)) {
                    o.copy_from_slice(&encode_block(b, img.channels));
                }
            });
        }
    });
    out
}

/// The 4 x 4 blocks of an image, row-major, each 16 pixels x 4 channels as f32 (unused channels 0).
pub(crate) fn blocks(img: &Image) -> Vec<[[f32; 4]; 16]> {
    let (w, h, c) = (img.width, img.height, img.channels);
    let (bw, bh) = (w.div_ceil(4).max(1), h.div_ceil(4).max(1));
    let mut out = Vec::with_capacity(bw * bh);
    for by in 0..bh {
        for bx in 0..bw {
            let mut b = [[0f32; 4]; 16];
            for p in 0..16 {
                let x = (bx * 4 + p % 4).min(w - 1);
                let y = (by * 4 + p / 4).min(h - 1);
                let px = img.pixel(x, y);
                for ch in 0..c {
                    b[p][ch] = px[ch] as f32;
                }
            }
            out.push(b);
        }
    }
    out
}

/// numpy's float64 dot of two short contiguous vectors: two SIMD lanes (even and odd products), then added.
fn dot2(a: &[f64], b: &[f64]) -> f64 {
    let mut lanes = [0f64; 2];
    for k in 0..a.len() {
        lanes[k % 2] += a[k] * b[k];
    }
    lanes[0] + lanes[1]
}

struct Fit {
    q0: [i64; 4],
    p0: i64,
    q1: [i64; 4],
    p1: i64,
    idx: [usize; 16],
    err: u64,
}

/// bc7._quantize: 7-bit endpoint values and the p-bit with less error (float64; p 0 wins ties).
fn quantize(e: &[f64], ch: usize) -> ([i64; 4], i64) {
    let mut best = ([0i64; 4], 0i64, f64::INFINITY);
    for p in 0..2 {
        let mut q = [0i64; 4];
        let mut err = 0f64;
        for c in 0..ch {
            let v = ((e[c] - p as f64) / 2.0).round_ties_even().clamp(0.0, 127.0);
            q[c] = v as i64;
            let d = (v * 2.0 + p as f64) - e[c];
            err += d * d;
        }
        if p == 0 || err < best.2 {
            best = (q, p, err);
        }
    }
    (best.0, best.1)
}

/// bc7._fit: quantize both endpoints, build the palette, pick each pixel's nearest entry.
fn fit(px: &[[f32; 4]; 16], ch: usize, e0: &[f64], e1: &[f64]) -> Fit {
    let clip = |e: &[f64]| -> Vec<f64> { e.iter().map(|v| v.clamp(0.0, 255.0)).collect() };
    let (q0, p0) = quantize(&clip(e0), ch);
    let (q1, p1) = quantize(&clip(e1), ch);
    let mut pal = [[0i64; 4]; 16];
    for (k, w) in WEIGHTS.iter().enumerate() {
        for c in 0..ch {
            let a = (q0[c] * 2 + p0) as f32;
            let b = (q1[c] * 2 + p1) as f32;
            pal[k][c] = (((64.0 - w) * a + w * b + 32.0) / 64.0).floor() as i64;
        }
    }
    let mut idx = [0usize; 16];
    let mut err = 0u64;
    for p in 0..16 {
        let mut best = (u64::MAX, 0usize);
        for (k, entry) in pal.iter().enumerate() {
            let mut d = 0u64;
            for c in 0..ch {
                let diff = px[p][c] as i64 - entry[c];
                d += (diff * diff) as u64;
            }
            if d < best.0 {
                best = (d, k);
            }
        }
        idx[p] = best.1;
        err += best.0;
    }
    Fit { q0, p0, q1, p1, idx, err }
}

/// One block (bc7._encode_batch for a single block).
fn encode_block(px: &[[f32; 4]; 16], ch: usize) -> [u8; 16] {
    // mean and centred colours (exact)
    let mut mean = [0f32; 4];
    for c in 0..ch {
        let mut s = 0f32;
        for p in px {
            s += p[c];
        }
        mean[c] = s / 16.0;
    }
    let mut cc = [[0f32; 4]; 16];
    for p in 0..16 {
        for c in 0..ch {
            cc[p][c] = px[p][c] - mean[c];
        }
    }
    // covariance: float32, sequential over the pixels
    let mut cov = [[0f64; 4]; 4];
    for i in 0..ch {
        for j in 0..ch {
            let mut s = 0f32;
            for p in &cc {
                s += p[i] * p[j];
            }
            cov[i][j] = s as f64;
        }
    }
    // principal axis: float64 power iteration
    let mut axis = vec![1.0f64 / (ch as f64).sqrt(); ch];
    for _ in 0..8 {
        let next: Vec<f64> = (0..ch).map(|i| dot2(&cov[i][..ch], &axis)).collect();
        let mut s = 0f64;
        for v in &next {
            s += v * v;
        }
        let norm = s.sqrt().max(1e-6);
        axis = next.iter().map(|v| v / norm).collect();
    }
    let t: Vec<f64> = cc.iter().map(|p| dot2(&p[..ch].iter().map(|&v| v as f64).collect::<Vec<_>>(), &axis)).collect();
    let tmin = t.iter().cloned().fold(f64::INFINITY, f64::min);
    let tmax = t.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let e0: Vec<f64> = (0..ch).map(|c| mean[c] as f64 + tmin * axis[c]).collect();
    let e1: Vec<f64> = (0..ch).map(|c| mean[c] as f64 + tmax * axis[c]).collect();
    let first = fit(px, ch, &e0, &e1);

    // one least-squares refit of the endpoints to the chosen indices (float32), kept where it helps
    let mut aa = 0f32;
    let mut bb = 0f32;
    let mut ab = 0f32;
    let mut ax = [0f32; 4];
    let mut bx = [0f32; 4];
    for p in 0..16 {
        let w = WEIGHTS[first.idx[p]] / 64.0;
        let (a, b) = (1.0 - w, w);
        aa += a * a;
        bb += b * b;
        ab += a * b;
        for c in 0..ch {
            ax[c] += a * px[p][c];
            bx[c] += b * px[p][c];
        }
    }
    let det = aa * bb - ab * ab;
    let ok = det.abs() > 1e-6f32;
    let (r0, r1): (Vec<f64>, Vec<f64>) = if ok {
        (
            (0..ch).map(|c| ((bb * ax[c] - ab * bx[c]) / det) as f64).collect(),
            (0..ch).map(|c| ((aa * bx[c] - ab * ax[c]) / det) as f64).collect(),
        )
    } else {
        (e0.clone(), e1.clone())
    };
    let refit = fit(px, ch, &r0, &r1);
    let mut f = if refit.err < first.err { refit } else { first };

    // the anchor (pixel 0) index must have its top bit clear: swap the endpoints and mirror otherwise
    if f.idx[0] >= 8 {
        std::mem::swap(&mut f.q0, &mut f.q1);
        std::mem::swap(&mut f.p0, &mut f.p1);
        for v in f.idx.iter_mut() {
            *v = 15 - *v;
        }
    }
    pack(&f, ch)
}

fn pack(f: &Fit, ch: usize) -> [u8; 16] {
    let mut bits: u128 = 0;
    let mut pos = 0u32;
    let mut put = |value: u64, n: u32| {
        bits |= ((value & ((1u64 << n) - 1)) as u128) << pos;
        pos += n;
    };
    put(1 << 6, 7); // mode 6
    for c in 0..3 {
        put(f.q0[c] as u64, 7);
        put(f.q1[c] as u64, 7);
    }
    if ch == 4 {
        put(f.q0[3] as u64, 7);
        put(f.q1[3] as u64, 7);
    } else {
        put(127, 7); // alpha endpoints: opaque
        put(127, 7);
    }
    put(f.p0 as u64, 1);
    put(f.p1 as u64, 1);
    put(f.idx[0] as u64, 3);
    for k in 1..16 {
        put(f.idx[k] as u64, 4);
    }
    bits.to_le_bytes()
}
