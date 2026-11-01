"""Caveman's banner trail: a nyan ribbon (the ratio trigger) whose width follows the board's sideways axis at the
moment each segment was laid, so the rainbow twists with flips and rolls like a trailing banner (knowledge/banner-trail.md).
Three patched programs, every one gated so other ribbons are unchanged:

  spawn  shaders/bytecode/4a6fd8f8-3248-b91e-c135-56f2d0d7c93d (cas_main_sharedbundle; compute): a nyan emitter
         writes the board's right axis (column 0 of the emitter transform) into the particle's +36 float3, which
         otherwise holds the initial velocity
  sim    shaders/bytecode/ebf5a811-9430-2052-f916-e1cbbbd3ba76 (cas_main_sharedbundle; compute): a nyan emitter keeps
         +36 (the axis) and the particle's position as they are (no drift: the stored axis is not a velocity)
  vs     shaders/bytecode/6cfc6613-53b8-5466-0a0f-1c718a455601 (globals.toc configuration bundle; vertex), applied on
         top of nyanvspatch's flat patch: a nyan ribbon's width direction is the stored axis of this or the next
         particle, made perpendicular to the trail; the flat direction when no usable axis is stored

The nyan test in the compute programs reads the emitter's colour parameter (EmitterProperties offset 532: red, green;
the ribbon graph's PropertyId -1752990270 at DataOffset 25); the vertex program keeps nyanvspatch's particle-colour test.
Usage: nyanbannerpatch.py <game dir> <out dir>   (writes the three patched, signed programs)"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dxc  # noqa: E402
import nyanpatch  # noqa: E402
import nyanvspatch  # noqa: E402
from shaderpatch import CONFIG_BUNDLE, GLOBALS_TOC, f32, wrap  # noqa: E402

SHARED_BUNDLE = 'win32/characters/customization/configs/cas_main_sharedbundle'   # in items.toc
SPAWN_PROGRAM = '4a6fd8f8-3248-b91e-c135-56f2d0d7c93d'
SIM_PROGRAM = 'ebf5a811-9430-2052-f916-e1cbbbd3ba76'
COLOUR_OFFSET = 532       # EmitterProperties byte offset of the ribbon colour (-1752990270): red, green, blue
AXIS_OFFSET = 36          # the particle's float3 the banner axis rides in (the stock initial / current velocity)
MIN_AXIS = 0.25           # squared length below which the vertex program falls back to the flat direction


def _one(pattern, ir, what):
    found = re.findall(pattern, ir, re.M)
    assert len(found) == 1, f'{what}: {len(found)} matches'
    return found[0]


def _gate(c, prefix, red, green):
    """Instructions setting %<prefix>on to the nyan ratio test on (red, green), as nyanpatch's pixel test."""
    c.append(f'  %{prefix}gp = fcmp ogt float {green}, {f32(0.0001)}')
    c.append(f'  %{prefix}rg = fmul float {green}, {f32(nyanpatch.RATIO)}')
    c.append(f'  %{prefix}rd = fsub float {red}, %{prefix}rg')
    c.append(f'  %{prefix}ra = call float @dx.op.unary.f32(i32 6, float %{prefix}rd)')   # FAbs
    c.append(f'  %{prefix}tl = fmul float {green}, {f32(nyanpatch.RATIO_TOL)}')
    c.append(f'  %{prefix}ok = fcmp olt float %{prefix}ra, %{prefix}tl')
    c.append(f'  %{prefix}on = and i1 %{prefix}gp, %{prefix}ok')


DECLARATIONS = {   # the DXIL operations the patches use that a small program may not declare yet (all readnone)
    '@dx.op.unary.f32': 'declare float @dx.op.unary.f32(i32, float)',
    '@dx.op.dot3.f32': 'declare float @dx.op.dot3.f32(i32, float, float, float, float, float, float)',
}


def _declare(ir):
    """Declares the operations the patch uses but the program did not, with the program's readnone attribute group."""
    group = re.search(r'^attributes (#\d+) = \{ nounwind readnone \}$', ir, re.M).group(1)
    for name, decl in DECLARATIONS.items():
        if name in ir and f'declare float {name}(' not in ir:
            ir = ir.replace('\nattributes #', f'\n{decl} {group}\n\nattributes #', 1)
    return ir


def _set_valver(ir):
    valver = re.search(r'!dx\.valver = !\{!(\d+)\}', ir).group(1)
    return re.sub(rf'^!{valver} = !\{{i32 1, i32 \d+\}}$', f'!{valver} = !{{i32 1, i32 7}}', ir, flags=re.M)


