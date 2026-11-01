//! A BC1 encoder, 4-colour mode only: per 4 x 4 block the two endpoints are the block's two texels farthest apart
//! (its extremes along its spread), quantised to RGB565 and ordered so the first is the greater (the 4-colour mode),
//! and each texel's index is the nearest of the four palette colours, interpolated as the decoder does. Plain and
//! deterministic: it writes the world copy of The Caveman's graphic (the BC1 texture the game re-fills at every
//! equip, `caveman-fx` board.rs), where exactness against the game's own encoder does not matter. Sizes that are
//! not multiples of 4 are padded by repeating the edge.

use crate::image::Image;

/// BC1 blocks (8 bytes each, row-major by block) of an RGB or RGBA image (the alpha dropped).
pub fn encode(img: &Image) -> Vec<u8> {
    let rgb = img.to_rgb();
    let (w, h) = (rgb.width, rgb.height);
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let mut out = Vec::with_capacity(bw * bh * 8);
    for by in 0..bh {
        for bx in 0..bw {
            let mut px = [[0u8; 3]; 16];
            for (t, p) in px.iter_mut().enumerate() {
                let sx = (bx * 4 + t % 4).min(w - 1);
                let sy = (by * 4 + t / 4).min(h - 1);
                let at = (sy * w + sx) * 3;
                *p = [rgb.data[at], rgb.data[at + 1], rgb.data[at + 2]];
            }
            out.extend_from_slice(&encode_block(&px));
        }
    }
    out
}

fn to565(c: [u8; 3]) -> u16 {
    ((c[0] as u16 >> 3) << 11) | ((c[1] as u16 >> 2) << 5) | (c[2] as u16 >> 3)
}

fn from565(v: u16) -> [u8; 3] {
    let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

fn dist2(a: [u8; 3], b: [u8; 3]) -> i64 {
    (0..3).map(|c| (a[c] as i64 - b[c] as i64).pow(2)).sum()
}

/// One block: 16 texels row-major.
pub fn encode_block(px: &[[u8; 3]; 16]) -> [u8; 8] {
    let (mut a, mut b, mut far) = (0usize, 0usize, -1i64);
    for i in 0..16 {
        for j in i..16 {
            let d = dist2(px[i], px[j]);
            if d > far {
                far = d;
                a = i;
                b = j;
            }
        }
    }
    let (mut c0, mut c1) = (to565(px[a]), to565(px[b]));
    if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1);
    }
    let (p0, p1) = (from565(c0), from565(c1));
    let mix = |x: u8, y: u8, wx: u32, wy: u32| ((x as u32 * wx + y as u32 * wy + 1) / 3) as u8;
    let palette = [
        p0,
        p1,
        [mix(p0[0], p1[0], 2, 1), mix(p0[1], p1[1], 2, 1), mix(p0[2], p1[2], 2, 1)],
        [mix(p0[0], p1[0], 1, 2), mix(p0[1], p1[1], 1, 2), mix(p0[2], p1[2], 1, 2)],
    ];
    let mut idx = 0u32;
    if c0 != c1 {
        for (t, p) in px.iter().enumerate() {
            let best = (0..4).min_by_key(|&k| dist2(*p, palette[k])).unwrap_or(0);
            idx |= (best as u32) << (t * 2);
        }
    }
    // (equal endpoints: the 3-colour mode, every index 0: the colour itself)
    let [i0, i1, i2, i3] = idx.to_le_bytes();
    [c0 as u8, (c0 >> 8) as u8, c1 as u8, (c1 >> 8) as u8, i0, i1, i2, i3]
}
