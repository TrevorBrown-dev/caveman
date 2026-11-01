//! Superbundle TOCs: read (Huffman names) and write patch TOCs with perfect hashes (fb.read_toc, fbw.write_patch_toc).
//!
//! A TOC file is a 0x22C-byte envelope (magic `00 D1 CE 01`) followed by a big-endian body: a 48-byte header, the
//! bundle hash table and records (name offset, load flag and region size, region offset), the chunk hash table,
//! GUIDs and location words, the names (plain, or Huffman-coded when flag 4 is set, as in the game's own TOCs), and
//! the bundle regions.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// The envelope before a TOC's body (`fb.ENVELOPE`).
pub const ENVELOPE: usize = 0x22C;
/// The envelope's first four bytes.
pub const MAGIC: [u8; 4] = [0x00, 0xD1, 0xCE, 0x01];
/// The FNV prime `fbw.toc_hash` uses (32-bit FNV-1, signed bytes, reduced modulo the prime).
pub const FNV_PRIME: u32 = 0x0100_0193;
/// The FNV offset basis `fbw.toc_hash` starts from.
pub const FNV_OFFSET: u32 = 0x811C_9DC5;

/// A GUID in Frostbite's in-memory (little-endian, `uuid.bytes_le`) byte order: what bundle manifests, texture
/// headers and EBX hold, and what `fbw.guid_bytes` returns. `Display` / `FromStr` use the usual text form, as the
/// Python toolkit's `guid_str` / `uuid.UUID(text)`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// From the in-memory (little-endian) bytes.
    pub fn from_le_bytes(b: [u8; 16]) -> Guid {
        Guid(b)
    }

    /// From the big-endian (RFC 4122 text order) bytes, as a TOC chunk table stores them reversed.
    pub fn from_be_bytes(b: [u8; 16]) -> Guid {
        let mut le = b;
        le[0..4].reverse();
        le[4..6].reverse();
        le[6..8].reverse();
        Guid(le)
    }

    /// The in-memory (little-endian) bytes.
    pub fn le_bytes(&self) -> [u8; 16] {
        self.0
    }

    /// The RFC 4122 (text order) bytes.
    pub fn be_bytes(&self) -> [u8; 16] {
        let mut b = self.0;
        b[0..4].reverse();
        b[4..6].reverse();
        b[6..8].reverse();
        b
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.be_bytes();
        for (i, x) in b.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{x:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

impl FromStr for Guid {
    type Err = TocError;

    fn from_str(s: &str) -> Result<Guid, TocError> {
        let hex: String = s.trim_matches(|c| c == '{' || c == '}').chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 {
            return Err(TocError::Guid(s.to_string()));
        }
        let mut b = [0u8; 16];
        for (i, slot) in b.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| TocError::Guid(s.to_string()))?;
        }
        Ok(Guid::from_be_bytes(b))
    }
}

#[derive(Debug)]
pub enum TocError {
    Io(std::io::Error),
    /// Not a TOC: the envelope magic is wrong.
    BadMagic,
    /// A field points outside the file.
    Truncated(&'static str),
    /// A Huffman-coded name that never terminates.
    UnterminatedName,
    /// A name that is not UTF-8 (plain names).
    BadName,
    /// `perfect_hash` / `write_patch_toc`: duplicate keys.
    DuplicateKeys,
    /// Text that is not a GUID.
    Guid(String),
    /// A value too large for its field.
    TooLarge(&'static str),
}

impl fmt::Display for TocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TocError::Io(e) => write!(f, "{e}"),
            TocError::BadMagic => f.write_str("bad TOC envelope"),
            TocError::Truncated(what) => write!(f, "TOC truncated ({what})"),
            TocError::UnterminatedName => f.write_str("unterminated name"),
            TocError::BadName => f.write_str("a TOC name is not UTF-8"),
            TocError::DuplicateKeys => f.write_str("TOC perfect-hash keys must be unique"),
            TocError::Guid(s) => write!(f, "not a GUID: {s:?}"),
            TocError::TooLarge(what) => write!(f, "{what} is too large for a TOC"),
        }
    }
}

impl std::error::Error for TocError {}

impl From<std::io::Error> for TocError {
    fn from(e: std::io::Error) -> Self {
        TocError::Io(e)
    }
}

