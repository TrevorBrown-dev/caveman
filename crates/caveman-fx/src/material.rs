//! The ridden board's live material parameters: the graphic's tint, the holo's colour, pattern and amount, the
//! metal mask, read from and written into the `Deck_Mat` material instances of the board the local skater rides,
//! then published to its meshes with the game's own call, so the board changes at once (no re-equip, no texture).
//!
//! Ported from ReSkate's own code (GPL-3.0, as Caveman's ported code is), build 20260929, which colours the boards
//! of its listed players this way every client tick (`Extension/Customization/developer_board_material.h`,
//! `developer_board.cpp`, `developer_hoodie_material.h`; the addresses in `Engine/Game/Build/20260929/
//! native_cosmetics.h` and `engine.h`):
//!
//! - The board entity (scene.rs finds it) has a skater appearance component (vtable exe+0x61c9ec0) whose item
//!   controllers (pointers from +0x90 to +0x98; each names its slot at +0x30 and points back at the appearance's
//!   +0x40 from +0x48) include the deck's, "Characters/Customization/Board/Slots/Skateboard_Deck_Slot", loaded when
//!   bit 1 of +0xb0 is set. Its named materials are a hash map at +0x1f0 (buckets), +0x1f8 (size), +0x1fc (count):
//!   nodes chained at +0x28, the name's hash (djb2 with xor, lower case) at +0, up to four material instances
//!   (variants) from +8; "Deck_Mat" is the deck's.
//! - A material instance's parameters are a hash map at +0x268 (buckets), +0x270 (size), +0x274 (count): nodes
//!   chained at +0x40, keyed by the first eight serialized bytes of the parameter's GUID (at +8); each node starts
//!   with the native ParamDbKey (identity = the GUID's last two bytes, size, type index, count), holds the value
//!   inline at +0x10, the type descriptor at +0x20 and flags at +0x28 (bit 0: set).
//! - A write: the value into the node, byte +0x2f6 of the material set to 1, then the game's publish
//!   (exe+0x1331420, `(item, 0)`), which copies the parameters to the render meshes. It shows one publish late, so a
//!   value that then stands still is published three more times (ReSkate's `settle_publishes`).
//!
//! Every address is checked against its class's vtable or its bounds before it is followed, all reads and writes go
//! through the kernel (win.rs), and the publish call's code is compared with the known build's first 24 bytes first.

use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
// (the publish job's statics are below; the worker thread only queues, the engine thread publishes)

use serde_json::{json, Value};

use crate::{scene, win};

/// RVAs of build 20260929 (from ReSkate's build tables).
pub mod rva {
    /// The skater appearance component (the board entity has one too).
    pub const SKATER_APPEARANCE_VTABLE: usize = 0x61c9ec0;
    /// The customization component: its bytes +0x10a and +0x10d say a recipe is on its way.
    pub const COSMETIC_COMPONENT_VTABLE: usize = 0x6084c88;
    /// Publishes an appearance item's per-instance named material map to its render meshes: `(item, map_index)`.
    pub const PUBLISH_MATERIALS: usize = 0x1331420;
    pub const PUBLISH_MATERIALS_PREFIX: [u8; 24] =
        [0x48, 0x89, 0x5c, 0x24, 0x08, 0x89, 0x54, 0x24, 0x10, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xec, 0x20];
    /// The typed per-instance shader setter (`MaterialInstance:addEsVector`): `(material, key, value, priority)`.
    pub const SET_SHADER_PARAMETER: usize = 0x1312d00;
    pub const SET_SHADER_PARAMETER_PREFIX: [u8; 24] =
        [0x48, 0x89, 0x5c, 0x24, 0x20, 0x48, 0x89, 0x4c, 0x24, 0x08, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x48, 0x8d, 0x6c, 0x24, 0xc9, 0x48, 0x81];
    /// The ColorRgb type descriptor (its info: hash 0x885eff59, size 12).
    pub const COLOR_RGB_TYPE: usize = 0x7780c50;
}

pub const DECK_SLOT: &str = "Characters/Customization/Board/Slots/Skateboard_Deck_Slot";
pub const DECK_MAT: &str = "Deck_Mat";
/// The type hash in a ColorRgb descriptor's info.
pub const COLOR_RGB_HASH: u32 = 0x885eff59;

