"""Caveman's deck effects patched into the game's own deck pixel shaders (DXIL assembly), gated by a marker in the deck art.
Usage: shaderpatch.py <game dir> <out dir>   (writes the patched, signed programs as shaders/bytecode resources + a report)

The stock deck material (Deck_Mat of the popsicle mesh, key 0x282d2d3b1e1f71d1) draws with six pixel programs in
globals.toc's win32/configurations/gameconfigurations/delmargameconfiguration bundle (DECK_PROGRAMS). Each is
disassembled with the game's dxcompiler.dll, given Caveman's code just before its G-buffer stores, set to validator
1.7 (what the game's dxil.dll knows) and assembled, validated and signed again (dxc.py). Every deck still renders
exactly as before unless its art carries the marker; see knowledge/shaders.md."""
import os
import re
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dxc  # noqa: E402

GLOBALS_TOC = 'globals.toc'
CONFIG_BUNDLE = 'win32/configurations/gameconfigurations/delmargameconfiguration'
DECK_PROGRAMS = ['2a8dba87-9d32-cb86-fb2c-f3b9ee93f6f8', 'c125a7ae-b71c-5a21-3c8a-a62e86e68fe7',
                 '1b6ae129-33e2-1ee7-bc84-d45555a76771', '4daed1c1-2a79-452d-548c-6f019ec3b59d',
                 'a54c586c-3f98-f214-ab2b-32390d262d24', 'a82b7e84-ed0f-54b3-eca0-250126b2aeca']
BYTECODE_TYPE = 0x0E2B02F7
CLOCK_REGISTER = 262   # PerPassConstants (b4) register whose x is the game clock in seconds

# The marker: three 16 x 16 solid blocks in the top-left corner (512 x 2048 texels), far off the visible deck, of
# the deck's HOLO MAP (param_graphic_rgb; linear BC7). The texture the programs sample as t0 (t1 in variant B) is
# the holo map, not the art: the stock code uses only its red channel, as the holo mask (checked in the IR,
# 2026-10-20, after a first test with the marker in the art showed nothing). The art reaches the programs
# composited with the stickers (the board compositor's Composite C).
ART_W, ART_H, BLOCK = 512, 2048, 16
MARK_A = (255, 0, 255)    # block 0: magenta
MARK_B = (0, 255, 0)      # block 1: green
# block 2: parameters. R: effect (below), G: speed (1.0 = 2 turns a second), B: glow strength, exponential:
# the emissive is the colour x (2^(12 B) - 1), so B 64 is x7, 128 x63, 192 x511, 255 x4095 (the scale the game's
# emissive buffer expects is not known yet: a first try at x2 and x4 showed nothing)
EFFECT_NONE, EFFECT_CYCLE, EFFECT_STATIC, EFFECT_PULSE, EFFECT_NEON = 0, 64, 128, 192, 240
# none (glow only); hue cycle; a fixed half turn of the hues (diagnostic: needs no clock); brightness pulsing
# every 2 seconds (diagnostic: shows the clock runs); neon: the emissive alone, unlit (diagnostic: the
# emissive buffer is read at all)


def marker_pixels(effect=EFFECT_CYCLE, speed=32, glow=0):
    """[(x0, y0, x1, y1, (r, g, b))] rectangles to paint into a deck's 512 x 2048 holo map (linear values)."""
    return [(0, 0, BLOCK, BLOCK, MARK_A), (BLOCK, 0, 2 * BLOCK, BLOCK, MARK_B),
            (2 * BLOCK, 0, 3 * BLOCK, BLOCK, (effect, speed, glow))]


def f32(x):
    """An LLVM IR float literal (a double holding the exact float value)."""
    return f"0x{struct.unpack('<Q', struct.pack('<d', struct.unpack('<f', struct.pack('<f', x))[0]))[0]:016X}"


def _handles(ir):
    out = {}
    for m in re.finditer(r'(%[\w.]+) = call %dx\.types\.Handle @dx\.op\.createHandle\(i32 57, i8 (\d), i32 \d+, i32 (\d+), i1 false\)', ir):
        out[('t', 'u', 'cb', 's')[int(m.group(2))] + m.group(3)] = m.group(1)
    return out


def _art_texture(ir, handles):
    """The holo map's register (where the marker is): the lowest Texture2D below t32 (t0, or t1 where t0 is a
    structured buffer)."""
    regs = []
    for m in re.finditer(r'^;\s+texture\s+(\S+)\s+(\S+)\s+\S+\s+t(\d+)\s+1$', ir, re.M):
        if m.group(1) == 'f32' and m.group(2) == '2d' and int(m.group(3)) < 32:
            regs.append(int(m.group(3)))
    return f't{min(regs)}'


