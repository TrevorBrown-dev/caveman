"""Reads ExpressionShaderDatabases (`*_win32_shaderstate_db`): material key -> records -> permutation ids, and a
permutation resource -> its program GUID (format in knowledge/shaders.md).
Usage: shaderdb.py <toc relative to Data/Win32> <bundle name> <material key (decimal or 0x hex)>"""
import os
import struct
import sys
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fb  # noqa: E402
import gamedata  # noqa: E402

DB_TYPE = 0xE2E6955B
PERMUTATION_TYPE = 0xD489FACB
RECORD = 0xA8


def read_db(data, meta):
    """(records, lookup): records [{'hash', 'permutations': [u64]}], lookup {material key: [record index]}."""
    prog_size, reloc_size, lookup_size, _ = struct.unpack_from('<4I', meta)
    count, first = struct.unpack_from('<IQ', data, 0)
    records = []
    for i in range(count):
        at = first + RECORD * i
        n, ptr = struct.unpack_from('<IQ', data, at + 0x60)
        perms = list(struct.unpack_from(f'<{n}Q', data, ptr)) if n else []
        records.append({'hash': struct.unpack_from('<Q', data, at)[0], 'permutations': perms})
    at = prog_size + reloc_size
    keys = struct.unpack_from('<I', data, at)[0]
    at += 4
    lookup = {}
    for _ in range(keys):
        key, n = struct.unpack_from('<QI', data, at)
        lookup[key] = list(struct.unpack_from(f'<{n}I', data, at + 12))
        at += 12 + 4 * n
    return records, lookup


def program_guid(permutation):
    """The program GUID (text, as in `shaders/bytecode/<guid>`) a permutation resource names (+0x30)."""
    return str(uuid.UUID(bytes_le=bytes(permutation[0x30:0x40])))


def bundle_resources(game, toc, bundle):
    """{name: (type, meta, decoded bytes loader)} of a bundle's resources."""
    b = game.bundle(bundle, toc=toc)
    m = b['manifest']
    out = {}
    for i, r in enumerate(m['res']):
        f = b['files'][1 + len(m['ebx']) + i]
        out[r['name']] = (r['type'], r['meta'], (lambda f=f: game.decode(f)))
    return out


def main():
    toc, bundle, key = sys.argv[1], sys.argv[2], int(sys.argv[3], 0)
    game = gamedata.Game(r'E:\Games\ReSkate')
    res = bundle_resources(game, toc, bundle)
    for name, (rtype, meta, load) in res.items():
        if rtype != DB_TYPE:
            continue
        records, lookup = read_db(load(), meta)
        print(f'{name}: {len(records)} records, {len(lookup)} keys')
        for k in ([key] if key in lookup else []):
            for ri in lookup[k]:
                print(f'  record {ri}: {len(records[ri]["permutations"])} permutations')
                for p in records[ri]['permutations']:
                    pname = f'expressionshader/permutation{p}'
                    if pname in res:
                        print(f'    {pname} -> shaders/bytecode/{program_guid(res[pname][2]())}')
                    else:
                        print(f'    {pname} (not in this bundle)')


if __name__ == '__main__':
    main()