/// A bundle record of a TOC (`fb.read_toc`'s bundle dicts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocBundle {
    pub name: String,
    /// The load flag (top two bits of the size word); 1 means the region is in this TOC.
    pub load: u32,
    /// The bundle region (`fb.read_bundle_region` input), when `load` is 1.
    pub region: Option<Vec<u8>>,
    pub region_off: u64,
    pub size: u32,
}

/// A chunk record of a TOC (`fb.read_toc`'s chunk dicts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TocChunk {
    pub guid: Guid,
    /// The record's location word is -1: the patch removes the chunk.
    pub removed: bool,
    pub patch: bool,
    pub install: u32,
    pub archive: u16,
    pub offset: u32,
    pub size: u32,
}

/// A parsed TOC (`fb.read_toc`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toc {
    pub flags: u32,
    pub bundles: Vec<TocBundle>,
    pub chunks: Vec<TocChunk>,
}

fn be32(d: &[u8], o: usize) -> Result<u32, TocError> {
    d.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])).ok_or(TocError::Truncated("word"))
}

fn be64(d: &[u8], o: usize) -> Result<u64, TocError> {
    d.get(o..o + 8).map(|b| u64::from_be_bytes(b.try_into().expect("8 bytes"))).ok_or(TocError::Truncated("offset"))
}

/// The Huffman name decoder of the game's TOCs (`fb.Huffman`).
struct Huffman {
    words: Vec<u32>,
    /// (value, left, right); a leaf has no children.
    nodes: Vec<(u32, Option<usize>, Option<usize>)>,
    root: Option<usize>,
}

impl Huffman {
    fn new(d: &[u8], names_off: usize, names_count: usize, table_off: usize, table_count: usize) -> Result<Huffman, TocError> {
        let words = (0..names_count).map(|i| be32(d, names_off + 4 * i)).collect::<Result<Vec<_>, _>>()?;
        let mut nodes: Vec<(u32, Option<usize>, Option<usize>)> = Vec::new();
        let mut by_value = std::collections::HashMap::new();
        let mut left: Option<usize> = None;
        let mut counter: u32 = 0;
        let mut root = None;
        for i in 0..table_count {
            let v = be32(d, table_off + 4 * i)?;
            let node = match by_value.get(&v) {
                Some(&n) => n,
                None => {
                    nodes.push((v, None, None));
                    by_value.insert(v, nodes.len() - 1);
                    nodes.len() - 1
                }
            };
            match left {
                None => left = Some(node),
                Some(l) => {
                    let parent = nodes.len();
                    nodes.push((counter, Some(l), Some(node)));
                    by_value.entry(counter).or_insert(parent);
                    root = Some(parent);
                    counter += 1;
                    left = None;
                }
            }
        }
        Ok(Huffman { words, nodes, root })
    }

    fn decode(&self, mut bit: usize) -> Result<String, TocError> {
        let root = self.root.ok_or(TocError::UnterminatedName)?;
        let total = self.words.len() * 32;
        let mut out = String::new();
        while bit < total {
            let mut n = root;
            while let (Some(l), Some(r)) = (self.nodes[n].1, self.nodes[n].2) {
                let word = *self.words.get(bit >> 5).ok_or(TocError::UnterminatedName)?;
                n = if (word >> (bit & 31)) & 1 == 1 { r } else { l };
                bit += 1;
            }
            let ch = !self.nodes[n].0 & 0xFFFF;
            if ch == 0 {
                return Ok(out);
            }
            out.push(char::from_u32(ch).ok_or(TocError::BadName)?);
        }
        Err(TocError::UnterminatedName)
    }
}

/// Reads a TOC file (`fb.read_toc`).
pub fn read_file(path: &Path) -> Result<Toc, TocError> {
    read(&std::fs::read(path)?)
}

