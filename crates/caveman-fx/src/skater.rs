//! The skater's states, read in the skater's own context exactly as the game's getter does (0x11dcdd0), but with safe
//! reads: the world from a game thread's TLS, the skater effects manager's records, the ANT registry entry of each
//! record's id, and the state's page and slot in that context (knowledge/state-effects-runtime.md, "The probe's
//! answer": the skater's record is the one whose context has the most pages; the board's has 2).

use crate::game::{layout, rva};
use crate::rules::Kind;
use crate::win;

/// The game's globals this build keeps them at.
#[derive(Clone, Copy, Debug)]
pub struct Runtime {
    pub base: usize,
    pub tls_index: u32,
    pub manager_offset: u32,
}

/// One skater record of the effects manager and its context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Record {
    pub manager: usize,
    pub id: u32,
    pub ctx: usize,
    pub pages: u32,
}

/// A state's value in a context.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// None: unreadable (the asset or the context went away).
    pub value: Option<f64>,
    /// The value came from the context's slot (false: the asset's Default, as the game would fall back to).
    pub live: bool,
}

impl Runtime {
    pub fn new(base: usize) -> Option<Runtime> {
        let tls_index = win::read_u32(base + rva::TLS_INDEX)?;
        let manager_offset = win::read_u32(base + rva::MANAGER_OFFSET)?;
        (tls_index < 1088 && manager_offset != 0 && manager_offset < 0x1000_0000).then_some(Runtime { base, tls_index, manager_offset })
    }

    /// The world a thread is in (its TLS block's world pointer), if any.
    pub fn world_of(&self, tid: u32) -> Option<usize> {
        let teb = teb_of(tid)?;
        let tls = win::read_ptr(teb + layout::TEB_TLS)?;
        let block = win::read_ptr(tls + self.tls_index as usize * 8)?;
        if win::read_u8(block + layout::TLS_INIT)? == 0 {
            return None;
        }
        win::read_ptr(block + layout::TLS_WORLD).filter(|&w| w >= win::LOWEST)
    }

    /// Every world some thread of this process is in: (world, a thread in it).
    pub fn worlds(&self) -> Vec<(usize, u32)> {
        let mut out: Vec<(usize, u32)> = Vec::new();
        for tid in thread_ids() {
            if let Some(w) = self.world_of(tid) {
                if !out.iter().any(|(x, _)| *x == w) {
                    out.push((w, tid));
                }
            }
        }
        out
    }

    /// The world's skater effects manager, when it is in use.
    pub fn manager(&self, world: usize) -> Option<usize> {
        let m = world.checked_add(self.manager_offset as usize)?;
        (win::read_u8(m + layout::MANAGER_READY)? != 0).then_some(m)
    }

    /// The context the trigger update uses for a record id (the ANT registry, as 0x10c4c70 looks it up).
    pub fn context(&self, id: u32) -> Option<usize> {
        let reg = win::read_ptr(self.base + rva::REGISTRY)?;
        let begin = win::read_ptr(reg + layout::REGISTRY_BEGIN)?;
        let end = win::read_ptr(reg + layout::REGISTRY_END)?;
        if end < begin || (end - begin) / 16 > 0x10000 {
            return None;
        }
        let index = (id & 0xFFFF) as usize;
        if index >= (end - begin) / 16 {
            return None;
        }
        let at = begin + index * 16;
        let generation = win::read_u16(at + 8)? as i16 as i32;
        if ((generation << 16) as u32 | index as u32) != id {
            return None;
        }
        let entry = win::read_ptr(at)?;
        (entry >= win::LOWEST).then_some(entry + layout::ENTRY_CONTEXT)
    }

    /// The records of a manager with their contexts.
    pub fn records(&self, manager: usize) -> Vec<Record> {
        let mut out = Vec::new();
        for node in tree_nodes(manager + layout::MANAGER_TREE, 256) {
            let Some(id) = win::read_u32(node + layout::NODE_ID) else { continue };
            if id == u32::MAX {
                continue;
            }
            if let Some(ctx) = self.context(id) {
                out.push(Record { manager, id, ctx, pages: page_count(ctx) });
            }
        }
        out
    }
}

