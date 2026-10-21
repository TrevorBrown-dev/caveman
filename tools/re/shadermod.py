"""Builds a shader-only test mod that swaps the game's six deck pixel programs for Caveman's patched ones (shaderpatch.py)
and the ribbon program for Caveman's solid rainbow trail (nyanpatch.py).
Usage: shadermod.py <game dir> <out mod dir> [--mark <deck png> <marked png out>]
Writes only <out mod dir> (and the marked PNG); installing it is a separate, manual step with the game closed."""
import datetime
import hashlib
import json
import os
import shutil
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fb  # noqa: E402
import fbw  # noqa: E402
import gamedata  # noqa: E402
import shaderpatch  # noqa: E402

LOC = dict(patch=True, install=fbw.MOD_INSTALL_CHUNK, archive=fbw.MOD_ARCHIVE)


def build_mod(game_dir, mod_dir, title='Caveman Shaders (test)'):
    game = gamedata.Game(game_dir)
    b = game.bundle(shaderpatch.CONFIG_BUNDLE, toc=shaderpatch.GLOBALS_TOC)
    m = b['manifest']
    index = {r['name']: i for i, r in enumerate(m['res'])}

    def read_program(name):
        return game.decode(b['files'][1 + len(m['ebx']) + index[name]])
    programs = shaderpatch.build(read_program, game_dir)
    # the ribbon program too: Caveman's solid rainbow trail (nyanpatch.py), in the same bundle
    import nyanpatch
    name, payload, meta = nyanpatch.build(game_dir)
    programs[name] = (payload, meta, 'nyan ribbon')
    # its vertex program (nyanvspatch.py: flat, pulled toward the camera) with the banner on top (nyanbannerpatch.py:
    # the width follows the board's axis as laid), and the banner's spawn and simulate programs, which live in the
    # shared customization bundle (items.toc)
    import nyanbannerpatch
    shared_programs = {}
    for toc_name, bundle_name, name, payload, meta in nyanbannerpatch.build(game_dir):
        target = programs if bundle_name == shaderpatch.CONFIG_BUNDLE else shared_programs
        target[name] = (payload, meta, 'nyan banner')
    cas = bytearray()
    config_region = _patch_bundle(game, b, programs, cas)
    shared_region = _patch_bundle(game, game.bundle(nyanbannerpatch.SHARED_BUNDLE), shared_programs, cas)
    toc = fbw.write_patch_toc([{'name': shaderpatch.CONFIG_BUNDLE, 'region': config_region, 'load': 1}], [])
    items_toc = fbw.write_patch_toc([{'name': nyanbannerpatch.SHARED_BUNDLE, 'region': shared_region, 'load': 1}], [])
    _write_mod(game, mod_dir, title, cas, {shaderpatch.GLOBALS_TOC: toc, 'items.toc': items_toc})
    return {'cas': len(cas), 'toc': len(toc), 'items_toc': len(items_toc)}


def _patch_bundle(game, b, programs, cas):
    """The bundle region of a copy of bundle `b` whose `programs` ({name: (payload, meta, notes)}) are replaced, their
    files appended to `cas` (the manifest's SHA-1s, sizes and metas edited in place)."""
    m = b['manifest']
    assert m['endian'] == '<', 'expected a little-endian manifest'
    ne, nr, nc = len(m['ebx']), len(m['res']), len(m['chunks'])
    total = ne + nr + nc
    index = {r['name']: i for i, r in enumerate(m['res'])}
    files = [dict(f) for f in b['files']]
    raw = bytearray(b['manifest_raw'])
    for name, (payload, meta, notes) in programs.items():
        i = index[name]
        assert m['res'][i]['type'] == shaderpatch.BYTECODE_TYPE
        enc = fbw.encode_cas(payload, game.root)
        files[1 + ne + i] = dict(LOC, offset=len(cas), size=len(enc))
        cas += enc
        raw[36 + 20 * (ne + i): 56 + 20 * (ne + i)] = hashlib.sha1(enc).digest()
        names = 36 + 20 * total + 8 * ne
        struct.pack_into('<I', raw, names + 8 * i + 4, len(payload))          # res size
        metas = names + 8 * nr + 4 * nr
        raw[metas + 16 * i: metas + 16 * i + 16] = meta                      # res meta
        print(f'{name}: {notes}')
    check = fb.read_binary_bundle(bytes(raw))
    for name, (payload, meta, _) in programs.items():
        r = check['res'][index[name]]
        assert r['size'] == len(payload) and r['meta'] == meta and r['type'] == shaderpatch.BYTECODE_TYPE
    files[0] = dict(LOC, offset=len(cas), size=len(raw))
    cas += raw
    return fbw.write_bundle_region(files)