def patch_spawn(ir):
    """The spawn program: a nyan emitter stores its right axis (normalized) at the particle's +36."""
    props = _one(r'^\s*(%\d+) = call %dx\.types\.Handle @dx\.op\.createHandle\(i32 57, i8 0, i32 0, i32 4, i1 false\)', ir, 'EmitterProperties handle')
    rows = {}
    for res, index, offset in re.findall(rf'^\s*(%\d+) = call %dx\.types\.ResRet\.f32 @dx\.op\.bufferLoad\.f32\(i32 68, %dx\.types\.Handle {props}, i32 (%\d+), i32 (0|16|32)\)', ir, re.M):
        rows[int(offset)] = (res, index)
    assert sorted(rows) == [0, 16, 32], f'transform rows {sorted(rows)}'
    emitter = rows[0][1]
    line, uav, addr, v0, v1, v2 = _one(r'^(\s*call void @dx\.op\.bufferStore\.i32\(i32 69, %dx\.types\.Handle (%\d+), i32 (%\d+), i32 undef, i32 (%\d+), i32 (%\d+), i32 (%\d+), i32 undef, i8 7\).*)$', ir, 'the +36 store')
    c = [f'  %bns_c = call %dx.types.ResRet.f32 @dx.op.bufferLoad.f32(i32 68, %dx.types.Handle {props}, i32 {emitter}, i32 {COLOUR_OFFSET})',
         '  %bns_cr = extractvalue %dx.types.ResRet.f32 %bns_c, 0',
         '  %bns_cg = extractvalue %dx.types.ResRet.f32 %bns_c, 1']
    _gate(c, 'bns_', '%bns_cr', '%bns_cg')
    for k, off in enumerate((0, 16, 32)):
        c.append(f'  %bns_a{k} = extractvalue %dx.types.ResRet.f32 {rows[off][0]}, 0')        # column 0: the right axis
    c.append('  %bns_l2 = call float @dx.op.dot3.f32(i32 55, float %bns_a0, float %bns_a1, float %bns_a2, float %bns_a0, float %bns_a1, float %bns_a2)')
    c.append(f'  %bns_l2e = fadd float %bns_l2, {f32(1e-12)}')
    c.append('  %bns_il = call float @dx.op.unary.f32(i32 25, float %bns_l2e)')                 # Rsqrt
    for k, v in enumerate((v0, v1, v2)):
        c.append(f'  %bns_n{k} = fmul float %bns_a{k}, %bns_il')
        c.append(f'  %bns_i{k} = bitcast float %bns_n{k} to i32')
        c.append(f'  %bns_s{k} = select i1 %bns_on, i32 %bns_i{k}, i32 {v}')
    store = line.replace(f'i32 {v0}, i32 {v1}, i32 {v2}, i32 undef, i8 7', 'i32 %bns_s0, i32 %bns_s1, i32 %bns_s2, i32 undef, i8 7')
    out = ir.replace(line, '\n'.join(c) + '\n' + store, 1)
    return _set_valver(_declare(out))


def patch_sim(ir):
    """The sim program: a nyan emitter's particles keep their +36 (the axis) and their position."""
    props = _one(r'^\s*(%\d+) = call %dx\.types\.Handle @dx\.op\.createHandle\(i32 57, i8 0, i32 0, i32 4, i1 false\)', ir, 'EmitterProperties handle')
    emitter = _one(rf'^\s*%\d+ = call %dx\.types\.ResRet\.f32 @dx\.op\.bufferLoad\.f32\(i32 68, %dx\.types\.Handle {props}, i32 (%\d+), i32 0\)', ir, 'emitter index')
    pline, uav, paddr, p0, p1, p2, pw = _one(r'^(\s*call void @dx\.op\.bufferStore\.i32\(i32 69, %dx\.types\.Handle (%\d+), i32 (%\d+), i32 undef, i32 (%\d+), i32 (%\d+), i32 (%\d+), i32 (%\d+), i8 15\).*)$', ir, 'the position store')
    vline, _, vaddr, v0, v1, v2 = _one(r'^(\s*call void @dx\.op\.bufferStore\.i32\(i32 69, %dx\.types\.Handle (%\d+), i32 (%\d+), i32 undef, i32 (%\d+), i32 (%\d+), i32 (%\d+), i32 undef, i8 7\).*)$', ir, 'the +36 store')

    def loaded(addr):
        res = _one(rf'^\s*(%\d+) = call %dx\.types\.ResRet\.i32 @dx\.op\.bufferLoad\.i32\(i32 68, %dx\.types\.Handle {uav}, i32 {re.escape(addr)}, i32 undef\)', ir, f'the load at {addr}')
        comps = dict((int(k), n) for n, k in re.findall(rf'^\s*(%\d+) = extractvalue %dx\.types\.ResRet\.i32 {res}, (\d)', ir, re.M))
        return [comps[k] for k in range(3)]
    old_p, old_v = loaded(paddr), loaded(vaddr)
    c = [f'  %bnm_c = call %dx.types.ResRet.f32 @dx.op.bufferLoad.f32(i32 68, %dx.types.Handle {props}, i32 {emitter}, i32 {COLOUR_OFFSET})',
         '  %bnm_cr = extractvalue %dx.types.ResRet.f32 %bnm_c, 0',
         '  %bnm_cg = extractvalue %dx.types.ResRet.f32 %bnm_c, 1']
    _gate(c, 'bnm_', '%bnm_cr', '%bnm_cg')
    for k, (new, old) in enumerate(zip((p0, p1, p2), old_p)):
        c.append(f'  %bnm_p{k} = select i1 %bnm_on, i32 {old}, i32 {new}')
    pstore = pline.replace(f'i32 {p0}, i32 {p1}, i32 {p2}, i32 {pw}, i8 15', f'i32 %bnm_p0, i32 %bnm_p1, i32 %bnm_p2, i32 {pw}, i8 15')
    out = ir.replace(pline, '\n'.join(c) + '\n' + pstore, 1)
    keep = [f'  %bnm_v{k} = select i1 %bnm_on, i32 {old}, i32 {new}' for k, (new, old) in enumerate(zip((v0, v1, v2), old_v))]
    vstore = vline.replace(f'i32 {v0}, i32 {v1}, i32 {v2}, i32 undef, i8 7', 'i32 %bnm_v0, i32 %bnm_v1, i32 %bnm_v2, i32 undef, i8 7')
    out = out.replace(vline, '\n'.join(keep) + '\n' + vstore, 1)
    return _set_valver(_declare(out))


