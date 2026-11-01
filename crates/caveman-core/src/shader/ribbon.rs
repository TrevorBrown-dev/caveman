//! Caveman's solid rainbow trail (`nyanpatch.py`): the game's ribbon pixel program patched so a ribbon whose colour is
//! a trigger draws six hard-edged bands across its width, stepped along its length, opaque, with no noise. Two
//! triggers: red below `SENTINEL` (shows no trail in game: negative colours break the ribbon first) and red exactly
//! `RATIO` x green (the one that works). Every other ribbon is unchanged.

use super::deck::replace_once;
use super::ir::{self, f32_lit as f32};

/// `nyanpatch.RIBBON_PROGRAM`: the ribbon material's program (global db key 2720338288095328273, record 376).
pub const RIBBON_PROGRAM: &str = "f9af19ba-1e20-e282-f9a7-938f31781dc2";
/// `nyanpatch.PALETTE`: Nyan Cat's six bands, top to bottom (sRGB).
pub const PALETTE: [(u8, u8, u8); 6] = [(255, 0, 0), (255, 153, 0), (255, 255, 0), (51, 255, 0), (0, 153, 255), (102, 51, 255)];
/// `nyanpatch.SENTINEL`: ribbon colour red below this switches nyan mode on (the negative trigger).
pub const SENTINEL: f64 = -0.5;
/// `nyanpatch.RATIO`, `RATIO_TOL`, `RATIO_STEPS`: the ratio trigger, colour (RATIO, 1, S / RATIO_STEPS).
pub const RATIO: f64 = 0.0371;
pub const RATIO_TOL: f64 = 0.002;
pub const RATIO_STEPS: f64 = 64.0;

