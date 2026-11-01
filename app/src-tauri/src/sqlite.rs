//! A reader for the few SQLite tables Caveman needs out of ReSkate's profile, in Rust alone (the GNU toolchain this
//! builds with has no C compiler for SQLite itself): the database file with its write-ahead log laid over it, the
//! table b-trees walked, the records decoded. Read only, whole-file reads, nothing locked: a torn read while the game
//! writes fails a check and the caller keeps its last good answer. The file format is SQLite's documented one
//! (https://www.sqlite.org/fileformat2.html): the 100-byte header, b-tree pages of cells, varints, record serial
//! types, overflow chains, and the WAL's frames with their salts and running checksums.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

/// A database's pages as they stand (the write-ahead log applied), 1-based.
pub struct Db {
    pages: Vec<Vec<u8>>,
    usable: usize,
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// A SQLite varint: up to nine bytes, seven bits each, the ninth whole. (value, bytes used).
fn varint(b: &[u8], at: usize) -> Option<(i64, usize)> {
    let mut v: u64 = 0;
    for i in 0..9 {
        let byte = *b.get(at + i)?;
        if i == 8 {
            v = (v << 8) | byte as u64;
            return Some((v as i64, 9));
        }
        v = (v << 7) | (byte & 0x7f) as u64;
        if byte & 0x80 == 0 {
            return Some((v as i64, i + 1));
        }
    }
    None
}

/// The WAL's running checksum over 8-byte steps (two 32-bit words, native order by the magic).
fn wal_checksum(data: &[u8], big_endian: bool, mut s: (u32, u32)) -> (u32, u32) {
    for chunk in data.as_chunks::<8>().0 {
        let (x0, x1) = if big_endian {
            (u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]), u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]))
        } else {
            (u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]), u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]))
        };
        s.0 = s.0.wrapping_add(x0).wrapping_add(s.1);
        s.1 = s.1.wrapping_add(x1).wrapping_add(s.0);
    }
    s
}

