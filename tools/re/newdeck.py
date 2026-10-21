"""New decks: each PNG becomes its own deck item, laid out the way ReSkate Studio's deck packs are, with
every template cloned from the installed game (never from another mod).

Per deck, in the host deck's bundle (HOSTS: a plain stock deck, or the Primitive holo deck for holo):
  characters/customization/reskate/board_bottomart/<base>_ap            appearance preset (host's, re-pointed)
  characters/customization/reskate/board_bottomart/<base>_role3_<slot>  texture asset, texture resource, chunk
                                                                        (the art; and the deck's own mask,
                                                                        normal map, holo map: mixins.py)
In cas_main_sharedbundle:
  items/board_bottomart/own_<base>                                      the item
  thumbnail/tool/own_<base>, ..._lrg                                    thumbnail textures (chunks in the TOC only)
  thumbnail/cdn/img_own_<base>, ..._lrg                                 CDN thumbnail records
and, once per mod, items/itemmastercollection (one entry per item) and the bundle reference table (each
appearance preset pointed at the host bundle).

The deck texture is whole (first mip 0) and Kraken-compressed. Its header records the encoded offsets of
mips 1 and 2 (bytes 0-7) and the hash of its name (bytes 120-127), as the game's and Studio's do."""
import copy
import hashlib
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
import statefx
import thumbs
import vfx
from patch import LOC, Patch

HOST = '00003'                    # the item and thumbnail templates: deck_gen_graphic_00003's
SHARED = 'win32/characters/customization/configs/cas_main_sharedbundle'
BRT = 'characters/customization/configs/cas_main_bundlereftable'
MASTER = 'items/itemmastercollection'
SLOT = '2259d419-387f-b1ba-30fc-c5140100b012'   # param_graphic_c
DEFAULTS = 'characters/skateboard/defaults/deck/common/generic/graphic/deck_gen_graphic_default'
# Other slots a deck can fill (mixins.py), by ParamKey GUID. Their templates are the host's textures in
# those slots (same size and mip chain as the art; BC7, linear).
SLOTS = {
    'osk': 'c1b71b8a-395f-e8ce-30fc-c5140100a381',   # param_graphic_osk: the mask
    'ny': '830b93ff-7cad-e8ee-30fc-c5140100c668',    # param_graphic_ny: the normal map
    'rgb': '698a46f9-c8de-d48e-30fc-c5140100a381',   # param_graphic_rgb: red where holo (holo decks only)
}
# Host decks: the stock deck whose bundle new decks are added to, and whose appearance preset they are
# cloned from. Plain decks: deck_gen_graphic_00003 (untouched by the installed mods). Decks with their
# own mask: the gold deck deck_gen_graphic_00041, whose preset sets param_graphic_metalmaskvalue 1.0
# (without it the shader ignores the mask's metal channel: foil built on the plain host looked plain in
# game, 2026-10-20). Holo and shader-effect decks: the Primitive holo deck, whose preset carries the holo
# parameters and the holo map slot (as the community's holo packs do). Other mods add decks to the
# metal and holo hosts' bundles too; ReSkate combines them.
HOSTS = {
    'plain': 'characters/skateboard/unlicensed/deck/common/generic/graphic/deck_gen_graphic_00003/2023/deck_gen_graphic_00003',
    'metal': 'characters/skateboard/unlicensed/deck/common/generic/graphic/deck_gen_graphic_00041/deck_gen_graphic_00041',
    'holo': 'characters/skateboard/licensed/deck/common/primitive/graphic/deck_primitive_graphic_00001/2026/deck_primitive_graphic_00001',
}
# The deck shader's holo patterns (param_foilmode): enumerators of common/shadercomponents/configuration/
# enum/foilmode, chosen by the preset's import of one of them. The holo host's preset imports Pearl.
FOIL_MODES = {
    'Cellular': '5ecf14cd-091a-41c0-81f6-9caaff2b1c9c',
    'Frost': 'e04615d5-ea8d-416e-acc5-a3b1b9f989e8',
    'Pearl': 'b7d407a4-b44c-42e2-b91e-877d817c872d',
    'Glitter': '77c5e80f-6036-48b5-afe4-319772124a52',
    'Vertical': '7a67636f-9767-473b-95c7-b174736504ad',
}
HOST_FOIL_MODE = 'Pearl'
PREFIX = 'characters/customization/reskate/board_bottomart/'
MIP1, MIP2 = 0x100000, 0x140000  # logical starts of mips 1 and 2 of a 512 x 2048 BC7 chain
LIGHT_DECKS = 40
SKIP_LIGHT = {'00041', '00042'}
# The game's all-metal decks (mask B 255 everywhere; found by surveying every deck's mask): their
# thumbnails give the light of a metal deck, which mirrors its surroundings tinted by the art.
METAL_DECKS = ('gen_00041', 'gen_00042', 'chocolate_00001', 'nhs_00013')


