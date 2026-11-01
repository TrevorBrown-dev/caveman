//! The MeshSet reader and the deck rim on the player's install (ignored: they need the game).
mod io_common;

use caveman_core::mesh;
use io_common::*;

#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate"]
fn deck_mesh_reads() {
    let game = game();
    let m = mesh::deck_mesh(&game).unwrap();
    let (lo, hi) = m.bounds();
    eprintln!("deck mesh: {} vertices, {} triangles, {} sections; bounds {lo:?} .. {hi:?}", m.positions.len(), m.triangles.len(), m.sections.len());
    for s in &m.sections {
        let ps = &m.positions[s.first_vertex as usize..(s.first_vertex + s.vertices) as usize];
        let mut l = [f32::INFINITY; 3];
        let mut h = [f32::NEG_INFINITY; 3];
        for p in ps {
            for k in 0..3 {
                l[k] = l[k].min(p[k]);
                h[k] = h[k].max(p[k]);
            }
        }
        eprintln!("  section {s:?}: {l:?} .. {h:?}");
    }
    assert!(m.positions.len() > 100 && m.triangles.len() > 100);
    // the board's frame: about 0.2 across (x), 0.8 along (z), the underside 8 cm up (the origin is on the ground)
    assert!((0.18..0.24).contains(&(hi[0] - lo[0])) && (0.75..0.87).contains(&(hi[2] - lo[2])), "{lo:?} {hi:?}");
    assert!((0.06..0.10).contains(&lo[1]) && hi[1] - lo[1] < 0.12, "{lo:?} {hi:?}");
}

#[test]
#[ignore = "needs the game at E:\\Games\\ReSkate"]
fn deck_rim_goes_round_the_kicks() {
    let game = game();
    let m = mesh::deck_mesh(&game).unwrap();
    let n = caveman_core::rim::POINTS;
    let rim = caveman_core::rim::extract(&m, n).unwrap();
    let (lo, hi) = rim.bounds();
    eprintln!("rim: {} points, {:.3} m round, bounds {lo:?} .. {hi:?}", rim.points.len(), rim.length());
    for k in (0..n).step_by(n / 32) {
        let (p, o, q) = (rim.points[k], rim.out[k], rim.normal[k]);
        eprintln!(
            "  {k:3}: point ({:+.4}, {:.4}, {:+.4}) out ({:+.3}, {:+.3}, {:+.3}) normal ({:+.3}, {:+.3}, {:+.3})",
            p[0], p[1], p[2], o[0], o[1], o[2], q[0], q[1], q[2]
        );
    }
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    // a closed loop about 0.2 by 0.8, starting at the nose's centre (+z) and going towards +x
    assert_eq!(rim.points.len(), n);
    assert!((0.18..0.24).contains(&(hi[0] - lo[0])) && (0.75..0.87).contains(&(hi[2] - lo[2])), "{lo:?} {hi:?}");
    assert!(rim.points[0][0].abs() < 1e-3 && rim.points[0][2] > 0.35 && rim.points[1][0] > 0.0);
    let step = rim.length() / n as f32;
    for k in 0..n {
        let gap = (0..3).map(|i| (rim.points[(k + 1) % n][i] - rim.points[k][i]).powi(2)).sum::<f32>().sqrt();
        assert!(gap < step * 3.0, "a gap of {gap} m after point {k}");
    }
    // the flat middle at the deck's height (its underside is 8 cm up); both kicks rising well above it
    let side: Vec<f32> = rim.points.iter().filter(|p| p[2].abs() < 0.1).map(|p| p[1]).collect();
    let middle = side.iter().sum::<f32>() / side.len() as f32;
    assert!((0.08..0.11).contains(&middle), "the middle's rim at {middle}");
    let (nose, tail) = (rim.points[0][1], rim.points[n / 2][1]);
    assert!(nose > middle + 0.03 && tail > middle + 0.03, "nose {nose}, tail {tail}, middle {middle}");
    // the way out: unit, along the surface, away from the middle seen from above, turning smoothly; tilted up at the
    // kicks' ends, level-ish at the sides
    for k in 0..n {
        let (p, o, q) = (rim.points[k], rim.out[k], rim.normal[k]);
        assert!((dot(o, o).sqrt() - 1.0).abs() < 1e-3 && (dot(q, q).sqrt() - 1.0).abs() < 1e-3 && q[1] > 0.5);
        assert!(dot(o, q).abs() < 1e-3);
        assert!(o[0] * p[0] + o[2] * p[2] > 0.0, "out at {k}: {o:?} at {p:?}");
        assert!(dot(o, rim.out[(k + 1) % n]) > 0.9, "out turns at {k}");
    }
    assert!(rim.out[0][1] > 0.15 && rim.out[n / 2][1] > 0.15, "nose out {:?}, tail out {:?}", rim.out[0], rim.out[n / 2]);
    assert!(rim.out[n / 4][1].abs() < 0.2 && rim.out[n / 4][0] > 0.95, "side out {:?}", rim.out[n / 4]);
}
