"""Caveman's solid rainbow trail, the geometry half: patches the ribbon's VERTEX program so a nyan ribbon (the ratio
trigger of nyanpatch.py: particle colour red = RATIO x green) lies flat (its width runs horizontally, perpendicular to
the trail, instead of turning to face the camera) and is pulled BIAS metres toward the camera (it wins against the
ground it would clip into, not against the board above it). Every other ribbon is unchanged.
Usage: nyanvspatch.py <game dir> <out dir>   (writes the patched, signed program; see knowledge/trails-geometry.md)

Where the program is: the ribbon material's permutation (expressionshader/permutation14243237207213057914) names
two programs: +0x20 the vertex program (this one), +0x30 the pixel program (nyanpatch.py)."""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dxc  # noqa: E402
from nyanpatch import RATIO, RATIO_TOL  # noqa: E402
from shaderpatch import CONFIG_BUNDLE, GLOBALS_TOC, f32, wrap  # noqa: E402

RIBBON_VS_PROGRAM = '6cfc6613-53b8-5466-0a0f-1c718a455601'
BIAS = 0.04             # metres toward the camera (the deck's underside is about 3 cm above the trail)


def _one(pattern, text, what, flags=re.M):
    found = list(re.finditer(pattern, text, flags))
    if len(found) != 1:
        raise ValueError(f'{what}: expected one match, found {len(found)}')
    return found[0]


def _gate(prefix, r, g):
    """IR lines setting %<prefix>_on: the ratio trigger on the particle colour (as nyanpatch's pixel branch)."""
    return [f'  %{prefix}_gp = fcmp ogt float {g}, {f32(0.0001)}',
            f'  %{prefix}_rg = fmul float {g}, {f32(RATIO)}',
            f'  %{prefix}_rd = fsub float {r}, %{prefix}_rg',
            f'  %{prefix}_ra = call float @dx.op.unary.f32(i32 6, float %{prefix}_rd)',    # FAbs
            f'  %{prefix}_tl = fmul float {g}, {f32(RATIO_TOL)}',
            f'  %{prefix}_ok = fcmp olt float %{prefix}_ra, %{prefix}_tl',
            f'  %{prefix}_on = and i1 %{prefix}_gp, %{prefix}_ok']


def _rename_after(text, at, mapping):
    """`text` with each %name of `mapping` replaced by its new name after offset `at` (whole names only)."""
    tail = text[at:]
    for old, new in mapping.items():
        tail = re.sub(rf'{re.escape(old)}\b', new, tail)
    return text[:at] + tail


