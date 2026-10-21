//! The item cache: what each deck or wheel adds to a mod (its encoded textures, EBX, chunk records, preset, item,
//! thumbnail), kept per item between builds so a build only encodes the items that are new or changed (Trevor,
//! 2026-10-20: "adding a singular new board causing an entire library recompile" should not happen).
//!
//! An item's entry is named by a hash of everything its pieces depend on: the build's inputs for it (the art file's
//! bytes and its effects, hashed by the caller into `input`), its display name and key, the mod it goes into, the
//! game build (templates and thumbnail lighting come from the game) and the engine itself (this executable's bytes,
//! so a new Caveman never reuses an old Caveman's pieces). Entries live in `<cache>/items/<mod folder hash>/`; a build
//! removes the entries of items it no longer has.
//!
//! The pieces are recorded as operations on a mod (add an EBX, a resource, a chunk), with every byte already encoded,
//! so a cached item is replayed into a build exactly as it was first built. Its GUIDs and resource ids come from the
//! item's key (build.rs `Ids::for_item`), not from a random stream, so a mod assembled from cached items is
//! byte-identical to a clean build of the same inputs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::ebx::Guid;
use crate::image::Image;
use crate::toc;

/// Bump when the build's output for the same inputs changes in a way the engine identity would not catch (the engine
/// identity already covers any change to the executable).
pub const CACHE_VERSION: &str = "caveman-items-1";
const MAGIC: &[u8; 16] = b"CAVEMAN-ITEM\0v1\0";

/// FNV-1a 64 over length-prefixed parts: the cache's content hash (fast enough for multi-megabyte PNGs).
pub fn hash64(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for p in parts {
        eat(&(p.len() as u64).to_le_bytes());
        eat(p);
    }
    h
}

/// This executable's identity: a hash of its bytes (computed once per process). A rebuilt Caveman gets new entries.
pub fn engine_id() -> u64 {
    static ID: OnceLock<u64> = OnceLock::new();
    *ID.get_or_init(|| {
        let bytes = std::env::current_exe().ok().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        hash64(&[CACHE_VERSION.as_bytes(), env!("CARGO_PKG_VERSION").as_bytes(), &bytes])
    })
}

/// One recorded change to a mod: the pieces of an item, already encoded (Kraken) as they go into the mod's archive.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// An EBX added to `bundle`: its decoded size and encoded bytes.
    Ebx { bundle: String, name: String, size: u32, enc: Vec<u8> },
    /// A resource added to `bundle`.
    Res { bundle: String, name: String, rtype: u32, meta: [u8; 16], rid: u64, size: u32, enc: Vec<u8> },
    /// A chunk: always in the TOC; in `bundle` (whole, with its chunk-metadata record, written by ndb::write) when given.
    Chunk { bundle: Option<String>, guid: toc::Guid, enc: Vec<u8>, record: Option<Vec<u8>> },
}

/// Everything one deck or wheel adds to a mod.
#[derive(Clone, Debug)]
pub struct Product {
    pub display: String,
    pub base: String,
    /// Notes about the art (from fitting it), reported again when the item comes from the cache.
    pub notes: Vec<String>,
    pub ops: Vec<Op>,
    /// The appearance preset and its bundle, for the bundle reference table.
    pub preset: (String, String),
    /// The item's (file, instance) GUIDs, for the master item list.
    pub item_ref: (Guid, Guid),
    /// The 768 px menu thumbnail (RGBA).
    pub thumbnail: Image,
    /// A deck's 256 px render without effects: the mod's icon when it is the first deck.
    pub icon: Option<Image>,
}

