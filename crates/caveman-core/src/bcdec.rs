//! BC1, BC3 and BC7 decoders, matching Pillow's `BcnDecode.c` (what the Python toolkit uses through DDS to read
//! stock deck art, thumbnails and masks). Output is RGBA.

use crate::image::Image;

/// Texture formats the decoders handle, by Frostbite format number (gamedata.DXGI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BcFormat {
    Bc1,
    Bc3,
    Bc7,
}

impl BcFormat {
    /// The format of a Frostbite texture format number: 54/55 BC1, 60/61 BC3, 66/67 BC7.
    pub fn from_frostbite(format: i32) -> Option<BcFormat> {
        match format {
            54 | 55 => Some(BcFormat::Bc1),
            60 | 61 => Some(BcFormat::Bc3),
            66 | 67 => Some(BcFormat::Bc7),
            _ => None,
        }
    }

    pub fn block_bytes(self) -> usize {
        match self {
            BcFormat::Bc1 => 8,
            _ => 16,
        }
    }
}

/// Decodes a `width` x `height` mip (row-major blocks) to RGBA, as Pillow's DDS reader does.
pub fn decode(format: BcFormat, data: &[u8], width: usize, height: usize) -> Image {
    let (bw, bh) = (width.div_ceil(4).max(1), height.div_ceil(4).max(1));
    let unit = format.block_bytes();
    let mut img = Image::new(width, height, 4);
    for by in 0..bh {
        for bx in 0..bw {
            let at = (by * bw + bx) * unit;
            let block = &data[at..at + unit];
            let px = match format {
                BcFormat::Bc1 => decode_bc1(block),
                BcFormat::Bc3 => decode_bc3(block),
                BcFormat::Bc7 => decode_bc7(block),
            };
            for (p, rgba) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + p % 4, by * 4 + p / 4);
                if x < width && y < height {
                    img.pixel_mut(x, y).copy_from_slice(rgba);
                }
            }
        }
    }
    img
}

fn decode_565(x: u16) -> [u8; 4] {
    let mut r = ((x & 0xf800) >> 8) as u32;
    r |= r >> 5;
    let mut g = ((x & 0x7e0) >> 3) as u32;
    g |= g >> 6;
    let mut b = ((x & 0x1f) << 3) as u32;
    b |= b >> 5;
    [r as u8, g as u8, b as u8, 0xff]
}

fn decode_bc1_color(src: &[u8], separate_alpha: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([src[0], src[1]]);
    let c1 = u16::from_le_bytes([src[2], src[3]]);
    let lut = u32::from_le_bytes([src[4], src[5], src[6], src[7]]);
    let mut p = [decode_565(c0), decode_565(c1), [0; 4], [0; 4]];
    let (a, b) = (p[0].map(|v| v as u32), p[1].map(|v| v as u32));
    if c0 > c1 || separate_alpha {
        for c in 0..3 {
            p[2][c] = ((2 * a[c] + b[c]) / 3) as u8;
            p[3][c] = ((a[c] + 2 * b[c]) / 3) as u8;
        }
        p[2][3] = 0xff;
        p[3][3] = 0xff;
    } else {
        for c in 0..3 {
            p[2][c] = ((a[c] + b[c]) / 2) as u8;
        }
        p[2][3] = 0xff;
        p[3] = [0, 0, 0, 0];
    }
    let mut out = [[0u8; 4]; 16];
    for (n, px) in out.iter_mut().enumerate() {
        *px = p[(3 & (lut >> (2 * n))) as usize];
    }
    out
}

fn decode_bc1(src: &[u8]) -> [[u8; 4]; 16] {
    decode_bc1_color(src, false)
}

fn decode_bc3(src: &[u8]) -> [[u8; 4]; 16] {
    let mut out = decode_bc1_color(&src[8..], true);
    let (a0, a1) = (src[0] as u32, src[1] as u32);
    let mut a = [a0 as u8, a1 as u8, 0, 0, 0, 0, 0, 0];
    if a0 > a1 {
        for k in 2..8u32 {
            a[k as usize] = (((8 - k) * a0 + (k - 1) * a1) / 7) as u8;
        }
    } else {
        for k in 2..6u32 {
            a[k as usize] = (((6 - k) * a0 + (k - 1) * a1) / 5) as u8;
        }
        a[6] = 0;
        a[7] = 0xff;
    }
    let lut1 = src[2] as u32 | (src[3] as u32) << 8 | (src[4] as u32) << 16;
    let lut2 = src[5] as u32 | (src[6] as u32) << 8 | (src[7] as u32) << 16;
    for n in 0..8 {
        out[n][3] = a[(7 & (lut1 >> (3 * n))) as usize];
        out[8 + n][3] = a[(7 & (lut2 >> (3 * n))) as usize];
    }
    out
}

// ---------------------------------------------------------------- BC7

