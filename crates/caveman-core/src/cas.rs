//! CAS blocks: 8-byte headers, stored / Kraken payloads, decode and encode (fb.decode_cas, fbw.encode_cas,
//! fbw.block_offset).
//!
//! A block header is 8 bytes: a big-endian u32 whose low 24 bits are the decoded size, a compression byte (low 7
//! bits: 0x00 stored, 0x11/0x15/0x19 Oodle, 0x0F zstd), a byte whose low nibble holds bits 16-19 of the encoded size
//! (Caveman and the game write 0x7 in the high nibble), and a big-endian u16 with the encoded size's low 16 bits.

use std::fmt;
use std::path::Path;

use crate::oodle::{self, OodleError};

/// The decoded size of one block (`fbw.BLOCK`).
pub const BLOCK: usize = 0x10000;
/// The compression code of an Oodle Kraken block (`fbw.KRAKEN`).
pub const KRAKEN: u8 = 0x11;
/// The compression code of a stored block.
pub const STORED: u8 = 0x00;
/// The largest encoded payload a block header can express (20 bits).
pub const MAX_PAYLOAD: usize = 0xFFFFF;

#[derive(Debug)]
pub enum CasError {
    /// A block header or payload runs past the end of the data.
    Truncated {
        at: usize,
    },
    /// A compression code this reader does not handle (zstd 0x0F, or unknown).
    Compression(u8),
    /// An encoded block payload over 20 bits (`OverflowError` in `fbw.encode_cas`).
    PayloadTooLarge(usize),
    /// `block_offset` / `block_containing`: no block starts at, or holds, this logical offset.
    NoBlock(usize),
    Oodle(OodleError),
}

impl fmt::Display for CasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CasError::Truncated { at } => write!(f, "CAS data truncated at {at:#x}"),
            CasError::Compression(c) => write!(f, "compression {c:#x}"),
            CasError::PayloadTooLarge(n) => write!(f, "encoded CAS block is too large ({n} bytes)"),
            CasError::NoBlock(o) => write!(f, "no block at {o:#x}"),
            CasError::Oodle(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CasError {}

impl From<OodleError> for CasError {
    fn from(e: OodleError) -> Self {
        CasError::Oodle(e)
    }
}

/// One parsed block header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    /// Where the header starts in the encoded data.
    pub at: usize,
    /// Decoded size (24 bits).
    pub decoded: usize,
    /// Compression code (low 7 bits of byte 4).
    pub compression: u8,
    /// Encoded payload size (20 bits).
    pub encoded: usize,
}

impl BlockHeader {
    /// The offset just past this block's payload.
    pub fn end(&self) -> usize {
        self.at + 8 + self.encoded
    }
}

/// The header of the block at `at` (`None` when fewer than 8 bytes remain).
pub fn header_at(data: &[u8], at: usize) -> Option<BlockHeader> {
    if at.checked_add(8)? > data.len() {
        return None;
    }
    let decoded = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize & 0xFF_FFFF;
    let comp = data[at + 4] as u16 | ((data[at + 5] as u16) << 8);
    let encoded = u16::from_be_bytes([data[at + 6], data[at + 7]]) as usize + ((((comp >> 8) & 0xF) as usize) << 16);
    Some(BlockHeader { at, decoded, compression: (comp & 0x7F) as u8, encoded })
}

/// Every block header in `data`, in order (`newdeck._block_starts` gives their `at`s). Stops at the first header
/// that does not fit, as the Python helpers do.
pub fn blocks(data: &[u8]) -> Vec<BlockHeader> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(h) = header_at(data, at) {
        at = h.end();
        out.push(h);
    }
    out
}

/// The offsets where each block starts (`newdeck._block_starts`).
pub fn block_starts(data: &[u8]) -> Vec<usize> {
    blocks(data).into_iter().map(|h| h.at).collect()
}

