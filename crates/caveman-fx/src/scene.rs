//! What the overlay needs from the game every frame: the camera (world matrix and field of view) and the local
//! skater's board (world matrix), read with safe reads on the render thread just before the frame is presented.
//!
//! Ported from ReSkate's own code (GPL-3.0, as Caveman's ported code is), build 20260929:
//!
//! - **The camera**, as ReSkate's nametags place names over the world (`Extension/Skater/source_spawn_camera.cpp`,
//!   `publish_local_camera_view`; addresses in `Engine/Game/Build/20260929/client_source_spawn.h` and `engine.h`):
//!   the camera view manager (pointer at exe+0x75cbc48) lists the views (+8 .. +0x10); the local view has zeros at
//!   +0x148 and +0x14c; its active camera (+0x2b0) is the game's camera or the free camera, whose world matrix (rows:
//!   right, up, back, position) is at +0x50 and its vertical field of view in degrees at +0xac.
//! - **The board**, as ReSkate's multiplayer captures the local skater (`Extension/Multiplayer/Remote/native_skater.cpp`
//!   `capture_local`, `native_pose_layout.h` `read_native_board`): the client context (a game thread's TLS world,
//!   type 0xbf0f9789) -> its local player manager -> the one local player -> the skater entity (+0xb8) -> its skater
//!   animation component (in the entity's component collection +0x70) -> the board component (+0x60) -> the
//!   ClientSkateboardEntity; an entity's world transform sits in its component collection at
//!   `0x10 + (first + 2 * extra) * 0x20` (rows: right, up, forward, position).
//!
//! Every pointer is checked against the class's vtable before it is followed; any mismatch reads as "no scene" and
//! nothing is drawn. Verified live (read only, skwatch) on 2026-10-21 in a menu: the chain resolves; the skater at
//! (627.2, 199.9, 1062.9) and its board beside it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::bars::{Board, Camera, Scene};
use crate::win;

/// RVAs of build 20260929 (from ReSkate's build tables).
pub mod rva {
    /// u32: the offset of the type record in a game context.
    pub const CONTEXT_TYPE_OFFSET: usize = 0x6fc24b8;
    /// u32: the offset of the owning client in a game context.
    pub const CONTEXT_CLIENT_OFFSET: usize = 0x6fc25f0;
    /// u32: the offset of the player manager in a game context.
    pub const CONTEXT_PLAYER_MANAGER_OFFSET: usize = 0x7161720;
    /// Pointer: the camera view manager.
    pub const VIEW_MANAGER: usize = 0x75cbc48;
    pub const CLIENT_VTABLE: usize = 0x60842c0;
    pub const LOCAL_PLAYER_MANAGER_VTABLE: usize = 0x63e30a0;
    pub const LOCAL_PLAYER_VTABLE: usize = 0x63e2d30;
    pub const SKATER_ENTITY_VTABLE: usize = 0x6087908;
    pub const SKATER_COMPONENT_VTABLE: usize = 0x6170410;
    pub const BOARD_COMPONENT_VTABLE: usize = 0x6170e38;
    pub const BOARD_ENTITY_VTABLE: usize = 0x6171808;
    pub const VIEW_MANAGER_VTABLE: usize = 0x63ece68;
    pub const CAMERA_VIEW_VTABLE: usize = 0x63ed0a0;
    /// The game's own camera, and the free camera (noclip, ReSkate's freecam and spectate).
    pub const CAMERA_VTABLE: usize = 0x6255870;
    pub const FREE_CAMERA_VTABLE: usize = 0x63fc220;
    /// The board's animation resource holder (`board_holder_vtable`) and the animation definition's bone-count getter
    /// (`animation_count_getter`), from ReSkate's `engine.h`: the rig's pose buffer is found through them.
    pub const BOARD_HOLDER_VTABLE: usize = 0x63db690;
    pub const ANIMATION_COUNT_GETTER: usize = 0x11e5040;
}

/// One bone of a pose buffer as the animation system lays it out (ReSkate's `NativeBone`, 0x30 bytes): scale,
/// rotation (x, y, z, w) and position, each padded to four floats. Each bone is in its parent's frame (the skeleton
/// asset's hierarchy: `characters/_prototype/skateboard/skateboard_default_skeleton`, 16 bones).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoneLane {
    pub scale: [f32; 4],
    pub rotation: [f32; 4],
    pub position: [f32; 4],
}

