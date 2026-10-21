"""Texture headers in one bundle of the base game, or of a mod. Usage: base_textures.py <layer root> <game root> <toc relative to Win32> <bundle name substring> [name filter]"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fb  # noqa: E402

root, game, toc_rel, wanted = sys.argv[1:5]
name_filter = sys.argv[5] if len(sys.argv) > 5 else ''
PKG = 'configurations/layout/initialinstallpackage'


class BaseLayer(fb.Layer):
    def read_raw(self, f):
        path = os.path.join(self.root, 'Win32', PKG, f"cas_{f['archive']:02d}.cas")
        with open(path, 'rb') as h:
            h.seek(f['offset'])
            return h.read(f['size'])


layer = BaseLayer(root, game)
toc = fb.read_toc(os.path.join(root, 'Win32', toc_rel))
for b in toc['bundles']:
    if wanted not in b['name']:
        continue
    print('BUNDLE', b['name'], 'load', b['load'])
    m, files = layer.bundle(b['region'])
    ne, nr = len(m['ebx']), len(m['res'])
    chunk_by_guid = {c['guid']: c for c in m['chunks']}
    for i, r in enumerate(m['res']):
        if r['type'] != fb.TEXTURE_TYPE or name_filter not in r['name']:
            continue
        h = fb.texture_header(fb.decode_cas(layer.read_raw(files[ne + i]), game)[0], r['meta'])
        c = chunk_by_guid.get(h['chunk'])
        cl = f"bundle chunk lo={c['logical_offset']:#x} ls={c['logical_size']}" if c else 'chunk not in bundle'
        print(f"  {r['name']}: fmt={h['format']} {h['w']}x{h['h']} mips={h['mips']} first={h['first_mip']} flags={h['flags']} chunk_size={h['chunk_size']} | {cl}")
