"""Caveman's solid rainbow trail: patches the game's ribbon pixel program (the material of
effects/_global/emittergraphs/eg_ribbon_trail_distance_emit_a) so a ribbon whose colour is a "sentinel" (red below
-0.5) draws six hard-edged rainbow bands across its width, stepped Nyan Cat style along its length, opaque, with no
noise and no soft edge. Every other ribbon (Kichi's, the oni costume's, Caveman's aura) is unchanged.
Usage: nyanpatch.py <game dir> <out dir>   (writes the patched, signed program as a shaders/bytecode resource; see
knowledge/trails.md)"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dxc  # noqa: E402
from shaderpatch import CONFIG_BUNDLE, GLOBALS_TOC, f32, wrap  # noqa: E402

# global db (globals.toc delmargameconfiguration): material key 2720338288095328273 -> record 376 -> permutation
# 14243237207213057914 (both of its entries) -> this program
RIBBON_PROGRAM = 'f9af19ba-1e20-e282-f9a7-938f31781dc2'
# Nyan Cat's six bands, top to bottom (sRGB), as linear values
PALETTE = [(255, 0, 0), (255, 153, 0), (255, 255, 0), (51, 255, 0), (0, 153, 255), (102, 51, 255)]
SENTINEL = -0.5         # ribbon colour red below this: nyan mode (natural colours are never negative)
# The emitter's colour carries the settings: (-1, S, B): S = steps per unit of the along-ribbon coordinate (the band
# pattern shifts by half a band on every other step; 0 = straight bands), B = brightness (1 = the palette as is).
# A second trigger with no negative numbers (the first showed no trail at all in game, 2026-10-20; negative colours may
# break the particle system before the pixel shader): colour (RATIO, 1, S / RATIO_STEPS), red exactly RATIO x green
# (a ratio survives fades, which scale all channels alike).
RATIO, RATIO_TOL, RATIO_STEPS = 0.0371, 0.002, 64.0


def _lin(c):
    c = c / 255
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def patch(ir):
    """The ribbon program's disassembly with the nyan branch. Returns the new IR."""
    stores = {}
    for m in re.finditer(r'call void @dx\.op\.storeOutput\.f32\(i32 5, i32 0, i32 0, i8 (\d), float ([^)]+)\)', ir):
        stores[int(m.group(1))] = (m.group(2), m.start())
    assert sorted(stores) == [0, 1, 2, 3], 'expected one float4 SV_Target'
    inputs = {}
    for m in re.finditer(r'(%\d+) = call float @dx\.op\.loadInput\.f32\(i32 4, i32 (\d), i32 0, i8 (\d), i32 undef\)', ir):
        inputs[(int(m.group(2)), int(m.group(3)))] = m.group(1)
    u, v = inputs[(0, 0)], inputs[(0, 1)]                                # TEXCOORD0.x across the width, .y along
    r, g, b, a = (inputs[(1, c)] for c in range(4))                      # TEXCOORD1: the particle colour and alpha
    # the stock exposure-like scalars the colour is multiplied by: cb4[459].w, cb4[479].w, cb1[4].w
    scal = re.findall(r'(%\d+) = extractvalue %dx\.types\.CBufRet\.f32 (%\d+), 3', ir)
    loads = dict(re.findall(r'(%\d+) = call %dx\.types\.CBufRet\.f32 @dx\.op\.cbufferLoadLegacy\.f32\(i32 59, %dx\.types\.Handle %\d+, i32 (\d+)\)', ir))
    def scalar(reg):
        return next(s for s, load in scal if loads.get(load) == str(reg))
    k = [scalar(459), scalar(479), scalar(4)]
    first = min(s[1] for s in stores.values())
    at = ir.rfind('\n', 0, first) + 1
    c = []
    c.append(f'  %ny_neg = fcmp olt float {r}, {f32(SENTINEL)}')
    c.append(f'  %ny_nr = fsub float {f32(0.0)}, {r}')
    c.append(f'  %ny_s1 = fdiv float {g}, %ny_nr')                      # steps (ratio, survives fades)
    c.append(f'  %ny_br1 = fdiv float {b}, %ny_nr')                     # brightness
    # the ratio trigger: green > 0 and |red - RATIO x green| < RATIO_TOL x green; steps = RATIO_STEPS x blue / green
    c.append(f'  %ny_gpos = fcmp ogt float {g}, {f32(0.0001)}')
    c.append(f'  %ny_rg = fmul float {g}, {f32(RATIO)}')
    c.append(f'  %ny_rd = fsub float {r}, %ny_rg')
    c.append('  %ny_rda = call float @dx.op.unary.f32(i32 6, float %ny_rd)')   # FAbs
    c.append(f'  %ny_tol = fmul float {g}, {f32(RATIO_TOL)}')
    c.append('  %ny_rok = fcmp olt float %ny_rda, %ny_tol')
    c.append('  %ny_rat = and i1 %ny_gpos, %ny_rok')
    c.append(f'  %ny_bg = fdiv float {b}, {g}')
    c.append(f'  %ny_s2 = fmul float %ny_bg, {f32(RATIO_STEPS)}')
    c.append('  %ny_on = or i1 %ny_neg, %ny_rat')
    c.append('  %ny_s = select i1 %ny_neg, float %ny_s1, float %ny_s2')
    c.append(f'  %ny_br = select i1 %ny_neg, float %ny_br1, float {f32(1.0)}')
    c.append(f'  %ny_vs = fmul float {v}, %ny_s')
    c.append('  %ny_vf = call float @dx.op.unary.f32(i32 27, float %ny_vs)')   # Round_ni: floor
    c.append(f'  %ny_vh = fmul float %ny_vf, {f32(0.5)}')
    c.append('  %ny_vp = call float @dx.op.unary.f32(i32 22, float %ny_vh)')   # Frc: 0 or 0.5
    c.append(f'  %ny_off = fmul float %ny_vp, {f32(2.0 / 12)}')       # half a band on every other step
    c.append(f'  %ny_u0 = fadd float {u}, %ny_off')
    c.append(f'  %ny_u = fmul float %ny_u0, {f32(6.0)}')
    for ch in range(3):
        cur = f32(_lin(PALETTE[0][ch]))
        for i in range(1, 6):
            c.append(f'  %ny_t{ch}_{i} = fcmp oge float %ny_u, {f32(float(i))}')
            c.append(f'  %ny_c{ch}_{i} = select i1 %ny_t{ch}_{i}, float {f32(_lin(PALETTE[i][ch]))}, float {cur}')
            cur = f'%ny_c{ch}_{i}'
        c.append(f'  %ny_p{ch} = fmul float {cur}, %ny_br')
    c.append(f'  %ny_k0 = fmul float {k[0]}, {k[1]}')
    c.append(f'  %ny_k = fmul float %ny_k0, {k[2]}')
    c.append(f'  %ny_live = fcmp ogt float {a}, {f32(0.02)}')               # opaque until the ribbon dies: no fade
    c.append(f'  %ny_a = select i1 %ny_live, float {f32(1.0)}, float {f32(0.0)}')
    for ch in range(3):
        c.append(f'  %ny_q{ch} = fmul float %ny_p{ch}, %ny_k')
        c.append(f'  %ny_o{ch} = fmul float %ny_q{ch}, %ny_a')          # premultiplied, as the stock output
        c.append(f'  %ny_f{ch} = select i1 %ny_on, float %ny_o{ch}, float {stores[ch][0]}')
    c.append(f'  %ny_f3 = select i1 %ny_on, float %ny_a, float {stores[3][0]}')
    out = ir[:at] + '\n'.join(c) + '\n' + ir[at:]
    for ch in range(4):
        old = f'storeOutput.f32(i32 5, i32 0, i32 0, i8 {ch}, float {stores[ch][0]})'
        assert out.count(old) == 1, old
        out = out.replace(old, f'storeOutput.f32(i32 5, i32 0, i32 0, i8 {ch}, float %ny_f{ch})')
    valver = re.search(r'!dx\.valver = !\{!(\d+)\}', out).group(1)
    return re.sub(rf'^!{valver} = !\{{i32 1, i32 \d+\}}$', f'!{valver} = !{{i32 1, i32 7}}', out, flags=re.M)


def build(game_dir):
    """(name, payload, meta) of the patched ribbon program, built from the player's own game."""
    import gamedata
    game = gamedata.Game(game_dir)
    bnd = game.bundle(CONFIG_BUNDLE, toc=GLOBALS_TOC)
    m = bnd['manifest']
    name = f'shaders/bytecode/{RIBBON_PROGRAM}'
    i = next(i for i, r in enumerate(m['res']) if r['name'] == name)
    original = game.decode(bnd['files'][1 + len(m['ebx']) + i])
    d = dxc.Dxc(game_dir)
    ir = d.disassemble(dxc.read_bytecode_resource(original)['dxbc'])
    obj, errors, ok = d.assemble(patch(ir))
    if not ok:
        raise RuntimeError(f'{name}: {errors}')
    payload, meta = wrap(original, obj)
    return name, payload, meta


if __name__ == '__main__':
    name, payload, meta = build(sys.argv[1])
    os.makedirs(sys.argv[2], exist_ok=True)
    open(os.path.join(sys.argv[2], f'{RIBBON_PROGRAM}.bin'), 'wb').write(payload)
    print(f'{name}: {len(payload)} bytes, meta {meta.hex()}, assembled, validated and signed')
