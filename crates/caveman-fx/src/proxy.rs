//! The frame's own transforms, for the overlay (the fix for the bars' jitter in motion).
//!
//! The game simulates at a fixed rate (30 or 60 Hz, the "Simulation rate" setting) and renders at whatever rate it
//! can (87 frames a second seen). The camera object and the board entity's transform that [`crate::scene`] reads
//! change once a simulation tick, and at different moments (the camera never within 0.7 ms of the board), while each
//! frame is drawn from transforms the renderer interpolates. Read at Present, the simulation's pair mixes ticks and
//! does not match the picture: the bars jitter in motion and show doubled ends; at rest they are clean.
//!
//! The renderer keeps a **transform proxy** per renderable (vtable exe+0x65eff48, about 12,000 in San Van), rewritten
//! once per frame: +0x48 its render object, +0x50 the frame's world matrix (rows: right, up, forward or back,
//! position; the fourth lanes hold other data), +0x90 the previous frame's (for motion vectors). Measured read only
//! (`caveman-trainer probe-timing`, 2026-10-21 13:20, Trevor riding at 88 presents a second): the board's proxy (its
//! render object a skinned mesh of 16 bones, vtable exe+0x65f2910) and the proxies of the renderables that sit on the
//! camera (the camera's rows and position) change exactly once per present, at the same moment, about a third into the
//! frame interval; the simulation's board and camera 60 times a second.
//!
//! So the overlay takes the camera and the board from those proxies: a pair from the same frame. The worker finds
//! them ([`Finder`]: a scan for the proxy vtable, then the proxy nearest the simulation's board whose render object
//! is skinned, and the proxy on the simulation's camera with its rows); the render thread reads them every frame
//! ([`frame_scene`]) and falls back to the simulation's pair, asking for a new search, when one looks wrong.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::bars::{self, Board, Camera, Scene};
use crate::scan::WordScan;
use crate::win;

pub mod rva {
    /// A render transform proxy: +0x48 render object, +0x50 this frame's world matrix, +0x90 the previous frame's.
    pub const PROXY_VTABLE: usize = 0x65eff48;
    /// A skinned render object (ReSkate's `skater_render.h`): its bone count at +0xf8 (u32).
    pub const SKINNED_VTABLE: usize = 0x65f2910;
}

pub const MATRIX: usize = 0x50;
/// The previous frame's world matrix (for motion vectors).
pub const PREVIOUS: usize = 0x90;
pub const OBJECT: usize = 0x48;
const BONES: usize = 0xf8;

/// The proxies in use (0: none).
pub static CAMERA: AtomicUsize = AtomicUsize::new(0);
pub static BOARD: AtomicUsize = AtomicUsize::new(0);
/// The render thread asks the worker to look (again).
pub static WANTED: AtomicBool = AtomicBool::new(true);
/// Searches done, for the status.
pub static SEARCHES: AtomicU32 = AtomicU32::new(0);
/// What the last search found, for the status.
pub static LAST: Mutex<String> = Mutex::new(String::new());

fn note(s: String) {
    if let Ok(mut l) = LAST.lock() {
        *l = s;
    }
}

/// A proxy's frame matrix, if `proxy` still is one and the matrix is sound (read until two reads agree: the game may
/// be writing it).
pub fn read(base: usize, proxy: usize) -> Option<[f32; 16]> {
    if proxy == 0 || win::read_ptr(proxy) != Some(base + rva::PROXY_VTABLE) {
        return None;
    }
    let mut a = [0u8; 64];
    let mut b = [0u8; 64];
    if !win::read_into(proxy + MATRIX, &mut a) {
        return None;
    }
    for _ in 0..3 {
        if !win::read_into(proxy + MATRIX, &mut b) {
            return None;
        }
        if a == b {
            let m: [f32; 16] = std::array::from_fn(|k| f32::from_le_bytes(a[k * 4..k * 4 + 4].try_into().unwrap()));
            return bars::valid_matrix(&m).then_some(m);
        }
        TORN.fetch_add(1, Ordering::Relaxed);
        a = b;
    }
    None
}

/// Reads of a proxy's matrix that caught the game writing it (two reads differed), for the record.
pub static TORN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn pos(m: &[f32; 16]) -> bars::V3 {
    [m[12], m[13], m[14]]
}

fn row(m: &[f32; 16], r: usize) -> bars::V3 {
    [m[r * 4], m[r * 4 + 1], m[r * 4 + 2]]
}

