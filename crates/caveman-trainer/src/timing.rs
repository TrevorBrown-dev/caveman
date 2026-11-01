//! `probe-timing`: when the camera and the board's transform change, measured from outside (ReadProcessMemory,
//! read only, no injection), against the presents the injected DLL counts (its own `PRESENTS` counter, found in its
//! image). Also finds copies of both matrices in the game's memory (the renderer's own copies are candidates for
//! "the transform the frame was drawn with") and times them the same way. Built to explain the bars' jitter in
//! motion (docs/fx-hook-plan.md): whether the board's transform changes at the simulation rate while the frames
//! (and the camera) run faster.

use std::time::{Duration, Instant};

use caveman_fx::proto::{self, Request};
use caveman_fx::win::{self, Handle};
use caveman_fx::{bars, proxy, scene, skater};

use crate::{arg_value, check_build, find_game, modules, Result};

const CHUNK: usize = 4 << 20;
const PAGE: usize = 4096;
const MAX_COPIES: usize = 48;

type M = [f32; 16];

fn read_m(addr: usize) -> Option<M> {
    let mut raw = [0u8; 64];
    if !win::read_into(addr, &mut raw) {
        return None;
    }
    Some(std::array::from_fn(|k| f32::from_le_bytes(raw[k * 4..k * 4 + 4].try_into().unwrap())))
}

/// Whether `m` (rows right, up, forward or back, position) is a copy of `r`: rows within about 8 degrees, the
/// position within `tol` metres on each axis.
fn like(m: &[f32], r: &M, tol: f32) -> bool {
    for k in 0..3 {
        if (m[12 + k] - r[12 + k]).abs() > tol || !m[12 + k].is_finite() {
            return false;
        }
    }
    for row in 0..3 {
        let a = [m[row * 4], m[row * 4 + 1], m[row * 4 + 2]];
        let b = [r[row * 4], r[row * 4 + 1], r[row * 4 + 2]];
        let la = bars::length(a);
        if !(0.95..1.05).contains(&la) || bars::dot(a, b) / la < 0.99 {
            return false;
        }
    }
    true
}

fn readable(protect: u32, wc: bool) -> bool {
    let bad = if wc { win::PAGE_GUARD } else { win::PAGE_GUARD | win::PAGE_NOCACHE | win::PAGE_WRITECOMBINE };
    protect & bad == 0 && matches!(protect & 0xFF, win::PAGE_READWRITE | win::PAGE_EXECUTE_READWRITE)
}

struct Copy {
    of: usize,
    addr: usize,
    wc: bool,
}

