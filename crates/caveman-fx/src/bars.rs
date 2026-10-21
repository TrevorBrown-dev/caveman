//! The bars: Caveman FX's first "DLL draws" effect (Trevor's stream gimmick). A row of short, rounded, glowing bars
//! sticks out sideways from both long edges of the skateboard deck, spaced along it between the trucks, in the board's
//! plane, moving like an audio equaliser; a landing shoots them out extra far, then they settle over about a second.
//!
//! - [`BarSettings`]: what the trainer sets (`caveman-trainer bars ...`), saved as JSON;
//! - [`LevelSource`]: where the per-bar levels come from: [`RandomSource`] (a lively random rhythm) or the PC's audio
//!   output ([`AudioSource`], fed by [`crate::audio`]: WASAPI loopback split into bands);
//! - [`Bars`]: the levels smoothed like a meter (fast attack, slower release) plus the landing boost;
//! - the geometry: [`build`] turns the scene (camera, board) and the bar lengths into clip-space quads for the overlay
//!   ([`crate::overlay`]). Everything here is plain math, tested offline.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- settings

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Random,
    Audio,
}

/// Which way each bar's quad faces: in the board's plane (Trevor's sketch), or turned to the camera (always seen
/// face on, never edge on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Facing {
    Board,
    Camera,
}

/// The board's long axis in its transform: z (forward, the default) or x.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Z,
    X,
}

/// How to write colours into the back buffer: from its format and the display (auto), or forced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HdrMode {
    Auto,
    Sdr,
    Hdr10,
    Scrgb,
}

/// Everything the trainer can set. Lengths are in metres (the game's units).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BarSettings {
    /// Draw the bars.
    pub enabled: bool,
    /// `#rrggbb`.
    pub colour: String,
    /// Bars per side.
    pub count: u32,
    /// How far the levels push the bars out (1 = `length` at full level).
    pub gain: f32,
    pub source: Source,
    /// The same levels on both sides (a mirror image), or each side its own.
    pub mirror: bool,
    /// The length at full level (metres, beyond `base`).
    pub length: f32,
    /// The length at level 0.
    pub base: f32,
    /// Each bar's thickness.
    pub width: f32,
    /// The glow around each bar.
    pub glow: f32,
    /// How much of the deck's length the row covers (between the trucks).
    pub span: f32,
    /// Where the bars start: the deck's half width.
    pub edge: f32,
    /// Moves the row along the board's up axis (the transform's origin may not be the deck's top).
    pub height: f32,
    /// Moves the row along the board's long axis.
    pub shift: f32,
    pub axis: Axis,
    pub facing: Facing,
    /// The landing spike: extra length as a share of `length` (0 turns it off).
    pub land: f32,
    /// How long the spike takes to settle (milliseconds).
    pub land_ms: f32,
    /// How bright the colour's full value is on an HDR display, as a multiple of 200 nits.
    pub brightness: f32,
    pub hdr: HdrMode,
    /// How lively the random rhythm is (1 normal; higher changes faster).
    pub tempo: f32,
}

impl Default for BarSettings {
    fn default() -> Self {
        BarSettings {
            enabled: false,
            colour: "#ffd21f".into(),
            count: 8,
            gain: 1.0,
            source: Source::Random,
            mirror: true,
            length: 0.30,
            base: 0.025,
            width: 0.032,
            glow: 0.03,
            span: 0.40,
            edge: 0.105,
            height: 0.0,
            shift: 0.0,
            axis: Axis::Z,
            facing: Facing::Board,
            land: 1.5,
            land_ms: 1000.0,
            brightness: 2.0,
            hdr: HdrMode::Auto,
            tempo: 1.0,
        }
    }
}

pub const MAX_COUNT: u32 = 32;

/// `#rrggbb` (or `rrggbb`, or `0xrrggbb`) as 0..1 floats.
pub fn parse_colour(text: &str) -> Option<[f32; 3]> {
    let t = text.trim();
    let t = t.strip_prefix('#').or_else(|| t.strip_prefix("0x")).unwrap_or(t);
    if t.len() != 6 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some([((v >> 16) & 0xFF) as f32 / 255.0, ((v >> 8) & 0xFF) as f32 / 255.0, (v & 0xFF) as f32 / 255.0])
}