/// Parses TOC bytes, envelope included (`fb.read_toc`).
pub fn read(raw: &[u8]) -> Result<Toc, TocError> {
    if raw.len() < ENVELOPE || raw[..4] != MAGIC {
        return Err(TocError::BadMagic);
    }
    let d = &raw[ENVELOPE..];
    let w = |i: usize| be32(d, 4 * i);
    let (_bundle_hash_off, bundle_data_off, bundle_count) = (w(0)?, w(1)? as usize, w(2)? as i32);
    let (_chunk_hash_off, chunk_guid_off, chunk_count) = (w(3)?, w(4)? as usize, w(5)? as i32);
    let (names_off, chunk_data_off, data_count, flags) = (w(8)? as usize, w(9)? as usize, w(10)? as i32, w(11)?);
    let huff = if flags & 4 != 0 {
        let (names_count, table_count, table_off) = (be32(d, 48)? as usize, be32(d, 52)? as usize, be32(d, 56)? as usize);
        Some(Huffman::new(d, names_off, names_count, table_off, table_count)?)
    } else {
        None
    };
    let mut bundles = Vec::with_capacity(bundle_count.max(0) as usize);
    for i in 0..bundle_count.max(0) as usize {
        let o = bundle_data_off + 16 * i;
        let name_off = be32(d, o)? as usize;
        let size_flag = be32(d, o + 4)?;
        let region_off = be64(d, o + 8)?;
        let size = size_flag & 0x3FFF_FFFF;
        let load = size_flag >> 30;
        let name = match &huff {
            Some(h) => h.decode(name_off)?,
            None => {
                let start = names_off + name_off;
                let rest = d.get(start..).ok_or(TocError::Truncated("name"))?;
                let end = rest.iter().position(|&b| b == 0).ok_or(TocError::Truncated("name"))?;
                std::str::from_utf8(&rest[..end]).map_err(|_| TocError::BadName)?.to_string()
            }
        };
        let region = if load == 1 {
            let start = usize::try_from(region_off).map_err(|_| TocError::Truncated("region"))?;
            Some(d.get(start..start + size as usize).ok_or(TocError::Truncated("region"))?.to_vec())
        } else {
            None
        };
        bundles.push(TocBundle { name, load, region, region_off, size });
    }
    let mut chunks = Vec::with_capacity(chunk_count.max(0) as usize);
    if chunk_count > 0 {
        let words = (0..data_count.max(0) as usize).map(|i| be32(d, chunk_data_off + 4 * i)).collect::<Result<Vec<_>, _>>()?;
        for i in 0..chunk_count as usize {
            let o = chunk_guid_off + 20 * i;
            let g = d.get(o..o + 16).ok_or(TocError::Truncated("chunk GUID"))?;
            let mut raw16 = [0u8; 16];
            for (k, b) in g.iter().rev().enumerate() {
                raw16[k] = *b;
            }
            let guid = Guid::from_le_bytes(raw16);
            let enc = be32(d, o + 16)? as i32;
            if enc == -1 {
                chunks.push(TocChunk { guid, removed: true, patch: false, install: 0, archive: 0, offset: 0, size: 0 });
                continue;
            }
            let wi = (enc as u32 & 0xFF_FFFF) as usize;
            let word = |k: usize| words.get(wi + k).copied().ok_or(TocError::Truncated("chunk data"));
            let ident = ((word(0)? as u64) << 32) | word(1)? as u64;
            chunks.push(TocChunk {
                guid,
                removed: false,
                patch: ident & (0xFF << 48) != 0,
                install: ((ident >> 16) & 0xFFFF_FFFF) as u32,
                archive: (ident & 0xFFFF) as u16,
                offset: word(2)?,
                size: word(3)?,
            });
        }
    }
    Ok(Toc { flags, bundles, chunks })
}

/// The TOC key hash (`fbw.toc_hash`): 32-bit FNV-1 over the key's bytes taken as signed, reduced modulo the prime.
pub fn toc_hash(key: &[u8], seed: u32) -> u32 {
    let mut h = seed;
    for &b in key {
        h = h.wrapping_mul(FNV_PRIME) ^ (b as i8 as i32 as u32);
    }
    h % FNV_PRIME
}