def djb32(text):
    """HashedAssetKey (of Key) and NameHash (of Name): djb2 with xor, 32-bit, case as written."""
    h = 5381
    for c in text.encode():
        h = ((h * 33) & 0xFFFFFFFF) ^ c
    return h


def owner_hash(name):
    """64-bit djb2-xor of the lower-case name: chunk metadata h64 and texture header bytes 120-127."""
    h = 5381
    for c in name.lower().encode():
        h = ((h * 33) & 0xFFFFFFFFFFFFFFFF) ^ c
    return h


def new_chunk_guid():
    """A chunk GUID with the low bit of its last (little-endian) byte set: compressed blocks."""
    b = bytearray(uuid.uuid4().bytes_le)
    b[15] |= 1
    return str(uuid.UUID(bytes_le=bytes(b)))


class DeckPatch(Patch):
    """A Patch that also adds assets to bundles and rewrites their manifests."""

    def __init__(self, game):
        super().__init__(game)
        self.rids = set()

    def bundle(self, name):
        if name not in self.bundles:
            b = self.game.bundle(name)
            m = b['manifest']
            b['manifest'] = dict(m, ebx=[dict(a) for a in m['ebx']], res=[dict(a) for a in m['res']],
                                 chunks=[dict(a) for a in m['chunks']])
            b['files'] = [dict(f) for f in b['files']]
            self.bundles[name] = b
        return self.bundles[name]

    def _put(self, data):
        enc = fbw.encode_cas(data, self.game.root)
        return enc, dict(LOC, offset=self._append(enc), size=len(enc))

    def new_rid(self):
        """A new resource id: random, and ODD, like every resource id the game ships (24,694 of 24,694).
        The game keeps a resource's slot as its id until loaded, then a tagged pointer, and tells them
        apart by the low bits: an id ending in binary 10 was taken for a pointer and crashed the game at
        boot (2026-10-20)."""
        while True:
            rid = int.from_bytes(os.urandom(8), 'little') | 1
            if rid and rid not in self.rids:
                self.rids.add(rid)
                return rid

    def add_ebx(self, bundle, name, data):
        b = self.bundle(bundle)
        m = b['manifest']
        enc, loc = self._put(data)
        b['files'].insert(1 + len(m['ebx']), loc)
        m['ebx'].append({'name': name, 'size': len(data), 'sha1': hashlib.sha1(enc).hexdigest()})

    def add_res(self, bundle, name, rtype, meta, rid, data):
        b = self.bundle(bundle)
        m = b['manifest']
        enc, loc = self._put(data)
        b['files'].insert(1 + len(m['ebx']) + len(m['res']), loc)
        m['res'].append({'name': name, 'size': len(data), 'sha1': hashlib.sha1(enc).hexdigest(), 'type': rtype,
                         'meta': bytes(meta), 'rid': rid})

    def add_chunk(self, bundle, guid, enc, record=None):
        """A chunk already encoded: in the TOC always, and in `bundle` (whole) when given."""
        at = self._append(enc)
        self.chunks[guid] = dict(LOC, guid=guid, offset=at, size=len(enc))
        if bundle is None:
            return
        b = self.bundle(bundle)
        m = b['manifest']
        decoded = sum(struct.unpack_from('>I', enc, p)[0] & 0xFFFFFF for p in _block_starts(enc))
        b['files'].append(dict(LOC, offset=at, size=len(enc)))
        m['chunks'].append({'guid': guid, 'logical_offset': 0, 'logical_size': decoded,
                            'sha1': hashlib.sha1(enc).hexdigest()})
        if record is not None:
            root, end = ndb.read(m['chunk_meta'])
            assert end == len(m['chunk_meta'])
            root.children.insert(len(m['chunks']) - 1, record)
            m['chunk_meta'] = ndb.write(root)

    def replace_ebx(self, bundle, name, data):
        b = self.bundle(bundle)
        m = b['manifest']
        i = next(i for i, e in enumerate(m['ebx']) if e['name'] == name)
        enc, loc = self._put(data)
        b['files'][1 + i] = loc
        m['ebx'][i].update(size=len(data), sha1=hashlib.sha1(enc).hexdigest())

    def replace_res(self, bundle, name, data, meta):
        b = self.bundle(bundle)
        m = b['manifest']
        i = next(i for i, r in enumerate(m['res']) if r['name'] == name)
        enc, loc = self._put(data)
        b['files'][1 + len(m['ebx']) + i] = loc
        m['res'][i].update(size=len(data), sha1=hashlib.sha1(enc).hexdigest(), meta=bytes(meta))

    def write(self, *args, **kwargs):
        for b in self.bundles.values():
            b['manifest_raw'] = bytearray(fbw.write_binary_bundle(b['manifest']))
        return super().write(*args, **kwargs)