/// Decodes CAS data (`fb.decode_cas`): stored blocks are copied, Oodle blocks decompressed with the game's
/// `oo2core` (loaded only when a compressed block is met). Returns the decoded bytes and each block's compression
/// code.
pub fn decode(data: &[u8], game_root: &Path) -> Result<(Vec<u8>, Vec<u8>), CasError> {
    let mut out = Vec::new();
    let mut codes = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let h = header_at(data, at).ok_or(CasError::Truncated { at })?;
        let payload = data.get(at + 8..h.end()).ok_or(CasError::Truncated { at })?;
        at = h.end();
        codes.push(h.compression);
        match h.compression {
            STORED => out.extend_from_slice(payload),
            0x11 | 0x15 | 0x19 => out.extend(oodle::get(game_root)?.decompress(payload, h.decoded)?),
            other => return Err(CasError::Compression(other)),
        }
    }
    Ok((out, codes))
}

/// Decodes CAS data, discarding the block codes.
pub fn decode_bytes(data: &[u8], game_root: &Path) -> Result<Vec<u8>, CasError> {
    Ok(decode(data, game_root)?.0)
}

fn push_block(out: &mut Vec<u8>, decoded: usize, compression: u8, payload: &[u8]) -> Result<(), CasError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(CasError::PayloadTooLarge(payload.len()));
    }
    let packed: u64 = (((decoded & 0xFF_FFFF) as u64) << 32) | ((compression as u64) << 24) | (0x7 << 20) | payload.len() as u64;
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// Stored (uncompressed) 64 KiB blocks (`fbw.encode_cas_raw`). Only for small payloads: a chunk the game streams
/// must be compressed like the game's own (see `encode`).
pub fn encode_raw(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8 * (data.len() / BLOCK + 1));
    if data.is_empty() {
        push_block(&mut out, 0, STORED, &[]).expect("an empty block fits");
        return out;
    }
    for part in data.chunks(BLOCK) {
        push_block(&mut out, part.len(), STORED, part).expect("a 64 KiB block fits");
    }
    out
}

/// 64 KiB blocks compressed with Oodle Kraken from the game's own `oo2core` (`fbw.encode_cas`, default level 4): a
/// block that does not shrink, or that Oodle fails on, is stored raw, exactly as the game does.
pub fn encode(data: &[u8], game_root: &Path) -> Result<Vec<u8>, CasError> {
    encode_level(data, game_root, oodle::DEFAULT_LEVEL)
}

/// `encode` at a given Oodle level.
pub fn encode_level(data: &[u8], game_root: &Path, level: i32) -> Result<Vec<u8>, CasError> {
    let o = oodle::get(game_root)?;
    let mut out = Vec::with_capacity(data.len() / 2 + 64);
    let parts: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(BLOCK).collect() };
    for part in parts {
        let packed = if part.is_empty() { None } else { o.compress(oodle::KRAKEN_COMPRESSOR, part, level).ok() };
        match packed {
            Some(c) if !c.is_empty() && c.len() < part.len() => push_block(&mut out, part.len(), KRAKEN, &c)?,
            _ => push_block(&mut out, part.len(), STORED, part)?,
        }
    }
    Ok(out)
}

/// Byte offset in `encoded` of the block that decodes to `logical` onwards (`fbw.block_offset`).
pub fn block_offset(encoded: &[u8], logical: usize) -> Result<usize, CasError> {
    let mut at = 0;
    let mut decoded = 0;
    while let Some(h) = header_at(encoded, at) {
        if decoded == logical {
            return Ok(at);
        }
        decoded += h.decoded;
        at = h.end();
    }
    Err(CasError::NoBlock(logical))
}

/// Byte offset in `encoded` of the block whose decoded range holds `logical` (`newwheel.block_containing`). A
/// texture header's bytes 0-7 are these offsets for mips 1 and 2; they equal `block_offset` when a mip starts on a
/// block boundary (decks), and differ when it starts mid-block (the game's 512 x 512 BC1 wheel masks).
pub fn block_containing(encoded: &[u8], logical: usize) -> Result<usize, CasError> {
    let mut at = 0;
    let mut decoded = 0;
    while let Some(h) = header_at(encoded, at) {
        if decoded <= logical && logical < decoded + h.decoded {
            return Ok(at);
        }
        decoded += h.decoded;
        at = h.end();
    }
    Err(CasError::NoBlock(logical))
}
