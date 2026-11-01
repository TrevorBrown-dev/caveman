//! The copy tracer: every `CopyTextureRegion` and `CopyResource` the game records (hooked in the lists' own tables as
//! the barriers are: `depth.rs`, slots 16 and 17) goes into a lock-free ring of 1024 entries: the present, the list
//! and its type, the source and destination resources and subresources, whole or a box, and (from the
//! `ExecuteCommandLists` hook) the queue the list was submitted on and its type. Our own copies (the `OURS`
//! thread-local of the upload) are skipped. The game records more than a thousand copies a frame (streaming slices,
//! back buffers), so the generic ring holds a second or less; a second ring, the deck ring of 8192, takes only the
//! copies whose source or destination is deck-shaped (2D, the main's width and height, any format: a set the render
//! thread fills from the describe cache), whose destination is a tile array (a 256 x 256 texture array of a BC
//! format: the slices the game's GPU compressor fills at equip, which the world samples), or not described yet (a
//! resource just made: described at the next present, and once described as something else its later copies are
//! skipped). `board copies` prints the generic ring,
//! `board copies --deck` the deck ring, with each resource's shape (`board.rs`). The question they answer: after one
//! equip, which destinations the equip-time textures (the UAV working textures, the RGBA8 sRGB and BC7 results) are
//! copied into: that destination is the ridden board's texture.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use serde_json::{json, Value};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;

pub const RING: usize = 1024;
pub const DECK_RING: usize = 8192;
#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO64: AtomicU64 = AtomicU64::new(0);

/// One ring of copies: `kind << 60 | list type << 56 | box << 55 | queue type << 48 | src subresource << 24 | dst
/// subresource` in `meta`.
struct Ring<const N: usize> {
    src: [AtomicUsize; N],
    dst: [AtomicUsize; N],
    list: [AtomicUsize; N],
    queue: [AtomicUsize; N],
    present: [AtomicU64; N],
    meta: [AtomicU64; N],
    next: AtomicUsize,
}

impl<const N: usize> Ring<N> {
    const fn new() -> Ring<N> {
        Ring { src: [ZERO; N], dst: [ZERO; N], list: [ZERO; N], queue: [ZERO; N], present: [ZERO64; N], meta: [ZERO64; N], next: AtomicUsize::new(0) }
    }

    fn push(&self, list: *mut c_void, present: u64, meta: u64, src: usize, dst: usize) {
        let at = self.next.fetch_add(1, Ordering::AcqRel) % N;
        self.src[at].store(0, Ordering::Release);
        self.present[at].store(present, Ordering::Relaxed);
        self.list[at].store(list as usize, Ordering::Relaxed);
        self.queue[at].store(0, Ordering::Relaxed);
        self.meta[at].store(meta, Ordering::Relaxed);
        self.dst[at].store(dst, Ordering::Relaxed);
        self.src[at].store(src, Ordering::Release);
    }

    /// The copies of the last few presents recorded on `lists` are on `queue` now (its type noted).
    fn stamp_queue(&self, now: u64, queue: usize, qtype: &mut Option<u64>, lists: &[*mut c_void], queue_ptr: *mut c_void) {
        let next = self.next.load(Ordering::Acquire);
        for i in 1..=N {
            let at = (next + N - i) % N;
            let p = self.present[at].load(Ordering::Relaxed);
            if self.src[at].load(Ordering::Acquire) == 0 && p == 0 {
                break;
            }
            if now.saturating_sub(p) > 4 {
                break;
            }
            let l = self.list[at].load(Ordering::Relaxed);
            if self.queue[at].load(Ordering::Relaxed) == 0 && lists.iter().any(|&x| x as usize == l) {
                // SAFETY: the queue whose method we are in
                let t = *qtype.get_or_insert_with(|| unsafe {
                    ID3D12CommandQueue::from_raw_borrowed(&queue_ptr).map(|q| q.GetDesc().Type.0 as u64 & 0xff).unwrap_or(TYPE_UNKNOWN)
                });
                self.queue[at].store(queue, Ordering::Relaxed);
                self.meta[at].fetch_and(!(0xff << 48), Ordering::AcqRel);
                self.meta[at].fetch_or(t << 48, Ordering::AcqRel);
            }
        }
    }

