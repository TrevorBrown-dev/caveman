//! The solid rainbow trail's geometry (`nyanvspatch.py`): patches the ribbon's VERTEX program so a nyan ribbon (the
//! ratio trigger of the pixel patch, `ribbon.rs`: particle colour red = RATIO x green) lies flat, its width running
//! horizontally and perpendicular to the trail instead of turning to face the camera, and is pulled BIAS metres toward
//! the camera, so it wins against the ground it would clip into but not against the board above it. Every other
//! ribbon is unchanged. See knowledge/trails-geometry.md.
//!
//! The program: the ribbon material's permutation (`expressionshader/permutation14243237207213057914`) names two
//! programs, +0x20 this vertex program and +0x30 the pixel program (`ribbon::RIBBON_PROGRAM`).

use super::ir::{f32_lit as f32, set_valver_17, valver};
use super::ribbon::{RATIO, RATIO_TOL};
use super::{read_bytecode_resource, wrap, Dxc, ShaderError};

/// `nyanvspatch.RIBBON_VS_PROGRAM`: the ribbon's vertex program, in the global configuration bundle.
pub const RIBBON_VS_PROGRAM: &str = "6cfc6613-53b8-5466-0a0f-1c718a455601";
/// `nyanvspatch.BIAS`: metres toward the camera (the deck's underside is about 3 cm above the trail).
pub const BIAS: f64 = 0.04;

/// `%\d+`: a numbered value at the start of `s`; returns it and the rest.
fn numbered(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    if b.first() != Some(&b'%') {
        return None;
    }
    let n = b[1..].iter().take_while(|c| c.is_ascii_digit()).count();
    if n == 0 {
        return None;
    }
    Some((&s[..1 + n], &s[1 + n..]))
}

/// `^  (%\d+) = <rest>`: a line defining a numbered value; returns (name, rest of the line).
fn def(line: &str) -> Option<(&str, &str)> {
    let (name, rest) = numbered(line.strip_prefix("  ")?)?;
    Some((name, rest.strip_prefix(" = ")?))
}

/// The lines of `text` with their byte offsets (a line excludes its '\n').
fn lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split('\n') {
        out.push((at, line));
        at += line.len() + 1;
    }
    out
}

/// Exactly one item, or an error naming `what` (`nyanvspatch._one`).
fn one<T>(found: Vec<T>, what: &str) -> Result<T, String> {
    let n = found.len();
    let mut it = found.into_iter();
    match (n, it.next()) {
        (1, Some(x)) => Ok(x),
        _ => Err(format!("{what}: expected one match, found {n}")),
    }
}

/// `select i1 %C, float %\d+, float %\d+` (the whole rest of a line): returns %C.
fn select_numbered(rest: &str) -> Option<&str> {
    let (cond, r) = numbered(rest.strip_prefix("select i1 ")?)?;
    let (_, r) = numbered(r.strip_prefix(", float ")?)?;
    let (_, r) = numbered(r.strip_prefix(", float ")?)?;
    r.is_empty().then_some(cond)
}

/// `nyanvspatch._gate`: IR lines setting %<prefix>_on, the ratio trigger on the particle colour.
fn gate(prefix: &str, r: &str, g: &str) -> Vec<String> {
    vec![
        format!("  %{prefix}_gp = fcmp ogt float {g}, {}", f32(0.0001)),
        format!("  %{prefix}_rg = fmul float {g}, {}", f32(RATIO)),
        format!("  %{prefix}_rd = fsub float {r}, %{prefix}_rg"),
        format!("  %{prefix}_ra = call float @dx.op.unary.f32(i32 6, float %{prefix}_rd)"),
        format!("  %{prefix}_tl = fmul float {g}, {}", f32(RATIO_TOL)),
        format!("  %{prefix}_ok = fcmp olt float %{prefix}_ra, %{prefix}_tl"),
        format!("  %{prefix}_on = and i1 %{prefix}_gp, %{prefix}_ok"),
    ]
}

