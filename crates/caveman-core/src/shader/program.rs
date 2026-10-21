//! The `shaders/bytecode/<guid>` resource (type `0x0E2B02F7`): a 0x20-byte header, the RC2S binding block, the DXBC
//! container, padding to 16 bytes and two relocations (`dxc.read_bytecode_resource`, `shaderpatch.wrap`).

/// The parts of a bytecode resource (`dxc.read_bytecode_resource`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BytecodeResource<'a> {
    /// The 0x20-byte header.
    pub header: &'a [u8],
    /// Header word 3: the body length (RC2S block plus DXBC).
    pub blob_size: u32,
    /// Header word 4: the data size (the end of the padded program data).
    pub data_size: u32,
    /// The RC2S block between the header and the DXBC (its last u32 is the DXBC size).
    pub pre: &'a [u8],
    /// Where the DXBC starts.
    pub dxbc_at: usize,
    /// The DXBC container.
    pub dxbc: &'a [u8],
    /// Everything after the DXBC (padding and relocations).
    pub tail: &'a [u8],
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Splits a bytecode resource; None when it holds no DXBC (`dxc.read_bytecode_resource`'s `{'dxbc': None}`).
pub fn read_bytecode_resource(data: &[u8]) -> Option<BytecodeResource<'_>> {
    let at = data.windows(4).position(|w| w == b"DXBC")?;
    let size = u32_at(data, at + 24)? as usize;
    let end = (at + size).min(data.len());
    Some(BytecodeResource {
        header: data.get(..0x20)?,
        blob_size: u32_at(data, 12)?,
        data_size: u32_at(data, 16)?,
        pre: data.get(0x20..at)?,
        dxbc_at: at,
        dxbc: &data[at..end],
        tail: &data[end..],
    })
}

/// A bytecode resource like `original` (its header and RC2S block) holding `dxbc` (`shaderpatch.wrap`): the RC2S
/// block's last word set to the DXBC size, the header's body length and data size updated, zero padding to 16 bytes
/// and the two relocations (0x10, 0x18). Returns (payload, resource meta).
pub fn wrap(original: &[u8], dxbc: &[u8]) -> Result<(Vec<u8>, [u8; 16]), String> {
    let at = original.windows(4).position(|w| w == b"DXBC").ok_or("no DXBC in the original program")?;
    if at < 0x24 {
        return Err("the original program has no RC2S block".into());
    }
    let mut pre = original[0x20..at].to_vec();
    let n = pre.len();
    pre[n - 4..].copy_from_slice(&u32::try_from(dxbc.len()).map_err(|_| "DXBC over 4 GiB")?.to_le_bytes());
    let body_len = pre.len() + dxbc.len();
    let end = 0x20 + body_len;
    let data_size = end.div_ceil(16) * 16; // the program data is padded to 16 bytes (checked on all six game programs)
    let mut head = original[..0x20].to_vec();
    head[12..16].copy_from_slice(&(body_len as u32).to_le_bytes());
    head[0x10..0x18].copy_from_slice(&(data_size as u64).to_le_bytes());
    head[0x18..0x20].copy_from_slice(&0x20u64.to_le_bytes());
    let mut payload = Vec::with_capacity(data_size + 8);
    payload.extend_from_slice(&head);
    payload.extend_from_slice(&pre);
    payload.extend_from_slice(dxbc);
    payload.resize(data_size, 0);
    payload.extend_from_slice(&0x10u32.to_le_bytes());
    payload.extend_from_slice(&0x18u32.to_le_bytes());
    let mut meta = [0u8; 16];
    meta[..4].copy_from_slice(&(data_size as u32).to_le_bytes());
    meta[4..8].copy_from_slice(&8u32.to_le_bytes());
    Ok((payload, meta))
}