/// How well a matrix's rows agree with three axes (the smallest dot product).
fn agreement(m: &[f32; 16], axes: [bars::V3; 3]) -> f32 {
    (0..3).map(|r| bars::dot(row(m, r), axes[r])).fold(f32::MAX, f32::min)
}

/// Picks the board's proxy (nearest the simulation's board, rows within about 25 degrees, its render object
/// skinned) and the camera's (on the simulation's camera, rows within about 18 degrees) among proxy addresses.
pub fn pick(base: usize, found: &[usize], camera: &Camera, board: &Board) -> (Option<usize>, Option<usize>, usize, usize) {
    let mut best_cam: Option<(f32, usize)> = None;
    let mut best_board: Option<(f32, usize)> = None;
    let (mut cams, mut boards) = (0, 0);
    for &p in found {
        let Some(m) = read(base, p) else { continue };
        let at = pos(&m);
        let dc = bars::length(bars::sub(at, camera.pos));
        if dc < 0.6 && agreement(&m, [camera.right, camera.up, camera.back]) > 0.95 {
            cams += 1;
            if best_cam.is_none_or(|(d, _)| dc < d) {
                best_cam = Some((dc, p));
            }
        }
        let db = bars::length(bars::sub(at, board.pos));
        if db < 0.6 && agreement(&m, [board.right, board.up, board.forward]) > 0.9 {
            let object = win::read_ptr(p + OBJECT).unwrap_or(0);
            let skinned = object != 0 && win::read_ptr(object) == Some(base + rva::SKINNED_VTABLE);
            let bones = if skinned { win::read_u32(object + BONES).unwrap_or(0) } else { 0 };
            if skinned && (1..=64).contains(&bones) {
                boards += 1;
                if best_board.is_none_or(|(d, _)| db < d) {
                    best_board = Some((db, p));
                }
            }
        }
    }
    (best_cam.map(|x| x.1), best_board.map(|x| x.1), cams, boards)
}

/// The worker's side: finds the proxies when asked (and the bars are on). First among the proxies the last full scan
/// found (a few milliseconds: after a pause or a menu the same two are usually still there), then by a full scan.
#[derive(Default)]
pub struct Finder {
    scan: Option<WordScan>,
    not_before: u64,
    started: u64,
    /// Every proxy the last full scan found.
    known: Vec<usize>,
    /// When the known ones were last tried, and when the current search began (0: none).
    quick_at: u64,
    wanted_since: u64,
}

/// How long the known proxies are tried (every quarter second) before a full scan: what dropped them (a camera cut, a
/// respawn) is usually over within it, and the same two are found again in milliseconds.
const QUICK_FOR_MS: u64 = 10_000;

impl Finder {
    /// One worker tick. `scene` is the simulation's scene now (None: no board, nothing to find).
    pub fn tick(&mut self, base: usize, now_ms: u64, scene: Option<Scene>) {
        if self.scan.is_none() {
            if !WANTED.load(Ordering::Acquire) {
                self.wanted_since = 0;
                return;
            }
            if self.wanted_since == 0 {
                self.wanted_since = now_ms;
            }
            if now_ms < self.not_before {
                return;
            }
            let Some(Scene { camera, board: Some(board) }) = scene else { return };
            // the known proxies first, four times a second, for a while
            if !self.known.is_empty() && now_ms >= self.quick_at + 250 {
                self.quick_at = now_ms;
                let t = std::time::Instant::now();
                let (cam, brd, cams, boards) = pick(base, &self.known, &camera, &board);
                if let (Some(c), Some(b)) = (cam, brd) {
                    CAMERA.store(c, Ordering::Release);
                    BOARD.store(b, Ordering::Release);
                    WANTED.store(false, Ordering::Release);
                    QUICK.fetch_add(1, Ordering::Relaxed);
                    note(format!(
                        "found again among the {} known proxies in {} ms: camera {c:#x} ({cams} on the camera), board {b:#x} ({boards} on the board)",
                        self.known.len(),
                        t.elapsed().as_millis()
                    ));
                    return;
                }
            }
            if !self.known.is_empty() && now_ms < self.wanted_since + QUICK_FOR_MS {
                return; // keep trying the known ones
            }
            match WordScan::new(&[(base + rva::PROXY_VTABLE) as u64], 1 << 18) {
                Ok(s) => {
                    self.scan = Some(s);
                    self.started = now_ms;
                }
                Err(e) => {
                    note(format!("cannot scan: {e}"));
                    self.not_before = now_ms + 10_000;
                }
            }
            return;
        }
        let s = self.scan.as_mut().unwrap();
        if !s.step(32 << 20, 4) {
            return;
        }
        let s = self.scan.take().unwrap();
        SEARCHES.fetch_add(1, Ordering::Relaxed);
        self.known = s.found.clone();
        let took = now_ms.saturating_sub(self.started);
        let Some(Scene { camera, board: Some(board) }) = scene else {
            note(format!("{} proxies in {took} ms; no board to match (retrying)", self.known.len()));
            self.not_before = now_ms + 2_000;
            return;
        };
        let (cam, brd, cams, boards) = pick(base, &self.known, &camera, &board);
        CAMERA.store(cam.unwrap_or(0), Ordering::Release);
        BOARD.store(brd.unwrap_or(0), Ordering::Release);
        let ok = cam.is_some() && brd.is_some();
        note(format!(
            "{} proxies ({} MB) in {took} ms: camera {} ({cams} on the camera), board {} ({boards} on the board)",
            self.known.len(),
            s.bytes >> 20,
            cam.map_or("not found".to_string(), |a| format!("{a:#x}")),
            brd.map_or("not found".to_string(), |a| format!("{a:#x}"))
        ));
        if ok {
            WANTED.store(false, Ordering::Release);
        } else {
            self.not_before = now_ms + 3_000;
        }
    }
}