/// `nyanvspatch._rename_after`: each name of `mapping` replaced after byte `at`, whole names only (`re.escape(old)`
/// followed by `\b`).
fn rename_after(text: &str, at: usize, mapping: &[(&str, &str)]) -> String {
    let mut tail = text[at..].to_string();
    for (old, new) in mapping {
        let mut out = String::with_capacity(tail.len());
        let mut last = 0;
        for (i, _) in tail.match_indices(old) {
            if i < last {
                continue;
            }
            let after = tail.as_bytes().get(i + old.len());
            if after.is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                continue;
            }
            out.push_str(&tail[last..i]);
            out.push_str(new);
            last = i + old.len();
        }
        out.push_str(&tail[last..]);
        tail = out;
    }
    format!("{}{}", &text[..at], tail)
}

/// `nyanvspatch.patch`: the ribbon vertex program's disassembly with the nyan geometry.
pub fn patch(ir: &str) -> Result<String, String> {
    // TEXCOORD1: the particle colour (the last store of each channel, as dict(re.findall(...)))
    let mut colour: [Option<String>; 2] = [None, None];
    const STORE: &str = "storeOutput.f32(i32 5, i32 1, i32 0, i8 ";
    for (i, _) in ir.match_indices(STORE) {
        let s = &ir[i + STORE.len()..];
        let ch = s.as_bytes().first().copied().unwrap_or(0);
        if !ch.is_ascii_digit() {
            continue;
        }
        if let Some(rest) = s[1..].strip_prefix(", float ") {
            if let Some((name, r)) = numbered(rest) {
                if r.starts_with(')') && (ch == b'0' || ch == b'1') {
                    colour[(ch - b'0') as usize] = Some(name.to_string());
                }
            }
        }
    }
    let r = colour[0].clone().ok_or("no TEXCOORD1.x store")?;
    let g = colour[1].clone().ok_or("no TEXCOORD1.y store")?;
    let ls = lines(ir);
    let corner_y = one(
        ls.iter()
            .filter_map(|(_, l)| def(l))
            .filter(|(_, rest)| rest.starts_with("call float @dx.op.loadInput.f32(i32 4, i32 1, i32 0, i8 1, i32 undef)"))
            .map(|(n, _)| n.to_string())
            .collect(),
        "POSITION.y",
    )?;
    // the width direction: three selects on "far end of the segment", then a fourth (the width), then width x dir.x
    let mut width = Vec::new();
    for k in 0..ls.len().saturating_sub(4) {
        let d: Vec<Option<(&str, &str)>> = (0..5).map(|j| def(ls[k + j].1)).collect();
        if d.iter().any(|x| x.is_none()) {
            continue;
        }
        let d: Vec<(&str, &str)> = d.into_iter().map(|x| x.unwrap()).collect();
        let Some(cond) = select_numbered(d[0].1) else { continue };
        if (1..4).any(|j| select_numbered(d[j].1) != Some(cond)) {
            continue;
        }
        if d[4].1 != format!("fmul float {}, {}", d[3].0, d[0].0) {
            continue;
        }
        width.push((k, d[0].0.to_string(), cond.to_string(), d[1].0.to_string(), d[2].0.to_string()));
    }
    let (wk, dx, far, dy, dz) = one(width, "width direction")?;
    // the segment (next point - this point): what (POSITION.y + 1) / 2 moves the vertex along
    let mut segment = Vec::new();
    for k in 0..ls.len().saturating_sub(4) {
        let Some((s1, r1)) = def(ls[k].1) else { continue };
        if r1 != format!("fadd float {corner_y}, 1.000000e+00") {
            continue;
        }
        let Some((s2, r2)) = def(ls[k + 1].1) else { continue };
        if r2 != format!("fmul float {s1}, 5.000000e-01") {
            continue;
        }
        let mut seg = Vec::new();
        for j in 2..5 {
            let Some((_, r3)) = def(ls[k + j].1) else { break };
            let Some(v) = r3.strip_prefix(&format!("fmul float {s2}, ")) else { break };
            match numbered(v) {
                Some((name, "")) => seg.push(name.to_string()),
                _ => break,
            }
        }
        if seg.len() == 3 {
            segment.push(seg);
        }
    }
    let seg = one(segment, "segment")?;
    // the tangents the stock code uses: this point's (segment + previous) and the next point's (segment + next)
    let tangents = |s: &str| -> Vec<String> {
        ls.iter()
            .filter_map(|(_, l)| def(l))
            .filter(|(_, rest)| {
                rest.strip_prefix(&format!("fadd float {s}, ")).is_some_and(|v| matches!(numbered(v), Some((_, ""))))
            })
            .map(|(n, _)| n.to_string())
            .collect()
    };
    let (tx, tz) = (tangents(&seg[0]), tangents(&seg[2]));
    if tx.len() != 2 || tz.len() != 2 {
        return Err(format!("tangents: expected two each, found {tx:?} {tz:?}"));
    }
    let mut flat = gate("nyw", &r, &g);
    flat.extend([
        format!("  %nyw_tx = select i1 {far}, float {}, float {}", tx[1], tx[0]),
        format!("  %nyw_tz = select i1 {far}, float {}, float {}", tz[1], tz[0]),
        "  %nyw_xx = fmul float %nyw_tx, %nyw_tx".to_string(),
        "  %nyw_zz = fmul float %nyw_tz, %nyw_tz".to_string(),
        "  %nyw_l2 = fadd float %nyw_xx, %nyw_zz".to_string(),
        format!("  %nyw_l2e = fadd float %nyw_l2, {}", f32(1e-12)),
        "  %nyw_il = call float @dx.op.unary.f32(i32 25, float %nyw_l2e)".to_string(),
        format!("  %nyw_mz = fsub float {}, %nyw_tz", f32(0.0)),
        "  %nyw_fx = fmul float %nyw_mz, %nyw_il".to_string(),
        "  %nyw_fz = fmul float %nyw_tx, %nyw_il".to_string(),
        format!("  %nyw_dx = select i1 %nyw_on, float %nyw_fx, float {dx}"),
        format!("  %nyw_dy = select i1 %nyw_on, float {}, float {dy}", f32(0.0)),
        format!("  %nyw_dz = select i1 %nyw_on, float %nyw_fz, float {dz}"),
    ]);
    let at = ls[wk + 3].0; // before the width select (after the three direction selects)
    let joined = flat.join("\n");
    let out = format!("{}{}\n{}", &ir[..at], joined, &ir[at..]);
    let out = rename_after(&out, at + joined.len() + 1, &[(&dx, "%nyw_dx"), (&dy, "%nyw_dy"), (&dz, "%nyw_dz")]);
    // the depth bias: the vertex relative to the camera (position - camera, register 455), right before clip space
    let ls = lines(&out);
    const LOAD: &str = "call %dx.types.CBufRet.f32 @dx.op.cbufferLoadLegacy.f32(i32 59, %dx.types.Handle ";
    let load_of = |reg: &str| -> Vec<(usize, String)> {
        ls.iter()
            .filter_map(|(o, l)| def(l).map(|(n, rest)| (*o, n, rest)))
            .filter(|(_, _, rest)| {
                rest.strip_prefix(LOAD)
                    .and_then(numbered)
                    .is_some_and(|(_, after)| after.starts_with(&format!(", i32 {reg})")))
            })
            .map(|(o, n, _)| (o, n.to_string()))
            .collect()
    };
    let cam_load = one(load_of("455"), "camera")?.1;
    let mut cam = Vec::new();
    for c in 0..3 {
        let want = format!("extractvalue %dx.types.CBufRet.f32 {cam_load}, {c}");
        cam.push(one(
            ls.iter().filter_map(|(_, l)| def(l)).filter(|(_, rest)| *rest == want).map(|(n, _)| n.to_string()).collect(),
            &format!("camera {c}"),
        )?);
    }
    let clip = one(load_of("421"), "view-projection")?.0;
    let mut rel = Vec::new();
    let tail: Vec<&(usize, &str)> = ls.iter().filter(|(o, _)| *o >= clip).collect();
    for k in 0..tail.len().saturating_sub(2) {
        let mut names = Vec::new();
        for (j, cam_c) in cam.iter().enumerate() {
            let Some((name, rest)) = def(tail[k + j].1) else { break };
            let Some(v) = rest.strip_prefix("fsub float ") else { break };
            let Some((_, v)) = numbered(v) else { break };
            if v != format!(", {cam_c}") {
                break;
            }
            names.push(name.to_string());
        }
        if names.len() == 3 {
            rel.push((tail[k + 2].0 + tail[k + 2].1.len() + 1, names));
        }
    }
    let (at, rel) = one(rel, "camera-relative position")?;
    let (rx, ry, rz) = (&rel[0], &rel[1], &rel[2]);
    let mut bias = gate("nyb", &r, &g);
    bias.extend([
        format!("  %nyb_d2 = call float @dx.op.dot3.f32(i32 55, float {rx}, float {ry}, float {rz}, float {rx}, float {ry}, float {rz})"),
        format!("  %nyb_d2e = fadd float %nyb_d2, {}", f32(1e-12)),
        "  %nyb_il = call float @dx.op.unary.f32(i32 25, float %nyb_d2e)".to_string(),
        format!("  %nyb_k = fmul float %nyb_il, {}", f32(BIAS)),
        format!("  %nyb_s0 = fsub float {}, %nyb_k", f32(1.0)),
        format!("  %nyb_s1 = call float @dx.op.binary.f32(i32 35, float %nyb_s0, float {})", f32(0.0)),
        format!("  %nyb_s = select i1 %nyb_on, float %nyb_s1, float {}", f32(1.0)),
        format!("  %nyb_x = fmul float {rx}, %nyb_s"),
        format!("  %nyb_y = fmul float {ry}, %nyb_s"),
        format!("  %nyb_z = fmul float {rz}, %nyb_s"),
    ]);
    let joined = bias.join("\n");
    let out = format!("{}{}\n{}", &out[..at], joined, &out[at..]);
    let out = rename_after(&out, at + joined.len() + 1, &[(rx, "%nyb_x"), (ry, "%nyb_y"), (rz, "%nyb_z")]);
    let node = valver(&out).ok_or("no !dx.valver")?.to_string();
    Ok(set_valver_17(&out, &node))
}

/// `nyanvspatch.build` minus the game access: the ribbon vertex program `original` (its `shaders/bytecode` payload)
/// disassembled, patched, assembled, validated and signed, and wrapped again. Returns (payload, meta).
pub fn build(dxc: &Dxc, original: &[u8]) -> Result<(Vec<u8>, [u8; 16]), ShaderError> {
    let res = read_bytecode_resource(original)
        .ok_or_else(|| ShaderError::Patch(format!("shaders/bytecode/{RIBBON_VS_PROGRAM}: not a bytecode resource")))?;
    let ir = dxc.disassemble(res.dxbc)?;
    let patched = patch(&ir).map_err(|e| ShaderError::Patch(format!("shaders/bytecode/{RIBBON_VS_PROGRAM}: {e}")))?;
    let out = dxc.assemble(&patched, true)?;
    if !out.ok {
        return Err(ShaderError::Dxc(format!("shaders/bytecode/{RIBBON_VS_PROGRAM}: {}", out.messages)));
    }
    wrap(original, &out.bytes).map_err(ShaderError::Patch)
}