/// Copies of the reference matrices (16-byte aligned, rows layout) in the game's private read-write memory (resident
/// pages only); with `wc`, write-combined and uncached memory too (the driver's upload heaps: slow to read).
fn scan_copies(refs_at: &[(&str, usize)], tol: f32, wc: bool, object_vt: u64) -> (Vec<Copy>, Vec<usize>, u64, Duration) {
    let t = Instant::now();
    let mut out = Vec::new();
    let mut objects = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let mut pages: Vec<win::PSAPI_WORKING_SET_EX_INFORMATION> = Vec::with_capacity(CHUNK / PAGE);
    let mut next = win::LOWEST;
    let mut bytes = 0u64;
    let mut refs: Vec<(M, usize)>;
    while next < win::HIGHEST && out.len() < MAX_COPIES * refs_at.len() {
        let Some(mbi) = win::query(next) else { break };
        let start = mbi.BaseAddress as usize;
        let end = start.saturating_add(mbi.RegionSize);
        if end <= next {
            break;
        }
        next = end;
        if mbi.State != win::MEM_COMMIT || mbi.Type != win::MEM_PRIVATE || !readable(mbi.Protect, wc) {
            continue;
        }
        let region_wc = mbi.Protect & (win::PAGE_NOCACHE | win::PAGE_WRITECOMBINE) != 0;
        let mut at = start;
        while at < end {
            let len = (end - at).min(CHUNK);
            // the references now (they move while the skater rides)
            refs = refs_at.iter().map(|(_, a)| (read_m(*a).unwrap_or([f32::NAN; 16]), *a)).collect();
            let near = [refs[1].0[12], refs[1].0[13], refs[1].0[14]];
            let npages = len / PAGE;
            pages.clear();
            for p in 0..npages {
                pages.push(win::PSAPI_WORKING_SET_EX_INFORMATION { VirtualAddress: (at + p * PAGE) as *mut _, VirtualAttributes: 0 });
            }
            let size = (npages * std::mem::size_of::<win::PSAPI_WORKING_SET_EX_INFORMATION>()) as u32;
            if npages > 0 && unsafe { win::K32QueryWorkingSetEx(win::target(), pages.as_mut_ptr().cast(), size) } != 0 {
                let mut p = 0;
                while p < npages {
                    if pages[p].VirtualAttributes & 1 == 0 {
                        p += 1;
                        continue;
                    }
                    let first = p;
                    while p < npages && pages[p].VirtualAttributes & 1 != 0 {
                        p += 1;
                    }
                    let (s, n) = (at + first * PAGE, (p - first) * PAGE);
                    if !win::read_into(s, &mut buf[..n]) {
                        continue;
                    }
                    bytes += n as u64;
                    let floats: Vec<f32> = buf[..n].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
                    for (j, w) in buf[..n].chunks_exact(8).enumerate() {
                        if u64::from_le_bytes(w.try_into().unwrap()) == object_vt && objects.len() < 4096 {
                            let obj = s + j * 8;
                            if let Some(m) = read_m(obj + 0x50) {
                                if (0..3).all(|k| (m[12 + k] - near[k]).abs() < 1.5) {
                                    objects.push(obj);
                                }
                            }
                        }
                    }
                    for q in (0..floats.len().saturating_sub(15)).step_by(4) {
                        for (i, (r, own)) in refs.iter().enumerate() {
                            let addr = s + q * 4;
                            if addr != *own && like(&floats[q..q + 16], r, tol) && out.iter().filter(|c: &&Copy| c.of == i).count() < MAX_COPIES {
                                out.push(Copy { of: i, addr, wc: region_wc });
                            }
                        }
                    }
                }
            }
            at += len;
        }
    }
    (out, objects, bytes, t.elapsed())
}

/// The DLL's present counter in its image (a u64 that tracks the status's `presents`), if the DLL is loaded.
fn find_present_counter(pid: u32) -> Option<usize> {
    let (_, base, size) = modules(pid).into_iter().find(|(n, _, _)| n.to_ascii_lowercase().starts_with("caveman_fx"))?;
    let presents = |v: &serde_json::Value| v["render"]["presents"].as_u64();
    let p0 = presents(&proto::request(proto::PIPE, &Request::Status, 2000).ok()?)?;
    let mut image = vec![0u8; size];
    // the image may have unreadable gaps: page by page
    let mut found = Vec::new();
    for (k, page) in image.chunks_mut(PAGE).enumerate() {
        if win::read_into(base + k * PAGE, page) {
            for (j, w) in page.chunks_exact(8).enumerate() {
                let v = u64::from_le_bytes(w.try_into().unwrap());
                if v >= p0 && v <= p0 + 3000 {
                    found.push((base + k * PAGE + j * 8, v));
                }
            }
        }
    }
    std::thread::sleep(Duration::from_millis(250));
    let p1 = presents(&proto::request(proto::PIPE, &Request::Status, 2000).ok()?)?;
    found
        .into_iter()
        .filter_map(|(a, v)| {
            let now = win::read_u64(a)?;
            (now > v && now - v < 200).then_some((a, now))
        })
        .min_by_key(|(_, now)| now.abs_diff(p1))
        .map(|(a, _)| a)
}