impl BarSettings {
    /// Checked and clamped to sane ranges; Err names what is wrong.
    pub fn sanitized(mut self) -> Result<BarSettings, String> {
        let c = parse_colour(&self.colour).ok_or_else(|| format!("colour {:?} is not #rrggbb", self.colour))?;
        self.colour = format!("#{:02x}{:02x}{:02x}", (c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8);
        let finite = [self.gain, self.length, self.base, self.width, self.glow, self.span, self.edge, self.height, self.shift, self.land, self.land_ms, self.brightness, self.tempo];
        if finite.iter().any(|v| !v.is_finite()) {
            return Err("every number must be finite".into());
        }
        self.count = self.count.clamp(1, MAX_COUNT);
        self.gain = self.gain.clamp(0.0, 10.0);
        self.length = self.length.clamp(0.0, 3.0);
        self.base = self.base.clamp(0.0, 1.0);
        self.width = self.width.clamp(0.002, 0.5);
        self.glow = self.glow.clamp(0.0, 0.5);
        self.span = self.span.clamp(0.0, 3.0);
        self.edge = self.edge.clamp(0.0, 1.0);
        self.height = self.height.clamp(-1.0, 1.0);
        self.shift = self.shift.clamp(-1.0, 1.0);
        self.land = self.land.clamp(0.0, 10.0);
        self.land_ms = self.land_ms.clamp(50.0, 10_000.0);
        self.brightness = self.brightness.clamp(0.1, 20.0);
        self.tempo = self.tempo.clamp(0.1, 10.0);
        Ok(self)
    }

    /// These settings with the fields of `patch` (a JSON object, as the trainer sends) applied.
    pub fn patched(&self, patch: &serde_json::Value) -> Result<BarSettings, String> {
        let obj = patch.as_object().ok_or("the settings must be a JSON object")?;
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        for (k, x) in obj {
            if v.get(k).is_none() {
                return Err(format!("no setting {k:?} (settings: {})", v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()));
            }
            v[k] = x.clone();
        }
        let s: BarSettings = serde_json::from_value(v).map_err(|e| e.to_string())?;
        s.sanitized()
    }

    /// How many independent levels the row shows.
    pub fn levels_needed(&self) -> usize {
        self.count as usize * if self.mirror { 1 } else { 2 }
    }
}

// ---------------------------------------------------------------- levels

/// Where the bars' levels come from. One value per bar, 0 to 1 (more is allowed: louder than full).
pub trait LevelSource: Send {
    /// Fills `out` for time `now` (seconds); `dt` is the time since the last call.
    fn levels(&mut self, now: f64, dt: f32, tempo: f32, out: &mut [f32]);
    fn source(&self) -> Source;
}

/// A small fast generator (xorshift64*), seeded once.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A lively equaliser without music: a random kick at a dance tempo pumps every bar, and each bar wanders between
/// new random targets every 60 to 200 ms, the low bars (the tail end) heavier than the high ones.
pub struct RandomSource {
    rng: Rng,
    targets: Vec<f32>,
    next_at: Vec<f64>,
    beat_at: f64,
    beat: f32,
}

impl RandomSource {
    pub fn new(seed: u64) -> RandomSource {
        RandomSource { rng: Rng::new(seed), targets: Vec::new(), next_at: Vec::new(), beat_at: 0.0, beat: 0.0 }
    }
}

impl LevelSource for RandomSource {
    fn levels(&mut self, now: f64, dt: f32, tempo: f32, out: &mut [f32]) {
        let n = out.len();
        if self.targets.len() != n {
            self.targets = vec![0.3; n];
            self.next_at = vec![now; n];
        }
        // the kick: every 0.42-0.5 s (about 125 BPM), sometimes skipped, sometimes doubled
        if now >= self.beat_at {
            let r = self.rng.unit();
            self.beat = if r < 0.12 { 0.35 } else { 1.0 };
            let gap = if r > 0.9 { 0.23 } else { 0.42 + 0.08 * self.rng.unit() as f64 };
            self.beat_at = now + gap / tempo as f64;
        }
        self.beat *= (-dt * 7.0 * tempo).exp();
        for k in 0..n {
            if now >= self.next_at[k] {
                let low = if n > 1 { 1.0 - k as f32 / (n - 1) as f32 } else { 1.0 };
                let r = self.rng.unit();
                self.targets[k] = (0.12 + 0.6 * r * r + 0.15 * low).min(1.0);
                self.next_at[k] = now + (0.06 + 0.14 * self.rng.unit() as f64) / tempo as f64;
            }
            let low = if n > 1 { 1.0 - k as f32 / (n - 1) as f32 } else { 1.0 };
            out[k] = (self.targets[k] + self.beat * (0.25 + 0.35 * low)).min(1.2);
        }
    }

