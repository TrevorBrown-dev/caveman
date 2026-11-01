//! The board cam: the skateboard's meshes (the deck, the trucks, the wheels), read from the player's own install by
//! the trainer (`caveman-trainer pip load`: caveman-core's `mesh`, no geometry shipped) and sent here, drawn by the
//! overlay in a corner of the screen from a fixed viewpoint, turned as the board under the skater is this frame
//! (`overlay.rs`, the `pip` settings of `bars::BarSettings`). The deck's underside shows the board's live art.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A run of the index buffer drawn as one thing: the deck (its underside takes the art), the trucks or the wheels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipPart {
    pub kind: String,
    pub first_index: u32,
    pub index_count: u32,
}

/// The meshes as the trainer sends them: six floats a vertex (x, y, z, then the unit normal), in the board's frame
/// (metres: x across to the right, y up, z along with the nose at +z, the origin on the ground under the deck's
/// middle); triangle indices; the parts; the deck's bounds (min, max), which place the art.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipMesh {
    #[serde(default)]
    pub label: String,
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub parts: Vec<PipPart>,
    pub bounds: [[f32; 3]; 2],
}

pub const MAX_VERTICES: usize = 400_000;
pub const KINDS: [&str; 3] = ["deck", "trucks", "wheels"];

impl PipMesh {
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / 6
    }

    /// Err names what is wrong with it.
    pub fn check(&self) -> Result<(), String> {
        if !self.vertices.len().is_multiple_of(6) {
            return Err("the vertices are not six floats each".into());
        }
        let n = self.vertex_count();
        if !(3..=MAX_VERTICES).contains(&n) {
            return Err(format!("{n} vertices (3 to {MAX_VERTICES})"));
        }
        if !self.indices.len().is_multiple_of(3) || self.indices.is_empty() {
            return Err("the indices are not whole triangles".into());
        }
        if let Some(i) = self.indices.iter().find(|&&i| i as usize >= n) {
            return Err(format!("index {i} is past the {n} vertices"));
        }
        if self.vertices.iter().any(|v| !v.is_finite()) {
            return Err("a vertex is not finite".into());
        }
        if self.parts.is_empty() {
            return Err("no parts".into());
        }
        for p in &self.parts {
            if !KINDS.contains(&p.kind.as_str()) {
                return Err(format!("part kind {:?} (deck, trucks or wheels)", p.kind));
            }
            if p.index_count == 0 || p.index_count % 3 != 0 || (p.first_index as usize + p.index_count as usize) > self.indices.len() {
                return Err(format!("the {} part's indices ({} from {}) do not fit", p.kind, p.index_count, p.first_index));
            }
        }
        let (lo, hi) = (self.bounds[0], self.bounds[1]);
        if (0..3).any(|k| !(lo[k].is_finite() && hi[k].is_finite()) || hi[k] <= lo[k]) {
            return Err("the bounds are not a box".into());
        }
        Ok(())
    }
}

static MESH: Mutex<Option<Arc<PipMesh>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// What the overlay drew last: frames drawn, and whether the art was bound (for the status).
pub static DRAWN: AtomicU64 = AtomicU64::new(0);
pub static ART_BOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn note_drawn(art: bool) {
    DRAWN.fetch_add(1, Ordering::Relaxed);
    ART_BOUND.store(art, Ordering::Relaxed);
}

/// A test host's own texture for the deck's underside (a raw `ID3D12Resource` the host keeps alive, in the pixel
/// shader resource state), in place of the ridden board's art; 0: none (the game).
static TEST_ART: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn set_test_art(raw: usize) {
    TEST_ART.store(raw, Ordering::Release);
}

pub fn test_art() -> usize {
    TEST_ART.load(Ordering::Acquire)
}

/// The mesh the board cam draws now, if one was sent, and its generation (changes with every set).
pub fn current() -> Option<(Arc<PipMesh>, u64)> {
    MESH.lock().unwrap_or_else(|e| e.into_inner()).clone().map(|m| (m, GENERATION.load(Ordering::Acquire)))
}

pub fn set(mesh: PipMesh) {
    *MESH.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(mesh));
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn clear() {
    *MESH.lock().unwrap_or_else(|e| e.into_inner()) = None;
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

pub fn status() -> Value {
    match current() {
        Some((m, g)) => json!({
            "loaded": true,
            "label": m.label,
            "vertices": m.vertex_count(),
            "triangles": m.indices.len() / 3,
            "parts": m.parts.iter().map(|p| json!({"kind": p.kind, "triangles": p.index_count / 3})).collect::<Vec<_>>(),
            "bounds": m.bounds,
            "generation": g,
            "frames_drawn": DRAWN.load(Ordering::Relaxed),
            "art_bound": ART_BOUND.load(Ordering::Relaxed),
        }),
        None => json!({"loaded": false}),
    }
}

/// `%LOCALAPPDATA%\Caveman\fx\pip-mesh.json`: the last meshes sent in the game, used again after the DLL is loaded
/// anew (they came from the player's own install).
fn saved_path() -> Option<std::path::PathBuf> {
    crate::worker::data_dir().map(|d| d.join("pip-mesh.json"))
}

pub fn save(mesh: &PipMesh) {
    let Some(file) = saved_path() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string(mesh) {
        let _ = std::fs::write(&file, text);
    }
}

pub fn forget_saved() {
    if let Some(file) = saved_path() {
        let _ = std::fs::remove_file(file);
    }
}

/// Loads the kept meshes, if there are any (in the game only). What happened, for the log.
pub fn load_saved() -> Option<String> {
    let file = saved_path()?;
    let text = std::fs::read_to_string(&file).ok()?;
    match serde_json::from_str::<PipMesh>(&text).map_err(|e| e.to_string()).and_then(|m| m.check().map(|_| m)) {
        Ok(m) => {
            let line =
                format!("board cam meshes loaded from {} ({}: {} vertices, {} triangles)", file.display(), m.label, m.vertex_count(), m.indices.len() / 3);
            set(m);
            Some(line)
        }
        Err(e) => Some(format!("board cam meshes in {} ignored: {e}", file.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri() -> PipMesh {
        PipMesh {
            label: "test".into(),
            vertices: vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            indices: vec![0, 1, 2],
            parts: vec![PipPart { kind: "deck".into(), first_index: 0, index_count: 3 }],
            bounds: [[0.0, 0.0, 0.0], [1.0, 0.1, 1.0]],
        }
    }

    #[test]
    fn checks_the_wire_shape() {
        assert!(tri().check().is_ok());
        let mut m = tri();
        m.indices = vec![0, 1, 7];
        assert!(m.check().is_err());
        let mut m = tri();
        m.parts[0].kind = "grip".into();
        assert!(m.check().is_err());
        let mut m = tri();
        m.parts[0].index_count = 6;
        assert!(m.check().is_err());
        let mut m = tri();
        m.bounds = [[0.0; 3], [0.0; 3]];
        assert!(m.check().is_err());
    }
}