    /// Newest first.
    fn entries(&self) -> Vec<Copy> {
        let next = self.next.load(Ordering::Acquire);
        let mut out = Vec::new();
        for i in 1..=N {
            let at = (next + N - i) % N;
            let src = self.src[at].load(Ordering::Acquire);
            if src == 0 {
                continue;
            }
            let m = self.meta[at].load(Ordering::Relaxed);
            out.push(Copy {
                present: self.present[at].load(Ordering::Relaxed),
                list: self.list[at].load(Ordering::Relaxed),
                list_type: (m >> 56) & 0xf,
                queue: self.queue[at].load(Ordering::Relaxed),
                queue_type: (m >> 48) & 0xff,
                kind: m >> 60,
                boxed: (m >> 55) & 1 != 0,
                src,
                src_sub: ((m >> 24) & 0xff_ffff) as u32,
                dst: self.dst[at].load(Ordering::Relaxed),
                dst_sub: (m & 0xff_ffff) as u32,
            });
        }
        out
    }
}

static GENERIC: Ring<RING> = Ring::new();
static DECK: Ring<DECK_RING> = Ring::new();
/// Copies recorded since the DLL came in (the rings keep the last 1024 and 8192).
pub static COPIES_SEEN: AtomicU64 = AtomicU64::new(0);
pub static DECK_SEEN: AtomicU64 = AtomicU64::new(0);

/// The deck-shaped set: resources described as 2D of the main's width and height (any format), filled by the
/// render thread from the describe cache (`set_deck`), packed (the first 0 ends it).
pub const DECK_SET: usize = 256;
static DECK_RES: [AtomicUsize; DECK_SET] = [ZERO; DECK_SET];
/// The tile arrays: resources described as 256 x 256 texture arrays of a BC format (the slices the game's GPU
/// compressor writes at equip: the world's pages), filled by the render thread (`set_tiles`), packed.
static TILE_RES: [AtomicUsize; DECK_SET] = [ZERO; DECK_SET];

/// The render thread: the tile arrays are these (at most `DECK_SET`).
pub fn set_tiles(resources: &[usize]) {
    for (k, slot) in TILE_RES.iter().enumerate() {
        slot.store(resources.get(k).copied().unwrap_or(0), Ordering::Release);
    }
}

/// Whether `res` is a tile array.
pub fn in_tiles(res: usize) -> bool {
    for slot in &TILE_RES {
        let r = slot.load(Ordering::Relaxed);
        if r == 0 {
            return false;
        }
        if r == res {
            return true;
        }
    }
    false
}

/// Resources described (a table by a hash of the pointer; a collision evicts, so a resource may count as not
/// described and be queued again: harmless).
const DESCRIBED_N: usize = 4096;
static DESCRIBED: [AtomicUsize; DESCRIBED_N] = [ZERO; DESCRIBED_N];

/// The render thread: `res` was described (deck-shaped or not).
pub fn note_described(res: usize) {
    if res != 0 {
        DESCRIBED[(res >> 4) % DESCRIBED_N].store(res, Ordering::Release);
    }
}

/// The render thread: the deck-shaped set is these (at most `DECK_SET`).
pub fn set_deck(resources: &[usize]) {
    for (k, slot) in DECK_RES.iter().enumerate() {
        slot.store(resources.get(k).copied().unwrap_or(0), Ordering::Release);
    }
}

/// Whether `res` is in the deck-shaped set (a few atomic loads: the set is packed).
pub fn in_deck(res: usize) -> bool {
    for slot in &DECK_RES {
        let r = slot.load(Ordering::Relaxed);
        if r == 0 {
            return false;
        }
        if r == res {
            return true;
        }
    }
    false
}

/// Whether `res` has been described (the shape known: deck-shaped or not).
pub fn described(res: usize) -> bool {
    DESCRIBED[(res >> 4) % DESCRIBED_N].load(Ordering::Relaxed) == res
}

pub const KIND_REGION: u64 = 1;
pub const KIND_RESOURCE: u64 = 2;
/// A subresource index that means a buffer location (a placed footprint) in `CopyTextureRegion`.
pub const SUB_FOOTPRINT: u32 = 0xff_fffe;
/// The whole resource (`CopyResource`).
pub const SUB_ALL: u32 = 0xff_ffff;
const TYPE_UNKNOWN: u64 = 0xff;

fn list_type(list: *mut c_void) -> u64 {
    // SAFETY: the list is the object whose method we are in
    unsafe { ID3D12CommandList::from_raw_borrowed(&list).map(|l| l.GetType().0 as u64 & 0xff).unwrap_or(TYPE_UNKNOWN) }
}