struct ModeInfo {
    ns: usize,  // subsets
    pb: usize,  // partition bits
    rb: usize,  // rotation bits
    isb: usize, // index selection bits
    cb: usize,  // colour bits
    ab: usize,  // alpha bits
    epb: bool,  // endpoint p-bits
    spb: bool,  // shared (per subset) p-bits
    ib: usize,  // index bits
    ib2: usize, // second index bits
}

const MODES: [ModeInfo; 8] = [
    ModeInfo { ns: 3, pb: 4, rb: 0, isb: 0, cb: 4, ab: 0, epb: true, spb: false, ib: 3, ib2: 0 },
    ModeInfo { ns: 2, pb: 6, rb: 0, isb: 0, cb: 6, ab: 0, epb: false, spb: true, ib: 3, ib2: 0 },
    ModeInfo { ns: 3, pb: 6, rb: 0, isb: 0, cb: 5, ab: 0, epb: false, spb: false, ib: 2, ib2: 0 },
    ModeInfo { ns: 2, pb: 6, rb: 0, isb: 0, cb: 7, ab: 0, epb: true, spb: false, ib: 2, ib2: 0 },
    ModeInfo { ns: 1, pb: 0, rb: 2, isb: 1, cb: 5, ab: 6, epb: false, spb: false, ib: 2, ib2: 3 },
    ModeInfo { ns: 1, pb: 0, rb: 2, isb: 0, cb: 7, ab: 8, epb: false, spb: false, ib: 2, ib2: 2 },
    ModeInfo { ns: 1, pb: 0, rb: 0, isb: 0, cb: 7, ab: 7, epb: true, spb: false, ib: 4, ib2: 0 },
    ModeInfo { ns: 2, pb: 6, rb: 0, isb: 0, cb: 5, ab: 5, epb: true, spb: false, ib: 2, ib2: 0 },
];

/// Two-subset partitions: bit n = the subset of pixel n.
const SI2: [u16; 64] = [
    0xcccc, 0x8888, 0xeeee, 0xecc8, 0xc880, 0xfeec, 0xfec8, 0xec80, 0xc800, 0xffec, 0xfe80, 0xe800, 0xffe8, 0xff00, 0xfff0, 0xf000, 0xf710, 0x008e, 0x7100,
    0x08ce, 0x008c, 0x7310, 0x3100, 0x8cce, 0x088c, 0x3110, 0x6666, 0x366c, 0x17e8, 0x0ff0, 0x718e, 0x399c, 0xaaaa, 0xf0f0, 0x5a5a, 0x33cc, 0x3c3c, 0x55aa,
    0x9696, 0xa55a, 0x73ce, 0x13c8, 0x324c, 0x3bdc, 0x6996, 0xc33c, 0x9966, 0x0660, 0x0272, 0x04e4, 0x4e40, 0x2720, 0xc936, 0x936c, 0x39c6, 0x639c, 0x9336,
    0x9cc6, 0x817e, 0xe718, 0xccf0, 0x0fcc, 0x7744, 0xee22,
];

/// Three-subset partitions: 2 bits per pixel.
const SI3: [u32; 64] = [
    0xaa685050, 0x6a5a5040, 0x5a5a4200, 0x5450a0a8, 0xa5a50000, 0xa0a05050, 0x5555a0a0, 0x5a5a5050, 0xaa550000, 0xaa555500, 0xaaaa5500, 0x90909090, 0x94949494,
    0xa4a4a4a4, 0xa9a59450, 0x2a0a4250, 0xa5945040, 0x0a425054, 0xa5a5a500, 0x55a0a0a0, 0xa8a85454, 0x6a6a4040, 0xa4a45000, 0x1a1a0500, 0x0050a4a4, 0xaaa59090,
    0x14696914, 0x69691400, 0xa08585a0, 0xaa821414, 0x50a4a450, 0x6a5a0200, 0xa9a58000, 0x5090a0a8, 0xa8a09050, 0x24242424, 0x00aa5500, 0x24924924, 0x24499224,
    0x50a50a50, 0x500aa550, 0xaaaa4444, 0x66660000, 0xa5a0a5a0, 0x50a050a0, 0x69286928, 0x44aaaa44, 0x66666600, 0xaa444444, 0x54a854a8, 0x95809580, 0x96969600,
    0xa85454a8, 0x80959580, 0xaa141414, 0x96960000, 0xaaaa1414, 0xa05050a0, 0xa0a5a5a0, 0x96000000, 0x40804080, 0xa9a8a9a8, 0xaaaaaa44, 0x2a4a5254,
];

/// Anchor of the second subset of two-subset partitions.
const AI0: [usize; 64] = [
    15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8, 2, 2, 8, 8, 2, 2, 15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2,
    2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2, 2, 15, 15, 15, 15, 15, 2, 2, 15,
];

/// Anchor of the second subset of three-subset partitions.
const AI1: [usize; 64] = [
    3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3, 3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5, 15, 15, 8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15,
    15, 15, 3, 15, 5, 5, 5, 8, 5, 10, 5, 10, 8, 13, 15, 12, 3, 3,
];

/// Anchor of the third subset of three-subset partitions.
const AI2: [usize; 64] = [
    15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8, 15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6, 10, 15, 15, 10, 8, 15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8,
    15, 3, 6, 6, 8, 15, 3, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
];

