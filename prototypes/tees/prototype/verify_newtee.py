"""Re-reads a Caveman T-shirt mod from scratch and checks it (in the spirit of verify_newdeck.py and
verify_newwheel.py). Usage: verify_newtee.py <mod folder> <game root> [<previews folder with the input sheets>]"""
import hashlib
import io
import os
import struct
import sys
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
sys.path.insert(0, HERE)
import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

import brt  # noqa: E402
import ebx  # noqa: E402
import fb  # noqa: E402
import ndb  # noqa: E402
import newdeck  # noqa: E402
import newtee  # noqa: E402
import newwheel  # noqa: E402
import teelib  # noqa: E402
from gamedata import DXGI, Game, PKG, dds  # noqa: E402

mod, game_root = sys.argv[1], sys.argv[2]
previews = sys.argv[3] if len(sys.argv) > 3 else None
game = Game(game_root)
cas = open(os.path.join(mod, 'Win32', *PKG.split('/'), 'cas_01.cas'), 'rb').read()
fails = []
checks = 0


def check(ok, what):
    global checks
    checks += 1
    if not ok:
        fails.append(what)
        print('  FAIL', what)


def raw(f):
    return cas[f['offset']:f['offset'] + f['size']] if f['patch'] else game.raw(f)


def dec(f):
    return fb.decode_cas(raw(f), game_root)[0]


# ---- container files
for name in ('manifest.json', 'README.md', 'icon.png', '.reskate-studio-patch', 'reskate-build.json', 'layout.toc', 'initfs_win32'):
    check(os.path.isfile(os.path.join(mod, name)), f'{name} present')
stamp = open(os.path.join(mod, '.reskate-studio-patch'), 'rb').read()
skate = hashlib.sha256(open(os.path.join(game_root, 'Skate.exe'), 'rb').read()).hexdigest()
check(stamp == f'ReSkate Studio native Patch v1\nskate_sha256={skate}\n'.encode(), 'build stamp names this Skate.exe')
lay = open(os.path.join(mod, 'layout.toc'), 'rb').read()
glay = open(os.path.join(game.data, 'layout.toc'), 'rb').read()
check(lay[:8] == glay[:8] and lay[8:fb.ENVELOPE] == bytes(fb.ENVELOPE - 8) and lay[fb.ENVELOPE:] == glay[fb.ENVELOPE:], 'layout.toc = game layout, signature zeroed')

# ---- TOC, bundles, manifests
toc = fb.read_toc(os.path.join(mod, 'Win32', 'items.toc'))
tocc = {c['guid']: c for c in toc['chunks']}
print('TOC: bundles', [b['name'].split('/')[-1] for b in toc['bundles']], 'chunks', len(toc['chunks']))
check({b['name'] for b in toc['bundles']} == {newtee.HOST_BUNDLE, newdeck.SHARED}, 'TOC lists exactly the host and shared bundles')
for c in toc['chunks']:
    check(uuid.UUID(c['guid']).bytes_le[15] & 1 == 1, f'chunk {c["guid"]}: guid low bit (compressed)')
    check(c['patch'] and c['offset'] + c['size'] <= len(cas), f'chunk {c["guid"]}: in the mod archive')
bundles = {}
for b in toc['bundles']:
    inline, files = fb.read_bundle_region(b['region'])
    m = fb.read_binary_bundle(raw(files[0]))
    ne, nr, nc = len(m['ebx']), len(m['res']), len(m['chunks'])
    check(len(files) == 1 + ne + nr + nc, f'{b["name"]}: file count')
    bm = game.bundle(b['name'])['manifest']
    check([e['name'] for e in m['ebx'][:len(bm['ebx'])]] == [e['name'] for e in bm['ebx']], 'base ebx order kept')
    check([r['name'] for r in m['res'][:len(bm['res'])]] == [r['name'] for r in bm['res']], 'base res order kept')
    check([c['guid'] for c in m['chunks'][:len(bm['chunks'])]] == [c['guid'] for c in bm['chunks']], 'base chunk order kept')
    for k, a in enumerate(m['ebx'] + m['res'] + m['chunks']):
        f = files[1 + k]
        if k >= ne + nr and a['logical_offset']:
            continue    # the game's own streamed chunks, untouched
        if f['patch'] or k < ne:
            check(bytes.fromhex(a['sha1']) == hashlib.sha1(raw(f)).digest(), f'sha1 {a.get("name") or a.get("guid")}')
    rids = [r['rid'] for r in m['res'][len(bm['res']):]]
    check(all(r & 1 for r in rids) and len(set(rids)) == len(rids), f'{b["name"].split("/")[-1]}: new resource ids odd and unique')
    recs = ndb.read(m['chunk_meta'])[0].children
    brecs = ndb.read(bm['chunk_meta'])[0].children
    check(len(recs) == nc and len(recs) - len(brecs) == nc - len(bm['chunks']), f'{b["name"].split("/")[-1]}: chunk meta, one record per chunk (as the game)')
    print(f'  {b["name"].split("/")[-1]}: ebx {ne} (+{ne - len(bm["ebx"])}), res {nr} (+{nr - len(bm["res"])}), chunks {nc} (+{nc - len(bm["chunks"])})')
    bundles[b['name']] = (m, files, recs)

