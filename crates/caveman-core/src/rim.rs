//! The deck's rim: the loop round its edge (nose, sides, tail, the kicks included) as Caveman FX's `rim` bars layout
//! roots its bars on it, extracted from the stock deck mesh in the player's install (`mesh::deck_mesh`; no geometry is
//! shipped).
//!
//! The deck mesh's model space is the board transform's frame (the renderer's skateboard proxy; `caveman-fx`'s
//! `bars::Board`): x across (right), y up, z along with the nose at +z, the origin on the ground under the deck's
//! middle (the deck's underside is 8 cm up there, its nose and tail kick up to about 14 cm). The planform (x, z) is
//! convex: its outline is the convex hull of the vertices seen from above. Round that outline, each point takes the
//! height of the edge there (the middle of the edge's wall: halfway between the lowest and highest vertex close to
//! it), the deck's surface normal (its top and underside near the edge, area-weighted) and the way out along the
//! surface: the outline's outward normal, made tangent to the deck (so it tilts up with the kicks).

use crate::mesh::Mesh;

pub type V3 = [f32; 3];

/// How many points a rim has by default (about 8 mm apart round the stock deck).
pub const POINTS: usize = 256;

/// The deck's rim: `points` round its edge in the mesh's (the board's) frame, from the nose's centre towards +x
/// (the same order as the `ring` layout's outline), evenly spaced round the outline seen from above; a closed loop
/// (the last point joins the first). At each point `out` is the way out along the deck's surface (perpendicular to the
/// edge, tangent to the deck) and `normal` the deck's surface normal (up out of the grip); both unit length.
#[derive(Clone, Debug, PartialEq)]
pub struct Rim {
    pub points: Vec<V3>,
    pub out: Vec<V3>,
    pub normal: Vec<V3>,
}

