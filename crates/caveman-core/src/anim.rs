//! Animated decks (knowledge/animated-decks.md): a GIF or a video file in the drop folder becomes a deck whose
//! graphic plays the clip. The frames go into one flipbook atlas in the deck's holo map slot (`param_graphic_rgb`,
//! the texture the deck programs sample raw as t0, or t1 in variant B), the deck's art is the first frame, and
//! Caveman's deck shader picks the frame from the game clock when the marker in the atlas's top-left corner asks for
//! it: block 2's effect code is [`effects::EFFECT_ANIM`] and block 3 carries the frame count, the frames a second and
//! the number of columns ([`marker_block3`]).
//!
//! The atlas is a grid of equal cells, `cols` x `rows`, each a frame at one of three sizes (full 512 x 2048, half
//! 256 x 1024, quarter 128 x 512), with no gutters: the deck's visible area is inset from the cell edge (12 texels at
//! full size), so bilinear filtering only mixes neighbours at the first few mips. Both atlas sides are powers of two
//! and at most 16384; the frame size is the largest whose grid for the frame count fits the texel budget (16 MiB by
//! default, one byte a texel in BC7, plus a third for the mips).
//!
//! The clip's timing is kept by resampling: the output frames are spread evenly over the clip's duration, and the
//! shader plays them at a constant whole frame rate (`frac(clock * fps / n) * n`), so a loop lasts `n / fps` seconds.
//! The marker's numbers are even (the frame count, the frame rate) or multiples of 16 (log2 of the columns) because
//! the mode-6 BC7 encoder reproduces even values of a solid block exactly.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::effects::{self, EFFECT_ANIM};
use crate::image::Image;

/// Names the atlas layout and marker scheme; part of an animated deck's item-cache input, bumped when they change.
pub const SCHEME: &str = "caveman-anim-1";
/// The most frames an atlas holds (even, fits the marker byte with room).
pub const MAX_FRAMES: usize = 254;
/// The frame cap when the settings give none.
pub const DEFAULT_MAX_FRAMES: usize = 64;
pub const MIN_FPS: u32 = 2;
pub const MAX_FPS: u32 = 60;
/// Texels of the atlas's first mip: the default budget and the most the settings may ask for.
pub const DEFAULT_BUDGET: usize = 16 << 20;
pub const MAX_BUDGET: usize = 32 << 20;
/// The longest video Caveman decodes (seconds); a GIF is taken whole up to the frame cap.
pub const DEFAULT_MAX_SECONDS: f64 = 30.0;
const MAX_SIDE: usize = 16384;
/// Texels of a decoded frame at most (a 4K video is scaled down before it is read).
const MAX_SOURCE_SIDE: usize = 2048;
/// A GIF frame delay under 20 ms plays at 100 ms, as browsers do.
const MIN_GIF_DELAY_MS: u32 = 20;
const SLOW_GIF_DELAY_MS: u32 = 100;

#[derive(Debug)]
pub struct AnimError(pub String);

impl fmt::Display for AnimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AnimError {}

fn err<T>(message: impl Into<String>) -> Result<T, AnimError> {
    Err(AnimError(message.into()))
}

/// Decoded frames (RGBA, all the clip's size) and how long each shows.
#[derive(Clone, Debug)]
pub struct Clip {
    pub width: usize,
    pub height: usize,
    pub frames: Vec<Image>,
    pub delays_ms: Vec<u32>,
}

impl Clip {
    pub fn duration_ms(&self) -> u32 {
        self.delays_ms.iter().sum()
    }

    /// Frames a second the clip was made at (the median delay).
    pub fn native_fps(&self) -> f64 {
        let mut d: Vec<u32> = self.delays_ms.clone();
        if d.is_empty() {
            return 10.0;
        }
        d.sort_unstable();
        let median = d[d.len() / 2].max(1);
        1000.0 / median as f64
    }
}

/// The size of one frame in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameSize {
    /// The largest size whose grid fits the budget.
    Auto,
    Full,
    Half,
    Quarter,
}

impl FrameSize {
    /// Width and height of a frame; None for Auto.
    pub fn dims(self) -> Option<(usize, usize)> {
        match self {
            FrameSize::Auto => None,
            FrameSize::Full => Some((512, 2048)),
            FrameSize::Half => Some((256, 1024)),
            FrameSize::Quarter => Some((128, 512)),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FrameSize::Auto => "auto",
            FrameSize::Full => "full",
            FrameSize::Half => "half",
            FrameSize::Quarter => "quarter",
        }
    }
}