    fn source(&self) -> Source {
        Source::Random
    }
}

/// Levels from the PC's audio output (the bands [`crate::audio`] publishes); silence when the capture is not running.
pub struct AudioSource;

impl LevelSource for AudioSource {
    fn levels(&mut self, _now: f64, _dt: f32, _tempo: f32, out: &mut [f32]) {
        crate::audio::bands(out);
    }

    fn source(&self) -> Source {
        Source::Audio
    }
}

/// The bars' live state: smoothed levels and the landing boost.
pub struct Bars {
    source: Box<dyn LevelSource>,
    raw: Vec<f32>,
    levels: Vec<f32>,
    land_at: Option<f64>,
    landings: u32,
    last: Option<f64>,
}

impl Default for Bars {
    fn default() -> Self {
        Bars::new()
    }
}

impl Bars {
    pub fn new() -> Bars {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x1234_5678);
        Bars { source: Box::new(RandomSource::new(seed)), raw: Vec::new(), levels: Vec::new(), land_at: None, landings: 0, last: None }
    }

    /// Starts a landing spike now (the trainer's `bars land`, or a landing seen by the rules engine).
    pub fn land(&mut self, now: f64) {
        self.land_at = Some(now);
    }

    /// Advances to `now` (seconds). `landings` is the count of landings seen so far: a change starts a spike.
    pub fn update(&mut self, s: &BarSettings, now: f64, landings: u32) {
        if landings != self.landings {
            self.landings = landings;
            self.land_at = Some(now);
        }
        if self.source.source() != s.source {
            self.source = match s.source {
                Source::Random => Box::new(RandomSource::new(now.to_bits() ^ 0x9E37_79B9_7F4A_7C15)),
                Source::Audio => Box::new(AudioSource),
            };
        }
        let n = s.levels_needed();
        if self.levels.len() != n {
            self.levels = vec![0.0; n];
            self.raw = vec![0.0; n];
        }
        let dt = self.last.map_or(1.0 / 60.0, |t| (now - t).clamp(0.0, 0.25) as f32);
        self.last = Some(now);
        self.source.levels(now, dt, s.tempo, &mut self.raw);
        for (level, &target) in self.levels.iter_mut().zip(&self.raw) {
            let target = if target.is_finite() { target.clamp(0.0, 2.0) } else { 0.0 };
            let tau = if target > *level { 0.035 } else { 0.12 };
            *level += (target - *level) * (1.0 - (-dt / tau).exp());
        }
    }

    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    /// The landing boost of bar `k` of `n` at `now`: rises in 30 ms, decays so it has settled after `land_ms`; it
    /// starts in the middle of the row and ripples out to the ends.
    pub fn boost(&self, s: &BarSettings, k: usize, n: usize, now: f64) -> f32 {
        let Some(at) = self.land_at else { return 0.0 };
        let centre = (n as f32 - 1.0) * 0.5;
        let delay = 0.018 * (k as f32 - centre).abs();
        let t = (now - at) as f32 - delay;
        if t <= 0.0 {
            return 0.0;
        }
        let tau = s.land_ms / 1000.0 / 3.0;
        s.land * (1.0 - (-t / 0.03).exp()) * (-t / tau).exp()
    }

    /// The length of each bar (metres): `count` per side, the second side after the first.
    pub fn lengths(&self, s: &BarSettings, now: f64) -> Vec<f32> {
        let n = s.count as usize;
        let mut out = Vec::with_capacity(n * 2);
        for side in 0..2 {
            for k in 0..n {
                let li = if s.mirror { k } else { side * n + k };
                let level = self.levels.get(li).copied().unwrap_or(0.0);
                let len = s.base + s.length * (s.gain * level + self.boost(s, k, n, now));
                out.push(len.clamp(0.0, 4.0));
            }
        }
        out
    }
}