/// `nyanpatch._lin`: an sRGB channel (0-255) as linear light.
pub fn lin(c: u8) -> f64 {
    let c = f64::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The ribbon program's disassembly with the nyan branch (`nyanpatch.patch`).
pub fn patch(ir_text: &str) -> Result<String, String> {
    let stores = ir::target0_stores(ir_text);
    let mut cols: Vec<u32> = stores.iter().map(|s| s.0).collect();
    cols.sort_unstable();
    if cols != [0, 1, 2, 3] {
        return Err("expected one float4 SV_Target".into());
    }
    let store = |ch: u32| stores.iter().find(|s| s.0 == ch).map(|s| s.1.as_str()).unwrap_or_default();
    let inputs = ir::inputs(ir_text);
    let input = |row: u32, col: u32| inputs.iter().find(|(k, _)| *k == (row, col)).map(|(_, v)| v.as_str()).ok_or_else(|| format!("no input {row}.{col}"));
    let (u, v) = (input(0, 0)?, input(0, 1)?); // TEXCOORD0.x across the width, .y along
    let (r, g, b, a) = (input(1, 0)?, input(1, 1)?, input(1, 2)?, input(1, 3)?); // TEXCOORD1: colour and alpha
                                                                                 // the stock exposure-like scalars the colour is multiplied by: cb4[459].w, cb4[479].w, cb1[4].w
    let scal = ir::cbuf_w(ir_text);
    let loads = ir::cbuf_loads(ir_text);
    let scalar = |reg: u32| -> Result<&str, String> {
        let reg = reg.to_string();
        scal.iter()
            .find(|(_, load)| loads.iter().any(|(name, r)| name == load && *r == reg))
            .map(|(s, _)| s.as_str())
            .ok_or_else(|| format!("no cbuffer register {reg} .w"))
    };
    let k = [scalar(459)?, scalar(479)?, scalar(4)?];
    let first = stores.iter().map(|s| s.2).min().ok_or("no stores")?;
    let at = ir_text[..first].rfind('\n').map_or(0, |i| i + 1);
    let mut c: Vec<String> = Vec::new();
    c.push(format!("  %ny_neg = fcmp olt float {r}, {}", f32(SENTINEL)));
    c.push(format!("  %ny_nr = fsub float {}, {r}", f32(0.0)));
    c.push(format!("  %ny_s1 = fdiv float {g}, %ny_nr")); // steps (ratio, survives fades)
    c.push(format!("  %ny_br1 = fdiv float {b}, %ny_nr")); // brightness
                                                           // the ratio trigger: green > 0 and |red - RATIO x green| < RATIO_TOL x green; steps = RATIO_STEPS x blue / green
    c.push(format!("  %ny_gpos = fcmp ogt float {g}, {}", f32(0.0001)));
    c.push(format!("  %ny_rg = fmul float {g}, {}", f32(RATIO)));
    c.push(format!("  %ny_rd = fsub float {r}, %ny_rg"));
    c.push("  %ny_rda = call float @dx.op.unary.f32(i32 6, float %ny_rd)".into()); // FAbs
    c.push(format!("  %ny_tol = fmul float {g}, {}", f32(RATIO_TOL)));
    c.push("  %ny_rok = fcmp olt float %ny_rda, %ny_tol".into());
    c.push("  %ny_rat = and i1 %ny_gpos, %ny_rok".into());
    c.push(format!("  %ny_bg = fdiv float {b}, {g}"));
    c.push(format!("  %ny_s2 = fmul float %ny_bg, {}", f32(RATIO_STEPS)));
    c.push("  %ny_on = or i1 %ny_neg, %ny_rat".into());
    c.push("  %ny_s = select i1 %ny_neg, float %ny_s1, float %ny_s2".into());
    c.push(format!("  %ny_br = select i1 %ny_neg, float %ny_br1, float {}", f32(1.0)));
    c.push(format!("  %ny_vs = fmul float {v}, %ny_s"));
    c.push("  %ny_vf = call float @dx.op.unary.f32(i32 27, float %ny_vs)".into()); // Round_ni: floor
    c.push(format!("  %ny_vh = fmul float %ny_vf, {}", f32(0.5)));
    c.push("  %ny_vp = call float @dx.op.unary.f32(i32 22, float %ny_vh)".into()); // Frc: 0 or 0.5
    c.push(format!("  %ny_off = fmul float %ny_vp, {}", f32(2.0 / 12.0))); // half a band on every other step
    c.push(format!("  %ny_u0 = fadd float {u}, %ny_off"));
    c.push(format!("  %ny_u = fmul float %ny_u0, {}", f32(6.0)));
    let channel = |p: (u8, u8, u8), ch: usize| [p.0, p.1, p.2][ch];
    for ch in 0..3 {
        let mut cur = f32(lin(channel(PALETTE[0], ch)));
        for (i, colour) in PALETTE.iter().enumerate().skip(1) {
            c.push(format!("  %ny_t{ch}_{i} = fcmp oge float %ny_u, {}", f32(i as f64)));
            c.push(format!("  %ny_c{ch}_{i} = select i1 %ny_t{ch}_{i}, float {}, float {cur}", f32(lin(channel(*colour, ch)))));
            cur = format!("%ny_c{ch}_{i}");
        }
        c.push(format!("  %ny_p{ch} = fmul float {cur}, %ny_br"));
    }
    c.push(format!("  %ny_k0 = fmul float {}, {}", k[0], k[1]));
    c.push(format!("  %ny_k = fmul float %ny_k0, {}", k[2]));
    c.push(format!("  %ny_live = fcmp ogt float {a}, {}", f32(0.02))); // opaque until the ribbon dies: no fade
    c.push(format!("  %ny_a = select i1 %ny_live, float {}, float {}", f32(1.0), f32(0.0)));
    for ch in 0..3u32 {
        c.push(format!("  %ny_q{ch} = fmul float %ny_p{ch}, %ny_k"));
        c.push(format!("  %ny_o{ch} = fmul float %ny_q{ch}, %ny_a")); // premultiplied, as the stock output
        c.push(format!("  %ny_f{ch} = select i1 %ny_on, float %ny_o{ch}, float {}", store(ch)));
    }
    c.push(format!("  %ny_f3 = select i1 %ny_on, float %ny_a, float {}", store(3)));
    let mut out = format!("{}{}\n{}", &ir_text[..at], c.join("\n"), &ir_text[at..]);
    for ch in 0..4u32 {
        let old = format!("storeOutput.f32(i32 5, i32 0, i32 0, i8 {ch}, float {})", store(ch));
        replace_once(&mut out, &old, &format!("storeOutput.f32(i32 5, i32 0, i32 0, i8 {ch}, float %ny_f{ch})"))?;
    }
    let node = ir::valver(&out).ok_or("no !dx.valver")?.to_string();
    Ok(ir::set_valver_17(&out, &node))
}