def patch(ir):
    """The disassembly of a deck program with Caveman's hue cycle (and experimental glow) added. Returns (ir, notes)."""
    h = _handles(ir)
    art, samp, clock = h[_art_texture(ir, h)], h['s0'], h['cb4']
    stores = {}
    for m in re.finditer(r'call void @dx\.op\.storeOutput\.(f32|i32)\(i32 5, i32 (\d), i32 0, i8 (\d), (?:float|i32) ([^)]+)\)', ir):
        stores[(int(m.group(2)), int(m.group(3)))] = (m.group(1), m.group(4), m.start())
    first_store = min(s[2] for s in stores.values())
    line_start = ir.rfind('\n', 0, first_store) + 1
    base = [stores[(1, c)][1] for c in range(3)]
    glow_target = all(stores.get((3, c), ('i32',))[0] == 'f32' for c in range(3))
    code = []

    def sample(name, bx):
        u, v = (bx * BLOCK + BLOCK / 2) / ART_W, (BLOCK / 2) / ART_H
        code.append(f'  %{name} = call %dx.types.ResRet.f32 @dx.op.sampleLevel.f32(i32 62, %dx.types.Handle {art}, '
                    f'%dx.types.Handle {samp}, float {f32(u)}, float {f32(v)}, float undef, float undef, i32 0, i32 0, '
                    f'i32 undef, float 0.000000e+00)')
        for c, ch in enumerate('rgb'):
            code.append(f'  %{name}_{ch} = extractvalue %dx.types.ResRet.f32 %{name}, {c}')

    sample('cvm_a', 0)
    sample('cvm_b', 1)
    sample('cvm_c', 2)
    hi, lo = f32(0.9), f32(0.1)
    tests = [('cvm_a_r', 'ogt', hi), ('cvm_a_g', 'olt', lo), ('cvm_a_b', 'ogt', hi),
             ('cvm_b_r', 'olt', lo), ('cvm_b_g', 'ogt', hi), ('cvm_b_b', 'olt', lo)]
    for i, (v, op, k) in enumerate(tests):
        code.append(f'  %cvm_t{i} = fcmp {op} float %{v}, {k}')
    code.append('  %cvm_m0 = and i1 %cvm_t0, %cvm_t1')
    for i in range(2, 5):
        code.append(f'  %cvm_m{i - 1} = and i1 %cvm_m{i - 2}, %cvm_t{i}')
    code.append('  %cvm_on = and i1 %cvm_m3, %cvm_t5')
    # time and angle
    code.append(f'  %cvm_clock = call %dx.types.CBufRet.f32 @dx.op.cbufferLoadLegacy.f32(i32 59, %dx.types.Handle {clock}, i32 {CLOCK_REGISTER})')
    code.append('  %cvm_t = extractvalue %dx.types.CBufRet.f32 %cvm_clock, 0')
    code.append(f'  %cvm_speed = fmul float %cvm_c_g, {f32(2.0)}')
    code.append('  %cvm_turns = fmul float %cvm_t, %cvm_speed')
    code.append('  %cvm_frac = call float @dx.op.unary.f32(i32 22, float %cvm_turns)')
    # the effect (block 2 R): none < 0.125 <= hue cycle < 0.375 <= fixed half turn < 0.625 <= pulse < 0.875 <= neon
    code.append(f'  %cvm_is_cyc = fcmp oge float %cvm_c_r, {f32(0.125)}')
    code.append(f'  %cvm_is_sta = fcmp oge float %cvm_c_r, {f32(0.375)}')
    code.append(f'  %cvm_is_pul0 = fcmp oge float %cvm_c_r, {f32(0.625)}')
    code.append(f'  %cvm_is_neo = fcmp oge float %cvm_c_r, {f32(0.875)}')
    code.append(f'  %cvm_not_neo = fcmp olt float %cvm_c_r, {f32(0.875)}')
    code.append('  %cvm_is_pul = and i1 %cvm_is_pul0, %cvm_not_neo')
    code.append('  %cvm_neon = and i1 %cvm_on, %cvm_is_neo')
    code.append(f'  %cvm_f0 = select i1 %cvm_is_cyc, float %cvm_frac, float {f32(0.0)}')
    code.append(f'  %cvm_f1 = select i1 %cvm_is_sta, float {f32(0.5)}, float %cvm_f0')
    code.append(f'  %cvm_f2 = select i1 %cvm_is_pul0, float {f32(0.0)}, float %cvm_f1')
    code.append(f'  %cvm_ang = fmul float %cvm_f2, {f32(6.2831855)}')
    # pulse: brightness 0.1 to 1 over 2 seconds of the clock
    code.append(f'  %cvm_pt = fmul float %cvm_t, {f32(0.5)}')
    code.append('  %cvm_pf = call float @dx.op.unary.f32(i32 22, float %cvm_pt)')
    code.append(f'  %cvm_pk = call float @dx.op.tertiary.f32(i32 46, float %cvm_pf, float {f32(0.9)}, float {f32(0.1)})')
    code.append(f'  %cvm_bright = select i1 %cvm_is_pul, float %cvm_pk, float {f32(1.0)}')
    code.append('  %cvm_cos = call float @dx.op.unary.f32(i32 12, float %cvm_ang)')
    code.append('  %cvm_sin = call float @dx.op.unary.f32(i32 13, float %cvm_ang)')
    # rotate the colour about the grey axis: c cos + (k x c) sin + k (k . c)(1 - cos), k = (1, 1, 1) / sqrt 3
    r, g, b = base
    code.append(f'  %cvm_s0 = fadd float {r}, {g}')
    code.append(f'  %cvm_s1 = fadd float %cvm_s0, {b}')
    code.append(f'  %cvm_mean = fmul float %cvm_s1, {f32(1 / 3)}')
    code.append(f'  %cvm_omc = fsub float {f32(1.0)}, %cvm_cos')
    code.append('  %cvm_grey = fmul float %cvm_mean, %cvm_omc')
    code.append(f'  %cvm_ks = fmul float %cvm_sin, {f32(0.57735026)}')
    for ch, (x, d0, d1) in zip('rgb', [(r, b, g), (g, r, b), (b, g, r)]):
        code.append(f'  %cvm_{ch}c = fmul float {x}, %cvm_cos')
        code.append(f'  %cvm_{ch}d = fsub float {d0}, {d1}')
        code.append(f'  %cvm_{ch}x = fmul float %cvm_{ch}d, %cvm_ks')
        code.append(f'  %cvm_{ch}y = fadd float %cvm_{ch}c, %cvm_{ch}x')
        code.append(f'  %cvm_{ch}z = fadd float %cvm_{ch}y, %cvm_grey')
        code.append(f'  %cvm_{ch}s = call float @dx.op.unary.f32(i32 7, float %cvm_{ch}z)')
        code.append(f'  %cvm_{ch}p = fmul float %cvm_{ch}s, %cvm_bright')
        code.append(f'  %cvm_out_{ch} = select i1 %cvm_on, float %cvm_{ch}p, float {x}')
    # glow: render target 3 is the emissive buffer (knowledge/glow.md). Layout A stores it as float; layout B as
    # half floats in a uint4 whose alpha is a code (kept: with the deck's codes the emissive adds to the lit colour)
    glow_half = not glow_target and all(stores.get((3, c), ('f32',))[0] == 'i32' for c in range(3))
    if glow_target or glow_half:
        # (names: %cvm_<channel><step> belong to the colour rotation above, so these say "glow")
        code.append(f'  %cvm_glow_x = fmul float %cvm_c_b, {f32(12.0)}')
        code.append('  %cvm_glow_e = call float @dx.op.unary.f32(i32 21, float %cvm_glow_x)')   # Exp: 2^x
        code.append(f'  %cvm_glow = fsub float %cvm_glow_e, {f32(1.0)}')
        for c, ch in enumerate('rgb'):
            code.append(f'  %cvm_{ch}e = fmul float %cvm_out_{ch}, %cvm_glow')
            if glow_target:
                code.append(f'  %cvm_{ch}g = select i1 %cvm_on, float %cvm_{ch}e, float {stores[(3, c)][1]}')
            else:
                code.append(f'  %cvm_{ch}h = call i32 @dx.op.legacyF32ToF16(i32 130, float %cvm_{ch}e)')
    out = ir[:line_start] + '\n'.join(code) + '\n' + ir[line_start:]
    for c, ch in enumerate('rgb'):
        old = f'storeOutput.f32(i32 5, i32 1, i32 0, i8 {c}, float {base[c]})'
        assert out.count(old) == 1, old
        out = out.replace(old, f'storeOutput.f32(i32 5, i32 1, i32 0, i8 {c}, float %cvm_out_{ch})')
        if glow_target:
            old = f'storeOutput.f32(i32 5, i32 3, i32 0, i8 {c}, float {stores[(3, c)][1]})'
            assert out.count(old) == 1, old
            out = out.replace(old, f'storeOutput.f32(i32 5, i32 3, i32 0, i8 {c}, float %cvm_{ch}g)')
        elif glow_half:
            # the stock half-float operand is computed after Caveman's block: select right before the store
            operand = stores[(3, c)][1]
            store = re.compile(rf'^(\s*)call void @dx\.op\.storeOutput\.i32\(i32 5, i32 3, i32 0, i8 {c}, i32 {re.escape(operand)}\)', re.M)
            assert len(store.findall(out)) == 1, f'RT3 store {c}'
            out = store.sub(lambda m: f'{m.group(1)}%cvm_{ch}g = select i1 %cvm_on, i32 %cvm_{ch}h, i32 {operand}\n'
                                      f'{m.group(1)}call void @dx.op.storeOutput.i32(i32 5, i32 3, i32 0, i8 {c}, i32 %cvm_{ch}g)', out)
    if glow_half:
        assert 'declare i32 @dx.op.legacyF32ToF16(' in out, 'legacyF32ToF16 is not declared in this program'
    # neon: the emissive alone, unlit. Layout B: the alpha code's high byte (the lighting weight) to 0; layout A: alpha
    # (inferred to be the lighting weight) to 0. Selected right before the store (the code is computed late).
    if glow_target or glow_half:
        kind, arg = ('f32', 'float') if glow_target else ('i32', 'i32')
        operand = stores[(3, 3)][1]
        store = re.compile(rf'^(\s*)call void @dx\.op\.storeOutput\.{kind}\(i32 5, i32 3, i32 0, i8 3, {arg} {re.escape(operand)}\)', re.M)
        assert len(store.findall(out)) == 1, 'RT3 alpha store'
        if glow_target:
            neon = f'%cvm_na = select i1 %cvm_neon, float {f32(0.0)}, float {operand}'
        else:
            neon = f'%cvm_nm = and i32 {operand}, 255\n{{0}}%cvm_na = select i1 %cvm_neon, i32 %cvm_nm, i32 {operand}'
        out = store.sub(lambda m: f'{m.group(1)}{neon.format(m.group(1))}\n'
                                  f'{m.group(1)}call void @dx.op.storeOutput.{kind}(i32 5, i32 3, i32 0, i8 3, {arg} %cvm_na)', out)
    if 'declare %dx.types.ResRet.f32 @dx.op.sampleLevel.f32(' not in out:
        out = out.replace('\nattributes #0', '\ndeclare %dx.types.ResRet.f32 @dx.op.sampleLevel.f32(i32, %dx.types.Handle, '
                          '%dx.types.Handle, float, float, float, float, i32, i32, i32, float) #2\n\nattributes #0', 1)
    # the game's dxil.dll validates up to 1.7; the shipped programs say 1.10 (dxcoob 1.8.2505)
    valver = re.search(r'!dx\.valver = !\{!(\d+)\}', out).group(1)
    out = re.sub(rf'^!{valver} = !\{{i32 1, i32 \d+\}}$', f'!{valver} = !{{i32 1, i32 7}}', out, flags=re.M)
    notes = f'holo map {_art_texture(ir, h)}, base colour {base}, glow {"RT3 float" if glow_target else "RT3 half" if glow_half else "none"}'
    return out, notes