/// The local board's pose buffer: every bone's lane, read through the board entity's animation holder (entity+0xf0),
/// as ReSkate's `read_native_pose_layout` does (`Extension/Multiplayer/Remote/native_pose_layout.h`): holder -> rig
/// (+0x78) -> definition (+0x18, whose class has the known bone-count getter) -> skeleton resource (+0x1a0, the bone
/// count at +0xc) and the rig's output header (+0x20) -> a tagged offset table -> the pose's address. At most `bound`
/// bones.
pub fn read_bone_lanes(base: usize, entity: usize, bound: usize) -> Result<Vec<BoneLane>, &'static str> {
    let holder = ptr(entity + 0xf0).ok_or("the board has no animation holder")?;
    if !is(holder, base + rva::BOARD_HOLDER_VTABLE) {
        return Err("the board's animation holder is not the known class");
    }
    let rig = ptr(holder + 0x78).ok_or("the board has no rig")?;
    let definition = ptr(rig + 0x18).ok_or("the rig has no definition")?;
    let table_v = ptr(definition).ok_or("the definition has no class")?;
    if win::read_ptr(table_v + 0x78) != Some(base + rva::ANIMATION_COUNT_GETTER) {
        return Err("the animation definition's class is not the known one");
    }
    let resource = ptr(definition + 0x1a0).ok_or("the definition has no skeleton")?;
    let count = win::read_u32(resource + 0xc).ok_or("the skeleton's bone count is unreadable")? as usize;
    if count == 0 || count > bound {
        return Err("the skeleton's bone count is not usable");
    }
    let header = ptr(rig + 0x20).ok_or("the rig has no output header")?;
    let tagged = win::read_u64(header + 0x10).ok_or("the offset table is unreadable")?;
    let tag = tagged >> 60;
    if tag > 1 {
        return Err("the offset table's tag is not one this reader knows");
    }
    let low = ((tagged << 4) as i64) >> 4;
    let table = if tag == 1 { (header as i64 + 0x10 + low) as usize } else { low as usize };
    if !(win::LOWEST..=win::HIGHEST).contains(&table) {
        return Err("the offset table is out of range");
    }
    let index = win::read_u32(table).ok_or("the table index is unreadable")? as i32;
    if !(-4096..=4096).contains(&index) {
        return Err("the table index is out of range");
    }
    let relative = (10 - index as i64 * 8) * 4;
    let offset = win::read_u32((table as i64 + relative) as usize).ok_or("the pose offset is unreadable")? as usize;
    let start = win::read_u16(header + 0x1c).ok_or("the data offset is unreadable")? as usize;
    let address = if start != 0 { header + start + offset } else { offset };
    if address < win::LOWEST || address > win::HIGHEST - count * 0x30 {
        return Err("the pose address is out of range");
    }
    let mut raw = vec![0u8; count * 0x30];
    if !win::read_into(address, &mut raw) {
        return Err("the pose is unreadable");
    }
    let f = |o: usize| f32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
    let mut lanes = Vec::with_capacity(count);
    for b in 0..count {
        let o = b * 0x30;
        let lane = BoneLane {
            scale: [f(o), f(o + 4), f(o + 8), f(o + 12)],
            rotation: [f(o + 16), f(o + 20), f(o + 24), f(o + 28)],
            position: [f(o + 32), f(o + 36), f(o + 40), f(o + 44)],
        };
        // (the fourth lane of the scale and the position is padding: whatever the game leaves there)
        if !lane.scale[..3].iter().chain(&lane.rotation).chain(&lane.position[..3]).all(|v| v.is_finite()) {
            return Err("the pose is not finite");
        }
        lanes.push(lane);
    }
    Ok(lanes)
}

impl Reader {
    /// The local board's bone lanes now (the board's chain as last resolved by `read`), or why not.
    pub fn bones(&self, base: usize, bound: usize) -> Result<Vec<BoneLane>, &'static str> {
        let (_, entity, ..) = self.board.ok_or("no board")?;
        read_bone_lanes(base, entity, bound)
    }

    /// The board entity as last resolved (for probes).
    pub fn board_entity(&self) -> Option<usize> {
        self.board.map(|(_, e, ..)| e)
    }
}

