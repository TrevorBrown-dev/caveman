//! The banner trail (`nyanbannerpatch.py`): a nyan ribbon (the ratio trigger of `ribbon.rs`) whose width follows the
//! board's sideways axis at the moment each segment was laid, so the rainbow twists with flips and rolls like a
//! trailing banner. Three patched programs, each gated so every other ribbon is unchanged (knowledge/banner-trail.md):
//!
//! - spawn (`SPAWN_PROGRAM`, compute, in `cas_main_sharedbundle`): a nyan emitter writes the board's right axis (column
//!   0 of the emitter transform) into the particle's +36 float3, which otherwise holds the initial velocity;
//! - sim (`SIM_PROGRAM`, compute, same bundle): a nyan emitter's particles keep +36 (the axis) and their position (no
//!   drift: the stored axis is not a velocity);
//! - vertex (`ribbon_vs::RIBBON_VS_PROGRAM`, applied on top of `ribbon_vs::patch`'s flat patch): a nyan ribbon's width
//!   direction is the stored axis of this or the next particle, made perpendicular to the trail; the flat direction
//!   when no usable axis is stored.
//!
//! The compute programs recognise a nyan emitter by its colour parameter (`EmitterProperties` byte 532: red, green;
//! the ribbon graph's PropertyId -1752990270 at DataOffset 25); the vertex program keeps `ribbon_vs`'s particle-colour
//! test.

use std::collections::{BTreeMap, HashMap};

use super::ir::{f32_lit as f32, set_valver_17, valver};
use super::ribbon::{RATIO, RATIO_TOL};
use super::{read_bytecode_resource, ribbon_vs, wrap, Dxc, ShaderError, CONFIG_BUNDLE, GLOBALS_TOC};
use crate::game::{Game, ITEMS_TOC};

/// `nyanbannerpatch.SHARED_BUNDLE`: the customization shared bundle (in items.toc) holding the ribbon's compute programs.
pub const SHARED_BUNDLE: &str = "win32/characters/customization/configs/cas_main_sharedbundle";
/// `nyanbannerpatch.SPAWN_PROGRAM`: the ribbon graph's spawn compute program.
pub const SPAWN_PROGRAM: &str = "4a6fd8f8-3248-b91e-c135-56f2d0d7c93d";
/// `nyanbannerpatch.SIM_PROGRAM`: the ribbon graph's simulate compute program.
pub const SIM_PROGRAM: &str = "ebf5a811-9430-2052-f916-e1cbbbd3ba76";
/// `nyanbannerpatch.COLOUR_OFFSET`: EmitterProperties byte offset of the ribbon colour (red, green, blue).
pub const COLOUR_OFFSET: u32 = 532;
/// `nyanbannerpatch.AXIS_OFFSET`: the particle float3 the banner axis rides in (the stock velocity).
pub const AXIS_OFFSET: u32 = 36;
/// `nyanbannerpatch.MIN_AXIS`: squared length below which the vertex program falls back to the flat direction.
pub const MIN_AXIS: f64 = 0.25;

/// `nyanbannerpatch.DECLARATIONS`: operations a small program may not declare yet (all readnone).
const DECLARATIONS: [(&str, &str); 2] = [
    ("@dx.op.unary.f32", "declare float @dx.op.unary.f32(i32, float)"),
    ("@dx.op.dot3.f32", "declare float @dx.op.dot3.f32(i32, float, float, float, float, float, float)"),
];

/// One patched program and where it goes.
#[derive(Debug, Clone)]
pub struct BannerProgram {
    /// The superbundle TOC holding the bundle (`items.toc` for the compute programs, `globals.toc` for the vertex one).
    pub toc: &'static str,
    /// The bundle the program is replaced in.
    pub bundle: &'static str,
    /// `shaders/bytecode/<guid>`.
    pub name: String,
    pub payload: Vec<u8>,
    pub meta: [u8; 16],
}

