"""New T-shirts (investigation prototype, built on the frozen Python toolkit): a print sheet (RGBA, the tee
mesh's UV1 space, see teelib.py) and a shirt colour become a tee item, laid out the way ReSkate Studio's
clothing packs are, with every template cloned from the player's own install (nothing from another mod).

Host: the stock Vans 00006 tee (vans_shirt_tshirtrelaxed_00006), whose preset puts one logo sheet over the
whole print area (param_graphic_c / _osk / _ny at graphic0/1/2 scale 1.0) and whose bundle no installed mod
touches. Per tee, in the host bundle:
  characters/customization/reskate/cust_tops/<base>_ap                  the Vans preset, re-pointed
  characters/customization/reskate/cust_tops/<base>_role3_<slot>        the sheet's colour (BC7 sRGB) and mask
                                                                         (BC1), whole, Kraken, odd ids
and once per mod a flat normal map for the print (BC7 linear). In cas_main_sharedbundle: the item
items/cust_tops/own_<base> (the Vans tee's, AssetPaths[1] -> the new preset; AssetPaths[0] stays the stock
tee mesh preset Gen_Shirt_TshirtRelaxed_complex_dmPreset), two thumbnails and two CDN records; once per
mod, items/itemmastercollection and the bundle reference table (each new preset -> the Vans bundle).

Preset variants:
  'plain'      the Vans preset with its four base textures (colour, mask, normal map, stitches) pointed at
               the generic tee fabric, which the mod lists in the Vans bundle (list_game_textures: the game's
               own entries, as the generic colourway bundles list them), the shirt colour in region colour
               0/0a/0b/transition and neutral stitch colours. (Test 1 dropped the four parameters instead,
               expecting the mesh's defaults: in game the fabric came out white and ignored the colour.)
  'vans-tint'  the Vans jersey with the shirt colour in its region colours (a diagnostic);
  'vans'       the Vans preset unchanged but for the sheet (the Vans jersey base under the print)."""
import copy
import os
import struct
import uuid

import numpy as np
from PIL import Image

import bc1
import bc7
import brt
import ebx
import fb
import fbw
import ndb
from newdeck import (DeckPatch, SHARED, BRT, MASTER, _clone, _ebx, _ref, _res, _template, _write, djb32,
                     new_chunk_guid, owner_hash)
from newwheel import add_texture, mips

HOST = 'characters/maincharacters/generic/cas/clothing/licensed/vans/apparel/top/shirt/tshirtrelaxed/2023/colorways/vans_shirt_tshirtrelaxed_00006'
HOST_BUNDLE = f'win32/{HOST}_ap_cas_main_bundlereftable'
LOGO = 'characters/maincharacters/generic/cas/clothing/licensed/vans/apparel/top/shirt/tshirtrelaxed/2023/colorways/logo_vans_shirt_tshirtrelaxed_logosheet_00006'
HOST_ITEM = 'items/cust_tops/own_topshirt_vans_tshirtrelaxed_00006'
HOST_THUMB = 'own_topshirt_vans_tshirtrelaxed_00006'
PREFIX = 'characters/customization/reskate/cust_tops/'
ITEMS = 'items/cust_tops/'
LEVELS = 11
# parameter keys (the presets' ParamKey GUIDs; also the _role3_ suffix of texture names)
GRAPHIC_C = '2259d419-387f-b1ba-30fc-c5140100b012'
GRAPHIC_OSK = 'c1b71b8a-395f-e8ce-30fc-c5140100a381'
GRAPHIC_NY = '830b93ff-7cad-e8ee-30fc-c5140100c668'
BASE_PARAMS = {'0bb23445-4861-79a3-30fc-c5140100e899': 'param_base_c', '00c854f4-547f-9695-30fc-c5140100edb5': 'param_base_msk',
               '3f4cd4ca-ab37-af17-30fc-c5140100f7d6': 'param_base_ny', '89aaa101-e0f3-4084-30fc-c514010073d2': 'param_stitchesrgb'}