def patch_vs(flat_ir):
    """The vertex program, after nyanvspatch.patch: a nyan ribbon's width direction is the stored axis (this or the
    next particle's, as the stock code picks this or the next point), minus its component along the trail."""
    base = _one(r'^\s*(%\d+) = extractvalue %dx\.types\.ResRet\.i32 %\d+, 0\n\s*%\d+ = call %dx\.types\.ResRet\.i32 @dx\.op\.bufferLoad\.i32\(i32 68, %dx\.types\.Handle %\d+, i32 %\d+, i32 420\)', flat_ir, 'particle data base')
    handle = re.search(r'^\s*(%\d+) = call %dx\.types\.Handle @dx\.op\.createHandle\(i32 57, i8 0, i32 0, i32 0, i1 false\)', flat_ir, re.M).group(1)
    # particle positions (+0) are loaded at add(slot x stride, base) for this, the previous, the next and the one after
    addrs = re.findall(rf'^\s*(%\d+) = add i32 %\d+, {re.escape(base)}\n\s*%\d+ = call %dx\.types\.ResRet\.i32 @dx\.op\.bufferLoad\.i32\(i32 68, %dx\.types\.Handle {re.escape(handle)}, i32 \1, i32 undef\)', flat_ir, re.M)
    assert len(addrs) == 4, f'particle position loads: {addrs}'
    this_addr, _, next_addr, _ = addrs
    far = _one(r'^\s*%nyw_tx = select i1 (%\d+), float %\d+, float %\d+$', flat_ir, 'the far-end switch')
    tx = re.search(r'^\s*%nyw_tx = select i1 %\d+, float (%\d+), float (%\d+)$', flat_ir, re.M).groups()
    tz = re.search(r'^\s*%nyw_tz = select i1 %\d+, float (%\d+), float (%\d+)$', flat_ir, re.M).groups()

    def middle(x, z):   # the y component: the fadd defined between the x and z ones (consecutive in the stock code)
        nx, nz = int(x[1:]), int(z[1:])
        assert nz == nx + 2, f'tangent components {x}, {z}'
        return f'%{nx + 1}'
    ty = (middle(tx[0], tz[0]), middle(tx[1], tz[1]))
    anchor = _one(r'^(\s*%nyw_dz = select i1 %nyw_on, float %nyw_fz, float %\d+)$', flat_ir, 'the flat direction')
    c = []
    for tag, addr in (('t', this_addr), ('n', next_addr)):
        c.append(f'  %bnv_{tag}a = add i32 {addr}, {AXIS_OFFSET}')
        c.append(f'  %bnv_{tag}l = call %dx.types.ResRet.i32 @dx.op.bufferLoad.i32(i32 68, %dx.types.Handle {handle}, i32 %bnv_{tag}a, i32 undef)')
        for k in range(3):
            c.append(f'  %bnv_{tag}i{k} = extractvalue %dx.types.ResRet.i32 %bnv_{tag}l, {k}')
            c.append(f'  %bnv_{tag}f{k} = bitcast i32 %bnv_{tag}i{k} to float')
    for k in range(3):
        c.append(f'  %bnv_a{k} = select i1 {far}, float %bnv_nf{k}, float %bnv_tf{k}')
    c.append(f'  %bnv_t0 = select i1 {far}, float {tx[0]}, float {tx[1]}')
    c.append(f'  %bnv_t1 = select i1 {far}, float {ty[0]}, float {ty[1]}')
    c.append(f'  %bnv_t2 = select i1 {far}, float {tz[0]}, float {tz[1]}')
    # w = a - t (a . t) / (t . t): the axis made perpendicular to the trail
    c.append('  %bnv_at = call float @dx.op.dot3.f32(i32 55, float %bnv_a0, float %bnv_a1, float %bnv_a2, float %bnv_t0, float %bnv_t1, float %bnv_t2)')
    c.append('  %bnv_tt = call float @dx.op.dot3.f32(i32 55, float %bnv_t0, float %bnv_t1, float %bnv_t2, float %bnv_t0, float %bnv_t1, float %bnv_t2)')
    c.append(f'  %bnv_tte = fadd float %bnv_tt, {f32(1e-12)}')
    c.append('  %bnv_k = fdiv float %bnv_at, %bnv_tte')
    for k in range(3):
        c.append(f'  %bnv_kt{k} = fmul float %bnv_t{k}, %bnv_k')
        c.append(f'  %bnv_w{k} = fsub float %bnv_a{k}, %bnv_kt{k}')
    c.append('  %bnv_ww = call float @dx.op.dot3.f32(i32 55, float %bnv_w0, float %bnv_w1, float %bnv_w2, float %bnv_w0, float %bnv_w1, float %bnv_w2)')
    c.append(f'  %bnv_use0 = fcmp ogt float %bnv_ww, {f32(MIN_AXIS)}')
    c.append('  %bnv_use = and i1 %bnv_use0, %nyw_on')
    c.append(f'  %bnv_wwe = fadd float %bnv_ww, {f32(1e-12)}')
    c.append('  %bnv_il = call float @dx.op.unary.f32(i32 25, float %bnv_wwe)')                     # Rsqrt
    for k, axis in enumerate('xyz'):
        c.append(f'  %bnv_u{k} = fmul float %bnv_w{k}, %bnv_il')
        c.append(f'  %bnv_d{axis} = select i1 %bnv_use, float %bnv_u{k}, float %nyw_d{axis}')
    out = flat_ir.replace(anchor, anchor + '\n' + '\n'.join(c), 1)
    for axis in 'xyz':
        old = re.search(rf'fmul float (%\d+), %nyw_d{axis}$', out, re.M)
        assert old, f'the width multiplication by %nyw_d{axis}'
        out = re.sub(rf'fmul float (%\d+), %nyw_d{axis}$', rf'fmul float \1, %bnv_d{axis}', out, count=1, flags=re.M)
    return _declare(out)


