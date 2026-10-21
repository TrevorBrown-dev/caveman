"""Re-reads a Caveman wheel mod from scratch and checks it. Usage: verify_newwheel.py <mod folder> <game root>"""
import hashlib
import io
import os
import struct
import sys
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import brt  # noqa: E402
import ebx  # noqa: E402
import fb  # noqa: E402
import ndb  # noqa: E402
import newdeck  # noqa: E402
import newwheel  # noqa: E402
from gamedata import DXGI, Game, PKG, dds  # noqa: E402
import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

mod, game_root = sys.argv[1], sys.argv[2]
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


toc = fb.read_toc(os.path.join(mod, 'Win32', 'items.toc'))
tocc = {c['guid']: c for c in toc['chunks']}
print('TOC: bundles', [b['name'].split('/')[-1] for b in toc['bundles']], 'chunks', len(toc['chunks']))
bundles = {}
for b in toc['bundles']:
    inline, files = fb.read_bundle_region(b['region'])
    m = fb.read_binary_bundle(raw(files[0]))
    ne, nr, nc = len(m['ebx']), len(m['res']), len(m['chunks'])
    check(len(files) == 1 + ne + nr + nc, f'{b["name"]}: file count')
    bm = game.bundle(b['name'])['manifest']
    check([e['name'] for e in m['ebx'][:len(bm['ebx'])]] == [e['name'] for e in bm['ebx']], 'base ebx order kept')
    check([r['name'] for r in m['res'][:len(bm['res'])]] == [r['name'] for r in bm['res']], 'base res order kept')
    for k, a in enumerate(m['ebx'] + m['res'] + m['chunks']):
        f = files[1 + k]
        if f['patch'] or k < len(bm['ebx']):
            check(bytes.fromhex(a['sha1']) == hashlib.sha1(raw(f)).digest(), f'sha1 {a.get("name") or a.get("guid")}')
    print(f'  {b["name"].split("/")[-1]}: ebx {ne} (+{ne - len(bm["ebx"])}), res {nr} (+{nr - len(bm["res"])}), chunks {nc} (+{nc - len(bm["chunks"])})')
    bundles[b['name']] = (m, files)

host_name = f'win32/{newwheel.HOST}_ap_cas_main_bundlereftable'
hm, hf = bundles[host_name]
sm, sf = bundles[newdeck.SHARED]
docs = {}
for name, (m, files) in bundles.items():
    for i, e in enumerate(m['ebx']):
        if files[1 + i]['patch']:
            d = dec(files[1 + i])
            check(len(d) == e['size'], f'{e["name"]}: size')
            doc = ebx.read_document(d)
            check(ebx.write_document(ebx.read_document(d)) == d, f'{e["name"]}: round trip')
            check(doc.get('Name').lower() == e['name'], f'{e["name"]}: Name matches manifest')
            docs[e['name']] = doc