/// `%\d+` at the start of `s`: (the value, the rest).
fn numbered(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'%') {
        return None;
    }
    let n = b[1..].iter().take_while(|c| c.is_ascii_digit()).count();
    (n > 0).then(|| (&s[..1 + n], &s[1 + n..]))
}

/// `^\s*(%\d+) = <rest>`: a line defining a numbered value.
fn def(line: &str) -> Option<(&str, &str)> {
    let (name, rest) = numbered(line.trim_start())?;
    Some((name, rest.strip_prefix(" = ")?))
}

/// `nyanbannerpatch._one`.
fn one<T>(found: Vec<T>, what: &str) -> Result<T, String> {
    let n = found.len();
    if n != 1 {
        return Err(format!("{what}: {n} matches"));
    }
    Ok(found.into_iter().next().unwrap())
}

/// `nyanbannerpatch._gate`: lines setting %<prefix>on to the nyan ratio test on (red, green).
fn gate(c: &mut Vec<String>, prefix: &str, red: &str, green: &str) {
    c.push(format!("  %{prefix}gp = fcmp ogt float {green}, {}", f32(0.0001)));
    c.push(format!("  %{prefix}rg = fmul float {green}, {}", f32(RATIO)));
    c.push(format!("  %{prefix}rd = fsub float {red}, %{prefix}rg"));
    c.push(format!("  %{prefix}ra = call float @dx.op.unary.f32(i32 6, float %{prefix}rd)"));
    c.push(format!("  %{prefix}tl = fmul float {green}, {}", f32(RATIO_TOL)));
    c.push(format!("  %{prefix}ok = fcmp olt float %{prefix}ra, %{prefix}tl"));
    c.push(format!("  %{prefix}on = and i1 %{prefix}gp, %{prefix}ok"));
}

/// `nyanbannerpatch._declare`: declares the operations used but not declared, with the readnone attribute group.
fn declare(ir: String) -> Result<String, String> {
    let group = ir
        .split('\n')
        .find_map(|l| l.strip_prefix("attributes #").and_then(|r| r.strip_suffix(" = { nounwind readnone }")))
        .filter(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()))
        .ok_or("no readnone attribute group")?
        .to_string();
    let mut ir = ir;
    for (name, decl) in DECLARATIONS {
        if ir.contains(name) && !ir.contains(&format!("declare float {name}(")) {
            ir = ir.replacen("\nattributes #", &format!("\n{decl} #{group}\n\nattributes #"), 1);
        }
    }
    Ok(ir)
}

/// `nyanbannerpatch._set_valver`.
fn set_valver(ir: &str) -> Result<String, String> {
    let node = valver(ir).ok_or("no !dx.valver")?.to_string();
    Ok(set_valver_17(ir, &node))
}

/// The EmitterProperties handle: `createHandle(i32 57, i8 0, i32 0, i32 4, i1 false)` (t4), exactly one.
fn props_handle(ir: &str) -> Result<String, String> {
    let found = ir
        .split('\n')
        .filter_map(def)
        .filter(|(_, r)| r.starts_with("call %dx.types.Handle @dx.op.createHandle(i32 57, i8 0, i32 0, i32 4, i1 false)"))
        .map(|(n, _)| n.to_string())
        .collect();
    one(found, "EmitterProperties handle")
}