struct Track {
    name: String,
    addr: usize,
    len: usize,
    last: Vec<u8>,
    /// Change times (microseconds from the start) and, for matrices, the position then.
    changes: Vec<(u64, [f32; 3])>,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// `probe-projection [--fov DEGREES] [--aspect A] [--pid N]`: projection matrices in the game's memory, found by their
/// two scale terms (1 / tan(fov / 2) on the vertical axis, divided by the aspect on the horizontal), with what their
/// depth terms say (reversed or not, the near and far planes). Read only.
pub fn probe_projection(args: &[String]) -> Result<()> {
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let process = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_READ, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe (pid {}) for reading", game.pid));
    }
    check_build(process.0)?;
    let base = modules(game.pid).into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case("skate.exe")).map(|(_, b, _)| b).ok_or("cannot find Skate.exe's base")?;
    win::set_target(process.0, game.pid);
    let fov: f32 = match arg_value(args, "--fov").and_then(|v| v.parse().ok()) {
        Some(f) => f,
        None => scene::read_camera(base).map(|c| c.fov_y).unwrap_or(65.0),
    };
    let aspect: f32 = arg_value(args, "--aspect").and_then(|v| v.parse().ok()).unwrap_or(16.0 / 9.0);
    let fy = 1.0 / (fov.to_radians() * 0.5).tan();
    let fx = fy / aspect;
    println!("looking for projections with fov {fov} (y scale {fy:.5}) and aspect {aspect:.4} (x scale {fx:.5})");
    let mut buf = vec![0u8; CHUNK];
    let mut next = win::LOWEST;
    let mut found = 0;
    let t = Instant::now();
    while next < win::HIGHEST && found < 64 {
        let Some(mbi) = win::query(next) else { break };
        let start = mbi.BaseAddress as usize;
        let end = start.saturating_add(mbi.RegionSize);
        if end <= next {
            break;
        }
        next = end;
        if mbi.State != win::MEM_COMMIT || mbi.Type != win::MEM_PRIVATE || !readable(mbi.Protect, false) {
            continue;
        }
        let mut at = start;
        while at < end && found < 64 {
            let len = (end - at).min(CHUNK);
            if !win::read_into(at, &mut buf[..len]) {
                at += len;
                continue;
            }
            let f: Vec<f32> = buf[..len].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
            for i in 5..f.len().saturating_sub(11) {
                if (f[i] - fy).abs() < 2e-3 && (f[i - 5] - fx).abs() < 2e-3 && f[i - 4] == 0.0 && f[i - 1] == 0.0 {
                    let m = &f[i - 5..i + 11];
                    found += 1;
                    let addr = at + (i - 5) * 4;
                    // row vectors: z' = z m22 + m32, w' = z m23 + m33; columns: the transpose
                    let (a, b, c, d) = (m[10], m[11], m[14], m[15]);
                    println!(
                        "{addr:#x}: rows {:?}",
                        m.chunks(4).map(|r| r.iter().map(|v| format!("{v:.5}")).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>()
                    );
                    println!("    m22 {a:.6} m23 {b:.6} m32 {c:.6} m33 {d:.6}: {}", describe_depth(a, b, c, d));
                }
            }
            at += len;
        }
    }
    println!("{found} candidate(s) in {:.1} s", t.elapsed().as_secs_f64());
    Ok(())
}

/// What a projection's depth terms say, for row vectors (z' = z m22 + m32, w' = z m23 + m33) and view space looking
/// down -z or +z.
fn describe_depth(m22: f32, m23: f32, m32: f32, m33: f32) -> String {
    // depth at view distance d (looking down -z when m23 = -1: z = -d)
    let depth = |d: f32| {
        let z = if m23 < 0.0 { -d } else { d };
        let w = z * m23 + m33;
        (z * m22 + m32) / w
    };
    let (near, far) = (depth(0.5), depth(1000.0));
    let kind = if near > far { "reversed" } else { "standard" };
    let infinite = m22.abs() < 1e-6;
    let n = if infinite {
        m32.abs()
    } else if kind == "reversed" {
        (m32 / (m22 + m23.signum() * 1.0)).abs()
    } else {
        (m32 / m22).abs()
    };
    format!("{kind}{}, near about {n:.4} m (depth at 0.5 m {near:.5}, at 1000 m {far:.6})", if infinite { ", infinite far" } else { "" })
}

