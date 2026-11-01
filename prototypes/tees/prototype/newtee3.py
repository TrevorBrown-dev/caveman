"""New T-shirts, test 3: on a multi-logo host whose colour provably comes from its region colours.

Why not the Vans tee any more: tests 1 and 2 (2026-10-21) cloned the Vans 00006 preset (one print sheet
over the whole shirt). In game the print landed right, but the shirt ignored every region colour Caveman
set, on the generic fabric and on the Vans jersey alike (test 2's Control Red stayed the plain jersey).

The host now: chocolate_shirt_tshirtrelaxed_00003 (white; untouched by the installed mods). Its preset and
chocolate_shirt_tshirtrelaxed_00002's (black) have the same 42 parameters and differ only in region colour
0/0a/0b/transition and two logo textures; the game's own thumbnails show one white and one black: the
colour of this preset structure comes from param_region_color_0. Its base textures are the generic tee
fabric, already in its bundle.

Print: multi-logo mode. graphic0_co, graphic1_co and graphic2_co (one decal each on UV1 tile 0 front,
tile 1 back, tile 2 sleeves and hem strip) all point at one new RGBA sheet texture (BC7 sRGB, alpha =
where the print shows), each at scale 1.0, offsets 0, aspect 1.0, which maps the sheet 1:1 onto UV1 as
the stencil draws it.

Per tee, in the host bundle: characters/customization/reskate/cust_tops/<base>_ap (the chocolate preset,
re-pointed and re-valued) and <base>_role3_<graphic1_co key> (the sheet, whole, Kraken, odd id). In
cas_main_sharedbundle: the item, two thumbnails, two CDN records; once per mod the master list and the
bundle reference table (as newtee.py)."""
import os
import struct
import uuid

import numpy as np
from PIL import Image

import bc7
import brt
import ebx
import fbw
import ndb
from newdeck import (DeckPatch, SHARED, BRT, MASTER, _clone, _ebx, _ref, _res, _template, _write, djb32,
                     new_chunk_guid, owner_hash)
from newtee import REGION, linear_to_srgb, params, set_vec, srgb_to_linear
from newwheel import add_texture, mips

HOST = 'characters/maincharacters/generic/cas/clothing/licensed/chocolate/apparel/top/shirt/tshirtrelaxed/2023/colorways/chocolate_shirt_tshirtrelaxed_00003'
HOST_BUNDLE = f'win32/{HOST}_ap_cas_main_bundlereftable'
HOST_ITEM = 'items/cust_tops/own_topshirt_chocolate_tshirtrelaxed_00003'
HOST_THUMB = 'own_topshirt_chocolate_tshirtrelaxed_00003'
SHEET_TEMPLATE = 'characters/materials/logo/licensed/chocolate/logo_chocolate_2x2_004_co'   # 1024 x 1024, 11 mips, BC7 sRGB
PREFIX = 'characters/customization/reskate/cust_tops/'
ITEMS = 'items/cust_tops/'
LEVELS = 11
LOGO_KEY = '5596d145-3dc1-b380-30fc-c51401005b02'   # param_graphic1_co: names the sheet texture (Studio's _role3_ style)
# parameter names -> the value each tee gets (the sheet at scale 1, centred, square)
PLACEMENT = {f'param_graphic{n}_scale': 1.0 for n in (0, 1, 2)}
PLACEMENT.update({f'param_graphic{n}_offset{d}': 0.0 for n in (0, 1, 2) for d in ('horizontal', 'vertical')})
PLACEMENT['param_graphic0_aspect'] = 1.0
LOGOS = ('param_graphic0_co', 'param_graphic1_co', 'param_graphic2_co')


def named_params(doc, names):
    """{parameter asset leaf name: (ParamKey text, info, boxed ref)} of a preset; names: file guid -> asset name."""
    out = {}
    for key, info, boxed in params(doc):
        p = info.find('Value').value.data.find('Parameter').value.data
        out[names[doc.imports[p.index][0]].split('/')[-1]] = (key, info, boxed)
    return out


