//! Which encoding the skateboard meshes' usage-52 element (4 bytes a vertex) uses for the tangent space: a family of
//! candidate decodings, each scored by how often its normal agrees with the geometric vertex normal (area-weighted,
//! from the triangles). Ignored: reads the install.
//! `cargo test -p caveman-core --test mesh_normals -- --ignored --nocapture`.

use caveman_core::game::{chunk_loc, Game, ITEMS_TOC};
use caveman_core::mesh::{self, DECK_BUNDLE_SUFFIX, DECK_MESH, LEVEL_BUNDLE, LEVEL_TOC, MESHSET_TYPE, TRUCKS_MESH, WHEELS_MESH};
use std::path::Path;

const GAME: &str = r"E:\Games\ReSkate";

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn ptr_at(b: &[u8], o: usize) -> usize {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize + 16
}
fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn snorm8(b: u8) -> f32 {
    ((b as i8) as f32 / 127.0).clamp(-1.0, 1.0)
}
fn unorm8(b: u8) -> f32 {
    b as f32 / 255.0 * 2.0 - 1.0
}
fn oct(x: f32, y: f32) -> [f32; 3] {
    let mut n = [x, y, 1.0 - x.abs() - y.abs()];
    if n[2] < 0.0 {
        let (ox, oy) = (n[0], n[1]);
        n[0] = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        n[1] = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
    }
    norm(n)
}
fn rotate_axis(q: [f32; 4], k: usize) -> [f32; 3] {
    let [x, y, z, w] = q;
    match k {
        0 => [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + w * z), 2.0 * (x * z - w * y)],
        1 => [2.0 * (x * y - w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x)],
        _ => [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)],
    }
}
/// The normal as the deck vertex shader decodes the four unorm bytes (ported from its DXIL).
pub fn shader_normal(b: [u8; 4]) -> [f32; 3] {
    let u = |k: usize| b[k] as f32 / 255.0;
    let flags = (u(3) * 255.0 + 0.25) as u32;
    let s = std::f32::consts::FRAC_1_SQRT_2;
    let a = u(0) * s - s + if flags & 16 != 0 { s } else { 0.0 };
    let bb = u(1) * s - s + if flags & 32 != 0 { s } else { 0.0 };
    let w0 = (1.0 - (a * a + bb * bb)).abs().sqrt();
    let w = if flags & 1 != 0 { -w0 } else { w0 };
    let (v102, v103) = if flags & 4 != 0 { (w, a) } else { (a, w) };
    if flags & 2 != 0 {
        [bb, v103, v102]
    } else {
        [v102, bb, v103]
    }
}

fn norm4(q: [f32; 4]) -> [f32; 4] {
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-9);
    [q[0] / l, q[1] / l, q[2] / l, q[3] / l]
}
fn sn(v: u32, bits: u32) -> f32 {
    let half = 1u32 << (bits - 1);
    let i = if v >= half { v as i32 - (1 << bits) } else { v as i32 };
    (i as f32 / (half - 1) as f32).clamp(-1.0, 1.0)
}
fn un(v: u32, bits: u32) -> f32 {
    v as f32 / ((1u32 << bits) - 1) as f32 * 2.0 - 1.0
}

const PERMS: [[usize; 4]; 24] = [
    [0, 1, 2, 3],
    [0, 1, 3, 2],
    [0, 2, 1, 3],
    [0, 2, 3, 1],
    [0, 3, 1, 2],
    [0, 3, 2, 1],
    [1, 0, 2, 3],
    [1, 0, 3, 2],
    [1, 2, 0, 3],
    [1, 2, 3, 0],
    [1, 3, 0, 2],
    [1, 3, 2, 0],
    [2, 0, 1, 3],
    [2, 0, 3, 1],
    [2, 1, 0, 3],
    [2, 1, 3, 0],
    [2, 3, 0, 1],
    [2, 3, 1, 0],
    [3, 0, 1, 2],
    [3, 0, 2, 1],
    [3, 1, 0, 2],
    [3, 1, 2, 0],
    [3, 2, 0, 1],
    [3, 2, 1, 0],
];