impl Db {
    /// Reads the database and, when there is one beside it, its write-ahead log (`<file>-wal`).
    pub fn open(path: &Path) -> Result<Db, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if bytes.len() < 100 || &bytes[..16] != b"SQLite format 3\0" {
            return Err(format!("{}: not a SQLite database", path.display()));
        }
        let page_size = match be16(&bytes, 16).unwrap() {
            1 => 65536,
            n => n as usize,
        };
        if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(format!("{}: page size {page_size}", path.display()));
        }
        let reserved = bytes[20] as usize;
        let mut pages: Vec<Vec<u8>> = bytes.chunks(page_size).map(|c| c.to_vec()).collect();
        if pages.last().is_some_and(|p| p.len() < page_size) {
            pages.pop();
        }
        // the write-ahead log: frames with the header's salts and an unbroken checksum chain, up to the last commit
        let wal_path = PathBuf::from(format!("{}-wal", path.display()));
        if let Ok(wal) = std::fs::read(&wal_path) {
            if wal.len() >= 32 {
                let magic = be32(&wal, 0).unwrap();
                let big_endian = magic == 0x377f_0683;
                if (magic == 0x377f_0682 || big_endian) && be32(&wal, 8).unwrap() as usize == page_size {
                    let salt1 = be32(&wal, 16).unwrap();
                    let salt2 = be32(&wal, 20).unwrap();
                    let mut sum = wal_checksum(&wal[..24], big_endian, (0, 0));
                    let header_ok = sum.0 == be32(&wal, 24).unwrap() && sum.1 == be32(&wal, 28).unwrap();
                    if header_ok {
                        let frame = 24 + page_size;
                        let mut pending: Vec<(u32, Vec<u8>)> = Vec::new();
                        let mut committed: HashMap<u32, Vec<u8>> = HashMap::new();
                        let mut size_after: Option<u32> = None;
                        let mut at = 32;
                        while at + frame <= wal.len() {
                            let h = &wal[at..at + 24];
                            if be32(h, 8).unwrap() != salt1 || be32(h, 12).unwrap() != salt2 {
                                break;
                            }
                            sum = wal_checksum(&h[..8], big_endian, sum);
                            sum = wal_checksum(&wal[at + 24..at + frame], big_endian, sum);
                            if sum.0 != be32(h, 16).unwrap() || sum.1 != be32(h, 20).unwrap() {
                                break;
                            }
                            let pgno = be32(h, 0).unwrap();
                            let commit = be32(h, 4).unwrap();
                            pending.push((pgno, wal[at + 24..at + frame].to_vec()));
                            if commit != 0 {
                                for (p, data) in pending.drain(..) {
                                    committed.insert(p, data);
                                }
                                size_after = Some(commit);
                            }
                            at += frame;
                        }
                        if let Some(n) = size_after {
                            pages.resize(n as usize, vec![0; page_size]);
                            for (p, data) in committed {
                                if p >= 1 && (p as usize) <= pages.len() {
                                    pages[p as usize - 1] = data;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(Db { pages, usable: page_size - reserved })
    }

    fn page(&self, n: u32) -> Result<&[u8], String> {
        self.pages.get(n as usize - 1).map(|p| p.as_slice()).ok_or_else(|| format!("page {n} is past the end ({} pages)", self.pages.len()))
    }

    /// The root page of a table, from sqlite_master.
    pub fn table_root(&self, name: &str) -> Result<u32, String> {
        for row in self.rows(1)? {
            if row.first().and_then(|v| v.as_text()) == Some("table") && row.get(1).and_then(|v| v.as_text()).is_some_and(|n| n.eq_ignore_ascii_case(name)) {
                return row.get(3).and_then(|v| v.as_int()).map(|r| r as u32).ok_or_else(|| format!("table {name}: no root page"));
            }
        }
        Err(format!("no table {name}"))
    }

    /// Every row of the table rooted at `root`, in rowid order; a row is its record's values.
    pub fn rows(&self, root: u32) -> Result<Vec<Vec<Value>>, String> {
        let mut out = Vec::new();
        self.walk(root, &mut out, 0)?;
        Ok(out)
    }

    fn walk(&self, pgno: u32, out: &mut Vec<Vec<Value>>, depth: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("the b-tree is too deep".into());
        }
        let page = self.page(pgno)?;
        let base = if pgno == 1 { 100 } else { 0 };
        let kind = *page.get(base).ok_or("an empty page")?;
        let cells = be16(page, base + 3).ok_or("a short page header")? as usize;
        let u = self.usable;
        // the most of a payload kept on the page: table leaves keep up to U - 35, index pages less
        let (table_x, index_x) = (u - 35, (u - 12) * 64 / 255 - 23);
        match kind {
            // a table leaf: rowid cells
            0x0d => {
                for c in 0..cells {
                    let at = be16(page, base + 8 + c * 2).ok_or("a short cell pointer array")? as usize;
                    let (payload_len, n1) = varint(page, at).ok_or("a bad cell")?;
                    let (_rowid, n2) = varint(page, at + n1).ok_or("a bad cell")?;
                    let payload = self.payload(page, at + n1 + n2, payload_len as usize, table_x)?;
                    out.push(record(&payload)?);
                }
            }
            // a table interior: child pointers only
            0x05 => {
                for c in 0..cells {
                    let at = be16(page, base + 12 + c * 2).ok_or("a short cell pointer array")? as usize;
                    let child = be32(page, at).ok_or("a bad interior cell")?;
                    self.walk(child, out, depth + 1)?;
                }
                let right = be32(page, base + 8).ok_or("a short interior header")?;
                self.walk(right, out, depth + 1)?;
            }
            // an index leaf (a WITHOUT ROWID table's rows are index entries: the whole row is the key)
            0x0a => {
                for c in 0..cells {
                    let at = be16(page, base + 8 + c * 2).ok_or("a short cell pointer array")? as usize;
                    let (payload_len, n1) = varint(page, at).ok_or("a bad index cell")?;
                    let payload = self.payload(page, at + n1, payload_len as usize, index_x)?;
                    out.push(record(&payload)?);
                }
            }
            // an index interior: each cell is an entry too, between its left child's and the next's
            0x02 => {
                for c in 0..cells {
                    let at = be16(page, base + 12 + c * 2).ok_or("a short cell pointer array")? as usize;
                    let child = be32(page, at).ok_or("a bad index interior cell")?;
                    self.walk(child, out, depth + 1)?;
                    let (payload_len, n1) = varint(page, at + 4).ok_or("a bad index interior cell")?;
                    let payload = self.payload(page, at + 4 + n1, payload_len as usize, index_x)?;
                    out.push(record(&payload)?);
                }
                let right = be32(page, base + 8).ok_or("a short interior header")?;
                self.walk(right, out, depth + 1)?;
            }
            other => return Err(format!("page {pgno} is of kind {other:#x}, not a b-tree page")),
        }
        Ok(())
    }

    /// A cell's payload, the overflow chain followed; `x` is the most a cell of this page's kind keeps locally.
    fn payload(&self, page: &[u8], at: usize, len: usize, x: usize) -> Result<Vec<u8>, String> {
        let u = self.usable;
        let local = if len <= x {
            len
        } else {
            let m = ((u - 12) * 32 / 255) - 23;
            let k = m + ((len - m) % (u - 4));
            if k <= x {
                k
            } else {
                m
            }
        };
        let mut out = page.get(at..at + local).ok_or("a payload past its page")?.to_vec();
        if local < len {
            let mut next = be32(page, at + local).ok_or("a missing overflow pointer")?;
            while out.len() < len && next != 0 {
                let p = self.page(next)?;
                let take = (len - out.len()).min(u - 4);
                out.extend_from_slice(p.get(4..4 + take).ok_or("a short overflow page")?);
                next = be32(p, 0).unwrap_or(0);
            }
            if out.len() < len {
                return Err("an overflow chain ended early".into());
            }
        }
        Ok(out)
    }
}

/// A record's values from its payload: the header's serial types, then the bodies.
fn record(payload: &[u8]) -> Result<Vec<Value>, String> {
    let (header_len, n) = varint(payload, 0).ok_or("a bad record header")?;
    let header_len = header_len as usize;
    let mut types = Vec::new();
    let mut at = n;
    while at < header_len {
        let (t, k) = varint(payload, at).ok_or("a bad serial type")?;
        types.push(t);
        at += k;
    }
    let mut out = Vec::with_capacity(types.len());
    let mut body = header_len;
    for t in types {
        let (value, size) = match t {
            0 => (Value::Null, 0),
            1..=6 => {
                let size = match t {
                    1 => 1,
                    2 => 2,
                    3 => 3,
                    4 => 4,
                    5 => 6,
                    _ => 8,
                };
                let b = payload.get(body..body + size).ok_or("a short integer")?;
                let mut v: i64 = if b[0] & 0x80 != 0 { -1 } else { 0 };
                for &byte in b {
                    v = (v << 8) | byte as i64;
                }
                (Value::Int(v), size)
            }
            7 => {
                let b = payload.get(body..body + 8).ok_or("a short real")?;
                (Value::Real(f64::from_be_bytes(b.try_into().unwrap())), 8)
            }
            8 => (Value::Int(0), 0),
            9 => (Value::Int(1), 0),
            t if t >= 12 && t % 2 == 0 => {
                let size = (t as usize - 12) / 2;
                (Value::Blob(payload.get(body..body + size).ok_or("a short blob")?.to_vec()), size)
            }
            t if t >= 13 => {
                let size = (t as usize - 13) / 2;
                (Value::Text(String::from_utf8_lossy(payload.get(body..body + size).ok_or("a short text")?).into_owned()), size)
            }
            other => return Err(format!("serial type {other}")),
        };
        out.push(value);
        body += size;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints() {
        assert_eq!(varint(&[0x7f], 0), Some((127, 1)));
        assert_eq!(varint(&[0x81, 0x00], 0), Some((128, 2)));
        assert_eq!(varint(&[0x82, 0x2c], 0), Some((300, 2)));
    }

    #[test]
    fn records() {
        // header: size 4, types text(5 chars) = 23, int8 = 1, null = 0; body "hello", 7
        let payload = [4u8, 23, 1, 0, b'h', b'e', b'l', b'l', b'o', 7];
        assert_eq!(record(&payload).unwrap(), vec![Value::Text("hello".into()), Value::Int(7), Value::Null]);
    }

    /// Reads this machine's ReSkate profile (ignored: needs it).
    #[test]
    #[ignore]
    fn reads_the_profile() {
        let path = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("ReSkate/profiles/offline/reskate.sqlite3");
        // the file alone first (copied without its log), then with the log
        let alone = std::env::temp_dir().join("caveman-sqlite-test").join("reskate.sqlite3");
        std::fs::create_dir_all(alone.parent().unwrap()).unwrap();
        std::fs::copy(&path, &alone).unwrap();
        let _ = std::fs::remove_file(PathBuf::from(format!("{}-wal", alone.display())));
        for (label, p) in [("alone", &alone), ("with the log", &path)] {
            let db = Db::open(p).unwrap();
            println!("{label}: {} pages", db.pages.len());
            let master = db.rows(1).unwrap();
            for r in master.iter().take(40) {
                println!("  master: {:?} {:?} root {:?}", r.first().and_then(|v| v.as_text()), r.get(1).and_then(|v| v.as_text()), r.get(3));
            }
        }
        let db = Db::open(&path).unwrap();
        for table in ["cosmetic_loadouts", "profile_extensions", "player_settings"] {
            let root = db.table_root(table).unwrap();
            let rows = db.rows(root).unwrap();
            println!(
                "{table}: root {root}, {} rows; the first: {:?}",
                rows.len(),
                rows.first().map(|r| r
                    .iter()
                    .map(|v| match v {
                        Value::Text(s) => format!("text({}: {})", s.len(), &s[..s.len().min(60)]),
                        other => format!("{other:?}"),
                    })
                    .collect::<Vec<_>>())
            );
            assert!(!rows.is_empty());
        }
    }
}
