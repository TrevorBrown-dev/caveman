//! A BC7 decoder (every mode), for the slice match (`board.rs`): the corner blocks of the game's tile slices and our
//! art's blocks decoded and compared as colours, since the game's encoder is not ours. The partition and anchor
//! tables are in `bc7_tables.rs` (generated from Pillow's decoder, a port of DirectXTex); `tests/bc7_decode.rs`
//! checks every mode against it.

// ---------------------------------------------------------------- the decoder

/// Reads bits of a 128-bit block, least significant first.
struct Reader<'a> {
    block: &'a [u8; 16],
    at: usize,
}

impl Reader<'_> {
    fn bits(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for k in 0..n {
            let i = self.at + k;
            v |= (((self.block[i / 8] >> (i % 8)) & 1) as u32) << k;
        }
        self.at += n;
        v
    }
}

const W2: [u32; 4] = [0, 21, 43, 64];
const W3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const W4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn weight(bits: usize, index: u32) -> u32 {
    match bits {
        2 => W2[index as usize],
        3 => W3[index as usize],
        _ => W4[index as usize],
    }
}

fn lerp(a: u8, b: u8, w: u32) -> u8 {
    ((a as u32 * (64 - w) + b as u32 * w + 32) >> 6) as u8
}

/// An endpoint value of `bits` bits (the p-bit, if any, already appended) widened to 8 bits as the spec does.
fn expand(v: u32, bits: usize) -> u8 {
    let x = v << (8 - bits);
    (x | (x >> bits)) as u8
}

/// Decodes one BC7 block (any mode) to its 16 texels (row-major, RGBA). A block with no mode bit set (reserved) is
/// black.
pub fn decode_block(block: &[u8; 16]) -> [[u8; 4]; 16] {
    let mut out = [[0u8, 0, 0, 255]; 16];
    let mut mode = 0usize;
    while mode < 8 && (block[0] >> mode) & 1 == 0 {
        mode += 1;
    }
    if mode == 8 {
        return [[0u8; 4]; 16];
    }
    // (subsets, partition bits, rotation bits, index selector bits, colour bits, alpha bits, p-bit mode (0 none,
    // 1 unique per endpoint, 2 shared per subset), index bits, second index bits)
    let (subsets, pbits_n, rot_n, sel_n, cb, ab, pmode, ib, ib2): (usize, usize, usize, usize, usize, usize, u8, usize, usize) = match mode {
        0 => (3, 4, 0, 0, 4, 0, 1, 3, 0),
        1 => (2, 6, 0, 0, 6, 0, 2, 3, 0),
        2 => (3, 6, 0, 0, 5, 0, 0, 2, 0),
        3 => (2, 6, 0, 0, 7, 0, 1, 2, 0),
        4 => (1, 0, 2, 1, 5, 6, 0, 2, 3),
        5 => (1, 0, 2, 0, 7, 8, 0, 2, 2),
        6 => (1, 0, 0, 0, 7, 7, 1, 4, 0),
        _ => (2, 6, 0, 0, 5, 5, 1, 2, 0),
    };
    let mut r = Reader { block, at: mode + 1 };
    let partition = r.bits(pbits_n) as usize;
    let rotation = r.bits(rot_n);
    let selector = r.bits(sel_n);
    let n_end = subsets * 2;
    // the endpoints: every channel's values for all endpoints, then the p-bits
    let mut e = [[0u32; 4]; 6];
    for ch in 0..3 {
        for k in 0..n_end {
            e[k][ch] = r.bits(cb);
        }
    }
    if ab > 0 {
        for k in 0..n_end {
            e[k][3] = r.bits(ab);
        }
    }
    let (mut cbits, mut abits) = (cb, ab);
    match pmode {
        1 => {
            for k in 0..n_end {
                let p = r.bits(1);
                for ch in 0..4 {
                    e[k][ch] = e[k][ch] << 1 | p;
                }
            }
            cbits += 1;
            abits += if ab > 0 { 1 } else { 0 };
        }
        2 => {
            for s in 0..subsets {
                let p = r.bits(1);
                for k in [s * 2, s * 2 + 1] {
                    for ch in 0..4 {
                        e[k][ch] = e[k][ch] << 1 | p;
                    }
                }
            }
            cbits += 1;
            abits += if ab > 0 { 1 } else { 0 };
        }
        _ => {}
    }
    let mut ep = [[0u8; 4]; 6];
    for k in 0..n_end {
        for ch in 0..3 {
            ep[k][ch] = expand(e[k][ch], cbits);
        }
        ep[k][3] = if ab > 0 { expand(e[k][3], abits) } else { 255 };
    }
    // the subsets and anchors
    let subset_of = |t: usize| -> usize {
        match subsets {
            3 => crate::bc7_tables::PARTITION3[partition][t] as usize,
            2 => crate::bc7_tables::PARTITION2[partition][t] as usize,
            _ => 0,
        }
    };
    let anchors: [usize; 3] = match subsets {
        3 => [0, crate::bc7_tables::ANCHOR3_1[partition] as usize, crate::bc7_tables::ANCHOR3_2[partition] as usize],
        2 => [0, crate::bc7_tables::ANCHOR2[partition] as usize, 16],
        _ => [0, 16, 16],
    };
    // the indices: the first set, then (modes 4 and 5) the second
    let mut idx = [0u32; 16];
    let mut idx2 = [0u32; 16];
    for (t, slot) in idx.iter_mut().enumerate() {
        let n = if anchors.contains(&t) { ib - 1 } else { ib };
        *slot = r.bits(n);
    }
    if ib2 > 0 {
        for (t, slot) in idx2.iter_mut().enumerate() {
            let n = if t == 0 { ib2 - 1 } else { ib2 };
            *slot = r.bits(n);
        }
    }
    for t in 0..16 {
        let s = subset_of(t);
        let (a, b) = (ep[s * 2], ep[s * 2 + 1]);
        let (ci, cbits_i, ai, abits_i) = if ib2 == 0 {
            (idx[t], ib, idx[t], ib)
        } else if mode == 4 && selector == 1 {
            // the 3-bit set colours, the 2-bit set the alpha
            (idx2[t], ib2, idx[t], ib)
        } else {
            (idx[t], ib, idx2[t], ib2)
        };
        let wc = weight(cbits_i, ci);
        let wa = weight(abits_i, ai);
        let mut px = [lerp(a[0], b[0], wc), lerp(a[1], b[1], wc), lerp(a[2], b[2], wc), lerp(a[3], b[3], wa)];
        match rotation {
            1 => px.swap(0, 3),
            2 => px.swap(1, 3),
            3 => px.swap(2, 3),
            _ => {}
        }
        out[t] = px;
    }
    out
}

