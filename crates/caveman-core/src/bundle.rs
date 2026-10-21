//! Bundle regions and binary bundle manifests: read and write (fb.read_bundle_region, fb.read_binary_bundle,
//! fbw.write_bundle_region, fbw.write_binary_bundle).
//!
//! A bundle's region (in a TOC) lists where each of its files lives: the manifest first, then every EBX, resource and
//! chunk, in manifest order. The manifest names the assets with their sizes and SHA-1s (of the encoded CAS bytes),
//! resource types, metas and ids, chunk GUIDs with their logical ranges, and the chunk metadata (a native DB).

use std::fmt;

use crate::toc::{identifier, Guid};

/// XORed into a manifest's magic (`fb.SALT`).
pub const SALT: u32 = 0x7065_636E;
/// A manifest's magic before the salt (`fb.BUNDLE_MAGIC`).
pub const BUNDLE_MAGIC: u32 = 0xED1C_EDB8;
/// The install chunk of a mod's own files: `configurations/layout/initialinstallpackage` (`fbw.MOD_INSTALL_CHUNK`).
pub const MOD_INSTALL_CHUNK: u32 = 3_162_564_249;
/// The archive number of a mod's own CAS file (`fbw.MOD_ARCHIVE`).
pub const MOD_ARCHIVE: u16 = 1;

#[derive(Debug)]
pub enum BundleError {
    /// A field points outside the data.
    Truncated(&'static str),
    /// The region's two file counts differ.
    CountMismatch,
    /// The manifest's declared size is not its length minus 4.
    SizeMismatch { declared: u32, actual: usize },
    /// Neither byte order gives the manifest magic.
    BadMagic,
    /// An asset name that is not UTF-8.
    BadName,
    /// A resource meta that is not 16 bytes.
    BadMeta(usize),
    /// A value too large for its field.
    TooLarge(&'static str),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Truncated(what) => write!(f, "bundle data truncated ({what})"),
            BundleError::CountMismatch => f.write_str("bundle region file counts differ"),
            BundleError::SizeMismatch { declared, actual } => {
                write!(f, "bundle size mismatch: declared {declared}, data {actual}")
            }
            BundleError::BadMagic => f.write_str("not a bundle manifest"),
            BundleError::BadName => f.write_str("a manifest name is not UTF-8"),
            BundleError::BadMeta(n) => write!(f, "a resource meta is {n} bytes, not 16"),
            BundleError::TooLarge(what) => write!(f, "{what} is too large for a manifest"),
        }
    }
}

impl std::error::Error for BundleError {}

/// Where one file of a bundle lives (`fb.read_bundle_region`'s file dicts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileLoc {
    pub patch: bool,
    pub install: u32,
    pub archive: u16,
    pub offset: u32,
    pub size: u32,
}

impl FileLoc {
    /// A location in a mod's own CAS file (`patch.LOC`): patch, the mod install chunk, archive 1.
    pub fn in_mod(offset: u32, size: u32) -> FileLoc {
        FileLoc { patch: true, install: MOD_INSTALL_CHUNK, archive: MOD_ARCHIVE, offset, size }
    }
}

fn be32(r: &[u8], o: usize) -> Result<u32, BundleError> {
    r.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or(BundleError::Truncated("region"))
}