// ---------------------------------------------------------------- geometry

pub type V3 = [f32; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}
pub fn normalize(a: V3) -> Option<V3> {
    let l = length(a);
    (l.is_finite() && l > 1e-6).then(|| mul(a, 1.0 / l))
}

/// A Frostbite world matrix as the game keeps it: four rows of four floats (the fourth lanes are not the matrix's);
/// rows 0 to 2 the axes, row 3 the position.
pub fn rows(m: &[f32; 16]) -> [V3; 4] {
    [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]], [m[12], m[13], m[14]]]
}

/// Whether a world matrix is usable: finite, its axes about unit length, its position within the world.
pub fn valid_matrix(m: &[f32; 16]) -> bool {
    let r = rows(m);
    r.iter().flatten().all(|v| v.is_finite()) && r[..3].iter().all(|a| (0.9..1.1).contains(&length(*a))) && r[3].iter().all(|v| v.abs() < 1.0e6)
}

/// The camera: its world matrix rows (right, up, back, position) and vertical field of view in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub right: V3,
    pub up: V3,
    pub back: V3,
    pub pos: V3,
    pub fov_y: f32,
}

impl Camera {
    pub fn from_matrix(m: &[f32; 16], fov_y: f32) -> Option<Camera> {
        if !valid_matrix(m) || !(1.0..175.0).contains(&fov_y) {
            return None;
        }
        let r = rows(m);
        Some(Camera { right: normalize(r[0])?, up: normalize(r[1])?, back: normalize(r[2])?, pos: r[3], fov_y })
    }
}

/// The board: its world matrix rows (right, up, forward, position).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Board {
    pub right: V3,
    pub up: V3,
    pub forward: V3,
    pub pos: V3,
}

impl Board {
    pub fn from_matrix(m: &[f32; 16]) -> Option<Board> {
        if !valid_matrix(m) {
            return None;
        }
        let r = rows(m);
        Some(Board { right: normalize(r[0])?, up: normalize(r[1])?, forward: normalize(r[2])?, pos: r[3] })
    }
}

/// What the overlay needs from the game for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    pub camera: Camera,
    pub board: Option<Board>,
}

/// The near plane (metres): closer points are clipped.
pub const NEAR: f32 = 0.05;

/// A world point in clip space, for a camera and the back buffer's aspect ratio (width / height). Behind the camera
/// (or nearer than [`NEAR`]) w is below the near plane and the GPU clips it.
pub fn project(cam: &Camera, aspect: f32, p: V3) -> [f32; 4] {
    let d = sub(p, cam.pos);
    let x = dot(d, cam.right);
    let y = dot(d, cam.up);
    let z = -dot(d, cam.back);
    let f = 1.0 / (cam.fov_y.to_radians() * 0.5).tan();
    [x * f / aspect, y * f, z - NEAR, z]
}

/// One vertex of the overlay (64 bytes, the input layout of `shaders/overlay.hlsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    /// Clip space.
    pub pos: [f32; 4],
    /// The bar's frame: along, across (metres), the bar's length, its radius.
    pub local: [f32; 4],
    /// Colour (sRGB-encoded) and an alpha multiplier.
    pub color: [f32; 4],
    /// Glow width (metres), 1 for a solid quad.
    pub extra: [f32; 4],
}

/// Two triangles from four corners (00, 10, 01, 11).
fn quad(out: &mut Vec<Vertex>, c: [Vertex; 4]) {
    out.extend_from_slice(&[c[0], c[1], c[2], c[2], c[1], c[3]]);
}