/// The board rig's skeleton resource (the runtime object the animation definition points at, whose bone count is at
/// +0xc) and `len` bytes of it, for a look at the runtime bone order (its name hashes).
pub fn read_skeleton_resource(base: usize, entity: usize, len: usize) -> Result<(usize, Vec<u8>), &'static str> {
    let holder = ptr(entity + 0xf0).ok_or("the board has no animation holder")?;
    if !is(holder, base + rva::BOARD_HOLDER_VTABLE) {
        return Err("the board's animation holder is not the known class");
    }
    let rig = ptr(holder + 0x78).ok_or("the board has no rig")?;
    let definition = ptr(rig + 0x18).ok_or("the rig has no definition")?;
    let resource = ptr(definition + 0x1a0).ok_or("the definition has no skeleton")?;
    let mut raw = vec![0u8; len];
    if !win::read_into(resource, &mut raw) {
        return Err("the skeleton resource is unreadable");
    }
    Ok((resource, raw))
}

/// The type record of a client game context.
pub const CLIENT_CONTEXT_TYPE: u32 = 0xbf0f9789;

/// Skate.exe's base, set by the worker once the build is verified (0: no game: nothing is read).
pub static EXE_BASE: AtomicUsize = AtomicUsize::new(0);
/// The client game context, set by the worker from the worlds it sees (0: none: menus, loading).
pub static CLIENT_CTX: AtomicUsize = AtomicUsize::new(0);
/// A scene given by a test host instead of the game's (never in the game: the export refuses there).
pub static TEST_SCENE: Mutex<Option<Scene>> = Mutex::new(None);

/// Whether a test host gave a scene (then the game's transforms are not read at all).
pub fn test_scene_active() -> bool {
    TEST_SCENE.lock().map(|t| t.is_some()).unwrap_or(false)
}

fn ptr(addr: usize) -> Option<usize> {
    win::read_ptr(addr).filter(|&p| (win::LOWEST..=win::HIGHEST).contains(&p) && p % 8 == 0)
}

fn is(addr: usize, vtable: usize) -> bool {
    win::read_ptr(addr) == Some(vtable)
}

/// Whether `ctx` is a client game context (its type record).
pub fn is_client_context(base: usize, ctx: usize) -> bool {
    let Some(off) = win::read_u32(base + rva::CONTEXT_TYPE_OFFSET) else { return false };
    (8..=0x100_0000).contains(&off) && win::read_u32(ctx + off as usize) == Some(CLIENT_CONTEXT_TYPE)
}

/// Picks the client context among the worlds the worker sees and publishes it.
pub fn publish_worlds(base: usize, worlds: &[(usize, u32)]) {
    let ctx = worlds.iter().map(|(w, _)| *w).find(|&w| is_client_context(base, w)).unwrap_or(0);
    CLIENT_CTX.store(ctx, Ordering::Release);
}

/// The active camera of the local view.
pub fn read_camera(base: usize) -> Result<Camera, &'static str> {
    let camera = camera_object(base)?;
    let mut raw = [0u8; 0xb0];
    if !win::read_into(camera, &mut raw) {
        return Err("the camera is unreadable");
    }
    let vt = usize::from_le_bytes(raw[..8].try_into().unwrap());
    if vt != base + rva::CAMERA_VTABLE && vt != base + rva::FREE_CAMERA_VTABLE {
        return Err("the active camera is of an unknown kind");
    }
    let mut m = [0f32; 16];
    for (k, v) in m.iter_mut().enumerate() {
        *v = f32::from_le_bytes(raw[0x50 + k * 4..0x54 + k * 4].try_into().unwrap());
    }
    let fov = f32::from_le_bytes(raw[0xac..0xb0].try_into().unwrap());
    Camera::from_matrix(&m, fov).ok_or("the camera's matrix or field of view is not usable")
}