def _block_starts(enc):
    at = 0
    while at + 8 <= len(enc):
        yield at
        at += 8 + struct.unpack_from('>H', enc, at + 6)[0] + ((enc[at + 5] & 0xF) << 16)


def _ebx(game, bundle, name):
    i = next(i for i, e in enumerate(bundle['manifest']['ebx']) if e['name'] == name)
    return game.decode(bundle['files'][1 + i])


def _res(game, bundle, name):
    m = bundle['manifest']
    i = next(i for i, r in enumerate(m['res']) if r['name'] == name)
    return game.decode(bundle['files'][1 + len(m['ebx']) + i]), m['res'][i]


def _clone(data, name):
    """A template EBX as a new document: new file and instance GUIDs, renamed."""
    doc = ebx.read_document(data)
    doc.set_file_guid(ebx.Guid.new())
    for i in range(len(doc.instances)):
        doc.set_instance_guid(i, ebx.Guid.new())
    doc.rename(name)
    return doc


def _write(doc):
    return ebx.write_document(doc, preserve_layout=False)


def _ref(doc):
    """(file guid, root instance guid): how another document imports this one."""
    return doc.file_guid, doc.instances[0].instance_guid


def mips_bc7(art, levels=12, normal=False):
    """The full mip chain, BC7. A normal map's smaller mips are made unit length again."""
    out = []
    for m in range(levels):
        level = art if m == 0 else art.resize((max(1, art.width >> m), max(1, art.height >> m)), Image.LANCZOS)
        px = np.asarray(level)
        if normal and m:
            n = px.astype(np.float64) / 127.5 - 1
            n /= np.maximum(np.linalg.norm(n, axis=2, keepdims=True), 1e-6)
            px = np.clip(np.round((n + 1) * 127.5), 0, 255).astype(np.uint8)
        out.append(bc7.encode(px))
    return b''.join(out)


def default_layers(game):
    """The game's default mask and normal map for decks (PIL images): what mixins start from."""
    host = game.bundle(_host_bundle('plain'))
    return game.texture_image(host, f'{DEFAULTS}_osk'), game.texture_image(host, f'{DEFAULTS}_ny')


def _host_bundle(kind):
    return f'win32/{HOSTS[kind]}_ap_cas_main_bundlereftable'