REGION = ('4b18a9e2-cd35-8c46-59ff-5e880100548c', '2f4ec0b2-b574-1567-59ff-5e880100e616', 'cb33c53b-a942-1564-59ff-5e880100e616',
          'c20ab77c-43a7-5e19-59ff-5e8801002210')     # region colour 0, 0a, 0b, transition
STITCH = ('c679da8d-4e85-1898-59ff-5e88010091d2', 'd8da57bb-6fc9-2bf9-59ff-5e880100b424', 'd8474129-b51b-13cf-59ff-5e880100a09d')
FLAT_NY = (128, 127, 191)     # the stock tee logo sheets' flat print normal (nike_shirt_tshirtrelaxed logos)
# The plain tee fabric: the generic relaxed-tee textures every generic colourway names explicitly. The Vans
# bundle does not hold them, so the mod lists them in it, exactly as the generic colourway bundle below does
# (the same manifest entries, pointing at the game's own data; nothing is copied). In game (2026-10-21), a
# preset WITHOUT its base parameters showed white fabric and ignored the region colour: absent base textures
# are not filled from the mesh's defaults.
GENERIC_DIR = 'characters/maincharacters/generic/cas/clothing/unlicensed/generic/apparel/top/shirt/tshirtrelaxed/2023/'
GENERIC = {'0bb23445-4861-79a3-30fc-c5140100e899': f'{GENERIC_DIR}gen_shirt_tshirtrelaxed_c',
           '00c854f4-547f-9695-30fc-c5140100edb5': f'{GENERIC_DIR}gen_shirt_tshirtrelaxed_msk',
           '3f4cd4ca-ab37-af17-30fc-c5140100f7d6': f'{GENERIC_DIR}gen_shirt_tshirtrelaxed_ny',
           '89aaa101-e0f3-4084-30fc-c514010073d2': f'{GENERIC_DIR}gen_shirt_tshirtrelaxed_rgb'}
VANS_BASE = {'0bb23445-4861-79a3-30fc-c5140100e899': f'{HOST}_c', '00c854f4-547f-9695-30fc-c5140100edb5': f'{HOST}_msk',
             '3f4cd4ca-ab37-af17-30fc-c5140100f7d6': f'{HOST}_ny', '89aaa101-e0f3-4084-30fc-c514010073d2': f'{HOST}_rgb'}
GENERIC_SOURCE = 'win32/characters/maincharacters/generic/cas/clothing/unlicensed/generic/apparel/top/shirt/tshirtrelaxed/2023/colorways/gen_shirt_tshirtrelaxed_00008_ap_cas_main_bundlereftable'


def list_game_textures(patch, host_name, source_name, names):
    """Lists textures of the game in another bundle (`host_name`) as `source_name` lists them: the EBX, the
    resource and the chunk entries with their SHA-1s, sizes, resource ids and metas, the file locations (the
    game's own archives) and the chunk-metadata records, all copied. No data is written."""
    game = patch.game
    src = game.bundle(source_name)
    sm, sf = src['manifest'], src['files']
    ne, nr = len(sm['ebx']), len(sm['res'])
    records = ndb.read(sm['chunk_meta'])[0].children
    b = patch.bundle(host_name)
    m = b['manifest']
    have = {e['name'] for e in m['ebx']}
    for name in names:
        assert name not in have, name
        i = next(i for i, e in enumerate(sm['ebx']) if e['name'] == name)
        ri = next(i for i, r in enumerate(sm['res']) if r['name'] == name)
        h = fb.texture_header(game.decode(sf[1 + ne + ri]), sm['res'][ri]['meta'])
        cj = next(j for j, c in enumerate(sm['chunks']) if c['guid'] == h['chunk'])
        record = next(r for r in records if r.field('h64') is not None and
                      struct.unpack('<Q', r.field('h64').payload)[0] == owner_hash(name))
        b['files'].insert(1 + len(m['ebx']), dict(sf[1 + i]))
        m['ebx'].append(dict(sm['ebx'][i]))
        b['files'].insert(1 + len(m['ebx']) + len(m['res']), dict(sf[1 + ne + ri]))
        m['res'].append(dict(sm['res'][ri]))
        b['files'].append(dict(sf[1 + ne + nr + cj]))
        m['chunks'].append(dict(sm['chunks'][cj]))
        root, end = ndb.read(m['chunk_meta'])
        assert end == len(m['chunk_meta'])
        root.children.insert(len(m['chunks']) - 1, copy.deepcopy(record))
        m['chunk_meta'] = ndb.write(root)
    return {name: ebx.read_document(_ebx(game, src, name)) for name in names}


