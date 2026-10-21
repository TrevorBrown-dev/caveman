"""Decodes every stock deck graphic colour texture (<deck>_c) the base game ships.
Usage: stock_decks.py <game root> <out dir>   -> out/<name>.png plus a summary of formats and sizes"""
import collections
import io
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fb  # noqa: E402
from PIL import Image  # noqa: E402
from extract_texture_lib import dds, DXGI  # noqa: E402

game, out_dir = sys.argv[1], sys.argv[2]
root = os.path.join(game, 'Data')
PKG = 'configurations/layout/initialinstallpackage'


def read_raw(f):
    with open(os.path.join(root, 'Win32', PKG, f"cas_{f['archive']:02d}.cas"), 'rb') as h:
        h.seek(f['offset'])
        return h.read(f['size'])


class L(fb.Layer):
    def read_raw(self, f):
        return read_raw(f)


layer = L(root, game)
toc = fb.read_toc(os.path.join(root, 'Win32', 'items.toc'))
toc_chunks = {c['guid']: c for c in toc['chunks']}
os.makedirs(out_dir, exist_ok=True)
summary = collections.Counter()
for b in toc['bundles']:
    if '/deck/' not in b['name'] or '/graphic/' not in b['name']:
        continue
    m, files = layer.bundle(b['region'])
    ne = len(m['ebx'])
    for i, r in enumerate(m['res']):
        leaf = r['name'].split('/')[-1]
        if r['type'] != fb.TEXTURE_TYPE or 'graphic' not in leaf or not leaf.endswith('_c'):
            continue
        h = fb.texture_header(fb.decode_cas(read_raw(files[ne + i]), game)[0], r['meta'])
        summary[(h['format'], h['w'], h['h'], h['mips'], h['first_mip'])] += 1
        c = toc_chunks.get(h['chunk'])
        if not c:
            continue
        data = fb.decode_cas(read_raw(c), game)[0]
        dxgi, unit = DXGI[h['format']]
        mip0 = max(1, (h['w'] + 3) // 4) * max(1, (h['h'] + 3) // 4) * unit
        img = Image.open(io.BytesIO(dds(h['w'], h['h'], dxgi, data[:mip0])))
        img.convert('RGB').save(os.path.join(out_dir, leaf + '.png'))
print(dict(summary))