/// How a clip becomes an atlas: the frame rate (None: the clip's own), the frame cap, the frame size and the texel
/// budget of the atlas's first mip.
#[derive(Clone, Debug)]
pub struct Policy {
    pub fps: Option<f64>,
    pub max_frames: usize,
    pub size: FrameSize,
    pub budget: usize,
    pub max_seconds: f64,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy { fps: None, max_frames: DEFAULT_MAX_FRAMES, size: FrameSize::Auto, budget: DEFAULT_BUDGET, max_seconds: DEFAULT_MAX_SECONDS }
    }
}

/// What the atlas will be: the frame rate, which source frame each cell shows, the cell size and the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub fps: u32,
    pub picks: Vec<usize>,
    pub frame: (usize, usize),
    pub cols: usize,
    pub rows: usize,
    pub notes: Vec<String>,
}

impl Plan {
    pub fn frames(&self) -> usize {
        self.picks.len()
    }

    pub fn width(&self) -> usize {
        self.cols * self.frame.0
    }

    pub fn height(&self) -> usize {
        self.rows * self.frame.1
    }

    /// Seconds of one loop.
    pub fn loop_seconds(&self) -> f64 {
        self.frames() as f64 / self.fps as f64
    }
}

fn round_even(x: f64) -> u32 {
    ((x / 2.0).round_ties_even() * 2.0).max(0.0) as u32
}

/// The frame rate (even, 2 to 60) and frame count (even, at most `max_frames`) for a clip of `duration_ms` at
/// `native_fps`, or at `wanted_fps`; a clip with more frames than the cap plays at a lower rate so the whole loop
/// still fits.
pub fn timing(duration_ms: u32, native_fps: f64, wanted_fps: Option<f64>, max_frames: usize) -> (u32, usize) {
    let max_frames = (max_frames.clamp(2, MAX_FRAMES) / 2) * 2;
    let seconds = (duration_ms.max(1) as f64 / 1000.0).max(0.01);
    let mut fps = wanted_fps.unwrap_or(native_fps).clamp(MIN_FPS as f64, MAX_FPS as f64);
    if seconds * fps > max_frames as f64 {
        fps = max_frames as f64 / seconds;
    }
    let fps = round_even(fps).clamp(MIN_FPS, MAX_FPS);
    let mut n = (seconds * fps as f64).round() as usize;
    if n % 2 == 1 {
        n += 1;
    }
    (fps, n.clamp(2, max_frames))
}

/// The source frame showing at each of `n` moments spread evenly over the clip.
fn picks(clip: &Clip, n: usize) -> Vec<usize> {
    let total = clip.duration_ms().max(1) as f64;
    let mut ends = Vec::with_capacity(clip.frames.len());
    let mut acc = 0u64;
    for d in &clip.delays_ms {
        acc += *d as u64;
        ends.push(acc as f64);
    }
    (0..n)
        .map(|k| {
            let t = (k as f64 + 0.5) * total / n as f64;
            ends.iter().position(|&e| t < e).unwrap_or(clip.frames.len() - 1)
        })
        .collect()
}

/// The grid for `n` cells of `frame` within `budget` texels: columns and rows both powers of two, the smallest
/// atlas, then the squarest.
fn arrange(n: usize, frame: (usize, usize), budget: usize) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, usize, usize)> = None; // area, max side, cols, rows
    let mut cols = 1;
    while cols <= 32 {
        let rows = (n.div_ceil(cols)).next_power_of_two();
        let (w, h) = (cols * frame.0, rows * frame.1);
        if w <= MAX_SIDE && h <= MAX_SIDE && w * h <= budget {
            let key = (w * h, w.max(h), cols, rows);
            if best.map_or(true, |b| (key.0, key.1) < (b.0, b.1)) {
                best = Some(key);
            }
        }
        cols *= 2;
    }
    best.map(|b| (b.2, b.3))
}