/// Parses a bundle region (`fb.read_bundle_region`): the inline manifest, if the region carries one, and every
/// file's location (the manifest's first when not inline).
pub fn read_region(r: &[u8]) -> Result<(Option<Vec<u8>>, Vec<FileLoc>), BundleError> {
    let meta_off = be32(r, 0)? as i32;
    let meta_size = be32(r, 4)? as i32;
    let loc_off = be32(r, 8)? as usize;
    let total = be32(r, 12)? as i32;
    let data_off = be32(r, 16)? as usize;
    let total2 = be32(r, 32)? as i32;
    if total != total2 {
        return Err(BundleError::CountMismatch);
    }
    let inline = if meta_size != 0 {
        let (a, n) = (meta_off as usize, meta_size as usize);
        Some(r.get(a..a + n).ok_or(BundleError::Truncated("inline manifest"))?.to_vec())
    } else {
        None
    };
    let mut files = Vec::with_capacity(total.max(0) as usize);
    let mut at = data_off;
    let mut cur: Option<(bool, u32, u16)> = None;
    for i in 0..total.max(0) as usize {
        let flag = *r.get(loc_off + i).ok_or(BundleError::Truncated("location flags"))?;
        if flag == 1 {
            let e = be32(r, at)?;
            at += 4;
            cur = Some((e & 0x00FF_0000 != 0, (e >> 8) & 0xFF, (e & 0xFF) as u16));
        } else if flag != 0 {
            let v = u64::from_be_bytes(r.get(at..at + 8).ok_or(BundleError::Truncated("location"))?.try_into().expect("8"));
            at += 8;
            cur = Some((v & (0xFF << 48) != 0, ((v >> 16) & 0xFFFF_FFFF) as u32, (v & 0xFFFF) as u16));
        }
        let (patch, install, archive) = cur.ok_or(BundleError::Truncated("first location"))?;
        let offset = be32(r, at)?;
        let size = be32(r, at + 4)?;
        at += 8;
        files.push(FileLoc { patch, install, archive, offset, size });
    }
    Ok((inline, files))
}

/// A bundle region for `files` (`fbw.write_bundle_region`): a full location record whenever the (patch, install,
/// archive) triple changes, the first flag marked 0x84.
pub fn write_region(files: &[FileLoc]) -> Vec<u8> {
    let mut table = Vec::with_capacity(files.len() * 16);
    let mut flags = Vec::with_capacity(files.len());
    let mut previous: Option<(bool, u32, u16)> = None;
    for f in files {
        let loc = (f.patch, f.install, f.archive);
        if previous == Some(loc) {
            flags.push(0u8);
        } else {
            flags.push(0x80);
            table.extend_from_slice(&identifier(f.patch, f.install, f.archive).to_be_bytes());
            previous = Some(loc);
        }
        table.extend_from_slice(&f.offset.to_be_bytes());
        table.extend_from_slice(&f.size.to_be_bytes());
    }
    if let Some(first) = flags.first_mut() {
        *first |= 0x04;
    }
    let data_offset: u32 = 0x24;
    let location_offset = data_offset + table.len() as u32;
    let n = files.len() as u32;
    let mut out = Vec::with_capacity(0x24 + table.len() + flags.len());
    for v in [0, 0, location_offset, n, data_offset, data_offset, data_offset, 0, n] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&table);
    out.extend_from_slice(&flags);
    out
}

/// The byte order of a manifest's body (the leading total size is always big endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Big,
    Little,
}

/// An EBX entry of a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEbx {
    pub name: String,
    pub size: u32,
    pub sha1: [u8; 20],
}

/// A resource entry of a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRes {
    pub name: String,
    pub size: u32,
    pub sha1: [u8; 20],
    /// The resource type (`0x6BDE20BA` textures, ...).
    pub rtype: u32,
    pub meta: [u8; 16],
    /// The resource id (odd for every one the game ships).
    pub rid: u64,
}

/// A chunk entry of a manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestChunk {
    pub guid: Guid,
    pub logical_offset: u32,
    pub logical_size: u32,
    pub sha1: [u8; 20],
}

/// A parsed bundle manifest (`fb.read_binary_bundle`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub endian: Endian,
    pub ebx: Vec<ManifestEbx>,
    pub res: Vec<ManifestRes>,
    pub chunks: Vec<ManifestChunk>,
    /// The chunk metadata (a native DB; `ndb::read`), empty when the bundle has none.
    pub chunk_meta: Vec<u8>,
}

