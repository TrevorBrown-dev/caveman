//! The deck's rim, for the bars' `rim` layout: a closed path round the deck's real edge (nose, sides, tail, the kicks
//! included) in the board's frame, with the way out along the deck's surface and the surface normal at each point.
//! The trainer reads it from the stock deck mesh in the player's install (`caveman-trainer bars rim load`;
//! caveman-core's `rim::extract`) and sends it over the pipe ([`RimPath`]); nothing of the game's geometry is in the
//! DLL. Without a rim the `rim` layout draws the `ring`.
//!
//! The frame is the board transform's (`bars::Board`): x across (right), y up, z along with the nose at +z, the origin
//! on the ground under the deck's middle. The path starts at the nose's centre and goes towards +x, as the ring's
//! outline does (so the bars' bands are the ring's).

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::bars::{add, cross, dot, length, mul, normalize, sub, V3};

/// The rim as the trainer sends it: `points` round the edge, `out` (the way out along the surface) and `normal` (the
/// deck's surface normal) at each, in the board's frame (metres); a closed loop, the last point joining the first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RimPath {
    /// Where it came from (`deck_gen_popsicle_mesh LOD0`).
    #[serde(default)]
    pub label: String,
    pub points: Vec<[f32; 3]>,
    pub out: Vec<[f32; 3]>,
    pub normal: Vec<[f32; 3]>,
}

pub const MIN_POINTS: usize = 8;
pub const MAX_POINTS: usize = 4096;

/// A point of the rim: where, the way out along the deck's surface, the surface normal, and the way along the edge
/// (`normal` x `out`); all but `point` unit length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RimPoint {
    pub point: V3,
    pub out: V3,
    pub normal: V3,
    pub along: V3,
}

/// A checked rim, with its length round.
#[derive(Clone, Debug, PartialEq)]
pub struct Rim {
    pub path: RimPath,
    /// The distance round to each point (the first 0; one more at the end: the whole loop).
    cum: Vec<f32>,
}

impl Rim {
    /// The rim of a path, checked: 8 to 4096 points, all finite and within 2 m of the board, `out` and `normal` not
    /// zero (made unit length), no gap over 25 cm.
    pub fn new(mut path: RimPath) -> Result<Rim, String> {
        let n = path.points.len();
        if !(MIN_POINTS..=MAX_POINTS).contains(&n) {
            return Err(format!("a rim of {n} points: {MIN_POINTS} to {MAX_POINTS}"));
        }
        if path.out.len() != n || path.normal.len() != n {
            return Err(format!("a rim of {n} points needs {n} outs and normals ({} and {})", path.out.len(), path.normal.len()));
        }
        for i in 0..n {
            let p = path.points[i];
            if !p.iter().all(|v| v.is_finite() && v.abs() <= 2.0) {
                return Err(format!("rim point {i} {p:?} is not within 2 m of the board"));
            }
            path.out[i] = normalize(path.out[i]).ok_or_else(|| format!("rim point {i} has no way out"))?;
            path.normal[i] = normalize(path.normal[i]).ok_or_else(|| format!("rim point {i} has no normal"))?;
        }
        let mut cum = Vec::with_capacity(n + 1);
        cum.push(0.0f32);
        for i in 0..n {
            let gap = length(sub(path.points[(i + 1) % n], path.points[i]));
            if gap > 0.25 {
                return Err(format!("a gap of {gap:.3} m after rim point {i}"));
            }
            cum.push(cum[i] + gap);
        }
        if cum[n] < 0.1 {
            return Err(format!("a rim {:.3} m round", cum[n]));
        }
        Ok(Rim { path, cum })
    }

    pub fn len(&self) -> usize {
        self.path.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.path.points.is_empty()
    }

    /// The loop's length (metres).
    pub fn length(&self) -> f32 {
        self.cum[self.len()]
    }

    /// The rim `along` metres round from its first point (wrapping round), between the two points it falls between.
    pub fn at(&self, along: f32) -> RimPoint {
        let n = self.len();
        let total = self.length();
        let s = along.rem_euclid(total);
        let i = self.cum.partition_point(|&c| c <= s).clamp(1, n) - 1;
        let j = (i + 1) % n;
        let t = ((s - self.cum[i]) / (self.cum[i + 1] - self.cum[i]).max(1e-9)).clamp(0.0, 1.0);
        let p = &self.path;
        let lerp = |a: V3, b: V3| add(a, mul(sub(b, a), t));
        let normal = normalize(lerp(p.normal[i], p.normal[j])).unwrap_or(p.normal[i]);
        let out = lerp(p.out[i], p.out[j]);
        // the way out kept in the surface (perpendicular to the normal)
        let out = normalize(sub(out, mul(normal, dot(out, normal)))).unwrap_or(p.out[i]);
        let along = normalize(cross(normal, out)).unwrap_or([0.0, 0.0, 1.0]);
        RimPoint { point: lerp(p.points[i], p.points[j]), out, normal, along }
    }

    /// The points' bounding box (min, max).
    pub fn bounds(&self) -> (V3, V3) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for q in &self.path.points {
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        (lo, hi)
    }

    /// What the status shows.
    pub fn status(&self) -> Value {
        let (lo, hi) = self.bounds();
        let r3 = |v: V3| v.map(|x| (x * 10_000.0).round() / 10_000.0);
        let n = self.len();
        json!({
            "loaded": true,
            "label": self.path.label,
            "points": n,
            "length": (self.length() * 1000.0).round() / 1000.0,
            "min": r3(lo),
            "max": r3(hi),
            // the size seen from above, the heights of the nose's and tail's centres
            "size": [((hi[0] - lo[0]) * 1000.0).round() / 1000.0, ((hi[2] - lo[2]) * 1000.0).round() / 1000.0],
            "nose_y": (self.path.points[0][1] * 1000.0).round() / 1000.0,
            "tail_y": (self.path.points[n / 2][1] * 1000.0).round() / 1000.0,
        })
    }
}

