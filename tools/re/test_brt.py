"""Checks brt.py against the game's bundle-reference table and every installed mod's copy of it.
Usage: test_brt.py <game root> [Mods dir]

For each mod: parse its copy, list the presets it adds, and rebuild the copy from the base by inserting
those presets in the order they sit in the copy's pool (brt.Builder). A byte-identical rebuild means the
mod's tool writes tables exactly as ReSkate's merge does."""
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import brt  # noqa: E402
import fb  # noqa: E402

PKG = 'configurations/layout/initialinstallpackage'
SHARED = 'win32/characters/customization/configs/cas_main_sharedbundle'
TABLE = 'characters/customization/configs/cas_main_bundlereftable'

game = sys.argv[1]
mods_dir = sys.argv[2] if len(sys.argv) > 2 else os.path.join(game, 'Mods')
data_root = os.path.join(game, 'Data')


def raw(layer_root, f):
    root = layer_root if f['patch'] else data_root
    with open(os.path.join(root, 'Win32', PKG, f"cas_{f['archive']:02d}.cas"), 'rb') as h:
        h.seek(f['offset'])
        return h.read(f['size'])


def read_table(layer_root):
    """(resource bytes, meta, patch?) of the layer's table, or None when the layer does not ship the bundle."""
    toc_path = os.path.join(layer_root, 'Win32', 'items.toc')
    if not os.path.exists(toc_path):
        return None
    toc = fb.read_toc(toc_path)
    b = next((b for b in toc['bundles'] if b['name'] == SHARED), None)
    if b is None or b['region'] is None:
        return None
    inline, files = fb.read_bundle_region(b['region'])
    m = fb.read_binary_bundle(inline if inline is not None else raw(layer_root, files[0]))
    files = files if inline is not None else files[1:]
    ri = next((i for i, r in enumerate(m['res']) if r['name'] == TABLE), None)
    if ri is None:
        return None
    f = files[len(m['ebx']) + ri]
    return fb.decode_cas(raw(layer_root, f), game)[0], m['res'][ri]['meta'], f['patch']


data, meta, _ = read_table(data_root)
base = brt.Table(data, meta)
print(f'base: {len(data)} bytes, meta {meta.hex()}, {len(base.rows)} rows, {len(base.presets)} presets, '
      f'{base.bundle_count} bundles')
same = base.data == brt.Builder(base).write()[0]
print('base unchanged write is identical:', same)

identical = different = skipped = equivalent = 0
copies = []
for mod in sorted(glob.glob(os.path.join(mods_dir, '*', ''))):
    name = os.path.basename(os.path.dirname(mod))
    if name.startswith('.'):
        continue
    try:
        got = read_table(mod)
    except Exception as e:  # a mod whose files this reader cannot follow
        print(f'  {name}: unreadable ({e})')
        skipped += 1
        continue
    if got is None:
        continue
    cdata, cmeta, patch = got
    if not patch:
        continue  # ships the bundle but not a changed table
    try:
        copy = brt.Table(cdata, cmeta)
        added = brt.added_presets(base, copy)
    except ValueError as e:
        print(f'  {name}: refused ({e})')
        different += 1
        continue
    builder = brt.Builder(base)
    for path, b in added:
        builder.insert(path, b)
    rdata, rmeta = builder.write()
    ok = rdata == cdata and rmeta == cmeta
    identical += ok
    different += not ok
    copies.append(copy)
    if not ok:
        # Same lookups (hash, bundle, path text) and only the pool packed differently: equivalent, since
        # ReSkate's merge re-inserts every added preset by path.
        rebuilt = brt.Table(rdata, rmeta)
        same_rows = ({(h, b, rebuilt.text(t)) for h, b, t in rebuilt.rows} == {(h, b, copy.text(t)) for h, b, t in copy.rows})
        equivalent += same_rows
        diff = next((i for i in range(min(len(rdata), len(cdata))) if rdata[i] != cdata[i]), None)
        print(f'  {name}: {len(added)} presets added, rebuild differs in bytes (size {len(rdata)} vs {len(cdata)}, '
              f'first diff at {diff}); lookups {"identical (pool packing only)" if same_rows else "DIFFER"}')
print(f'mods with a changed table: {identical + different}; rebuilt byte-identical: {identical}; '
      f'equivalent (same lookups): {equivalent}; different: {different - equivalent}; unreadable: {skipped}')
if copies:
    mdata, mmeta, added, conflicts, shadowed = brt.merge(base, copies)
    merged = brt.Table(mdata, mmeta)
    print(f'merge of all copies: {added} presets added, {conflicts} conflicts, {len(shadowed)} under their full path only; '
          f'merged table parses with {len(merged.rows)} rows')