/// Parses a binary bundle manifest (`fb.read_binary_bundle`).
pub fn read_manifest(d: &[u8]) -> Result<Manifest, BundleError> {
    let declared = be32(d, 0)?;
    if declared as usize != d.len().wrapping_sub(4) {
        return Err(BundleError::SizeMismatch { declared, actual: d.len() });
    }
    let endian = if be32(d, 4)? ^ SALT == BUNDLE_MAGIC {
        Endian::Big
    } else {
        let le = u32::from_le_bytes(d.get(4..8).ok_or(BundleError::Truncated("magic"))?.try_into().expect("4"));
        if le ^ SALT != BUNDLE_MAGIC {
            return Err(BundleError::BadMagic);
        }
        Endian::Little
    };
    let u = |o: usize| -> Result<u32, BundleError> {
        let b: [u8; 4] = d.get(o..o + 4).ok_or(BundleError::Truncated("manifest"))?.try_into().expect("4");
        Ok(match endian {
            Endian::Big => u32::from_be_bytes(b),
            Endian::Little => u32::from_le_bytes(b),
        })
    };
    let u64_at = |o: usize| -> Result<u64, BundleError> {
        let b: [u8; 8] = d.get(o..o + 8).ok_or(BundleError::Truncated("manifest"))?.try_into().expect("8");
        Ok(match endian {
            Endian::Big => u64::from_be_bytes(b),
            Endian::Little => u64::from_le_bytes(b),
        })
    };
    let (total, ne, nr, nc) = (u(8)? as usize, u(12)? as usize, u(16)? as usize, u(20)? as usize);
    let strings = u(24)? as usize + 4;
    let (meta_off, meta_size) = (u(28)? as usize, u(32)? as usize);
    let mut at = 36;
    let hash = |i: usize| -> Result<[u8; 20], BundleError> {
        Ok(d.get(36 + 20 * i..56 + 20 * i).ok_or(BundleError::Truncated("SHA-1"))?.try_into().expect("20"))
    };
    if total < ne + nr + nc {
        return Err(BundleError::Truncated("asset counts"));
    }
    at += 20 * total;
    let name = |off: usize| -> Result<String, BundleError> {
        let rest = d.get(strings + off..).ok_or(BundleError::Truncated("name"))?;
        let end = rest.iter().position(|&b| b == 0).ok_or(BundleError::Truncated("name"))?;
        Ok(std::str::from_utf8(&rest[..end]).map_err(|_| BundleError::BadName)?.to_string())
    };
    let mut ebx = Vec::with_capacity(ne);
    for i in 0..ne {
        ebx.push(ManifestEbx { name: name(u(at)? as usize)?, size: u(at + 4)?, sha1: hash(i)? });
        at += 8;
    }
    let mut res = Vec::with_capacity(nr);
    for i in 0..nr {
        res.push(ManifestRes {
            name: name(u(at)? as usize)?,
            size: u(at + 4)?,
            sha1: hash(ne + i)?,
            rtype: 0,
            meta: [0; 16],
            rid: 0,
        });
        at += 8;
    }
    for r in res.iter_mut() {
        r.rtype = u(at)?;
        at += 4;
    }
    for r in res.iter_mut() {
        r.meta = d.get(at..at + 16).ok_or(BundleError::Truncated("meta"))?.try_into().expect("16");
        at += 16;
    }
    for r in res.iter_mut() {
        r.rid = u64_at(at)?;
        at += 8;
    }
    let mut chunks = Vec::with_capacity(nc);
    for i in 0..nc {
        let mut g: [u8; 16] = d.get(at..at + 16).ok_or(BundleError::Truncated("chunk"))?.try_into().expect("16");
        if endian == Endian::Big {
            g.reverse();
        }
        chunks.push(ManifestChunk {
            guid: Guid::from_le_bytes(g),
            logical_offset: u(at + 16)?,
            logical_size: u(at + 20)?,
            sha1: hash(ne + nr + i)?,
        });
        at += 24;
    }
    let chunk_meta = if meta_size != 0 {
        d.get(4 + meta_off..4 + meta_off + meta_size).ok_or(BundleError::Truncated("chunk metadata"))?.to_vec()
    } else {
        Vec::new()
    };
    Ok(Manifest { endian, ebx, res, chunks, chunk_meta })
}

