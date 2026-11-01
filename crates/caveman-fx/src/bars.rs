//! The bars: Caveman FX's first "DLL draws" effect (Trevor's stream gimmick). A row of short bars (crisp, opaque
//! rectangles by default; the first look, rounded glowing capsules, is the `glow` style) sticks out sideways from both
//! long edges of the skateboard deck, spaced along it between the trucks, in the board's plane, moving like an audio
//! equaliser; a landing shoots them out extra far, then they settle over about a second. The `ring` layout puts them
//! all the way round the deck's outline instead (an analytic popsicle planform), like the game's own volume meter.
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

/// How a bar looks: a crisp, opaque rectangle (Trevor, 2026-10-21: "crisp bars without the bloom"), or the first
/// look: a rounded capsule with a glow and a white-hot centre line, blended additively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Solid,
    Glow,
}

/// Which camera and board transforms place the bars: the frame's (the renderer's own per-frame copies, the same pair
/// the picture was drawn with) or the simulation's (they change once a tick, at different moments: the bars jitter in
/// motion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Timing {
    Frame,
    Tick,
}

/// What the audio bands are measured against: the loudest band of the moment (every bar sits where its band is
/// within the window under that peak: balanced music keeps them all near the top), or each band's own recent peak
/// (every bar shows its own band's dynamics: the kick pumps the low bars, the hats the high ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    All,
    Band,
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
    /// The audio bands' window under their reference, in dB (a narrower window spreads the bars out more).
    pub window: f32,
    /// The reference the bands are measured against (see [`Track`]).
    pub track: Track,
    /// A level under this (0 to 1) shows as nothing, and the rest stretches to the full range: the bars fall between
    /// beats instead of idling half way up.
    pub floor: f32,
    /// How fast a reference falls after a peak, in dB a second (faster: the bars recover sooner after a loud moment).
    pub release: f32,
    /// The meter's rise and fall times (ms): how fast a bar follows its band up, and down.
    pub rise: f32,
    pub fall: f32,
    /// The board cam (pip.rs, overlay.rs): the skateboard drawn in a corner from a fixed viewpoint, turned as the board
    /// under the skater is. `pip_size` is its square's side as a fraction of the screen's height, `pip_margin` the
    /// gap to the corner (`tl`, `tr`, `bl`, `br`); `pip_view` `camera` turns the board relative to the game camera's
    /// heading (a flip looks as it does on screen), `world` relative to the world; the viewpoint's `pip_elevation` and
    /// `pip_azimuth` (degrees; azimuth 0 is behind the tail, positive to the right), `pip_fov` and `pip_zoom`;
    /// `pip_art` puts the board's live art on the deck's underside (`pip_mirror` and `pip_flip` turn it if it lands
    /// mirrored or nose-to-tail); `pip_backdrop` is the dark square behind it (0: none).
    pub pip: bool,
    pub pip_size: f32,
    pub pip_margin: f32,
    pub pip_corner: String,
    pub pip_view: String,
    pub pip_elevation: f32,
    pub pip_azimuth: f32,
    pub pip_fov: f32,
    pub pip_zoom: f32,
    pub pip_art: bool,
    pub pip_mirror: bool,
    pub pip_flip: bool,
    pub pip_backdrop: f32,
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
    /// Moves the row along the board's up axis: the transform's origin is at the ground (0.08, the default, is just
    /// under the deck).
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
    pub style: Style,
    /// A darker outline around each solid bar (metres; 0: none), for bright ground.
    pub outline: f32,
    pub timing: Timing,
    /// How far in front of a bar the game's scene must be to hide it (metres), when drawing tests the game's depth.
    pub depth_bias: f32,
    /// Where the bars stand: along both long edges (`sides`), or all the way round the deck's outline (`ring`, like the
    /// game's own volume meter; `count` is then the bars in the ring).
    pub layout: Layout,
    /// The outer part of a solid bar takes this colour as its level rises (`#rrggbb`).
    pub tip_colour: String,
    /// Very loud bars' tips take this colour instead (`#rrggbb`; empty: none).
    pub accent_colour: String,
    /// A solid bar's outer end fades out over this length (metres; 0: a crisp end).
    pub soft: f32,
    /// The deck's outline for the ring (metres): width, length, and the radius of its nose and tail (at most half
    /// the width: the stock popsicle's are round).
    pub deck_width: f32,
    pub deck_length: f32,
    pub deck_end: f32,
    /// The `rim` layout: how far out from the deck's edge the bars stand, along the deck's surface (metres; negative:
    /// in over the deck).
    pub rim_out: f32,
    /// The `rim` layout: how far off the edge the bars stand along the deck's surface normal (metres; negative: under
    /// the edge's middle, towards the underside).
    pub rim_lift: f32,
}

/// Where the bars stand. `Rim` is the ring on the deck's real edge (`rim.rs`: the stock deck mesh's, sent by
/// `caveman-trainer bars rim load`), kicks included, each bar pointing out along the deck's surface; `height` does not
/// move it (`rim_lift` and `rim_out` do), and without a rim it draws the `Ring`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    Sides,
    Ring,
    Rim,
}