impl Product {
    /// The resource ids the item adds.
    pub fn rids(&self) -> impl Iterator<Item = u64> + '_ {
        self.ops.iter().filter_map(|op| if let Op::Res { rid, .. } = op { Some(*rid) } else { None })
    }

    /// The chunk GUIDs the item adds.
    pub fn chunk_guids(&self) -> impl Iterator<Item = toc::Guid> + '_ {
        self.ops.iter().filter_map(|op| if let Op::Chunk { guid, .. } = op { Some(*guid) } else { None })
    }

    /// The entry's bytes (thumbnails as PNG).
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut w = Writer(MAGIC.to_vec());
        w.str(&self.display);
        w.str(&self.base);
        w.u32(self.notes.len() as u32);
        for n in &self.notes {
            w.str(n);
        }
        w.str(&self.preset.0);
        w.str(&self.preset.1);
        w.bytes(&self.item_ref.0 .0);
        w.bytes(&self.item_ref.1 .0);
        w.bytes(&self.thumbnail.encode_png().map_err(|e| e.to_string())?);
        match &self.icon {
            Some(icon) => {
                w.u32(1);
                w.bytes(&icon.encode_png().map_err(|e| e.to_string())?);
            }
            None => w.u32(0),
        }
        w.u32(self.ops.len() as u32);
        for op in &self.ops {
            match op {
                Op::Ebx { bundle, name, size, enc } => {
                    w.u32(0);
                    w.str(bundle);
                    w.str(name);
                    w.u32(*size);
                    w.bytes(enc);
                }
                Op::Res { bundle, name, rtype, meta, rid, size, enc } => {
                    w.u32(1);
                    w.str(bundle);
                    w.str(name);
                    w.u32(*rtype);
                    w.bytes(meta);
                    w.u64(*rid);
                    w.u32(*size);
                    w.bytes(enc);
                }
                Op::Chunk { bundle, guid, enc, record } => {
                    w.u32(2);
                    w.opt_str(bundle.as_deref());
                    w.bytes(&guid.le_bytes());
                    w.bytes(enc);
                    match record {
                        Some(r) => {
                            w.u32(1);
                            w.bytes(r);
                        }
                        None => w.u32(0),
                    }
                }
            }
        }
        Ok(w.0)
    }

    /// Reads an entry written by [`Product::encode`]; None when it is not one (or is damaged).
    pub fn decode(data: &[u8]) -> Option<Product> {
        if data.get(..16)? != MAGIC {
            return None;
        }
        let mut r = Reader { data, at: 16 };
        let display = r.str()?;
        let base = r.str()?;
        let notes = (0..r.u32()?).map(|_| r.str()).collect::<Option<Vec<_>>>()?;
        let preset = (r.str()?, r.str()?);
        let item_ref = (Guid(r.array16()?), Guid(r.array16()?));
        let thumbnail = Image::decode_png(r.bytes()?).ok()?;
        let icon = match r.u32()? {
            0 => None,
            _ => Some(Image::decode_png(r.bytes()?).ok()?),
        };
        let mut ops = Vec::new();
        for _ in 0..r.u32()? {
            ops.push(match r.u32()? {
                0 => Op::Ebx { bundle: r.str()?, name: r.str()?, size: r.u32()?, enc: r.bytes()?.to_vec() },
                1 => Op::Res { bundle: r.str()?, name: r.str()?, rtype: r.u32()?, meta: r.array16()?, rid: r.u64()?, size: r.u32()?, enc: r.bytes()?.to_vec() },
                2 => {
                    let bundle = r.opt_str()?;
                    let guid = toc::Guid::from_le_bytes(r.array16()?);
                    let enc = r.bytes()?.to_vec();
                    let record = match r.u32()? {
                        0 => None,
                        _ => Some(r.bytes()?.to_vec()),
                    };
                    Op::Chunk { bundle, guid, enc, record }
                }
                _ => return None,
            });
        }
        (r.at == data.len()).then_some(Product { display, base, notes, ops, preset, item_ref, thumbnail, icon })
    }
}

struct Writer(Vec<u8>);

impl Writer {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u64(b.len() as u64);
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }
    fn opt_str(&mut self, s: Option<&str>) {
        match s {
            Some(s) => {
                self.u32(1);
                self.str(s);
            }
            None => self.u32(0),
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let out = self.data.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(out)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(self.u64()?).ok()?;
        self.take(n)
    }
    fn array16(&mut self) -> Option<[u8; 16]> {
        let b = self.bytes()?;
        b.try_into().ok()
    }
    fn str(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }
    fn opt_str(&mut self) -> Option<Option<String>> {
        match self.u32()? {
            0 => Some(None),
            _ => Some(Some(self.str()?)),
        }
    }
}

/// The cache of one mod's items: `<cache root>/items/<hash of the mod folder>/<entry>.bin`.
pub struct ItemCache {
    dir: Option<PathBuf>,
    used: Mutex<HashSet<String>>,
}

impl ItemCache {
    /// The cache for the mod `mods_dir/mod_name` under `root`; `None` turns caching off (every item is built).
    pub fn open(root: Option<&Path>, mods_dir: &Path, mod_name: &str) -> ItemCache {
        let dir = root.map(|root| {
            let mod_dir = std::path::absolute(mods_dir.join(mod_name)).unwrap_or_else(|_| mods_dir.join(mod_name));
            let id = hash64(&[mod_dir.to_string_lossy().to_lowercase().as_bytes()]);
            root.join("items").join(format!("{id:016x}"))
        });
        ItemCache { dir, used: Mutex::new(HashSet::new()) }
    }