/// The nodes of an EASTL red-black tree (anchor: right +0, left +8, root +0x10; nodes: right +0, left +8).
pub fn tree_nodes(anchor: usize, limit: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(root) = win::read_ptr(anchor + 0x10) else { return out };
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n < win::LOWEST || n == anchor || out.contains(&n) || out.len() >= limit {
            continue;
        }
        let mut lr = [0u8; 16];
        if !win::read_into(n, &mut lr) {
            continue;
        }
        out.push(n);
        for k in 0..2 {
            let c = usize::from_le_bytes(lr[k * 8..k * 8 + 8].try_into().unwrap());
            if c >= win::LOWEST && c != anchor {
                stack.push(c);
            }
        }
    }
    out
}

/// How many of a context's 64 page pointers are set.
pub fn page_count(ctx: usize) -> u32 {
    let mut pages = [0u8; 64 * 8];
    if !win::read_into(ctx + layout::CTX_PAGES, &mut pages) {
        return 0;
    }
    pages.chunks_exact(8).filter(|p| p.iter().any(|&b| b != 0)).count() as u32
}

/// A state's value in `ctx`, as the game's getter reads it: the asset's handle names a page and a byte offset; the
/// page must be in the context and the offset's bit set in the page's bitmap; else the asset's Default (BoolAsset)
/// or 0.
pub fn read_state(asset: usize, kind: Kind, ctx: Option<usize>) -> Reading {
    let mut hdr = [0u8; 0x80];
    if !win::read_into(asset, &mut hdr) {
        return Reading { value: None, live: false };
    }
    let default = if kind == Kind::Bool { (hdr[layout::BOOL_DEFAULT] != 0) as u8 as f64 } else { 0.0 };
    let fallback = Reading { value: Some(default), live: false };
    let handle = u32::from_le_bytes(hdr[layout::ASSET_HANDLE..layout::ASSET_HANDLE + 4].try_into().unwrap());
    let Some(ctx) = ctx else { return Reading { value: None, live: false } };
    if hdr[layout::ASSET_BOUND] == 0 {
        return fallback;
    }
    let (page, offset) = ((handle & 0xFFF) as usize, ((handle >> 12) & 0x7FF) as usize);
    let Some(pg) = win::read_ptr(ctx + layout::CTX_PAGES + page * 8) else { return Reading { value: None, live: false } };
    if pg < win::LOWEST {
        return fallback;
    }
    let Some(bits) = win::read_u32(pg + layout::PAGE_BITMAP + (offset >> 5) * 4) else { return Reading { value: None, live: false } };
    if (bits >> (offset & 31)) & 1 == 0 {
        return fallback;
    }
    let value = match kind {
        Kind::Bool => win::read_u8(pg + offset).map(|b| (b != 0) as u8 as f64),
        Kind::Int => win::read_u32(pg + offset).map(|v| v as i32 as f64),
        Kind::Float => win::read_u32(pg + offset).map(|v| f32::from_bits(v) as f64),
    };
    Reading { value, live: value.is_some() }
}

/// The ids of this process's threads.
pub fn thread_ids() -> Vec<u32> {
    let mut out = Vec::new();
    let snap = win::Handle(unsafe { win::CreateToolhelp32Snapshot(win::TH32CS_SNAPTHREAD, 0) });
    if !snap.valid() {
        return out;
    }
    let pid = win::target_pid();
    let mut te: win::THREADENTRY32 = unsafe { std::mem::zeroed() };
    te.dwSize = std::mem::size_of::<win::THREADENTRY32>() as u32;
    if unsafe { win::Thread32First(snap.0, &mut te) } != 0 {
        loop {
            if te.th32OwnerProcessID == pid {
                out.push(te.th32ThreadID);
            }
            if unsafe { win::Thread32Next(snap.0, &mut te) } == 0 {
                break;
            }
        }
    }
    out
}