impl Default for BarSettings {
    fn default() -> Self {
        BarSettings {
            enabled: false,
            colour: "#ffd21f".into(),
            count: 8,
            gain: 1.0,
            window: 42.0,
            track: Track::All,
            floor: 0.0,
            release: 8.0,
            rise: 35.0,
            fall: 120.0,
            pip: false,
            pip_size: 0.24,
            pip_margin: 0.02,
            pip_corner: "tr".into(),
            pip_view: "camera".into(),
            // from slightly below: the art and the wheels are the resting view, a flip shows the grip
            pip_elevation: -30.0,
            pip_azimuth: -35.0,
            pip_fov: 30.0,
            pip_zoom: 1.0,
            pip_art: true,
            pip_mirror: false,
            pip_flip: false,
            pip_backdrop: 0.55,
            source: Source::Random,
            mirror: true,
            length: 0.30,
            base: 0.025,
            width: 0.032,
            glow: 0.03,
            span: 0.40,
            edge: 0.105,
            // the board transform's origin is at the ground: 8 cm brings the bars out from under the deck (Trevor,
            // 2026-10-21 14:20: "perfect"); 0 puts them on the floor
            height: 0.08,
            shift: 0.0,
            axis: Axis::Z,
            facing: Facing::Board,
            land: 1.5,
            land_ms: 1000.0,
            brightness: 2.0,
            hdr: HdrMode::Auto,
            tempo: 1.0,
            style: Style::Solid,
            outline: 0.0,
            timing: Timing::Frame,
            depth_bias: 0.03,
            layout: Layout::Sides,
            tip_colour: "#ffd21f".into(),
            accent_colour: String::new(),
            soft: 0.0,
            deck_width: 0.21,
            deck_length: 0.81,
            deck_end: 0.105,
            rim_out: 0.0,
            rim_lift: 0.0,
        }
    }
}

pub const MAX_COUNT: u32 = 256;

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
        let hex = |c: [f32; 3]| format!("#{:02x}{:02x}{:02x}", (c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8);
        let c = parse_colour(&self.colour).ok_or_else(|| format!("colour {:?} is not #rrggbb", self.colour))?;
        self.colour = hex(c);
        let c = parse_colour(&self.tip_colour).ok_or_else(|| format!("tip_colour {:?} is not #rrggbb", self.tip_colour))?;
        self.tip_colour = hex(c);
        if !self.accent_colour.trim().is_empty() {
            let c = parse_colour(&self.accent_colour).ok_or_else(|| format!("accent_colour {:?} is not #rrggbb (or empty: none)", self.accent_colour))?;
            self.accent_colour = hex(c);
        } else {
            self.accent_colour.clear();
        }
        let finite = [
            self.gain,
            self.window,
            self.floor,
            self.release,
            self.rise,
            self.fall,
            self.pip_size,
            self.pip_margin,
            self.pip_elevation,
            self.pip_azimuth,
            self.pip_fov,
            self.pip_zoom,
            self.pip_backdrop,
            self.length,
            self.base,
            self.width,
            self.glow,
            self.span,
            self.edge,
            self.height,
            self.shift,
            self.land,
            self.land_ms,
            self.brightness,
            self.tempo,
            self.outline,
            self.depth_bias,
            self.soft,
            self.deck_width,
            self.deck_length,
            self.deck_end,
            self.rim_out,
            self.rim_lift,
        ];
        if finite.iter().any(|v| !v.is_finite()) {
            return Err("every number must be finite".into());
        }
        self.count = self.count.clamp(1, MAX_COUNT);
        self.gain = self.gain.clamp(0.0, 10.0);
        self.window = self.window.clamp(6.0, 60.0);
        self.pip_size = self.pip_size.clamp(0.05, 0.9);
        self.pip_margin = self.pip_margin.clamp(0.0, 0.3);
        self.pip_elevation = self.pip_elevation.clamp(-89.0, 89.0);
        self.pip_azimuth = ((self.pip_azimuth + 180.0).rem_euclid(360.0)) - 180.0;
        self.pip_fov = self.pip_fov.clamp(5.0, 120.0);
        self.pip_zoom = self.pip_zoom.clamp(0.2, 5.0);
        self.pip_backdrop = self.pip_backdrop.clamp(0.0, 1.0);
        if !["tl", "tr", "bl", "br"].contains(&self.pip_corner.as_str()) {
            return Err(format!("pip_corner {:?} is not tl, tr, bl or br", self.pip_corner));
        }
        if !["camera", "world"].contains(&self.pip_view.as_str()) {
            return Err(format!("pip_view {:?} is not camera or world", self.pip_view));
        }
        self.floor = self.floor.clamp(0.0, 0.95);
        self.release = self.release.clamp(1.0, 60.0);
        self.rise = self.rise.clamp(5.0, 500.0);
        self.fall = self.fall.clamp(20.0, 2000.0);
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
        self.outline = self.outline.clamp(0.0, 0.05);
        self.depth_bias = self.depth_bias.clamp(0.0, 1.0);
        self.soft = self.soft.clamp(0.0, 1.0);
        self.deck_width = self.deck_width.clamp(0.02, 2.0);
        self.deck_length = self.deck_length.clamp(self.deck_width, 4.0);
        self.deck_end = self.deck_end.clamp(0.0, self.deck_width * 0.5);
        self.rim_out = self.rim_out.clamp(-0.1, 0.5);
        self.rim_lift = self.rim_lift.clamp(-0.2, 0.2);
        Ok(self)
    }

    /// These settings with the fields of `patch` (a JSON object, as the trainer sends) applied.
    pub fn patched(&self, patch: &serde_json::Value) -> Result<BarSettings, String> {
        let obj = patch.as_object().ok_or("the settings must be a JSON object")?;
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        for (k, x) in obj {
            if v.get(k).is_none() {
                return Err(format!(
                    "no setting {k:?} (settings: {})",
                    v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()
                ));
            }
            v[k] = x.clone();
        }
        let s: BarSettings = serde_json::from_value(v).map_err(|e| e.to_string())?;
        s.sanitized()
    }

    /// How many independent levels the row shows (the ring: one per band, mirrored left and right, from the tail to
    /// the nose).
    pub fn levels_needed(&self) -> usize {
        match self.layout {
            Layout::Sides => self.count as usize * if self.mirror { 1 } else { 2 },
            Layout::Ring | Layout::Rim => self.count as usize / 2 + 1,
        }
    }

    /// How many bars are drawn.
    pub fn bars(&self) -> usize {
        match self.layout {
            Layout::Sides => self.count as usize * 2,
            Layout::Ring | Layout::Rim => self.count as usize,
        }
    }

    /// The band (level index) of ring bar `k` of `n`: the bars go round the outline from the nose's centre; both
    /// halves share bands, low at the tail, high at the nose.
    pub fn ring_band(k: usize, n: usize) -> usize {
        let bands = n / 2 + 1;
        let m = k.min(n.saturating_sub(1 + k)); // bars from the nose, the same on both sides
        let last = n.div_ceil(2).saturating_sub(1).max(1);
        let from_nose = (m * (bands - 1) + last / 2) / last; // rounded
        (bands - 1).saturating_sub(from_nose)
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
        // the audio analysis takes the settings' window, reference, floor and release (atomics: cheap every frame)
        crate::audio::tune(s.window, s.track == Track::Band, s.floor, s.release);
        self.source.levels(now, dt, s.tempo, &mut self.raw);
        for (level, &target) in self.levels.iter_mut().zip(&self.raw) {
            let target = if target.is_finite() { target.clamp(0.0, 2.0) } else { 0.0 };
            let tau = if target > *level { s.rise / 1000.0 } else { s.fall / 1000.0 };
            *level += (target - *level) * (1.0 - (-dt / tau).exp());
        }
    }

    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    /// The landing boost of bar `k` of `n` at `now`: shoots up in about 50 ms, holds to 80 ms, then decays so it has
    /// settled (5 %) after `land_ms`; it starts in the middle of the row and ripples out to the ends.
    pub fn boost(&self, s: &BarSettings, k: usize, n: usize, now: f64) -> f32 {
        let Some(at) = self.land_at else { return 0.0 };
        let centre = (n as f32 - 1.0) * 0.5;
        let delay = 0.018 * (k as f32 - centre).abs();
        let t = (now - at) as f32 - delay;
        if t <= 0.0 {
            return 0.0;
        }
        const HOLD: f32 = 0.08;
        let tau = (s.land_ms / 1000.0 - HOLD).max(0.02) / 3.0;
        s.land * (1.0 - (-t / 0.018).exp()) * (-(t - HOLD).max(0.0) / tau).exp()
    }

    /// The length of each bar (metres): `count` per side, the second side after the first; or the ring's, from the
    /// nose round.
    pub fn lengths(&self, s: &BarSettings, now: f64) -> Vec<f32> {
        self.bars_now(s, now).into_iter().map(|(len, _)| len).collect()
    }

    /// Each bar's length (metres) and level (0 to about 1, with the landing spike: what lights its tip).
    pub fn bars_now(&self, s: &BarSettings, now: f64) -> Vec<(f32, f32)> {
        let n = s.count as usize;
        let one = |li: usize, k: usize, of: usize| {
            let level = self.levels.get(li).copied().unwrap_or(0.0) * s.gain + self.boost(s, k, of, now);
            ((s.base + s.length * level).clamp(0.0, 4.0), level)
        };
        match s.layout {
            Layout::Sides => {
                let mut out = Vec::with_capacity(n * 2);
                for side in 0..2 {
                    for k in 0..n {
                        out.push(one(if s.mirror { k } else { side * n + k }, k, n));
                    }
                }
                out
            }
            Layout::Ring | Layout::Rim => {
                let bands = n / 2 + 1;
                (0..n).map(|k| BarSettings::ring_band(k, n)).map(|b| one(b, b, bands)).collect()
            }
        }
    }
}