/// `probe-timing [SECONDS] [--wait-moving] [--copies [--tol METRES] [--wc]] [--pid N]`.
pub fn probe_timing(args: &[String]) -> Result<()> {
    let seconds: f64 = args.iter().find(|a| !a.starts_with("--") && a.parse::<f64>().is_ok()).and_then(|a| a.parse().ok()).unwrap_or(10.0);
    let tol: f32 = arg_value(args, "--tol").and_then(|v| v.parse().ok()).unwrap_or(0.3);
    let wc = args.iter().any(|a| a == "--wc");
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let process = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_READ, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe (pid {}) for reading: error {}", game.pid, unsafe { win::GetLastError() }));
    }
    check_build(process.0)?;
    let base = modules(game.pid).into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case("skate.exe")).map(|(_, b, _)| b).ok_or("cannot find Skate.exe's base")?;
    win::set_target(process.0, game.pid);
    let rt = skater::Runtime::new(base).ok_or("the game's globals are not readable")?;
    let mut worlds: Vec<(usize, u32)> = Vec::new();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        for w in rt.worlds() {
            if !worlds.iter().any(|x| x.0 == w.0) {
                worlds.push(w);
            }
        }
        scene::publish_worlds(base, &worlds);
        if scene::CLIENT_CTX.load(std::sync::atomic::Ordering::Acquire) != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let ctx = scene::CLIENT_CTX.load(std::sync::atomic::Ordering::Acquire);
    if ctx == 0 {
        return Err("no client world (a menu or loading?)".into());
    }
    let cam = scene::camera_object(base)?;
    let entity = scene::find_board_entity(base, ctx)?;
    let (coll, off) = scene::transform_location(entity).ok_or("the board's transform is not where expected")?;
    let board_at = coll + off;
    let cam_m = read_m(cam + 0x50).ok_or("the camera is unreadable")?;
    let board_m = read_m(board_at).ok_or("the board is unreadable")?;
    println!("camera object {cam:#x} (matrix at +0x50), position ({:.3}, {:.3}, {:.3})", cam_m[12], cam_m[13], cam_m[14]);
    println!("board entity {entity:#x}, transform at {board_at:#x}, position ({:.3}, {:.3}, {:.3})", board_m[12], board_m[13], board_m[14]);
    let presents_at = find_present_counter(game.pid);
    match presents_at {
        Some(a) => println!("the DLL's present counter at {a:#x}"),
        None => println!("no present counter (the DLL is not loaded, or its counter was not found): timing without presents"),
    }
    let mut tracks: Vec<Track> = vec![
        Track { name: "camera".into(), addr: cam + 0x50, len: 64, last: Vec::new(), changes: Vec::new() },
        Track { name: "board".into(), addr: board_at, len: 64, last: Vec::new(), changes: Vec::new() },
    ];
    // the renderer's per-frame transforms, found as the DLL finds them (proxy.rs)
    let fov = f32::from_bits(win::read_u32(cam + 0xac).unwrap_or(0));
    let mut frame_pair = None;
    {
        let t = Instant::now();
        let mut s = caveman_fx::scan::WordScan::new(&[(base + proxy::rva::PROXY_VTABLE) as u64], 1 << 17)?;
        while !s.step(256 << 20, 1000) {}
        let found: Vec<usize> = s.found.clone();
        let sim_cam = read_m(cam + 0x50).and_then(|m| bars::Camera::from_matrix(&m, fov));
        let sim_board = read_m(board_at).and_then(|m| bars::Board::from_matrix(&m));
        match (sim_cam, sim_board) {
            (Some(c), Some(b)) => {
                let (pc, pb, cams, boards) = proxy::pick(base, &found, &c, &b);
                println!(
                    "{} render transform proxies ({} MB) in {:.1} s: camera {} ({cams} on the camera), board {} ({boards} on the board)",
                    found.len(),
                    s.bytes >> 20,
                    t.elapsed().as_secs_f64(),
                    pc.map_or("not found".into(), |a| format!("{a:#x}")),
                    pb.map_or("not found".into(), |a| format!("{a:#x}"))
                );
                if let (Some(pc), Some(pb)) = (pc, pb) {
                    frame_pair = Some((pc, pb));
                }
            }
            _ => println!("the simulation's camera or board is not usable now: no proxies picked"),
        }
    }
    if let Some((pc, pb)) = frame_pair {
        tracks.push(Track { name: "frame-camera".into(), addr: pc + proxy::MATRIX, len: 64, last: Vec::new(), changes: Vec::new() });
        tracks.push(Track { name: "frame-board".into(), addr: pb + proxy::MATRIX, len: 64, last: Vec::new(), changes: Vec::new() });
    }
    if args.iter().any(|a| a == "--copies") {
        let refs = [("camera", cam + 0x50), ("board", board_at)];
        // skinned render objects (ReSkate's skater_render.h: vtable 0x65f2910, the actor matrix at +0x50)
        let object_vt = (base + 0x65f2910) as u64;
        let (copies, objects, bytes, took) = scan_copies(&refs, tol, wc, object_vt);
        println!(
            "scanned {} MB in {:.1} s: {} copies (within {tol} m), {} skinned render objects within 1.5 m of the board",
            bytes >> 20,
            took.as_secs_f64(),
            copies.len(),
            objects.len()
        );
        for (k, c) in copies.iter().enumerate() {
            let name = format!("{}-copy{}", refs[c.of].0, k);
            println!("  {name} at {:#x}{}", c.addr, if c.wc { " (write-combined)" } else { "" });
            tracks.push(Track { name, addr: c.addr, len: 64, last: Vec::new(), changes: Vec::new() });
        }
        for (k, o) in objects.iter().enumerate() {
            let m = read_m(o + 0x50).unwrap_or([0.0; 16]);
            let rows: Vec<String> = (0..3)
                .map(|r| format!("{:.2}", bars::dot([m[r * 4], m[r * 4 + 1], m[r * 4 + 2]], [board_m[r * 4], board_m[r * 4 + 1], board_m[r * 4 + 2]])))
                .collect();
            let bones = win::read_u32(o + 0xf8).unwrap_or(0);
            let name = format!("render{k}");
            println!(
                "  {name}: object {o:#x}, matrix +0x50 at ({:.3}, {:.3}, {:.3}), rows . board's rows {}, {bones} bones",
                m[12],
                m[13],
                m[14],
                rows.join("/")
            );
            tracks.push(Track { name, addr: o + 0x50, len: 64, last: Vec::new(), changes: Vec::new() });
        }
    }
    if args.iter().any(|a| a == "--wait-moving") {
        println!("waiting (up to 120 s) for the board to move (0.5 m in half a second)");
        let t = Instant::now();
        let start = read_m(board_at).map(|m| [m[12], m[13], m[14]]).unwrap_or_default();
        let mut from = (Instant::now(), start);
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let p = read_m(board_at).map(|m| [m[12], m[13], m[14]]).unwrap_or_default();
            if bars::length(bars::sub(p, from.1)) > 0.5 {
                break;
            }
            if from.0.elapsed() > Duration::from_millis(500) {
                from = (Instant::now(), p);
            }
            if t.elapsed() > Duration::from_secs(120) {
                return Err("the board did not move".into());
            }
        }
    }
    println!("polling for {seconds} s: ride now (changes only count while things move)");
    let t0 = Instant::now();
    let end = Duration::from_secs_f64(seconds);
    let mut present_times: Vec<u64> = Vec::new();
    let mut last_present = presents_at.and_then(win::read_u64).unwrap_or(0);
    let mut polls = 0u64;
    let mut buf = [0u8; 64];
    // where a bar's tip (0.4 m out on the board's right) lands on a 3840x2160 screen at each present, placed by the
    // simulation's pair and by the frame's: the picture moves smoothly, so the rougher series is the jitter
    let tip = |c: Option<M>, b: Option<M>| -> Option<(f32, f32)> {
        let cam = bars::Camera::from_matrix(&c?, fov)?;
        let bd = bars::Board::from_matrix(&b?)?;
        let q = bars::project(&cam, 16.0 / 9.0, bars::add(bd.pos, bars::mul(bd.right, 0.405)));
        (q[3] > 0.3).then(|| ((q[0] / q[3] + 1.0) * 0.5 * 3840.0, (1.0 - q[1] / q[3]) * 0.5 * 2160.0))
    };
    let mut sim_tips: Vec<Option<(f32, f32)>> = Vec::new();
    let mut frame_tips: Vec<Option<(f32, f32)>> = Vec::new();
    while t0.elapsed() < end {
        polls += 1;
        if let Some(a) = presents_at {
            if let Some(p) = win::read_u64(a) {
                if p != last_present {
                    let now = t0.elapsed().as_micros() as u64;
                    for _ in 0..(p.saturating_sub(last_present)).min(4) {
                        present_times.push(now);
                    }
                    sim_tips.push(tip(read_m(cam + 0x50), read_m(board_at)));
                    frame_tips.push(frame_pair.and_then(|(pc, pb)| tip(read_m(pc + proxy::MATRIX), read_m(pb + proxy::MATRIX))));
                    last_present = p;
                }
            }
        }
        for tr in tracks.iter_mut() {
            if !win::read_into(tr.addr, &mut buf[..tr.len]) {
                continue;
            }
            if tr.last.as_slice() != &buf[..tr.len] {
                let now = t0.elapsed().as_micros() as u64;
                if !tr.last.is_empty() {
                    let f = |k: usize| f32::from_le_bytes(buf[k * 4..k * 4 + 4].try_into().unwrap());
                    tr.changes.push((now, [f(12), f(13), f(14)]));
                }
                tr.last.clear();
                tr.last.extend_from_slice(&buf[..tr.len]);
            }
        }
    }
    let secs = t0.elapsed().as_secs_f64();
    println!(
        "{polls} polls in {secs:.1} s ({:.0} us a poll); {} presents ({:.1}/s)",
        secs * 1e6 / polls as f64,
        present_times.len(),
        present_times.len() as f64 / secs
    );
    let board_changes = tracks[1].changes.clone();
    let camera_changes = tracks[0].changes.clone();
    println!(
        "{:<14} {:>8} {:>7} {:>22} {:>26} {:>18} {:>12}",
        "source", "changes", "per s", "interval ms p10/50/90", "per present 0/1/2+ (phase)", "with camera/board", "from board m"
    );
    for tr in &tracks {
        let n = tr.changes.len();
        let mut intervals: Vec<f64> = tr.changes.windows(2).map(|w| (w[1].0 - w[0].0) as f64 / 1000.0).collect();
        let (p10, p50, p90) = (pct(&mut intervals, 0.1), pct(&mut intervals, 0.5), pct(&mut intervals, 0.9));
        let mut per = [0u32; 3];
        let mut phases = Vec::new();
        for w in present_times.windows(2) {
            let inside: Vec<u64> = tr.changes.iter().map(|c| c.0).filter(|&c| c >= w[0] && c < w[1]).collect();
            per[inside.len().min(2)] += 1;
            for c in inside {
                phases.push((c - w[0]) as f64 / (w[1] - w[0]).max(1) as f64);
            }
        }
        let phase = pct(&mut phases, 0.5);
        let near = |list: &[(u64, [f32; 3])], t: u64| list.iter().any(|c| c.0.abs_diff(t) <= 700);
        let with_cam = tr.changes.iter().filter(|c| near(&camera_changes, c.0)).count();
        let with_board = tr.changes.iter().filter(|c| near(&board_changes, c.0)).count();
        // how far this source's position is from the board entity's latest position when it changes
        let mut gaps: Vec<f64> = tr
            .changes
            .iter()
            .filter_map(|c| {
                let b = board_changes.iter().rev().find(|b| b.0 <= c.0)?;
                Some(bars::length(bars::sub(c.1, b.1)) as f64)
            })
            .collect();
        let gap = pct(&mut gaps, 0.5);
        println!(
            "{:<14} {:>8} {:>7.1} {:>22} {:>26} {:>18} {:>12}",
            tr.name,
            n,
            n as f64 / secs,
            format!("{p10:.1}/{p50:.1}/{p90:.1}"),
            if present_times.len() > 1 { format!("{}/{}/{} ({phase:.2})", per[0], per[1], per[2]) } else { "-".into() },
            format!("{with_cam}/{with_board}"),
            if gap.is_finite() { format!("{gap:.3}") } else { "-".into() }
        );
    }
    println!("(per present: how many present intervals saw 0, 1, 2+ changes, and the median phase of a change in its interval; with camera/board: changes within 0.7 ms of one of theirs; from board m: median distance from the board entity's position at that time)");
    // the jitter: how much a bar tip's screen position departs from a smooth path, frame to frame (the second
    // difference: zero for steady motion, large when a frame jumps ahead or lags)
    let rough = |tips: &[Option<(f32, f32)>]| -> Option<(f64, f64, usize)> {
        let mut d2: Vec<f64> = tips
            .windows(3)
            .filter_map(|w| {
                let (a, b, c) = (w[0]?, w[1]?, w[2]?);
                Some((((c.0 - 2.0 * b.0 + a.0) as f64).powi(2) + ((c.1 - 2.0 * b.1 + a.1) as f64).powi(2)).sqrt())
            })
            .collect();
        if d2.is_empty() {
            return None;
        }
        let rms = (d2.iter().map(|x| x * x).sum::<f64>() / d2.len() as f64).sqrt();
        Some((rms, pct(&mut d2, 0.95), d2.len()))
    };
    for (name, tips) in [("simulation's camera and board (tick)", &sim_tips), ("frame proxies (frame)", &frame_tips)] {
        match rough(tips) {
            Some((rms, p95, n)) => println!("jitter of a bar tip on a 3840x2160 screen, {name}: {rms:.1} px RMS, 95th percentile {p95:.1} px ({n} frames)"),
            None => println!("jitter, {name}: no samples"),
        }
    }
    Ok(())
}