/// The deck shader's parameters by their GUID, as the appearance presets carry them (`tests/preset_params.rs` dumps a
/// preset's), with the names Caveman uses for them: (GUID, name, what it is).
pub const KNOWN: &[(&str, &str, &str)] = &[
    ("36e50d59-48af-873f-59ff-5e88010042d4", "tint", "param_graphic_tintcolor: the graphic's tint (ColorRgb; 0.5 0.5 0.5 is neutral)"),
    ("8d102968-b0e2-0a94-59ff-5e880100facd", "holotint", "the holo's colour (ColorRgb; the holo host's 0.64 1.0 .., Carpet Co's gold 0.67 0.50 0.17)"),
    (
        "6f007e99-9dc5-d442-2d5c-b62b01007daa",
        "iridescence",
        "param_glass_iridescenceamount (float; 0.25 on the holo host, 1.0 on The Caveman and the strong stock decks)",
    ),
    ("568ec558-64fd-0507-2d5c-b62b0100fa45", "iridescence2", "a second iridescence value (float; 0.25 on the holo host, 0 on the stock holo decks)"),
    ("a2189044-0676-ba57-2d5c-b62b0100a211", "metal", "param_graphic_metalmaskvalue (float; 1.0 on the metal and holo hosts)"),
    ("3a957fd1-02b6-b993-7e81-4a8e0100a9aa", "foilmode", "param_foilmode (the pattern: Cellular 0, Frost 1, Pearl 2, Glitter 3, Vertical 4)"),
    ("a3f199b4-fb9f-ebef-d5e9-362a0100dd4d", "pair", "two values, 1 1 on the holo host (a scale?)"),
    ("c10f9af0-3a4b-cfad-4434-ed36010029a0", "zero", "a value of 0 on every deck preset"),
    ("2259d419-387f-b1ba-30fc-c5140100b012", "art", "param_graphic_c (texture)"),
    ("c1b71b8a-395f-e8ce-30fc-c5140100a381", "osk", "param_graphic_osk (texture)"),
    ("830b93ff-7cad-e8ee-30fc-c5140100c668", "ny", "param_graphic_ny (texture)"),
    ("698a46f9-c8de-d48e-30fc-c5140100a381", "rgb", "param_graphic_rgb (texture, the holo map)"),
];

/// A GUID's 16 bytes as the preset serializes them: the first three groups little-endian, the rest in order.
pub fn guid_bytes(text: &str) -> Option<[u8; 16]> {
    let hex: String = text.trim().chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    out[..4].reverse();
    out[4..6].reverse();
    out[6..8].reverse();
    Some(out)
}

/// A parameter's native key and identity from its GUID: the first eight serialized bytes, and the last two.
pub fn key_of_guid(text: &str) -> Option<(u64, u16)> {
    let b = guid_bytes(text)?;
    Some((u64::from_le_bytes(b[..8].try_into().unwrap()), u16::from_le_bytes([b[14], b[15]])))
}

/// A key as its GUID's text starts ("36e50d59-48af-873f").
pub fn guid8(key: u64) -> String {
    let b = key.to_le_bytes();
    format!("{:08x}-{:04x}-{:04x}", u32::from_le_bytes([b[0], b[1], b[2], b[3]]), u16::from_le_bytes([b[4], b[5]]), u16::from_le_bytes([b[6], b[7]]))
}

/// Caveman's name for a key, if [`KNOWN`] has it.
pub fn name_of(key: u64) -> Option<&'static str> {
    KNOWN.iter().find(|(g, ..)| key_of_guid(g).map(|k| k.0) == Some(key)).map(|(_, n, _)| *n)
}

/// A key from a name of [`KNOWN`], a GUID, a GUID's first three groups, or 0x hex.
pub fn key_of(text: &str) -> Result<u64, String> {
    let t = text.trim();
    if let Some((g, ..)) = KNOWN.iter().find(|(_, n, _)| n.eq_ignore_ascii_case(t)) {
        return key_of_guid(g).map(|k| k.0).ok_or_else(|| "a bad GUID in KNOWN".to_string());
    }
    if let Some(h) = t.strip_prefix("0x") {
        return u64::from_str_radix(h, 16).map_err(|_| format!("key {t:?}: 0x then up to 16 hex digits"));
    }
    if let Some(k) = key_of_guid(t) {
        return Ok(k.0);
    }
    let groups: Vec<&str> = t.split('-').collect();
    if groups.len() == 3 && groups[0].len() == 8 && groups[1].len() == 4 && groups[2].len() == 4 {
        return key_of_guid(&format!("{t}-0000-000000000000")).map(|k| k.0).ok_or_else(|| format!("key {t:?}: hex"));
    }
    Err(format!("key {t:?}: a name ({}), a GUID, its first three groups, or 0x hex", KNOWN.iter().map(|(_, n, _)| *n).collect::<Vec<_>>().join(", ")))
}