/// A point of the deck's outline and its outward normal, `along` metres round it from the nose's centre (clockwise
/// seen from above: towards +x, across the board, first), in the board's plane: x across, y along (the nose at +y).
/// The outline is a rounded rectangle `width` by `length` whose corners have radius `end` (at half the width: a
/// stadium, the stock popsicle's planform). Returns (x, y, nx, ny).
pub fn outline_point(width: f32, length: f32, end: f32, along: f32) -> (f32, f32, f32, f32) {
    let (a, b) = (width * 0.5, length * 0.5);
    let r = end.clamp(0.0, a);
    let (fa, fb) = (a - r, b - r); // the flat halves
    let quarter = std::f32::consts::FRAC_PI_2 * r;
    let perimeter = outline_length(width, length, end);
    let mut s = along.rem_euclid(perimeter.max(1e-6));
    // the pieces in order: nose flat (right half), nose-right corner, right side, tail-right corner, tail flat, tail-left
    // corner, left side, nose-left corner, nose flat (left half)
    let arc = |cx: f32, cy: f32, from: f32, t: f32| {
        let ang = from - t / r.max(1e-6);
        let (sn, cs) = ang.sin_cos();
        (cx + r * cs, cy + r * sn, cs, sn)
    };
    if s < fa {
        return (s, b, 0.0, 1.0);
    }
    s -= fa;
    if s < quarter {
        return arc(fa, fb, std::f32::consts::FRAC_PI_2, s);
    }
    s -= quarter;
    if s < 2.0 * fb {
        return (a, fb - s, 1.0, 0.0);
    }
    s -= 2.0 * fb;
    if s < quarter {
        return arc(fa, -fb, 0.0, s);
    }
    s -= quarter;
    if s < 2.0 * fa {
        return (fa - s, -b, 0.0, -1.0);
    }
    s -= 2.0 * fa;
    if s < quarter {
        return arc(-fa, -fb, -std::f32::consts::FRAC_PI_2, s);
    }
    s -= quarter;
    if s < 2.0 * fb {
        return (-a, -fb + s, -1.0, 0.0);
    }
    s -= 2.0 * fb;
    if s < quarter {
        return arc(-fa, fb, std::f32::consts::PI, s);
    }
    s -= quarter;
    (-fa + s, b, 0.0, 1.0)
}