/// Proxies found again among the known ones (no full scan).
pub static QUICK: AtomicU32 = AtomicU32::new(0);
/// Proxies dropped, and why the last time.
pub static LOSSES: AtomicU32 = AtomicU32::new(0);
pub static LOST_WHY: Mutex<&'static str> = Mutex::new("");

/// The render thread's per-frame state: notices when a proxy stops following the simulation.
#[derive(Default)]
pub struct Follow {
    last: Option<([f32; 16], [f32; 16], bars::V3)>,
    /// Frames the board's proxy stood still while the simulation's board moved (the board not drawn: culled), and how
    /// far the board moved meanwhile while on screen.
    frozen: u32,
    frozen_on_screen: f32,
    /// How far the simulation's board travelled (jumps left out) while the proxies were far from it, over how many
    /// frames in a row.
    far_by: f32,
    far_frames: u32,
    /// Why the last frame used the simulation's pair or nothing, or "frame".
    pub why: &'static str,
}

impl Follow {
    /// The proxies' matrices the last frame was drawn with (the camera's, the board's), when it was.
    pub fn drawn(&self) -> Option<([f32; 16], [f32; 16])> {
        self.last.map(|(cm, bm, _)| (cm, bm))
    }
}

/// How far the simulation's board must travel while the proxies stay far from the simulation's pair (and the board's
/// keeps changing; over at least three seconds of frames), or while the board's proxy stands still with the board on
/// screen, before they are dropped and searched for again. A jump of the board (a respawn, a teleport: half a metre or
/// more in one frame) is not travel: the frame's proxies follow it a frame later.
const DROP_AFTER_M: f32 = 2.0;
const DROP_AFTER_FRAMES: u32 = 180;
const DROP_FROZEN_ON_SCREEN_M: f32 = 1.0;
const JUMP_M: f32 = 0.5;

/// What a frame draws with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Framed {
    /// The frame's pair.
    Frame(Scene),
    /// The simulation's pair (no proxies yet, or they are far from it: a pause or a menu).
    Sim(Scene),
    /// Nothing: the board is not drawn this frame (its proxy stands still while the board moves: culled, off screen).
    Hidden,
}

/// Whether the board's origin is on screen for a camera and aspect.
fn on_screen(camera: &Camera, aspect: f32, p: bars::V3) -> bool {
    let c = bars::project(camera, aspect, p);
    c[3] > bars::NEAR && (c[0] / c[3]).abs() < 1.05 && (c[1] / c[3]).abs() < 1.05
}