/// The hash of a material's name as the item's named-material map keys it (djb2 with xor, lower case).
pub fn material_hash(name: &str) -> u32 {
    let mut h: u32 = 5381;
    for c in name.bytes() {
        h = h.wrapping_mul(33) ^ c.to_ascii_lowercase() as u32;
    }
    h
}

/// One parameter node of a material instance.
#[derive(Clone, Debug)]
pub struct Param {
    pub node: usize,
    pub key: u64,
    pub identity: u16,
    pub size: u16,
    pub type_index: u16,
    pub count: u16,
    pub flags: u32,
    pub type_ptr: usize,
    pub type_hash: u32,
    pub type_size: u16,
    /// The inline value (size x count bytes, at most 16).
    pub value: Vec<u8>,
}

/// One `Deck_Mat` instance (the item keeps up to four variants).
#[derive(Clone, Debug)]
pub struct Material {
    pub address: usize,
    pub variant: usize,
    pub params: Vec<Param>,
}

/// The ridden board's deck materials, found afresh by one walk.
#[derive(Clone, Debug)]
pub struct Deck {
    pub entity: usize,
    pub appearance: usize,
    /// The deck slot's item controller: what the publish takes.
    pub item: usize,
    /// A recipe is on its way (the customization component says so): what is found may be replaced.
    pub pending: bool,
    pub materials: Vec<Material>,
}

fn ptr(a: usize) -> Option<usize> {
    win::read_ptr(a).filter(|p| *p >= win::LOWEST)
}

/// The ridden board's `Deck_Mat` instances and their parameters.
pub fn walk(base: usize, ctx: usize) -> Result<Deck, String> {
    if base == 0 {
        return Err("no game".into());
    }
    if ctx == 0 {
        return Err("no client world (menu or loading)".into());
    }
    let entity = scene::find_board_entity(base, ctx).map_err(|e| e.to_string())?;
    let appearance = scene::component(entity, base + rva::SKATER_APPEARANCE_VTABLE).ok_or("the board has no appearance component")?;
    let pending = match scene::component(entity, base + rva::COSMETIC_COMPONENT_VTABLE) {
        Some(c) => win::read_u8(c + 0x10a).unwrap_or(0) != 0 || win::read_u8(c + 0x10d).unwrap_or(0) != 0,
        None => false,
    };
    let (begin, end, cap) =
        (win::read_ptr(appearance + 0x90).unwrap_or(0), win::read_ptr(appearance + 0x98).unwrap_or(0), win::read_ptr(appearance + 0xa0).unwrap_or(0));
    if begin > end || end > cap || (end - begin) % 8 != 0 || (cap - begin) / 8 > 64 || (begin == 0 && cap != 0) {
        return Err("the board's controller array is not as expected".into());
    }
    let mut item = 0usize;
    let mut entry = begin;
    while entry < end {
        if let Some(it) = ptr(entry) {
            if win::read_ptr(it + 0x48) == Some(appearance + 0x40) {
                if let Some(name) = ptr(it + 0x30).and_then(|p| win::read_cstr(p, 160)) {
                    if name == DECK_SLOT {
                        if item != 0 {
                            return Err("the board has two deck slot controllers".into());
                        }
                        item = it;
                    }
                }
            }
        }
        entry += 8;
    }
    if item == 0 {
        return Err("the board has no deck slot controller yet (loading?)".into());
    }
    if win::read_u32(item + 0xb0).unwrap_or(0) & 2 == 0 {
        return Err("the deck is still on its way (its slot is not loaded)".into());
    }
    let buckets = ptr(item + 0x1f0).ok_or("the deck slot has no material map")?;
    let size = win::read_u32(item + 0x1f8).unwrap_or(0) as usize;
    let total = win::read_u32(item + 0x1fc).unwrap_or(0) as usize;
    if size == 0 || size > 256 || total == 0 || total > 128 {
        return Err(format!("the deck's material map is not as expected ({size} buckets, {total} materials)"));
    }
    let sentinel = win::read_ptr(buckets + size * 8).unwrap_or(0);
    let hash = material_hash(DECK_MAT);
    let mut node = win::read_ptr(buckets + (hash as usize % size) * 8).unwrap_or(0);
    let mut visited = 0;
    let mut found = 0usize;
    while node != 0 && node != sentinel && visited < total {
        if win::read_u32(node) == Some(hash) {
            found = node;
            break;
        }
        visited += 1;
        node = win::read_ptr(node + 0x28).unwrap_or(0);
    }
    if found == 0 {
        return Err(format!("the deck has no {DECK_MAT} among its {total} material(s)"));
    }
    let mut materials = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    for v in 0..4 {
        let Some(m) = ptr(found + 8 + v * 8) else { continue };
        if seen.contains(&m) {
            continue;
        }
        seen.push(m);
        materials.push(Material { address: m, variant: v, params: params_of(base, m)? });
    }
    if materials.is_empty() {
        return Err(format!("{DECK_MAT} has no material instance"));
    }
    Ok(Deck { entity, appearance, item, pending, materials })
}

