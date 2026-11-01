//! `board video SOURCE`: frames onto the ridden board through the DLL's video ring (caveman-fx video.rs). The source
//! is a video file (decoded by ffmpeg at its own pace, looped unless `--once`) or `obs`, OBS Studio's virtual camera
//! (a DirectShow device, "OBS Virtual Camera": whatever the OBS scene shows), or `camera:NAME` for any other.
//! Each frame is fitted to the in-world copy's shape (cover by default), made into an RGBA8 mip chain and put into
//! the ring; the DLL uploads the newest one every present.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use caveman_core::anim;
use caveman_core::image::Image;
use caveman_fx::proto::Request;
use caveman_fx::video as ring;
use caveman_fx::win;

use crate::board::{rgba_chain_levels, send};
use crate::{arg_value, Result};

pub const USAGE: &str = "board video FILE|obs|camera:NAME [--once] [--rotate cw|ccw|none] [--fit cover|contain] [--fps N] [--size WxH] [--slots N] [--seconds N] | board video off | board video status";

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(_: u32) -> win::BOOL {
    STOP.store(true, Ordering::Release);
    1
}

/// The producer's side of the ring: created here, filled frame by frame.
struct Ring {
    handle: win::HANDLE,
    view: *mut u8,
    slots: u32,
    slot_bytes: usize,
    seq: u64,
}

impl Ring {
    fn create(width: u32, height: u32, levels: u32, slots: u32, fps: f64) -> Result<Ring> {
        let slot_bytes = ring::chain_bytes(width, height, levels);
        let total = ring::mapping_bytes(slot_bytes, slots);
        let name = win::wide(ring::MAPPING_NAME);
        // SAFETY: plain Win32 calls; the mapping is backed by the page file
        let handle = unsafe {
            win::CreateFileMappingW(win::INVALID_HANDLE_VALUE, std::ptr::null_mut(), win::PAGE_READWRITE, (total >> 32) as u32, total as u32, name.as_ptr())
        };
        if handle.is_null() {
            return Err(format!("the video ring could not be created (error {})", unsafe { win::GetLastError() }));
        }
        if unsafe { win::GetLastError() } == win::ERROR_ALREADY_EXISTS {
            println!("  (another producer has the ring open: this one takes it over)");
        }
        let view = unsafe { win::MapViewOfFile(handle, win::FILE_MAP_ALL_ACCESS, 0, 0, total) } as *mut u8;
        if view.is_null() {
            unsafe { win::CloseHandle(handle) };
            return Err(format!("the video ring could not be mapped (error {})", unsafe { win::GetLastError() }));
        }
        let put32 = |off: usize, v: u32| unsafe { std::ptr::write_volatile(view.add(off) as *mut u32, v) };
        let put64 = |off: usize, v: u64| unsafe { AtomicU64::from_ptr(view.add(off) as *mut u64).store(v, Ordering::Release) };
        put64(ring::H_SEQ, 0);
        put64(ring::H_CONSUMER_SEQ, 0);
        put64(ring::H_CONSUMER_DROPS, 0);
        put64(ring::H_PRODUCER_ID, win::now_ms() ^ ((std::process::id() as u64) << 40));
        put32(ring::H_WIDTH, width);
        put32(ring::H_HEIGHT, height);
        put32(ring::H_LEVELS, levels);
        put32(ring::H_SLOTS, slots);
        put32(ring::H_SLOT_BYTES, slot_bytes as u32);
        put32(ring::H_FPS_X1000, (fps * 1000.0).round() as u32);
        put32(ring::H_VERSION, ring::VERSION);
        put32(ring::H_MAGIC, ring::MAGIC);
        Ok(Ring { handle, view, slots, slot_bytes, seq: 0 })
    }

