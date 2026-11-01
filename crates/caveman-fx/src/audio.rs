//! The bars' audio source: the PC's audio output captured with WASAPI loopback (what the speakers play: the game, the
//! stream's music, anything), split into frequency bands, one per bar. A thread of its own captures and analyses;
//! the overlay only reads the published band levels ([`bands`]), so a stalled device can never hold up a frame.
//!
//! Analysis: a 2048-point FFT (Hann window) of the latest samples about 80 times a second; bands spaced
//! logarithmically from 40 Hz to 16 kHz; each band's energy in dB, tilted +3 dB per octave (music has less energy
//! up high), against a running reference (fast up, slow down: an automatic gain) over a 42 dB window, to 0..1.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

pub const MAX_BANDS: usize = 64;

/// The analysis's tuning, set by the bars' settings every frame (`tune`): the window under the reference (dB, as
/// f32 bits), per-band references or the loudest band's, the floor (0..1) and the reference's release (dB a second).
static TUNE_WINDOW: AtomicU32 = AtomicU32::new(0x4228_0000); // 42.0
static TUNE_PER_BAND: AtomicBool = AtomicBool::new(false);
static TUNE_FLOOR: AtomicU32 = AtomicU32::new(0);
static TUNE_RELEASE: AtomicU32 = AtomicU32::new(0x4100_0000); // 8.0

pub fn tune(window_db: f32, per_band: bool, floor: f32, release_db_s: f32) {
    TUNE_WINDOW.store(window_db.to_bits(), Ordering::Relaxed);
    TUNE_PER_BAND.store(per_band, Ordering::Relaxed);
    TUNE_FLOOR.store(floor.to_bits(), Ordering::Relaxed);
    TUNE_RELEASE.store(release_db_s.to_bits(), Ordering::Relaxed);
}

fn tuning() -> (f32, bool, f32, f32) {
    (
        f32::from_bits(TUNE_WINDOW.load(Ordering::Relaxed)),
        TUNE_PER_BAND.load(Ordering::Relaxed),
        f32::from_bits(TUNE_FLOOR.load(Ordering::Relaxed)),
        f32::from_bits(TUNE_RELEASE.load(Ordering::Relaxed)),
    )
}
const FFT: usize = 2048;
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;
const RANGE_DB: f32 = 42.0;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU32 = AtomicU32::new(0);
/// The published band levels (f32 bits), [`BAND_COUNT`] of them.
static BANDS: [AtomicU32; MAX_BANDS] = [ZERO; MAX_BANDS];
static BAND_COUNT: AtomicUsize = AtomicUsize::new(0);
/// How many bands the bars want (the analysis follows it).
static WANT: AtomicUsize = AtomicUsize::new(8);
static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// Analyses done (proves the capture is alive).
static FRAMES: AtomicU64 = AtomicU64::new(0);
static STATE: Mutex<State> = Mutex::new(State { thread: None, status: String::new() });

struct State {
    thread: Option<std::thread::JoinHandle<()>>,
    status: String,
}

fn set_status(text: impl Into<String>) {
    if let Ok(mut s) = STATE.lock() {
        s.status = text.into();
    }
}

/// The current band levels, resampled to `out.len()` bars (zeros while nothing is captured).
pub fn bands(out: &mut [f32]) {
    let n = out.len();
    if n == 0 {
        return;
    }
    if WANT.load(Ordering::Relaxed) != n.min(MAX_BANDS) {
        WANT.store(n.min(MAX_BANDS), Ordering::Relaxed);
    }
    let m = BAND_COUNT.load(Ordering::Acquire).min(MAX_BANDS);
    if !RUNNING.load(Ordering::Acquire) || m == 0 {
        out.iter_mut().for_each(|x| *x = 0.0);
        return;
    }
    for (k, o) in out.iter_mut().enumerate() {
        let src = if m == n { k } else { ((k as f32 + 0.5) * m as f32 / n as f32) as usize };
        *o = f32::from_bits(BANDS[src.min(m - 1)].load(Ordering::Relaxed));
    }
}