/// The bars' quads for one frame. `lengths` from [`Bars::lengths`]; `aspect` the back buffer's. Nothing without a
/// board, or when the board is far from the camera (a menu's camera, a replay).
pub fn build(s: &BarSettings, scene: &Scene, aspect: f32, lengths: &[f32], out: &mut Vec<Vertex>) {
    let Some(board) = scene.board else { return };
    let cam = &scene.camera;
    if length(sub(board.pos, cam.pos)) > 60.0 {
        return;
    }
    let colour = parse_colour(&s.colour).unwrap_or([1.0, 0.82, 0.12]);
    let (long, across) = match s.axis {
        Axis::Z => (board.forward, board.right),
        Axis::X => (board.right, board.forward),
    };
    let n = s.count as usize;
    let r = s.width * 0.5;
    let g = s.glow;
    let pad = r + g;
    let centre = add(add(board.pos, mul(board.up, s.height)), mul(long, s.shift));
    for side in 0..2 {
        let sign = if side == 0 { 1.0 } else { -1.0 };
        let out_dir = mul(across, sign);
        for k in 0..n {
            let Some(&len) = lengths.get(side * n + k) else { continue };
            let t = if n > 1 { k as f32 / (n - 1) as f32 - 0.5 } else { 0.0 };
            let base = add(add(centre, mul(long, t * s.span)), mul(out_dir, s.edge));
            let width_dir = match s.facing {
                Facing::Board => long,
                Facing::Camera => normalize(cross(out_dir, sub(base, cam.pos))).unwrap_or(long),
            };
            let (u0, u1, v0, v1) = (-pad, len + pad, -pad, pad);
            let corner = |u: f32, v: f32| -> Vertex {
                let p = add(add(base, mul(out_dir, u)), mul(width_dir, v));
                Vertex { pos: project(cam, aspect, p), local: [u, v, len, r], color: [colour[0], colour[1], colour[2], 1.0], extra: [g.max(1e-4), 0.0, 0.0, 0.0] }
            };
            quad(out, [corner(u0, v0), corner(u1, v0), corner(u0, v1), corner(u1, v1)]);
        }
    }
}