    /// A finished chain into the next slot, then the sequence number after it (the DLL takes it from here on).
    fn push(&mut self, chain: &[u8]) {
        debug_assert_eq!(chain.len(), self.slot_bytes);
        let next = self.seq + 1;
        let slot = (next % self.slots as u64) as usize;
        // SAFETY: the slot lies inside the mapping (create sized it so)
        unsafe {
            let dst = self.view.add(ring::HEADER_BYTES + slot * ring::slot_stride(self.slot_bytes));
            std::ptr::copy_nonoverlapping(chain.as_ptr(), dst, chain.len().min(self.slot_bytes));
            AtomicU64::from_ptr(self.view.add(ring::H_SEQ) as *mut u64).store(next, Ordering::Release);
        }
        self.seq = next;
    }

    /// What the DLL took last and how many frames it dropped.
    fn consumer(&self) -> (u64, u64) {
        unsafe {
            (
                AtomicU64::from_ptr(self.view.add(ring::H_CONSUMER_SEQ) as *mut u64).load(Ordering::Acquire),
                AtomicU64::from_ptr(self.view.add(ring::H_CONSUMER_DROPS) as *mut u64).load(Ordering::Acquire),
            )
        }
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        unsafe {
            win::UnmapViewOfFile(self.view as *const _);
            win::CloseHandle(self.handle);
        }
    }
}

fn video_request(on: Option<bool>) -> Request {
    Request::Video { on }
}

fn print_status(v: &serde_json::Value) {
    let s = &v["video"];
    println!(
        "video: {}; frames {} (dropped busy {}, torn {}, skipped {}), last seq {} {}; fill {} us, whole {} us; ring {}{}",
        if s["on"] == true { "on" } else { "off" },
        s["frames"],
        s["dropped_busy"],
        s["dropped_torn"],
        s["skipped"],
        s["last_seq"],
        s["last_ms_ago"].as_u64().map(|m| format!("({m} ms ago)")).unwrap_or_default(),
        s["fill_us"],
        s["total_us"],
        if s["ring"].is_null() {
            "not open".to_string()
        } else {
            format!(
                "{}x{}, {} levels, {} slots, {} fps, seq {}",
                s["ring"]["width"], s["ring"]["height"], s["ring"]["levels"], s["ring"]["slots"], s["ring"]["fps"], s["ring"]["seq"]
            )
        },
        s["note"].as_str().filter(|n| !n.is_empty()).map(|n| format!("; {n}")).unwrap_or_default()
    );
}

/// The in-world art copy's shape from the board's status (width, height, mips), or the usual demoted one.
fn copy_shape() -> (u32, u32, u32, bool) {
    if let Ok(v) = send(&Request::board()) {
        for t in v["board"]["targets"].as_array().into_iter().flatten() {
            if t["kind"] == "ridden" {
                if let (Some(w), Some(h), Some(m)) = (t["width"].as_u64(), t["height"].as_u64(), t["mips"].as_u64()) {
                    if w > 0 && h > 0 && m > 0 {
                        return (w as u32, h as u32, m as u32, true);
                    }
                }
            }
        }
    }
    (256, 1024, 9, false)
}