/// The scene from the frame's proxies when they are found and sound. The field of view always comes from the
/// simulation's camera (the proxies hold no projection).
///
/// Proxies are kept through pauses, menus, camera cuts, respawns and the board going off screen, and dropped (with a
/// search asked for) only when they are gone (no longer proxies) or plainly not the frame's while the skater moves: far
/// from the simulation's pair for three seconds over two metres of the board's travel (jumps left out), or the board's
/// proxy standing still while the board travels a metre on screen. While the board's proxy stands still and the board
/// moves, the board is not being drawn (culled): there is nothing to draw the bars on.
pub fn frame_scene(base: usize, sim: Scene, aspect: f32, follow: &mut Follow) -> Framed {
    let Some(sim_board) = sim.board else {
        follow.why = "no board";
        return Framed::Sim(sim);
    };
    let (cp, bp) = (CAMERA.load(Ordering::Acquire), BOARD.load(Ordering::Acquire));
    if cp == 0 || bp == 0 {
        follow.why = "proxies: searching";
        WANTED.store(true, Ordering::Release);
        return Framed::Sim(sim);
    }
    let lost = |follow: &mut Follow, why: &'static str| {
        follow.why = why;
        follow.last = None;
        follow.frozen = 0;
        follow.frozen_on_screen = 0.0;
        follow.far_by = 0.0;
        follow.far_frames = 0;
        CAMERA.store(0, Ordering::Release);
        BOARD.store(0, Ordering::Release);
        WANTED.store(true, Ordering::Release);
        LOSSES.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut w) = LOST_WHY.lock() {
            *w = why;
        }
    };
    let (Some(cm), Some(bm)) = (read(base, cp), read(base, bp)) else {
        lost(follow, "proxies: gone");
        return Framed::Sim(sim);
    };
    let (Some(camera), Some(board)) = (Camera::from_matrix(&cm, sim.camera.fov_y), Board::from_matrix(&bm)) else {
        lost(follow, "proxies: unusable matrices");
        return Framed::Sim(sim);
    };
    let moved = follow.last.map_or(0.0, |(_, _, lsim)| bars::length(bars::sub(sim_board.pos, lsim)));
    let board_changed = follow.last.is_none_or(|(_, lb, _)| lb != bm);
    follow.last = Some((cm, bm, sim_board.pos));
    // the board's proxy standing still while the board moves: the board is not drawn (culled: dropped off screen)
    if moved > 0.002 && !board_changed {
        follow.frozen += 1;
        if on_screen(&sim.camera, aspect, sim_board.pos) {
            follow.frozen_on_screen += moved;
        }
        if follow.frozen_on_screen > DROP_FROZEN_ON_SCREEN_M {
            lost(follow, "proxies: the board's stood still while the board moved on screen");
            return Framed::Sim(sim);
        }
        if follow.frozen >= 3 {
            follow.why = "proxies: kept (the board is not drawn: off screen?)";
            return Framed::Hidden;
        }
    } else if board_changed {
        follow.frozen = 0;
        follow.frozen_on_screen = 0.0;
    }
    if follow.frozen >= 3 {
        // still frozen, the board not moving this frame: still not drawn
        follow.why = "proxies: kept (the board is not drawn: off screen?)";
        return Framed::Hidden;
    }
    // the frame's pair lags or leads the simulation's by a tick at most: far apart is a menu's or a pause's camera,
    // or another object now (then it stays far while the skater rides on)
    if bars::length(bars::sub(camera.pos, sim.camera.pos)) > 1.5 || bars::length(bars::sub(board.pos, sim_board.pos)) > 1.5 {
        follow.far_frames += 1;
        if moved < JUMP_M {
            follow.far_by += moved;
        }
        if follow.far_frames > DROP_AFTER_FRAMES && follow.far_by > DROP_AFTER_M {
            lost(follow, "proxies: far from the simulation's camera or board for 3 s while the board travelled 2 m");
            return Framed::Sim(sim);
        }
        follow.why = "proxies: kept (far from the simulation's pair: a menu, a pause, a camera cut?)";
        return Framed::Sim(sim);
    }
    follow.far_by = 0.0;
    follow.far_frames = 0;
    follow.why = "frame";
    Framed::Frame(Scene { camera, board: Some(board) })
}

