"""Writes mip 0 of matching texture resources as PNG.
Usage: extract_texture.py <layer root> <game root> <toc relative to Win32> <bundle substring> <resource substring> <out dir>"""
import io
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fb  # noqa: E402
from PIL import Image  # noqa: E402

root, game, toc_rel, wanted_bundle, wanted_res, out_dir = sys.argv[1:7]
PKG = 'configurations/layout/initialinstallpackage'
# Frostbite ordinal -> (DXGI format, bytes per 4x4 block)
DXGI = {54: (71, 8), 55: (72, 8), 66: (98, 16), 67: (99, 16), 60: (77, 16), 61: (78, 16)}


def read_raw(f):
    path = os.path.join(root, 'Win32', PKG, f"cas_{f['archive']:02d}.cas")
    with open(path, 'rb') as h:
        h.seek(f['offset'])
        return h.read(f['size'])


def dds(width, height, dxgi, data):
    flags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000
    header = struct.pack('<4sIIIIIII44x', b'DDS ', 124, flags, height, width, len(data), 0, 1)
    pf = struct.pack('<II4s20x', 32, 0x4, b'DX10')
    caps = struct.pack('<I16x', 0x1000)
    dx10 = struct.pack('<IIIII', dxgi, 3, 0, 1, 0)
    return header + pf + caps + dx10 + data


class L(fb.Layer):
    def read_raw(self, f):
        return read_raw(f)


layer = L(root, game)
toc = fb.read_toc(os.path.join(root, 'Win32', toc_rel))
toc_chunks = {c['guid']: c for c in toc['chunks']}
os.makedirs(out_dir, exist_ok=True)
for b in toc['bundles']:
    if wanted_bundle not in b['name']:
        continue
    m, files = layer.bundle(b['region'])
    ne, nr = len(m['ebx']), len(m['res'])
    bundle_chunks = {c['guid']: (i, c) for i, c in enumerate(m['chunks'])}
    for i, r in enumerate(m['res']):
        if r['type'] != fb.TEXTURE_TYPE or wanted_res not in r['name']:
            continue
        h = fb.texture_header(fb.decode_cas(read_raw(files[ne + i]), game)[0], r['meta'])
        if h['chunk'] in toc_chunks:
            c = toc_chunks[h['chunk']]
            data = fb.decode_cas(read_raw(c), game)[0]
            source = 'toc chunk'
        else:
            j, c = bundle_chunks[h['chunk']]
            data = fb.decode_cas(read_raw(files[ne + nr + j]), game)[0]
            source = f"bundle chunk lo={c['logical_offset']:#x}"
        dxgi, unit = DXGI[h['format']]
        mip0 = max(1, (h['w'] + 3) // 4) * max(1, (h['h'] + 3) // 4) * unit
        print(f"{r['name']}: fmt={h['format']} {h['w']}x{h['h']} data={len(data)} (chunk_size {h['chunk_size']}) from {source}")
        if len(data) < mip0:
            print('   only the tail mips are here; skipped')
            continue
        img = Image.open(io.BytesIO(dds(h['w'], h['h'], dxgi, data[:mip0])))
        img.load()
        out = os.path.join(out_dir, r['name'].split('/')[-1][:60] + '.png')
        img.save(out)
        print('   ->', out, img.mode, img.size)