def wrap(original, dxbc):
    """A shaders/bytecode resource like `original` (header, RC2S block, DXBC) holding `dxbc`. Returns (payload, meta)."""
    at = original.find(b'DXBC')
    pre = bytearray(original[0x20:at])
    struct.pack_into('<I', pre, len(pre) - 4, len(dxbc))
    body = bytes(pre) + dxbc
    end = 0x20 + len(body)
    data_size = (end + 15) // 16 * 16   # the program data is padded to 16 bytes (checked on all six game programs)
    head = bytearray(original[:0x20])
    struct.pack_into('<I', head, 12, len(body))
    struct.pack_into('<QQ', head, 0x10, data_size, 0x20)
    payload = bytes(head) + body + bytes(data_size - end) + struct.pack('<II', 0x10, 0x18)
    return payload, struct.pack('<IIII', data_size, 8, 0, 0)


def build(read_program, game_dir):
    """{name: (payload, meta, notes)} for every deck program; read_program(name) gives the game's payload."""
    d = dxc.Dxc(game_dir)
    out = {}
    for guid in DECK_PROGRAMS:
        name = f'shaders/bytecode/{guid}'
        original = read_program(name)
        ir = d.disassemble(dxc.read_bytecode_resource(original)['dxbc'])
        patched, notes = patch(ir)
        obj, errors, ok = d.assemble(patched)
        if not ok:
            raise RuntimeError(f'{name}: {errors}')
        payload, meta = wrap(original, obj)
        out[name] = (payload, meta, notes)
    return out


def main():
    import gamedata
    game_dir, out_dir = sys.argv[1], sys.argv[2]
    game = gamedata.Game(game_dir)
    b = game.bundle(CONFIG_BUNDLE, toc=GLOBALS_TOC)
    m = b['manifest']
    ne = len(m['ebx'])
    index = {r['name']: i for i, r in enumerate(m['res'])}

    def read_program(name):
        return game.decode(b['files'][1 + ne + index[name]])
    os.makedirs(out_dir, exist_ok=True)
    for name, (payload, meta, notes) in build(read_program, game_dir).items():
        path = os.path.join(out_dir, name.split('/')[-1] + '.bin')
        open(path, 'wb').write(payload)
        print(f'{name}: {len(payload)} bytes, meta {meta.hex()}, {notes}')


if __name__ == '__main__':
    main()
