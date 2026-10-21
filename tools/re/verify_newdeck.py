"""Re-reads a Caveman new-deck mod from scratch and checks it. Usage: verify_newdeck.py <mod folder> <game root>"""
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
from gamedata import Game, dds  # noqa: E402
from PIL import Image  # noqa: E402

mod, game_root = sys.argv[1], sys.argv[2]
game = Game(game_root)
PKG = 'configurations/layout/initialinstallpackage'
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
    base = game.bundle(b['name'])
    bm = base['manifest']
    check([e['name'] for e in m['ebx'][:len(bm['ebx'])]] == [e['name'] for e in bm['ebx']], 'base ebx order kept')
    check([r['name'] for r in m['res'][:len(bm['res'])]] == [r['name'] for r in bm['res']], 'base res order kept')
    mr = raw(files[0])
    assets = m['ebx'] + m['res'] + m['chunks']
    for k, a in enumerate(assets):
        f = files[1 + k]
        digest = bytes.fromhex(a['sha1'])
        if k >= ne + nr and a['logical_offset']:
            t = tocc.get(a['guid']) or game.toc()['chunk_by_guid'][a['guid']]
            want = hashlib.sha1(raw(t) if t.get('patch') else game.raw(t)).digest()
        else:
            want = hashlib.sha1(raw(f)).digest()
        if f['patch'] or k < len(bm['ebx']):
            check(digest == want, f'sha1 {a.get("name") or a.get("guid")}')
    # base entries that were not replaced still point at the game
    patched = sum(1 for f in files[1:] if f['patch'])
    print(f'  {b["name"].split("/")[-1]}: ebx {ne} (+{ne - len(bm["ebx"])}), res {nr} (+{nr - len(bm["res"])}), chunks {nc} (+{nc - len(bm["chunks"])}), patch files {patched}')
    bundles[b['name']] = (m, files)

sm, sf = bundles[newdeck.SHARED]
host_kinds = {newdeck._host_bundle(k): k for k in newdeck.HOSTS}
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
by_file = {str(d.file_guid): (n, d) for n, d in docs.items()}
new_items = [n for n in docs if n.startswith('items/board_bottomart/')]
print(f'new EBX parsed: {len(docs)} ({len(new_items)} items)')