/// How the clip becomes an atlas under `policy`.
pub fn plan(clip: &Clip, policy: &Policy) -> Result<Plan, AnimError> {
    if clip.frames.is_empty() || clip.frames.len() != clip.delays_ms.len() {
        return err("the clip has no frames");
    }
    let budget = policy.budget.clamp(1 << 20, MAX_BUDGET);
    let (fps, mut n) = timing(clip.duration_ms(), clip.native_fps(), policy.fps, policy.max_frames);
    let mut notes = Vec::new();
    let sizes: Vec<FrameSize> = match policy.size {
        FrameSize::Auto => vec![FrameSize::Full, FrameSize::Half, FrameSize::Quarter],
        s => vec![s, FrameSize::Quarter],
    };
    let mut chosen: Option<(FrameSize, (usize, usize), (usize, usize))> = None;
    for s in &sizes {
        let dims = s.dims().unwrap();
        if let Some(grid) = arrange(n, dims, budget) {
            chosen = Some((*s, dims, grid));
            break;
        }
    }
    let (size, dims, grid) = match chosen {
        Some(c) => c,
        None => {
            // even quarter frames do not fit: fewer frames
            let dims = FrameSize::Quarter.dims().unwrap();
            let fit = ((budget / (dims.0 * dims.1)).min(MAX_FRAMES) / 2 * 2).max(2);
            notes.push(format!("{n} frames do not fit the texture budget: {fit} kept"));
            n = fit;
            let grid = arrange(n, dims, budget).ok_or_else(|| AnimError("no atlas fits the texture budget".into()))?;
            (FrameSize::Quarter, dims, grid)
        }
    };
    if policy.size != FrameSize::Auto && size != policy.size {
        notes.push(format!("{} frames at {} size do not fit the texture budget: {} used", n, policy.size.name(), size.name()));
    }
    if clip.frames.len() > n {
        notes.push(format!("{} frames resampled to {n} at {fps} a second", clip.frames.len()));
    }
    Ok(Plan { fps, picks: picks(clip, n), frame: dims, cols: grid.0, rows: grid.1, notes })
}

// ---------------------------------------------------------------- composing frames onto the deck

/// The wood at a frame's size: what shows around a clip that does not fill the deck.
pub fn backdrop(wood: &Image, width: usize, height: usize) -> Image {
    wood.to_rgba().resize_lanczos(width, height)
}

/// One frame on the deck (RGB, the backdrop's size): a frame wider than tall is turned a quarter turn clockwise
/// (its left edge becomes the top, as sideways art is), then fitted inside the deck and centred; the backdrop shows
/// through transparent parts and around it.
pub fn compose(frame: &Image, backdrop: &Image) -> Image {
    let (w, h) = (backdrop.width, backdrop.height);
    let mut frame = frame.to_rgba();
    if frame.width > frame.height {
        frame = frame.rotate_cw();
    }
    let scale = (w as f64 / frame.width as f64).min(h as f64 / frame.height as f64);
    let (fw, fh) = (((frame.width as f64 * scale).round() as usize).clamp(1, w), ((frame.height as f64 * scale).round() as usize).clamp(1, h));
    let fitted = frame.resize_lanczos(fw, fh);
    let (left, top) = ((w - fw) / 2, (h - fh) / 2);
    let mut layer = Image::new(w, h, 4);
    for y in 0..fh {
        let src = &fitted.data[y * fw * 4..(y + 1) * fw * 4];
        let at = ((top + y) * w + left) * 4;
        layer.data[at..at + fw * 4].copy_from_slice(src);
    }
    backdrop.alpha_composite(&layer).to_rgb()
}

/// The atlas (RGB, `plan.width()` x `plan.height()`) and the deck art (the first frame at 512 x 2048).
pub struct Atlas {
    pub image: Image,
    pub art: Image,
}

/// Lays the planned frames out in the grid (cells past the last frame repeat it) and composes the art.
pub fn atlas(clip: &Clip, plan: &Plan, wood: &Image) -> Result<Atlas, AnimError> {
    let (fw, fh) = plan.frame;
    let (w, h) = (plan.width(), plan.height());
    if plan.picks.is_empty() || plan.picks.iter().any(|&p| p >= clip.frames.len()) {
        return err("the plan names frames the clip does not have");
    }
    let small = backdrop(wood, fw, fh);
    let mut image = Image::new(w, h, 3);
    let mut cache: std::collections::HashMap<usize, Image> = std::collections::HashMap::new();
    let cells = plan.cols * plan.rows;
    for cell in 0..cells {
        let pick = plan.picks[cell.min(plan.picks.len() - 1)];
        let composed = cache.entry(pick).or_insert_with(|| compose(&clip.frames[pick], &small));
        let (cx, cy) = ((cell % plan.cols) * fw, (cell / plan.cols) * fh);
        for y in 0..fh {
            let src = &composed.data[y * fw * 3..(y + 1) * fw * 3];
            let at = ((cy + y) * w + cx) * 3;
            image.data[at..at + fw * 3].copy_from_slice(src);
        }
    }
    let art = compose(&clip.frames[plan.picks[0]], &backdrop(wood, effects::W, effects::H));
    Ok(Atlas { image, art })
}

// ---------------------------------------------------------------- the marker