/// The local view's active camera object (its world matrix at +0x50, its vertical field of view at +0xac).
pub fn camera_object(base: usize) -> Result<usize, &'static str> {
    let vm = ptr(base + rva::VIEW_MANAGER).ok_or("no camera view manager")?;
    if !is(vm, base + rva::VIEW_MANAGER_VTABLE) {
        return Err("the camera view manager is not the known class");
    }
    let mut range = [0u8; 16];
    if !win::read_into(vm + 8, &mut range) {
        return Err("the camera views are unreadable");
    }
    let begin = usize::from_le_bytes(range[..8].try_into().unwrap());
    let end = usize::from_le_bytes(range[8..].try_into().unwrap());
    if end < begin || (end - begin) % 8 != 0 || (end - begin) / 8 > 32 {
        return Err("the camera view list is odd");
    }
    let mut local = None;
    for i in 0..(end - begin) / 8 {
        let Some(view) = ptr(begin + i * 8) else { continue };
        if !is(view, base + rva::CAMERA_VIEW_VTABLE) {
            continue;
        }
        let mut ids = [0u8; 8];
        if win::read_into(view + 0x148, &mut ids) && ids == [0; 8] {
            if local.is_some() {
                return Err("two local camera views");
            }
            local = Some(view);
        }
    }
    let view = local.ok_or("no local camera view")?;
    ptr(view + 0x2b0).ok_or("the local view has no camera")
}

/// The component of an entity whose vtable is `vtable` (its component collection at +0x70).
pub fn component(entity: usize, vtable: usize) -> Option<usize> {
    let collection = ptr(entity + 0x70)?;
    if win::read_ptr(collection)? != entity {
        return None;
    }
    let count = win::read_u8(collection + 8)? as usize;
    if count > 128 {
        return None;
    }
    let mut found = None;
    for i in 0..count {
        let Some(c) = ptr(collection + 0x20 + i * 0x20) else { continue };
        if is(c, vtable) {
            if found.is_some() || win::read_ptr(c + 0x18)? != collection {
                return None;
            }
            found = Some(c);
        }
    }
    found
}

/// Where an entity's world transform is: (its component collection, the matrix's offset in it).
pub fn transform_location(entity: usize) -> Option<(usize, usize)> {
    let collection = ptr(entity + 0x70)?;
    let mut head = [0u8; 16];
    if !win::read_into(collection, &mut head) || usize::from_le_bytes(head[..8].try_into().unwrap()) != entity {
        return None;
    }
    let (first, extra) = (head[9] as usize, head[10] as usize);
    (first <= 128 && extra <= 32).then_some((collection, 0x10 + (first + 2 * extra) * 0x20))
}

/// The local skater's board entity, through the client context.
pub fn find_board_entity(base: usize, ctx: usize) -> Result<usize, &'static str> {
    let client_off = win::read_u32(base + rva::CONTEXT_CLIENT_OFFSET).ok_or("unreadable globals")? as usize;
    let manager_off = win::read_u32(base + rva::CONTEXT_PLAYER_MANAGER_OFFSET).ok_or("unreadable globals")? as usize;
    if !(8..=0x100_0000).contains(&client_off) || manager_off > 0x100_0000 {
        return Err("the context layout is not as expected");
    }
    let client = ptr(ctx + client_off).ok_or("no client")?;
    if !is(client, base + rva::CLIENT_VTABLE) || win::read_ptr(client + 8) != Some(ctx) {
        return Err("the client is not the known class");
    }
    let manager = ptr(ctx + manager_off).ok_or("no local player manager")?;
    if !is(manager, base + rva::LOCAL_PLAYER_MANAGER_VTABLE) {
        return Err("the local player manager is not the known class");
    }
    let (begin, end) = (ptr(manager + 0x4c8).ok_or("no local player")?, ptr(manager + 0x4d0).ok_or("no local player")?);
    if end != begin + 8 {
        return Err("not exactly one local player");
    }
    let player = ptr(begin).ok_or("no local player")?;
    if !is(player, base + rva::LOCAL_PLAYER_VTABLE) || win::read_u8(player + 0x45) != Some(1) || win::read_u8(player + 0x44) != Some(0) {
        return Err("the local player is not a local human player");
    }
    let skater = ptr(player + 0xb8).ok_or("no skater")?;
    if !is(skater, base + rva::SKATER_ENTITY_VTABLE) || win::read_ptr(skater + 0xf8) != Some(player) || win::read_ptr(skater + 0x20) != Some(ctx) {
        return Err("the skater entity is not the local player's");
    }
    let skater_component = component(skater, base + rva::SKATER_COMPONENT_VTABLE).ok_or("no skater animation component")?;
    let board_component = ptr(skater_component + 0x60).ok_or("no board (on foot?)")?;
    if !is(board_component, base + rva::BOARD_COMPONENT_VTABLE) || win::read_ptr(board_component + 0x58) != Some(skater_component) {
        return Err("the board component is not the skater's");
    }
    let collection = ptr(board_component + 0x18).ok_or("no board collection")?;
    let board = ptr(collection).ok_or("no board entity")?;
    if !is(board, base + rva::BOARD_ENTITY_VTABLE) || win::read_ptr(board + 0x20) != Some(ctx) {
        return Err("the board entity is not the known class");
    }
    Ok(board)
}