/// A copy the game recorded on `list` (from the hooks; never our own): the generic ring always, the deck ring when
/// the source or the destination is deck-shaped or not described yet.
pub fn record(list: *mut c_void, kind: u64, src: usize, src_sub: u32, dst: usize, dst_sub: u32, boxed: bool) {
    // (the board's own copies, recorded on the render thread with its RECORDING flag set, are not the game's)
    if crate::board::recording() {
        return;
    }
    let present = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    let meta = (kind & 0xf) << 60
        | list_type(list) << 56
        | (boxed as u64) << 55
        | TYPE_UNKNOWN << 48
        | (src_sub as u64 & 0xff_ffff) << 24
        | (dst_sub as u64 & 0xff_ffff);
    GENERIC.push(list, present, meta, src, dst);
    COPIES_SEEN.fetch_add(1, Ordering::Relaxed);
    if in_deck(src) || in_deck(dst) || in_tiles(dst) || !described(src) || !described(dst) {
        DECK.push(list, present, meta, src, dst);
        DECK_SEEN.fetch_add(1, Ordering::Relaxed);
    }
    crate::board::want_desc(src);
    crate::board::want_desc(dst);
}

/// From the overlay's `ExecuteCommandLists` hook: the copies of the last few presents recorded on these lists are
/// now on `queue` (its type noted), in both rings.
///
/// # Safety
/// `queue` is the queue whose method we are in.
pub unsafe fn on_execute(queue: *mut c_void, lists: &[*mut c_void]) {
    let now = crate::depth::PRESENT_NO.load(Ordering::Relaxed);
    let mut qtype: Option<u64> = None;
    GENERIC.stamp_queue(now, queue as usize, &mut qtype, lists, queue);
    DECK.stamp_queue(now, queue as usize, &mut qtype, lists, queue);
}

/// One copy of a ring.
#[derive(Clone, Copy, Debug)]
pub struct Copy {
    pub present: u64,
    pub list: usize,
    pub list_type: u64,
    pub queue: usize,
    pub queue_type: u64,
    pub kind: u64,
    pub boxed: bool,
    pub src: usize,
    pub src_sub: u32,
    pub dst: usize,
    pub dst_sub: u32,
}

/// The generic ring, newest first.
pub fn entries() -> Vec<Copy> {
    GENERIC.entries()
}

/// The deck ring, newest first.
pub fn deck_entries() -> Vec<Copy> {
    DECK.entries()
}

fn type_name(t: u64) -> &'static str {
    match t {
        0 => "direct",
        1 => "bundle",
        2 => "compute",
        3 => "copy",
        _ => "unknown",
    }
}

fn sub_name(s: u32) -> String {
    match s {
        SUB_ALL => "all".into(),
        SUB_FOOTPRINT => "buffer".into(),
        n => n.to_string(),
    }
}

fn entry_json(c: &Copy, now_present: u64, desc: &impl Fn(usize) -> Value) -> Value {
    json!({
        "present": c.present,
        "presents_ago": now_present.saturating_sub(c.present),
        "kind": if c.kind == KIND_RESOURCE { "CopyResource" } else { "CopyTextureRegion" },
        "box": c.boxed,
        "list": format!("{:#x}", c.list),
        "list_type": type_name(c.list_type),
        "queue": if c.queue == 0 { Value::Null } else { json!(format!("{:#x}", c.queue)) },
        "queue_type": if c.queue == 0 { "not submitted yet" } else { type_name(c.queue_type) },
        "src": {"resource": format!("{:#x}", c.src), "subresource": sub_name(c.src_sub), "desc": desc(c.src)},
        "dst": {"resource": format!("{:#x}", c.dst), "subresource": sub_name(c.dst_sub), "desc": desc(c.dst)},
    })
}

/// The generic ring for the status (`board copies`): `desc` gives a resource's shape when it was described.
pub fn status(now_present: u64, desc: impl Fn(usize) -> Value) -> Value {
    entries().iter().map(|c| entry_json(c, now_present, &desc)).collect()
}

/// The deck ring for the status (`board copies --deck`).
pub fn deck_status(now_present: u64, desc: impl Fn(usize) -> Value) -> Value {
    deck_entries().iter().map(|c| entry_json(c, now_present, &desc)).collect()
}

/// The resources of both rings' last `presents` presents (for the render thread to describe).
pub fn recent_resources(now_present: u64, presents: u64) -> Vec<usize> {
    let mut out = Vec::new();
    for c in entries().into_iter().chain(deck_entries()) {
        if now_present.saturating_sub(c.present) > presents {
            continue;
        }
        for r in [c.src, c.dst] {
            if !out.contains(&r) {
                out.push(r);
            }
        }
    }
    out
}
