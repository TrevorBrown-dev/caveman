//! Texture resources: the header (`fb.texture_header`) and the edits Caveman makes to a cloned one
//! (`newdeck._add_texture`, `newwheel.add_texture`), mip chain sizes, and the name hash.
//!
//! The 180-byte header (resource meta version >= 12):
//!
//! | Bytes | What |
//! |---|---|
//! | 0-3, 4-7 | encoded (CAS) offsets of the blocks holding the starts of mips 1 and 2 |
//! | 8-11 | type; 12-15 format (Frostbite number: 54 BC1, 66 BC7, 67 BC7 sRGB, ...) |
//! | 20-29 | flags, width, height, depth, slices (u16 each) |
//! | 30, 31 | mip count, first mip (32 too: Caveman sets 31 and 32 to 0 for a whole texture) |
//! | 40-55 | the chunk GUID (little-endian GUID bytes) |
//! | 56-115 | mip sizes (15 x u32); 116-119 chunk size |
//! | 120-127 | the owner hash of the texture's name |

use std::fmt;

#[derive(Debug)]
pub struct TextureError(pub String);

impl fmt::Display for TextureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TextureError {}

/// A parsed texture header (fb.texture_header).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureHeader {
    pub version: u32,
    pub len: usize,
    pub kind: u32,
    pub format: i32,
    pub flags: u16,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub slices: u16,
    pub mips: u8,
    pub first_mip: u8,
    /// Little-endian GUID bytes, as stored.
    pub chunk: [u8; 16],
    pub mip_sizes: Vec<u32>,
    pub chunk_size: u32,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

impl TextureHeader {
    /// Parses a texture resource's payload; `meta` is the resource's 16-byte meta (its first u32 the version).
    pub fn parse(payload: &[u8], meta: &[u8]) -> Result<TextureHeader, TextureError> {
        if meta.len() < 4 || payload.len() < 120 {
            return Err(TextureError(format!("texture header too short ({} bytes)", payload.len())));
        }
        let version = u32_at(meta, 0);
        let ext = payload.len() == 180;
        let mut o = 8;
        let kind = u32_at(payload, o);
        let format = u32_at(payload, o + 4) as i32;
        o += 8;
        if version >= 12 {
            o += 4;
        }
        let (flags, width, height, depth, slices) =
            (u16_at(payload, o), u16_at(payload, o + 2), u16_at(payload, o + 4), u16_at(payload, o + 6), u16_at(payload, o + 8));
        o += 10;
        let (mips, first_mip) = (payload[o], payload[o + 1]);
        o += 2;
        o += if ext { 8 } else { 4 };
        let chunk: [u8; 16] = payload[o..o + 16].try_into().unwrap();
        o += 16;
        let all: Vec<u32> = (0..15).map(|i| u32_at(payload, o + 4 * i)).collect();
        o += 60;
        let chunk_size = u32_at(payload, o);
        Ok(TextureHeader {
            version,
            len: payload.len(),
            kind,
            format,
            flags,
            width,
            height,
            depth,
            slices,
            mips,
            first_mip,
            chunk,
            mip_sizes: all[..(mips as usize).min(15)].to_vec(),
            chunk_size,
        })
    }

    /// Logical (decoded) start of mip `m` in the chunk.
    pub fn mip_start(&self, m: usize) -> u32 {
        self.mip_sizes[..m].iter().sum()
    }
}

/// Bytes of one 4 x 4 block of a Frostbite texture format (gamedata.DXGI): 8 for BC1, 16 for BC3/BC7.
pub fn block_bytes(format: i32) -> Option<usize> {
    match format {
        54 | 55 => Some(8),
        60 | 61 | 66 | 67 => Some(16),
        _ => None,
    }
}

/// The byte sizes of a mip chain of a block-compressed `width` x `height` texture.
pub fn mip_sizes(width: usize, height: usize, mips: usize, block_bytes: usize) -> Vec<usize> {
    (0..mips)
        .map(|m| {
            let (w, h) = ((width >> m).max(1), (height >> m).max(1));
            w.div_ceil(4).max(1) * h.div_ceil(4).max(1) * block_bytes
        })
        .collect()
}