def srgb_to_linear(c):
    c = np.asarray(c, np.float64) / 255
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(c):
    c = np.clip(np.asarray(c, np.float64), 0, 1)
    return 255 * np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)


def params(doc):
    """[(ParamKey text, AppearanceExpressionParamInfo object, boxed ref)] of a preset."""
    out = []
    for v in doc.instances[0].object.find('ParamInfo').value.data:
        info = v.data
        key = str(info.find('ParamKey').value.data.find('Data').value.data)
        out.append((key, info, info.find('Value').value.data.find('Value').value.data))
    return out


def set_vec(doc, key, rgb):
    records = {r.offset: r for r in doc.boxed_values}
    for k, _, boxed in params(doc):
        if k == key:
            rec = records[boxed.data_offset]
            raw = bytearray(rec.raw_bytes)
            struct.pack_into('<3f', raw, 0, *(float(x) for x in rgb))
            rec.raw_bytes = bytes(raw)
            return True
    return False


def drop_params(doc, keys):
    """Removes parameters (and their boxed values) from a preset; their imports stay listed, unused."""
    plist = doc.instances[0].object.find('ParamInfo').value
    keep, dropped = [], []
    for v in plist.data:
        key = str(v.data.find('ParamKey').value.data.find('Data').value.data)
        (dropped if key in keys else keep).append(v)
    gone = {v.data.find('Value').value.data.find('Value').value.data.data_offset for v in dropped}
    still = {v.data.find('Value').value.data.find('Value').value.data.data_offset for v in keep}
    doc.boxed_values = [r for r in doc.boxed_values if r.offset not in gone or r.offset in still]
    plist.data = keep
    return len(dropped)


