//! Finds the game's state assets in memory, in process and off the game's threads: every 8-byte aligned word of the
//! process's committed private memory that equals one of the asset classes' vtables is an asset candidate; its Name
//! pointer (+0x18) names it. Only resident pages are read (`QueryWorkingSetEx`): reading a committed page the game
//! never touched would make Windows give it memory. The scan runs in steps of a byte budget, so the worker keeps
//! ticking (and answering) while it runs.

use crate::game::layout;
use crate::rules::Kind;
use crate::win;

/// What to look for.
#[derive(Clone, Debug)]
pub struct ScanSpec {
    /// (asset vtable address, kind), up to three.
    pub classes: Vec<(usize, Kind)>,
    /// A BoolAsset must also carry this vtable at +0x60 (its bool view).
    pub bool_view_vtable: Option<usize>,
    /// Names wanted exactly (any case).
    pub wanted: Vec<String>,
    /// And every asset whose name starts with this (lower case): Caveman's own.
    pub prefix: String,
    /// Also scan mapped (file-backed or shared) memory, not only private.
    pub mapped: bool,
}

/// An asset found.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub asset: usize,
    pub kind: Kind,
    /// The Name as the game holds it.
    pub name: String,
}

const PAGE: usize = 0x1000;
const CHUNK: usize = 1 << 20;

/// A scan in progress.
pub struct Scanner {
    spec: ScanSpec,
    wanted_lower: Vec<String>,
    targets: [u64; 3],
    kinds: [Kind; 3],
    next: usize,
    region_end: usize,
    cursor: usize,
    buf: *mut u8,
    pages: Vec<win::PSAPI_WORKING_SET_EX_INFORMATION>,
    candidates: Vec<usize>,
    pub hits: Vec<Hit>,
    /// Bytes read, regions seen, words matching a vtable.
    pub bytes: u64,
    pub regions: u32,
    pub candidates_seen: u64,
    done: bool,
}

// The buffer is only touched by the thread that steps the scan.
unsafe impl Send for Scanner {}

impl Drop for Scanner {
    fn drop(&mut self) {
        if !self.buf.is_null() {
            unsafe { win::VirtualFree(self.buf.cast(), 0, win::MEM_RELEASE) };
        }
    }
}

/// Readable, and plain cached memory: guard pages are left alone, and uncached or write-combined memory (the
/// graphics driver's upload heaps) is skipped: it holds no assets, and reading it is very slow.
fn readable(protect: u32) -> bool {
    protect & (win::PAGE_GUARD | win::PAGE_NOCACHE | win::PAGE_WRITECOMBINE) == 0
        && matches!(
            protect & 0xFF,
            win::PAGE_READONLY | win::PAGE_READWRITE | win::PAGE_WRITECOPY | win::PAGE_EXECUTE_READ | win::PAGE_EXECUTE_READWRITE | win::PAGE_EXECUTE_WRITECOPY
        )
}

/// The indices of the words equal to one of `t` (blocks are tested together first, so the common case is a few
/// vector compares per 256 bytes).
pub fn find_words(words: &[u64], t: [u64; 3], out: &mut Vec<usize>) {
    const B: usize = 32;
    let mut blocks = words.chunks_exact(B);
    let mut at = 0;
    for block in &mut blocks {
        let mut any = false;
        for &w in block {
            any |= (w == t[0]) | (w == t[1]) | (w == t[2]);
        }
        if any {
            for (k, &w) in block.iter().enumerate() {
                if w == t[0] || w == t[1] || w == t[2] {
                    out.push(at + k);
                }
            }
        }
        at += B;
    }
    for (k, &w) in blocks.remainder().iter().enumerate() {
        if w == t[0] || w == t[1] || w == t[2] {
            out.push(at + k);
        }
    }
}