def patch(ir):
    """The ribbon vertex program's disassembly with the nyan geometry. Returns the new IR."""
    colour = dict(re.findall(r'storeOutput\.f32\(i32 5, i32 1, i32 0, i8 (\d), float (%\d+)\)', ir))
    r, g = colour['0'], colour['1']                                   # TEXCOORD1: the particle colour
    corner_y = _one(r'^  (%\d+) = call float @dx\.op\.loadInput\.f32\(i32 4, i32 1, i32 0, i8 1, i32 undef\)', ir,
                    'POSITION.y').group(1)
    # the width direction: three selects on "far end of the segment", then width x direction.x
    w = _one(r'^  (%\d+) = select i1 (%\d+), float %\d+, float %\d+\n'
             r'  (%\d+) = select i1 \2, float %\d+, float %\d+\n'
             r'  (%\d+) = select i1 \2, float %\d+, float %\d+\n'
             r'  (%\d+) = select i1 \2, float %\d+, float %\d+\n'
             r'  %\d+ = fmul float \5, \1\n', ir, 'width direction')
    dx, far, dy, dz = w.group(1), w.group(2), w.group(3), w.group(4)
    # the segment (next point - this point): what (POSITION.y + 1) / 2 moves the vertex along
    s = _one(rf'^  (%\d+) = fadd float {re.escape(corner_y)}, 1\.000000e\+00\n'
             r'  (%\d+) = fmul float \1, 5\.000000e-01\n'
             r'  %\d+ = fmul float \2, (%\d+)\n'
             r'  %\d+ = fmul float \2, (%\d+)\n'
             r'  %\d+ = fmul float \2, (%\d+)\n', ir, 'segment')
    seg_x, seg_z = s.group(3), s.group(5)
    # the tangents the stock code uses: this point's (segment + previous) and the next point's (segment + next)
    tx = re.findall(rf'^  (%\d+) = fadd float {re.escape(seg_x)}, %\d+$', ir, re.M)
    tz = re.findall(rf'^  (%\d+) = fadd float {re.escape(seg_z)}, %\d+$', ir, re.M)
    if len(tx) != 2 or len(tz) != 2:
        raise ValueError(f'tangents: expected two each, found {tx} {tz}')
    flat = _gate('nyw', r, g) + [
        f'  %nyw_tx = select i1 {far}, float {tx[1]}, float {tx[0]}',
        f'  %nyw_tz = select i1 {far}, float {tz[1]}, float {tz[0]}',
        '  %nyw_xx = fmul float %nyw_tx, %nyw_tx',
        '  %nyw_zz = fmul float %nyw_tz, %nyw_tz',
        '  %nyw_l2 = fadd float %nyw_xx, %nyw_zz',
        f'  %nyw_l2e = fadd float %nyw_l2, {f32(1e-12)}',
        '  %nyw_il = call float @dx.op.unary.f32(i32 25, float %nyw_l2e)',                 # Rsqrt
        f'  %nyw_mz = fsub float {f32(0.0)}, %nyw_tz',
        '  %nyw_fx = fmul float %nyw_mz, %nyw_il',                                         # (-t.z, 0, t.x) / |t.xz|
        '  %nyw_fz = fmul float %nyw_tx, %nyw_il',
        f'  %nyw_dx = select i1 %nyw_on, float %nyw_fx, float {dx}',
        f'  %nyw_dy = select i1 %nyw_on, float {f32(0.0)}, float {dy}',
        f'  %nyw_dz = select i1 %nyw_on, float %nyw_fz, float {dz}']
    at = w.start(5) - 2                                               # before the width select (after the three)
    out = ir[:at] + '\n'.join(flat) + '\n' + ir[at:]
    out = _rename_after(out, at + len('\n'.join(flat)) + 1, {dx: '%nyw_dx', dy: '%nyw_dy', dz: '%nyw_dz'})
    # the depth bias: the vertex relative to the camera (position - camera, register 455), right before clip space
    cam_load = _one(r'^  (%\d+) = call %dx\.types\.CBufRet\.f32 @dx\.op\.cbufferLoadLegacy\.f32\(i32 59, '
                    r'%dx\.types\.Handle %\d+, i32 455\)', out, 'camera').group(1)
    cam = [_one(rf'^  (%\d+) = extractvalue %dx\.types\.CBufRet\.f32 {re.escape(cam_load)}, {c}$', out,
                f'camera {c}').group(1) for c in range(3)]
    clip = _one(r'^  %\d+ = call %dx\.types\.CBufRet\.f32 @dx\.op\.cbufferLoadLegacy\.f32\(i32 59, '
                r'%dx\.types\.Handle %\d+, i32 421\)', out, 'view-projection').start()
    rel = _one(rf'^  (%\d+) = fsub float %\d+, {re.escape(cam[0])}\n'
               rf'  (%\d+) = fsub float %\d+, {re.escape(cam[1])}\n'
               rf'  (%\d+) = fsub float %\d+, {re.escape(cam[2])}\n', out[clip:], 'camera-relative position')
    rx, ry, rz = rel.group(1), rel.group(2), rel.group(3)
    bias = _gate('nyb', r, g) + [
        f'  %nyb_d2 = call float @dx.op.dot3.f32(i32 55, float {rx}, float {ry}, float {rz}, float {rx}, float {ry}, float {rz})',
        f'  %nyb_d2e = fadd float %nyb_d2, {f32(1e-12)}',
        '  %nyb_il = call float @dx.op.unary.f32(i32 25, float %nyb_d2e)',                 # 1 / distance
        f'  %nyb_k = fmul float %nyb_il, {f32(BIAS)}',
        f'  %nyb_s0 = fsub float {f32(1.0)}, %nyb_k',
        f'  %nyb_s1 = call float @dx.op.binary.f32(i32 35, float %nyb_s0, float {f32(0.0)})',   # FMax
        f'  %nyb_s = select i1 %nyb_on, float %nyb_s1, float {f32(1.0)}',
        f'  %nyb_x = fmul float {rx}, %nyb_s',
        f'  %nyb_y = fmul float {ry}, %nyb_s',
        f'  %nyb_z = fmul float {rz}, %nyb_s']
    at = clip + rel.end()
    out = out[:at] + '\n'.join(bias) + '\n' + out[at:]
    out = _rename_after(out, at + len('\n'.join(bias)) + 1, {rx: '%nyb_x', ry: '%nyb_y', rz: '%nyb_z'})
    valver = re.search(r'!dx\.valver = !\{!(\d+)\}', out).group(1)
    return re.sub(rf'^!{valver} = !\{{i32 1, i32 \d+\}}$', f'!{valver} = !{{i32 1, i32 7}}', out, flags=re.M)


def build(game_dir):
    """(name, payload, meta) of the patched ribbon vertex program, built from the player's own game."""
    import gamedata
    game = gamedata.Game(game_dir)
    bnd = game.bundle(CONFIG_BUNDLE, toc=GLOBALS_TOC)
    m = bnd['manifest']
    name = f'shaders/bytecode/{RIBBON_VS_PROGRAM}'
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
    open(os.path.join(sys.argv[2], f'{RIBBON_VS_PROGRAM}.bin'), 'wb').write(payload)
    print(f'{name}: {len(payload)} bytes, meta {meta.hex()}, assembled, validated and signed')