/// The mean absolute difference per channel (RGB) between two decoded blocks.
pub fn block_error(a: &[[u8; 4]; 16], b: &[[u8; 4]; 16]) -> f64 {
    let mut sum = 0u32;
    for (x, y) in a.iter().zip(b) {
        for ch in 0..3 {
            sum += (x[ch] as i32 - y[ch] as i32).unsigned_abs();
        }
    }
    sum as f64 / 48.0
}

// ---------------------------------------------------------------- BC1 and BC3 (for the colour hunt)

fn rgb565(v: u16) -> [u8; 3] {
    let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

/// Decodes a BC1 block (8 bytes): two RGB565 endpoints, 2-bit indices; the 3-colour mode (first endpoint not above
/// the second) with its transparent black.
pub fn decode_bc1(block: &[u8; 8]) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |x: u8, y: u8, wx: u32, wy: u32, d: u32| ((x as u32 * wx + y as u32 * wy + d / 2) / d) as u8;
    let palette: [[u8; 4]; 4] = if c0 > c1 {
        [
            [a[0], a[1], a[2], 255],
            [b[0], b[1], b[2], 255],
            [mix(a[0], b[0], 2, 1, 3), mix(a[1], b[1], 2, 1, 3), mix(a[2], b[2], 2, 1, 3), 255],
            [mix(a[0], b[0], 1, 2, 3), mix(a[1], b[1], 1, 2, 3), mix(a[2], b[2], 1, 2, 3), 255],
        ]
    } else {
        [[a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255], [mix(a[0], b[0], 1, 1, 2), mix(a[1], b[1], 1, 1, 2), mix(a[2], b[2], 1, 1, 2), 255], [0, 0, 0, 0]]
    };
    let idx = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let mut out = [[0u8; 4]; 16];
    for (t, px) in out.iter_mut().enumerate() {
        *px = palette[((idx >> (t * 2)) & 3) as usize];
    }
    out
}

/// Decodes a BC3 block (16 bytes): a BC4 alpha block, then a BC1 colour block (always its 4-colour mode).
pub fn decode_bc3(block: &[u8; 16]) -> [[u8; 4]; 16] {
    let (a0, a1) = (block[0] as u32, block[1] as u32);
    let alphas: [u8; 8] = if a0 > a1 {
        [
            a0 as u8,
            a1 as u8,
            ((6 * a0 + a1 + 3) / 7) as u8,
            ((5 * a0 + 2 * a1 + 3) / 7) as u8,
            ((4 * a0 + 3 * a1 + 3) / 7) as u8,
            ((3 * a0 + 4 * a1 + 3) / 7) as u8,
            ((2 * a0 + 5 * a1 + 3) / 7) as u8,
            ((a0 + 6 * a1 + 3) / 7) as u8,
        ]
    } else {
        [
            a0 as u8,
            a1 as u8,
            ((4 * a0 + a1 + 2) / 5) as u8,
            ((3 * a0 + 2 * a1 + 2) / 5) as u8,
            ((2 * a0 + 3 * a1 + 2) / 5) as u8,
            ((a0 + 4 * a1 + 2) / 5) as u8,
            0,
            255,
        ]
    };
    let bits = u64::from_le_bytes([block[2], block[3], block[4], block[5], block[6], block[7], 0, 0]);
    let colour: [u8; 8] = block[8..16].try_into().unwrap();
    // the colour part is always four colours, whatever the endpoints' order
    let c0 = u16::from_le_bytes([colour[0], colour[1]]);
    let c1 = u16::from_le_bytes([colour[2], colour[3]]);
    let (a, b) = (rgb565(c0), rgb565(c1));
    let mix = |x: u8, y: u8, wx: u32, wy: u32| ((x as u32 * wx + y as u32 * wy + 1) / 3) as u8;
    let palette = [
        [a[0], a[1], a[2]],
        [b[0], b[1], b[2]],
        [mix(a[0], b[0], 2, 1), mix(a[1], b[1], 2, 1), mix(a[2], b[2], 2, 1)],
        [mix(a[0], b[0], 1, 2), mix(a[1], b[1], 1, 2), mix(a[2], b[2], 1, 2)],
    ];
    let idx = u32::from_le_bytes([colour[4], colour[5], colour[6], colour[7]]);
    let mut out = [[0u8; 4]; 16];
    for (t, px) in out.iter_mut().enumerate() {
        let c = palette[((idx >> (t * 2)) & 3) as usize];
        *px = [c[0], c[1], c[2], alphas[((bits >> (t * 3)) & 7) as usize]];
    }
    out
}