static RIM: Mutex<Option<Arc<Rim>>> = Mutex::new(None);

/// The rim the bars use now, if one was sent.
pub fn current() -> Option<Arc<Rim>> {
    RIM.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Uses a rim from now on.
pub fn set(rim: Rim) {
    *RIM.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(rim));
}

/// Forgets the rim (the `rim` layout draws the ring).
pub fn clear() {
    *RIM.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The rim's state for the status.
pub fn status() -> Value {
    match current() {
        Some(r) => r.status(),
        None => json!({"loaded": false}),
    }
}

/// `%LOCALAPPDATA%\Caveman\fx\rim.json`: the last rim sent in the game, used again after the DLL is loaded anew (it
/// came from the player's own install).
fn saved_path() -> Option<std::path::PathBuf> {
    crate::worker::data_dir().map(|d| d.join("rim.json"))
}

/// Keeps a rim for the next load (in the game only).
pub fn save(path: &RimPath) {
    let Some(file) = saved_path() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(path) {
        let _ = std::fs::write(&file, text);
    }
}

/// Forgets the kept rim.
pub fn forget_saved() {
    if let Some(file) = saved_path() {
        let _ = std::fs::remove_file(file);
    }
}

/// Loads the kept rim, if there is one (in the game only). Returns what happened, for the log.
pub fn load_saved() -> Option<String> {
    let file = saved_path()?;
    let text = std::fs::read_to_string(&file).ok()?;
    match serde_json::from_str::<RimPath>(&text).map_err(|e| e.to_string()).and_then(Rim::new) {
        Ok(rim) => {
            let line = format!("deck rim loaded from {} ({} points, {:.3} m round)", file.display(), rim.len(), rim.length());
            set(rim);
            Some(line)
        }
        Err(e) => Some(format!("deck rim in {} ignored: {e}", file.display())),
    }
}

/// A made-up kicked rim for tests (no game geometry): the ring's stadium outline (`width` by `length`), its edge at
/// `y0`, rising at `kick_deg` over the last `kick_len` metres at each end, `n` points; the way out tilted with the
/// kicks, the normals leaning back over the deck there.
pub fn synthetic(n: usize, width: f32, length: f32, y0: f32, kick_deg: f32, kick_len: f32) -> RimPath {
    let per = crate::bars::outline_length(width, length, width * 0.5);
    let flat = length * 0.5 - kick_len;
    let slope = kick_deg.to_radians().tan();
    let mut path = RimPath { label: format!("synthetic {kick_deg} degree kicks"), points: Vec::new(), out: Vec::new(), normal: Vec::new() };
    for k in 0..n {
        let (x, z, nx, nz) = crate::bars::outline_point(width, length, width * 0.5, k as f32 * per / n as f32);
        let past = z.abs() - flat;
        let y = y0 + past.max(0.0) * slope;
        // the surface's gradient: rising with |z| past the flat
        let normal = if past > 0.0 { normalize([0.0, 1.0, -z.signum() * slope]).unwrap() } else { [0.0, 1.0, 0.0] };
        let flat_out = [nx, 0.0, nz];
        let out = normalize(sub(flat_out, mul(normal, dot(flat_out, normal)))).unwrap();
        path.points.push([x, y, z]);
        path.out.push(out);
        path.normal.push(normal);
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square rim 0.2 on a side at height 0.1, its corners and mid-sides, way out level.
    fn square() -> RimPath {
        let pts: Vec<[f32; 3]> =
            vec![[0.0, 0.1, 0.1], [0.1, 0.1, 0.1], [0.1, 0.1, 0.0], [0.1, 0.1, -0.1], [0.0, 0.1, -0.1], [-0.1, 0.1, -0.1], [-0.1, 0.1, 0.0], [-0.1, 0.1, 0.1]];
        let out = pts.iter().map(|p| normalize([p[0], 0.0, p[2]]).unwrap()).collect();
        RimPath { label: "square".into(), points: pts, out, normal: vec![[0.0, 2.0, 0.0]; 8] }
    }

    #[test]
    fn checks_and_walks_round() {
        let rim = Rim::new(square()).unwrap();
        assert!((rim.length() - 0.8).abs() < 1e-5);
        assert_eq!(rim.path.normal[0], [0.0, 1.0, 0.0]); // made unit length
        let a = rim.at(0.05);
        assert!(length(sub(a.point, [0.05, 0.1, 0.1])) < 1e-5, "{a:?}");
        assert!(dot(a.out, a.normal).abs() < 1e-5 && (length(a.out) - 1.0).abs() < 1e-5 && (length(a.along) - 1.0).abs() < 1e-5);
        // wraps round both ways
        assert!(length(sub(rim.at(0.85).point, a.point)) < 1e-5 && length(sub(rim.at(-0.75).point, a.point)) < 1e-5);
        // the last stretch joins the last point to the first
        assert!(length(sub(rim.at(0.75).point, [-0.05, 0.1, 0.1])) < 1e-5);
        assert_eq!(rim.status()["points"], 8);
        let mut bad = square();
        bad.out.pop();
        assert!(Rim::new(bad).is_err());
        let mut bad = square();
        bad.points[3] = [f32::NAN, 0.0, 0.0];
        assert!(Rim::new(bad).is_err());
        let mut bad = square();
        bad.points[3] = [1.0, 0.1, -0.1];
        assert!(Rim::new(bad).unwrap_err().contains("gap"));
        let mut bad = square();
        bad.normal[2] = [0.0; 3];
        assert!(Rim::new(bad).is_err());
    }
}
