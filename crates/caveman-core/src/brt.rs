//! Character bundle-reference tables (`characters/customization/configs/cas_main_bundlereftable` and its kind,
//! resource type 0x428ec9d4): which bundle to load for each preset, looked up by the FNV-1 hash of the preset's full
//! path and of its leaf name (`brt.py`, itself ported from ReSkate's `Engine/Resource/bundle_ref_table.cpp`).
//!
//! Layout (all little-endian), as the game ships it:
//!
//! - header: 0 suffix string ptr, 8 lookups ptr, 16 path pool ptr (128), 24 bundle list ptr, 32 empty string ptr,
//!   72 lookup count, 76 bundle count, 92 version (1)
//! - pool: path descriptors {u32 parent token, inline text or u32 offset}
//! - bundles: {u32 descriptor token, u32 flags} per bundle, index 0 a sentinel
//! - lookups: {u64 hash, u32 bundle index, u32 path token}, sorted by hash
//! - trailer: 5 relocations (the header pointers), 20 bytes
//!
//! Resource meta is 16 bytes: payload size, trailer size (20), zero. A cosmetic mod registers each preset it adds
//! with a pair of rows (full path, leaf name) pointing at a bundle the base already lists; the path text goes into
//! the pool as an inline descriptor.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

/// The resource type of bundle-reference tables (brt.TYPE).
pub const TYPE: u32 = 0x428E_C9D4;
/// The largest table accepted (brt.MAXIMUM_BYTES).
pub const MAXIMUM_BYTES: usize = 8 * 1024 * 1024;
const NONE: u32 = 0xFFFF_FFFF;

/// A table that does not parse or cannot take a preset (brt.py raises ValueError).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrtError(pub String);

impl fmt::Display for BrtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BrtError {}

fn fail<T>(message: impl Into<String>) -> Result<T, BrtError> {
    Err(BrtError(message.into()))
}

/// 64-bit FNV-1 (multiply, then xor) of a name, ASCII upper case folded to lower (brt.fnv_hash).
pub fn fnv_hash(name: &str) -> u64 {
    let mut value: u64 = 0xCBF2_9CE4_8422_2325;
    // brt.py encodes as latin-1: every char is one byte (preset paths are ASCII)
    for ch in name.chars() {
        let mut c = (ch as u32 & 0xFF) as u64;
        if (65..=90).contains(&c) {
            c += 32;
        }
        value = value.wrapping_mul(0x0000_0100_0000_01B3) ^ c;
    }
    value
}