/// Reads the scene for the overlay, caching the board's chain (re-resolved twice a second or when it fails).
#[derive(Default)]
pub struct Reader {
    board: Option<(usize, usize, usize, usize)>,
    resolved_at: f64,
    /// Why the last read gave no scene, or "ok".
    pub why: &'static str,
}

impl Reader {
    /// The scene now (`now` in seconds), or None (with the reason in `why`).
    pub fn read(&mut self, now: f64) -> Option<Scene> {
        if let Ok(t) = TEST_SCENE.lock() {
            if let Some(s) = *t {
                self.why = "ok (test scene)";
                return Some(s);
            }
        }
        let base = EXE_BASE.load(Ordering::Acquire);
        if base == 0 {
            self.why = "no game";
            return None;
        }
        let camera = match read_camera(base) {
            Ok(c) => c,
            Err(why) => {
                self.why = why;
                return None;
            }
        };
        let board = self.board(base, now);
        self.why = if board.is_some() { "ok" } else { self.why };
        Some(Scene { camera, board })
    }

    fn board(&mut self, base: usize, now: f64) -> Option<Board> {
        let ctx = CLIENT_CTX.load(Ordering::Acquire);
        if ctx == 0 {
            self.board = None;
            self.why = "no client world (menu or loading)";
            return None;
        }
        let stale = self.board.is_none_or(|(c, ..)| c != ctx) || now - self.resolved_at > 0.5;
        if stale {
            self.resolved_at = now;
            self.board = match find_board_entity(base, ctx).map(|e| (e, transform_location(e))) {
                Ok((entity, Some((collection, offset)))) => Some((ctx, entity, collection, offset)),
                Ok((_, None)) => {
                    self.why = "the board's transform is not where expected";
                    None
                }
                Err(why) => {
                    self.why = why;
                    None
                }
            };
        }
        let (_, entity, collection, offset) = self.board?;
        let mut head = [0u8; 8];
        let mut raw = [0u8; 64];
        if !win::read_into(collection, &mut head) || usize::from_le_bytes(head) != entity || !win::read_into(collection + offset, &mut raw) {
            self.board = None;
            self.why = "the board went away";
            return None;
        }
        let mut m = [0f32; 16];
        for (k, v) in m.iter_mut().enumerate() {
            *v = f32::from_le_bytes(raw[k * 4..k * 4 + 4].try_into().unwrap());
        }
        let b = Board::from_matrix(&m);
        if b.is_none() {
            self.why = "the board's matrix is not usable";
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake entity laid out as the game's: its component collection and transform.
    #[test]
    fn transform_location_follows_the_counts() {
        let mut coll = vec![0u64; 0x400];
        let mut entity = vec![0u64; 0x20];
        let e2 = entity.as_ptr() as usize;
        coll[0] = e2 as u64;
        // count 8, first 8, extra 1 (the board's, live)
        coll[1] = 0x0001_0808;
        entity[0x70 / 8] = coll.as_ptr() as u64;
        let (c, off) = transform_location(e2).unwrap();
        assert_eq!(c, coll.as_ptr() as usize);
        assert_eq!(off, 0x10 + 10 * 0x20);
        // the skater's live counts: first 0x27, extra 5 -> 0x630
        coll[1] = 0x0005_2727;
        assert_eq!(transform_location(e2).unwrap().1, 0x630);
        // an owner mismatch is refused
        coll[0] = 0x1234_0000;
        assert!(transform_location(e2).is_none());
    }

    #[test]
    fn no_game_no_scene() {
        let mut r = Reader::default();
        assert!(r.read(0.0).is_none());
        assert_eq!(r.why, "no game");
    }
}
