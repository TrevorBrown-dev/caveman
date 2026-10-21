"""Builds a ReSkate mod (a Frostbite patch layer) that changes assets of the game's bundles."""
import datetime
import hashlib
import json
import os
import shutil

import fb
import fbw
from gamedata import PKG

LOC = dict(patch=True, install=fbw.MOD_INSTALL_CHUNK, archive=fbw.MOD_ARCHIVE)


def owner_hash(name):
    """The chunk-metadata h64 of a resource: 5381, then h * 33 ^ lowercase char (64-bit)."""
    h = 5381
    for ch in name:
        h = ((h * 33) & 0xFFFFFFFFFFFFFFFF) ^ ord(ch.lower())
    return h


class Patch:
    def __init__(self, game):
        self.game = game
        self.cas = bytearray()
        self.bundles = {}   # name -> {'files', 'manifest_raw', 'manifest', 'region'}
        self.chunks = {}    # guid -> TOC chunk entry

    def _append(self, data):
        at = len(self.cas)
        self.cas += data
        return at

    def bundle(self, name):
        """The bundle as this patch will ship it (a copy of the game's, edited in place)."""
        if name not in self.bundles:
            b = self.game.bundle(name)
            b['files'] = [dict(f) for f in b['files']]
            b['manifest_raw'] = bytearray(b['manifest_raw'])
            self.bundles[name] = b
        return self.bundles[name]

    def _set_sha1(self, b, index, digest):
        b['manifest_raw'][36 + 20 * index: 56 + 20 * index] = digest

    def replace_texture_pixels(self, bundle_name, res_name, pixels):
        """New pixel data (all mips, the game's exact format and size) for a texture in a bundle.
        The TOC chunk always moves to the new data. When the bundle also lists the chunk (a texture
        with mips resident in the bundle), the bundle is shipped too, pointing at the new data;
        otherwise (menu thumbnails) the bundle stays the game's and is left out of the patch."""
        orig = self._game_bundle(bundle_name)   # read the game's own entries, never the patched copy
        m = orig['manifest']
        ne, nr = len(m['ebx']), len(m['res'])
        h, ri, old = self.game.texture(orig, res_name)
        assert len(pixels) == len(old) == h['chunk_size'], (res_name, len(pixels), len(old))
        # Compressed like the game's own: all-stored blocks crashed the game when it streamed them
        # (encoded larger than decoded), with the board equipped at spawn.
        enc = fbw.encode_cas(pixels, self.game.root)
        at = self._append(enc)
        self.chunks[h['chunk']] = dict(LOC, guid=h['chunk'], offset=at, size=len(enc))
        ci = next((j for j, c in enumerate(m['chunks']) if c['guid'] == h['chunk']), None)
        if ci is not None:
            b = self.bundle(bundle_name)
            lo = m['chunks'][ci]['logical_offset']
            tail = fbw.block_offset(enc, lo)  # the bundle holds the resident tail (all of it when lo is 0)
            b['files'][1 + ne + nr + ci] = dict(LOC, offset=at + tail, size=len(enc) - tail)
            # A chunk's manifest SHA-1 is of its whole encoded data, resident from 0 or not.
            self._set_sha1(b, ne + nr + ci, hashlib.sha1(enc).digest())
        if h['mips'] > 1:
            # The header's first 8 bytes are where mips 1 and 2 start in the *encoded* chunk (the
            # block offsets). Left as the game's, they point into the game's own compressed data: the
            # cause of the crash and the hang when the board loaded at spawn (2026-10-20).
            import struct
            header = self._header(bundle_name, ri)
            mip1 = h['mip_sizes'][0]
            mip2 = mip1 + h['mip_sizes'][1]
            struct.pack_into('<II', header, 0, fbw.block_offset(enc, mip1), fbw.block_offset(enc, mip2))
            self._write_header(bundle_name, ri, header)
        return h

    def _game_bundle(self, name):
        if not hasattr(self, '_orig'):
            self._orig = {}
        if name not in self._orig:
            self._orig[name] = self.game.bundle(name)
        return self._orig[name]

    def _header(self, bundle_name, ri):
        """The texture header as this patch has it (edited before), else the game's."""
        if not hasattr(self, '_headers'):
            self._headers = {}
        if (bundle_name, ri) not in self._headers:
            orig = self._game_bundle(bundle_name)
            ne = len(orig['manifest']['ebx'])
            self._headers[(bundle_name, ri)] = bytearray(self.game.decode(orig['files'][1 + ne + ri]))
        return self._headers[(bundle_name, ri)]

    def _write_header(self, bundle_name, ri, header):
        b = self.bundle(bundle_name)
        ne = len(b['manifest']['ebx'])
        enc = fbw.encode_cas(bytes(header), self.game.root)
        at = self._append(enc)
        b['files'][1 + ne + ri] = dict(LOC, offset=at, size=len(enc))
        self._set_sha1(b, ne + ri, hashlib.sha1(enc).digest())

    def make_texture_whole(self, bundle_name, res_name):
        """Turns a texture whose top mips stream into one the bundle holds whole (first mip 0), as
        Studio ships cosmetics: a cosmetic texture streamed from a mod crashes the game (stored blocks)
        or leaves its load waiting forever (compressed). Call after replace_texture_pixels."""
        import ndb
        import struct
        b = self.bundle(bundle_name)
        m = b['manifest']
        raw = b['manifest_raw']
        ne, nr, nc = len(m['ebx']), len(m['res']), len(m['chunks'])
        h, ri, _ = self.game.texture(self._game_bundle(bundle_name), res_name)
        ci = next(j for j, c in enumerate(m['chunks']) if c['guid'] == h['chunk'])
        # The header (with the mip offsets replace_texture_pixels wrote): first mip and resident
        # first mip (0x1F, 0x20) to 0.
        header = self._header(bundle_name, ri)
        header[0x1F] = header[0x20] = 0
        assert fb.texture_header(bytes(header), m['res'][ri]['meta'])['first_mip'] == 0
        self._write_header(bundle_name, ri, header)
        # The bundle's chunk entry: logical offset 0, the whole size, the whole encoded chunk.
        toc_entry = self.chunks[h['chunk']]
        b['files'][1 + ne + nr + ci] = dict(LOC, offset=toc_entry['offset'], size=toc_entry['size'])
        total = ne + nr + nc
        entry = 36 + total * 20 + ne * 8 + nr * (8 + 4 + 16 + 8) + ci * 24
        struct.pack_into('<II', raw, entry + 16, 0, h['chunk_size'])
        # Its chunk-metadata record: firstMip 0 (same length, patched in place).
        meta_off, meta_size = struct.unpack_from('<II', raw, 28)
        start = 4 + meta_off
        node, end = ndb.read(bytes(raw[start:start + meta_size]))
        want = owner_hash(res_name)
        rec = next(r for r in node.children if r.field('h64') and
                   struct.unpack('<Q', r.field('h64').payload)[0] == want)
        rec.field('meta').field('firstMip').payload = struct.pack('<i', 0)
        out = ndb.write(node)
        assert len(out) == end
        raw[start:start + end] = out
        check = fb.read_binary_bundle(bytes(raw))['chunks'][ci]
        assert (check['logical_offset'], check['logical_size']) == (0, h['chunk_size'])

    def write(self, mod_dir, title, description, tool_version='0.0.1', icon=None, readme=None):
        regions = []
        for name, b in self.bundles.items():
            at = self._append(bytes(b['manifest_raw']))
            b['files'][0] = dict(LOC, offset=at, size=len(b['manifest_raw']))
            regions.append({'name': name, 'region': fbw.write_bundle_region(b['files']), 'load': 1})
        toc = fbw.write_patch_toc(regions, list(self.chunks.values()))
        if os.path.isdir(mod_dir):
            shutil.rmtree(mod_dir)
        os.makedirs(os.path.join(mod_dir, 'Win32', *PKG.split('/')))
        open(os.path.join(mod_dir, 'Win32', *PKG.split('/'), 'cas_01.cas'), 'wb').write(self.cas)
        open(os.path.join(mod_dir, 'Win32', 'items.toc'), 'wb').write(toc)
        data = self.game.data
        layout = open(os.path.join(data, 'layout.toc'), 'rb').read()
        open(os.path.join(mod_dir, 'layout.toc'), 'wb').write(layout[:8] + bytes(fb.ENVELOPE - 8) + layout[fb.ENVELOPE:])
        shutil.copyfile(os.path.join(data, 'initfs_Win32'), os.path.join(mod_dir, 'initfs_win32'))
        skate_sha = hashlib.sha256(open(os.path.join(self.game.root, 'Skate.exe'), 'rb').read()).hexdigest()
        open(os.path.join(mod_dir, '.reskate-studio-patch'), 'wb').write(
            f'ReSkate Studio native Patch v1\nskate_sha256={skate_sha}\n'.encode())
        now = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        json.dump({'schema': 1, 'tool': 'Caveman', 'version': tool_version, 'built': now},
                  open(os.path.join(mod_dir, 'reskate-build.json'), 'w'), indent=2)
        json.dump({'name': title, 'author': 'Caveman', 'version_number': tool_version, 'website_url': '',
                   'description': description, 'dependencies': []},
                  open(os.path.join(mod_dir, 'manifest.json'), 'w'), indent=4)
        open(os.path.join(mod_dir, 'README.md'), 'w').write(readme or f'# {title}\n\n{description}\n')
        if icon is not None:
            icon.save(os.path.join(mod_dir, 'icon.png'))
        return {'cas': len(self.cas), 'toc': len(toc), 'bundles': len(regions), 'chunks': len(self.chunks)}
