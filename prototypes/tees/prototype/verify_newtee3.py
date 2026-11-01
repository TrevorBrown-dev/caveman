"""Re-reads a Caveman test-3 T-shirt mod (newtee3.py, the chocolate multi-logo host) from scratch and checks it.
Usage: verify_newtee3.py <mod folder> <game root> [<previews folder with the input sheets>]"""
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
import newtee3  # noqa: E402
import newwheel  # noqa: E402
import teelib  # noqa: E402
from gamedata import Game, PKG, dds  # noqa: E402

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
skate = hashlib.sha256(open(os.path.join(game_root, 'Skate.exe'), 'rb').read()).hexdigest()
check(open(os.path.join(mod, '.reskate-studio-patch'), 'rb').read() == f'ReSkate Studio native Patch v1\nskate_sha256={skate}\n'.encode(), 'build stamp names this Skate.exe')
lay, glay = open(os.path.join(mod, 'layout.toc'), 'rb').read(), open(os.path.join(game.data, 'layout.toc'), 'rb').read()
check(lay[:8] == glay[:8] and lay[8:fb.ENVELOPE] == bytes(fb.ENVELOPE - 8) and lay[fb.ENVELOPE:] == glay[fb.ENVELOPE:], 'layout.toc = game layout, signature zeroed')

# ---- TOC, bundles, manifests
toc = fb.read_toc(os.path.join(mod, 'Win32', 'items.toc'))
tocc = {c['guid']: c for c in toc['chunks']}
print('TOC: bundles', [b['name'].split('/')[-1] for b in toc['bundles']], 'chunks', len(toc['chunks']))
check({b['name'] for b in toc['bundles']} == {newtee3.HOST_BUNDLE, newdeck.SHARED}, 'TOC lists exactly the host and shared bundles')
for c in toc['chunks']:
    check(uuid.UUID(c['guid']).bytes_le[15] & 1 == 1, f'chunk {c["guid"]}: guid low bit (compressed)')
    check(c['patch'] and c['offset'] + c['size'] <= len(cas), f'chunk {c["guid"]}: in the mod archive')
bundles = {}
for b in toc['bundles']:
    _, files = fb.read_bundle_region(b['region'])
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
            continue
        if f['patch'] or k < ne:
            check(bytes.fromhex(a['sha1']) == hashlib.sha1(raw(f)).digest(), f'sha1 {a.get("name") or a.get("guid")}')
    rids = [r['rid'] for r in m['res'][len(bm['res']):]]
    check(all(r & 1 for r in rids) and len(set(rids)) == len(rids), f'{b["name"].split("/")[-1]}: new resource ids odd and unique')
    recs = ndb.read(m['chunk_meta'])[0].children
    brecs = ndb.read(bm['chunk_meta'])[0].children
    check(len(recs) == nc and len(recs) - len(brecs) == nc - len(bm['chunks']), f'{b["name"].split("/")[-1]}: chunk meta, one record per chunk')
    print(f'  {b["name"].split("/")[-1]}: ebx {ne} (+{ne - len(bm["ebx"])}), res {nr} (+{nr - len(bm["res"])}), chunks {nc} (+{nc - len(bm["chunks"])})')
    bundles[b['name']] = (m, files, recs)
hm, hf, hrecs = bundles[newtee3.HOST_BUNDLE]
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
new_items = [n for n in docs if n.startswith(newtee3.ITEMS)]
print(f'new EBX parsed: {len(docs)} ({len(new_items)} tees)')
host_guids = {ebx.read_file_guid(dec(hf[1 + i])): e['name'] for i, e in enumerate(hm['ebx'])}
host_bundle = game.bundle(newtee3.HOST_BUNDLE)
stock = ebx.read_document(newdeck._ebx(game, host_bundle, f'{newtee3.HOST}_ap'))
stock_params = newtee.params(stock)
stock_recs = {r.offset: r for r in stock.boxed_values}


def masked(doc, rec):
    """A boxed record's bytes with its self-relative pointer slots blanked (they move with the layout)."""
    raw = bytearray(rec.raw_bytes)
    for slot in doc.pointer_offsets:
        if rec.offset <= slot < rec.offset + len(raw):
            raw[slot - rec.offset:slot - rec.offset + 8] = bytes(8)
    return bytes(raw)