    pub fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// The entry name for an item: everything its pieces depend on.
    #[allow(clippy::too_many_arguments)]
    pub fn entry(kind: &str, mod_name: &str, key: &str, display: &str, input: u64, salt: u32, seed: u64, game_id: (u64, i64)) -> String {
        let h = hash64(&[
            CACHE_VERSION.as_bytes(),
            &engine_id().to_le_bytes(),
            &game_id.0.to_le_bytes(),
            &game_id.1.to_le_bytes(),
            kind.as_bytes(),
            mod_name.to_lowercase().as_bytes(),
            key.as_bytes(),
            display.as_bytes(),
            &input.to_le_bytes(),
            &salt.to_le_bytes(),
            &seed.to_le_bytes(),
        ]);
        format!("{kind}-{h:016x}")
    }

    fn path(&self, entry: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{entry}.bin")))
    }

    /// The cached item, if there is one (and it reads back whole). Marks the entry as used.
    pub fn load(&self, entry: &str) -> Option<Product> {
        let product = Product::decode(&std::fs::read(self.path(entry)?).ok()?)?;
        self.used.lock().unwrap().insert(entry.to_string());
        Some(product)
    }

    /// The notes of a cached item, without reading its pieces: whether the item is cached.
    pub fn peek_notes(&self, entry: &str) -> Option<Vec<String>> {
        let path = self.path(entry)?;
        let mut f = std::fs::File::open(path).ok()?;
        // the header (magic, display, base, notes) is small: read a bounded prefix
        let mut head = vec![0u8; 64 * 1024];
        let n = std::io::Read::read(&mut f, &mut head).ok()?;
        let mut r = Reader { data: &head[..n], at: 0 };
        if r.take(16)? != MAGIC {
            return None;
        }
        r.str()?;
        r.str()?;
        (0..r.u32()?).map(|_| r.str()).collect()
    }

    /// Keeps an item for later builds. Failure to write the cache never fails the build (returns the reason).
    pub fn store(&self, entry: &str, product: &Product) -> Result<(), String> {
        let Some(path) = self.path(entry) else { return Ok(()) };
        self.used.lock().unwrap().insert(entry.to_string());
        let data = product.encode()?;
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // write then rename: an interrupted build never leaves a half entry behind
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Removes the entries this build did not use (items no longer present, or built from older inputs). Returns how
    /// many were removed.
    pub fn prune(&self) -> usize {
        let Some(dir) = &self.dir else { return 0 };
        let used = self.used.lock().unwrap();
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        let mut removed = 0;
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".bin").or_else(|| name.strip_suffix(".tmp"));
            if let Some(stem) = stem {
                if !used.contains(stem) && std::fs::remove_file(e.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_round_trip() {
        let p = Product {
            display: "000 - Cool Deck".into(),
            base: "000_-_cool_deck".into(),
            notes: vec!["cropped to fill the deck (10 x 10 is not 1:4)".into()],
            ops: vec![
                Op::Ebx { bundle: "b".into(), name: "n".into(), size: 3, enc: vec![1, 2, 3] },
                Op::Res { bundle: "b".into(), name: "r".into(), rtype: 7, meta: [9; 16], rid: 0x1234_5679, size: 4, enc: vec![4] },
                Op::Chunk { bundle: None, guid: toc::Guid::from_le_bytes([1; 16]), enc: vec![5, 6], record: None },
                Op::Chunk { bundle: Some("b".into()), guid: toc::Guid::from_le_bytes([3; 16]), enc: vec![7], record: Some(vec![8, 9]) },
            ],
            preset: ("p".into(), "h".into()),
            item_ref: (Guid([1; 16]), Guid([2; 16])),
            thumbnail: Image::filled(4, 4, &[1, 2, 3, 255]),
            icon: Some(Image::filled(2, 2, &[9, 8, 7, 255])),
        };
        let bytes = p.encode().unwrap();
        let back = Product::decode(&bytes).unwrap();
        assert_eq!(back.ops, p.ops);
        assert_eq!((&back.display, &back.base, &back.notes, &back.preset, &back.item_ref), (&p.display, &p.base, &p.notes, &p.preset, &p.item_ref));
        assert_eq!(back.thumbnail.data, p.thumbnail.data);
        assert_eq!(back.icon.as_ref().unwrap().data, p.icon.as_ref().unwrap().data);
        let mut bad = bytes.clone();
        bad.pop();
        assert!(Product::decode(&bad).is_none());
    }

    #[test]
    fn hashes() {
        assert_ne!(hash64(&[b"ab", b"c"]), hash64(&[b"a", b"bc"]));
        assert_eq!(hash64(&[]), 0xcbf2_9ce4_8422_2325);
    }
}