impl Rim {
    /// The points' bounding box (min, max).
    pub fn bounds(&self) -> (V3, V3) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.points {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    /// The loop's length (metres).
    pub fn length(&self) -> f32 {
        let n = self.points.len();
        (0..n).map(|i| len(sub(self.points[(i + 1) % n], self.points[i]))).sum()
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> Option<V3> {
    let l = len(a);
    (l.is_finite() && l > 1e-9).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

type P2 = [f32; 2];

/// The convex hull of points in the plane, clockwise (seen with the first coordinate to the right and the second up),
/// without collinear points (Andrew's monotone chain).
fn hull_clockwise(mut pts: Vec<P2>) -> Vec<P2> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let turn = |o: P2, a: P2, b: P2| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<P2> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && turn(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<P2> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && turn(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper); // counter-clockwise
    lower.reverse();
    lower
}

/// How close to the outline (inward, metres) a vertex counts as the edge's, and how far along it.
const EDGE_BAND: f32 = 0.006;
const EDGE_WINDOW: f32 = 0.012;
/// Where the surface normal is taken: triangles this far in from the outline (past the rounded edge), this far
/// along it, facing within 60 degrees of up or down.
const SURFACE_IN: (f32, f32) = (0.008, 0.045);
const SURFACE_WINDOW: f32 = 0.025;
const SURFACE_MIN_UP: f32 = 0.5;

/// The rim of a deck mesh, `n` points round (at least 16). Err when the mesh has no planform to go round.
pub fn extract(mesh: &Mesh, n: usize) -> Result<Rim, String> {
    let n = n.clamp(16, 4096);
    let plan: Vec<P2> = mesh.positions.iter().map(|p| [p[0], p[2]]).collect();
    let hull = hull_clockwise(plan.clone());
    if hull.len() < 3 {
        return Err(format!("the mesh's {} vertices have no outline", mesh.positions.len()));
    }
    // start where the outline crosses x = 0 at the nose (+z), going towards +x
    let m = hull.len();
    let start = (0..m)
        .filter_map(|i| {
            let (a, b) = (hull[i], hull[(i + 1) % m]);
            (a[0] <= 0.0 && b[0] > 0.0).then(|| {
                let t = -a[0] / (b[0] - a[0]);
                (i, [0.0, a[1] + t * (b[1] - a[1])])
            })
        })
        .max_by(|x, y| x.1[1].total_cmp(&y.1[1]))
        .ok_or("the outline never crosses the deck's middle (x = 0)")?;
    let mut ring: Vec<P2> = vec![start.1];
    for k in 1..=m {
        ring.push(hull[(start.0 + k) % m]);
    }
    ring.push(start.1);
    ring.dedup();
    let mut cum = vec![0.0f32];
    for w in ring.windows(2) {
        let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        cum.push(cum.last().unwrap() + d);
    }
    let total = *cum.last().unwrap();
    if !(total.is_finite() && total > 0.05) {
        return Err(format!("the outline is {total} m round"));
    }
    let at = |s: f32| -> P2 {
        let s = s.rem_euclid(total);
        let i = cum.partition_point(|&c| c <= s).clamp(1, cum.len() - 1);
        let (a, b) = (ring[i - 1], ring[i]);
        let span = (cum[i] - cum[i - 1]).max(1e-9);
        let t = ((s - cum[i - 1]) / span).clamp(0.0, 1.0);
        [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
    };
    let step = total / n as f32;
    let samples: Vec<P2> = (0..n).map(|k| at(k as f32 * step)).collect();
    // the triangles' centres and unit normals turned to face up, for the surface normal
    let faces: Vec<(P2, V3, f32)> = mesh
        .triangles
        .iter()
        .filter_map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            let nrm = cross(sub(b, a), sub(c, a));
            let area = len(nrm) * 0.5;
            let mut u = unit(nrm)?;
            if u[1] < 0.0 {
                u = [-u[0], -u[1], -u[2]];
            }
            (u[1] >= SURFACE_MIN_UP).then(|| ([(a[0] + b[0] + c[0]) / 3.0, (a[2] + b[2] + c[2]) / 3.0], u, area))
        })
        .collect();
    let mut rim = Rim { points: Vec::with_capacity(n), out: Vec::with_capacity(n), normal: Vec::with_capacity(n) };
    for k in 0..n {
        let p = samples[k];
        // the outline's direction here (over a few millimetres each way) and its outward normal: going round clockwise
        // (x right, z up), out is the direction turned a quarter turn anticlockwise, (-tz, tx)
        let (a, b) = (at(k as f32 * step - 0.004), at(k as f32 * step + 0.004));
        let t = unit([b[0] - a[0], 0.0, b[1] - a[1]]).ok_or("the outline doubles back on itself")?;
        let t2 = [t[0], t[2]];
        let n2 = [-t2[1], t2[0]];
        // the edge's height: halfway between the lowest and highest vertex close to the outline here
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for (band, window) in [(EDGE_BAND, EDGE_WINDOW), (EDGE_BAND * 3.0, EDGE_WINDOW * 3.0)] {
            for (q, pos) in plan.iter().zip(&mesh.positions) {
                let d = [p[0] - q[0], p[1] - q[1]];
                let inward = d[0] * n2[0] + d[1] * n2[1];
                let along = (d[0] * t2[0] + d[1] * t2[1]).abs();
                if (-0.001..=band).contains(&inward) && along <= window {
                    lo = lo.min(pos[1]);
                    hi = hi.max(pos[1]);
                }
            }
            if lo.is_finite() {
                break;
            }
        }
        if !lo.is_finite() {
            return Err(format!("no vertex near the outline at ({:.3}, {:.3})", p[0], p[1]));
        }
        // the surface normal: the deck's faces a little way in, area-weighted
        let mut sum = [0.0f32; 3];
        for (c, u, area) in &faces {
            let d = [p[0] - c[0], p[1] - c[1]];
            let inward = d[0] * n2[0] + d[1] * n2[1];
            let along = (d[0] * t2[0] + d[1] * t2[1]).abs();
            if (SURFACE_IN.0..=SURFACE_IN.1).contains(&inward) && along <= SURFACE_WINDOW {
                for i in 0..3 {
                    sum[i] += u[i] * area;
                }
            }
        }
        let normal = unit(sum).unwrap_or([0.0, 1.0, 0.0]);
        let flat_out = [n2[0], 0.0, n2[1]];
        let out =
            unit(sub(flat_out, [normal[0] * dot(flat_out, normal), normal[1] * dot(flat_out, normal), normal[2] * dot(flat_out, normal)])).unwrap_or(flat_out);
        rim.points.push([p[0], (lo + hi) * 0.5, p[1]]);
        rim.out.push(out);
        rim.normal.push(normal);
    }
    Ok(rim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Mesh, Section};

    /// A synthetic deck: a slab `w` wide and `l` long (a stadium planform: half-circle ends), 1 cm thick, its underside
    /// at `y0` in the middle, kicking up by `kick` radians over the last `kick_len` metres at each end (gentler at the
    /// tail when `tail` is below 1). Rows across, columns along; top and bottom faces and the side walls.
    pub(crate) fn kicked_deck(w: f32, l: f32, y0: f32, kick: f32, kick_len: f32) -> Mesh {
        let (cols, rows) = (161usize, 21usize);
        let half_l = l * 0.5;
        let r = w * 0.5;
        let flat = half_l - kick_len;
        // the slab's middle surface height and slope at z
        let lift = |z: f32| -> f32 {
            let e = z.abs() - flat;
            if e <= 0.0 {
                0.0
            } else {
                e * kick.tan()
            }
        };
        let mut m = Mesh::default();
        let thick = 0.01;
        for layer in 0..2 {
            for j in 0..cols {
                let v = j as f32 / (cols - 1) as f32; // 0 tail .. 1 nose
                let z = -half_l + v * l;
                // the half width at z (stadium: round ends of radius r)
                let ez = z.abs() - (half_l - r);
                let hw = if ez > 0.0 { (r * r - ez * ez).max(0.0).sqrt() } else { r };
                for i in 0..rows {
                    let u = i as f32 / (rows - 1) as f32 * 2.0 - 1.0;
                    let y = y0 + lift(z) + if layer == 0 { 0.0 } else { thick };
                    m.positions.push([u * hw, y, z]);
                }
            }
        }
        let idx = |layer: usize, j: usize, i: usize| (layer * cols * rows + j * rows + i) as u32;
        for layer in 0..2 {
            for j in 0..cols - 1 {
                for i in 0..rows - 1 {
                    m.triangles.push([idx(layer, j, i), idx(layer, j + 1, i), idx(layer, j, i + 1)]);
                    m.triangles.push([idx(layer, j, i + 1), idx(layer, j + 1, i), idx(layer, j + 1, i + 1)]);
                }
            }
        }
        // the side walls (the long sides' rows 0 and rows-1 between the layers)
        for j in 0..cols - 1 {
            for i in [0, rows - 1] {
                m.triangles.push([idx(0, j, i), idx(0, j + 1, i), idx(1, j, i)]);
                m.triangles.push([idx(1, j, i), idx(0, j + 1, i), idx(1, j + 1, i)]);
            }
        }
        m.sections.push(Section { first_vertex: 0, vertices: m.positions.len() as u32, first_triangle: 0, triangles: m.triangles.len() as u32 });
        m
    }

    #[test]
    fn hull_is_clockwise() {
        let h = hull_clockwise(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.5, 0.5], [0.5, 0.0]]);
        assert_eq!(h.len(), 4);
        let area: f32 = (0..4).map(|i| h[i][0] * h[(i + 1) % 4][1] - h[(i + 1) % 4][0] * h[i][1]).sum();
        assert!(area < 0.0, "{h:?}");
    }

    #[test]
    fn rim_of_a_kicked_deck() {
        let kick = 20f32.to_radians();
        let m = kicked_deck(0.21, 0.81, 0.08, kick, 0.15);
        let rim = extract(&m, 128).unwrap();
        assert_eq!((rim.points.len(), rim.out.len(), rim.normal.len()), (128, 128, 128));
        let (lo, hi) = rim.bounds();
        assert!((hi[0] - lo[0] - 0.21).abs() < 0.01 && (hi[2] - lo[2] - 0.81).abs() < 0.01, "{lo:?} {hi:?}");
        // the first point: the nose's centre; the next ones towards +x
        assert!(rim.points[0][0].abs() < 1e-4 && rim.points[0][2] > 0.4, "{:?}", rim.points[0]);
        assert!(rim.points[1][0] > 0.0);
        // the flat middle at the slab's middle height; the nose and tail kicked up
        let mid = rim.points.iter().filter(|p| p[2].abs() < 0.1).map(|p| p[1]).fold(f32::NAN, f32::max);
        assert!((mid - 0.085).abs() < 0.002, "middle at {mid}");
        let nose = rim.points[0][1];
        let tail = rim.points[64][1];
        let rise = 0.15 * kick.tan();
        assert!((nose - 0.085 - rise).abs() < 0.01 && (tail - 0.085 - rise).abs() < 0.01, "nose {nose}, tail {tail}, rise {rise}");
        for k in 0..128 {
            let (p, o, nrm) = (rim.points[k], rim.out[k], rim.normal[k]);
            assert!((len(o) - 1.0).abs() < 1e-4 && (len(nrm) - 1.0).abs() < 1e-4);
            assert!(dot(o, nrm).abs() < 1e-3, "out along the surface at {k}");
            // out points away from the middle, seen from above
            assert!(o[0] * p[0] + o[2] * p[2] > 0.0, "out at {k}: {o:?} at {p:?}");
            // neighbours agree
            let next = rim.out[(k + 1) % 128];
            assert!(dot(o, next) > 0.9, "out turns at {k}");
        }
        // at the nose tip and the tail tip the way out tilts up with the kick; at the sides it is level
        assert!((rim.out[0][1] - kick.sin()).abs() < 0.03, "nose out {:?}", rim.out[0]);
        assert!((rim.out[64][1] - kick.sin()).abs() < 0.03, "tail out {:?}", rim.out[64]);
        assert!(rim.out[32][1].abs() < 0.02 && rim.out[32][0] > 0.99, "side out {:?}", rim.out[32]);
        assert!(rim.out[96][0] < -0.99, "other side out {:?}", rim.out[96]);
    }
}