/// Every parameter node of a material instance.
fn params_of(base: usize, material: usize) -> Result<Vec<Param>, String> {
    let buckets = ptr(material + 0x268).ok_or_else(|| format!("material {material:#x}: no parameter map"))?;
    let size = win::read_u32(material + 0x270).unwrap_or(0) as usize;
    let total = win::read_u32(material + 0x274).unwrap_or(0) as usize;
    if size == 0 || size > 1024 || total > 512 {
        return Err(format!("material {material:#x}: a parameter map of {size} buckets and {total} entries is not as expected"));
    }
    let stop = win::read_ptr(buckets + size * 8).unwrap_or(0);
    let mut out = Vec::new();
    let mut visited = 0;
    for bucket in 0..size {
        let mut node = win::read_ptr(buckets + bucket * 8).unwrap_or(0);
        while node != 0 && node != stop {
            let key = win::read_u64(node + 8).unwrap_or(0);
            if (key % size as u64) as usize != bucket {
                // the next bucket's
                break;
            }
            if visited >= total {
                return Err(format!("material {material:#x}: the parameter chain exceeds its count ({total})"));
            }
            visited += 1;
            out.push(read_param(base, node, key));
            node = win::read_ptr(node + 0x40).unwrap_or(0);
        }
    }
    Ok(out)
}