/// The leaf of a path: the text after its last `/` (brt.leaf).
pub fn leaf(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, BrtError> {
    match b.get(at..at + 4) {
        Some(s) => Ok(u32::from_le_bytes(s.try_into().unwrap())),
        None => fail("truncated bundle-reference table"),
    }
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, BrtError> {
    match b.get(at..at + 8) {
        Some(s) => Ok(u64::from_le_bytes(s.try_into().unwrap())),
        None => fail("truncated bundle-reference table"),
    }
}

/// A lookup row: (hash, bundle index, path token).
pub type Row = (u64, u32, u32);

/// A parsed table (brt.Table). `rows` are sorted by hash; `presets` maps each path to its bundle index (the first
/// row naming the path); `bundle_names` are the bundle list's names, index 0 the sentinel.
#[derive(Debug)]
pub struct Table {
    pub data: Vec<u8>,
    pub meta: [u8; 16],
    pub payload: usize,
    pub pool: usize,
    pub bundles: usize,
    pub lookups: usize,
    pub bundle_count: usize,
    pub rows: Vec<Row>,
    pub presets: HashMap<String, u32>,
    pub bundle_entries: Vec<(u32, u32)>,
    pub bundle_names: Vec<String>,
    cache: RefCell<HashMap<u32, String>>,
}

impl Table {
    /// Parses a table resource and its 16-byte meta (brt.Table).
    pub fn parse(data: &[u8], meta: &[u8]) -> Result<Table, BrtError> {
        if data.len() < 0x100 || data.len() > MAXIMUM_BYTES || meta.len() != 16 {
            return fail("not a bundle-reference table");
        }
        let payload = data.len() - 20;
        if u32_at(meta, 0)? as usize != payload || u32_at(meta, 4)? != 20 || u64_at(meta, 8)? != 0 {
            return fail("bundle-reference table metadata differs");
        }
        for i in 0..5 {
            if u32_at(data, payload + i * 4)? != (i * 8) as u32 {
                return fail("bundle-reference relocations differ");
            }
        }
        let pointer = |at: usize| -> Result<usize, BrtError> {
            let value = u64_at(data, at)?;
            if value >= payload as u64 {
                return fail("bundle-reference pointer out of range");
            }
            Ok(value as usize)
        };
        let (pool, bundles, lookups) = (pointer(16)?, pointer(24)?, pointer(8)?);
        if pool != 128 || bundles <= pool || lookups <= bundles || bundles & 15 != 0 || lookups & 15 != 0 || u32_at(data, 92)? != 1 {
            return fail("bundle-reference table layout differs");
        }
        let count = u32_at(data, 72)? as usize;
        let bundle_count = u32_at(data, 76)? as usize;
        if count > 200_000 || !(2..=100_000).contains(&bundle_count) || bundles + bundle_count * 8 != lookups || lookups + count * 16 != payload {
            return fail("bundle-reference table counts differ");
        }
        let mut table = Table {
            data: data.to_vec(),
            meta: meta.try_into().unwrap(),
            payload,
            pool,
            bundles,
            lookups,
            bundle_count,
            rows: Vec::with_capacity(count),
            presets: HashMap::new(),
            bundle_entries: Vec::new(),
            bundle_names: Vec::new(),
            cache: RefCell::new(HashMap::new()),
        };
        for i in 0..count {
            let at = lookups + i * 16;
            let row = (u64_at(data, at)?, u32_at(data, at + 8)?, u32_at(data, at + 12)?);
            if table.rows.last().is_some_and(|r| r.0 >= row.0) || row.1 as usize >= bundle_count {
                return fail("bundle-reference lookups are not sorted or name no bundle");
            }
            table.rows.push(row);
            let path = table.text(row.2)?;
            if path.is_empty() {
                return fail("bundle-reference lookup has no path");
            }
            table.presets.entry(path).or_insert(row.1);
        }
        for i in 0..bundle_count {
            table.bundle_entries.push((u32_at(data, bundles + 8 * i)?, u32_at(data, bundles + 8 * i + 4)?));
        }
        let mut names = Vec::with_capacity(bundle_count);
        for &(tok, _) in &table.bundle_entries {
            names.push(if tok != NONE && tok >> 24 != 0 { table.text(tok)? } else { String::new() });
        }
        table.bundle_names = names;
        Ok(table)
    }

    /// The path a pool token names: its parent's text, then its own fragment (brt.Table.text).
    pub fn text(&self, token: u32) -> Result<String, BrtError> {
        self.text_at(token, 0)
    }

    fn text_at(&self, token: u32, depth: usize) -> Result<String, BrtError> {
        if token == NONE {
            return Ok(String::new());
        }
        if depth > 128 {
            return fail("bundle-reference path is cyclic");
        }
        if let Some(t) = self.cache.borrow().get(&token) {
            return Ok(t.clone());
        }
        let at = self.pool + (token & 0x7F_FFFF) as usize;
        let length = (token >> 24) as usize;
        if length == 0 || at > self.bundles || self.bundles - at < 8 {
            return fail("bad bundle-reference path");
        }
        let fragment = if token & 0x80_0000 != 0 { at + 4 } else { self.pool + u32_at(&self.data, at + 4)? as usize };
        if fragment < self.pool || fragment > self.bundles || self.bundles - fragment < length {
            return fail("bundle-reference path outside its pool");
        }
        let mut text = self.text_at(u32_at(&self.data, at)?, depth + 1)?;
        let bytes = &self.data[fragment..fragment + length];
        if bytes.iter().any(|&c| !(32..=126).contains(&c)) {
            return fail("bundle-reference path is not text");
        }
        text.push_str(std::str::from_utf8(bytes).unwrap());
        if text.chars().any(|c| !(' '..='~').contains(&c)) {
            return fail("bundle-reference path is not text");
        }
        self.cache.borrow_mut().insert(token, text.clone());
        Ok(text)
    }

    /// The index of a bundle in the list, by its list name (the stock preset path) or by its items.toc name
    /// (`win32/<that>_cas_main_bundlereftable`); case-insensitive (brt.Table.bundle_index).
    pub fn bundle_index(&self, name: &str) -> Option<u32> {
        let mut name = name.to_lowercase();
        if name.starts_with("win32/") && name.ends_with("_cas_main_bundlereftable") {
            name = name["win32/".len()..name.len() - "_cas_main_bundlereftable".len()].to_string();
        }
        self.bundle_names.iter().position(|n| n.to_lowercase() == name).map(|i| i as u32)
    }
}

/// A copy of `table` with each (preset path, bundle name) added, as a cosmetic mod ships it (brt.add_presets).
/// Returns (resource bytes, meta). Fails when a path is already a lookup or a bundle is not listed.
pub fn add_presets(table: &Table, presets: &[(String, String)]) -> Result<(Vec<u8>, [u8; 16]), BrtError> {
    let mut builder = Builder::new(table)?;
    for (path, bundle) in presets {
        let index = table.bundle_index(bundle).ok_or_else(|| BrtError(format!("{bundle} is not in the bundle list")))?;
        builder.insert(path, index)?;
    }
    Ok(builder.write())
}

/// A table being added to, as ReSkate's merge (and a cosmetic mod) adds presets (brt.Builder): each new path is
/// appended to the pool as an inline descriptor, with a full-path row and, unless another preset already answers to
/// it, a leaf-name row, both pointing at an existing bundle.
pub struct Builder<'a> {
    table: &'a Table,
    rows: Vec<Row>,
    hashes: HashSet<u64>,
    leaves: HashMap<u64, String>,
    pool: Vec<u8>,
    size: usize,
    changed: bool,
}

impl<'a> Builder<'a> {
    pub fn new(table: &'a Table) -> Result<Builder<'a>, BrtError> {
        let rows = table.rows.clone();
        let hashes = rows.iter().map(|r| r.0).collect();
        let mut leaves = HashMap::new();
        let mut paths: Vec<&String> = table.presets.keys().collect();
        paths.sort(); // in path order: each leaf keeps the first preset
        for path in paths {
            leaves.entry(fnv_hash(leaf(path))).or_insert_with(|| path.clone());
        }
        Ok(Builder { table, rows, hashes, leaves, pool: Vec::new(), size: table.data.len(), changed: false })
    }

    /// Adds `path` in bundle index `bundle` (brt.Builder.insert). Returns the preset that keeps the leaf name when
    /// this one only got its full-path row, else an empty string.
    pub fn insert(&mut self, path: &str, bundle: u32) -> Result<String, BrtError> {
        if self.size > MAXIMUM_BYTES {
            return fail("not a bundle-reference table");
        }
        if self.rows.len() > 200_000 {
            return fail("bundle-reference table counts differ");
        }
        let (full, short) = (fnv_hash(path), fnv_hash(leaf(path)));
        if self.hashes.contains(&full) {
            return fail(format!("{path} collides with an existing lookup"));
        }
        let with_leaf = full != short && !self.hashes.contains(&short);
        let mut holder = String::new();
        if full != short && !with_leaf {
            holder = self.leaves.get(&short).cloned().unwrap_or_else(|| "another lookup".to_string());
        }
        if !path.is_ascii() {
            return fail(format!("{path} is not ASCII"));
        }
        let raw = path.as_bytes();
        if raw.len() > 255 {
            return fail(format!("{path} is too long for a lookup"));
        }
        let growth = (4 + raw.len() + 15) & !15;
        let offset = self.table.bundles + self.pool.len() - self.table.pool;
        if offset > 0x7F_FFFF {
            return fail("bundle-reference path pool is full");
        }
        let token = offset as u32 | 0x80_0000 | ((raw.len() as u32) << 24);
        let at = self.pool.len();
        self.pool.resize(at + growth, 0);
        self.pool[at..at + 4].copy_from_slice(&NONE.to_le_bytes());
        self.pool[at + 4..at + 4 + raw.len()].copy_from_slice(raw);
        self.rows.push((full, bundle, token));
        self.hashes.insert(full);
        if with_leaf {
            self.rows.push((short, bundle, token));
            self.hashes.insert(short);
        }
        let replace = match self.leaves.get(&short) {
            None => true,
            Some(held) => path < held.as_str(),
        };
        if replace {
            self.leaves.insert(short, path.to_string());
        }
        self.size += growth + if with_leaf { 32 } else { 16 };
        self.changed = true;
        Ok(holder)
    }

    /// (resource bytes, resource meta): the base's own bytes when nothing was added (brt.Builder.write).
    pub fn write(&self) -> (Vec<u8>, [u8; 16]) {
        let t = self.table;
        if !self.changed {
            return (t.data.clone(), t.meta);
        }
        let bundles = t.bundles + self.pool.len();
        let lookups = t.lookups + self.pool.len();
        let payload = lookups + self.rows.len() * 16;
        let mut out = vec![0u8; payload + 20];
        out[..t.bundles].copy_from_slice(&t.data[..t.bundles]);
        out[t.bundles..bundles].copy_from_slice(&self.pool);
        out[bundles..lookups].copy_from_slice(&t.data[t.bundles..t.lookups]);
        let mut rows = self.rows.clone();
        rows.sort_by_key(|r| r.0); // stable, as Python's sorted
        for (i, (h, b, tok)) in rows.iter().enumerate() {
            let at = lookups + i * 16;
            out[at..at + 8].copy_from_slice(&h.to_le_bytes());
            out[at + 8..at + 12].copy_from_slice(&b.to_le_bytes());
            out[at + 12..at + 16].copy_from_slice(&tok.to_le_bytes());
        }
        out[payload..].copy_from_slice(&t.data[t.payload..]);
        out[8..16].copy_from_slice(&(lookups as u64).to_le_bytes());
        out[24..32].copy_from_slice(&(bundles as u64).to_le_bytes());
        out[72..76].copy_from_slice(&(rows.len() as u32).to_le_bytes());
        let mut meta = t.meta;
        meta[0..4].copy_from_slice(&(payload as u32).to_le_bytes());
        (out, meta)
    }
}

/// Presets a copy adds over the base, in the order their descriptors sit in the copy's pool (the order they were
/// inserted) (brt.added_presets). Fails when the copy moves or drops a base preset or changes the bundle list, as
/// ReSkate's merge would.
pub fn added_presets(base: &Table, copy: &Table) -> Result<Vec<(String, u32)>, BrtError> {
    if copy.data[copy.bundles..copy.lookups] != base.data[base.bundles..base.lookups] {
        return fail("the copy changes the bundle list");
    }
    for (path, b) in &base.presets {
        if copy.presets.get(path) != Some(b) {
            return fail(format!("the copy moves or drops {path}"));
        }
    }
    let mut tokens: HashMap<String, (u32, u32)> = HashMap::new();
    for &(_, b, tok) in &copy.rows {
        let path = copy.text(tok)?;
        if !base.presets.contains_key(&path) {
            tokens.entry(path).or_insert((tok & 0x7F_FFFF, b));
        }
    }
    let mut out: Vec<(String, (u32, u32))> = tokens.into_iter().collect();
    // sorted by pool offset; ties (none in practice) by path to stay deterministic
    out.sort_by(|a, b| a.1 .0.cmp(&b.1 .0).then_with(|| a.0.cmp(&b.0)));
    Ok(out.into_iter().map(|(p, (_, b))| (p, b)).collect())
}

/// The result of [`merge`]: the merged resource and meta, how many presets were added, conflicts (a path two copies
/// put in different bundles) and presets that only got their full-path row: (copy index, path, holder).
pub struct Merged {
    pub data: Vec<u8>,
    pub meta: [u8; 16],
    pub added: usize,
    pub conflicts: usize,
    pub shadowed: Vec<(usize, String, String)>,
}

/// ReSkate's merge: every preset any copy adds, the first copy winning a path (brt.merge).
pub fn merge(base: &Table, copies: &[Table]) -> Result<Merged, BrtError> {
    let mut builder = Builder::new(base)?;
    let mut added: BTreeMap<String, u32> = BTreeMap::new();
    let (mut conflicts, mut shadowed) = (0, Vec::new());
    for (index, copy) in copies.iter().enumerate() {
        let mut presets = added_presets(base, copy)?;
        presets.sort();
        for (path, b) in presets {
            if let Some(&prev) = added.get(&path) {
                conflicts += (prev != b) as usize;
                continue;
            }
            let holder = builder.insert(&path, b)?;
            if !holder.is_empty() {
                shadowed.push((index, path.clone(), holder));
            }
            added.insert(path, b);
        }
    }
    let (data, meta) = builder.write();
    Ok(Merged { data, meta, added: added.len(), conflicts, shadowed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_folds_ascii_case() {
        assert_eq!(fnv_hash("Abc"), fnv_hash("abc"));
        assert_eq!(leaf("a/b/c_ap"), "c_ap");
        assert_eq!(leaf("plain"), "plain");
    }
}