/// A thread's TEB address.
pub fn teb_of(tid: u32) -> Option<usize> {
    let h = win::Handle(unsafe { win::OpenThread(win::THREAD_QUERY_LIMITED_INFORMATION, 0, tid) });
    if !h.valid() {
        return None;
    }
    let mut tbi: win::THREAD_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let status = unsafe { win::NtQueryInformationThread(h.0, 0, (&mut tbi as *mut win::THREAD_BASIC_INFORMATION).cast(), std::mem::size_of::<win::THREAD_BASIC_INFORMATION>() as u32, std::ptr::null_mut()) };
    (status == 0 && !tbi.TebBaseAddress.is_null()).then_some(tbi.TebBaseAddress as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake asset and context laid out as the game's: the reading follows the handle, the page and its bitmap.
    #[test]
    fn reads_like_the_getter() {
        let mut page = vec![0u8; 0x800];
        let mut ctx = vec![0u64; 3 + 64];
        let mut asset = vec![0u8; 0x80];
        // bool at page 2, offset 1758 (the riding state's slot in the probe), bound, Default 0
        let offset = 1758usize.min(0x71b);
        let handle = 2u32 | ((offset as u32) << 12) | (1 << 24);
        asset[0x28..0x2c].copy_from_slice(&handle.to_le_bytes());
        ctx[3 + 2] = page.as_ptr() as u64;
        let ctx_addr = ctx.as_ptr() as usize;
        let a = asset.as_ptr() as usize;
        // not valid yet: Default
        assert_eq!(read_state(a, Kind::Bool, Some(ctx_addr)), Reading { value: Some(0.0), live: false });
        page[offset] = 1;
        let bit = 0x71c + (offset >> 5) * 4;
        let word = 1u32 << (offset & 31);
        page[bit..bit + 4].copy_from_slice(&word.to_le_bytes());
        assert_eq!(read_state(a, Kind::Bool, Some(ctx_addr)), Reading { value: Some(1.0), live: true });
        // an enum: 4 bytes at an aligned offset
        let eoff = 0x40usize;
        let eh = 2u32 | ((eoff as u32) << 12) | (4 << 24);
        asset[0x28..0x2c].copy_from_slice(&eh.to_le_bytes());
        page[eoff..eoff + 4].copy_from_slice(&20i32.to_le_bytes());
        let eb = 0x71c + (eoff >> 5) * 4;
        let w = u32::from_le_bytes(page[eb..eb + 4].try_into().unwrap()) | (1 << (eoff & 31));
        page[eb..eb + 4].copy_from_slice(&w.to_le_bytes());
        assert_eq!(read_state(a, Kind::Int, Some(ctx_addr)), Reading { value: Some(20.0), live: true });
        // unbound: Default; page missing: Default; no context: unknown
        asset[0x2b] = 0;
        asset[0x78] = 1;
        assert_eq!(read_state(a, Kind::Bool, Some(ctx_addr)), Reading { value: Some(1.0), live: false });
        asset[0x2b] = 4;
        ctx[3 + 2] = 0;
        assert_eq!(read_state(a, Kind::Int, Some(ctx_addr)), Reading { value: Some(0.0), live: false });
        assert_eq!(read_state(a, Kind::Int, None).value, None);
        assert_eq!(read_state(0x10, Kind::Int, Some(ctx_addr)).value, None);
        assert_eq!(page_count(ctx_addr), 0);
        drop((page, ctx, asset));
    }

    #[test]
    fn tree_walk() {
        // anchor + three nodes: root with a left and a right child
        let mut nodes = vec![[0u64; 8]; 4];
        let addr = |v: &Vec<[u64; 8]>, k: usize| v[k].as_ptr() as u64;
        let (anchor, root, left, right) = (addr(&nodes, 0), addr(&nodes, 1), addr(&nodes, 2), addr(&nodes, 3));
        nodes[0][2] = root;
        nodes[1][0] = right;
        nodes[1][1] = left;
        nodes[2][0] = anchor; // a leaf pointing back at the anchor (as EASTL's do not, but must be safe)
        let mut got = tree_nodes(anchor as usize, 16);
        got.sort();
        let mut want = vec![root as usize, left as usize, right as usize];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn this_process_has_threads_and_tebs() {
        let tids = thread_ids();
        assert!(!tids.is_empty());
        let me = unsafe { win::GetCurrentThreadId() };
        assert!(tids.contains(&me));
        assert!(teb_of(me).is_some());
    }
}