fn read_param(base: usize, node: usize, key: u64) -> Param {
    let mut head = [0u8; 0x30];
    let _ = win::read_into(node, &mut head);
    let u16_at = |o: usize| u16::from_le_bytes([head[o], head[o + 1]]);
    let (identity, size, type_index, count) = (u16_at(0), u16_at(2), u16_at(4), u16_at(6));
    let type_ptr = usize::from_le_bytes(head[0x20..0x28].try_into().unwrap());
    let flags = u32::from_le_bytes(head[0x28..0x2c].try_into().unwrap());
    let n = (size as usize).saturating_mul(count.max(1) as usize).min(16);
    let value = head[0x10..0x10 + n].to_vec();
    let (type_hash, type_size) = if type_ptr >= base && type_ptr < base + 0x0914_4000 {
        let info = win::read_ptr(type_ptr).unwrap_or(0);
        (win::read_u32(info).unwrap_or(0), win::read_u16(info + 6).unwrap_or(0))
    } else {
        (0, 0)
    };
    Param { node, key, identity, size, type_index, count, flags, type_ptr, type_hash, type_size, value }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn param_json(p: &Param) -> Value {
    let floats: Vec<f32> = p.value.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
    json!({
        "node": format!("{:#x}", p.node),
        "key": format!("{:#018x}", p.key),
        "guid8": guid8(p.key),
        "name": name_of(p.key),
        "identity": p.identity,
        "size": p.size,
        "type": p.type_index,
        "count": p.count,
        "flags": p.flags,
        "type_ptr": format!("{:#x}", p.type_ptr),
        "type_hash": format!("{:#010x}", p.type_hash),
        "type_size": p.type_size,
        "color": p.type_hash == COLOR_RGB_HASH,
        "value_hex": hex(&p.value),
        "floats": floats,
        "int": (p.value.len() >= 4).then(|| i32::from_le_bytes(p.value[..4].try_into().unwrap())),
    })
}

pub fn deck_json(d: &Deck) -> Value {
    json!({
        "entity": format!("{:#x}", d.entity),
        "appearance": format!("{:#x}", d.appearance),
        "item": format!("{:#x}", d.item),
        "pending": d.pending,
        "materials": d.materials.iter().map(|m| json!({
            "address": format!("{:#x}", m.address),
            "variant": m.variant,
            "params": m.params.iter().map(param_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Writes `bytes` as the value of the parameter `key` on every `Deck_Mat` instance that has it (the sizes must agree)
/// and marks those materials changed. The nodes written.
pub fn set(deck: &Deck, key: u64, bytes: &[u8]) -> Result<Vec<usize>, String> {
    let mut written = Vec::new();
    for m in &deck.materials {
        for p in m.params.iter().filter(|p| p.key == key) {
            let want = p.size as usize * p.count.max(1) as usize;
            if want != bytes.len() {
                return Err(format!("{}: the parameter is {want} byte(s) (size {} x {}), not {}", guid8(key), p.size, p.count, bytes.len()));
            }
            if want > 16 {
                return Err(format!("{}: a parameter of {want} bytes is not held inline: not written", guid8(key)));
            }
            if !win::write_into(p.node + 0x10, bytes) {
                return Err(format!("node {:#x}: not writable", p.node));
            }
            if !win::write_into(m.address + 0x2f6, &[1]) {
                return Err(format!("material {:#x}: its changed flag is not writable", m.address));
            }
            written.push(p.node);
        }
    }
    if written.is_empty() {
        return Err(format!("{}: no {DECK_MAT} instance has that parameter", guid8(key)));
    }
    Ok(written)
}

/// The game's own publish of an item's materials to its meshes, after its code is checked against the known build.
pub fn publish(base: usize, item: usize) -> Result<(), String> {
    let mut prefix = [0u8; 24];
    if !win::read_into(base + rva::PUBLISH_MATERIALS, &mut prefix) || prefix != rva::PUBLISH_MATERIALS_PREFIX {
        return Err("the game's material publish is not the known build's: not called".into());
    }
    if win::read_ptr(item).is_none() {
        return Err(format!("item {item:#x}: unreadable"));
    }
    // SAFETY: the function's first bytes are the known build's; it takes (item, map index) and returns nothing.
    let f: extern "system" fn(usize, i32) = unsafe { std::mem::transmute(base + rva::PUBLISH_MATERIALS) };
    f(item, 0);
    Ok(())
}

/// The publish job for the engine thread: the item, and how many publishes are still owed.
static JOB_ITEM: AtomicUsize = AtomicUsize::new(0);
static JOB_LEFT: AtomicU8 = AtomicU8::new(0);
/// Publishes done on the engine thread so far, and failures.
pub static PUBLISHED: AtomicU64 = AtomicU64::new(0);
pub static PUBLISH_FAILED: AtomicU64 = AtomicU64::new(0);
/// How many times a value that stands still is published again (the renderer shows a publish one late).
pub const SETTLE_PUBLISHES: u8 = 3;

/// The engine thread's turn (tick.rs, right after the game's own tick): one publish per tick while any is owed.
/// Never called from the DLL's own threads: the publish needs the engine thread's context.
pub fn engine_tick() {
    let left = JOB_LEFT.load(Ordering::Acquire);
    if left == 0 {
        return;
    }
    let item = JOB_ITEM.load(Ordering::Acquire);
    let base = scene::EXE_BASE.load(Ordering::Acquire);
    if item == 0 || base == 0 {
        JOB_LEFT.store(0, Ordering::Release);
        return;
    }
    match publish(base, item) {
        Ok(()) => {
            PUBLISHED.fetch_add(1, Ordering::AcqRel);
            JOB_LEFT.store(left - 1, Ordering::Release);
        }
        Err(_) => {
            PUBLISH_FAILED.fetch_add(1, Ordering::AcqRel);
            JOB_LEFT.store(0, Ordering::Release);
        }
    }
}

/// `Request::Material`: the dump, or one parameter written (floats, an int, or raw hex bytes) and published.
#[allow(clippy::too_many_arguments)]
pub fn handle(
    base: usize,
    ctx: usize,
    key: Option<&str>,
    values: Option<&[f32]>,
    int: Option<i64>,
    raw: Option<&str>,
    publish_after: bool,
    now: u64,
) -> Result<Value, String> {
    let deck = walk(base, ctx)?;
    let mut out = deck_json(&deck);
    let Some(k) = key else { return Ok(out) };
    let key = key_of(k)?;
    let bytes: Vec<u8> = if let Some(r) = raw {
        let r: String = r.chars().filter(|c| !c.is_whitespace()).collect();
        if !r.len().is_multiple_of(2) {
            return Err("--raw: an even number of hex digits".into());
        }
        (0..r.len() / 2).map(|i| u8::from_str_radix(&r[i * 2..i * 2 + 2], 16).map_err(|_| "--raw: hex digits".to_string())).collect::<Result<_, _>>()?
    } else if let Some(i) = int {
        (i as i32).to_le_bytes().to_vec()
    } else if let Some(v) = values.filter(|v| !v.is_empty()) {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    } else {
        return Err("a value: floats, --int N or --raw HEX".into());
    };
    let written = set(&deck, key, &bytes)?;
    let mut published = false;
    let mut publish_note = String::new();
    if publish_after {
        // the publish runs on the engine thread (tick.rs), never here: queue it and wait for the first one
        match unsafe { crate::tick::install(base) } {
            Ok(n) => publish_note = n,
            Err(e) => return Err(format!("written, but not published: the engine-thread hook: {e}")),
        }
        let before = PUBLISHED.load(Ordering::Acquire);
        let failed_before = PUBLISH_FAILED.load(Ordering::Acquire);
        JOB_ITEM.store(deck.item, Ordering::Release);
        JOB_LEFT.store(1 + SETTLE_PUBLISHES, Ordering::Release);
        let t0 = win::now_ms();
        while win::now_ms().saturating_sub(t0) < 1500 {
            if PUBLISHED.load(Ordering::Acquire) > before {
                published = true;
                break;
            }
            if PUBLISH_FAILED.load(Ordering::Acquire) > failed_before {
                break;
            }
            // SAFETY: a plain sleep
            unsafe { win::Sleep(5) };
        }
        if !published {
            publish_note = format!(
                "{publish_note}; no publish within {} ms (tick calls {}, failures {})",
                win::now_ms().saturating_sub(t0),
                crate::tick::CALLS.load(Ordering::Relaxed),
                PUBLISH_FAILED.load(Ordering::Acquire) - failed_before
            );
        }
    }
    let after = walk(base, ctx)?;
    out["written"] = json!(written.iter().map(|n| format!("{n:#x}")).collect::<Vec<_>>());
    out["published"] = json!(published);
    out["publish_note"] = json!(publish_note);
    out["tick"] = crate::tick::status();
    out["waited_ms"] = json!(win::now_ms().saturating_sub(now));
    out["after"] = json!(after.materials.iter().flat_map(|m| m.params.iter().filter(|p| p.key == key).map(param_json)).collect::<Vec<_>>());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_reskates() {
        // ReSkate's developer_board_material.h: graphic_color 0x873f48af36e50d59 with identity 0xd442, base_color
        // 0x8c46cd354b18a9e2; the identity is the GUID's last two serialized bytes
        assert_eq!(key_of_guid("36e50d59-48af-873f-59ff-5e88010042d4"), Some((0x873f48af36e50d59, 0xd442)));
        assert_eq!(key_of("tint"), Ok(0x873f48af36e50d59));
        assert_eq!(key_of("36e50d59-48af-873f"), Ok(0x873f48af36e50d59));
        assert_eq!(key_of("0x873f48af36e50d59"), Ok(0x873f48af36e50d59));
        assert_eq!(guid8(0x873f48af36e50d59), "36e50d59-48af-873f");
        assert_eq!(name_of(0x873f48af36e50d59), Some("tint"));
        assert_eq!(key_of_guid("6f007e99-9dc5-d442-2d5c-b62b01007daa").map(|k| k.0), Some(0xd4429dc56f007e99));
        assert!(key_of("nothing").is_err());
    }

    #[test]
    fn material_hash_is_lower_case_djb2_xor() {
        assert_eq!(material_hash("Deck_Mat"), material_hash("deck_mat"));
        let mut h: u32 = 5381;
        for c in b"deck_mat" {
            h = h.wrapping_mul(33) ^ *c as u32;
        }
        assert_eq!(material_hash("Deck_Mat"), h);
    }
}
