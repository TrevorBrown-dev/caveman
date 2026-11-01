//! Caveman's deck effects patched into the game's deck pixel programs (`shaderpatch.patch`): the marker read from the
//! deck's holo map (t0, or t1 in variant B), the effect codes (none, hue cycle, static half turn, pulse, neon), the
//! clock, the exponential glow written to render target 3 in both G-buffer layouts, and neon (the emissive alone).
//!
//! With the animated-deck feature on ([`patch_anim`], knowledge/animated-decks.md) the same programs also play a
//! flipbook: the marker is read at fixed texels (so it is found in an atlas of any size), block 3 gives the frame
//! count, rate and grid, and where the stock code samples the composited art the frame from the atlas (the holo map
//! texture itself) takes its place, converted from sRGB; the holo mask that texture normally supplies is zeroed for
//! such decks. [`patch`] (the feature off) writes exactly what it did before.

use super::ir::{self, f32_lit as f32};
use super::{BLOCK, MARKER_H, MARKER_W};

/// `shaderpatch.CLOCK_REGISTER`: the PerPassConstants (b4) register whose x is the game clock in seconds.
pub const CLOCK_REGISTER: u32 = 262;

/// The patched disassembly and a note on what was found (`shaderpatch.patch`): Caveman's code just before the first
/// G-buffer store, the stores re-pointed, and the validator version set to 1.7.
pub fn patch(ir_text: &str) -> Result<(String, String), String> {
    patch_with(ir_text, false)
}

/// [`patch`] plus the flipbook for animated decks.
pub fn patch_anim(ir_text: &str) -> Result<(String, String), String> {
    patch_with(ir_text, true)
}

/// One insertion into the program text: at `at` (an offset of the original), `text`, and names whose later uses are
/// renamed from then on.
struct Insert {
    at: usize,
    text: String,
    renames: Vec<(String, String)>,
}

/// The args of a `call ... (a, b, c)` line, split at ", " (the DXIL calls here nest no parentheses).
fn call_args(line: &str) -> Vec<&str> {
    let Some(open) = line.find('(') else { return Vec::new() };
    let Some(close) = line[open..].find(')') else { return Vec::new() };
    line[open + 1..open + close].split(", ").collect()
}

/// The line holding `at`: (start, end past the newline).
fn line_of(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    (start, end)
}

/// The value defined by the line holding `at` (`  %name = ...`).
fn defined_name(text: &str, at: usize) -> Option<&str> {
    let (s, e) = line_of(text, at);
    let line = text[s..e].trim_start();
    let name = line.split(" = ").next()?;
    name.starts_with('%').then_some(name)
}

/// `old` renamed to `new` wherever it stands as a whole value name.
fn rename(text: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let b = text.as_bytes();
    for (i, _) in text.match_indices(old) {
        if i < last {
            continue;
        }
        let after = b.get(i + old.len()).copied();
        let whole = !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.');
        if whole {
            out.push_str(&text[last..i]);
            out.push_str(new);
            last = i + old.len();
        }
    }
    out.push_str(&text[last..]);
    out
}