def host_textures(kind):
    """{slot: the host deck's texture in that slot}: the templates of a new deck's textures."""
    stock = HOSTS[kind]
    if kind == 'plain':
        return {'art': f'{stock}_c', 'osk': f'{DEFAULTS}_osk', 'ny': f'{DEFAULTS}_ny'}
    if kind == 'metal':
        return {'art': f'{stock}_c', 'osk': f'{stock}_osk', 'ny': f'{DEFAULTS}_ny'}
    return {'art': f'{stock}_c', 'osk': f'{stock}_osk', 'ny': f'{stock}_ny', 'rgb': f'{stock}_rgb'}


def host_kind(layers):
    """Which host a deck with these layers needs: holo (a holo map), metal (its own mask), or plain."""
    if 'rgb' in layers:
        return 'holo'
    return 'metal' if 'osk' in layers else 'plain'


def _load_host(game, kind):
    """(bundle name, appearance preset EBX, {slot: texture template}) of a host deck."""
    name = _host_bundle(kind)
    bundle = game.bundle(name)
    # The host bundle's chunk-metadata records for its own textures: the shape new records copy.
    records = ndb.read(bundle['manifest']['chunk_meta'])[0].children
    textures = {slot: _template(game, bundle, tex, records) for slot, tex in host_textures(kind).items()}
    return name, _ebx(game, bundle, f'{HOSTS[kind]}_ap'), textures


def _template(game, host, name, records):
    """What a new texture is cloned from: (EBX bytes, EBX document, resource header, resource entry,
    chunk-metadata record) of the host bundle's texture `name`."""
    data = _ebx(game, host, name)
    hdr, res = _res(game, host, name)
    record = next(r for r in records if r.field('h64') is not None and
                  struct.unpack('<Q', r.field('h64').payload)[0] == owner_hash(name))
    return data, ebx.read_document(data), hdr, res, record


def _add_texture(patch, host_name, template, name, pixels):
    """A new texture in the host bundle: whole, Kraken, header offsets and hash. Returns its document."""
    data, _, t_hdr, t_res, t_record = template
    enc = fbw.encode_cas(pixels, patch.game.root)
    chunk = new_chunk_guid()
    hdr = bytearray(t_hdr)
    struct.pack_into('<II', hdr, 0, fbw.block_offset(enc, MIP1), fbw.block_offset(enc, MIP2))
    hdr[0x1F] = hdr[0x20] = 0
    hdr[40:56] = uuid.UUID(chunk).bytes_le
    struct.pack_into('<Q', hdr, 120, owner_hash(name))
    rid = patch.new_rid()
    tex = _clone(data, name)
    tex.set('Resource', ebx.ResourceRef(rid))
    patch.add_ebx(host_name, name, _write(tex))
    patch.add_res(host_name, name, t_res['type'], t_res['meta'], rid, bytes(hdr))
    record = copy.deepcopy(t_record)
    record.field('h64').payload = struct.pack('<Q', owner_hash(name))
    record.field('meta').field('firstMip').payload = struct.pack('<i', 0)
    patch.add_chunk(host_name, chunk, enc, record)
    return tex


def thumbnail_lighting(game, log=print):
    """{size: (light, sheen, alpha, metal light)} fitted from stock decks' art and thumbnails (as
    build_replace.py). Kept in %LOCALAPPDATA%\\Caveman\\cache, keyed by the Skate.exe it was fitted from:
    it only changes with the game, and the fit is about half a build."""
    exe = os.path.join(game.root, 'Skate.exe')
    st = os.stat(exe)
    cache_dir = os.path.join(os.environ.get('LOCALAPPDATA', game.root), 'Caveman', 'cache')
    cache = os.path.join(cache_dir, f'thumbnail-lighting-{st.st_size}-{int(st.st_mtime)}-{LIGHT_DECKS}-metal{len(METAL_DECKS)}grey.npz')
    keys = ('light', 'sheen', 'alpha', 'metal')
    try:
        with np.load(cache) as z:
            out = {size: tuple(z[f'{k}{size}'] for k in keys) for size in (768, 256)}
        log('  thumbnail lighting from the cache')
        return out
    except (OSError, KeyError, ValueError):
        pass
    out = _fit_thumbnail_lighting(game, log)
    try:
        os.makedirs(cache_dir, exist_ok=True)
        np.savez_compressed(cache, **{f'{k}{size}': v for size, values in out.items() for k, v in zip(keys, values)})
    except OSError as error:
        log(f'  note: could not cache the thumbnail lighting ({error})')
    return out