/// A store line `^(\s*call void @dx.op.bufferStore.i32(i32 69, %dx.types.Handle %U, i32 %A, i32 undef, <values>,
/// i8 MASK).*)$` with `count` numbered values then `tail` (", i32 undef" for mask 7): (line, uav, addr, values).
fn stores(ir: &str, count: usize, tail: &str, mask: &str) -> Vec<(String, String, String, Vec<String>)> {
    let mut out = Vec::new();
    for line in ir.split('\n') {
        let Some(r) = line.trim_start().strip_prefix("call void @dx.op.bufferStore.i32(i32 69, %dx.types.Handle ") else { continue };
        let Some((uav, r)) = numbered(r) else { continue };
        let Some(r) = r.strip_prefix(", i32 ") else { continue };
        let Some((addr, mut r)) = numbered(r) else { continue };
        let Some(rr) = r.strip_prefix(", i32 undef") else { continue };
        r = rr;
        let mut values = Vec::new();
        let mut ok = true;
        for _ in 0..count {
            match r.strip_prefix(", i32 ").and_then(numbered) {
                Some((v, rest)) => {
                    values.push(v.to_string());
                    r = rest;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && r.starts_with(&format!("{tail}, i8 {mask})")) {
            out.push((line.to_string(), uav.to_string(), addr.to_string(), values));
        }
    }
    out
}

/// `nyanbannerpatch.patch_spawn`: a nyan emitter stores its right axis (normalized) at the particle's +36.
pub fn patch_spawn(ir: &str) -> Result<String, String> {
    let props = props_handle(ir)?;
    let pre = format!("call %dx.types.ResRet.f32 @dx.op.bufferLoad.f32(i32 68, %dx.types.Handle {props}, i32 ");
    let mut rows: BTreeMap<u32, (String, String)> = BTreeMap::new();
    for (name, rest) in ir.split('\n').filter_map(def) {
        let Some(r) = rest.strip_prefix(pre.as_str()) else { continue };
        let Some((index, r)) = numbered(r) else { continue };
        let Some(r) = r.strip_prefix(", i32 ") else { continue };
        for (off, text) in [(0, "0)"), (16, "16)"), (32, "32)")] {
            if r.starts_with(text) {
                rows.insert(off, (name.to_string(), index.to_string()));
                break;
            }
        }
    }
    if rows.keys().copied().collect::<Vec<_>>() != [0, 16, 32] {
        return Err(format!("transform rows {:?}", rows.keys().collect::<Vec<_>>()));
    }
    let emitter = rows[&0].1.clone();
    let (line, _uav, _addr, v) = one(stores(ir, 3, ", i32 undef", "7"), "the +36 store")?;
    let mut c = vec![
        format!("  %bns_c = call %dx.types.ResRet.f32 @dx.op.bufferLoad.f32(i32 68, %dx.types.Handle {props}, i32 {emitter}, i32 {COLOUR_OFFSET})"),
        "  %bns_cr = extractvalue %dx.types.ResRet.f32 %bns_c, 0".to_string(),
        "  %bns_cg = extractvalue %dx.types.ResRet.f32 %bns_c, 1".to_string(),
    ];
    gate(&mut c, "bns_", "%bns_cr", "%bns_cg");
    for (k, off) in [0u32, 16, 32].iter().enumerate() {
        c.push(format!("  %bns_a{k} = extractvalue %dx.types.ResRet.f32 {}, 0", rows[off].0));
    }
    c.push("  %bns_l2 = call float @dx.op.dot3.f32(i32 55, float %bns_a0, float %bns_a1, float %bns_a2, float %bns_a0, float %bns_a1, float %bns_a2)".into());
    c.push(format!("  %bns_l2e = fadd float %bns_l2, {}", f32(1e-12)));
    c.push("  %bns_il = call float @dx.op.unary.f32(i32 25, float %bns_l2e)".into());
    for (k, val) in v.iter().enumerate() {
        c.push(format!("  %bns_n{k} = fmul float %bns_a{k}, %bns_il"));
        c.push(format!("  %bns_i{k} = bitcast float %bns_n{k} to i32"));
        c.push(format!("  %bns_s{k} = select i1 %bns_on, i32 %bns_i{k}, i32 {val}"));
    }
    let store = line.replace(
        &format!("i32 {}, i32 {}, i32 {}, i32 undef, i8 7", v[0], v[1], v[2]),
        "i32 %bns_s0, i32 %bns_s1, i32 %bns_s2, i32 undef, i8 7",
    );
    let out = ir.replacen(&line, &format!("{}\n{store}", c.join("\n")), 1);
    set_valver(&declare(out)?)
}

/// The three components of the i32 load at `addr` from `uav` (`nyanbannerpatch.patch_sim.loaded`).
fn loaded(ir: &str, uav: &str, addr: &str) -> Result<[String; 3], String> {
    let want = format!("call %dx.types.ResRet.i32 @dx.op.bufferLoad.i32(i32 68, %dx.types.Handle {uav}, i32 {addr}, i32 undef)");
    let res = one(
        ir.split('\n').filter_map(def).filter(|(_, r)| r.starts_with(want.as_str())).map(|(n, _)| n.to_string()).collect(),
        &format!("the load at {addr}"),
    )?;
    let pre = format!("extractvalue %dx.types.ResRet.i32 {res}, ");
    let mut comps: HashMap<u32, String> = HashMap::new();
    for (name, rest) in ir.split('\n').filter_map(def) {
        if let Some(d) = rest.strip_prefix(pre.as_str()).and_then(|r| r.bytes().next()).filter(u8::is_ascii_digit) {
            comps.insert((d - b'0') as u32, name.to_string());
        }
    }
    let get = |k: u32| comps.get(&k).cloned().ok_or_else(|| format!("component {k} of {res}"));
    Ok([get(0)?, get(1)?, get(2)?])
}

/// `nyanbannerpatch.patch_sim`: a nyan emitter's particles keep their +36 (the axis) and their position.
pub fn patch_sim(ir: &str) -> Result<String, String> {
    let props = props_handle(ir)?;
    let pre = format!("call %dx.types.ResRet.f32 @dx.op.bufferLoad.f32(i32 68, %dx.types.Handle {props}, i32 ");
    let emitter = one(
        ir.split('\n')
            .filter_map(def)
            .filter_map(|(_, r)| {
                let (index, r) = numbered(r.strip_prefix(pre.as_str())?)?;
                (r.strip_prefix(", i32 0)").is_some()).then(|| index.to_string())
            })
            .collect(),
        "emitter index",
    )?;
    let (pline, uav, paddr, p) = one(stores(ir, 4, "", "15"), "the position store")?;
    let (vline, _, vaddr, v) = one(stores(ir, 3, ", i32 undef", "7"), "the +36 store")?;
    let old_p = loaded(ir, &uav, &paddr)?;
    let old_v = loaded(ir, &uav, &vaddr)?;
    let mut c = vec![
        format!("  %bnm_c = call %dx.types.ResRet.f32 @dx.op.bufferLoad.f32(i32 68, %dx.types.Handle {props}, i32 {emitter}, i32 {COLOUR_OFFSET})"),
        "  %bnm_cr = extractvalue %dx.types.ResRet.f32 %bnm_c, 0".to_string(),
        "  %bnm_cg = extractvalue %dx.types.ResRet.f32 %bnm_c, 1".to_string(),
    ];
    gate(&mut c, "bnm_", "%bnm_cr", "%bnm_cg");
    for k in 0..3 {
        c.push(format!("  %bnm_p{k} = select i1 %bnm_on, i32 {}, i32 {}", old_p[k], p[k]));
    }
    let pstore = pline.replace(
        &format!("i32 {}, i32 {}, i32 {}, i32 {}, i8 15", p[0], p[1], p[2], p[3]),
        &format!("i32 %bnm_p0, i32 %bnm_p1, i32 %bnm_p2, i32 {}, i8 15", p[3]),
    );
    let out = ir.replacen(&pline, &format!("{}\n{pstore}", c.join("\n")), 1);
    let keep: Vec<String> = (0..3).map(|k| format!("  %bnm_v{k} = select i1 %bnm_on, i32 {}, i32 {}", old_v[k], v[k])).collect();
    let vstore = vline.replace(
        &format!("i32 {}, i32 {}, i32 {}, i32 undef, i8 7", v[0], v[1], v[2]),
        "i32 %bnm_v0, i32 %bnm_v1, i32 %bnm_v2, i32 undef, i8 7",
    );
    let out = out.replacen(&vline, &format!("{}\n{vstore}", keep.join("\n")), 1);
    set_valver(&declare(out)?)
}

/// The next line that is not blank, after index `i` (Python's `\n\s*` spans blank lines).
fn next_line<'a>(lines: &[&'a str], i: usize) -> Option<&'a str> {
    lines[i + 1..].iter().copied().find(|l| !l.trim().is_empty())
}

