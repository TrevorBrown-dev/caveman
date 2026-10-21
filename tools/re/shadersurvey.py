"""Lists every shader program (`shaders/bytecode/<guid>`) in a bundle with its stage and output signature.
Usage: shadersurvey.py <toc relative to Data/Win32> <bundle name> [--json out.json]"""
import json
import os
import struct
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dxc  # noqa: E402
import gamedata  # noqa: E402

BYTECODE_TYPE = 0x0E2B02F7
COMPONENT = {1: 'u32', 2: 's32', 3: 'f32'}


def signature(dxbc, fourcc):
    """[(semantic, index, component type, register, mask)] of a DXBC signature part (ISG1 or OSG1)."""
    part = next((p for p in dxc.container_parts(dxbc) if p[0] == fourcc), None)
    if part is None:
        return []
    _, at, _ = part
    count, first = struct.unpack_from('<II', dxbc, at)
    out = []
    for i in range(count):
        e = at + first + 32 * i
        _, name_off, index, _, comp, reg = struct.unpack_from('<6I', dxbc, e)
        mask = dxbc[e + 24]
        name = dxbc[at + name_off:dxbc.index(b'\0', at + name_off)].decode('latin1')
        out.append((name, index, COMPONENT.get(comp, comp), reg, mask))
    return out


def programs(game, toc, bundle):
    """{guid: (stage, outputs, payload)} of the bundle's programs."""
    b = game.bundle(bundle, toc=toc)
    m = b['manifest']
    ne = len(m['ebx'])
    out = {}
    for i, r in enumerate(m['res']):
        if r['type'] != BYTECODE_TYPE:
            continue
        data = game.decode(b['files'][1 + ne + i])
        dxbc = dxc.read_bytecode_resource(data)['dxbc']
        if dxbc is None:
            continue
        try:
            stage = dxc.shader_kind(dxbc)[0]
        except StopIteration:
            stage = '?'
        out[r['name'].rsplit('/', 1)[1]] = (stage, signature(dxbc, 'OSG1'), data)
    return out


def main():
    args = sys.argv[1:]
    out_json = None
    if '--json' in args:
        i = args.index('--json'); out_json = args[i + 1]; del args[i:i + 2]
    game = gamedata.Game(dxc.GAME)
    progs = programs(game, args[0], args[1])
    shapes = Counter((stage, tuple((n, i, c) for n, i, c, _, _ in outs)) for stage, outs, _ in progs.values())
    print(f'{len(progs)} programs')
    for (stage, outs), n in shapes.most_common():
        print(f'{n:5d} {stage}: {" ".join(f"{s}{i}:{c}" for s, i, c in outs)}')
    if out_json:
        json.dump({g: [s, o] for g, (s, o, _) in progs.items()}, open(out_json, 'w'))


if __name__ == '__main__':
    main()