impl Scanner {
    pub fn new(spec: ScanSpec) -> Result<Scanner, String> {
        if spec.classes.is_empty() || spec.classes.len() > 3 {
            return Err("one to three asset classes".into());
        }
        let mut targets = [spec.classes[0].0 as u64; 3];
        let mut kinds = [spec.classes[0].1; 3];
        for (i, (vt, kind)) in spec.classes.iter().enumerate() {
            targets[i] = *vt as u64;
            kinds[i] = *kind;
        }
        let buf = unsafe { win::VirtualAlloc(std::ptr::null_mut(), CHUNK, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE) } as *mut u8;
        if buf.is_null() {
            return Err("no memory for the scan buffer".into());
        }
        let wanted_lower = spec.wanted.iter().map(|w| w.to_ascii_lowercase()).collect();
        Ok(Scanner {
            spec,
            wanted_lower,
            targets,
            kinds,
            next: win::LOWEST,
            region_end: 0,
            cursor: 0,
            buf,
            pages: Vec::with_capacity(CHUNK / PAGE),
            candidates: Vec::new(),
            hits: Vec::new(),
            bytes: 0,
            regions: 0,
            candidates_seen: 0,
            done: false,
        })
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Scans on until about `budget` bytes have been read or `max_ms` milliseconds have passed; true when the whole
    /// address space is done.
    pub fn step(&mut self, budget: usize, max_ms: u64) -> bool {
        let start = std::time::Instant::now();
        let mut spent = 0usize;
        while !self.done && spent < budget && (start.elapsed().as_millis() as u64) < max_ms {
            if self.cursor >= self.region_end {
                if !self.next_region() {
                    self.done = true;
                    break;
                }
                continue;
            }
            let len = (self.region_end - self.cursor).min(CHUNK);
            spent += self.scan_chunk(self.cursor, len).max(PAGE);
            self.cursor += len;
        }
        self.done
    }

    /// Moves to the next region worth reading; false at the end of the address space.
    fn next_region(&mut self) -> bool {
        let own = self.buf as usize;
        while self.next < win::HIGHEST {
            let Some(mbi) = win::query(self.next) else { return false };
            let start = mbi.BaseAddress as usize;
            let end = start.saturating_add(mbi.RegionSize);
            if end <= self.next {
                return false;
            }
            self.next = end;
            let kind_ok = mbi.Type == win::MEM_PRIVATE || (self.spec.mapped && mbi.Type == win::MEM_MAPPED);
            if mbi.State == win::MEM_COMMIT && kind_ok && readable(mbi.Protect) && !(start <= own && own < end) {
                self.regions += 1;
                self.cursor = start;
                self.region_end = end;
                return true;
            }
        }
        false
    }

    /// Scans [at, at + len): its resident pages, in runs. Returns the bytes read.
    fn scan_chunk(&mut self, at: usize, len: usize) -> usize {
        let npages = len / PAGE;
        self.pages.clear();
        for p in 0..npages {
            self.pages.push(win::PSAPI_WORKING_SET_EX_INFORMATION { VirtualAddress: (at + p * PAGE) as *mut _, VirtualAttributes: 0 });
        }
        let size = (npages * std::mem::size_of::<win::PSAPI_WORKING_SET_EX_INFORMATION>()) as u32;
        if npages == 0 || unsafe { win::K32QueryWorkingSetEx(win::target(), self.pages.as_mut_ptr().cast(), size) } == 0 {
            return 0;
        }
        let mut read = 0;
        let mut p = 0;
        while p < npages {
            if self.pages[p].VirtualAttributes & 1 == 0 {
                p += 1;
                continue;
            }
            let first = p;
            while p < npages && self.pages[p].VirtualAttributes & 1 != 0 {
                p += 1;
            }
            let (start, bytes) = (at + first * PAGE, (p - first) * PAGE);
            // SAFETY: buf holds CHUNK bytes and bytes <= len <= CHUNK.
            let slice = unsafe { std::slice::from_raw_parts_mut(self.buf, bytes) };
            if !win::read_into(start, slice) {
                // a page went away between the query and the read: page by page
                for k in 0..(bytes / PAGE) {
                    let page = unsafe { std::slice::from_raw_parts_mut(self.buf, PAGE) };
                    if win::read_into(start + k * PAGE, page) {
                        self.scan_buffer(start + k * PAGE, PAGE);
                        read += PAGE;
                    }
                }
                continue;
            }
            self.scan_buffer(start, bytes);
            read += bytes;
        }
        self.bytes += read as u64;
        read
    }

    fn scan_buffer(&mut self, addr: usize, bytes: usize) {
        // SAFETY: the buffer is page-aligned (VirtualAlloc) and holds `bytes` bytes just read.
        let words = unsafe { std::slice::from_raw_parts(self.buf as *const u64, bytes / 8) };
        self.candidates.clear();
        find_words(words, self.targets, &mut self.candidates);
        let found: Vec<(usize, u64)> = self.candidates.iter().map(|&i| (addr + i * 8, words[i])).collect();
        self.candidates_seen += found.len() as u64;
        for (asset, vt) in found {
            let kind = self.kinds[self.targets.iter().position(|&t| t == vt).unwrap_or(0)];
            self.consider(asset, kind);
        }
    }

    fn consider(&mut self, asset: usize, kind: Kind) {
        let Some(name) = win::read_ptr(asset + layout::ASSET_NAME).and_then(|p| win::read_cstr(p, 160)) else { return };
        let lower = name.to_ascii_lowercase();
        if !(lower.starts_with(&self.spec.prefix) || self.wanted_lower.contains(&lower)) {
            return;
        }
        if kind == Kind::Bool {
            if let Some(vt) = self.spec.bool_view_vtable {
                if win::read_ptr(asset + layout::BOOL_VIEW) != Some(vt) {
                    return;
                }
            }
        }
        if !self.hits.iter().any(|h| h.asset == asset) {
            self.hits.push(Hit { asset, kind, name });
        }
    }
}

/// What a [`WordScan`] looks for in each 8-byte aligned word.
#[derive(Clone, Copy, Debug)]
pub enum Words {
    /// Equal to one of up to three values.
    Any([u64; 3]),
    /// In [lo, hi) (pointers into one module, say).
    Range(u64, u64),
    /// One of many values (`WordScan::many`: the sorted set is the scan's own field; a word's high half must be one
    /// of the set's first).
    Set,
}

/// Every 8-byte aligned word of committed, private, plain cached memory that matches, in steps (as [`Scanner`]):
/// for finding objects of a class by their vtable, or every pointer into a module. Only the addresses are kept (a
/// kept value would match itself when the scan reaches its own list).
pub struct WordScan {
    words: Words,
    /// `Words::Set`: the values, sorted, and the distinct high halves among them (the cheap test first).
    set: Vec<u64>,
    hi: Vec<u32>,
    /// Also read pages the process does not hold in memory now (they are paged in).
    all_pages: bool,
    next: usize,
    region_end: usize,
    cursor: usize,
    buf: *mut u8,
    pages: Vec<win::PSAPI_WORKING_SET_EX_INFORMATION>,
    candidates: Vec<usize>,
    /// Where the matching words are.
    pub found: Vec<usize>,
    /// At most this many are kept.
    pub limit: usize,
    pub bytes: u64,
    done: bool,
}

unsafe impl Send for WordScan {}

impl Drop for WordScan {
    fn drop(&mut self) {
        if !self.buf.is_null() {
            unsafe { win::VirtualFree(self.buf.cast(), 0, win::MEM_RELEASE) };
        }
    }
}

impl WordScan {
    /// Words equal to one of `values` (one to three), resident pages only.
    pub fn new(values: &[u64], limit: usize) -> Result<WordScan, String> {
        if values.is_empty() || values.len() > 3 {
            return Err("one to three values".into());
        }
        let mut targets = [values[0]; 3];
        targets[..values.len()].copy_from_slice(values);
        WordScan::with(Words::Any(targets), false, limit)
    }