/// The outline's length (metres).
pub fn outline_length(width: f32, length: f32, end: f32) -> f32 {
    let (a, b) = (width * 0.5, length * 0.5);
    let r = end.clamp(0.0, a);
    4.0 * (a - r) + 4.0 * (b - r) + 2.0 * std::f32::consts::PI * r
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

/// One vertex of the overlay (96 bytes, the input layout of `shaders/overlay.hlsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    /// Clip space.
    pub pos: [f32; 4],
    /// The bar's frame: along, across (metres), the bar's length, its radius.
    pub local: [f32; 4],
    /// Colour (sRGB-encoded) and an alpha multiplier.
    pub color: [f32; 4],
    /// x: the glow's width (metres, glow style) or the soft end's length (solid style); y: 1 for a screen-space quad
    /// (the test rectangle); z: 1 for the solid style; w: the solid style's outline (metres).
    pub extra: [f32; 4],
    /// The tip's colour (sRGB-encoded) and where along the bar it begins (metres; past the end: no tip).
    pub tip: [f32; 4],
    /// The same point in the clip space of the camera the depth copy was rendered with (the depth test reads the copy
    /// there and compares with its w): `pos` when the copy is the frame's own; see [`reproject`].
    pub dpos: [f32; 4],
}

/// The world point a clip-space point of `cam` (as [`project`] makes it) stands for.
pub fn unproject(cam: &Camera, aspect: f32, clip: [f32; 4]) -> V3 {
    let f = 1.0 / (cam.fov_y.to_radians() * 0.5).tan();
    let (x, y, z) = (clip[0] * aspect / f, clip[1] / f, clip[3]);
    sub(add(add(cam.pos, mul(cam.right, x)), mul(cam.up, y)), mul(cam.back, z))
}

/// A point given in `from`'s clip space, in `to`'s (the same aspect ratio).
pub fn reproject(from: &Camera, to: &Camera, aspect: f32, clip: [f32; 4]) -> [f32; 4] {
    project(to, aspect, unproject(from, aspect, clip))
}

/// The same, for a point carried by the board: where it was in an earlier frame (`to`'s camera and board) is where
/// that frame's board had it. The skater, the deck and the wheels move with the board, so an earlier frame's depth
/// lines up with the bars this way.
pub fn reproject_on_board(from: &Scene, to: &Scene, aspect: f32, clip: [f32; 4]) -> [f32; 4] {
    let p = unproject(&from.camera, aspect, clip);
    let p = match (from.board, to.board) {
        (Some(a), Some(b)) => {
            let d = sub(p, a.pos);
            add(add(add(b.pos, mul(b.right, dot(d, a.right))), mul(b.up, dot(d, a.up))), mul(b.forward, dot(d, a.forward)))
        }
        _ => p,
    };
    project(&to.camera, aspect, p)
}

/// Room around a solid bar's quad for its anti-aliased edge (metres): more than a pixel at any distance the bars
/// are drawn at.
pub const EDGE_ROOM: f32 = 0.01;
/// No tip colour.
const NO_TIP: f32 = 1.0e6;

/// Two triangles from four corners (00, 10, 01, 11).
fn quad(out: &mut Vec<Vertex>, c: [Vertex; 4]) {
    out.extend_from_slice(&[c[0], c[1], c[2], c[2], c[1], c[3]]);
}

/// How much of a bar's outer part takes the tip colour at a level: none below 45 % of the full level, the outer half
/// at full level and above.
pub fn tip_part(level: f32) -> f32 {
    ((level - 0.45) / 0.55).clamp(0.0, 1.0) * 0.5
}

/// The bars' quads for one frame. `bars` from [`Bars::bars_now`] (length, level); `aspect` the back buffer's.
/// Nothing without a board, or when the board is far from the camera (a menu's camera, a replay). The `rim` layout
/// stands on the rim the trainer sent ([`crate::rim::current`]).
pub fn build(s: &BarSettings, scene: &Scene, aspect: f32, bars: &[(f32, f32)], out: &mut Vec<Vertex>) {
    let rim = if s.layout == Layout::Rim { crate::rim::current() } else { None };
    build_on(s, scene, aspect, bars, rim.as_deref(), out);
}