/// The status line's part.
pub fn status() -> serde_json::Value {
    serde_json::json!({
        "camera": format!("{:#x}", CAMERA.load(Ordering::Relaxed)),
        "board": format!("{:#x}", BOARD.load(Ordering::Relaxed)),
        "searching": WANTED.load(Ordering::Relaxed),
        "searches": SEARCHES.load(Ordering::Relaxed),
        "last": LAST.lock().map(|l| l.clone()).unwrap_or_default(),
        "found_again": QUICK.load(Ordering::Relaxed),
        "losses": LOSSES.load(Ordering::Relaxed),
        "last_loss": LOST_WHY.lock().map(|w| *w).unwrap_or(""),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: usize = 0x1_4000_0000;

    /// A fake proxy (0x100 bytes) whose frame matrix is `m` and whose render object is `object`.
    fn fake_proxy(m: [f32; 16], object: usize) -> Box<[u64; 32]> {
        let mut p = Box::new([0u64; 32]);
        p[0] = (BASE + rva::PROXY_VTABLE) as u64;
        p[OBJECT / 8] = object as u64;
        for k in 0..8 {
            p[MATRIX / 8 + k] = (m[2 * k].to_bits() as u64) | ((m[2 * k + 1].to_bits() as u64) << 32);
        }
        p
    }

    fn fake_skinned(bones: u32) -> Box<[u64; 40]> {
        let mut o = Box::new([0u64; 40]);
        o[0] = (BASE + rva::SKINNED_VTABLE) as u64;
        o[BONES / 8] = bones as u64;
        o
    }

    fn matrix(right: bars::V3, up: bars::V3, third: bars::V3, pos: bars::V3) -> [f32; 16] {
        [right[0], right[1], right[2], 0.0, up[0], up[1], up[2], 0.0, third[0], third[1], third[2], 0.0, pos[0], pos[1], pos[2], 1.0]
    }

    #[test]
    fn picks_the_boards_and_the_cameras_proxies_and_follows_them() {
        let board = Board { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], forward: [0.0, 0.0, 1.0], pos: [10.0, 0.0, 20.0] };
        let camera = Camera { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], back: [0.0, 0.0, 1.0], pos: [10.0, 1.0, 22.5], fov_y: 65.0 };
        let deck = fake_skinned(16);
        let skater = fake_skinned(386);
        let rigid = Box::new([0u64; 40]); // not a skinned render object
                                          // the frame's board, 4 cm behind the simulation's; the skater's proxy beside it, turned 90 degrees; a rigid
                                          // object on the board; the camera's, 6 cm off; something far away
        let mut board_p = fake_proxy(matrix(board.right, board.up, board.forward, [10.0, 0.0, 19.96]), deck.as_ptr() as usize);
        let skater_p = fake_proxy(matrix([0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [10.0, 0.0, 20.0]), skater.as_ptr() as usize);
        let rigid_p = fake_proxy(matrix(board.right, board.up, board.forward, [10.0, 0.0, 20.0]), rigid.as_ptr() as usize);
        let cam_p = fake_proxy(matrix(camera.right, camera.up, camera.back, [10.0, 1.0, 22.44]), 0);
        let far_p = fake_proxy(matrix(board.right, board.up, board.forward, [500.0, 0.0, 20.0]), deck.as_ptr() as usize);
        let found: Vec<usize> = [&skater_p, &rigid_p, &far_p, &board_p, &cam_p].iter().map(|p| p.as_ptr() as usize).collect();
        let (cam, brd, cams, boards) = pick(BASE, &found, &camera, &board);
        assert_eq!(brd, Some(board_p.as_ptr() as usize), "the skinned, board-aligned proxy");
        assert_eq!(cam, Some(cam_p.as_ptr() as usize));
        assert_eq!((cams, boards), (1, 1));

        // the render thread takes the frame's pair
        CAMERA.store(cam.unwrap(), Ordering::Release);
        BOARD.store(brd.unwrap(), Ordering::Release);
        WANTED.store(false, Ordering::Release);
        let sim = Scene { camera, board: Some(board) };
        let mut follow = Follow::default();
        let Framed::Frame(s) = frame_scene(BASE, sim, 1.0, &mut follow) else { panic!("{}", follow.why) };
        assert_eq!(follow.why, "frame");
        assert_eq!(s.board.unwrap().pos, [10.0, 0.0, 19.96]);
        assert_eq!(s.camera.pos, [10.0, 1.0, 22.44]);
        assert_eq!(s.camera.fov_y, 65.0, "the field of view stays the simulation's");
        // the renderer redraws the board each frame: its proxy changes (a fourth lane is enough here)
        let touch = |p: &mut [u64; 32], k: usize| p[MATRIX / 8 + 1] = (p[MATRIX / 8 + 1] & 0xffff_ffff) | ((k as f32).to_bits() as u64) << 32;
        let ride = |k: usize| {
            let mut s = sim;
            s.board.as_mut().unwrap().pos[2] = 20.0 - k as f32 * 0.01;
            s
        };

        // the board dropped off screen: its proxy stands still while the board moves; nothing to draw, nothing dropped
        let away = Scene { camera: Camera { back: [0.0, 0.0, -1.0], right: [-1.0, 0.0, 0.0], ..camera }, board: Some(board) };
        let mut follow = Follow::default();
        let mut hidden = 0;
        for k in 0..300 {
            let s = Scene { camera: away.camera, board: ride(k).board };
            if frame_scene(BASE, s, 1.0, &mut follow) == Framed::Hidden {
                hidden += 1;
            }
        }
        assert!(hidden > 290 && BOARD.load(Ordering::Acquire) != 0 && !WANTED.load(Ordering::Acquire), "{hidden} {}", follow.why);
        // back on screen and drawn again: the same proxies, at once
        touch(&mut board_p, 1);
        assert!(matches!(frame_scene(BASE, sim, 1.0, &mut follow), Framed::Frame(_)), "{}", follow.why);
        // standing still while the board travels a metre on screen: not the board's proxy after all
        let mut follow = Follow::default();
        for k in 0..200 {
            if WANTED.load(Ordering::Acquire) {
                break;
            }
            frame_scene(BASE, ride(k), 1.0, &mut follow);
        }
        assert_eq!(follow.why, "proxies: the board's stood still while the board moved on screen");
        assert!(WANTED.load(Ordering::Acquire) && BOARD.load(Ordering::Acquire) == 0);

        // a pause or a menu: the simulation's camera elsewhere, nothing moving: the proxies are kept (the simulation's
        // pair drawn meanwhile), and used again as soon as the camera is back; dropped only if the board rides on far
        CAMERA.store(cam.unwrap(), Ordering::Release);
        BOARD.store(brd.unwrap(), Ordering::Release);
        WANTED.store(false, Ordering::Release);
        let mut follow = Follow::default();
        let menu = Scene { camera: Camera { pos: [0.0, -69.0, 0.0], ..camera }, board: Some(board) };
        for _ in 0..200 {
            assert_eq!(frame_scene(BASE, menu, 1.0, &mut follow), Framed::Sim(menu));
        }
        assert!(follow.why.starts_with("proxies: kept") && !WANTED.load(Ordering::Acquire) && BOARD.load(Ordering::Acquire) != 0);
        assert!(matches!(frame_scene(BASE, sim, 1.0, &mut follow), Framed::Frame(_)), "back from the menu: the same proxies");
        // a respawn: the simulation's board jumps 10 m; the frame's pair follows a frame later: kept
        let mut jumped = sim;
        jumped.board.as_mut().unwrap().pos[0] += 10.0;
        touch(&mut board_p, 1000);
        assert_eq!(frame_scene(BASE, jumped, 1.0, &mut follow), Framed::Sim(jumped));
        assert!(!WANTED.load(Ordering::Acquire) && BOARD.load(Ordering::Acquire) != 0, "a jump does not drop the proxies: {}", follow.why);
        touch(&mut board_p, 1001);
        assert!(matches!(frame_scene(BASE, sim, 1.0, &mut follow), Framed::Frame(_)), "{}", follow.why);
        // riding on while the proxies stay far: dropped after three seconds and two metres
        for k in 0..400 {
            if WANTED.load(Ordering::Acquire) {
                break;
            }
            touch(&mut board_p, k + 2);
            frame_scene(BASE, Scene { camera: menu.camera, board: ride(k).board }, 1.0, &mut follow);
        }
        assert_eq!(follow.why, "proxies: far from the simulation's camera or board for 3 s while the board travelled 2 m");

        // a proxy that went away (its vtable changed) is dropped at once
        CAMERA.store(cam.unwrap(), Ordering::Release);
        let mut gone = fake_proxy(matrix(board.right, board.up, board.forward, [10.0, 0.0, 20.0]), deck.as_ptr() as usize);
        gone[0] = 0;
        BOARD.store(gone.as_ptr() as usize, Ordering::Release);
        assert_eq!(frame_scene(BASE, sim, 1.0, &mut Follow::default()), Framed::Sim(sim));
        assert_eq!(*LOST_WHY.lock().unwrap(), "proxies: gone");
        CAMERA.store(0, Ordering::Release);
        BOARD.store(0, Ordering::Release);
    }
}