/// Block 3 of the shader marker for an animated deck: R the frame count, G the frames a second, B the grid as
/// 32 x log2(columns) + 4 x log2(rows) (all even, so BC7 keeps them exact; the shader divides by 4 and splits by 8).
pub fn marker_block3(frames: usize, fps: u32, cols: usize, rows: usize) -> Result<[u8; 3], AnimError> {
    if frames < 2 || frames > MAX_FRAMES || frames % 2 == 1 {
        return err(format!("{frames} frames: an animated deck has an even count, 2 to {MAX_FRAMES}"));
    }
    if fps < MIN_FPS || fps > MAX_FPS || fps % 2 == 1 {
        return err(format!("{fps} frames a second: an even rate, {MIN_FPS} to {MAX_FPS}"));
    }
    if !cols.is_power_of_two() || cols > 32 || !rows.is_power_of_two() || rows > 32 {
        return err(format!("{cols} x {rows} cells: powers of two up to 32"));
    }
    Ok([frames as u8, fps as u8, (cols.trailing_zeros() * 32 + rows.trailing_zeros() * 4) as u8])
}

/// The atlas with the shader marker in its top-left corner: blocks 0 and 1 the signature, block 2 (EFFECT_ANIM,
/// 0, glow), block 3 the animation's numbers. The deck programs read the blocks at fixed texels (8, 8), (24, 8),
/// (40, 8) and (56, 8), so the marker is the same 16 x 16 texel blocks whatever the atlas's size.
pub fn mark_atlas(atlas: &Image, frames: usize, fps: u32, cols: usize, rows: usize, glow: u8) -> Result<Image, AnimError> {
    let block3 = marker_block3(frames, fps, cols, rows)?;
    let mut out = effects::mark(atlas, EFFECT_ANIM, 0, glow);
    for y in 0..16 {
        for x in 48..64 {
            out.pixel_mut(x, y).copy_from_slice(&block3);
        }
    }
    Ok(out)
}

/// Whether a holo map carries the animated marker (its effect code).
pub fn is_marked(image: &Image) -> bool {
    image.width >= 64 && image.height >= 16 && image.pixel(40, 8)[0] == EFFECT_ANIM
}

// ---------------------------------------------------------------- GIF

/// The frames of an animated GIF, composed as a viewer shows them (disposal to background or to the previous frame
/// honoured; transparent pixels stay transparent so the deck's wood shows through).
pub fn decode_gif(bytes: &[u8]) -> Result<Clip, AnimError> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    options.set_memory_limit(gif::MemoryLimit::Unlimited);
    let mut decoder = options.read_info(std::io::Cursor::new(bytes)).map_err(|e| AnimError(format!("not a readable GIF: {e}")))?;
    let (w, h) = (decoder.width() as usize, decoder.height() as usize);
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return err(format!("a {w} x {h} GIF is not something a deck can use"));
    }
    let mut canvas = vec![0u8; w * h * 4];
    let (mut frames, mut delays) = (Vec::new(), Vec::new());
    loop {
        let frame = match decoder.read_next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                if frames.is_empty() {
                    return err(format!("not a readable GIF: {e}"));
                }
                break; // a truncated file: what was read plays
            }
        };
        if frames.len() >= 1024 {
            break;
        }
        let before = if frame.dispose == gif::DisposalMethod::Previous { Some(canvas.clone()) } else { None };
        let (fx, fy, fw, fh) = (frame.left as usize, frame.top as usize, frame.width as usize, frame.height as usize);
        for y in 0..fh {
            if fy + y >= h {
                break;
            }
            for x in 0..fw {
                if fx + x >= w {
                    break;
                }
                let s = (y * fw + x) * 4;
                let Some(px) = frame.buffer.get(s..s + 4) else { continue };
                if px[3] == 0 {
                    continue;
                }
                let d = ((fy + y) * w + fx + x) * 4;
                canvas[d..d + 4].copy_from_slice(px);
            }
        }
        frames.push(Image { width: w, height: h, channels: 4, data: canvas.clone() });
        let delay = frame.delay as u32 * 10;
        delays.push(if delay < MIN_GIF_DELAY_MS { SLOW_GIF_DELAY_MS } else { delay });
        match frame.dispose {
            gif::DisposalMethod::Background => {
                for y in fy..(fy + fh).min(h) {
                    for x in fx..(fx + fw).min(w) {
                        let d = (y * w + x) * 4;
                        canvas[d..d + 4].copy_from_slice(&[0, 0, 0, 0]);
                    }
                }
            }
            gif::DisposalMethod::Previous => {
                if let Some(b) = before {
                    canvas = b;
                }
            }
            _ => {}
        }
    }
    if frames.is_empty() {
        return err("the GIF has no frames");
    }
    Ok(Clip { width: w, height: h, frames, delays_ms: delays })
}