/// [`build`] with the rim given (`None`: the `rim` layout draws the ring).
pub fn build_on(s: &BarSettings, scene: &Scene, aspect: f32, bars: &[(f32, f32)], rim: Option<&crate::rim::Rim>, out: &mut Vec<Vertex>) {
    let Some(board) = scene.board else { return };
    let cam = &scene.camera;
    if length(sub(board.pos, cam.pos)) > 60.0 {
        return;
    }
    let colour = parse_colour(&s.colour).unwrap_or([1.0, 0.82, 0.12]);
    let tip_colour = parse_colour(&s.tip_colour).unwrap_or(colour);
    let accent = parse_colour(&s.accent_colour);
    let (long, across) = match s.axis {
        Axis::Z => (board.forward, board.right),
        Axis::X => (board.right, board.forward),
    };
    let r = s.width * 0.5;
    let g = s.glow;
    let solid = s.style == Style::Solid;
    // the quad's room around the bar: the glow (and the capsule's round ends), or a solid bar's outline and edge
    let (pad_u, pad_v) = if solid { (s.outline + EDGE_ROOM, r + s.outline + EDGE_ROOM) } else { (r + g, r + g) };
    let extra = if solid { [s.soft, 0.0, 1.0, s.outline] } else { [g.max(1e-4), 0.0, 0.0, 0.0] };
    let centre = add(add(board.pos, mul(board.up, s.height)), mul(long, s.shift));
    // one bar: its base, the way out, the way across in the board's plane, its length and level
    let mut bar = |base: V3, out_dir: V3, across_dir: V3, len: f32, level: f32| {
        let width_dir = match s.facing {
            Facing::Board => across_dir,
            Facing::Camera => normalize(cross(out_dir, sub(base, cam.pos))).unwrap_or(across_dir),
        };
        let part = tip_part(level);
        let tip_rgb = match accent {
            Some(a) if level > 0.9 => a,
            _ => tip_colour,
        };
        let tip = [tip_rgb[0], tip_rgb[1], tip_rgb[2], if part > 0.0 { len * (1.0 - part) } else { NO_TIP }];
        let (u0, u1, v0, v1) = (-pad_u, len + pad_u, -pad_v, pad_v);
        let corner = |u: f32, v: f32| -> Vertex {
            let p = add(add(base, mul(out_dir, u)), mul(width_dir, v));
            let pos = project(cam, aspect, p);
            Vertex { pos, local: [u, v, len, r], color: [colour[0], colour[1], colour[2], 1.0], extra, tip, dpos: pos }
        };
        quad(out, [corner(u0, v0), corner(u1, v0), corner(u0, v1), corner(u1, v1)]);
    };
    match s.layout {
        Layout::Sides => {
            let n = s.count as usize;
            for side in 0..2 {
                let out_dir = mul(across, if side == 0 { 1.0 } else { -1.0 });
                for k in 0..n {
                    let Some(&(len, level)) = bars.get(side * n + k) else { continue };
                    let t = if n > 1 { k as f32 / (n - 1) as f32 - 0.5 } else { 0.0 };
                    let base = add(add(centre, mul(long, t * s.span)), mul(out_dir, s.edge));
                    bar(base, out_dir, long, len, level);
                }
            }
        }
        Layout::Rim if rim.is_some() => {
            let places = rim_bars(s, &board, rim.unwrap(), bars.len());
            for (&(len, level), &(base, out_dir, along)) in bars.iter().zip(&places) {
                bar(base, out_dir, along, len, level);
            }
        }
        Layout::Ring | Layout::Rim => {
            let n = bars.len().max(1);
            let perimeter = outline_length(s.deck_width, s.deck_length, s.deck_end);
            for (k, &(len, level)) in bars.iter().enumerate() {
                let (x, y, nx, ny) = outline_point(s.deck_width, s.deck_length, s.deck_end, (k as f32 + 0.5) * perimeter / n as f32);
                let base = add(add(centre, mul(across, x)), mul(long, y));
                let out_dir = add(mul(across, nx), mul(long, ny));
                let tangent = add(mul(across, -ny), mul(long, nx));
                bar(base, out_dir, tangent, len, level);
            }
        }
    }
}

/// Where `n` bars of the `rim` layout stand, in the world: each one's base, the way out (along the deck's surface)
/// and the way along the edge. Evenly spaced round the rim from the nose's centre, as the ring's (bar k at
/// (k + 0.5) / n of the way round). The rim's frame is the board's (x across, y up, z along), mapped as the ring maps
/// its outline (x on `across`, z on `long`: `axis` swaps them), moved along the deck by `shift`, out from the edge by
/// `rim_out` and off it along the surface normal by `rim_lift`; `height` does not apply.
pub fn rim_bars(s: &BarSettings, board: &Board, rim: &crate::rim::Rim, n: usize) -> Vec<(V3, V3, V3)> {
    let (long, across) = match s.axis {
        Axis::Z => (board.forward, board.right),
        Axis::X => (board.right, board.forward),
    };
    let to_world = |v: V3| add(add(mul(across, v[0]), mul(board.up, v[1])), mul(long, v[2]));
    let origin = add(board.pos, mul(long, s.shift));
    (0..n)
        .map(|k| {
            let p = rim.at((k as f32 + 0.5) * rim.length() / n.max(1) as f32);
            let local = add(add(p.point, mul(p.out, s.rim_out)), mul(p.normal, s.rim_lift));
            (add(origin, to_world(local)), to_world(p.out), to_world(p.along))
        })
        .collect()
}