/// The test rectangle: a solid quad in screen space (normalised device coordinates), pulsing gently.
pub fn test_rect(colour: [f32; 3], now: f64, out: &mut Vec<Vertex>) {
    let a = 0.75 + 0.25 * (now * 3.0).sin() as f32;
    let (x0, x1, y0, y1) = (-0.9f32, -0.5f32, 0.9f32, 0.6f32);
    let v = |x: f32, y: f32| Vertex { pos: [x, y, 0.5, 1.0], local: [0.0; 4], color: [colour[0], colour[1], colour[2], a], extra: [0.0, 1.0, 0.0, 0.0] };
    quad(out, [v(x0, y0), v(x1, y0), v(x0, y1), v(x1, y1)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_camera() -> Camera {
        // looking down -z (back = +z), y up, at the origin, 90 degrees
        Camera { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], back: [0.0, 0.0, 1.0], pos: [0.0, 0.0, 0.0], fov_y: 90.0 }
    }

    fn ndc(c: [f32; 4]) -> (f32, f32) {
        (c[0] / c[3], c[1] / c[3])
    }

    #[test]
    fn projection() {
        let cam = identity_camera();
        let c = project(&cam, 1.0, [0.0, 0.0, -5.0]);
        assert_eq!(ndc(c), (0.0, 0.0));
        assert!((c[3] - 5.0).abs() < 1e-6 && (c[2] - (5.0 - NEAR)).abs() < 1e-6);
        // 90 degrees: x = z at the screen's edge; aspect 2 halves x
        let (x, y) = ndc(project(&cam, 1.0, [5.0, 5.0, -5.0]));
        assert!((x - 1.0).abs() < 1e-5 && (y - 1.0).abs() < 1e-5);
        let (x, _) = ndc(project(&cam, 2.0, [5.0, 0.0, -5.0]));
        assert!((x - 0.5).abs() < 1e-5);
        // behind the camera: w below the near plane (the GPU clips it)
        assert!(project(&cam, 1.0, [0.0, 0.0, 3.0])[3] < NEAR);
        // ReSkate's nametag formula (focal = h / (2 tan(fov/2)); pixel = centre + side * focal / depth) agrees
        let cam = Camera { fov_y: 55.0, ..identity_camera() };
        let (w, h) = (1920.0f32, 1080.0f32);
        let p = [1.3, -0.4, -7.0];
        let (x, y) = ndc(project(&cam, w / h, p));
        let focal = h / (2.0 * (55.0f32.to_radians() / 2.0).tan());
        let px = w / 2.0 + p[0] * focal / 7.0;
        let py = h / 2.0 - p[1] * focal / 7.0;
        assert!(((x + 1.0) * w / 2.0 - px).abs() < 0.01, "{x} {px}");
        assert!(((1.0 - y) * h / 2.0 - py).abs() < 0.01, "{y} {py}");
    }

    #[test]
    fn matrices() {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[12] = 627.0;
        m[13] = 199.9;
        m[14] = 1062.8;
        m[3] = -1.17; // a fourth lane: not the matrix's, ignored
        assert!(valid_matrix(&m));
        let b = Board::from_matrix(&m).unwrap();
        assert_eq!(b.pos, [627.0, 199.9, 1062.8]);
        m[5] = 3.0;
        assert!(!valid_matrix(&m));
        m[5] = f32::NAN;
        assert!(Board::from_matrix(&m).is_none());
        assert!(Camera::from_matrix(&[0.0; 16], 55.0).is_none());
    }

    #[test]
    fn settings_patch_and_sanitize() {
        let s = BarSettings::default();
        let p = s.patched(&serde_json::json!({"gain": 2.5, "colour": "FF0080", "count": 99})).unwrap();
        assert_eq!(p.gain, 2.5);
        assert_eq!(p.colour, "#ff0080");
        assert_eq!(p.count, MAX_COUNT);
        assert!(s.patched(&serde_json::json!({"gian": 1})).unwrap_err().contains("no setting \"gian\""));
        assert!(s.patched(&serde_json::json!({"colour": "yellow"})).is_err());
        assert!(s.patched(&serde_json::json!({"source": "midi"})).is_err());
        assert_eq!(s.patched(&serde_json::json!({"source": "audio"})).unwrap().source, Source::Audio);
        // round trip, and an old file with fewer fields loads with defaults
        let text = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<BarSettings>(&text).unwrap(), p);
        let old: BarSettings = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(old.enabled && old.count == 8);
        assert_eq!(parse_colour("#ffd21f"), Some([1.0, 210.0 / 255.0, 31.0 / 255.0]));
    }

    #[test]
    fn levels_move_and_landing_spikes_then_settles() {
        let s = BarSettings { enabled: true, ..BarSettings::default() };
        let mut bars = Bars::new();
        let mut t = 0.0f64;
        let mut seen_min = f32::MAX;
        let mut seen_max = 0.0f32;
        let mut changes = 0;
        let mut last = Vec::new();
        for _ in 0..600 {
            t += 1.0 / 60.0;
            bars.update(&s, t, 0);
            let l = bars.levels().to_vec();
            assert_eq!(l.len(), 8);
            for &x in &l {
                seen_min = seen_min.min(x);
                seen_max = seen_max.max(x);
            }
            if l != last {
                changes += 1;
            }
            last = l;
        }
        assert!(seen_max > 0.6 && seen_min < 0.3, "lively: {seen_min}..{seen_max}");
        assert!(changes > 500);
        // a landing: the bars shoot out, then settle within land_ms
        let before = bars.lengths(&s, t);
        bars.update(&s, t, 1);
        let at = t;
        let mut peak = 0.0f32;
        while t < at + 0.15 {
            t += 1.0 / 60.0;
            bars.update(&s, t, 1);
            peak = peak.max(bars.boost(&s, 3, 8, t));
        }
        assert!(peak > 0.9 * s.land * 0.85, "peak {peak}");
        let during = bars.lengths(&s, t);
        assert!(during.iter().zip(&before).filter(|(d, b)| d > b).count() >= 12, "{during:?} vs {before:?}");
        assert!(bars.boost(&s, 3, 8, at + s.land_ms as f64 / 1000.0) < 0.06 * s.land);
        assert!(bars.boost(&s, 3, 8, at + 2.0) < 0.002);
        // the ripple: the middle starts before the ends
        assert!(bars.boost(&s, 3, 8, at + 0.02) > bars.boost(&s, 0, 8, at + 0.02));
        // mirror: both sides alike; without it, each side its own
        let l = bars.lengths(&s, t);
        assert_eq!(&l[..8], &l[8..]);
        let s2 = BarSettings { mirror: false, ..s.clone() };
        bars.update(&s2, t + 0.1, 1);
        for _ in 0..30 {
            t += 1.0 / 60.0;
            bars.update(&s2, t + 0.1, 1);
        }
        assert_eq!(bars.levels().len(), 16);
    }

    #[test]
    fn bars_stick_out_sideways_in_the_boards_plane() {
        // the camera 3 m above the board looking straight down: x right, -z up the screen
        let cam = Camera { right: [1.0, 0.0, 0.0], up: [0.0, 0.0, -1.0], back: [0.0, 1.0, 0.0], pos: [10.0, 3.0, 20.0], fov_y: 60.0 };
        let board = Board { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], forward: [0.0, 0.0, 1.0], pos: [10.0, 0.0, 20.0] };
        let scene = Scene { camera: cam, board: Some(board) };
        let s = BarSettings { enabled: true, count: 4, ..BarSettings::default() };
        let lengths = vec![0.2f32; 8];
        let mut v = Vec::new();
        build(&s, &scene, 1.0, &lengths, &mut v);
        assert_eq!(v.len(), 8 * 6);
        // every bar's quad lies at the board's height (w = 3 m, the distance straight down)
        for x in &v {
            assert!((x.pos[3] - 3.0).abs() < 1e-4);
        }
        // side 1 (+right) is right of the centre on screen, side 2 left; each reaches edge + length + glow
        let f = 1.0 / (30.0f32.to_radians()).tan();
        let max_x = v[..24].iter().map(|x| x.pos[0] / x.pos[3]).fold(f32::MIN, f32::max);
        let min_x = v[24..].iter().map(|x| x.pos[0] / x.pos[3]).fold(f32::MAX, f32::min);
        let reach = (s.edge + 0.2 + s.width * 0.5 + s.glow) * f / 3.0;
        assert!((max_x - reach).abs() < 1e-4, "{max_x} {reach}");
        assert!((min_x + reach).abs() < 1e-4);
        // spaced along the board (z, which is screen y here) over the span
        let ys: Vec<f32> = (0..4).map(|k| v[k * 6].pos[1] / v[k * 6].pos[3]).collect();
        assert!(ys.windows(2).all(|w| w[1] < w[0]), "along +z = down the screen: {ys:?}");
        // the local frame: u runs from -pad to length + pad, the length and radius ride along
        assert_eq!(v[1].local[0], 0.2 + s.width * 0.5 + s.glow);
        assert_eq!(v[0].local[2], 0.2);
        // axis x turns the row: now the bars stick out along z
        let mut w = Vec::new();
        build(&BarSettings { axis: Axis::X, ..s.clone() }, &scene, 1.0, &lengths, &mut w);
        let max_y = w[..24].iter().map(|x| x.pos[1] / x.pos[3]).fold(f32::MIN, f32::max);
        assert!(max_y.abs() < reach * 1.01 && max_y.abs() > reach * 0.5);
        // no board, or a far one: nothing
        let mut none = Vec::new();
        build(&s, &Scene { camera: cam, board: None }, 1.0, &lengths, &mut none);
        let far = Board { pos: [500.0, 0.0, 20.0], ..board };
        build(&s, &Scene { camera: cam, board: Some(far) }, 1.0, &lengths, &mut none);
        assert!(none.is_empty());
        // facing the camera: the quads' width runs across the view, never along it
        let side_cam = Camera { right: [0.0, 0.0, -1.0], up: [0.0, 1.0, 0.0], back: [-1.0, 0.0, 0.0], pos: [13.0, 0.0, 20.0], fov_y: 60.0 };
        let mut c = Vec::new();
        build(&BarSettings { facing: Facing::Camera, ..s.clone() }, &Scene { camera: side_cam, board: Some(board) }, 1.0, &lengths, &mut c);
        let h = (c[2].pos[1] / c[2].pos[3] - c[0].pos[1] / c[0].pos[3]).abs();
        assert!(h > 0.005, "edge-on in the board's plane, face on when facing the camera: {h}");
    }

    #[test]
    fn vertex_layout() {
        assert_eq!(std::mem::size_of::<Vertex>(), 64);
        let mut v = Vec::new();
        test_rect([1.0, 0.0, 1.0], 0.0, &mut v);
        assert_eq!(v.len(), 6);
        assert!(v.iter().all(|x| x.extra[1] == 1.0 && x.pos[3] == 1.0));
    }
}