/// Writes a clip as a looping GIF (the test animation; 256 colours a frame).
pub fn write_gif(clip: &Clip, path: &Path) -> Result<(), AnimError> {
    let file = std::fs::File::create(path).map_err(|e| AnimError(format!("{}: {e}", path.display())))?;
    let mut out = std::io::BufWriter::new(file);
    let mut encoder = gif::Encoder::new(&mut out, clip.width as u16, clip.height as u16, &[]).map_err(|e| AnimError(e.to_string()))?;
    encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| AnimError(e.to_string()))?;
    for (frame, delay) in clip.frames.iter().zip(&clip.delays_ms) {
        let mut rgba = frame.to_rgba().data;
        let mut f = gif::Frame::from_rgba_speed(clip.width as u16, clip.height as u16, &mut rgba, 10);
        f.delay = (delay / 10).clamp(2, u16::MAX as u32) as u16;
        encoder.write_frame(&f).map_err(|e| AnimError(e.to_string()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- video through ffmpeg

/// `ffmpeg` and `ffprobe` on PATH, when they are there (Caveman never downloads them).
pub fn ffmpeg_tools() -> Option<(PathBuf, PathBuf)> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for exe in ["ffmpeg.exe", "ffmpeg"] {
            let f = dir.join(exe);
            if f.is_file() {
                let probe = dir.join(exe.replace("ffmpeg", "ffprobe"));
                if probe.is_file() {
                    return Some((f, probe));
                }
            }
        }
    }
    None
}

/// File extensions Caveman hands to ffmpeg.
pub const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "webm", "mov", "mkv", "avi", "m4v"];

/// What ffprobe says about a video's first stream.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    pub duration_ms: u32,
}

fn run(cmd: &mut Command, what: &str) -> Result<Vec<u8>, AnimError> {
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::piped()).stdout(Stdio::piped()).output().map_err(|e| AnimError(format!("{what}: {e}")))?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr);
        return err(format!("{what} failed: {}", text.lines().last().unwrap_or("no output")));
    }
    Ok(out.stdout)
}

pub fn video_probe(probe: &Path, file: &Path) -> Result<VideoInfo, AnimError> {
    let out = run(
        Command::new(probe).args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,avg_frame_rate,r_frame_rate,duration", "-show_entries", "format=duration", "-of", "json"]).arg(file),
        "ffprobe",
    )?;
    let v: serde_json::Value = serde_json::from_slice(&out).map_err(|e| AnimError(format!("ffprobe output: {e}")))?;
    let stream = v.get("streams").and_then(|s| s.get(0)).ok_or_else(|| AnimError("ffprobe found no video stream".into()))?;
    let num = |k: &str| stream.get(k).and_then(|x| x.as_u64()).map(|x| x as usize);
    let (width, height) = (num("width").unwrap_or(0), num("height").unwrap_or(0));
    if width == 0 || height == 0 {
        return err("ffprobe found no frame size");
    }
    let rate = |k: &str| -> Option<f64> {
        let text = stream.get(k)?.as_str()?;
        let (a, b) = text.split_once('/').unwrap_or((text, "1"));
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        (b > 0.0 && a > 0.0).then_some(a / b)
    };
    let fps = rate("avg_frame_rate").or_else(|| rate("r_frame_rate")).unwrap_or(24.0);
    let seconds = |x: Option<&serde_json::Value>| x.and_then(|d| d.as_str()).and_then(|d| d.parse::<f64>().ok()).filter(|d| *d > 0.0);
    let duration = seconds(stream.get("duration")).or_else(|| seconds(v.get("format").and_then(|f| f.get("duration")))).unwrap_or(0.0);
    Ok(VideoInfo { width, height, fps, duration_ms: (duration * 1000.0).round() as u32 })
}

/// `frames` frames of a video at `fps`, decoded by ffmpeg as RGBA (scaled so the longer side is at most 2048).
pub fn decode_video(ffmpeg: &Path, file: &Path, info: &VideoInfo, fps: u32, frames: usize, max_seconds: f64) -> Result<Clip, AnimError> {
    let scale = (MAX_SOURCE_SIDE as f64 / info.width.max(info.height) as f64).min(1.0);
    let even = |v: f64| (((v * scale).round() as usize).max(2) / 2 * 2).to_string();
    let (w, h): (usize, usize) = (even(info.width as f64).parse().unwrap(), even(info.height as f64).parse().unwrap());
    let filter = format!("fps={fps},scale={w}:{h}:flags=lanczos");
    let out = run(
        Command::new(ffmpeg)
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(file)
            .args(["-t", &format!("{max_seconds:.3}"), "-vf", &filter, "-frames:v", &frames.to_string(), "-f", "rawvideo", "-pix_fmt", "rgba", "-"]),
        "ffmpeg",
    )?;
    let frame_bytes = w * h * 4;
    let count = out.len() / frame_bytes;
    if count == 0 {
        return err("ffmpeg decoded no frames");
    }
    let frames: Vec<Image> = (0..count).map(|k| Image { width: w, height: h, channels: 4, data: out[k * frame_bytes..(k + 1) * frame_bytes].to_vec() }).collect();
    let delay = (1000.0 / fps as f64).round() as u32;
    Ok(Clip { width: w, height: h, delays_ms: vec![delay; count], frames })
}

