"""Texture formats and sizes of every board_bottomart (deck graphic) texture the installed mods ship.
Usage: scan_decks.py <Mods dir> <game root>"""
import collections
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fb  # noqa: E402

mods_dir, game = sys.argv[1], sys.argv[2]
summary = collections.Counter()
for mod in sorted(glob.glob(os.path.join(mods_dir, '*', ''))):
    name = os.path.basename(os.path.dirname(mod))
    if name.startswith('.'):
        continue
    layer = fb.Layer(mod, game)
    found = collections.Counter()
    for toc_path in glob.glob(os.path.join(mod, 'Win32', '**', '*.toc'), recursive=True):
        try:
            toc = fb.read_toc(toc_path)
        except Exception as e:
            print(name, 'toc failed', e)
            continue
        for b in toc['bundles']:
            if b['region'] is None or 'sharedbundle' in b['name']:
                continue
            try:
                m, files = layer.bundle(b['region'])
            except Exception as e:
                continue
            ne = len(m['ebx'])
            for i, r in enumerate(m['res']):
                if r['type'] != fb.TEXTURE_TYPE or 'bottomart' not in r['name']:
                    continue
                f = files[ne + i]
                if not f['patch']:
                    continue
                try:
                    h = fb.texture_header(fb.decode_cas(layer.read_raw(f), game)[0], r['meta'])
                except Exception as e:
                    found[('error', str(e)[:40])] += 1
                    continue
                key = (h['format'], h['w'], h['h'], h['mips'], h['first_mip'], h['flags'])
                found[key] += 1
                summary[key] += 1
    if found:
        print(f'{name}: ' + '; '.join(f'fmt={k[0]} {k[1]}x{k[2]} mips={k[3]} first={k[4]} flags={k[5]} x{v}' if k[0] != 'error' else f'{k} x{v}' for k, v in found.items()))
print('\nALL:', dict(summary))