pub fn running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// What the capture is doing, for the status.
pub fn status() -> serde_json::Value {
    let text = STATE.lock().map(|s| s.status.clone()).unwrap_or_default();
    serde_json::json!({ "running": running(), "analyses": FRAMES.load(Ordering::Relaxed), "status": text })
}

/// Starts the capture thread (nothing when it already runs).
pub fn start() {
    let mut s = match STATE.lock() {
        Ok(s) => s,
        Err(e) => e.into_inner(),
    };
    if s.thread.as_ref().is_some_and(|t| !t.is_finished()) {
        return;
    }
    STOP.store(false, Ordering::Release);
    s.status = "starting".into();
    s.thread = std::thread::Builder::new()
        .name("caveman-fx audio".into())
        .spawn(|| {
            let r = std::panic::catch_unwind(|| unsafe { capture() });
            RUNNING.store(false, Ordering::Release);
            match r {
                Ok(Ok(())) => set_status("stopped"),
                Ok(Err(e)) => set_status(format!("stopped: {e}")),
                Err(_) => set_status("stopped: panicked"),
            }
        })
        .ok();
}

/// Stops the capture thread and waits for it (at most about a second).
pub fn stop() {
    STOP.store(true, Ordering::Release);
    let thread = STATE.lock().ok().and_then(|mut s| s.thread.take());
    if let Some(t) = thread {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while !t.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if t.is_finished() {
            let _ = t.join();
        }
    }
    RUNNING.store(false, Ordering::Release);
    for b in &BANDS {
        b.store(0, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- analysis

/// An in-place radix-2 FFT (re, im; length a power of two).
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
        }
        len <<= 1;
    }
}

/// Turns sample windows into band levels.
pub struct Analyzer {
    rate: f32,
    window: Vec<f32>,
    reference: f32,
    /// Each band's own reference (`Track::Band`).
    references: Vec<f32>,
}

impl Analyzer {
    pub fn new(rate: f32) -> Analyzer {
        let window = (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT - 1) as f32).cos()).collect();
        Analyzer { rate, window, reference: -60.0, references: Vec::new() }
    }

    /// The levels (0..1) of `bands` bands for the last [`FFT`] samples of `samples` (mono). `dt` seconds since the
    /// last call (for the gain's release).
    pub fn analyse(&mut self, samples: &[f32], bands: usize, dt: f32) -> Vec<f32> {
        let mut re = vec![0.0f32; FFT];
        let mut im = vec![0.0f32; FFT];
        let start = samples.len().saturating_sub(FFT);
        for (i, &s) in samples[start..].iter().enumerate() {
            re[i] = s * self.window[i];
        }
        fft(&mut re, &mut im);
        let bin_hz = self.rate / FFT as f32;
        let ratio = (HIGH_HZ.min(self.rate * 0.45) / LOW_HZ).max(1.01);
        let mut db = Vec::with_capacity(bands);
        for b in 0..bands {
            let f0 = LOW_HZ * ratio.powf(b as f32 / bands as f32);
            let f1 = LOW_HZ * ratio.powf((b + 1) as f32 / bands as f32);
            let lo = ((f0 / bin_hz).floor() as usize).clamp(1, FFT / 2 - 1);
            let hi = ((f1 / bin_hz).ceil() as usize).clamp(lo + 1, FFT / 2);
            let energy: f32 = (lo..hi).map(|k| re[k] * re[k] + im[k] * im[k]).sum::<f32>() / (hi - lo) as f32;
            let centre = (f0 * f1).sqrt();
            let tilt = 3.0 * (centre / LOW_HZ).log2();
            db.push(10.0 * (energy + 1e-12).log10() + tilt);
        }
        let (range, per_band, floor, release) = tuning();
        let range = if range.is_finite() && range > 0.0 { range } else { RANGE_DB };
        let release = if release.is_finite() && release > 0.0 { release } else { 8.0 };
        let peak = db.iter().copied().fold(f32::MIN, f32::max);
        // the reference follows the loudest band: up at once, down `release` dB a second
        self.reference = if peak > self.reference { peak } else { (self.reference - release * dt).max(peak) };
        // and each band its own, for Track::Band
        if self.references.len() != bands {
            self.references = vec![-60.0; bands];
        }
        for (r, &d) in self.references.iter_mut().zip(&db) {
            *r = if d > *r { d } else { (*r - release * dt).max(d) };
        }
        let silent = samples[start..].iter().all(|s| s.abs() < 1e-5);
        let floor = floor.clamp(0.0, 0.95);
        db.iter()
            .enumerate()
            .map(|(b, &d)| {
                if silent {
                    return 0.0;
                }
                let reference = if per_band { self.references[b] } else { self.reference };
                let level = ((d - (reference - range)) / range).clamp(0.0, 1.0);
                ((level - floor) / (1.0 - floor)).clamp(0.0, 1.0)
            })
            .collect()
    }
}