/// A manifest in the game's little-endian form (`fbw.write_binary_bundle`): the total size big endian, then the
/// header, SHA-1s, entries, names (EBX then resources, each once, in order) and the chunk metadata.
pub fn write_manifest(m: &Manifest) -> Result<Vec<u8>, BundleError> {
    let total = m.ebx.len() + m.res.len() + m.chunks.len();
    let mut strings = Vec::new();
    let mut offsets = Vec::with_capacity(m.ebx.len() + m.res.len());
    for n in m.ebx.iter().map(|a| &a.name).chain(m.res.iter().map(|a| &a.name)) {
        offsets.push(u32::try_from(strings.len()).map_err(|_| BundleError::TooLarge("names"))?);
        strings.extend_from_slice(n.as_bytes());
        strings.push(0);
    }
    let sha_start = 36;
    let ebx_start = sha_start + total * 20;
    let string_start = ebx_start + m.ebx.len() * 8 + m.res.len() * (8 + 4 + 16 + 8) + m.chunks.len() * 24;
    let meta = &m.chunk_meta;
    let meta_start = string_start + strings.len();
    let length = meta_start + meta.len();
    let length32 = u32::try_from(length).map_err(|_| BundleError::TooLarge("manifest"))?;
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(&(length32 - 4).to_be_bytes());
    let header = [
        BUNDLE_MAGIC ^ SALT,
        total as u32,
        m.ebx.len() as u32,
        m.res.len() as u32,
        m.chunks.len() as u32,
        (string_start - 4) as u32,
        if meta.is_empty() { 0 } else { (meta_start - 4) as u32 },
        meta.len() as u32,
    ];
    for v in header {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for s in m.ebx.iter().map(|a| &a.sha1).chain(m.res.iter().map(|a| &a.sha1)).chain(m.chunks.iter().map(|a| &a.sha1)) {
        out.extend_from_slice(s);
    }
    for (i, a) in m.ebx.iter().enumerate() {
        out.extend_from_slice(&offsets[i].to_le_bytes());
        out.extend_from_slice(&a.size.to_le_bytes());
    }
    for (i, a) in m.res.iter().enumerate() {
        out.extend_from_slice(&offsets[m.ebx.len() + i].to_le_bytes());
        out.extend_from_slice(&a.size.to_le_bytes());
    }
    for a in &m.res {
        out.extend_from_slice(&a.rtype.to_le_bytes());
    }
    for a in &m.res {
        out.extend_from_slice(&a.meta);
    }
    for a in &m.res {
        out.extend_from_slice(&a.rid.to_le_bytes());
    }
    for c in &m.chunks {
        out.extend_from_slice(&c.guid.le_bytes());
        out.extend_from_slice(&c.logical_offset.to_le_bytes());
        out.extend_from_slice(&c.logical_size.to_le_bytes());
    }
    out.extend_from_slice(&strings);
    out.extend_from_slice(meta);
    debug_assert_eq!(out.len(), length);
    Ok(out)
}

/// The SHA-1 of bytes (a manifest entry's hash is the SHA-1 of the asset's encoded CAS bytes; a chunk's, of its whole
/// encoded data even when only a tail is resident).
pub fn sha1(data: &[u8]) -> [u8; 20] {
    sha1_smol::Sha1::from(data).digest().bytes()
}

impl Manifest {
    /// The index in the bundle's file list (after the manifest) of the EBX named `name`.
    pub fn ebx_index(&self, name: &str) -> Option<usize> {
        self.ebx.iter().position(|e| e.name == name)
    }

    /// The index among the resources of the resource named `name`.
    pub fn res_index(&self, name: &str) -> Option<usize> {
        self.res.iter().position(|r| r.name == name)
    }

    /// The index among the chunks of the chunk `guid`.
    pub fn chunk_index(&self, guid: &Guid) -> Option<usize> {
        self.chunks.iter().position(|c| c.guid == *guid)
    }

    /// The position in the region's file list (manifest at 0) of EBX `i`.
    pub fn ebx_file(&self, i: usize) -> usize {
        1 + i
    }

    /// The position in the region's file list of resource `i`.
    pub fn res_file(&self, i: usize) -> usize {
        1 + self.ebx.len() + i
    }

    /// The position in the region's file list of chunk `i`.
    pub fn chunk_file(&self, i: usize) -> usize {
        1 + self.ebx.len() + self.res.len() + i
    }
}
