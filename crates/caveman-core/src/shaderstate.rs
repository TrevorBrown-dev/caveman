//! Material keys for effects with Caveman's own textures (knowledge/effects-catalog.md, "Our own textures"): the
//! three tables a material key goes through, read and extended the way ReSkate Studio extends them for maps and
//! custom meshes (the FPV Drone mod's level edit is the worked example):
//!
//! - the **program lookup** (`*_win32_shaderstate_db`, type `0xE2E6955B`): key -> shader records. A new key is an
//!   alias: a row naming records the database already has (ReSkate's merge combines such rows from several mods);
//! - the **texture-set lookup** (`*_streamabletexturesetlookuptable`, type `0x2D254A89`): rows {key, texture-set id},
//!   sorted, one per key; a new key aliases an existing set;
//! - a **ShaderBlockDepot** (type `0x73312045`): key -> a parameter block (the material's values, textures among them
//!   as a type byte and the texture asset's instance and file GUIDs). A new key gets a depot resource of its own with
//!   a copy of the source key's block, its texture GUIDs swapped for ours.
//!
//! Formats in knowledge/shaders.md; the depot layout below was checked against the shared bundle's depot (2,822
//! keys) and the FPV Drone mod's (5 keys): header {u64 table offset 16, u64 0}, the table {i64 key, u64 block
//! offset}, the parameter lists, the blocks {u64 list offset, u64 hash, u64 list size} (8-aligned), zero padding to 16,
//! then the relocations (u32 offsets of every pointer field: per entry its block's list field when the block is new,
//! then the entry's block field; the table pointer at 0 last). Meta: data size, relocation size, `03 00 19 01`, count.

use std::fmt;

/// `0xE2E6955B`: an ExpressionShaderDatabase (its lookup is the program lookup).
pub const PROGRAM_LOOKUP_TYPE: u32 = 0xE2E6_955B;
/// `0x2D254A89`: the streamable texture-set lookup.
pub const TEXTURE_LOOKUP_TYPE: u32 = 0x2D25_4A89;
/// `0x73312045`: a ShaderBlockDepot.
pub const DEPOT_TYPE: u32 = 0x7331_2045;
/// A depot's meta word 2 (every depot the game and Studio write).
pub const DEPOT_FLAGS: u32 = 0x0119_0003;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateError(pub String);

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StateError {}

type Result<T> = std::result::Result<T, StateError>;

fn bad<T>(m: impl Into<String>) -> Result<T> {
    Err(StateError(m.into()))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| StateError(format!("short read at {at}")))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64> {
    b.get(at..at + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| StateError(format!("short read at {at}")))
}

// ---------------------------------------------------------------- the program lookup

/// The program lookup of a shader database: (where the lookup starts, its rows {key (as u64), record indices}).
pub fn program_rows(data: &[u8], meta: &[u8; 16]) -> Result<(usize, Vec<(u64, Vec<u32>)>)> {
    let (prog, reloc, look) = (u32_at(meta, 0)? as usize, u32_at(meta, 4)? as usize, u32_at(meta, 8)? as usize);
    let start = prog + reloc;
    if start + look != data.len() {
        return bad(format!("the database is {} bytes, its meta says {}", data.len(), start + look));
    }
    let n = u32_at(data, start)? as usize;
    let mut at = start + 4;
    let mut rows = Vec::with_capacity(n);
    for _ in 0..n {
        let key = u64_at(data, at)?;
        let c = u32_at(data, at + 8)? as usize;
        let mut recs = Vec::with_capacity(c);
        for k in 0..c {
            recs.push(u32_at(data, at + 12 + 4 * k)?);
        }
        rows.push((key, recs));
        at += 12 + 4 * c;
    }
    if at != data.len() {
        return bad("the program lookup does not end the database");
    }
    Ok((start, rows))
}

/// The record count of a shader database (its program blob's first u32).
pub fn record_count(data: &[u8]) -> Result<u32> {
    u32_at(data, 0)
}

/// The record indices a key has in the program lookup, or None.
pub fn program_records(data: &[u8], meta: &[u8; 16], key: u64) -> Result<Option<Vec<u32>>> {
    Ok(program_rows(data, meta)?.1.into_iter().find(|(k, _)| *k == key).map(|(_, r)| r))
}

