"""Read access to the installed game's data (the base layer): bundles of a superbundle TOC, and texture
resources decoded to Pillow images."""
import io
import os
import struct

from PIL import Image

import fb

PKG = 'configurations/layout/initialinstallpackage'
# Frostbite format ordinal -> (DXGI format Pillow decodes, bytes per 4x4 block)
DXGI = {54: (71, 8), 55: (71, 8), 66: (98, 16), 67: (98, 16), 60: (77, 16), 61: (77, 16)}


def dds(width, height, dxgi, data):
    flags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000
    header = struct.pack('<4sIIIIIII44x', b'DDS ', 124, flags, height, width, len(data), 0, 1)
    pf = struct.pack('<II4s20x', 32, 0x4, b'DX10')
    caps = struct.pack('<I16x', 0x1000)
    return header + pf + caps + struct.pack('<IIIII', dxgi, 3, 0, 1, 0) + data


class Game:
    def __init__(self, game_root):
        self.root = game_root
        self.data = os.path.join(game_root, 'Data')
        self._handles = {}
        self._tocs = {}

    def raw(self, f):
        path = os.path.join(self.data, 'Win32', PKG, f"cas_{f['archive']:02d}.cas")
        h = self._handles.get(path) or self._handles.setdefault(path, open(path, 'rb'))
        h.seek(f['offset'])
        return h.read(f['size'])

    def decode(self, f):
        return fb.decode_cas(self.raw(f), self.root)[0]

    def toc(self, name='items.toc'):
        if name not in self._tocs:
            t = fb.read_toc(os.path.join(self.data, 'Win32', name))
            t['chunk_by_guid'] = {c['guid']: c for c in t['chunks']}
            t['bundle_by_name'] = {b['name']: b for b in t['bundles']}
            self._tocs[name] = t
        return self._tocs[name]

    def bundle(self, name, toc='items.toc'):
        """{'name', 'region', 'files' (manifest first), 'manifest_raw', 'manifest'}"""
        b = self.toc(toc)['bundle_by_name'][name]
        inline, files = fb.read_bundle_region(b['region'])
        assert inline is None, 'expected the manifest in cas'
        raw = self.raw(files[0])
        return {'name': name, 'region': b['region'], 'files': files, 'manifest_raw': raw,
                'manifest': fb.read_binary_bundle(raw)}

    def find_bundle(self, suffix, toc='items.toc'):
        return next(n for n in self.toc(toc)['bundle_by_name'] if n.endswith(suffix))

    def texture(self, bundle, res_name, toc='items.toc'):
        """(header dict, resource index, full pixel bytes) of a texture resource in a bundle."""
        m = bundle['manifest']
        ri = next(i for i, r in enumerate(m['res']) if r['name'] == res_name)
        h = fb.texture_header(self.decode(bundle['files'][1 + len(m['ebx']) + ri]), m['res'][ri]['meta'])
        chunk = self.toc(toc)['chunk_by_guid'][h['chunk']]
        return h, ri, self.decode(chunk)

    def texture_image(self, bundle, res_name, toc='items.toc'):
        h, _, pixels = self.texture(bundle, res_name, toc)
        dxgi, unit = DXGI[h['format']]
        mip0 = max(1, (h['w'] + 3) // 4) * max(1, (h['h'] + 3) // 4) * unit
        return Image.open(io.BytesIO(dds(h['w'], h['h'], dxgi, pixels[:mip0]))).convert('RGBA')