def set_float(doc, key, value):
    records = {r.offset: r for r in doc.boxed_values}
    for k, _, boxed in params(doc):
        if k == key:
            rec = records[boxed.data_offset]
            raw = bytearray(rec.raw_bytes)
            struct.pack_into('<f', raw, 0, float(value))
            rec.raw_bytes = bytes(raw)
            return True
    return False


class Templates:
    def __init__(self, game, log=print):
        self.game = game
        host = game.bundle(HOST_BUNDLE)
        self.names = {ebx.read_file_guid(game.decode(host['files'][1 + i])): e['name'] for i, e in enumerate(host['manifest']['ebx'])}
        records = ndb.read(host['manifest']['chunk_meta'])[0].children
        self.sheet = _template(game, host, SHEET_TEMPLATE, records)
        self.ap = _ebx(game, host, f'{HOST}_ap')
        doc = ebx.read_document(self.ap)
        named = named_params(doc, self.names)
        recs = {r.offset: r for r in doc.boxed_values}
        # the stock logos each graphic slot names (file guid, instance guid)
        self.logos = {}
        for slot in LOGOS:
            rec = recs[named[slot][2].data_offset]
            self.logos[slot] = doc.imports[struct.unpack_from('<I', rec.raw_bytes)[0] >> 1]
        self.keys = {name: v[0] for name, v in named.items()}
        missing = [n for n in list(PLACEMENT) + list(LOGOS) if n not in self.keys]
        assert not missing, missing
        shared = game.bundle(SHARED)
        self.item = _ebx(game, shared, HOST_ITEM)
        self.thumb = {s: _ebx(game, shared, f'thumbnail/tool/{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.thumb_res = {s: _res(game, shared, f'thumbnail/tool/{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.cdn = {s: _ebx(game, shared, f'thumbnail/cdn/img_{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.cdn_refs = {s: _ref(ebx.read_document(self.cdn[s])) for s in ('', '_lrg')}
        self.thumb_refs = {s: _ref(ebx.read_document(self.thumb[s])) for s in ('', '_lrg')}


def add_tee(patch, t, key, sheet_rgba, shirt_srgb, thumbs, variant='print'):
    """One tee. sheet_rgba: 1024 x 1024 x 4 uint8 (colour bled, alpha = print; teelib.sheet_textures);
    variant 'print' (the sheet on all three logo slots) or 'colour' (the chocolate preset with only the
    shirt colour changed: the control). Returns (preset (name, bundle), item ref)."""
    game = t.game
    base = key[len('Own_'):].lower()
    ap_name = f'{PREFIX}{base}_ap'
    ap = _clone(t.ap, ap_name)
    if variant == 'print':
        tex = add_texture(patch, HOST_BUNDLE, t.sheet, f'{PREFIX}{base}_role3_{LOGO_KEY}',
                          mips(Image.fromarray(sheet_rgba, 'RGBA'), bc7.encode, LEVELS))
        # graphic1's logo import becomes the sheet; graphic0 and graphic2 name that same import (as the
        # stock nhs 00007 preset's graphic_c and graphic0_co share one), so no import is listed twice.
        fg, ig = t.logos['param_graphic1_co']
        ap.remap_imports({fg: tex.file_guid, ig: tex.instances[0].instance_guid})
        sheet_import = ap.imports.index((tex.file_guid, tex.instances[0].instance_guid))
        records = {r.offset: r for r in ap.boxed_values}
        named = named_params(ap, t.names)
        for slot in LOGOS:
            rec = records[named[slot][2].data_offset]
            raw = bytearray(rec.raw_bytes)
            struct.pack_into('<I', raw, 0, (sheet_import << 1) | 1)
            rec.raw_bytes = bytes(raw)
        for name, value in PLACEMENT.items():
            assert set_float(ap, t.keys[name], value), name
    lin = np.maximum(srgb_to_linear(shirt_srgb), 0.02)
    for k in REGION:
        assert set_vec(ap, k, lin), k
    patch.add_ebx(HOST_BUNDLE, ap_name, _write(ap))
    cdn_refs = {}
    for suffix, size in (('', 256), ('_lrg', 768)):
        th_name = f'thumbnail/tool/own_{base}{suffix}'
        th_hdr, th_res = t.thumb_res[suffix]
        tenc = fbw.encode_cas(bc7.encode(np.asarray(thumbs[size])), game.root)
        tchunk = new_chunk_guid()
        h2 = bytearray(th_hdr)
        h2[40:56] = uuid.UUID(tchunk).bytes_le
        struct.pack_into('<Q', h2, 120, owner_hash(th_name))
        trid = patch.new_rid()
        th = _clone(t.thumb[suffix], th_name)
        th.set('Resource', ebx.ResourceRef(trid))
        patch.add_ebx(SHARED, th_name, _write(th))
        patch.add_res(SHARED, th_name, th_res['type'], th_res['meta'], trid, bytes(h2))
        patch.add_chunk(None, tchunk, tenc)
        cdn_name = f'thumbnail/cdn/img_own_{base}{suffix}'
        cdn = _clone(t.cdn[suffix], cdn_name)
        cdn.remap_imports({t.thumb_refs[suffix][0]: th.file_guid, t.thumb_refs[suffix][1]: th.instances[0].instance_guid})
        cdn.set('NameHash', djb32(cdn_name))
        patch.add_ebx(SHARED, cdn_name, _write(cdn))
        cdn_refs[suffix] = _ref(cdn)
    item = _clone(t.item, f'{ITEMS}own_{base}')
    item.set('Key', key)
    item.set('HashedAssetKey', djb32(key))
    mapping = {}
    for s in ('', '_lrg'):
        mapping[t.cdn_refs[s][0]], mapping[t.cdn_refs[s][1]] = cdn_refs[s]
    item.remap_imports(mapping)
    di = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
    assert item.get('AssetPaths[0].AssetName', di) == 'Gen_Shirt_TshirtRelaxed_complex_dmPreset'
    item.set('AssetPaths[1].AssetName', f'{base}_ap', di)
    patch.add_ebx(SHARED, f'{ITEMS}own_{base}', _write(item))
    return (ap_name, HOST_BUNDLE), _ref(item)


def build_tees(game, tees, mods_dir, mod_name, log=print, icon=None):
    """tees: [(display, key 'Own_...', sheet_rgba or None, shirt_srgb, thumbs, variant)]."""
    patch = DeckPatch(game)
    t = Templates(game, log)
    shared = game.bundle(SHARED)
    master = ebx.read_document(_ebx(game, shared, MASTER))
    items_field = master.instances[0].object.find('Items')
    presets, built = [], []
    for display, key, sheet, shirt, thumbs, variant in tees:
        preset, item_ref = add_tee(patch, t, key, sheet, shirt, thumbs, variant)
        presets.append(preset)
        master.imports.append(item_ref)
        items_field.value.data.append(ebx.Value(ebx.Pointer('external', len(master.imports) - 1)))
        built.append((display, key))
        log(f'  built {display} ({key}, {variant})')
    patch.replace_ebx(SHARED, MASTER, _write(master))
    data, meta = _res(game, shared, BRT)
    resource, new_meta = brt.add_presets(brt.Table(data, meta['meta']), [(p.lower(), h) for p, h in presets])
    patch.replace_res(SHARED, BRT, resource, new_meta)
    lines = '\n'.join(f'- {d}' for d, _ in built)
    stats = patch.write(os.path.join(mods_dir, mod_name), mod_name, f'T-shirts built by Caveman: {", ".join(d for d, _ in built)}.',
                        icon=icon, readme=f'# {mod_name}\n\nT-shirts built by Caveman (test build 3, chocolate host):\n\n{lines}\n')
    return stats, built