/// `select i1 %C, float %A, float %B` exactly (the rest of a line): (C, A, B).
fn select3(rest: &str) -> Option<(&str, &str, &str)> {
    let (c, r) = numbered(rest.strip_prefix("select i1 ")?)?;
    let (a, r) = numbered(r.strip_prefix(", float ")?)?;
    let (b, r) = numbered(r.strip_prefix(", float ")?)?;
    r.is_empty().then_some((c, a, b))
}

/// `nyanbannerpatch.patch_vs`: on the flat-patched vertex program (`ribbon_vs::patch`), a nyan ribbon's width
/// direction is the stored axis of this or the next particle, minus its component along the trail.
pub fn patch_vs(flat_ir: &str) -> Result<String, String> {
    let lines: Vec<&str> = flat_ir.split('\n').collect();
    // the particle data base: `%B = extractvalue %dx.types.ResRet.i32 %N, 0` followed by the stride load at 420
    let mut bases = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some((name, rest)) = def(line) else { continue };
        let Some(r) = rest.strip_prefix("extractvalue %dx.types.ResRet.i32 ") else { continue };
        let Some((_, r)) = numbered(r) else { continue };
        if r != ", 0" {
            continue;
        }
        let Some((_, nr)) = next_line(&lines, i).and_then(def) else { continue };
        let Some(r) = nr.strip_prefix("call %dx.types.ResRet.i32 @dx.op.bufferLoad.i32(i32 68, %dx.types.Handle ") else { continue };
        let Some((_, r)) = numbered(r) else { continue };
        let Some((_, r)) = r.strip_prefix(", i32 ").and_then(numbered) else { continue };
        if r.starts_with(", i32 420)") {
            bases.push(name.to_string());
        }
    }
    let base = one(bases, "particle data base")?;
    let handle = lines
        .iter()
        .filter_map(|l| def(l))
        .find(|(_, r)| r.starts_with("call %dx.types.Handle @dx.op.createHandle(i32 57, i8 0, i32 0, i32 0, i1 false)"))
        .map(|(n, _)| n.to_string())
        .ok_or("particle buffer handle")?;
    // particle positions (+0): `%A = add i32 %S, BASE` then the load at %A, for this, previous, next and next-next
    let mut addrs = Vec::new();
    let load = |a: &str| format!("call %dx.types.ResRet.i32 @dx.op.bufferLoad.i32(i32 68, %dx.types.Handle {handle}, i32 {a}, i32 undef)");
    for (i, line) in lines.iter().enumerate() {
        let Some((name, rest)) = def(line) else { continue };
        let Some(r) = rest.strip_prefix("add i32 ") else { continue };
        let Some((_, r)) = numbered(r) else { continue };
        if r != format!(", {base}") {
            continue;
        }
        if next_line(&lines, i).and_then(def).is_some_and(|(_, nr)| nr.starts_with(load(name).as_str())) {
            addrs.push(name.to_string());
        }
    }
    if addrs.len() != 4 {
        return Err(format!("particle position loads: {addrs:?}"));
    }
    let (this_addr, next_addr) = (addrs[0].clone(), addrs[2].clone());
    let sel = |var: &str| -> Vec<(String, String, String)> {
        lines
            .iter()
            .filter_map(|l| l.trim_start().strip_prefix(var))
            .filter_map(|r| r.strip_prefix(" = ").and_then(select3))
            .map(|(c, a, b)| (c.to_string(), a.to_string(), b.to_string()))
            .collect()
    };
    let txs = sel("%nyw_tx");
    let (far, tx0, tx1) = one(txs, "the far-end switch")?;
    let (_, tz0, tz1) = sel("%nyw_tz").into_iter().next().ok_or("%nyw_tz")?;
    let middle = |x: &str, z: &str| -> Result<String, String> {
        let nx: u64 = x[1..].parse().map_err(|_| format!("tangent {x}"))?;
        let nz: u64 = z[1..].parse().map_err(|_| format!("tangent {z}"))?;
        if nz != nx + 2 {
            return Err(format!("tangent components {x}, {z}"));
        }
        Ok(format!("%{}", nx + 1))
    };
    let (ty0, ty1) = (middle(&tx0, &tz0)?, middle(&tx1, &tz1)?);
    let anchor = one(
        lines
            .iter()
            .filter(|l| {
                l.trim_start()
                    .strip_prefix("%nyw_dz = select i1 %nyw_on, float %nyw_fz, float ")
                    .and_then(numbered)
                    .is_some_and(|(_, r)| r.is_empty())
            })
            .map(|l| l.to_string())
            .collect(),
        "the flat direction",
    )?;
    let mut c = Vec::new();
    for (tag, addr) in [("t", &this_addr), ("n", &next_addr)] {
        c.push(format!("  %bnv_{tag}a = add i32 {addr}, {AXIS_OFFSET}"));
        c.push(format!("  %bnv_{tag}l = call %dx.types.ResRet.i32 @dx.op.bufferLoad.i32(i32 68, %dx.types.Handle {handle}, i32 %bnv_{tag}a, i32 undef)"));
        for k in 0..3 {
            c.push(format!("  %bnv_{tag}i{k} = extractvalue %dx.types.ResRet.i32 %bnv_{tag}l, {k}"));
            c.push(format!("  %bnv_{tag}f{k} = bitcast i32 %bnv_{tag}i{k} to float"));
        }
    }
    for k in 0..3 {
        c.push(format!("  %bnv_a{k} = select i1 {far}, float %bnv_nf{k}, float %bnv_tf{k}"));
    }
    c.push(format!("  %bnv_t0 = select i1 {far}, float {tx0}, float {tx1}"));
    c.push(format!("  %bnv_t1 = select i1 {far}, float {ty0}, float {ty1}"));
    c.push(format!("  %bnv_t2 = select i1 {far}, float {tz0}, float {tz1}"));
    c.push("  %bnv_at = call float @dx.op.dot3.f32(i32 55, float %bnv_a0, float %bnv_a1, float %bnv_a2, float %bnv_t0, float %bnv_t1, float %bnv_t2)".into());
    c.push("  %bnv_tt = call float @dx.op.dot3.f32(i32 55, float %bnv_t0, float %bnv_t1, float %bnv_t2, float %bnv_t0, float %bnv_t1, float %bnv_t2)".into());
    c.push(format!("  %bnv_tte = fadd float %bnv_tt, {}", f32(1e-12)));
    c.push("  %bnv_k = fdiv float %bnv_at, %bnv_tte".into());
    for k in 0..3 {
        c.push(format!("  %bnv_kt{k} = fmul float %bnv_t{k}, %bnv_k"));
        c.push(format!("  %bnv_w{k} = fsub float %bnv_a{k}, %bnv_kt{k}"));
    }
    c.push("  %bnv_ww = call float @dx.op.dot3.f32(i32 55, float %bnv_w0, float %bnv_w1, float %bnv_w2, float %bnv_w0, float %bnv_w1, float %bnv_w2)".into());
    c.push(format!("  %bnv_use0 = fcmp ogt float %bnv_ww, {}", f32(MIN_AXIS)));
    c.push("  %bnv_use = and i1 %bnv_use0, %nyw_on".into());
    c.push(format!("  %bnv_wwe = fadd float %bnv_ww, {}", f32(1e-12)));
    c.push("  %bnv_il = call float @dx.op.unary.f32(i32 25, float %bnv_wwe)".into());
    for (k, axis) in ['x', 'y', 'z'].iter().enumerate() {
        c.push(format!("  %bnv_u{k} = fmul float %bnv_w{k}, %bnv_il"));
        c.push(format!("  %bnv_d{axis} = select i1 %bnv_use, float %bnv_u{k}, float %nyw_d{axis}"));
    }
    let mut out = flat_ir.replacen(&anchor, &format!("{anchor}\n{}", c.join("\n")), 1);
    // the width multiplication by the flat direction now takes the banner direction (the first such line per axis)
    for axis in ['x', 'y', 'z'] {
        let tail = format!(", %nyw_d{axis}");
        let mut done = false;
        let mut text = Vec::new();
        for line in out.split('\n') {
            if !done {
                if let Some(pos) = line.find("fmul float %") {
                    let after = &line[pos + "fmul float ".len()..];
                    if let Some((v, r)) = numbered(after) {
                        if r == tail {
                            text.push(format!("{}fmul float {v}, %bnv_d{axis}", &line[..pos]));
                            done = true;
                            continue;
                        }
                    }
                }
            }
            text.push(line.to_string());
        }
        if !done {
            return Err(format!("the width multiplication by %nyw_d{axis}"));
        }
        out = text.join("\n");
    }
    declare(out)
}