class Templates:
    def __init__(self, game, log=print):
        self.game = game
        host = game.bundle(HOST_BUNDLE)
        records = ndb.read(host['manifest']['chunk_meta'])[0].children
        self.textures = {'c': _template(game, host, f'{LOGO}_c', records),
                         'osk': _template(game, host, f'{LOGO}_osk', records),
                         'ny': _template(game, host, f'{LOGO}_ny', records)}
        self.ap = _ebx(game, host, f'{HOST}_ap')
        self.vans_base = {k: ebx.read_document(_ebx(game, host, n)) for k, n in VANS_BASE.items()}
        shared = game.bundle(SHARED)
        self.item = _ebx(game, shared, HOST_ITEM)
        self.thumb = {s: _ebx(game, shared, f'thumbnail/tool/{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.thumb_res = {s: _res(game, shared, f'thumbnail/tool/{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.cdn = {s: _ebx(game, shared, f'thumbnail/cdn/img_{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.cdn_refs = {s: _ref(ebx.read_document(self.cdn[s])) for s in ('', '_lrg')}
        self.thumb_refs = {s: _ref(ebx.read_document(self.thumb[s])) for s in ('', '_lrg')}


def flat_normal(patch, t, name):
    img = Image.new('RGB', (1024, 1024), FLAT_NY)
    return add_texture(patch, HOST_BUNDLE, t.textures['ny'], name, mips(img, bc7.encode, LEVELS))


def add_tee(patch, t, key, colour_rgb, mask_rgb, shirt_srgb, thumbs, ny_doc, variant='plain', generic=None):
    """One tee. colour_rgb / mask_rgb: 1024 x 1024 x 3 uint8 (teelib.sheet_textures); shirt_srgb: 0-255;
    thumbs: {256: image, 768: image}; ny_doc: the mod's flat print normal (texture document); generic: the
    generic fabric textures listed in the host bundle ({name: document}, list_game_textures).
    Variants: 'plain' (the generic fabric, the shirt colour), 'vans-tint' (the Vans jersey, the shirt colour
    in its region colours: shows whether the Vans preset honours them), 'vans' (the Vans preset unchanged).
    Returns (preset (name, bundle), item ref)."""
    game = t.game
    base = key[len('Own_'):].lower()
    ap_name = f'{PREFIX}{base}_ap'
    tex = {'c': add_texture(patch, HOST_BUNDLE, t.textures['c'], f'{PREFIX}{base}_role3_{GRAPHIC_C}',
                            mips(Image.fromarray(colour_rgb), bc7.encode, LEVELS)),
           'osk': add_texture(patch, HOST_BUNDLE, t.textures['osk'], f'{PREFIX}{base}_role3_{GRAPHIC_OSK}',
                              mips(Image.fromarray(mask_rgb), bc1.encode, LEVELS)),
           'ny': ny_doc}
    remap = {}
    for slot, doc in tex.items():
        t_doc = t.textures[slot][1]
        remap[t_doc.file_guid] = doc.file_guid
        remap[t_doc.instances[0].instance_guid] = doc.instances[0].instance_guid
    if variant == 'plain':
        # the Vans jersey's base textures -> the generic tee fabric (listed in the host bundle)
        for key_, vans_doc in t.vans_base.items():
            g_doc = generic[GENERIC[key_]]
            remap[vans_doc.file_guid] = g_doc.file_guid
            remap[vans_doc.instances[0].instance_guid] = g_doc.instances[0].instance_guid
    ap = _clone(t.ap, ap_name)
    ap.remap_imports(remap)
    if variant in ('plain', 'vans-tint'):
        lin = np.maximum(srgb_to_linear(shirt_srgb), 0.02)
        for k in REGION:
            assert set_vec(ap, k, lin), k
    if variant == 'plain':
        for k in STITCH:
            assert set_vec(ap, k, (0.5, 0.5, 0.5)), k
    patch.add_ebx(HOST_BUNDLE, ap_name, _write(ap))
    # thumbnails and CDN records (as newwheel)
    cdn_refs = {}
    for suffix, size in (('', 256), ('_lrg', 768)):
        img = thumbs[size]
        th_name = f'thumbnail/tool/own_{base}{suffix}'
        th_hdr, th_res = t.thumb_res[suffix]
        tenc = fbw.encode_cas(bc7.encode(np.asarray(img)), game.root)
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


def build_tees(game, tees, mods_dir, mod_name, tag, log=print, icon=None):
    """tees: [(display, key 'Own_...', colour_rgb, mask_rgb, shirt_srgb, thumbs, variant)]. `tag` names the
    mod's shared print normal map (unique per mod). Writes <mods_dir>/<mod_name>."""
    patch = DeckPatch(game)
    t = Templates(game, log)
    shared = game.bundle(SHARED)
    master = ebx.read_document(_ebx(game, shared, MASTER))
    items_field = master.instances[0].object.find('Items')
    ny_doc = flat_normal(patch, t, f'{PREFIX}{tag}_print_role3_{GRAPHIC_NY}')
    generic = None
    if any(v == 'plain' for *_, v in tees):
        generic = list_game_textures(patch, HOST_BUNDLE, GENERIC_SOURCE, list(GENERIC.values()))
    presets, built = [], []
    for display, key, colour_rgb, mask_rgb, shirt, thumbs, variant in tees:
        preset, item_ref = add_tee(patch, t, key, colour_rgb, mask_rgb, shirt, thumbs, ny_doc, variant, generic)
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
                        icon=icon, readme=f'# {mod_name}\n\nT-shirts built by Caveman (test build):\n\n{lines}\n')
    return stats, built