hm, hf, hrecs = bundles[newtee.HOST_BUNDLE]
sm, sf, _ = bundles[newdeck.SHARED]
docs = {}
for name, (m, files, _) in bundles.items():
    for i, e in enumerate(m['ebx']):
        if files[1 + i]['patch']:
            d = dec(files[1 + i])
            check(len(d) == e['size'], f'{e["name"]}: size')
            doc = ebx.read_document(d)
            check(ebx.write_document(ebx.read_document(d)) == d, f'{e["name"]}: round trip')
            check(doc.get('Name').lower() == e['name'], f'{e["name"]}: Name matches manifest')
            docs[e['name']] = doc
new_items = [n for n in docs if n.startswith(newtee.ITEMS)]
print(f'new EBX parsed: {len(docs)} ({len(new_items)} tees)')

# guid -> EBX name for everything the host bundle (game or mod) holds
host_guids = {}
for i, e in enumerate(hm['ebx']):
    host_guids[ebx.read_file_guid(dec(hf[1 + i]))] = e['name']
host_bundle = game.bundle(newtee.HOST_BUNDLE)
host_ap = ebx.read_document(newdeck._ebx(game, host_bundle, f'{newtee.HOST}_ap'))
host_tex = {s: ebx.read_document(newdeck._ebx(game, host_bundle, f'{newtee.LOGO}_{s}')) for s in ('c', 'osk', 'ny')}
host_keys = [k for k, _, _ in newtee.params(host_ap)]
game_keys = set()
for e in game.bundle(newdeck.SHARED)['manifest']['ebx']:
    if e['name'].startswith('items/'):
        game_keys.add(e['name'])
tee = teelib.Tee(game)


def texture(name, fmt, size, levels):
    """Checks one new texture in the host bundle; returns its top mip as an RGB array."""
    ri = next(i for i, r in enumerate(hm['res']) if r['name'] == name)
    r = hm['res'][ri]
    doc = docs[name]
    check(doc.get('Resource').id == r['rid'] and r['rid'] & 1, f'{name}: resource id (odd) matches the texture asset')
    hdr = dec(hf[1 + len(hm['ebx']) + ri])
    h = fb.texture_header(hdr, r['meta'])
    check((h['format'], h['w'], h['h'], h['mips'], h['first_mip'], hdr[0x20]) == (fmt, size, size, levels, 0, 0),
          f'{name}: header {h["format"], h["w"], h["h"], h["mips"], h["first_mip"], hdr[0x20]} (whole)')
    check(struct.unpack_from('<Q', hdr, 120)[0] == newdeck.owner_hash(name), f'{name}: name hash')
    cj = next(j for j, c in enumerate(hm['chunks']) if c['guid'] == h['chunk'])
    c = hm['chunks'][cj]
    check((c['logical_offset'], c['logical_size']) == (0, h['chunk_size']), f'{name}: chunk whole')
    tc = tocc[h['chunk']]
    bfile = hf[1 + len(hm['ebx']) + len(hm['res']) + cj]
    check((bfile['offset'], bfile['size']) == (tc['offset'], tc['size']), f'{name}: bundle chunk == TOC chunk')
    enc = raw(tc)
    m1 = h['mip_sizes'][0]
    check(struct.unpack_from('<II', hdr, 0) == (newwheel.block_containing(enc, m1), newwheel.block_containing(enc, m1 + h['mip_sizes'][1])),
          f'{name}: header bytes 0-7 = mip 1 and 2 block offsets')
    comps = [enc[p + 4] for p in newdeck._block_starts(enc)]
    check(comps.count(0x11) >= len(comps) - 2, f'{name}: Kraken blocks ({comps.count(0x11)}/{len(comps)})')
    px = fb.decode_cas(enc, game_root)[0]
    check(len(px) == h['chunk_size'], f'{name}: decoded size')
    rec = hrecs[cj]
    check(struct.unpack('<Q', rec.field('h64').payload)[0] == newdeck.owner_hash(name) and rec.field('meta').field('firstMip').value() == 0,
          f'{name}: chunk meta record (name hash, firstMip 0)')
    dxgi, unit = DXGI[fmt]
    return np.asarray(Image.open(io.BytesIO(dds(size, size, dxgi, px[:m1]))).convert('RGB')).astype(np.float64)