/// A video file as a clip under `policy`: probed, its timing chosen, then exactly the planned frames decoded.
pub fn load_video(file: &Path, policy: &Policy) -> Result<Clip, AnimError> {
    let (ffmpeg, probe) = ffmpeg_tools().ok_or_else(|| AnimError("video decks need ffmpeg and ffprobe on PATH (ffmpeg.org); a GIF needs nothing".into()))?;
    let info = video_probe(&probe, file)?;
    let max_ms = (policy.max_seconds.max(0.5) * 1000.0) as u32;
    let duration = if info.duration_ms == 0 { max_ms } else { info.duration_ms.min(max_ms) };
    let (fps, n) = timing(duration, info.fps, policy.fps, policy.max_frames);
    decode_video(&ffmpeg, file, &info, fps, n, duration as f64 / 1000.0)
}

/// Whether a file name is a clip Caveman reads: a GIF, or a video when ffmpeg is on PATH (the caller decides how to
/// report a video without it).
pub fn clip_kind(file_name: &str) -> Option<&'static str> {
    let lower = file_name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next()?;
    if ext == "gif" {
        return Some("gif");
    }
    VIDEO_EXTENSIONS.contains(&ext).then_some("video")
}

/// A GIF or video file as a clip.
pub fn load_clip(file: &Path, policy: &Policy) -> Result<Clip, AnimError> {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match clip_kind(&name) {
        Some("gif") => decode_gif(&std::fs::read(file).map_err(|e| AnimError(format!("{}: {e}", file.display())))?),
        Some(_) => load_video(file, policy),
        None => err(format!("{name}: not a GIF or video")),
    }
}

// ---------------------------------------------------------------- the test animation

/// A 5 x 7 bitmap font for the test animation's labels (digits, F P S and a slash).
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        _ => [0; 7],
    }
}

/// Draws `text` with its top-left at (x, y), each font pixel `scale` wide, in `colour`.
fn draw_text(img: &mut Image, text: &str, x: usize, y: usize, scale: usize, colour: [u8; 3]) {
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) = (x + (i * 6 + col) * scale + dx, y + row * scale + dy);
                        if px < img.width && py < img.height {
                            img.pixel_mut(px, py).copy_from_slice(&colour);
                        }
                    }
                }
            }
        }
    }
}

/// Width of `text` drawn at `scale`.
fn text_width(text: &str, scale: usize) -> usize {
    (text.chars().count() * 6).saturating_sub(1) * scale
}