def _build(d, original, patcher, name):
    ir = d.disassemble(dxc.read_bytecode_resource(original)['dxbc'])
    obj, errors, ok = d.assemble(patcher(ir))
    if not ok:
        raise RuntimeError(f'{name}: {errors}')
    payload, meta = wrap(original, obj)
    return payload, meta


def build(game_dir):
    """[(superbundle TOC, bundle, name, payload, meta)] of the three patched programs, from the player's own game."""
    import gamedata
    game = gamedata.Game(game_dir)
    d = dxc.Dxc(game_dir)
    out = []
    shared = game.bundle(SHARED_BUNDLE)
    m = shared['manifest']
    for gid, patcher in ((SPAWN_PROGRAM, patch_spawn), (SIM_PROGRAM, patch_sim)):
        name = f'shaders/bytecode/{gid}'
        i = next(i for i, r in enumerate(m['res']) if r['name'] == name)
        original = game.decode(shared['files'][1 + len(m['ebx']) + i])
        out.append(('items.toc', SHARED_BUNDLE, name, *_build(d, original, patcher, name)))
    config = game.bundle(CONFIG_BUNDLE, toc=GLOBALS_TOC)
    m = config['manifest']
    name = f'shaders/bytecode/{nyanvspatch.RIBBON_VS_PROGRAM}'
    i = next(i for i, r in enumerate(m['res']) if r['name'] == name)
    original = game.decode(config['files'][1 + len(m['ebx']) + i])
    out.append((GLOBALS_TOC, CONFIG_BUNDLE, name, *_build(d, original, lambda ir: patch_vs(nyanvspatch.patch(ir)), name)))
    return out


if __name__ == '__main__':
    os.makedirs(sys.argv[2], exist_ok=True)
    for toc, bundle, name, payload, meta in build(sys.argv[1]):
        open(os.path.join(sys.argv[2], name.split('/')[-1] + '.bin'), 'wb').write(payload)
        print(f'{toc} {bundle.split("/")[-1]} {name}: {len(payload)} bytes, meta {meta.hex()}, assembled, validated and signed')