/// The database with alias rows added (each a new key naming records it already has), the lookup kept sorted by key;
/// returns the new data and meta. Refuses keys it already has and records past its end.
pub fn add_program_rows(data: &[u8], meta: &[u8; 16], add: &[(u64, Vec<u32>)]) -> Result<(Vec<u8>, [u8; 16])> {
    let (start, mut rows) = program_rows(data, meta)?;
    let records = record_count(data)?;
    for (key, recs) in add {
        if rows.iter().any(|(k, _)| k == key) {
            return bad(format!("the database already has key {key:#x}"));
        }
        if recs.is_empty() || recs.iter().any(|&r| r >= records) {
            return bad(format!("key {key:#x}: records {recs:?} are not all in the database ({records})"));
        }
        rows.push((*key, recs.clone()));
    }
    rows.sort_by_key(|(k, _)| *k);
    let mut out = data[..start].to_vec();
    out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    for (k, recs) in &rows {
        out.extend_from_slice(&k.to_le_bytes());
        out.extend_from_slice(&(recs.len() as u32).to_le_bytes());
        for r in recs {
            out.extend_from_slice(&r.to_le_bytes());
        }
    }
    let mut m = *meta;
    m[8..12].copy_from_slice(&((out.len() - start) as u32).to_le_bytes());
    Ok((out, m))
}

// ---------------------------------------------------------------- the texture-set lookup

/// The texture-set lookup's rows {key, set id}.
pub fn texture_rows(data: &[u8]) -> Result<Vec<(u64, u64)>> {
    if data.len() % 16 != 0 {
        return bad("the texture-set lookup is not whole rows");
    }
    (0..data.len() / 16).map(|i| Ok((u64_at(data, 16 * i)?, u64_at(data, 16 * i + 8)?))).collect()
}

/// The texture-set lookup with rows added (sorted by key, as the game's). Refuses keys it already has.
pub fn add_texture_rows(data: &[u8], add: &[(u64, u64)]) -> Result<Vec<u8>> {
    let mut rows = texture_rows(data)?;
    for &(key, set) in add {
        if rows.iter().any(|(k, _)| *k == key) {
            return bad(format!("the texture-set lookup already has key {key:#x}"));
        }
        rows.push((key, set));
    }
    rows.sort_by_key(|(k, _)| *k);
    Ok(rows.iter().flat_map(|(k, s)| k.to_le_bytes().into_iter().chain(s.to_le_bytes())).collect())
}

// ---------------------------------------------------------------- depots

/// A depot's entries: (key, the parameter list's bytes, the block hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Depot {
    pub entries: Vec<(i64, Vec<u8>, u64)>,
}

/// Reads a depot resource (several keys may share a block: each entry gets its own copy of the list here).
pub fn read_depot(data: &[u8], meta: &[u8; 16]) -> Result<Depot> {
    let (size, rsize, flags, count) = (u32_at(meta, 0)? as usize, u32_at(meta, 4)? as usize, u32_at(meta, 8)?, u32_at(meta, 12)? as usize);
    if flags != DEPOT_FLAGS || size + rsize != data.len() {
        return bad(format!("not a depot this reader knows (flags {flags:#x}, {size} + {rsize} of {} bytes)", data.len()));
    }
    let table = u64_at(data, 0)? as usize;
    let mut entries = Vec::with_capacity(count);
    for k in 0..count {
        let key = u64_at(data, table + 16 * k)? as i64;
        let block = u64_at(data, table + 16 * k + 8)? as usize;
        let (list, hash, len) = (u64_at(data, block)? as usize, u64_at(data, block + 8)?, u64_at(data, block + 16)? as usize);
        let bytes = data.get(list..list + len).ok_or_else(|| StateError(format!("key {key}: its list runs past the depot")))?.to_vec();
        entries.push((key, bytes, hash));
    }
    Ok(Depot { entries })
}

/// Writes a depot whose entries each have a block of their own (the layout of a new depot such as Studio's, above):
/// (data, meta).
pub fn write_depot(depot: &Depot) -> (Vec<u8>, [u8; 16]) {
    let n = depot.entries.len();
    let table = 16usize;
    let mut lists = Vec::with_capacity(n);
    let mut at = table + 16 * n;
    for (_, list, _) in &depot.entries {
        lists.push(at);
        at += list.len();
    }
    let blocks_at = at.div_ceil(8) * 8;
    let end = blocks_at + 24 * n;
    let size = end.div_ceil(16) * 16;
    let mut out = vec![0u8; size];
    out[0..8].copy_from_slice(&(table as u64).to_le_bytes());
    let mut relocs: Vec<u32> = Vec::with_capacity(2 * n + 1);
    for (k, (key, list, hash)) in depot.entries.iter().enumerate() {
        let e = table + 16 * k;
        let b = blocks_at + 24 * k;
        out[e..e + 8].copy_from_slice(&key.to_le_bytes());
        out[e + 8..e + 16].copy_from_slice(&(b as u64).to_le_bytes());
        out[lists[k]..lists[k] + list.len()].copy_from_slice(list);
        out[b..b + 8].copy_from_slice(&(lists[k] as u64).to_le_bytes());
        out[b + 8..b + 16].copy_from_slice(&hash.to_le_bytes());
        out[b + 16..b + 24].copy_from_slice(&(list.len() as u64).to_le_bytes());
        relocs.push(b as u32);
        relocs.push((e + 8) as u32);
    }
    relocs.push(0);
    for r in &relocs {
        out.extend_from_slice(&r.to_le_bytes());
    }
    let mut meta = [0u8; 16];
    meta[0..4].copy_from_slice(&(size as u32).to_le_bytes());
    meta[4..8].copy_from_slice(&((relocs.len() * 4) as u32).to_le_bytes());
    meta[8..12].copy_from_slice(&DEPOT_FLAGS.to_le_bytes());
    meta[12..16].copy_from_slice(&(n as u32).to_le_bytes());
    (out, meta)
}