fn hue_rgb(h: f64) -> [u8; 3] {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let i = h6.floor() as i64;
    let f = h6 - i as f64;
    let (q, t) = (1.0 - f, f);
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (1.0, t, 0.0),
        1 => (q, 1.0, 0.0),
        2 => (0.0, 1.0, t),
        3 => (0.0, q, 1.0),
        4 => (t, 0.0, 1.0),
        _ => (1.0, 0.0, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

/// A clip that makes the frame and the speed obvious in game: `frames` frames at `fps`, each with its number in big
/// digits, "k/N" and "F FPS" under it, a rainbow bar sweeping from the top of the deck to the bottom once a loop, and
/// a tick gauge on the left that fills as the loop plays. Portrait, `width` x `height` (1:4 fits the deck exactly).
pub fn test_clip(frames: usize, fps: u32, width: usize, height: usize) -> Clip {
    let n = frames.max(1);
    let mut out = Vec::with_capacity(n);
    let scale_big = (width / 24).max(2);
    let scale_small = (width / 64).max(1);
    for k in 0..n {
        let mut img = Image::filled(width, height, &[22, 22, 26]);
        // a border and a nose mark so the orientation is plain
        let border = (width / 64).max(1);
        for y in 0..height {
            for x in 0..width {
                let edge = x < border || y < border || x >= width - border || y >= height - border;
                let nose = y < height / 40 && (x as i64 - width as i64 / 2).abs() < (y as i64 + 1) * 3;
                if edge {
                    img.pixel_mut(x, y).copy_from_slice(&[90, 90, 96]);
                } else if nose {
                    img.pixel_mut(x, y).copy_from_slice(&[255, 255, 255]);
                }
            }
        }
        // the sweeping bar
        let bar_h = (height / 32).max(2);
        let top = (k as f64 / n as f64 * (height - bar_h) as f64) as usize;
        let colour = hue_rgb(k as f64 / n as f64);
        for y in top..top + bar_h {
            for x in border..width - border {
                img.pixel_mut(x, y).copy_from_slice(&colour);
            }
        }
        // the gauge: n ticks down the left edge, the first k + 1 lit
        let tick_h = ((height * 3 / 5) / n).max(2);
        let gauge_top = height / 5;
        for t in 0..n {
            let lit = t <= k;
            let c = if lit { [255, 220, 40] } else { [60, 60, 66] };
            for y in gauge_top + t * tick_h..gauge_top + t * tick_h + tick_h.saturating_sub(1).max(1) {
                for x in border * 2..border * 2 + width / 12 {
                    if y < height {
                        img.pixel_mut(x, y).copy_from_slice(&c);
                    }
                }
            }
        }
        // the labels
        let big = (k + 1).to_string();
        let bx = (width - text_width(&big, scale_big)) / 2;
        draw_text(&mut img, &big, bx, height / 4, scale_big, [255, 255, 255]);
        let small = format!("{}/{}", k + 1, n);
        let sx = (width - text_width(&small, scale_small)) / 2;
        draw_text(&mut img, &small, sx, height / 4 + 9 * scale_big, scale_small, [200, 200, 210]);
        let rate = format!("{fps} FPS");
        let rx = (width - text_width(&rate, scale_small)) / 2;
        draw_text(&mut img, &rate, rx, height / 4 + 9 * scale_big + 9 * scale_small, scale_small, colour);
        out.push(img.to_rgba());
    }
    let delay = (1000.0 / fps.max(1) as f64).round() as u32;
    Clip { width, height, delays_ms: vec![delay; n], frames: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(n: usize, delay: u32, w: usize, h: usize) -> Clip {
        Clip { width: w, height: h, frames: (0..n).map(|k| Image::filled(w, h, &[k as u8, 0, 0, 255])).collect(), delays_ms: vec![delay; n] }
    }

    #[test]
    fn timing_is_even_and_capped() {
        assert_eq!(timing(2000, 10.0, None, 64), (10, 20));
        assert_eq!(timing(1000, 25.0, None, 64), (24, 24));
        // 300 frames at 30 fps (10 s) capped to 64: a lower rate keeps the whole loop
        let (fps, n) = timing(10_000, 30.0, None, 64);
        assert!(fps % 2 == 0 && n <= 64 && n % 2 == 0, "{fps} {n}");
        assert!((n as f64 / fps as f64 - 10.0).abs() < 1.0);
        assert_eq!(timing(500, 2.0, Some(12.0), 64), (12, 6));
        assert_eq!(timing(100, 10.0, None, 64).1, 2);
    }

    #[test]
    fn plans_fit_the_budget() {
        let p = plan(&clip(8, 500, 100, 400), &Policy::default()).unwrap();
        assert_eq!((p.fps, p.frames(), p.frame, p.cols, p.rows), (2, 8, (512, 2048), 4, 2));
        assert_eq!((p.width(), p.height()), (2048, 4096));
        let p = plan(&clip(24, 83, 100, 400), &Policy::default()).unwrap();
        assert_eq!((p.fps, p.frames(), p.frame), (12, 24, (256, 1024)));
        assert_eq!((p.width(), p.height()), (2048, 4096));
        let p = plan(&clip(200, 40, 100, 400), &Policy { max_frames: 254, ..Policy::default() }).unwrap();
        assert_eq!(p.frame, (128, 512));
        assert!(p.width() * p.height() <= DEFAULT_BUDGET);
        // 25 fps becomes 24 (even): 192 frames keep the 8 s loop
        assert_eq!((p.fps, p.frames()), (24, 192));
        assert!((p.loop_seconds() - 8.0).abs() < 0.01);
        // asking for full frames with too many of them steps down with a note
        let p = plan(&clip(40, 100, 100, 400), &Policy { size: FrameSize::Full, ..Policy::default() }).unwrap();
        assert_eq!(p.frame, (128, 512));
        assert!(p.notes.iter().any(|n| n.contains("do not fit")));
    }

    #[test]
    fn picks_follow_the_clock() {
        let mut c = clip(3, 100, 4, 16);
        c.delays_ms = vec![100, 300, 100];
        assert_eq!(picks(&c, 10), vec![0, 0, 1, 1, 1, 1, 1, 1, 2, 2]);
    }

    #[test]
    fn marker_numbers() {
        assert_eq!(marker_block3(24, 12, 4, 8).unwrap(), [24, 12, 76]);
        assert_eq!(marker_block3(2, 2, 1, 1).unwrap(), [2, 2, 0]);
        assert_eq!(marker_block3(254, 60, 32, 32).unwrap(), [254, 60, 180]);
        assert!(marker_block3(25, 12, 4, 8).is_err());
        assert!(marker_block3(24, 13, 4, 8).is_err());
        assert!(marker_block3(24, 12, 3, 8).is_err());
        let a = mark_atlas(&Image::new(1024, 4096, 3), 24, 12, 4, 8, 0).unwrap();
        assert_eq!(a.pixel(40, 8), &[EFFECT_ANIM, 0, 0]);
        assert_eq!(a.pixel(56, 8), &[24, 12, 76]);
        assert!(is_marked(&a));
        assert!(!is_marked(&Image::new(512, 2048, 3)));
    }

    #[test]
    fn marker_survives_bc7() {
        // the marker blocks are solid 4 x 4 aligned blocks of even values: BC7 mode 6 keeps them exact
        let a = mark_atlas(&Image::filled(256, 64, &[200, 90, 30]), 24, 12, 4, 8, 128).unwrap();
        let blocks = crate::bc7::encode(&a);
        let back = crate::bcdec::decode(crate::bcdec::BcFormat::Bc7, &blocks, 256, 64);
        for (x, want) in [(8usize, [255u8, 0, 255]), (24, [0, 255, 0]), (40, [EFFECT_ANIM, 0, 128]), (56, [24, 12, 76])] {
            let got = back.pixel(x, 8);
            for c in 0..3 {
                assert!((got[c] as i32 - want[c] as i32).abs() <= 1, "block at {x}: {got:?} vs {want:?}");
            }
            if x >= 40 {
                assert_eq!(&got[..3], &want, "block at {x}");
            }
        }
    }

    #[test]
    fn gif_round_trip() {
        let c = test_clip(6, 10, 64, 256);
        let dir = std::env::temp_dir().join(format!("caveman-anim-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.gif");
        write_gif(&c, &path).unwrap();
        let back = decode_gif(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((back.width, back.height, back.frames.len()), (64, 256, 6));
        assert_eq!(back.delays_ms, vec![100; 6]);
        // the bar moves down: its row in frame 0 is near the top, in the last frame near the bottom
        // the bar is the one saturated colour on the centre column (the digits are white, the rest grey)
        let bar_row = |img: &Image| {
            (0..img.height).find(|&y| {
                let p = img.pixel(32, y);
                p[..3].iter().max().unwrap() > &200 && p[..3].iter().min().unwrap() < &60
            })
        };
        assert!(bar_row(&back.frames[0]).unwrap() < bar_row(&back.frames[5]).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_contains_and_turns() {
        let wood = Image::filled(512, 2048, &[180, 140, 90]);
        let small = backdrop(&wood, 128, 512);
        // a landscape frame is turned upright and fitted to the width
        let frame = Image::filled(160, 90, &[0, 0, 255, 255]);
        let out = compose(&frame, &small);
        assert_eq!((out.width, out.height), (128, 512));
        assert_eq!(out.pixel(64, 256), &[0, 0, 255]);
        assert_eq!(out.pixel(64, 10), &[180, 140, 90]);
        let c = Clip { width: 160, height: 90, frames: vec![frame.clone(), Image::filled(160, 90, &[255, 0, 0, 255])], delays_ms: vec![500, 500] };
        let p = plan(&c, &Policy { size: FrameSize::Quarter, ..Policy::default() }).unwrap();
        assert_eq!((p.frames(), p.cols, p.rows), (2, 2, 1));
        let a = atlas(&c, &p, &wood).unwrap();
        assert_eq!((a.image.width, a.image.height), (256, 512));
        assert_eq!(a.image.pixel(64, 256), &[0, 0, 255]);
        assert_eq!(a.image.pixel(192, 256), &[255, 0, 0]);
        assert_eq!((a.art.width, a.art.height), (512, 2048));
        assert_eq!(a.art.pixel(256, 1024), &[0, 0, 255]);
    }

    #[test]
    fn kinds() {
        assert_eq!(clip_kind("Cool.GIF"), Some("gif"));
        assert_eq!(clip_kind("Cool.mp4"), Some("video"));
        assert_eq!(clip_kind("Cool.png"), None);
    }
}