def _write_mod(game, mod_dir, title, cas, tocs):
    """The mod folder: the cas, the patch TOCs ({file name under Win32: bytes}), layout, initfs, stamps."""
    if os.path.isdir(mod_dir):
        shutil.rmtree(mod_dir)
    os.makedirs(os.path.join(mod_dir, 'Win32', *gamedata.PKG.split('/')))
    open(os.path.join(mod_dir, 'Win32', *gamedata.PKG.split('/'), 'cas_01.cas'), 'wb').write(cas)
    for toc_name, data in tocs.items():
        open(os.path.join(mod_dir, 'Win32', toc_name), 'wb').write(data)
    layout = open(os.path.join(game.data, 'layout.toc'), 'rb').read()
    open(os.path.join(mod_dir, 'layout.toc'), 'wb').write(layout[:8] + bytes(fb.ENVELOPE - 8) + layout[fb.ENVELOPE:])
    shutil.copyfile(os.path.join(game.data, 'initfs_Win32'), os.path.join(mod_dir, 'initfs_win32'))
    skate_sha = hashlib.sha256(open(os.path.join(game.root, 'Skate.exe'), 'rb').read()).hexdigest()
    open(os.path.join(mod_dir, '.reskate-studio-patch'), 'wb').write(
        f'ReSkate Studio native Patch v1\nskate_sha256={skate_sha}\n'.encode())
    now = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
    json.dump({'schema': 1, 'tool': 'Caveman', 'version': '0.0.1', 'built': now},
              open(os.path.join(mod_dir, 'reskate-build.json'), 'w'), indent=2)
    description = ("Caveman's deck and trail shaders: glow, colour cycle and the solid rainbow trail, only on decks and trails "
                   'that ask for them; everything else looks as before.')
    json.dump({'name': title, 'author': 'Caveman', 'version_number': '0.0.1', 'website_url': '',
               'description': description, 'dependencies': []},
              open(os.path.join(mod_dir, 'manifest.json'), 'w'), indent=4)
    open(os.path.join(mod_dir, 'README.md'), 'w').write(f'# {title}\n\n{description}\n')


def mark(png, out, effect=shaderpatch.EFFECT_CYCLE, speed=32, glow=0):
    """Paints the marker into an image (any size of the 1:4 template: the blocks scale with it). The programs
    read it from a deck's holo map, not its art: Caveman decks get it through mixins.layers (`cycle`, `glow`)."""
    from PIL import Image, ImageDraw
    im = Image.open(png).convert('RGB')
    sx, sy = im.width / shaderpatch.ART_W, im.height / shaderpatch.ART_H
    draw = ImageDraw.Draw(im)
    for x0, y0, x1, y1, colour in shaderpatch.marker_pixels(effect, speed, glow):
        draw.rectangle((round(x0 * sx), round(y0 * sy), round(x1 * sx) - 1, round(y1 * sy) - 1), fill=colour)
    im.save(out)


if __name__ == '__main__':
    args = sys.argv[1:]
    if '--mark' in args:
        i = args.index('--mark')
        mark(args[i + 1], args[i + 2])
        del args[i:i + 3]
    print(build_mod(args[0], args[1]))