stock_raw = [masked(stock, stock_recs[b.data_offset]) for _, _, b in stock_params]
names = {ebx.read_file_guid(newdeck._ebx(game, host_bundle, e['name'])): e['name'] for e in host_bundle['manifest']['ebx']}
stock_named = newtee3.named_params(stock, names)
key_of = {n: v[0] for n, v in stock_named.items()}
logo_keys = {key_of[s] for s in newtee3.LOGOS}
place_keys = {key_of[n]: v for n, v in newtee3.PLACEMENT.items()}
game_items = {e['name'] for e in game.bundle(newdeck.SHARED)['manifest']['ebx'] if e['name'].startswith('items/')}
tee = teelib.Tee(game)


def texture(name):
    ri = next(i for i, r in enumerate(hm['res']) if r['name'] == name)
    r = hm['res'][ri]
    check(docs[name].get('Resource').id == r['rid'] and r['rid'] & 1, f'{name}: resource id (odd) matches the texture asset')
    hdr = dec(hf[1 + len(hm['ebx']) + ri])
    h = fb.texture_header(hdr, r['meta'])
    check((h['format'], h['w'], h['h'], h['mips'], h['first_mip'], hdr[0x20]) == (67, 1024, 1024, 11, 0, 0), f'{name}: header BC7 sRGB 1024, 11 mips, whole')
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
    return np.asarray(Image.open(io.BytesIO(dds(1024, 1024, 99, px[:m1]))).convert('RGBA')).astype(np.float64)