fn patch_with(ir_text: &str, anim: bool) -> Result<(String, String), String> {
    let h = ir::handles(ir_text)?;
    let art_reg = ir::art_texture(ir_text)?;
    let art = ir::handle(&h, &art_reg)?;
    let samp = ir::handle(&h, "s0")?;
    let clock = ir::handle(&h, "cb4")?;
    let stores = ir::deck_stores(ir_text);
    let get = |row: u32, col: u32| stores.iter().find(|(k, _)| *k == (row, col)).map(|(_, s)| s);
    let first_store = stores.iter().map(|(_, s)| s.start).min().ok_or("no storeOutput")?;
    let line_start = ir_text[..first_store].rfind('\n').map_or(0, |i| i + 1);
    let base: Vec<&str> = (0..3).map(|c| get(1, c).map(|s| s.operand.as_str()).ok_or_else(|| format!("no SV_Target1 store {c}"))).collect::<Result<_, _>>()?;
    let glow_target = (0..3).all(|c| get(3, c).map_or("i32", |s| s.kind.as_str()) == "f32");
    // the marker, clock and effect flags: before the stores (as always), or at the top of the program when the
    // flipbook needs them earlier (anim)
    let mut early: Vec<String> = Vec::new();
    let mut code: Vec<String> = Vec::new();
    let mut inserts: Vec<Insert> = Vec::new();
    let mut anim_notes = String::new();

    {
        let setup: &mut Vec<String> = if anim { &mut early } else { &mut code };
        let sample = |code: &mut Vec<String>, name: &str, bx: u32| {
            if anim {
                // fixed texels: the centre of block bx's first 4 x 4 block row, whatever the texture's size
                code.push(format!(
                    "  %{name} = call %dx.types.ResRet.f32 @dx.op.textureLoad.f32(i32 66, %dx.types.Handle {art}, i32 0, i32 {}, i32 {}, \
                     i32 undef, i32 undef, i32 undef, i32 undef)",
                    bx * BLOCK + BLOCK / 2,
                    BLOCK / 2
                ));
            } else {
                let u = (f64::from(bx * BLOCK) + f64::from(BLOCK) / 2.0) / f64::from(MARKER_W);
                let v = (f64::from(BLOCK) / 2.0) / f64::from(MARKER_H);
                code.push(format!(
                    "  %{name} = call %dx.types.ResRet.f32 @dx.op.sampleLevel.f32(i32 62, %dx.types.Handle {art}, \
                     %dx.types.Handle {samp}, float {}, float {}, float undef, float undef, i32 0, i32 0, i32 undef, float 0.000000e+00)",
                    f32(u),
                    f32(v)
                ));
            }
            for (c, ch) in ["r", "g", "b"].iter().enumerate() {
                code.push(format!("  %{name}_{ch} = extractvalue %dx.types.ResRet.f32 %{name}, {c}"));
            }
        };
        sample(setup, "cvm_a", 0);
        sample(setup, "cvm_b", 1);
        sample(setup, "cvm_c", 2);
        if anim {
            sample(setup, "cvm_d", 3);
        }
        let (hi, lo) = (f32(0.9), f32(0.1));
        let tests = [
            ("cvm_a_r", "ogt", &hi),
            ("cvm_a_g", "olt", &lo),
            ("cvm_a_b", "ogt", &hi),
            ("cvm_b_r", "olt", &lo),
            ("cvm_b_g", "ogt", &hi),
            ("cvm_b_b", "olt", &lo),
        ];
        for (i, (v, op, k)) in tests.iter().enumerate() {
            setup.push(format!("  %cvm_t{i} = fcmp {op} float %{v}, {k}"));
        }
        setup.push("  %cvm_m0 = and i1 %cvm_t0, %cvm_t1".into());
        for i in 2..5 {
            setup.push(format!("  %cvm_m{} = and i1 %cvm_m{}, %cvm_t{i}", i - 1, i - 2));
        }
        setup.push("  %cvm_on = and i1 %cvm_m3, %cvm_t5".into());
        // time and angle
        setup.push(format!("  %cvm_clock = call %dx.types.CBufRet.f32 @dx.op.cbufferLoadLegacy.f32(i32 59, %dx.types.Handle {clock}, i32 {CLOCK_REGISTER})"));
        setup.push("  %cvm_t = extractvalue %dx.types.CBufRet.f32 %cvm_clock, 0".into());
        setup.push(format!("  %cvm_speed = fmul float %cvm_c_g, {}", f32(2.0)));
        setup.push("  %cvm_turns = fmul float %cvm_t, %cvm_speed".into());
        setup.push("  %cvm_frac = call float @dx.op.unary.f32(i32 22, float %cvm_turns)".into());
        // the effect (block 2 R): none < 0.125 <= hue cycle < 0.375 <= fixed half turn < 0.625 <= pulse < 0.875 <= neon
        setup.push(format!("  %cvm_is_cyc = fcmp oge float %cvm_c_r, {}", f32(0.125)));
        setup.push(format!("  %cvm_is_sta = fcmp oge float %cvm_c_r, {}", f32(0.375)));
        setup.push(format!("  %cvm_is_pul0 = fcmp oge float %cvm_c_r, {}", f32(0.625)));
        setup.push(format!("  %cvm_is_neo = fcmp oge float %cvm_c_r, {}", f32(0.875)));
        setup.push(format!("  %cvm_not_neo = fcmp olt float %cvm_c_r, {}", f32(0.875)));
        setup.push("  %cvm_is_pul = and i1 %cvm_is_pul0, %cvm_not_neo".into());
        setup.push("  %cvm_neon = and i1 %cvm_on, %cvm_is_neo".into());
        setup.push(format!("  %cvm_f0 = select i1 %cvm_is_cyc, float %cvm_frac, float {}", f32(0.0)));
        setup.push(format!("  %cvm_f1 = select i1 %cvm_is_sta, float {}, float %cvm_f0", f32(0.5)));
        setup.push(format!("  %cvm_f2 = select i1 %cvm_is_pul0, float {}, float %cvm_f1", f32(0.0)));
        setup.push(format!("  %cvm_ang = fmul float %cvm_f2, {}", f32(6.2831855)));
        // pulse: brightness 0.1 to 1 over 2 seconds of the clock
        setup.push(format!("  %cvm_pt = fmul float %cvm_t, {}", f32(0.5)));
        setup.push("  %cvm_pf = call float @dx.op.unary.f32(i32 22, float %cvm_pt)".into());
        setup.push(format!("  %cvm_pk = call float @dx.op.tertiary.f32(i32 46, float %cvm_pf, float {}, float {})", f32(0.9), f32(0.1)));
        setup.push(format!("  %cvm_bright = select i1 %cvm_is_pul, float %cvm_pk, float {}", f32(1.0)));
        setup.push("  %cvm_cos = call float @dx.op.unary.f32(i32 12, float %cvm_ang)".into());
        setup.push("  %cvm_sin = call float @dx.op.unary.f32(i32 13, float %cvm_ang)".into());
        if anim {
            // the flipbook: block 2 R in [0.04, 0.1) (EFFECT_ANIM 16); block 3 = frames, frames a second, 32 log2 cols
            // + 4 log2 rows; frame k = floor(frac(clock fps / n) n), at cell (k mod cols, k / cols)
            setup.push(format!("  %cvm_an0 = fcmp oge float %cvm_c_r, {}", f32(0.04)));
            setup.push(format!("  %cvm_an1 = fcmp olt float %cvm_c_r, {}", f32(0.1)));
            setup.push("  %cvm_an2 = and i1 %cvm_an0, %cvm_an1".into());
            setup.push("  %cvm_anim = and i1 %cvm_on, %cvm_an2".into());
            setup.push(format!("  %cvm_n0 = fmul float %cvm_d_r, {}", f32(255.0)));
            setup.push("  %cvm_n1 = call float @dx.op.unary.f32(i32 26, float %cvm_n0)".into());
            setup.push(format!("  %cvm_n = call float @dx.op.binary.f32(i32 35, float %cvm_n1, float {})", f32(1.0)));
            setup.push(format!("  %cvm_fps0 = fmul float %cvm_d_g, {}", f32(255.0)));
            setup.push("  %cvm_fps = call float @dx.op.unary.f32(i32 26, float %cvm_fps0)".into());
            setup.push(format!("  %cvm_g0 = fmul float %cvm_d_b, {}", f32(255.0 / 4.0)));
            setup.push("  %cvm_g1 = call float @dx.op.unary.f32(i32 26, float %cvm_g0)".into());
            setup.push(format!("  %cvm_lc0 = fmul float %cvm_g1, {}", f32(0.125)));
            setup.push("  %cvm_lc = call float @dx.op.unary.f32(i32 27, float %cvm_lc0)".into());
            setup.push(format!("  %cvm_lc8 = fmul float %cvm_lc, {}", f32(8.0)));
            setup.push("  %cvm_lr = fsub float %cvm_g1, %cvm_lc8".into());
            setup.push("  %cvm_cols = call float @dx.op.unary.f32(i32 21, float %cvm_lc)".into());
            setup.push("  %cvm_rows = call float @dx.op.unary.f32(i32 21, float %cvm_lr)".into());
            setup.push("  %cvm_tf = fmul float %cvm_t, %cvm_fps".into());
            setup.push("  %cvm_tn = fdiv float %cvm_tf, %cvm_n".into());
            setup.push("  %cvm_q = call float @dx.op.unary.f32(i32 22, float %cvm_tn)".into());
            setup.push("  %cvm_kf = fmul float %cvm_q, %cvm_n".into());
            setup.push("  %cvm_k = call float @dx.op.unary.f32(i32 27, float %cvm_kf)".into());
            setup.push("  %cvm_rk0 = fdiv float %cvm_k, %cvm_cols".into());
            setup.push("  %cvm_rowf = call float @dx.op.unary.f32(i32 27, float %cvm_rk0)".into());
            setup.push("  %cvm_rowc = fmul float %cvm_rowf, %cvm_cols".into());
            setup.push("  %cvm_colf = fsub float %cvm_k, %cvm_rowc".into());
        }
    }

    if anim {
        // the top of the program: after the last of the handles the marker code uses
        let mut top = 0;
        for name in [art, samp, clock] {
            let needle = format!("  {name} = call %dx.types.Handle @dx.op.createHandle(");
            let at = ir_text.find(&needle).ok_or_else(|| format!("no definition of handle {name}"))?;
            top = top.max(line_of(ir_text, at).1);
        }
        inserts.push(Insert { at: top, text: format!("{}\n", early.join("\n")), renames: Vec::new() });

        // the composited art: the 2d texture three registers above the holo map, sampled once at the art UV
        let holo_index: u32 = art_reg[1..].parse().map_err(|_| format!("holo map register {art_reg}"))?;
        let comp_reg = format!("t{}", holo_index + 3);
        let comp = ir::handle(&h, &comp_reg)?;
        let needle = format!("@dx.op.sampleBias.f32(i32 61, %dx.types.Handle {comp}, ");
        let hits: Vec<usize> = ir_text.match_indices(&needle).map(|(i, _)| i).collect();
        if hits.len() != 1 {
            return Err(format!("the composited art ({comp_reg}) is sampled {} times, expected once", hits.len()));
        }
        let (s, e) = line_of(ir_text, hits[0]);
        let line = &ir_text[s..e];
        let result = defined_name(ir_text, hits[0]).ok_or("the art sample defines no value")?.to_string();
        let args = call_args(line);
        if args.len() != 12 {
            return Err(format!("the art sample has {} args", args.len()));
        }
        let (u, v, bias) = (args[3].trim_start_matches("float "), args[4].trim_start_matches("float "), args[10].trim_start_matches("float "));
        let mut rgb = Vec::new();
        let mut after = 0;
        for c in 0..3 {
            let needle = format!(" = extractvalue %dx.types.ResRet.f32 {result}, {c}");
            let at = ir_text.find(&needle).ok_or_else(|| format!("no channel {c} of the art sample"))?;
            rgb.push(defined_name(ir_text, at).ok_or("the art channel defines no value")?.to_string());
            after = after.max(line_of(ir_text, at).1);
        }
        let mut frame: Vec<String> = Vec::new();
        frame.push(format!("  %cvm_fu0 = fadd float %cvm_colf, {u}"));
        frame.push("  %cvm_fu = fdiv float %cvm_fu0, %cvm_cols".into());
        frame.push(format!("  %cvm_fv0 = fadd float %cvm_rowf, {v}"));
        frame.push("  %cvm_fv = fdiv float %cvm_fv0, %cvm_rows".into());
        frame.push(format!(
            "  %cvm_fs = call %dx.types.ResRet.f32 @dx.op.sampleBias.f32(i32 61, %dx.types.Handle {art}, %dx.types.Handle {samp}, \
             float %cvm_fu, float %cvm_fv, float undef, float undef, i32 0, i32 0, i32 undef, float {bias}, float undef)"
        ));
        let mut renames = Vec::new();
        for (c, ch) in ["r", "g", "b"].iter().enumerate() {
            // the atlas holds sRGB bytes in a linear texture: the transfer curve here
            frame.push(format!("  %cvm_fs_{ch} = extractvalue %dx.types.ResRet.f32 %cvm_fs, {c}"));
            frame.push(format!("  %cvm_sl_{ch} = fmul float %cvm_fs_{ch}, {}", f32(1.0 / 12.92)));
            frame.push(format!("  %cvm_sa_{ch} = fadd float %cvm_fs_{ch}, {}", f32(0.055)));
            frame.push(format!("  %cvm_sb_{ch} = fmul float %cvm_sa_{ch}, {}", f32(1.0 / 1.055)));
            frame.push(format!("  %cvm_sm_{ch} = call float @dx.op.binary.f32(i32 35, float %cvm_sb_{ch}, float {})", f32(1e-6)));
            frame.push(format!("  %cvm_sc_{ch} = call float @dx.op.unary.f32(i32 23, float %cvm_sm_{ch})"));
            frame.push(format!("  %cvm_sd_{ch} = fmul float %cvm_sc_{ch}, {}", f32(2.4)));
            frame.push(format!("  %cvm_se_{ch} = call float @dx.op.unary.f32(i32 21, float %cvm_sd_{ch})"));
            frame.push(format!("  %cvm_sf_{ch} = fcmp ole float %cvm_fs_{ch}, {}", f32(0.04045)));
            frame.push(format!("  %cvm_sg_{ch} = select i1 %cvm_sf_{ch}, float %cvm_sl_{ch}, float %cvm_se_{ch}"));
            frame.push(format!("  %cvm_art_{ch} = select i1 %cvm_anim, float %cvm_sg_{ch}, float {}", rgb[c]));
            renames.push((rgb[c].clone(), format!("%cvm_art_{ch}")));
        }
        inserts.push(Insert { at: after, text: format!("{}\n", frame.join("\n")), renames });
        anim_notes.push_str(&format!(", anim: art {comp_reg} {}", ir::py_list(&rgb.iter().map(|s| s.as_str()).collect::<Vec<_>>())));

        // the holo mask the stock code reads from the same texture: zero for animated decks
        let needle = format!("@dx.op.sampleBias.f32(i32 61, %dx.types.Handle {art}, ");
        let holo_hits: Vec<usize> = ir_text.match_indices(&needle).map(|(i, _)| i).collect();
        if holo_hits.len() == 1 {
            let result = defined_name(ir_text, holo_hits[0]).ok_or("the holo sample defines no value")?.to_string();
            let needle = format!(" = extractvalue %dx.types.ResRet.f32 {result}, 0");
            if let Some(at) = ir_text.find(&needle) {
                let red = defined_name(ir_text, at).ok_or("the holo channel defines no value")?.to_string();
                let end = line_of(ir_text, at).1;
                inserts.push(Insert {
                    at: end,
                    text: format!("  %cvm_hr = select i1 %cvm_anim, float {}, float {red}\n", f32(0.0)),
                    renames: vec![(red.clone(), "%cvm_hr".to_string())],
                });
                anim_notes.push_str(&format!(", holo mask {red} off"));
            } else {
                anim_notes.push_str(", holo mask untouched (no red read)");
            }
        } else {
            anim_notes.push_str(&format!(", holo mask untouched ({} samples)", holo_hits.len()));
        }
    }

    // rotate the colour about the grey axis: c cos + (k x c) sin + k (k . c)(1 - cos), k = (1, 1, 1) / sqrt 3
    let (r, g, b) = (base[0], base[1], base[2]);
    code.push(format!("  %cvm_s0 = fadd float {r}, {g}"));
    code.push(format!("  %cvm_s1 = fadd float %cvm_s0, {b}"));
    code.push(format!("  %cvm_mean = fmul float %cvm_s1, {}", f32(1.0 / 3.0)));
    code.push(format!("  %cvm_omc = fsub float {}, %cvm_cos", f32(1.0)));
    code.push("  %cvm_grey = fmul float %cvm_mean, %cvm_omc".into());
    code.push(format!("  %cvm_ks = fmul float %cvm_sin, {}", f32(0.57735026)));
    for (ch, (x, d0, d1)) in ["r", "g", "b"].iter().zip([(r, b, g), (g, r, b), (b, g, r)]) {
        code.push(format!("  %cvm_{ch}c = fmul float {x}, %cvm_cos"));
        code.push(format!("  %cvm_{ch}d = fsub float {d0}, {d1}"));
        code.push(format!("  %cvm_{ch}x = fmul float %cvm_{ch}d, %cvm_ks"));
        code.push(format!("  %cvm_{ch}y = fadd float %cvm_{ch}c, %cvm_{ch}x"));
        code.push(format!("  %cvm_{ch}z = fadd float %cvm_{ch}y, %cvm_grey"));
        code.push(format!("  %cvm_{ch}s = call float @dx.op.unary.f32(i32 7, float %cvm_{ch}z)"));
        code.push(format!("  %cvm_{ch}p = fmul float %cvm_{ch}s, %cvm_bright"));
        code.push(format!("  %cvm_out_{ch} = select i1 %cvm_on, float %cvm_{ch}p, float {x}"));
    }
    // glow: render target 3 is the emissive buffer (knowledge/glow.md). Layout A stores it as float; layout B as
    // half floats in a uint4 whose alpha is a code (kept: with the deck's codes the emissive adds to the lit colour)
    let glow_half = !glow_target && (0..3).all(|c| get(3, c).map_or("f32", |s| s.kind.as_str()) == "i32");
    let rt3 = |c: u32| get(3, c).map(|s| s.operand.clone()).ok_or_else(|| format!("no SV_Target3 store {c}"));
    if glow_target || glow_half {
        // (names: %cvm_<channel><step> belong to the colour rotation above, so these say "glow")
        code.push(format!("  %cvm_glow_x = fmul float %cvm_c_b, {}", f32(12.0)));
        code.push("  %cvm_glow_e = call float @dx.op.unary.f32(i32 21, float %cvm_glow_x)".into());
        code.push(format!("  %cvm_glow = fsub float %cvm_glow_e, {}", f32(1.0)));
        for (c, ch) in ["r", "g", "b"].iter().enumerate() {
            code.push(format!("  %cvm_{ch}e = fmul float %cvm_out_{ch}, %cvm_glow"));
            if glow_target {
                code.push(format!("  %cvm_{ch}g = select i1 %cvm_on, float %cvm_{ch}e, float {}", rt3(c as u32)?));
            } else {
                code.push(format!("  %cvm_{ch}h = call i32 @dx.op.legacyF32ToF16(i32 130, float %cvm_{ch}e)"));
            }
        }
    }
    let mut out = if anim {
        inserts.push(Insert { at: line_start, text: format!("{}\n", code.join("\n")), renames: Vec::new() });
        inserts.sort_by_key(|i| i.at);
        let mut out = String::with_capacity(ir_text.len() + 8192);
        let mut cursor = 0;
        let mut pending: Vec<(String, String)> = Vec::new();
        for ins in inserts {
            let mut seg = ir_text[cursor..ins.at].to_string();
            for (old, new) in &pending {
                seg = rename(&seg, old, new);
            }
            out.push_str(&seg);
            out.push_str(&ins.text);
            pending.extend(ins.renames);
            cursor = ins.at;
        }
        let mut seg = ir_text[cursor..].to_string();
        for (old, new) in &pending {
            seg = rename(&seg, old, new);
        }
        out.push_str(&seg);
        out
    } else {
        format!("{}{}\n{}", &ir_text[..line_start], code.join("\n"), &ir_text[line_start..])
    };
    for (c, ch) in ["r", "g", "b"].iter().enumerate() {
        let old = format!("storeOutput.f32(i32 5, i32 1, i32 0, i8 {c}, float {})", base[c]);
        replace_once(&mut out, &old, &format!("storeOutput.f32(i32 5, i32 1, i32 0, i8 {c}, float %cvm_out_{ch})"))?;
        if glow_target {
            let old = format!("storeOutput.f32(i32 5, i32 3, i32 0, i8 {c}, float {})", rt3(c as u32)?);
            replace_once(&mut out, &old, &format!("storeOutput.f32(i32 5, i32 3, i32 0, i8 {c}, float %cvm_{ch}g)"))?;
        } else if glow_half {
            // the stock half-float operand is computed after Caveman's block: select right before the store
            let operand = rt3(c as u32)?;
            let target = format!("call void @dx.op.storeOutput.i32(i32 5, i32 3, i32 0, i8 {c}, i32 {operand})");
            let new = |indent: &str| {
                format!(
                    "{indent}%cvm_{ch}g = select i1 %cvm_on, i32 %cvm_{ch}h, i32 {operand}\n\
                     {indent}call void @dx.op.storeOutput.i32(i32 5, i32 3, i32 0, i8 {c}, i32 %cvm_{ch}g)"
                )
            };
            sub_line_once(&mut out, &target, new, &format!("RT3 store {c}"))?;
        }
    }
    if glow_half && !out.contains("declare i32 @dx.op.legacyF32ToF16(") {
        return Err("legacyF32ToF16 is not declared in this program".into());
    }
    // neon: the emissive alone, unlit. Layout B: the alpha code's high byte (the lighting weight) to 0; layout A:
    // alpha (inferred to be the lighting weight) to 0. Selected right before the store (the code is computed late).
    if glow_target || glow_half {
        let (kind, arg) = if glow_target { ("f32", "float") } else { ("i32", "i32") };
        let operand = rt3(3)?;
        let target = format!("call void @dx.op.storeOutput.{kind}(i32 5, i32 3, i32 0, i8 3, {arg} {operand})");
        let zero = f32(0.0);
        let new = |indent: &str| {
            let neon = if glow_target {
                format!("%cvm_na = select i1 %cvm_neon, float {zero}, float {operand}")
            } else {
                format!("%cvm_nm = and i32 {operand}, 255\n{indent}%cvm_na = select i1 %cvm_neon, i32 %cvm_nm, i32 {operand}")
            };
            format!("{indent}{neon}\n{indent}call void @dx.op.storeOutput.{kind}(i32 5, i32 3, i32 0, i8 3, {arg} %cvm_na)")
        };
        sub_line_once(&mut out, &target, new, "RT3 alpha store")?;
    }
    if anim {
        if !out.contains("declare %dx.types.ResRet.f32 @dx.op.textureLoad.f32(") {
            out = out.replacen(
                "\nattributes #0",
                "\ndeclare %dx.types.ResRet.f32 @dx.op.textureLoad.f32(i32, %dx.types.Handle, i32, i32, i32, i32, i32, i32, i32) #2\n\nattributes #0",
                1,
            );
        }
        for decl in ["declare float @dx.op.unary.f32(", "declare float @dx.op.binary.f32(", "declare %dx.types.ResRet.f32 @dx.op.sampleBias.f32("] {
            if !out.contains(decl) {
                return Err(format!("{decl}...) is not declared in this program"));
            }
        }
    } else if !out.contains("declare %dx.types.ResRet.f32 @dx.op.sampleLevel.f32(") {
        out = out.replacen(
            "\nattributes #0",
            "\ndeclare %dx.types.ResRet.f32 @dx.op.sampleLevel.f32(i32, %dx.types.Handle, %dx.types.Handle, float, float, \
             float, float, i32, i32, i32, float) #2\n\nattributes #0",
            1,
        );
    }
    // the game's dxil.dll validates up to 1.7; the shipped programs say 1.10 (dxcoob 1.8.2505)
    let node = ir::valver(&out).ok_or("no !dx.valver")?.to_string();
    let out = ir::set_valver_17(&out, &node);
    let glow = if glow_target {
        "RT3 float"
    } else if glow_half {
        "RT3 half"
    } else {
        "none"
    };
    let notes = format!("holo map {art_reg}, base colour {}, glow {glow}{anim_notes}", ir::py_list(&base));
    Ok((out, notes))
}

/// `assert out.count(old) == 1; out = out.replace(old, new)`.
pub(super) fn replace_once(out: &mut String, old: &str, new: &str) -> Result<(), String> {
    if out.matches(old).count() != 1 {
        return Err(format!("expected exactly one {old}"));
    }
    *out = out.replacen(old, new, 1);
    Ok(())
}

/// `re.compile(rf'^(\s*){re.escape(target)}', re.M)`: exactly one match, replaced by `new(indent)`.
fn sub_line_once(out: &mut String, target: &str, new: impl Fn(&str) -> String, what: &str) -> Result<(), String> {
    let found = ir::line_anchored(out, target);
    if found.len() != 1 {
        return Err(format!("{what}: {} matches", found.len()));
    }
    let (s, p) = found[0];
    let replacement = new(&out[s..p]);
    out.replace_range(s..p + target.len(), &replacement);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_whole_names_only() {
        assert_eq!(rename("%104 %1040 %104, (%104)", "%104", "%x"), "%x %1040 %x, (%x)");
        assert_eq!(call_args("  %1 = call f(i32 61, %dx.types.Handle %5, float %96)").len(), 3);
    }
}