# the generic tee fabric listed in the host bundle: entries identical to the stock colourway bundle's
src = game.bundle(newtee.GENERIC_SOURCE)
src_m, src_f = src['manifest'], src['files']
src_recs = ndb.read(src_m['chunk_meta'])[0].children
base_m = game.bundle(newtee.HOST_BUNDLE)['manifest']
added_ebx = [e['name'] for e in hm['ebx'][len(base_m['ebx']):]]
listed = [n for n in newtee.GENERIC.values() if n in added_ebx]
check(listed == [] or sorted(listed) == sorted(newtee.GENERIC.values()), 'generic fabric listed whole or not at all')
for name in listed:
    i, si = next(i for i, e in enumerate(hm['ebx']) if e['name'] == name), next(i for i, e in enumerate(src_m['ebx']) if e['name'] == name)
    check(hm['ebx'][i] == src_m['ebx'][si] and hf[1 + i] == src_f[1 + si], f'{name}: EBX entry and location = the stock bundle\'s')
    check(not hf[1 + i]['patch'] and hashlib.sha1(game.raw(hf[1 + i])).hexdigest() == hm['ebx'][i]['sha1'], f'{name}: EBX from the game\'s own archive, SHA-1 matches')
    ri, sri = next(i for i, r in enumerate(hm['res']) if r['name'] == name), next(i for i, r in enumerate(src_m['res']) if r['name'] == name)
    check(hm['res'][ri] == src_m['res'][sri] and hf[1 + len(hm['ebx']) + ri] == src_f[1 + len(src_m['ebx']) + sri], f'{name}: resource entry and location = the stock bundle\'s')
    h = fb.texture_header(dec(hf[1 + len(hm['ebx']) + ri]), hm['res'][ri]['meta'])
    cj = next(j for j, c in enumerate(hm['chunks']) if c['guid'] == h['chunk'])
    scj = next(j for j, c in enumerate(src_m['chunks']) if c['guid'] == h['chunk'])
    check(hm['chunks'][cj] == src_m['chunks'][scj], f'{name}: chunk entry = the stock bundle\'s')
    check(hf[1 + len(hm['ebx']) + len(hm['res']) + cj] == src_f[1 + len(src_m['ebx']) + len(src_m['res']) + scj], f'{name}: chunk location = the stock bundle\'s')
    check(h['chunk'] in game.toc()['chunk_by_guid'] and h['chunk'] not in tocc, f'{name}: chunk is the game\'s (TOC), not the mod\'s')
    rec = hrecs[cj]
    srec = next(r for r in src_recs if struct.unpack('<Q', r.field('h64').payload)[0] == newdeck.owner_hash(name))
    check(ndb.write(rec) == ndb.write(srec), f'{name}: chunk-metadata record = the stock bundle\'s, at the chunk\'s position')