pub fn video_cmd(args: &[String]) -> Result<()> {
    let source = args.get(1).filter(|a| !a.starts_with("--")).ok_or(USAGE)?;
    match source.to_ascii_lowercase().as_str() {
        "off" => {
            print_status(&send(&video_request(Some(false)))?);
            return Ok(());
        }
        "stop" => {
            // a producer running elsewhere (detached, or the app's) stops at the event, then the DLL's side goes off
            let name = win::wide(ring::STOP_EVENT_NAME);
            let ev = unsafe { win::OpenEventW(win::EVENT_MODIFY_STATE, 0, name.as_ptr()) };
            if ev.is_null() {
                println!("no producer is running (no stop event)");
            } else {
                unsafe {
                    win::SetEvent(ev);
                    win::CloseHandle(ev);
                }
                println!("the producer was told to stop");
                std::thread::sleep(std::time::Duration::from_millis(600));
            }
            print_status(&send(&video_request(Some(false)))?);
            return Ok(());
        }
        "status" => {
            print_status(&send(&video_request(None))?);
            return Ok(());
        }
        "on" => {
            // the DLL's side alone (a producer running already, or about to): it opens the ring now
            print_status(&send(&video_request(Some(true)))?);
            return Ok(());
        }
        _ => {}
    }
    let once = args.iter().any(|a| a == "--once");
    let rotate = arg_value(args, "--rotate").unwrap_or_else(|| {
        if source.eq_ignore_ascii_case("obs") || source.to_ascii_lowercase().starts_with("camera:") {
            "none".into()
        } else {
            "cw".into()
        }
    });
    let transpose = match rotate.as_str() {
        "cw" => "transpose=1,",
        "ccw" => "transpose=2,",
        "none" => "",
        other => return Err(format!("--rotate {other}: cw, ccw or none")),
    };
    let fit = arg_value(args, "--fit").unwrap_or_else(|| "cover".into());
    let fps: Option<f64> = arg_value(args, "--fps").map(|f| f.parse::<f64>().map_err(|_| format!("--fps {f}: a number"))).transpose()?;
    let slots: u32 = arg_value(args, "--slots")
        .map(|s| s.parse::<u32>().map_err(|_| format!("--slots {s}: 2 to {}", ring::MAX_SLOTS)))
        .transpose()?
        .unwrap_or(3)
        .clamp(2, ring::MAX_SLOTS);
    let seconds: Option<f64> = arg_value(args, "--seconds").map(|s| s.parse::<f64>().map_err(|_| format!("--seconds {s}: a number"))).transpose()?;
    let (ffmpeg, probe) = anim::ffmpeg_tools().ok_or("ffmpeg and ffprobe are not on PATH")?;
    // the shape: the in-world copy's (or given)
    let (width, height, levels, found) = match arg_value(args, "--size") {
        Some(s) => {
            let (w, h) = s.split_once('x').ok_or("--size WxH")?;
            let (w, h): (u32, u32) = (w.parse().map_err(|_| "--size WxH")?, h.parse().map_err(|_| "--size WxH")?);
            let mut levels = 1;
            let (mut a, mut b) = (w, h);
            while a > 1 || b > 1 {
                a = (a / 2).max(1);
                b = (b / 2).max(1);
                levels += 1;
                if a == 1 && b <= 4 {
                    break;
                }
            }
            (w, h, levels, true)
        }
        None => copy_shape(),
    };
    if !found {
        println!("  the in-world copy is not found yet (equip The Caveman once): frames are made {width}x{height}, its usual shape");
    }
    // the source
    let camera: Option<String> = if source.eq_ignore_ascii_case("obs") {
        Some("OBS Virtual Camera".into())
    } else {
        source.strip_prefix("camera:").or_else(|| source.strip_prefix("Camera:")).map(|n| n.to_string())
    };
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-v", "error", "-nostdin"]);
    let mut source_fps = fps.unwrap_or(30.0);
    match &camera {
        Some(name) => {
            cmd.args(["-f", "dshow", "-rtbufsize", "128M"]);
            if let Some(f) = fps {
                cmd.args(["-framerate", &format!("{f}")]);
            }
            cmd.arg("-i").arg(format!("video={name}"));
        }
        None => {
            let file = std::path::Path::new(source);
            if !file.is_file() {
                return Err(format!("{source}: not a file (or obs, or camera:NAME)"));
            }
            let info = anim::video_probe(&probe, file).map_err(|e| e.to_string())?;
            source_fps = info.fps;
            println!("  {}: {}x{} at {:.2} fps, {:.1} s", file.display(), info.width, info.height, info.fps, info.duration_ms as f64 / 1000.0);
            if !once {
                cmd.args(["-stream_loop", "-1"]);
            }
            cmd.args(["-re", "-i"]).arg(file);
        }
    }
    let fitted = match fit.as_str() {
        "cover" => format!("scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height}"),
        "contain" => format!("scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:black"),
        other => return Err(format!("--fit {other}: cover or contain")),
    };
    let vf = format!("{transpose}{fitted},format=rgba");
    cmd.args(["-vf", &vf, "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    cmd.stdin(Stdio::null()).stderr(Stdio::piped()).stdout(Stdio::piped());
    let mut ring = Ring::create(width, height, levels, slots, source_fps)?;
    println!("  ring: {width}x{height}, {levels} levels, {slots} slots ({} KB a frame); rotate {rotate}, fit {fit}", ring.slot_bytes / 1024);
    let v = send(&video_request(Some(true)))?;
    println!("  the DLL: {}", v["video"]["note"].as_str().unwrap_or(""));
    let mut child = cmd.spawn().map_err(|e| format!("ffmpeg: {e}"))?;
    let mut out = child.stdout.take().ok_or("ffmpeg: no output")?;
    // ffmpeg's errors as they come, but not its complaints about the pipe we close when stopping it
    let errors = child.stderr.take().map(|e| {
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut kept = Vec::new();
            for line in std::io::BufReader::new(e).lines().map_while(std::result::Result::ok) {
                if line.contains("Error muxing") || line.contains("writing trailer") || line.contains("closing file") {
                    continue;
                }
                eprintln!("  ffmpeg: {line}");
                kept.push(line);
            }
            kept
        })
    });
    unsafe { win::SetConsoleCtrlHandler(Some(on_ctrl), 1) };
    // the stop event (board video stop, the app): made or opened, and cleared for this run
    let stop_name = win::wide(ring::STOP_EVENT_NAME);
    let stop_event = unsafe { win::CreateEventW(std::ptr::null_mut(), 1, 0, stop_name.as_ptr()) };
    if !stop_event.is_null() {
        unsafe { win::ResetEvent(stop_event) };
    }
    println!("  playing (Ctrl+C or board video stop ends it){}", seconds.map(|s| format!(", for {s} s")).unwrap_or_default());
    let frame_bytes = (width * height * 4) as usize;
    let mut buf = vec![0u8; frame_bytes];
    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut frames: u64 = 0;
    let mut frames_at_report: u64 = 0;
    let mut chain_us_total: u128 = 0;
    let mut why = "the source ended";
    loop {
        if STOP.load(Ordering::Acquire) {
            why = "stopped";
            break;
        }
        if !stop_event.is_null() && unsafe { win::WaitForSingleObject(stop_event, 0) } == win::WAIT_OBJECT_0 {
            why = "stopped (board video stop)";
            break;
        }
        if seconds.is_some_and(|s| started.elapsed().as_secs_f64() >= s) {
            why = "the time is up";
            break;
        }
        if out.read_exact(&mut buf).is_err() {
            break;
        }
        let t = Instant::now();
        let img = Image::from_raw(width as usize, height as usize, 4, buf.clone()).map_err(|e| e.to_string())?;
        let chain = rgba_chain_levels(&img, levels as usize);
        ring.push(&chain);
        chain_us_total += t.elapsed().as_micros();
        frames += 1;
        if last_report.elapsed().as_secs_f64() >= 1.0 {
            let (taken, drops) = ring.consumer();
            let rate = (frames - frames_at_report) as f64 / last_report.elapsed().as_secs_f64();
            print!(
                "\r  sent {frames} ({rate:.1}/s, {} us a frame to prepare); the game took {taken}, dropped {drops}      ",
                if frames > 0 { chain_us_total / frames as u128 } else { 0 }
            );
            let _ = std::io::stdout().flush();
            last_report = Instant::now();
            frames_at_report = frames;
        }
    }
    println!();
    let _ = child.kill();
    let _ = child.wait();
    if let Some(t) = errors {
        if let Ok(lines) = t.join() {
            if frames == 0 && !lines.is_empty() {
                println!("  ffmpeg sent no frames: {}", lines.last().unwrap());
            }
        }
    }
    if !stop_event.is_null() {
        unsafe { win::CloseHandle(stop_event) };
    }
    let (taken, drops) = ring.consumer();
    println!("  {why}: {frames} frames sent in {:.1} s; the game took {taken}, dropped {drops}", started.elapsed().as_secs_f64());
    let v = send(&video_request(Some(false)))?;
    print_status(&v);
    Ok(())
}