const W2: [u32; 4] = [0, 21, 43, 64];
const W3: [u32; 8] = [0, 9, 18, 27, 37, 46, 55, 64];
const W4: [u32; 16] = [0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64];

fn weights(n: usize) -> &'static [u32] {
    match n {
        2 => &W2,
        3 => &W3,
        _ => &W4,
    }
}

fn subset(ns: usize, partition: usize, n: usize) -> usize {
    match ns {
        2 => (1 & (SI2[partition] >> n)) as usize,
        3 => (3 & (SI3[partition] >> (2 * n))) as usize,
        _ => 0,
    }
}

fn get_bits(src: &[u8], bit: usize, count: usize) -> u32 {
    let v = u128::from_le_bytes(src[..16].try_into().unwrap());
    if count == 0 {
        return 0;
    }
    ((v >> bit) & ((1u128 << count) - 1)) as u32
}

fn expand(v: u32, bits: usize) -> u32 {
    let v = (v << (8 - bits)) & 0xff;
    v | (v >> bits)
}

fn lerp(e0: [u32; 4], e1: [u32; 4], s0: u32, s1: u32) -> [u8; 4] {
    let (t0, t1) = (64 - s0, 64 - s1);
    [
        ((t0 * e0[0] + s0 * e1[0] + 32) >> 6) as u8,
        ((t0 * e0[1] + s0 * e1[1] + 32) >> 6) as u8,
        ((t0 * e0[2] + s0 * e1[2] + 32) >> 6) as u8,
        ((t1 * e0[3] + s1 * e1[3] + 32) >> 6) as u8,
    ]
}

fn decode_bc7(src: &[u8]) -> [[u8; 4]; 16] {
    let mut col = [[0u8, 0, 0, 255]; 16];
    if src[0] == 0 {
        return col;
    }
    let mode = src[0].trailing_zeros() as usize;
    let info = &MODES[mode];
    let mut bit = mode + 1;
    let mut load = |n: usize| {
        let v = get_bits(src, bit, n);
        bit += n;
        v
    };
    let partition = load(info.pb) as usize;
    let rotation = load(info.rb);
    let index_sel = load(info.isb);
    let numep = info.ns * 2;
    let mut ep = [[0u32; 4]; 6];
    for c in 0..3 {
        for e in ep.iter_mut().take(numep) {
            e[c] = load(info.cb);
        }
    }
    for e in ep.iter_mut().take(numep) {
        e[3] = if info.ab > 0 { load(info.ab) } else { 255 };
    }
    let (mut cb, mut ab) = (info.cb, info.ab);
    if info.epb {
        cb += 1;
        if ab > 0 {
            ab += 1;
        }
        for e in ep.iter_mut().take(numep) {
            let p = load(1);
            for c in 0..3 {
                e[c] = (e[c] << 1) | p;
            }
            if ab > 0 {
                e[3] = (e[3] << 1) | p;
            }
        }
    }
    if info.spb {
        cb += 1;
        if ab > 0 {
            ab += 1;
        }
        for i in (0..numep).step_by(2) {
            let p = load(1);
            for e in &mut ep[i..i + 2] {
                for c in 0..3 {
                    e[c] = (e[c] << 1) | p;
                }
                if ab > 0 {
                    e[3] = (e[3] << 1) | p;
                }
            }
        }
    }
    for e in ep.iter_mut().take(numep) {
        for c in 0..3 {
            e[c] = expand(e[c], cb);
        }
        if ab > 0 {
            e[3] = expand(e[3], ab);
        }
    }
    let cw = weights(info.ib);
    let aw = weights(if ab > 0 && info.ib2 > 0 { info.ib2 } else { info.ib });
    let mut cibit = bit;
    let mut aibit = cibit + 16 * info.ib - info.ns;
    for (i, out) in col.iter_mut().enumerate() {
        let s = subset(info.ns, partition, i) * 2;
        let mut ib = info.ib;
        if i == 0 || (info.ns == 2 && i == AI0[partition]) || (info.ns == 3 && (i == AI1[partition] || i == AI2[partition])) {
            ib -= 1;
        }
        let i0 = get_bits(src, cibit, ib) as usize;
        cibit += ib;
        let (e0, e1) = (ep[s], ep[s + 1]);
        let mut px = if ab > 0 && info.ib2 > 0 {
            let mut ib2 = info.ib2;
            if i == 0 {
                ib2 -= 1;
            }
            let i1 = get_bits(src, aibit, ib2) as usize;
            aibit += ib2;
            if index_sel != 0 {
                lerp(e0, e1, aw[i1], cw[i0])
            } else {
                lerp(e0, e1, cw[i0], aw[i1])
            }
        } else {
            lerp(e0, e1, cw[i0], cw[i0])
        };
        match rotation {
            1 => px.swap(0, 3),
            2 => px.swap(1, 3),
            3 => px.swap(2, 3),
            _ => {}
        }
        *out = px;
    }
    col
}