ri = next(i for i, r in enumerate(sm['res']) if r['name'] == newdeck.BRT)
table = brt.Table(dec(sf[1 + len(sm['ebx']) + ri]), sm['res'][ri]['meta'])
hidx = table.bundle_index(newtee3.HOST_BUNDLE)
rows = {row[0]: row[1] for row in table.rows}
for item_name in sorted(new_items):
    base = item_name.split('own_', 1)[1]
    item = docs[item_name]
    key = item.get('Key')
    check(key.lower() == f'own_{base}' and item.get('HashedAssetKey') == newdeck.djb32(key), f'{item_name}: Key and hash')
    check(item_name not in game_items, f'{item_name}: not a game item name')
    di = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
    check(item.get('AssetPaths[0].AssetTypeId', di) == 1 and item.get('AssetPaths[0].AssetName', di) == 'Gen_Shirt_TshirtRelaxed_complex_dmPreset', f'{item_name}: mesh preset path')
    check(item.get('AssetPaths[1].AssetTypeId', di) == 3 and item.get('AssetPaths[1].AssetName', di) == f'{base}_ap', f'{item_name}: appearance preset path')
    check(str(item.imports[item.get('Categories')[0].data.index][0]) == 'd9372eac-cf2b-4310-b1b1-8a8d792d0bf5', f'{item_name}: category cust_tops')
    for s in ('', '_lrg'):
        cdn = docs[f'thumbnail/cdn/img_own_{base}{s}']
        th_name = f'thumbnail/tool/own_{base}{s}'
        th = docs[th_name]
        check((cdn.file_guid, cdn.instances[0].instance_guid) in item.imports, f'{item_name}: imports its CDN record{s}')
        check(cdn.get('NameHash') == newdeck.djb32(cdn.get('Name')) and cdn.imports == [(th.file_guid, th.instances[0].instance_guid)], f'{base}{s}: CDN record')
        tri = next(i for i, r in enumerate(sm['res']) if r['name'] == th_name)
        r = sm['res'][tri]
        hdr = dec(sf[1 + len(sm['ebx']) + tri])
        h = fb.texture_header(hdr, r['meta'])
        size = 768 if s else 256
        check(th.get('Resource').id == r['rid'] and r['rid'] & 1, f'{th_name}: resource id (odd)')
        check((h['format'], h['w'], h['h'], h['mips'], h['first_mip']) == (67, size, size, 1, 0), f'{th_name}: header')
        check(h['chunk'] in tocc and not any(c['guid'] == h['chunk'] for c in sm['chunks']), f'{th_name}: chunk TOC-only')
        px = fb.decode_cas(raw(tocc[h['chunk']]), game_root)[0]
        img = np.asarray(Image.open(io.BytesIO(dds(size, size, 98, px))).convert('RGBA'))
        check(len(px) == h['chunk_size'] and img[..., 3].min() == 0, f'{th_name}: decodes, transparent background')
    # the preset: the stock chocolate preset, parameter for parameter
    ap_name = f'{newtee3.PREFIX}{base}_ap'
    ap = docs[ap_name]
    ps = newtee.params(ap)
    check([k for k, _, _ in ps] == [k for k, _, _ in stock_params], f'{base}: the stock parameter list, in order')
    recs = {r.offset: r for r in ap.boxed_values}
    imp_slots = set(ap.import_offsets)
    used = set()
    for (k, info, boxed), sraw in zip(ps, stock_raw):
        p = info.find('Value').value.data.find('Parameter').value.data
        used.add(ap.imports[p.index][0])
        rec = recs[boxed.data_offset]
        if rec.offset in imp_slots and not (boxed.encoded_type & 0x80000000):
            used.add(ap.imports[struct.unpack_from('<I', rec.raw_bytes)[0] >> 1][0])
    check(all(g in host_guids for g in used), f'{base}: every import the preset uses is in the host bundle')
    sheet_name = f'{newtee3.PREFIX}{base}_role3_{newtee3.LOGO_KEY}'
    variant = 'print' if sheet_name in docs else 'colour'
    changed = set()
    for (k, info, boxed), sraw in zip(ps, stock_raw):
        if masked(ap, recs[boxed.data_offset]) != sraw:
            changed.add(k)
    c0 = struct.unpack_from('<3f', recs[next(b for k, _, b in ps if k == newtee.REGION[0]).data_offset].raw_bytes)
    region_ok = all(struct.unpack_from('<3f', recs[b.data_offset].raw_bytes) == c0 for k, _, b in ps if k in newtee.REGION)
    check(region_ok, f'{base}: region colours 0/0a/0b/transition equal')
    if variant == 'print':
        tex = docs[sheet_name]
        ti = ap.imports.index((tex.file_guid, tex.instances[0].instance_guid))
        check(ap.imports.count((tex.file_guid, tex.instances[0].instance_guid)) == 1, f'{base}: the sheet imported once')
        for k, _, b in ps:
            if k in logo_keys:
                check(struct.unpack_from('<I', recs[b.data_offset].raw_bytes)[0] == (ti << 1) | 1, f'{base}: logo slot {k[:8]} -> the sheet')
            if k in place_keys:
                check(struct.unpack_from('<f', recs[b.data_offset].raw_bytes)[0] == place_keys[k], f'{base}: placement {k[:8]} = {place_keys[k]}')
        allowed = set(newtee.REGION) | logo_keys | set(place_keys)
        check(changed <= allowed, f'{base}: nothing changed but region colours, logo slots and placement ({sorted(changed - allowed)})')
        top = texture(sheet_name)
        u0, v0 = teelib.DEAD[0], teelib.DEAD[1]
        check(top[int(v0 * 1024):, int(u0 * 1024):, 3].max() < 8, f'{base}: sheet transparent in the collapsed corner')
        for name, pn in tee.panels.items():
            print(f'      {name}: printed over {(top[..., 3][tee.mask(name, 1024)] > 128).mean():.0%} of the panel')
        if previews:
            want_path = os.path.join(previews, f'{key[4:]} sheet.png')
            if os.path.isfile(want_path):
                want = np.asarray(Image.open(want_path).convert('RGBA')).astype(np.float64)
                a = want[..., 3] > 200
                err = np.abs(top[..., :3] - want[..., :3])[a].mean()
                aerr = np.abs(top[..., 3] - want[..., 3]).mean()
                check(err < 4 and aerr < 4, f'{base}: decoded sheet matches the input (colour {err:.2f}, alpha {aerr:.2f})')
                print(f'      colour error {err:.2f}, alpha error {aerr:.2f} (of 255)')
    else:
        check(changed <= set(newtee.REGION), f'{base}: control changes only the region colours ({sorted(changed)})')
    print(f'  {key}: {variant}, shirt colour (linear) {tuple(round(x, 3) for x in c0)} -> sRGB {tuple(int(x) for x in newtee.linear_to_srgb(c0).round())}; changed {len(changed)} values')
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