/// The perfect-hash table of a TOC's bundles or chunks (`fbw.perfect_hash`): returns (mapping, order), where
/// `mapping[slot]` is a seed (> 0) or `-free - 1`, and `order[slot]` the key index stored at that slot.
pub fn perfect_hash(keys: &[Vec<u8>]) -> Result<(Vec<i32>, Vec<usize>), TocError> {
    let size = keys.len();
    let mut mapping = vec![-1i32; size];
    let mut order: Vec<Option<usize>> = vec![None; size];
    if size == 0 {
        return Ok((mapping, Vec::new()));
    }
    if keys.iter().collect::<HashSet<_>>().len() != size {
        return Err(TocError::DuplicateKeys);
    }
    let modulo = size as u32;
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); size];
    for (i, k) in keys.iter().enumerate() {
        buckets[(toc_hash(k, FNV_OFFSET) % modulo) as usize].push(i);
    }
    // stable, as std::stable_sort (Python's list.sort with key=-len)
    buckets.sort_by_key(|b| std::cmp::Reverse(b.len()));
    let mut used = vec![false; size];
    let mut bi = 0;
    while bi < buckets.len() && buckets[bi].len() > 1 {
        let bucket = &buckets[bi];
        let mut seed: u32 = 1;
        let indices = loop {
            let mut indices: Vec<usize> = Vec::with_capacity(bucket.len());
            let mut collision = false;
            for &member in bucket {
                let idx = (toc_hash(&keys[member], seed) % modulo) as usize;
                if used[idx] || indices.contains(&idx) {
                    collision = true;
                    break;
                }
                indices.push(idx);
            }
            if !collision {
                break indices;
            }
            seed += 1;
        };
        mapping[(toc_hash(&keys[bucket[0]], FNV_OFFSET) % modulo) as usize] = i32::try_from(seed).map_err(|_| TocError::TooLarge("perfect-hash seed"))?;
        for (j, &member) in bucket.iter().enumerate() {
            order[indices[j]] = Some(member);
            used[indices[j]] = true;
        }
        bi += 1;
    }
    let mut free = 0usize;
    while bi < buckets.len() && !buckets[bi].is_empty() {
        while used[free] {
            free = (free + 1) % size;
        }
        let member = buckets[bi][0];
        mapping[(toc_hash(&keys[member], FNV_OFFSET) % modulo) as usize] = -(free as i32) - 1;
        order[free] = Some(member);
        used[free] = true;
        free = (free + 1) % size;
        bi += 1;
    }
    let order = order.into_iter().map(|o| o.expect("every slot is filled")).collect();
    Ok((mapping, order))
}

/// A bundle to write into a patch TOC (`fbw.write_patch_toc`'s bundle dicts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchBundle {
    pub name: String,
    pub region: Vec<u8>,
    pub load: u32,
}

/// A chunk to write into a patch TOC (`fbw.write_patch_toc`'s chunk dicts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchChunk {
    pub guid: Guid,
    pub patch: bool,
    pub install: u32,
    pub archive: u16,
    pub offset: u32,
    pub size: u32,
}

/// A file location's 64-bit identifier: patch flag at bit 48, install chunk at 16, archive in the low bits
/// (`fbw.identifier`).
pub fn identifier(patch: bool, install: u32, archive: u16) -> u64 {
    ((patch as u64) << 48) | ((install as u64) << 16) | archive as u64
}

fn align(v: usize, a: usize) -> usize {
    (v + a - 1) & !(a - 1)
}