// ---------------------------------------------------------------- capture

struct ComScope;

impl Drop for ComScope {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

/// The capture loop (runs until [`stop`]).
unsafe fn capture() -> Result<(), String> {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;

    CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(|e| format!("CoInitializeEx: {e}"))?;
    let _com = ComScope;
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("no audio device enumerator: {e}"))?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| format!("no default output device: {e}"))?;
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| format!("cannot open the output device: {e}"))?;
    let fmt = client.GetMixFormat().map_err(|e| format!("no mix format: {e}"))?;
    let format = read_format(fmt);
    let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 2_000_000, 0, fmt, None);
    CoTaskMemFree(Some(fmt as *const _));
    let format = format?;
    init.map_err(|e| format!("loopback capture refused: {e}"))?;
    let capture: IAudioCaptureClient = client.GetService().map_err(|e| format!("no capture client: {e}"))?;
    client.Start().map_err(|e| format!("cannot start the capture: {e}"))?;
    RUNNING.store(true, Ordering::Release);
    set_status(format!("capturing the default output ({} Hz, {} channels, {})", format.rate, format.channels, format.kind.name()));
    let mut analyzer = Analyzer::new(format.rate as f32);
    let mut ring: Vec<f32> = Vec::with_capacity(FFT * 4);
    let mut last = std::time::Instant::now();
    let mut quiet_since = std::time::Instant::now();
    while !STOP.load(Ordering::Acquire) {
        let mut got = false;
        loop {
            let packet = capture.GetNextPacketSize().map_err(|e| format!("capture failed: {e}"))?;
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).map_err(|e| format!("capture failed: {e}"))?;
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
            for f in 0..frames as usize {
                ring.push(if silent { 0.0 } else { format.mono(data, f) });
            }
            capture.ReleaseBuffer(frames).map_err(|e| format!("capture failed: {e}"))?;
            got = true;
        }
        if ring.len() > FFT * 2 {
            ring.drain(..ring.len() - FFT);
        }
        let now = std::time::Instant::now();
        if got {
            quiet_since = now;
        } else if now.duration_since(quiet_since).as_millis() > 150 {
            // nothing playing: the loopback stream delivers no packets at all; feed silence
            ring.extend(std::iter::repeat_n(0.0, (format.rate as usize / 100).max(1)));
        }
        let dt = now.duration_since(last).as_secs_f32();
        if dt >= 0.012 && ring.len() >= FFT / 2 {
            last = now;
            let want = WANT.load(Ordering::Relaxed).clamp(1, MAX_BANDS);
            let levels = analyzer.analyse(&ring, want, dt);
            for (k, l) in levels.iter().enumerate() {
                BANDS[k].store(l.to_bits(), Ordering::Relaxed);
            }
            BAND_COUNT.store(want, Ordering::Release);
            FRAMES.fetch_add(1, Ordering::Relaxed);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let _ = client.Stop();
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum SampleKind {
    F32,
    I16,
    I24,
    I32,
}

impl SampleKind {
    fn name(self) -> &'static str {
        match self {
            SampleKind::F32 => "float",
            SampleKind::I16 => "16-bit",
            SampleKind::I24 => "24-bit",
            SampleKind::I32 => "32-bit",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Format {
    rate: u32,
    channels: usize,
    block: usize,
    kind: SampleKind,
}

impl Format {
    /// Frame `f`'s channels averaged.
    unsafe fn mono(&self, data: *const u8, f: usize) -> f32 {
        let frame = data.add(f * self.block);
        let bytes = self.block / self.channels.max(1);
        let mut sum = 0.0f32;
        for c in 0..self.channels {
            let p = frame.add(c * bytes);
            sum += match self.kind {
                SampleKind::F32 => std::ptr::read_unaligned(p as *const f32),
                SampleKind::I16 => std::ptr::read_unaligned(p as *const i16) as f32 / 32768.0,
                SampleKind::I24 => (((*p as i32) << 8 | (*p.add(1) as i32) << 16 | (*p.add(2) as i32) << 24) >> 8) as f32 / 8_388_608.0,
                SampleKind::I32 => std::ptr::read_unaligned(p as *const i32) as f32 / 2_147_483_648.0,
            };
        }
        sum / self.channels.max(1) as f32
    }
}

/// The mix format: WAVEFORMATEX (packed), or WAVEFORMATEXTENSIBLE (its SubFormat's first dword: 1 PCM, 3 float).
unsafe fn read_format(fmt: *const windows::Win32::Media::Audio::WAVEFORMATEX) -> Result<Format, String> {
    if fmt.is_null() {
        return Err("no mix format".into());
    }
    let b = fmt as *const u8;
    let tag = std::ptr::read_unaligned(b as *const u16);
    let channels = std::ptr::read_unaligned(b.add(2) as *const u16) as usize;
    let rate = std::ptr::read_unaligned(b.add(4) as *const u32);
    let block = std::ptr::read_unaligned(b.add(12) as *const u16) as usize;
    let bits = std::ptr::read_unaligned(b.add(14) as *const u16);
    let extra = std::ptr::read_unaligned(b.add(16) as *const u16);
    let sub = if tag == 0xFFFE && extra >= 22 { std::ptr::read_unaligned(b.add(24) as *const u32) as u16 } else { tag };
    let kind = match (sub, bits) {
        (3, 32) => SampleKind::F32,
        (1, 16) => SampleKind::I16,
        (1, 24) => SampleKind::I24,
        (1, 32) => SampleKind::I32,
        _ => return Err(format!("unsupported mix format (tag {tag:#x}, sub {sub}, {bits} bits)")),
    };
    if channels == 0 || rate < 8000 || block < channels {
        return Err(format!("odd mix format ({channels} channels, {rate} Hz, block {block})"));
    }
    Ok(Format { rate, channels, block, kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_finds_a_tone() {
        let n = 1024;
        let mut re: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * 37.0 * i as f32 / n as f32).sin()).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let mag: Vec<f32> = (0..n / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).collect();
        let peak = mag.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, 37);
        assert!((mag[37] - n as f32 / 2.0).abs() < 1.0);
    }

    #[test]
    fn bands_follow_the_music() {
        let rate = 48_000.0;
        let mut a = Analyzer::new(rate);
        let tone = |hz: f32| -> Vec<f32> { (0..FFT).map(|i| 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate).sin()).collect() };
        // a bass note lights the low bars, a hi-hat-like tone the high ones
        let low = a.analyse(&tone(60.0), 8, 0.016);
        let high = a.analyse(&tone(9000.0), 8, 0.016);
        let argmax = |v: &[f32]| v.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).unwrap().0;
        assert!(argmax(&low) <= 1, "{low:?}");
        assert!(argmax(&high) >= 6, "{high:?}");
        assert!(low.iter().chain(&high).all(|x| (0.0..=1.0).contains(x)));
        // silence: all zero
        assert!(a.analyse(&vec![0.0; FFT], 8, 0.016).iter().all(|&x| x == 0.0));
    }

    #[test]
    fn published_bands_resample() {
        for (k, v) in [0.1f32, 0.9].iter().enumerate() {
            BANDS[k].store(v.to_bits(), Ordering::Relaxed);
        }
        BAND_COUNT.store(2, Ordering::Release);
        RUNNING.store(true, Ordering::Release);
        let mut out = [0.0f32; 4];
        bands(&mut out);
        assert_eq!(out, [0.1, 0.1, 0.9, 0.9]);
        RUNNING.store(false, Ordering::Release);
        bands(&mut out);
        assert_eq!(out, [0.0; 4]);
    }
}