/// Disassemble, patch, assemble, validate, sign and wrap one program.
fn build_one(dxc: &Dxc, original: &[u8], name: &str, patcher: impl Fn(&str) -> Result<String, String>) -> Result<(Vec<u8>, [u8; 16]), ShaderError> {
    let res = read_bytecode_resource(original).ok_or_else(|| ShaderError::Patch(format!("{name}: not a bytecode resource")))?;
    let ir = dxc.disassemble(res.dxbc)?;
    let patched = patcher(&ir).map_err(|e| ShaderError::Patch(format!("{name}: {e}")))?;
    let out = dxc.assemble(&patched, true)?;
    if !out.ok {
        return Err(ShaderError::Dxc(format!("{name}: {}", out.messages)));
    }
    wrap(original, &out.bytes).map_err(ShaderError::Patch)
}

/// `nyanbannerpatch.build`: the three patched programs from the player's own game (spawn and sim from the shared
/// bundle in items.toc, the vertex program, flat patch plus banner patch, from the configuration bundle in globals.toc).
pub fn build(game: &Game, dxc: &Dxc) -> Result<Vec<BannerProgram>, ShaderError> {
    let mut out = build_shared(game, dxc)?;
    let config = game.bundle(CONFIG_BUNDLE, GLOBALS_TOC)?;
    let name = format!("shaders/bytecode/{}", ribbon_vs::RIBBON_VS_PROGRAM);
    let i = config.manifest.res.iter().position(|r| r.name == name).ok_or_else(|| ShaderError::Patch(format!("no {name}")))?;
    let loc = config.res_loc(i).ok_or_else(|| ShaderError::Patch(format!("{name}: no file")))?;
    let original = game.decode(&loc)?;
    let (payload, meta) = build_one(dxc, &original, &name, |ir| patch_vs(&ribbon_vs::patch(ir)?))?;
    out.push(BannerProgram { toc: GLOBALS_TOC, bundle: CONFIG_BUNDLE, name, payload, meta });
    Ok(out)
}

/// The spawn and simulate programs alone, from the shared bundle (the shader mod builds the vertex program with the
/// configuration bundle's other programs).
pub fn build_shared(game: &Game, dxc: &Dxc) -> Result<Vec<BannerProgram>, ShaderError> {
    let mut out = Vec::new();
    let shared = game.bundle(SHARED_BUNDLE, ITEMS_TOC)?;
    type Patcher = fn(&str) -> Result<String, String>;
    for (gid, patcher) in [(SPAWN_PROGRAM, patch_spawn as Patcher), (SIM_PROGRAM, patch_sim as Patcher)] {
        let name = format!("shaders/bytecode/{gid}");
        let i = shared.manifest.res.iter().position(|r| r.name == name).ok_or_else(|| ShaderError::Patch(format!("no {name}")))?;
        let loc = shared.res_loc(i).ok_or_else(|| ShaderError::Patch(format!("{name}: no file")))?;
        let original = game.decode(&loc)?;
        let (payload, meta) = build_one(dxc, &original, &name, patcher)?;
        out.push(BannerProgram { toc: ITEMS_TOC, bundle: SHARED_BUNDLE, name, payload, meta });
    }
    Ok(out)
}