new_items = [n for n in docs if n.startswith(newwheel.ITEMS)]
print(f'new EBX parsed: {len(docs)} ({len(new_items)} wheels)')
host_doc = {s: ebx.read_document(newdeck._ebx(game, game.bundle(host_name), f'{newwheel.HOST}{s}')) for s in ('_c', '_osk')}
recs = ndb.read(hm['chunk_meta'])[0].children
check(len(recs) == len(hm['chunks']), 'chunk meta: one record per chunk')
table = None
for item_name in new_items:
    base = item_name.split('own_', 1)[1]
    item = docs[item_name]
    key = item.get('Key')
    check(key.lower() == f'own_{base}', f'{item_name}: Key')
    check(item.get('HashedAssetKey') == newdeck.djb32(key), f'{item_name}: HashedAssetKey')
    di = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
    check(item.get('AssetPaths[0].AssetName', di) == f'{base}_ap', f'{item_name}: AssetName')
    base_colour = tuple(item.get(f'BaseColor.{a}', di) for a in 'xyz')
    # thumbnails and CDN records
    for s in ('', '_lrg'):
        cdn = docs[f'thumbnail/cdn/img_own_{base}{s}']
        check((cdn.file_guid, cdn.instances[0].instance_guid) in item.imports, f'{item_name}: imports its CDN record{s}')
        check(cdn.get('NameHash') == newdeck.djb32(cdn.get('Name')), f'{base}{s}: CDN NameHash')
        th_name = f'thumbnail/tool/own_{base}{s}'
        th = docs[th_name]
        check(cdn.imports == [(th.file_guid, th.instances[0].instance_guid)], f'{base}{s}: CDN Ref')
        ri = next(i for i, r in enumerate(sm['res']) if r['name'] == th_name)
        r = sm['res'][ri]
        check(th.get('Resource').id == r['rid'] and r['rid'] & 1, f'{th_name}: resource id (odd)')
        hdr = dec(sf[1 + len(sm['ebx']) + ri])
        h = fb.texture_header(hdr, r['meta'])
        size = 768 if s else 256
        check((h['format'], h['w'], h['h'], h['mips'], h['first_mip']) == (67, size, size, 1, 0), f'{th_name}: header')
        check(struct.unpack_from('<Q', hdr, 120)[0] == newdeck.owner_hash(th_name), f'{th_name}: name hash')
        check(h['chunk'] in tocc and not any(c['guid'] == h['chunk'] for c in sm['chunks']), f'{th_name}: chunk TOC-only')
        check(uuid.UUID(h['chunk']).bytes_le[15] & 1 == 1, f'{th_name}: chunk guid bit')
        px = fb.decode_cas(raw(tocc[h['chunk']]), game_root)[0]
        check(len(px) == h['chunk_size'], f'{th_name}: chunk size')
        Image.open(io.BytesIO(dds(size, size, 98, px))).load()
    # preset: its own textures, the colour
    ap = docs[f'{newwheel.PREFIX}{base}_ap']
    colour = newwheel.region_colour(ap)
    check(colour is not None and all(0 <= c <= 1 for c in colour), f'{base}: region colour {colour}')
    check(colour is not None and np.allclose(colour, base_colour, atol=1e-6), f'{base}: item BaseColor = region colour')
    for slot, param, fmt in (('_c', newdeck.SLOT, 67), ('_osk', newwheel.OSK, 54)):
        tex_name = f'{newwheel.PREFIX}{base}_role3_{param}'
        tex = docs[tex_name]
        hd = host_doc[slot]
        check((tex.file_guid, tex.instances[0].instance_guid) in ap.imports, f'{base}: preset imports {slot}')
        check((hd.file_guid, hd.instances[0].instance_guid) not in ap.imports, f'{base}: preset no longer imports the host {slot}')
        ri = next(i for i, r in enumerate(hm['res']) if r['name'] == tex_name)
        r = hm['res'][ri]
        check(tex.get('Resource').id == r['rid'] and r['rid'] & 1, f'{tex_name}: resource id (odd)')
        hdr = dec(hf[1 + len(hm['ebx']) + ri])
        h = fb.texture_header(hdr, r['meta'])
        check((h['format'], h['w'], h['h'], h['mips'], h['first_mip'], hdr[0x20]) == (fmt, 512, 512, newwheel.LEVELS, 0, 0),
              f'{tex_name}: header {h["format"], h["w"], h["h"], h["mips"], h["first_mip"], hdr[0x20]}')
        check(struct.unpack_from('<Q', hdr, 120)[0] == newdeck.owner_hash(tex_name), f'{tex_name}: name hash')
        cj = next(j for j, c in enumerate(hm['chunks']) if c['guid'] == h['chunk'])
        c = hm['chunks'][cj]
        check((c['logical_offset'], c['logical_size']) == (0, h['chunk_size']), f'{tex_name}: chunk whole')
        tc = tocc[h['chunk']]
        bfile = hf[1 + len(hm['ebx']) + len(hm['res']) + cj]
        check((bfile['offset'], bfile['size']) == (tc['offset'], tc['size']), f'{tex_name}: bundle chunk == TOC chunk')
        enc = raw(tc)
        m1 = h['mip_sizes'][0]
        check(struct.unpack_from('<II', hdr, 0) == (newwheel.block_containing(enc, m1), newwheel.block_containing(enc, m1 + h['mip_sizes'][1])),
              f'{tex_name}: mip offsets')
        comps = [enc[p + 4] for p in newdeck._block_starts(enc)]
        check(comps.count(0x11) >= len(comps) - 2, f'{tex_name}: Kraken blocks ({comps.count(0x11)}/{len(comps)})')
        px = fb.decode_cas(enc, game_root)[0]
        check(len(px) == h['chunk_size'], f'{tex_name}: decoded size')
        dxgi, unit = DXGI[fmt]
        img = np.asarray(Image.open(io.BytesIO(dds(512, 512, dxgi, px[:h['mip_sizes'][0]]))).convert('RGB'))
        if slot == '_c':
            want = newwheel.linear_to_srgb(colour)
            check(np.abs(img[300:500, 20:490].reshape(-1, 3).mean(0) - want).max() < 3, f'{tex_name}: inner face is the wheel colour')
            Image.fromarray(img).save(os.path.join(os.path.dirname(mod), f'verify-{base}.png'))
        else:
            check(img[256:, :, 0].max() < 8, f'{tex_name}: mask R 0 on the inner face')
        rec = recs[cj]
        check(struct.unpack('<Q', rec.field('h64').payload)[0] == newdeck.owner_hash(tex_name) and rec.field('meta').field('firstMip').value() == 0,
              f'{tex_name}: chunk meta record')
    # bundle reference table
    if table is None:
        ri = next(i for i, r in enumerate(sm['res']) if r['name'] == newdeck.BRT)
        table = brt.Table(dec(sf[1 + len(sm['ebx']) + ri]), sm['res'][ri]['meta'])
    ap_name = f'{newwheel.PREFIX}{base}_ap'
    hidx = table.bundle_index(host_name)
    check(table.presets.get(ap_name) == hidx, f'{ap_name}: preset -> host bundle')
    rows = {row[0]: row[1] for row in table.rows}
    check(rows.get(brt.fnv_hash(ap_name)) == hidx and rows.get(brt.fnv_hash(brt.leaf(ap_name))) == hidx, f'{ap_name}: full and leaf rows')
    print(f'  {key}: item, 2 thumbnails, 2 CDN records, preset (colour {tuple(round(float(x), 3) for x in colour)}), 2 textures checked')
master = docs[newdeck.MASTER]
items = master.get('Items')
base_master = ebx.read_document(newdeck._ebx(game, game.bundle(newdeck.SHARED), newdeck.MASTER))
added = len(items) - len(base_master.get('Items'))   # the mod's new items: its wheels (and decks, in a combined mod)
check(added >= len(new_items), 'itemmastercollection: count')
tail = [master.imports[v.data.index] for v in items[-added:]] if added else []
check(all((docs[n].file_guid, docs[n].instances[0].instance_guid) in tail for n in new_items), 'itemmastercollection: entries -> new items')
print(f'itemmastercollection: {len(base_master.get("Items"))} -> {len(items)} items')
print(f'\n{checks} checks, {len(fails)} failed')