/// A parameter list with every 16-byte occurrence of `from` replaced by `to` (texture values hold the texture
/// asset's instance and file GUIDs as stored, little-endian); returns the list and how many were replaced.
pub fn swap_guids(list: &[u8], swaps: &[([u8; 16], [u8; 16])]) -> (Vec<u8>, usize) {
    let mut out = list.to_vec();
    let mut n = 0;
    let mut i = 0;
    while i + 16 <= out.len() {
        if let Some((_, to)) = swaps.iter().find(|(from, _)| out[i..i + 16] == from[..]) {
            out[i..i + 16].copy_from_slice(to);
            n += 1;
            i += 16;
        } else {
            i += 1;
        }
    }
    (out, n)
}

/// 64-bit FNV-1a: the hash Caveman gives a new block (the game's own block hash function is not known; a block's
/// hash only has to differ from every other block's, see knowledge/effects-catalog.md).
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_db(rows: &[(u64, Vec<u32>)], records: u32) -> (Vec<u8>, [u8; 16]) {
        let mut data = records.to_le_bytes().to_vec();
        data.resize(32, 0); // a stand-in program blob
        data.extend_from_slice(&[0u8; 8]); // and relocations
        let start = data.len();
        data.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        for (k, r) in rows {
            data.extend_from_slice(&k.to_le_bytes());
            data.extend_from_slice(&(r.len() as u32).to_le_bytes());
            for x in r {
                data.extend_from_slice(&x.to_le_bytes());
            }
        }
        let mut meta = [0u8; 16];
        meta[0..4].copy_from_slice(&32u32.to_le_bytes());
        meta[4..8].copy_from_slice(&8u32.to_le_bytes());
        meta[8..12].copy_from_slice(&((data.len() - start) as u32).to_le_bytes());
        (data, meta)
    }

    #[test]
    fn program_rows_alias_and_stay_sorted() {
        let (db, meta) = fake_db(&[(5, vec![1]), (u64::MAX - 1, vec![0, 2])], 3);
        let (out, m) = add_program_rows(&db, &meta, &[(7, vec![0, 2])]).unwrap();
        let (_, rows) = program_rows(&out, &m).unwrap();
        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), vec![5, 7, u64::MAX - 1]);
        assert_eq!(program_records(&out, &m, 7).unwrap(), Some(vec![0, 2]));
        assert!(add_program_rows(&db, &meta, &[(5, vec![0])]).is_err(), "a key it has");
        assert!(add_program_rows(&db, &meta, &[(9, vec![3])]).is_err(), "a record past the end");
        // adding nothing writes the same bytes
        assert_eq!(add_program_rows(&db, &meta, &[]).unwrap(), (db.clone(), meta));
    }

    #[test]
    fn texture_rows_sorted() {
        let data: Vec<u8> = [(1u64, 10u64), (9, 90)].iter().flat_map(|(k, s)| k.to_le_bytes().into_iter().chain(s.to_le_bytes())).collect();
        let out = add_texture_rows(&data, &[(4, 40)]).unwrap();
        assert_eq!(texture_rows(&out).unwrap(), vec![(1, 10), (4, 40), (9, 90)]);
        assert!(add_texture_rows(&data, &[(9, 1)]).is_err());
    }

    #[test]
    fn depot_round_trip() {
        let depot = Depot { entries: vec![(-5, vec![1, 2, 3], 0xAB), (77, vec![9; 13], 0xCD)] };
        let (data, meta) = write_depot(&depot);
        assert_eq!(read_depot(&data, &meta).unwrap(), depot);
        assert_eq!(data.len() % 4, 0);
        let (list, n) = swap_guids(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 99], &[([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16], [0xEE; 16])]);
        assert_eq!((list[0], list[1], list[16], list[17], n), (0, 0xEE, 0xEE, 99, 1));
    }
}