    pub fn with(words: Words, all_pages: bool, limit: usize) -> Result<WordScan, String> {
        let buf = unsafe { win::VirtualAlloc(std::ptr::null_mut(), CHUNK, win::MEM_COMMIT | win::MEM_RESERVE, win::PAGE_READWRITE) } as *mut u8;
        if buf.is_null() {
            return Err("no memory for the scan buffer".into());
        }
        Ok(WordScan {
            words,
            set: Vec::new(),
            hi: Vec::new(),
            all_pages,
            next: win::LOWEST,
            region_end: 0,
            cursor: 0,
            buf,
            pages: Vec::with_capacity(CHUNK / PAGE),
            candidates: Vec::new(),
            found: Vec::new(),
            limit,
            bytes: 0,
            done: false,
        })
    }

    /// Words equal to one of many `values` (pointers to resources, say: a word's high half is tested against the few
    /// the set has, then the sorted set is searched), resident pages only.
    pub fn many(values: &[u64], limit: usize) -> Result<WordScan, String> {
        if values.is_empty() {
            return Err("no values".into());
        }
        let mut s = WordScan::with(Words::Set, false, limit)?;
        s.set = values.to_vec();
        s.set.sort_unstable();
        s.set.dedup();
        s.hi = s.set.iter().map(|v| (v >> 32) as u32).collect();
        s.hi.dedup();
        Ok(s)
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Scans on for about `budget` bytes or `max_ms` milliseconds; true when the address space is done (or the limit
    /// is reached).
    pub fn step(&mut self, budget: usize, max_ms: u64) -> bool {
        let start = std::time::Instant::now();
        let mut spent = 0usize;
        while !self.done && spent < budget && (start.elapsed().as_millis() as u64) < max_ms {
            if self.found.len() >= self.limit {
                self.done = true;
                break;
            }
            if self.cursor >= self.region_end {
                if !self.next_region() {
                    self.done = true;
                    break;
                }
                continue;
            }
            let len = (self.region_end - self.cursor).min(CHUNK);
            spent += self.chunk(self.cursor, len).max(PAGE);
            self.cursor += len;
        }
        self.done
    }

    fn next_region(&mut self) -> bool {
        let own = self.buf as usize;
        while self.next < win::HIGHEST {
            let Some(mbi) = win::query(self.next) else { return false };
            let start = mbi.BaseAddress as usize;
            let end = start.saturating_add(mbi.RegionSize);
            if end <= self.next {
                return false;
            }
            self.next = end;
            if mbi.State == win::MEM_COMMIT && mbi.Type == win::MEM_PRIVATE && readable(mbi.Protect) && !(start <= own && own < end) {
                self.cursor = start;
                self.region_end = end;
                return true;
            }
        }
        false
    }

    fn chunk(&mut self, at: usize, len: usize) -> usize {
        let npages = len / PAGE;
        if npages == 0 {
            return 0;
        }
        if !self.all_pages {
            self.pages.clear();
            for p in 0..npages {
                self.pages.push(win::PSAPI_WORKING_SET_EX_INFORMATION { VirtualAddress: (at + p * PAGE) as *mut _, VirtualAttributes: 0 });
            }
            let size = (npages * std::mem::size_of::<win::PSAPI_WORKING_SET_EX_INFORMATION>()) as u32;
            if unsafe { win::K32QueryWorkingSetEx(win::target(), self.pages.as_mut_ptr().cast(), size) } == 0 {
                return 0;
            }
        }
        let resident = |p: usize, pages: &[win::PSAPI_WORKING_SET_EX_INFORMATION]| pages.get(p).is_none_or(|x| x.VirtualAttributes & 1 != 0);
        let pages = if self.all_pages { Vec::new() } else { std::mem::take(&mut self.pages) };
        let mut read = 0;
        let mut p = 0;
        while p < npages {
            if !resident(p, &pages) {
                p += 1;
                continue;
            }
            let first = p;
            while p < npages && resident(p, &pages) {
                p += 1;
            }
            let (start, bytes) = (at + first * PAGE, (p - first) * PAGE);
            // SAFETY: buf holds CHUNK bytes and bytes <= len <= CHUNK.
            let slice = unsafe { std::slice::from_raw_parts_mut(self.buf, bytes) };
            if !win::read_into(start, slice) {
                // a page went away: page by page
                for k in 0..(bytes / PAGE) {
                    let page = unsafe { std::slice::from_raw_parts_mut(self.buf, PAGE) };
                    if win::read_into(start + k * PAGE, page) {
                        self.match_words(start + k * PAGE, PAGE);
                        read += PAGE;
                    }
                }
                continue;
            }
            self.match_words(start, bytes);
            read += bytes;
        }
        if !self.all_pages {
            self.pages = pages;
        }
        self.bytes += read as u64;
        read
    }

    fn match_words(&mut self, start: usize, bytes: usize) {
        // SAFETY: page-aligned buffer, `bytes` just read.
        let words = unsafe { std::slice::from_raw_parts(self.buf as *const u64, bytes / 8) };
        match self.words {
            Words::Any(t) => {
                self.candidates.clear();
                find_words(words, t, &mut self.candidates);
                for &i in &self.candidates {
                    if self.found.len() < self.limit {
                        self.found.push(start + i * 8);
                    }
                }
            }
            Words::Range(lo, hi) => {
                for (i, &w) in words.iter().enumerate() {
                    if w >= lo && w < hi && self.found.len() < self.limit {
                        self.found.push(start + i * 8);
                    }
                }
            }
            Words::Set => {
                for (i, &w) in words.iter().enumerate() {
                    if w != 0 && self.hi.contains(&((w >> 32) as u32)) && self.set.binary_search(&w).is_ok() && self.found.len() < self.limit {
                        self.found.push(start + i * 8);
                    }
                }
            }
        }
    }
}

/// Whether an asset found earlier is still there: its vtable and its Name unchanged.
pub fn still_there(hit: &Hit, vtable: usize) -> bool {
    win::read_ptr(hit.asset) == Some(vtable)
        && win::read_ptr(hit.asset + layout::ASSET_NAME).and_then(|p| win::read_cstr(p, 160)).as_deref() == Some(hit.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        let mut w = vec![0u64; 100];
        w[3] = 7;
        w[40] = 9;
        w[99] = 7;
        let mut out = Vec::new();
        find_words(&w, [7, 8, 9], &mut out);
        assert_eq!(out, vec![3, 40, 99]);
    }

    /// Planted fake assets in this process are found, with names, kinds and the view check.
    #[test]
    fn finds_planted_assets() {
        // vtable values no real object here has: addresses inside a block of ours
        let marks = vec![0u64; 4];
        let base = marks.as_ptr() as u64;
        let (vt_bool, vt_enum, vt_view) = (base, base + 8, base + 16);
        let names: Vec<std::ffi::CString> =
            ["Animation/Caveman/FX.Test", "Animation/Dingo/EnumGS.Anim.CurrentFlipTrick", "Animation/Caveman/FX.NoView", "Animation/Other/Thing"]
                .iter()
                .map(|s| std::ffi::CString::new(*s).unwrap())
                .collect();
        let mut objects = vec![[0u64; 16]; 4];
        let kinds = [vt_bool, vt_enum, vt_bool, vt_bool];
        for (k, o) in objects.iter_mut().enumerate() {
            o[0] = kinds[k];
            o[3] = names[k].as_ptr() as u64;
            if k != 2 {
                o[12] = vt_view; // +0x60
            }
        }
        let spec = ScanSpec {
            classes: vec![(vt_bool as usize, Kind::Bool), (vt_enum as usize, Kind::Int)],
            bool_view_vtable: Some(vt_view as usize),
            wanted: vec!["animation/dingo/enumgs.anim.currentfliptrick".into()],
            prefix: "animation/caveman/".into(),
            mapped: false,
        };
        let mut s = Scanner::new(spec).unwrap();
        while !s.step(64 << 20, 1000) {}
        let at = |k: usize| objects[k].as_ptr() as usize;
        let mut got: Vec<(usize, Kind, String)> = s.hits.iter().map(|h| (h.asset, h.kind, h.name.clone())).collect();
        got.sort();
        let mut want =
            vec![(at(0), Kind::Bool, "Animation/Caveman/FX.Test".to_string()), (at(1), Kind::Int, "Animation/Dingo/EnumGS.Anim.CurrentFlipTrick".to_string())];
        want.sort();
        // the planted ones are found; a stale copy of them elsewhere in this process (a moved or freed buffer: it
        // happened now and then) is an asset by the scan's rules too, so other hits may only be copies of the two
        for w in &want {
            assert!(got.contains(w), "{w:?} not found in {got:?} (scanned {} MB in {} regions)", s.bytes >> 20, s.regions);
        }
        assert!(got.iter().all(|g| want.iter().any(|w| w.1 == g.1 && w.2 == g.2)), "{got:?}");
        assert!(!got.iter().any(|g| g.0 == at(2) || g.0 == at(3)), "no view, or not ours: {got:?}");
        let fx_test = s.hits.iter().find(|h| h.asset == at(0)).unwrap().clone();
        assert!(still_there(&fx_test, vt_bool as usize));
        let (a0, a1) = (at(0), at(1));
        objects[0][0] = 0;
        objects[1][0] = 0;
        assert!(!s.hits.iter().any(|h| (h.asset == a0 || h.asset == a1) && (still_there(h, vt_bool as usize) || still_there(h, vt_enum as usize))));
        drop(marks);
    }
}