/// Offset in a CAS-encoded chunk of the block whose decoded range holds `logical` (newwheel.block_containing).
/// A texture header's bytes 0-7 are these for the starts of mips 1 and 2. Block headers: a big-endian u32
/// whose low 24 bits are the decoded size, then the compression code (bytes 4, 5; byte 5's low nibble the
/// top bits of the encoded size) and a big-endian u16 encoded size.
pub fn block_containing(encoded: &[u8], logical: u64) -> Result<u32, TextureError> {
    let (mut at, mut decoded) = (0usize, 0u64);
    while at + 8 <= encoded.len() {
        let d = (u32::from_be_bytes(encoded[at..at + 4].try_into().unwrap()) & 0xFF_FFFF) as u64;
        if decoded <= logical && logical < decoded + d {
            return Ok(at as u32);
        }
        let size = u16::from_be_bytes([encoded[at + 6], encoded[at + 7]]) as usize + (((encoded[at + 5] & 0xF) as usize) << 16);
        at += 8 + size;
        decoded += d;
    }
    Err(TextureError(format!("no block holds {logical:#x}")))
}

/// The owner hash of a name (newdeck.owner_hash): 64-bit djb2 with xor over the lower-case name. Chunk
/// metadata `h64` and texture header bytes 120-127.
pub fn owner_hash(name: &str) -> u64 {
    let mut h: u64 = 5381;
    for c in name.to_lowercase().bytes() {
        h = h.wrapping_mul(33) ^ c as u64;
    }
    h
}

/// The number of mip levels of a full chain down to 1 x 1.
pub fn mip_levels(width: usize, height: usize) -> usize {
    let mut n = 1;
    let mut m = width.max(height);
    while m > 1 {
        m >>= 1;
        n += 1;
    }
    n
}

/// A cloned header given another size and mip count (an animated deck's atlas in the holo map slot, see
/// knowledge/animated-decks.md): width and height (bytes 22-25), the mip count (byte 30, and byte 33, which the
/// game's and Studio's headers keep equal to it), the 15 mip sizes and the chunk size. Everything else (format,
/// flags, the group name) stays the template's; [`new_texture_header`] then sets the offsets, first mip, chunk GUID
/// and name hash as for any whole texture.
pub fn resize_header(template: &[u8], meta: &[u8], width: usize, height: usize, levels: usize) -> Result<Vec<u8>, TextureError> {
    let h = TextureHeader::parse(template, meta)?;
    if template.len() < 128 {
        return Err(TextureError("texture header too short to resize".into()));
    }
    if !(1..=15).contains(&levels) || width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(TextureError(format!("{width} x {height} with {levels} mips is not a texture the header can describe")));
    }
    let block = block_bytes(h.format).ok_or_else(|| TextureError(format!("texture format {} is not block compressed", h.format)))?;
    let sizes = mip_sizes(width, height, levels, block);
    let mut out = template.to_vec();
    out[22..24].copy_from_slice(&(width as u16).to_le_bytes());
    out[24..26].copy_from_slice(&(height as u16).to_le_bytes());
    out[30] = levels as u8;
    out[33] = levels as u8;
    for (i, slot) in out[56..116].chunks_exact_mut(4).enumerate() {
        slot.copy_from_slice(&(sizes.get(i).copied().unwrap_or(0) as u32).to_le_bytes());
    }
    out[116..120].copy_from_slice(&(sizes.iter().sum::<usize>() as u32).to_le_bytes());
    Ok(out)
}

/// A cloned header made into a new, whole texture (newdeck._add_texture / newwheel.add_texture): the encoded
/// offsets of the blocks holding mips 1 and 2 in `encoded` (the new chunk, CAS-encoded), first mip 0 (bytes
/// 31 and 32), the new chunk GUID (little-endian bytes) and the name hash.
pub fn new_texture_header(template: &[u8], meta: &[u8], encoded: &[u8], chunk: [u8; 16], name: &str) -> Result<Vec<u8>, TextureError> {
    let h = TextureHeader::parse(template, meta)?;
    if template.len() < 128 || h.mip_sizes.len() < 3 {
        return Err(TextureError("a texture header with at least 3 mips is needed".into()));
    }
    let mut out = template.to_vec();
    let mip1 = block_containing(encoded, h.mip_start(1) as u64)?;
    let mip2 = block_containing(encoded, h.mip_start(2) as u64)?;
    out[0..4].copy_from_slice(&mip1.to_le_bytes());
    out[4..8].copy_from_slice(&mip2.to_le_bytes());
    out[0x1F] = 0;
    out[0x20] = 0;
    out[40..56].copy_from_slice(&chunk);
    out[120..128].copy_from_slice(&owner_hash(name).to_le_bytes());
    Ok(out)
}