# ---- per deck
mips = [1048576, 262144, 65536, 16384, 4096, 1024, 256, 64, 32, 16, 16, 16]
table = None
game_states = None   # {(file guid, instance guid): name} of statefx.STATES, read from the game when first needed
for item_name in new_items:
    base = item_name.split('own_', 1)[1]
    item = docs[item_name]
    key = item.get('Key')
    check(key.lower() == f'own_{base}', f'{item_name}: Key')
    check(item.get('HashedAssetKey') == newdeck.djb32(key), f'{item_name}: HashedAssetKey')
    di = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
    check(item.get('AssetPaths[0].AssetName', di) == f'{base}_ap', f'{item_name}: AssetName')
    imports = [str(f) for f, _ in item.imports]
    for s in ('', '_lrg'):
        cdn_name = f'thumbnail/cdn/img_own_{base}{s}'
        cdn = docs[cdn_name]
        check(str(cdn.file_guid) in imports and (cdn.file_guid, cdn.instances[0].instance_guid) in item.imports, f'{item_name}: imports {cdn_name}')
        check(cdn.get('NameHash') == newdeck.djb32(cdn.get('Name')), f'{cdn_name}: NameHash')
        th_name = f'thumbnail/tool/own_{base}{s}'
        th = docs[th_name]
        check(cdn.imports == [(th.file_guid, th.instances[0].instance_guid)], f'{cdn_name}: Ref -> {th_name}')
        ri = next(i for i, r in enumerate(sm['res']) if r['name'] == th_name)
        r = sm['res'][ri]
        check(th.get('Resource').id == r['rid'], f'{th_name}: resource id')
        hdr = dec(sf[1 + len(sm['ebx']) + ri])
        h = fb.texture_header(hdr, r['meta'])
        size = 768 if s else 256
        check((h['format'], h['w'], h['h'], h['mips'], h['first_mip']) == (67, size, size, 1, 0), f'{th_name}: header')
        check(struct.unpack_from('<Q', hdr, 120)[0] == newdeck.owner_hash(th_name), f'{th_name}: header name hash')
        check(h['chunk'] in tocc and not any(c['guid'] == h['chunk'] for c in sm['chunks']), f'{th_name}: chunk TOC-only')
        check(uuid.UUID(h['chunk']).bytes_le[15] & 1 == 1, f'{th_name}: chunk guid bit')
        px = fb.decode_cas(raw(tocc[h['chunk']]), game_root)[0]
        check(len(px) == h['chunk_size'], f'{th_name}: chunk size')
        Image.open(io.BytesIO(dds(size, size, 98, px))).load()
    # board effects: the item's SkaterVfx -> an always-on asset in the mod -> a blueprint in the mod, on the board
    vfx_ptr = item.instances[di].object.find('SkaterVfx').value.data
    if vfx_ptr.kind == 'external':
        asset_name, asset = by_file.get(str(item.imports[vfx_ptr.index][0]), (None, None))
        check(asset is not None and asset_name.startswith(newdeck.vfx.PREFIX), f'{item_name}: SkaterVfx -> an effect in the mod ({asset_name})')
        check(item.get('PreviewVFX', di) is True, f'{item_name}: PreviewVFX')
        nyan = asset is not None and asset_name.endswith('_nyan')
        burst = asset is not None and asset_name.endswith('_burst')
        if asset is not None:
            check((asset.file_guid, asset.instances[0].instance_guid) == item.imports[vfx_ptr.index], f'{item_name}: SkaterVfx instance')
            fx_item = asset.instances_of('SkaterVfxEffectItem')[0][1].object
            effects = fx_item.find('Effects').value.data
            parts = [(v.data.index, asset.instances[v.data.index]) for v in effects]
            check(len(parts) == (1 if nyan or burst else 2), f'{asset_name}: {len(parts)} attachments')
            # the trigger: always on (3), or a game state (6) whose asset the mod carries or the game has (statefx.py)
            ttype = fx_item.find('TriggerType').value.data
            if ttype == newdeck.statefx.GAME_STATE:
                sp = fx_item.find('TriggerEffectsGameState').value.data.find('AssetRef').value.data
                ref = asset.imports[sp.index] if sp.kind == 'external' else None
                state_name, state = by_file.get(str(ref[0]), (None, None)) if ref else (None, None)
                if state is not None:
                    check(state_name in newdeck.statefx.STATES.values() and (state.file_guid, state.instances[0].instance_guid) == ref,
                          f'{asset_name}: game state {state_name} carried in the mod')
                    where = 'carried in the mod'
                else:
                    if game_states is None:
                        states = newdeck.statefx.States(game)
                        game_states = {states.ref(n): n for n in newdeck.statefx.STATES.values()}
                    state_name = game_states.get(ref)
                    check(state_name is not None, f'{asset_name}: game state {ref} is one of the game\'s own')
                    where = "the game's own"
                cooldown_type = fx_item.find('CooldownType').value.data
                cp = fx_item.find('CooldownGameState').value.data.find('AssetRef').value.data
                cooldown_ref = asset.imports[cp.index] if cp.kind == 'external' else None
                if cooldown_type == 1:   # a state-based cooldown needs its state: the game does not check the view
                    check(cooldown_ref is not None, f'{asset_name}: CooldownType 1 has a CooldownGameState')
                cooldown_name = {newdeck.statefx.States(game).ref(n): n for n in newdeck.statefx.COOLDOWN_STATES.values()}.get(cooldown_ref, cooldown_ref) if cooldown_ref else None
                print(f'    trigger: {state_name} ({where}), usage {fx_item.find("TriggerOutputUsage").value.data}, '
                      f'cooldown type {cooldown_type}, {fx_item.find("CooldownTime").value.data:g} s'
                      + (f', cooldown state {cooldown_name}' if cooldown_ref else ''))
            else:
                check(ttype == 3, f'{asset_name}: always on (TriggerType {ttype})')
            for _, rec in parts:
                o = rec.object
                check(o.find('AttachType').value.data == newdeck.vfx.BOARD and o.find('TargetBoneHash').value.data == newdeck.vfx.NO_BONE,
                      f'{asset_name}: attached to the board')
                eff_ref = asset.imports[o.find('Effect').value.data.index]
                if burst:   # the game's own blueprint
                    stock = {str(newdeck.ebx.read_file_guid(newdeck._ebx(game, game.bundle(newdeck.SHARED), n))): n
                             for n in newdeck.statefx.BURSTS.values()}
                    bp_name, bp = stock.get(str(eff_ref[0])), None
                    check(bp_name is not None, f'{asset_name}: plays a stock burst ({bp_name})')
                    continue
                bp_name, bp = by_file.get(str(eff_ref[0]), (None, None))
                check(bp is not None and (bp.file_guid, bp.instances[0].instance_guid) == eff_ref,
                      f'{asset_name}: plays a blueprint in the mod ({bp_name})')
            if burst:
                print(f'    burst: {bp_name}')
            if bp is not None and nyan:
                effect = bp.instances[bp.instances_of('EffectEntityData')[0][0]].object
                comps = [bp.instances[v.data.index] for v in effect.find('Components').value.data]
                check(len(comps) == 1 and bp.type_name(comps[0].descriptor) == 'EmitterGraphEntityData', f'{bp_name}: one ribbon, nothing else')
                o = comps[0].object
                check(bp.imports[o.find('EmitterGraph').value.data.index][0] == newdeck.vfx.TRAIL_GRAPH, f'{bp_name}: the ribbon trail graph')
                sentinel = [[p.data.find('Value').value.data.find(c).value.data for c in 'xyz']
                            for p in o.find('EmitterGraphParams').value.data
                            if p.data.find('PropertyId').value.data in (newdeck.vfx.TRAIL_COLOUR, newdeck.vfx.TRAIL_COLOUR2)]
                import nyanpatch
                kinds = set()
                for s in sentinel:
                    if s[0] == -1:
                        kinds.add(f'negative, steps {s[1]:g}')
                    elif abs(s[0] - nyanpatch.RATIO * s[1]) < 1e-6:
                        kinds.add(f'ratio, steps {s[2] * nyanpatch.RATIO_STEPS:g}')
                    else:
                        kinds.add(f'control {[round(x, 3) for x in s]}')
                check(len(sentinel) == 2 and len(kinds) == 1, f'{bp_name}: one nyan colour on both params {sentinel}')
                print(f'    nyan: {kinds.pop() if kinds else "?"}, 1 ribbon on the board')
            elif bp is not None:
                light = bp.instances[bp.instances_of('PbrSphereLightEntityData')[0][0]].object.find('Color').value.data
                colour = [light.find(c).value.data for c in 'xyz']
                trails = []
                effect = bp.instances[bp.instances_of('EffectEntityData')[0][0]].object
                used = {v.data.index for v in effect.find('Components').value.data}
                for i, rec in bp.instances_of('EmitterGraphEntityData'):
                    if i not in used:
                        continue
                    g = rec.object.find('EmitterGraph').value.data
                    gf = bp.imports[g.index][0]
                    check(gf != newdeck.vfx.HELPER_GRAPH, f'{bp_name}: helper emitter dropped from the effect')
                    if gf == newdeck.vfx.TRAIL_GRAPH:
                        for p in rec.object.find('EmitterGraphParams').value.data:
                            if p.data.find('PropertyId').value.data == newdeck.vfx.TRAIL_COLOUR:
                                trails.append([p.data.find('Value').value.data.find(c).value.data for c in 'xyz'])
                check(len(trails) == 2 and all(abs(t[k] - colour[k] * newdeck.vfx.TRAIL_BOOST) < 1e-4 for t in trails for k in range(3)),
                      f'{bp_name}: light and trails share the colour')
                print(f'    aura: light colour {[round(c, 3) for c in colour]} (linear), {len(parts)} attachments on the board')
    # appearance preset and textures: the art, and the deck's own mask and normal map when it has them
    ap_name = f'{newdeck.PREFIX}{base}_ap'
    ap = docs[ap_name]
    host_name = next(n for n, (m, _) in bundles.items() if any(e['name'] == ap_name for e in m['ebx']))
    kind = host_kinds.get(host_name)
    check(kind is not None, f'{ap_name}: in a host bundle ({host_name})')
    hm, hf = bundles[host_name]
    host_tex = newdeck.host_textures(kind)
    textures = [('', f'{newdeck.PREFIX}{base}_role3_{newdeck.SLOT}', 67)]
    for slot, param in newdeck.SLOTS.items():
        name = f'{newdeck.PREFIX}{base}_role3_{param}'
        if slot not in host_tex:
            check(name not in docs, f'{name}: a {slot} texture on a {kind} deck')
            continue
        default = host_tex[slot]
        default_doc = ebx.read_document(newdeck._ebx(game, game.bundle(host_name), default))
        if name in docs:
            textures.append((f'-{slot}', name, 66))
            check((default_doc.file_guid, default_doc.instances[0].instance_guid) not in ap.imports, f'{ap_name}: no longer imports {default}')
        else:
            check((default_doc.file_guid, default_doc.instances[0].instance_guid) in ap.imports, f'{ap_name}: keeps {default}')
    if kind == 'holo':
        modes = [name for name, g in newdeck.FOIL_MODES.items() if any(str(i) == g for _, i in ap.imports)]
        check(len(modes) == 1, f'{ap_name}: one foil mode ({modes})')
        check(f'{newdeck.PREFIX}{base}_role3_{newdeck.SLOTS["osk"]}' in docs and f'{newdeck.PREFIX}{base}_role3_{newdeck.SLOTS["rgb"]}' in docs,
              f'{ap_name}: holo deck has its own mask and holo map')
        print(f'    holo, foil mode {modes}')
    for suffix, tex_name, fmt in textures:
        tex = docs[tex_name]
        check((tex.file_guid, tex.instances[0].instance_guid) in ap.imports, f'{ap_name}: imports {tex_name}')
        ri = next(i for i, r in enumerate(hm['res']) if r['name'] == tex_name)
        r = hm['res'][ri]
        check(tex.get('Resource').id == r['rid'], f'{tex_name}: resource id')
        hdr = dec(hf[1 + len(hm['ebx']) + ri])
        h = fb.texture_header(hdr, r['meta'])
        check((h['format'], h['w'], h['h'], h['mips'], h['first_mip'], hdr[0x20]) == (fmt, 512, 2048, 12, 0, 0), f'{tex_name}: header {h["format"], h["w"], h["h"], h["mips"], h["first_mip"], hdr[0x20]}')
        check(struct.unpack_from('<Q', hdr, 120)[0] == newdeck.owner_hash(tex_name), f'{tex_name}: header name hash')
        cj = next(j for j, c in enumerate(hm['chunks']) if c['guid'] == h['chunk'])
        c = hm['chunks'][cj]
        check((c['logical_offset'], c['logical_size']) == (0, 1398160), f'{tex_name}: chunk whole')
        tc = tocc[h['chunk']]
        bfile = hf[1 + len(hm['ebx']) + len(hm['res']) + cj]
        check((bfile['offset'], bfile['size']) == (tc['offset'], tc['size']), f'{tex_name}: bundle chunk == TOC chunk')
        enc = raw(tc)
        check(struct.unpack_from('<II', hdr, 0) == (newdeck.fbw.block_offset(enc, 0x100000), newdeck.fbw.block_offset(enc, 0x140000)), f'{tex_name}: mip offsets')
        comps = [enc[p + 4] for p in newdeck._block_starts(enc)]
        check(comps.count(0x11) >= len(comps) - 2, f'{tex_name}: Kraken blocks ({comps.count(0x11)}/{len(comps)})')
        px = fb.decode_cas(enc, game_root)[0]
        check(len(px) == 1398160, f'{tex_name}: decoded size')
        img = Image.open(io.BytesIO(dds(512, 2048, 98, px[:1048576]))).convert('RGB')
        img.save(os.path.join(os.path.dirname(mod), f'verify-{base}{suffix}.png'))
        recs = ndb.read(hm['chunk_meta'])[0].children
        check(len(recs) == len(hm['chunks']), 'chunk meta: one record per chunk')
        rec = recs[cj]
        check(struct.unpack('<Q', rec.field('h64').payload)[0] == newdeck.owner_hash(tex_name) and rec.field('meta').field('firstMip').value() == 0,
              f'{tex_name}: chunk meta record')
    # bundle reference table
    if table is None:
        ri = next(i for i, r in enumerate(sm['res']) if r['name'] == newdeck.BRT)
        table = brt.Table(dec(sf[1 + len(sm['ebx']) + ri]), sm['res'][ri]['meta'])
    hidx = table.bundle_index(host_name)
    check(table.presets.get(ap_name) == hidx, f'{ap_name}: preset -> host bundle')
    rows = {row[0]: row[1] for row in table.rows}
    check(rows.get(brt.fnv_hash(ap_name)) == hidx and rows.get(brt.fnv_hash(brt.leaf(ap_name))) == hidx, f'{ap_name}: full and leaf rows')
    print(f'  {key}: item, 2 thumbnails, 2 CDN records, preset, {len(textures)} texture(s) checked')

# ---- master item list
master = docs[newdeck.MASTER]
items = master.get('Items')
base_master = ebx.read_document(newdeck._ebx(game, game.bundle(newdeck.SHARED), newdeck.MASTER))
# a mod may hold wheels too (the Rust build puts decks and wheels in one mod): the list gains every new item
listed = new_items + [n for n in docs if n.startswith('items/board_wheelcolor/')]
check(len(items) == len(base_master.get('Items')) + len(listed), 'itemmastercollection: count')
tail = [master.imports[v.data.index] for v in items[-len(listed):]]
check(sorted(str(f) for f, _ in tail) == sorted(str(docs[n].file_guid) for n in listed), 'itemmastercollection: entries -> new items')
check(all((docs[n].file_guid, docs[n].instances[0].instance_guid) in tail for n in listed), 'itemmastercollection: instance guids')
print(f'itemmastercollection: {len(base_master.get("Items"))} -> {len(items)} items')
print(f'\n{checks} checks, {len(fails)} failed')