/// The test rectangle: a solid quad in screen space (normalised device coordinates), pulsing gently.
pub fn test_rect(colour: [f32; 3], now: f64, out: &mut Vec<Vertex>) {
    let a = 0.75 + 0.25 * (now * 3.0).sin() as f32;
    let (x0, x1, y0, y1) = (-0.9f32, -0.5f32, 0.9f32, 0.6f32);
    let v = |x: f32, y: f32| Vertex {
        pos: [x, y, 0.5, 1.0],
        local: [0.0; 4],
        color: [colour[0], colour[1], colour[2], a],
        extra: [0.0, 1.0, 0.0, 0.0],
        tip: [0.0, 0.0, 0.0, NO_TIP],
        dpos: [x, y, 0.5, 1.0],
    };
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
        let p = s.patched(&serde_json::json!({"gain": 2.5, "colour": "FF0080", "count": 999})).unwrap();
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
        // a file from before the solid style (Trevor's, 2026-10-21 13:11) loads: the new settings take their defaults
        let before = r##"{"enabled": true, "colour": "#ffd21f", "count": 8, "gain": 1.0, "source": "audio", "mirror": true, "length": 0.3,
            "base": 0.025, "width": 0.024, "glow": 0.0, "span": 0.4, "edge": 0.105, "height": 0.0, "shift": 0.0, "axis": "z",
            "facing": "board", "land": 1.5, "land_ms": 1000.0, "brightness": 2.0, "hdr": "auto", "tempo": 1.0}"##;
        let b: BarSettings = serde_json::from_str(before).unwrap();
        assert_eq!((b.style, b.outline, b.timing, b.width), (Style::Solid, 0.0, Timing::Frame, 0.024));
        assert_eq!(s.patched(&serde_json::json!({"style": "glow", "timing": "tick"})).unwrap().style, Style::Glow);
        assert!(s.patched(&serde_json::json!({"style": "round"})).is_err());
        assert_eq!(s.patched(&serde_json::json!({"outline": 1.0})).unwrap().outline, 0.05);
        // the ring's settings
        let r = s.patched(&serde_json::json!({"layout": "ring", "tip_colour": "FFE000", "accent_colour": "00ff66", "deck_end": 0.5})).unwrap();
        assert_eq!((r.layout, r.tip_colour.as_str(), r.accent_colour.as_str(), r.deck_end), (Layout::Ring, "#ffe000", "#00ff66", r.deck_width * 0.5));
        assert_eq!(s.patched(&serde_json::json!({"accent_colour": " "})).unwrap().accent_colour, "");
        assert!(s.patched(&serde_json::json!({"tip_colour": "gold"})).is_err());
        assert!(s.patched(&serde_json::json!({"layout": "circle"})).is_err());
        assert_eq!((BarSettings::default().layout, BarSettings::default().height), (Layout::Sides, 0.08));
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
        assert!(peak > 0.95 * s.land, "peak {peak}");
        let during = bars.lengths(&s, t);
        assert!(during.iter().zip(&before).filter(|(d, b)| d > b).count() >= 12, "{during:?} vs {before:?}");
        assert!(bars.boost(&s, 3, 8, at + s.land_ms as f64 / 1000.0) < 0.06 * s.land);
        assert!(bars.boost(&s, 3, 8, at + 2.0) < 0.005 * s.land);
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
        let s = BarSettings { enabled: true, count: 4, style: Style::Glow, height: 0.0, ..BarSettings::default() };
        let lengths = vec![(0.2f32, 0.0f32); 8];
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
        // side 1 now sticks out along +z, which is down the screen (the camera's up is -z)
        let min_y = w[..24].iter().map(|x| x.pos[1] / x.pos[3]).fold(f32::MAX, f32::min);
        assert!((min_y + reach).abs() < 1e-4, "{min_y} {reach}");
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
        // the solid style (the default): the quad hugs the bar (an outline and a centimetre for the edge), and says so
        let solid = BarSettings { style: Style::Solid, outline: 0.004, ..s.clone() };
        assert_eq!(BarSettings::default().style, Style::Solid);
        let mut q = Vec::new();
        build(&solid, &scene, 1.0, &lengths, &mut q);
        let max_x = q[..24].iter().map(|x| x.pos[0] / x.pos[3]).fold(f32::MIN, f32::max);
        let reach = (s.edge + 0.2 + 0.004 + EDGE_ROOM) * f / 3.0;
        assert!((max_x - reach).abs() < 1e-4, "{max_x} {reach}");
        assert!(q.iter().all(|x| x.extra == [0.0, 0.0, 1.0, 0.004]));
        assert_eq!(q[0].local, [-(0.004 + EDGE_ROOM), -(s.width * 0.5 + 0.004 + EDGE_ROOM), 0.2, s.width * 0.5]);
    }

    #[test]
    fn vertex_layout() {
        assert_eq!(std::mem::size_of::<Vertex>(), 96);
    }

    #[test]
    fn reprojects_between_cameras() {
        let a = Camera { right: [1.0, 0.0, 0.0], up: [0.0, 0.0, -1.0], back: [0.0, 1.0, 0.0], pos: [10.0, 3.0, 20.0], fov_y: 60.0 };
        let yaw = 0.2f32;
        let b = Camera { right: [yaw.cos(), 0.0, -yaw.sin()], up: [0.0, 1.0, 0.0], back: [yaw.sin(), 0.0, yaw.cos()], pos: [9.0, 1.0, 23.0], fov_y: 50.0 };
        for p in [[10.0f32, 0.0, 20.0], [10.3, 0.1, 19.6], [9.8, -0.2, 20.4]] {
            let r = reproject(&a, &b, 1.6, project(&a, 1.6, p));
            let want = project(&b, 1.6, p);
            assert!((0..4).all(|i| (r[i] - want[i]).abs() < 1e-4), "{r:?} vs {want:?}");
            assert_eq!(reproject(&a, &a, 1.6, project(&a, 1.6, p)).map(|v| (v * 1e4).round()), project(&a, 1.6, p).map(|v| (v * 1e4).round()));
        }
        // on the board: a point 10 cm right of a board that has since moved 8 cm on and turned, where the earlier board
        // had it
        let then = Board { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], forward: [0.0, 0.0, 1.0], pos: [10.0, 0.0, 20.0] };
        let now = Board { right: [yaw.cos(), 0.0, -yaw.sin()], up: [0.0, 1.0, 0.0], forward: [yaw.sin(), 0.0, yaw.cos()], pos: [10.0, 0.0, 20.08] };
        let p_now = add(now.pos, mul(now.right, 0.1));
        let r = reproject_on_board(&Scene { camera: a, board: Some(now) }, &Scene { camera: b, board: Some(then) }, 1.6, project(&a, 1.6, p_now));
        let want = project(&b, 1.6, [10.1, 0.0, 20.0]);
        assert!((0..4).all(|i| (r[i] - want[i]).abs() < 1e-4), "{r:?} vs {want:?}");
    }

    #[test]
    fn the_outline_goes_round_the_deck() {
        let (w, l, e) = (0.21f32, 0.81f32, 0.105f32);
        let p = outline_length(w, l, e);
        assert!((p - (2.0 * (l - w) + std::f32::consts::PI * w)).abs() < 1e-5, "a stadium: {p}");
        // the nose's centre first, the right side at a quarter, the tail's centre half way, the left side at three quarters
        let at = |f: f32| outline_point(w, l, e, f * p);
        let close = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
            (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4 && (a.2 - b.2).abs() < 1e-4 && (a.3 - b.3).abs() < 1e-4
        };
        assert!(close(at(0.0), (0.0, l / 2.0, 0.0, 1.0)), "{:?}", at(0.0));
        assert!(close(at(0.25), (w / 2.0, 0.0, 1.0, 0.0)), "{:?}", at(0.25));
        assert!(close(at(0.5), (0.0, -l / 2.0, 0.0, -1.0)), "{:?}", at(0.5));
        assert!(close(at(0.75), (-w / 2.0, 0.0, -1.0, 0.0)), "{:?}", at(0.75));
        // continuous, unit outward normals perpendicular to the way round, every point on the outline
        let mut last = at(0.0);
        for k in 1..=2000 {
            let q = at(k as f32 / 2000.0);
            let step = ((q.0 - last.0).powi(2) + (q.1 - last.1).powi(2)).sqrt();
            assert!(step < p / 2000.0 * 1.01, "a jump at {k}: {step}");
            assert!(((q.2 * q.2 + q.3 * q.3) - 1.0).abs() < 1e-4);
            assert!((q.2 * (q.0 - last.0) + q.3 * (q.1 - last.1)).abs() < 1e-3, "normal across the way at {k}");
            // on the outline: the distance to the inner segment (the rounded rectangle's core) is the end radius
            let (cx, cy) = (q.0.clamp(-(w / 2.0 - e), w / 2.0 - e), q.1.clamp(-(l / 2.0 - e), l / 2.0 - e));
            assert!((((q.0 - cx).powi(2) + (q.1 - cy).powi(2)).sqrt() - e).abs() < 1e-4, "off the outline at {k}: {q:?}");
            last = q;
        }
        // square ends (no rounding): the corners
        let sq = outline_point(0.2, 0.8, 0.0, 0.1);
        assert!(close(sq, (0.1, 0.4, 0.0, 1.0)) || close(sq, (0.1, 0.4, 1.0, 0.0)), "{sq:?}");
    }

    #[test]
    fn the_ring_mirrors_its_bands() {
        let n = 120;
        let bands: Vec<usize> = (0..n).map(|k| BarSettings::ring_band(k, n)).collect();
        assert_eq!(bands[0], n / 2, "the highest band at the nose");
        assert!(bands[n / 2 - 1] <= 1 && bands[n / 2] <= 1, "the lowest at the tail");
        for k in 0..n {
            assert_eq!(bands[k], bands[n - 1 - k], "left and right alike");
        }
        let s = BarSettings { layout: Layout::Ring, count: n as u32, ..BarSettings::default() };
        assert_eq!(s.levels_needed(), n / 2 + 1);
        assert_eq!(s.bars(), n);
        let mut bars = Bars::new();
        bars.update(&s, 0.1, 0);
        assert_eq!(bars.bars_now(&s, 0.1).len(), n);
    }

    #[test]
    fn ring_bars_stand_on_the_outline_normals() {
        // the camera 3 m above the board, looking straight down: x right, -z up the screen
        let cam = Camera { right: [1.0, 0.0, 0.0], up: [0.0, 0.0, -1.0], back: [0.0, 1.0, 0.0], pos: [10.0, 3.0, 20.0], fov_y: 60.0 };
        let board = Board { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], forward: [0.0, 0.0, 1.0], pos: [10.0, 0.0, 20.0] };
        let s = BarSettings { enabled: true, layout: Layout::Ring, count: 64, height: 0.0, style: Style::Solid, ..BarSettings::default() };
        let bars: Vec<(f32, f32)> = (0..64).map(|k| (0.1 + k as f32 * 0.001, if k == 0 { 1.0 } else { 0.0 })).collect();
        let mut v = Vec::new();
        build(&s, &Scene { camera: cam, board: Some(board) }, 1.0, &bars, &mut v);
        assert_eq!(v.len(), 64 * 6);
        let p = outline_length(s.deck_width, s.deck_length, s.deck_end);
        let f = 1.0 / (30.0f32.to_radians()).tan();
        for k in 0..64 {
            let (x, y, nx, ny) = outline_point(s.deck_width, s.deck_length, s.deck_end, (k as f32 + 0.5) * p / 64.0);
            // the middle of the base edge (corners 0 and 2: u = -pad, v = -/+ pad) and of the far edge (1 and 3)
            let mid = |a: usize, b: usize| ((v[k * 6 + a].pos[0] + v[k * 6 + b].pos[0]) / 2.0 / 3.0, (v[k * 6 + a].pos[1] + v[k * 6 + b].pos[1]) / 2.0 / 3.0);
            let (bx, by) = mid(0, 2);
            let (tx, ty) = mid(1, 5);
            let pad = EDGE_ROOM;
            let len = bars[k].0;
            // screen: x = world x * f / 3, y = -world z * f / 3 (the board's x across = world x, along = world z)
            let want = |u: f32| ((x + nx * u) * f / 3.0, -(y + ny * u) * f / 3.0);
            let (wbx, wby) = want(-pad);
            let (wtx, wty) = want(len + pad);
            assert!((bx - wbx).abs() < 1e-4 && (by - wby).abs() < 1e-4, "bar {k} base: {bx},{by} vs {wbx},{wby}");
            assert!((tx - wtx).abs() < 1e-4 && (ty - wty).abs() < 1e-4, "bar {k} tip: {tx},{ty} vs {wtx},{wty}");
        }
        // the loud bar (level 1) has a tip from half its length; the quiet ones none
        assert!((v[0].tip[3] - bars[0].0 * 0.5).abs() < 1e-6);
        assert!(v[6].tip[3] > 1000.0);
        let mut v = Vec::new();
        test_rect([1.0, 0.0, 1.0], 0.0, &mut v);
        assert_eq!(v.len(), 6);
        assert!(v.iter().all(|x| x.extra[1] == 1.0 && x.pos[3] == 1.0));
    }

    #[test]
    fn rim_bars_stand_on_the_rim_and_tilt_with_the_kicks() {
        let board = Board { right: [1.0, 0.0, 0.0], up: [0.0, 1.0, 0.0], forward: [0.0, 0.0, 1.0], pos: [10.0, 0.0, 20.0] };
        let (y0, kick, kick_len) = (0.0975f32, 25.0f32, 0.15f32);
        let rim = crate::rim::Rim::new(crate::rim::synthetic(256, 0.21, 0.81, y0, kick, kick_len)).unwrap();
        let n = 64;
        let s = BarSettings { enabled: true, layout: Layout::Rim, count: n as u32, style: Style::Solid, ..BarSettings::default() };
        assert_eq!((s.levels_needed(), s.bars()), (n / 2 + 1, n));
        let places = rim_bars(&s, &board, &rim, n);
        let top = y0 + kick_len * kick.to_radians().tan();
        // bar 0 by the nose's centre: on the kick's end, pointing out along it (25 degrees up)
        let (b, o, _) = places[0];
        assert!(b[2] - 20.0 > 0.39 && (b[1] - top).abs() < 0.01, "nose bar at {b:?}");
        assert!((o[1] - kick.to_radians().sin()).abs() < 0.05 && o[2] > 0.85, "nose bar out {o:?}");
        // the bars either side of the tail's centre: pointing back and up the same way
        let (b, o, _) = places[n / 2];
        assert!(
            b[2] - 20.0 < -0.39 && (b[1] - top).abs() < 0.01 && (o[1] - kick.to_radians().sin()).abs() < 0.05 && o[2] < -0.85,
            "tail bar at {b:?} out {o:?}"
        );
        // at the sides: level, on the edge at its height, straight out
        let (b, o, _) = places[n / 4];
        assert!((b[0] - 10.105).abs() < 1e-3 && (b[1] - y0).abs() < 1e-4 && o[0] > 0.999, "side bar at {b:?} out {o:?}");
        for k in 0..n {
            let (b, o, a) = places[k];
            assert!((length(o) - 1.0).abs() < 1e-4 && (length(a) - 1.0).abs() < 1e-4 && dot(o, a).abs() < 1e-4);
            // on the rim (within its point spacing), the two halves mirror images
            let near = rim.path.points.iter().map(|p| length(sub(add(*p, [10.0, 0.0, 20.0]), b))).fold(f32::INFINITY, f32::min);
            assert!(near < rim.length() / 256.0, "bar {k} is {near} m off the rim");
            let (m, mo, _) = places[n - 1 - k];
            assert!(
                (m[0] - 10.0 + (b[0] - 10.0)).abs() < 1e-4 && (m[1] - b[1]).abs() < 1e-4 && (m[2] - b[2]).abs() < 1e-4 && (mo[0] + o[0]).abs() < 1e-4,
                "bars {k} and {} mirror",
                n - 1 - k
            );
        }
        // rim_out and rim_lift move the bases: out along the surface, off it along the normal (at a side: x and y)
        let moved = rim_bars(&BarSettings { rim_out: 0.02, rim_lift: -0.01, ..s.clone() }, &board, &rim, n);
        let d = sub(moved[n / 4].0, places[n / 4].0);
        assert!((d[0] - 0.02).abs() < 1e-4 && (d[1] + 0.01).abs() < 1e-4 && d[2].abs() < 1e-4, "{d:?}");
        // the quads: on the rim with one; without, exactly the ring's
        let cam = Camera { right: [1.0, 0.0, 0.0], up: [0.0, 0.0, -1.0], back: [0.0, 1.0, 0.0], pos: [10.0, 3.0, 20.0], fov_y: 60.0 };
        let scene = Scene { camera: cam, board: Some(board) };
        let bars = vec![(0.1f32, 0.0f32); n];
        let (mut with, mut without, mut ring) = (Vec::new(), Vec::new(), Vec::new());
        build_on(&s, &scene, 1.0, &bars, Some(&rim), &mut with);
        build_on(&s, &scene, 1.0, &bars, None, &mut without);
        build_on(&BarSettings { layout: Layout::Ring, ..s.clone() }, &scene, 1.0, &bars, None, &mut ring);
        assert_eq!(without, ring);
        assert_eq!(with.len(), n * 6);
        for k in 0..n {
            let (b, o, _) = places[k];
            // the base edge's middle (corners 0 and 2): clip space is linear in the world point
            let mid: Vec<f32> = (0..4).map(|i| (with[k * 6].pos[i] + with[k * 6 + 2].pos[i]) * 0.5).collect();
            let want = project(&cam, 1.0, sub(b, mul(o, EDGE_ROOM)));
            assert!((0..4).all(|i| (mid[i] - want[i]).abs() < 1e-4), "bar {k}: {mid:?} vs {want:?}");
        }
    }
}