fn put32(body: &mut [u8], o: usize, v: u32) {
    body[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

/// A patch TOC, envelope included (`fbw.write_patch_toc`; flags 3 as Studio's and Caveman's mods).
pub fn write_patch_toc(bundles: &[PatchBundle], chunks: &[PatchChunk], flags: u32) -> Result<Vec<u8>, TocError> {
    let bkeys: Vec<Vec<u8>> = bundles.iter().map(|b| b.name.to_lowercase().into_bytes()).collect();
    let (bmap, border) = perfect_hash(&bkeys)?;
    let ckeys: Vec<Vec<u8>> = chunks.iter().map(|c| c.guid.le_bytes().to_vec()).collect();
    let (cmap, corder) = perfect_hash(&ckeys)?;
    let mut names = Vec::new();
    let mut name_offsets = Vec::with_capacity(border.len());
    for &src in &border {
        name_offsets.push(names.len());
        names.extend_from_slice(bundles[src].name.as_bytes());
        names.push(0);
    }
    let mut words: Vec<u32> = Vec::with_capacity(chunks.len() * 4);
    let mut ctable = Vec::with_capacity(chunks.len() * 20);
    for &src in &corder {
        let c = &chunks[src];
        let mut g = c.guid.le_bytes();
        g.reverse();
        ctable.extend_from_slice(&g);
        let wi = u32::try_from(words.len()).map_err(|_| TocError::TooLarge("chunk table"))?;
        ctable.extend_from_slice(&((0x80u32 << 24) | wi).to_be_bytes());
        let ident = identifier(c.patch, c.install, c.archive);
        words.extend_from_slice(&[(ident >> 32) as u32, ident as u32, c.offset, c.size]);
    }
    let mut pos = 48;
    let bundle_hash_off = pos;
    pos = align(pos + bundles.len() * 4, 8);
    let bundle_data_off = pos;
    pos = align(pos + bundles.len() * 16, 4);
    let chunk_hash_off = pos;
    pos = align(pos + chunks.len() * 4, 4);
    let chunk_guid_off = pos;
    pos = align(pos + ctable.len(), 4);
    let chunk_data_off = pos;
    pos += words.len() * 4;
    let names_off = pos;
    pos = align(pos + names.len(), 4);
    let mut region_offs = Vec::with_capacity(border.len());
    for &src in &border {
        region_offs.push(pos);
        pos = align(pos + bundles[src].region.len(), 4);
    }
    let mut body = vec![0u8; pos];
    let header: [u32; 12] = [
        bundle_hash_off as u32,
        bundle_data_off as u32,
        bundles.len() as u32,
        if chunks.is_empty() { 0 } else { chunk_hash_off as u32 },
        if chunks.is_empty() { 0 } else { chunk_guid_off as u32 },
        chunks.len() as u32,
        chunk_data_off as u32,
        chunk_data_off as u32,
        names_off as u32,
        chunk_data_off as u32,
        words.len() as u32,
        flags,
    ];
    for (i, v) in header.iter().enumerate() {
        put32(&mut body, 4 * i, *v);
    }
    for (i, v) in bmap.iter().enumerate() {
        put32(&mut body, bundle_hash_off + 4 * i, *v as u32);
    }
    for (i, &src) in border.iter().enumerate() {
        let b = &bundles[src];
        let size = u32::try_from(b.region.len()).map_err(|_| TocError::TooLarge("bundle region"))?;
        let o = bundle_data_off + 16 * i;
        put32(&mut body, o, name_offsets[i] as u32);
        put32(&mut body, o + 4, (b.load << 30) | size);
        body[o + 8..o + 16].copy_from_slice(&(region_offs[i] as u64).to_be_bytes());
    }
    if !chunks.is_empty() {
        for (i, v) in cmap.iter().enumerate() {
            put32(&mut body, chunk_hash_off + 4 * i, *v as u32);
        }
        body[chunk_guid_off..chunk_guid_off + ctable.len()].copy_from_slice(&ctable);
        for (i, w) in words.iter().enumerate() {
            put32(&mut body, chunk_data_off + 4 * i, *w);
        }
    }
    body[names_off..names_off + names.len()].copy_from_slice(&names);
    for (i, &src) in border.iter().enumerate() {
        let r = &bundles[src].region;
        body[region_offs[i]..region_offs[i] + r.len()].copy_from_slice(r);
    }
    let mut out = vec![0u8; ENVELOPE];
    out[..4].copy_from_slice(&MAGIC);
    out.extend_from_slice(&body);
    Ok(out)
}

impl Toc {
    /// The bundles and live chunks of a parsed patch TOC as `write_patch_toc` input (bundles without a region, and
    /// removed chunks, have no writer form and are left out).
    pub fn to_patch_input(&self) -> (Vec<PatchBundle>, Vec<PatchChunk>) {
        let bundles =
            self.bundles.iter().filter_map(|b| b.region.as_ref().map(|r| PatchBundle { name: b.name.clone(), region: r.clone(), load: b.load })).collect();
        let chunks = self
            .chunks
            .iter()
            .filter(|c| !c.removed)
            .map(|c| PatchChunk { guid: c.guid, patch: c.patch, install: c.install, archive: c.archive, offset: c.offset, size: c.size })
            .collect();
        (bundles, chunks)
    }

    /// The first bundle (in TOC order) whose name is `name`.
    pub fn bundle(&self, name: &str) -> Option<&TocBundle> {
        self.bundles.iter().find(|b| b.name == name)
    }
}