type Cand = (String, Box<dyn Fn([u8; 4]) -> [f32; 3]>);

fn candidates() -> Vec<Cand> {
    let mut out: Vec<Cand> = Vec::new();
    for (pi, p) in PERMS.iter().enumerate() {
        for (dn, dec) in [("s8", snorm8 as fn(u8) -> f32), ("u8", unorm8 as fn(u8) -> f32)] {
            let p = *p;
            // a 3-vector from three bytes (the fourth left out), only the permutations that differ for the first three
            if pi % 2 == 0 {
                out.push((format!("vec3 {dn} bytes {},{},{}", p[0], p[1], p[2]), Box::new(move |b| norm([dec(b[p[0]]), dec(b[p[1]]), dec(b[p[2]])]))));
            }
            for k in 0..3 {
                out.push((
                    format!("quat {dn} (x,y,z,w)=bytes {:?} -> axis {k}", p),
                    Box::new(move |b| rotate_axis(norm4([dec(b[p[0]]), dec(b[p[1]]), dec(b[p[2]]), dec(b[p[3]])]), k)),
                ));
                // w reconstructed from xyz (the fourth byte a sign or flags)
                out.push((
                    format!("quat {dn} xyz=bytes {},{},{} w=sqrt -> axis {k}", p[0], p[1], p[2]),
                    Box::new(move |b| {
                        let (x, y, z) = (dec(b[p[0]]), dec(b[p[1]]), dec(b[p[2]]));
                        let w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
                        rotate_axis([x, y, z, w], k)
                    }),
                ));
            }
        }
    }
    for (a, b2) in [(0usize, 1usize), (1, 0), (2, 3), (3, 2), (0, 2), (1, 3)] {
        out.push((format!("oct s8 bytes {a},{b2}"), Box::new(move |b| oct(snorm8(b[a]), snorm8(b[b2])))));
        out.push((format!("oct u8 bytes {a},{b2}"), Box::new(move |b| oct(unorm8(b[a]), unorm8(b[b2])))));
    }
    // packed words
    out.push((
        "10/10/10 low-first s".into(),
        Box::new(|b| {
            let v = u32::from_le_bytes(b);
            norm([sn(v & 0x3ff, 10), sn((v >> 10) & 0x3ff, 10), sn((v >> 20) & 0x3ff, 10)])
        }),
    ));
    out.push((
        "10/10/10 low-first u".into(),
        Box::new(|b| {
            let v = u32::from_le_bytes(b);
            norm([un(v & 0x3ff, 10), un((v >> 10) & 0x3ff, 10), un((v >> 20) & 0x3ff, 10)])
        }),
    ));
    out.push((
        "2/10/10/10 high-first s".into(),
        Box::new(|b| {
            let v = u32::from_le_bytes(b);
            norm([sn((v >> 2) & 0x3ff, 10), sn((v >> 12) & 0x3ff, 10), sn((v >> 22) & 0x3ff, 10)])
        }),
    ));
    out.push((
        "10/10/10 big-endian s".into(),
        Box::new(|b| {
            let v = u32::from_be_bytes(b);
            norm([sn(v & 0x3ff, 10), sn((v >> 10) & 0x3ff, 10), sn((v >> 20) & 0x3ff, 10)])
        }),
    ));
    out.push((
        "11/11/10 s".into(),
        Box::new(|b| {
            let v = u32::from_le_bytes(b);
            norm([sn(v & 0x7ff, 11), sn((v >> 11) & 0x7ff, 11), sn((v >> 22) & 0x3ff, 10)])
        }),
    ));
    out.push((
        "11/11/10 u".into(),
        Box::new(|b| {
            let v = u32::from_le_bytes(b);
            norm([un(v & 0x7ff, 11), un((v >> 11) & 0x7ff, 11), un((v >> 22) & 0x3ff, 10)])
        }),
    ));
    // two 16-bit octahedral vectors (normal first): 8+8 bits each as unsigned
    out.push((
        "oct16 lo-word u".into(),
        Box::new(|b| {
            let v = u16::from_le_bytes([b[0], b[1]]);
            oct(un((v & 0xff) as u32, 8), un((v >> 8) as u32, 8))
        }),
    ));
    out.push((
        "oct16 hi-word u".into(),
        Box::new(|b| {
            let v = u16::from_le_bytes([b[2], b[3]]);
            oct(un((v & 0xff) as u32, 8), un((v >> 8) as u32, 8))
        }),
    ));
    // smallest-three quaternion: three 10-bit components and a 2-bit index of the dropped (largest) component,
    // reconstructed as positive; every bit order, byte order, number mapping, scale, slot order and axis
    for be in [false, true] {
        for low_first in [true, false] {
            for unsigned in [false, true] {
                for scale in [std::f32::consts::FRAC_1_SQRT_2, 1.0f32] {
                    for (pi, perm) in PERMS.iter().enumerate() {
                        let perm = *perm;
                        for k in 0..3 {
                            for neg in [false, true] {
                                out.push((
                                    format!(
                                        "s3 {}{} {} x{scale:.2} perm{pi} -> {}axis {k}",
                                        if be { "BE " } else { "" },
                                        if low_first { "idx-high" } else { "idx-low" },
                                        if unsigned { "un" } else { "sn" },
                                        if neg { "-" } else { "+" }
                                    ),
                                    Box::new(move |b| {
                                        let v = if be { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) };
                                        let (f, idx) = if low_first {
                                            ([v & 0x3ff, (v >> 10) & 0x3ff, (v >> 20) & 0x3ff], (v >> 30) as usize)
                                        } else {
                                            ([(v >> 2) & 0x3ff, (v >> 12) & 0x3ff, (v >> 22) & 0x3ff], (v & 3) as usize)
                                        };
                                        let c: Vec<f32> = f.iter().map(|&x| if unsigned { un(x, 10) } else { sn(x, 10) } * scale).collect();
                                        let mut q = [0f32; 4];
                                        let mut j = 0;
                                        for slot in 0..4 {
                                            if slot == idx {
                                                continue;
                                            }
                                            q[slot] = c[j];
                                            j += 1;
                                        }
                                        q[idx] = (1.0 - q.iter().map(|x| x * x).sum::<f32>()).max(0.0).sqrt();
                                        let q = norm4([q[perm[0]], q[perm[1]], q[perm[2]], q[perm[3]]]);
                                        let n = rotate_axis(q, k);
                                        if neg {
                                            [-n[0], -n[1], -n[2]]
                                        } else {
                                            n
                                        }
                                    }),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    // the deck vertex shader's own decode (TEXCOORD4 as four unorm floats; shaders/bytecode/28c6a292-...): two
    // components scaled by 1/sqrt(2) with their signs in flag bits 4 and 5, the third reconstructed with its sign in
    // bit 0 and its place from bits 1 and 2; then every permutation of the result, to be sure of the order
    for perm in PERMS.iter() {
        if perm[3] != 3 {
            continue;
        }
        let perm = *perm;
        out.push((
            format!("shader decode, components {:?}", &perm[..3]),
            Box::new(move |b| {
                let n = shader_normal(b);
                [n[perm[0]], n[perm[1]], n[perm[2]]]
            }),
        ));
    }
    // spherical: theta, phi in two bytes
    for (a, b2) in [(0usize, 1usize), (2, 3)] {
        out.push((
            format!("spherical u8 bytes {a},{b2}"),
            Box::new(move |b| {
                let theta = b[a] as f32 / 255.0 * std::f32::consts::PI;
                let phi = b[b2] as f32 / 255.0 * 2.0 * std::f32::consts::PI;
                [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()]
            }),
        ));
    }
    out
}

fn analyse(game: &Game, toc: &str, bundle: &str, name: &str, only_section: usize) {
    let b = game.bundle(bundle, toc).unwrap();
    let i = b.manifest.res.iter().position(|r| r.rtype == MESHSET_TYPE && r.name == name).unwrap();
    let res = game.decode(&b.res_loc(i).unwrap()).unwrap();
    let l0 = mesh::lod(&res, 0).unwrap();
    let geometry: Vec<u8> = match game.chunk(&l0.chunk, toc).unwrap() {
        Some(c) if !c.removed => game.decode(&chunk_loc(&c)).unwrap(),
        _ => game.decode(&b.chunk_loc(b.manifest.chunk_index(&l0.chunk).unwrap()).unwrap()).unwrap(),
    };
    let m = mesh::read(&res, 0, &geometry).unwrap();
    let mut gn = vec![[0f32; 3]; m.positions.len()];
    for t in &m.triangles {
        let [a, bb, c] = [m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]];
        let u = [bb[0] - a[0], bb[1] - a[1], bb[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let f = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &k in t {
            for j in 0..3 {
                gn[k as usize][j] += f[j];
            }
        }
    }
    let at = ptr_at(&res, 48);
    let table = ptr_at(&res, at + 12);
    println!("\n==== {name}");
    let mut first_vertex = 0usize;
    for (s, sec) in m.sections.iter().enumerate() {
        if s != only_section {
            first_vertex += sec.vertices as usize;
            continue;
        }
        let sd = table + s * 384;
        let d = sd + 112;
        let (elements, streams) = (res[d + 96] as usize, res[d + 97] as usize);
        let count = u32_at(&res, sd + 44) as usize;
        let mut bases = Vec::new();
        let mut base = u32_at(&res, sd + 40) as usize;
        for k in 0..streams {
            bases.push((base, res[d + 64 + k * 2] as usize));
            base += res[d + 64 + k * 2] as usize * count;
        }
        let ts = (0..elements).map(|e| d + e * 4).find(|&e| res[e] == 52).map(|e| res[e + 3] as usize).unwrap();
        let (tb, tstride) = bases[ts];
        let mut scored: Vec<(f32, f32, String)> = Vec::new();
        for (label, f) in candidates() {
            let (mut sum, mut agree) = (0f32, 0usize);
            for v in 0..count {
                let o = tb + v * tstride;
                let bytes: [u8; 4] = geometry[o..o + 4].try_into().unwrap();
                let dp = dot(f(bytes), norm(gn[first_vertex + v]));
                sum += dp;
                if dp > 0.9 {
                    agree += 1;
                }
            }
            scored.push((agree as f32 / count as f32, sum / count as f32, label));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        println!("  section {s} ({count} vertices): the best decodings by agreement (dot > 0.9) with the geometric normals:");
        for (agree, mean, label) in scored.iter().take(6) {
            println!("    {:>5.1}%  mean dot {mean:+.3}  {label}", agree * 100.0);
        }
        first_vertex += sec.vertices as usize;
    }
}

#[test]
#[ignore]
fn tangent_space_encoding() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let deck_bundle = game.find_bundle(DECK_BUNDLE_SUFFIX, ITEMS_TOC).unwrap().unwrap();
    analyse(&game, ITEMS_TOC, &deck_bundle, DECK_MESH, 0);
    analyse(&game, ITEMS_TOC, &deck_bundle, DECK_MESH, 1);
    analyse(&game, LEVEL_TOC, LEVEL_BUNDLE, TRUCKS_MESH, 0);
    analyse(&game, LEVEL_TOC, LEVEL_BUNDLE, WHEELS_MESH, 0);
}

/// A table of the grip section's vertices: position, geometric normal, the 4 bytes, for a look by eye.
#[test]
#[ignore]
fn grip_vertex_table() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let deck_bundle = game.find_bundle(DECK_BUNDLE_SUFFIX, ITEMS_TOC).unwrap().unwrap();
    let b = game.bundle(&deck_bundle, ITEMS_TOC).unwrap();
    let i = b.manifest.res.iter().position(|r| r.rtype == MESHSET_TYPE && r.name == DECK_MESH).unwrap();
    let res = game.decode(&b.res_loc(i).unwrap()).unwrap();
    let l0 = mesh::lod(&res, 0).unwrap();
    let geometry: Vec<u8> = match game.chunk(&l0.chunk, ITEMS_TOC).unwrap() {
        Some(c) if !c.removed => game.decode(&chunk_loc(&c)).unwrap(),
        _ => game.decode(&b.chunk_loc(b.manifest.chunk_index(&l0.chunk).unwrap()).unwrap()).unwrap(),
    };
    let m = mesh::read(&res, 0, &geometry).unwrap();
    let mut gn = vec![[0f32; 3]; m.positions.len()];
    for t in &m.triangles {
        let [a, bb, c] = [m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]];
        let u = [bb[0] - a[0], bb[1] - a[1], bb[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let f = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &k in t {
            for j in 0..3 {
                gn[k as usize][j] += f[j];
            }
        }
    }
    let at = ptr_at(&res, 48);
    let table = ptr_at(&res, at + 12);
    let sd = table; // section 0
    let d = sd + 112;
    let count = u32_at(&res, sd + 44) as usize;
    let streams = res[d + 97] as usize;
    let mut bases = Vec::new();
    let mut base = u32_at(&res, sd + 40) as usize;
    for k in 0..streams {
        bases.push((base, res[d + 64 + k * 2] as usize));
        base += res[d + 64 + k * 2] as usize * count;
    }
    let (tb, ts) = bases[5];
    let (ub, us) = bases[3];
    println!("vertex | position | geometric normal | uv0 | bytes (hex) | as i8");
    for v in (0..count).step_by(count / 40) {
        let o = tb + v * ts;
        let bytes: [u8; 4] = geometry[o..o + 4].try_into().unwrap();
        let uo = ub + v * us;
        let uv = [mesh::half(u16::from_le_bytes([geometry[uo], geometry[uo + 1]])), mesh::half(u16::from_le_bytes([geometry[uo + 2], geometry[uo + 3]]))];
        let p = m.positions[v];
        let n = norm(gn[v]);
        println!(
            "{v:>4} | ({:+.3},{:+.3},{:+.3}) | ({:+.2},{:+.2},{:+.2}) | ({:.3},{:.3}) | {:02x}{:02x}{:02x}{:02x} | ({},{},{},{})",
            p[0],
            p[1],
            p[2],
            n[0],
            n[1],
            n[2],
            uv[0],
            uv[1],
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3],
            bytes[0] as i8,
            bytes[1] as i8,
            bytes[2] as i8,
            bytes[3] as i8
        );
    }
}

/// The deck body (section 1): vertices whose normals point down, sideways and up, with their bytes.
#[test]
#[ignore]
fn body_vertex_table() {
    let game = Game::open(Path::new(GAME)).unwrap();
    let deck_bundle = game.find_bundle(DECK_BUNDLE_SUFFIX, ITEMS_TOC).unwrap().unwrap();
    let b = game.bundle(&deck_bundle, ITEMS_TOC).unwrap();
    let i = b.manifest.res.iter().position(|r| r.rtype == MESHSET_TYPE && r.name == DECK_MESH).unwrap();
    let res = game.decode(&b.res_loc(i).unwrap()).unwrap();
    let l0 = mesh::lod(&res, 0).unwrap();
    let geometry: Vec<u8> = match game.chunk(&l0.chunk, ITEMS_TOC).unwrap() {
        Some(c) if !c.removed => game.decode(&chunk_loc(&c)).unwrap(),
        _ => game.decode(&b.chunk_loc(b.manifest.chunk_index(&l0.chunk).unwrap()).unwrap()).unwrap(),
    };
    let m = mesh::read(&res, 0, &geometry).unwrap();
    let mut gn = vec![[0f32; 3]; m.positions.len()];
    for t in &m.triangles {
        let [a, bb, c] = [m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]];
        let u = [bb[0] - a[0], bb[1] - a[1], bb[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let f = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        for &k in t {
            for j in 0..3 {
                gn[k as usize][j] += f[j];
            }
        }
    }
    let at = ptr_at(&res, 48);
    let table = ptr_at(&res, at + 12);
    for section in [0usize, 1] {
        let sd = table + section * 384;
        let d = sd + 112;
        let count = u32_at(&res, sd + 44) as usize;
        let first = m.sections[section].first_vertex as usize;
        let streams = res[d + 97] as usize;
        let mut bases = Vec::new();
        let mut base = u32_at(&res, sd + 40) as usize;
        for k in 0..streams {
            bases.push((base, res[d + 64 + k * 2] as usize));
            base += res[d + 64 + k * 2] as usize * count;
        }
        let (tb, ts) = bases[5];
        let (ub, us) = bases[3];
        println!(
            "
== section {section}: classes of normals (count): bytes of a few each"
        );
        let classes: [(&str, Box<dyn Fn([f32; 3]) -> bool>); 6] = [
            ("up", Box::new(|n| n[1] > 0.97)),
            ("down", Box::new(|n| n[1] < -0.97)),
            ("+x side", Box::new(|n| n[0] > 0.9)),
            ("-x side", Box::new(|n| n[0] < -0.9)),
            ("+z end", Box::new(|n| n[2] > 0.9)),
            ("-z end", Box::new(|n| n[2] < -0.9)),
        ];
        for (label, test) in &classes {
            let mut shown = 0;
            let mut total = 0;
            let mut rows = Vec::new();
            for v in 0..count {
                let n = norm(gn[first + v]);
                if !test(n) {
                    continue;
                }
                total += 1;
                if shown < 6 {
                    shown += 1;
                    let o = tb + v * ts;
                    let bytes: [u8; 4] = geometry[o..o + 4].try_into().unwrap();
                    let uo = ub + v * us;
                    let uv = [
                        mesh::half(u16::from_le_bytes([geometry[uo], geometry[uo + 1]])),
                        mesh::half(u16::from_le_bytes([geometry[uo + 2], geometry[uo + 3]])),
                    ];
                    rows.push(format!(
                        "      v{v:<4} n=({:+.2},{:+.2},{:+.2}) uv=({:.2},{:.2}) bytes {:02x}{:02x}{:02x}{:02x} i8=({},{},{},{})",
                        n[0], n[1], n[2], uv[0], uv[1], bytes[0], bytes[1], bytes[2], bytes[3], bytes[0] as i8, bytes[1] as i8, bytes[2] as i8, bytes[3] as i8
                    ));
                }
            }
            println!("   {label} ({total}):");
            for r in rows {
                println!("{r}");
            }
        }
        // the fit of byte 0 and byte 1 against n.x and n.z over the gently tilted vertices
        let (mut sx, mut sxx, mut sz, mut szz) = (0f64, 0f64, 0f64, 0f64);
        let mut n_used = 0;
        for v in 0..count {
            let n = norm(gn[first + v]);
            if n[1] < 0.93 {
                continue;
            }
            let o = tb + v * ts;
            let (b0, b1) = (geometry[o] as i8 as f64 / 127.0, geometry[o + 1] as i8 as f64 / 127.0);
            sx += b0 * n[0] as f64;
            sxx += (n[0] as f64).powi(2);
            sz += b1 * n[2] as f64;
            szz += (n[2] as f64).powi(2);
            n_used += 1;
        }
        println!("   fit over {n_used} up-ish vertices: byte0/127 = {:.3} * n.x, byte1/127 = {:.3} * n.z", sx / sxx, sz / szz);
    }
}