def _fit_thumbnail_lighting(game, log=print):
    shared = game.bundle(SHARED)
    names = {r['name'] for r in shared['manifest']['res']}
    candidates = []
    for b in sorted(game.toc()['bundle_by_name']):
        if '/deck/common/generic/graphic/deck_gen_graphic_' in b and b.endswith('_ap_cas_main_bundlereftable'):
            n = b.split('deck_gen_graphic_')[-1].split('_')[0]
            if n not in SKIP_LIGHT and {f'thumbnail/tool/own_deckgraphic_gen_popsicle_{n}{s}' for s in ('', '_lrg')} <= names:
                candidates.append(n)
    training = candidates[::max(1, len(candidates) // LIGHT_DECKS)][:LIGHT_DECKS]
    arts = []
    for n in training:
        b = game.bundle(game.find_bundle(f'/deck_gen_graphic_{n}_ap_cas_main_bundlereftable'))
        arts.append(game.texture_image(b, next(r['name'] for r in b['manifest']['res'] if r['name'].endswith(f'/deck_gen_graphic_{n}_c'))))
    metal_arts = []
    for d in METAL_DECKS:
        brand, n = d.split('_')
        b = game.bundle(game.find_bundle(f'/deck_{brand}_graphic_{n}_ap_cas_main_bundlereftable'))
        metal_arts.append(game.texture_image(b, next(r['name'] for r in b['manifest']['res'] if r['name'].endswith(f'/deck_{brand}_graphic_{n}_c'))))
    out = {}
    for suffix, size in (('_lrg', 768), ('', 256)):
        pairs = [(a, game.texture_image(shared, f'thumbnail/tool/own_deckgraphic_gen_popsicle_{n}{suffix}'))
                 for a, n in zip(arts, training)]
        metal_pairs = [(a, game.texture_image(shared, f'thumbnail/tool/own_deckgraphic_{d.split("_")[0]}_popsicle_{d.split("_")[1]}{suffix}'))
                       for a, d in zip(metal_arts, METAL_DECKS)]
        out[size] = thumbs.fit_light(pairs, size) + (thumbs.fit_metal(metal_pairs, size),)
    log(f'  thumbnail lighting fitted from {len(training)} stock decks and {len(METAL_DECKS)} metal ones')
    return out


def build(game, decks, mods_dir, mod_name, log=print):
    """decks: [(display name, item key 'Own_...', 512 x 2048 RGB art[, layers])]; `layers` (mixins.py)
    maps slots ('osk', 'ny', 'rgb': SLOTS) to 2048 x 512 RGB arrays for the deck's own textures,
    'foilmode' to a FOIL_MODES name for a holo deck, 'aura' to an (r, g, b) colour for a light and
    trails on the board, or 'nyan' to a step count for a solid rainbow trail (vfx.py; needs the patched ribbon
    program, nyanpatch.py). Writes <mods_dir>/<mod_name>.
    Returns (stats, [(display, base name)])."""
    patch = DeckPatch(game)
    hosts = {}
    shared = game.bundle(SHARED)
    t_item = _ebx(game, shared, f'items/cust_board/own_deck_gen_popsiclegraphic_{HOST}')
    t_thumb = {s: _ebx(game, shared, f'thumbnail/tool/own_deckgraphic_gen_popsicle_{HOST}{s}') for s in ('', '_lrg')}
    t_thumb_res = {s: _res(game, shared, f'thumbnail/tool/own_deckgraphic_gen_popsicle_{HOST}{s}') for s in ('', '_lrg')}
    t_cdn = {s: _ebx(game, shared, f'thumbnail/cdn/img_own_deckgraphic_gen_popsicle_{HOST}{s}') for s in ('', '_lrg')}
    t_item_doc = ebx.read_document(t_item)
    t_cdn_refs = {s: _ref(ebx.read_document(t_cdn[s])) for s in ('', '_lrg')}
    t_thumb_refs = {s: _ref(ebx.read_document(t_thumb[s])) for s in ('', '_lrg')}
    lighting = thumbnail_lighting(game, log)
    master = ebx.read_document(_ebx(game, shared, MASTER))
    items_field = master.instances[0].object.find('Items')
    presets = []
    built = []
    vfx_templates = None
    states, added_states = None, set()
    for display, key, art, *rest in decks:
        layers = rest[0] if rest else {}
        kind = host_kind(layers)
        if kind not in hosts:
            hosts[kind] = _load_host(game, kind)
        host_name, t_ap, templates = hosts[kind]
        base = key[len('Own_'):].lower()
        ap_name = f'{PREFIX}{base}_ap'
        item_name = f'items/board_bottomart/own_{base}'
        # ---- the board texture, and the deck's own mask, normal map and holo map when it has them
        textures = [('art', _add_texture(patch, host_name, templates['art'], f'{PREFIX}{base}_role3_{SLOT}', mips_bc7(art.convert('RGB'))))]
        for slot, param in SLOTS.items():
            if slot in layers:
                pixels = mips_bc7(Image.fromarray(layers[slot]), normal=slot == 'ny')
                textures.append((slot, _add_texture(patch, host_name, templates[slot], f'{PREFIX}{base}_role3_{param}', pixels)))
        remap = {}
        for slot, tex_doc in textures:
            t_doc = templates[slot][1]
            remap[t_doc.file_guid] = tex_doc.file_guid
            remap[t_doc.instances[0].instance_guid] = tex_doc.instances[0].instance_guid
        if 'foilmode' in layers:
            remap[ebx.Guid(FOIL_MODES[HOST_FOIL_MODE])] = ebx.Guid(FOIL_MODES[layers['foilmode']])
        # ---- the appearance preset: the host's, its slots pointed at the new texture assets
        ap = _clone(t_ap, ap_name)
        ap.remap_imports(remap)
        patch.add_ebx(host_name, ap_name, _write(ap))
        presets.append((ap_name, host_name))
        # ---- thumbnails: rendered on the board, textures with TOC-only chunks, and CDN records
        metal = Image.fromarray(layers['osk'][..., 2]) if 'osk' in layers else None
        cdn_refs = {}
        for suffix, size in (('', 256), ('_lrg', 768)):
            light, sheen, alpha, metal_light = lighting[size]
            img = thumbs.render(art, light, sheen, alpha, metal, metal_light)
            th_name = f'thumbnail/tool/own_{base}{suffix}'
            tpix = bc7.encode(np.asarray(img))
            th_hdr, th_res = t_thumb_res[suffix]
            tenc = fbw.encode_cas(tpix, game.root)
            tchunk = new_chunk_guid()
            h2 = bytearray(th_hdr)
            h2[40:56] = uuid.UUID(tchunk).bytes_le
            struct.pack_into('<Q', h2, 120, owner_hash(th_name))
            trid = patch.new_rid()
            th = _clone(t_thumb[suffix], th_name)
            th.set('Resource', ebx.ResourceRef(trid))
            patch.add_ebx(SHARED, th_name, _write(th))
            patch.add_res(SHARED, th_name, th_res['type'], th_res['meta'], trid, bytes(h2))
            patch.add_chunk(None, tchunk, tenc)
            cdn_name = f'thumbnail/cdn/img_own_{base}{suffix}'
            cdn = _clone(t_cdn[suffix], cdn_name)
            cdn.remap_imports({t_thumb_refs[suffix][0]: th.file_guid, t_thumb_refs[suffix][1]: th.instances[0].instance_guid})
            cdn.set('NameHash', djb32(cdn_name))
            patch.add_ebx(SHARED, cdn_name, _write(cdn))
            cdn_refs[suffix] = _ref(cdn)
        # ---- the item
        item = _clone(t_item, item_name)
        item.set('Key', key)
        item.set('HashedAssetKey', djb32(key))
        mapping = {}
        for s in ('', '_lrg'):
            mapping[t_cdn_refs[s][0]], mapping[t_cdn_refs[s][1]] = cdn_refs[s]
        item.remap_imports(mapping)
        data_index = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
        item.set('AssetPaths[0].AssetName', f'{base}_ap', data_index)
        style = next((s for s in ('nyan', 'aura', 'burst') if s in layers), None)   # one effect asset per item
        if style:
            # ---- board effects (vfx.py): an aura (a light and trails), a nyan rainbow trail or a stock burst, in the
            # shared bundle; optionally fired by a game state (statefx.py) instead of always on
            if vfx_templates is None:
                vfx_templates = (_ebx(game, shared, vfx.BLUEPRINT), _ebx(game, shared, vfx.ASSET))
            bp_name, va_name = f'{vfx.PREFIX}ebp_{base}_{style}', f'{vfx.PREFIX}{base}_{style}'
            if style == 'burst':   # layers['burst']: a statefx.BURSTS name; the game's own blueprint, not a copy
                bp_ref = _ref(ebx.read_document(_ebx(game, shared, statefx.BURSTS[layers['burst']])))
            else:
                bp = _clone(vfx_templates[0], bp_name)
                if style == 'nyan':   # layers['nyan']: steps, or (steps, trigger) for diagnostics (vfx.nyan_colour)
                    nyan = layers['nyan'] if isinstance(layers['nyan'], tuple) else (layers['nyan'],)
                    vfx.nyan_blueprint(bp, *nyan)
                else:
                    vfx.blueprint(bp, layers['aura'])
                patch.add_ebx(SHARED, bp_name, _write(bp))
                bp_ref = _ref(bp)
            va = _clone(vfx_templates[1], va_name)
            vfx.asset(va, bp_ref, vfx.POINTS if style == 'aura' else vfx.NYAN_POINTS)
            if 'trigger' in layers:
                # (state, usage WHILE/ONCE, cooldown seconds[, copy]): the state a statefx.STATES name or a full asset
                # name; copy (default True) also puts a byte copy of the state into the shared bundle. The first test
                # with copies never fired (2026-10-21), so the second one references the game's states alone.
                # an optional fifth element: a cooldown state (statefx.COOLDOWN_STATES name) for CooldownType 1
                state, usage, cooldown, *rest = layers['trigger']
                copy = rest[0] if rest else True
                cooldown_state = rest[1] if len(rest) > 1 else None
                if states is None:
                    states = statefx.States(game)
                state_name = statefx.STATES.get(state, state)
                if copy and state_name not in added_states:
                    patch.add_ebx(SHARED, state_name, states.ebx(state_name)[0])
                    added_states.add(state_name)
                cooldown_ref = states.ref(statefx.COOLDOWN_STATES[cooldown_state]) if cooldown_state else None
                statefx.trigger(va, states.ref(state_name), usage, cooldown, cooldown_ref)
            patch.add_ebx(SHARED, va_name, _write(va))
            vfx.attach(item, data_index, _ref(va))
        patch.add_ebx(SHARED, item_name, _write(item))
        master.imports.append(_ref(item))
        items_field.value.data.append(ebx.Value(ebx.Pointer('external', len(master.imports) - 1)))
        built.append((display, base))
        log(f'  built {display} ({key})')
    patch.replace_ebx(SHARED, MASTER, _write(master))
    data, meta = _res(game, shared, BRT)
    resource, new_meta = brt.add_presets(brt.Table(data, meta['meta']), [(p.lower(), h) for p, h in presets])
    patch.replace_res(SHARED, BRT, resource, new_meta)
    icon = Image.new('RGBA', (256, 256), (0, 0, 0, 0))
    if decks:
        icon.paste(thumbs.render(decks[0][2], *lighting[256][:3]), (0, 0))
    names = ', '.join(d for d, _ in built)
    stats = patch.write(os.path.join(mods_dir, mod_name), mod_name,
                        f'Decks built by Caveman from PNGs: {names}.', icon=icon,
                        readme=f'# {mod_name}\n\nDecks built by Caveman from PNGs:\n\n' + '\n'.join(f'- {d}' for d, _ in built) + '\n')
    return stats, built