print(f'generic fabric listed in the host bundle: {len(listed)} textures')
shared_ny = [n for n in docs if n.endswith(f'_print_role3_{newtee.GRAPHIC_NY}')]
check(len(shared_ny) == 1, 'one shared print normal map')
ny = texture(shared_ny[0], 66, 1024, 11)
check(np.abs(ny.reshape(-1, 3).mean(0) - newtee.FLAT_NY).max() < 2, f'print normal map flat {newtee.FLAT_NY}')
table = None
ri = next(i for i, r in enumerate(sm['res']) if r['name'] == newdeck.BRT)
table = brt.Table(dec(sf[1 + len(sm['ebx']) + ri]), sm['res'][ri]['meta'])
hidx = table.bundle_index(newtee.HOST_BUNDLE)
rows = {row[0]: row[1] for row in table.rows}
for item_name in sorted(new_items):
    base = item_name.split('own_', 1)[1]
    item = docs[item_name]
    key = item.get('Key')
    check(key.lower() == f'own_{base}', f'{item_name}: Key')
    check(item.get('HashedAssetKey') == newdeck.djb32(key), f'{item_name}: HashedAssetKey')
    check(item_name not in game_keys, f'{item_name}: not a game item name')
    di = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
    check(item.get('AssetPaths[0].AssetTypeId', di) == 1 and item.get('AssetPaths[0].AssetName', di) == 'Gen_Shirt_TshirtRelaxed_complex_dmPreset', f'{item_name}: mesh preset path')
    check(item.get('AssetPaths[1].AssetTypeId', di) == 3 and item.get('AssetPaths[1].AssetName', di) == f'{base}_ap', f'{item_name}: appearance preset path')
    cat = item.imports[item.get('Categories')[0].data.index]
    check(str(cat[0]) == 'd9372eac-cf2b-4310-b1b1-8a8d792d0bf5', f'{item_name}: category items/cust_tops/cust_tops')
    for s in ('', '_lrg'):
        cdn = docs[f'thumbnail/cdn/img_own_{base}{s}']
        check((cdn.file_guid, cdn.instances[0].instance_guid) in item.imports, f'{item_name}: imports its CDN record{s}')
        check(cdn.get('NameHash') == newdeck.djb32(cdn.get('Name')), f'{base}{s}: CDN NameHash')
        th_name = f'thumbnail/tool/own_{base}{s}'
        th = docs[th_name]
        check(cdn.imports == [(th.file_guid, th.instances[0].instance_guid)], f'{base}{s}: CDN Ref')
        tri = next(i for i, r in enumerate(sm['res']) if r['name'] == th_name)
        r = sm['res'][tri]
        check(th.get('Resource').id == r['rid'] and r['rid'] & 1, f'{th_name}: resource id (odd)')
        hdr = dec(sf[1 + len(sm['ebx']) + tri])
        h = fb.texture_header(hdr, r['meta'])
        size = 768 if s else 256
        check((h['format'], h['w'], h['h'], h['mips'], h['first_mip']) == (67, size, size, 1, 0), f'{th_name}: header')
        check(struct.unpack_from('<Q', hdr, 120)[0] == newdeck.owner_hash(th_name), f'{th_name}: name hash')
        check(h['chunk'] in tocc and not any(c['guid'] == h['chunk'] for c in sm['chunks']), f'{th_name}: chunk TOC-only')
        px = fb.decode_cas(raw(tocc[h['chunk']]), game_root)[0]
        check(len(px) == h['chunk_size'], f'{th_name}: chunk size')
        img = np.asarray(Image.open(io.BytesIO(dds(size, size, 98, px))).convert('RGBA'))
        check(img[..., 3].mean() > 20 and img[..., 3].min() == 0, f'{th_name}: decodes, transparent background')
    # the preset
    ap_name = f'{newtee.PREFIX}{base}_ap'
    ap = docs[ap_name]
    keys = [k for k, _, _ in newtee.params(ap)]
    check(keys == host_keys, f'{base}: preset keeps every host parameter, in order (test 1 dropped four: white fabric in game)')
    # which base textures each base parameter names
    recs0 = {r.offset: r for r in ap.boxed_values}
    base_tex = {}
    for k, info, boxed in newtee.params(ap):
        if k in newtee.BASE_PARAMS:
            rec = recs0[boxed.data_offset]
            base_tex[k] = ap.imports[struct.unpack_from('<I', rec.raw_bytes)[0] >> 1][0]
    is_generic = all(host_guids.get(base_tex[k]) == n for k, n in newtee.GENERIC.items())
    is_vans = all(host_guids.get(base_tex[k]) == n for k, n in newtee.VANS_BASE.items())
    check(is_generic or is_vans, f'{base}: base textures all generic or all Vans')
    c0 = None
    for k, info, boxed in newtee.params(ap):
        if k == newtee.REGION[0]:
            c0 = struct.unpack_from('<3f', recs0[boxed.data_offset].raw_bytes)
    variant = 'plain' if is_generic else 'vans' if np.allclose(c0, 0.5) else 'vans-tint'
    # every import resolves inside the host bundle (no cross-bundle references)
    used = set()
    for k, info, boxed in newtee.params(ap):
        p = info.find('Value').value.data.find('Parameter').value.data
        used.add(ap.imports[p.index][0])
    recs = {r.offset: r for r in ap.boxed_values}
    for k, info, boxed in newtee.params(ap):
        rec = recs.get(boxed.data_offset)
        check(rec is not None, f'{base}: parameter {k[:8]} has its value record')
        if rec is not None and rec.offset in set(ap.import_offsets) and not (boxed.encoded_type & 0x80000000):
            used.add(ap.imports[struct.unpack_from('<I', rec.raw_bytes)[0] >> 1][0])
    check(all(g in host_guids for g in used), f'{base}: every import the preset uses is in the host bundle')
    tex = {}
    for slot, param, fmt in (('c', newtee.GRAPHIC_C, 67), ('osk', newtee.GRAPHIC_OSK, 54)):
        name = f'{newtee.PREFIX}{base}_role3_{param}'
        tex[slot] = texture(name, fmt, 1024, 11)
        d = docs[name]
        check((d.file_guid, d.instances[0].instance_guid) in ap.imports, f'{base}: preset imports its {slot}')
        hd = host_tex[slot]
        check(hd.file_guid not in used, f'{base}: preset no longer uses the Vans {slot}')
    nd = docs[shared_ny[0]]
    check(nd.file_guid in used, f'{base}: preset uses the shared print normal map')
    check(host_tex['ny'].file_guid not in used, f'{base}: preset no longer uses the Vans normal map')
    # colours
    vecs = {k: struct.unpack_from('<3f', recs[b.data_offset].raw_bytes) for k, _, b in newtee.params(ap)
            if recs.get(b.data_offset) is not None and len(recs[b.data_offset].raw_bytes) >= 12 and b.encoded_type == 0x12}
    if variant in ('plain', 'vans-tint'):
        c0 = vecs.get(newtee.REGION[0])
        check(c0 is not None and all(np.allclose(vecs[k], c0) for k in newtee.REGION), f'{base}: region colours 0/0a/0b/transition equal')
        if variant == 'plain':
            check(all(np.allclose(vecs[k], 0.5) for k in newtee.STITCH), f'{base}: neutral stitch colours')
        print(f'  {key}: {variant} ({"generic fabric" if variant == "plain" else "Vans jersey"}), shirt colour (linear) {tuple(round(x, 3) for x in c0)} -> sRGB {tuple(int(x) for x in newtee.linear_to_srgb(c0).round())}')
    else:
        print(f'  {key}: control (Vans base, Vans colours)')
    # the sheet: mask R = the print, G 179, B 0, dead corner empty; colour matches the input where printed
    osk = tex['osk']
    u0, v0 = teelib.DEAD[0], teelib.DEAD[1]
    check(osk[int(v0 * 1024):, int(u0 * 1024):, 0].max() < 8, f'{base}: mask empty in the collapsed corner')
    check(np.abs(osk[..., 1] - 179).max() < 6 and osk[..., 2].max() < 8, f'{base}: mask G 179, B 0')
    for name, pn in tee.panels.items():
        cover = (osk[..., 0][tee.mask(name, 1024)] > 128).mean()
        print(f'      {name}: printed over {cover:.0%} of the panel')
    if previews:
        sheet = os.path.join(previews, f'{key[4:]} sheet.png')
        if os.path.isfile(sheet):
            want = np.asarray(Image.open(sheet).convert('RGBA')).astype(np.float64)
            a = want[..., 3] > 200
            err = np.abs(tex['c'] - want[..., :3])[a].mean()
            check(err < 4, f'{base}: decoded colour matches the input sheet (mean error {err:.2f})')
            merr = np.abs(osk[..., 0] - want[..., 3]).mean()
            check(merr < 4, f'{base}: mask R matches the input alpha (mean error {merr:.2f})')
            print(f'      colour error {err:.2f}, mask error {merr:.2f} (of 255)')
    # bundle reference table
    check(table.presets.get(ap_name) == hidx, f'{ap_name}: preset -> host bundle')
    check(rows.get(brt.fnv_hash(ap_name)) == hidx and rows.get(brt.fnv_hash(brt.leaf(ap_name))) == hidx, f'{ap_name}: full and leaf rows')
master = docs[newdeck.MASTER]
items = master.get('Items')
base_master = ebx.read_document(newdeck._ebx(game, game.bundle(newdeck.SHARED), newdeck.MASTER))
added = len(items) - len(base_master.get('Items'))
check(added == len(new_items), 'itemmastercollection: one entry per tee')
tail = [master.imports[v.data.index] for v in items[-added:]] if added else []
check(all((docs[n].file_guid, docs[n].instances[0].instance_guid) in tail for n in new_items), 'itemmastercollection: entries -> new items')
print(f'itemmastercollection: {len(base_master.get("Items"))} -> {len(items)} items')
print(f'\n{checks} checks, {len(fails)} failed')
sys.exit(1 if fails else 0)
