"""New wheels: a square face graphic and a colour become a graphic-wheel item, laid out the way ReSkate
Studio's wheel packs are, with every template cloned from the installed game (knowledge/wheels.md).

Per wheel, in the host wheel's bundle (the donut wheel wheel_gen_graphic_00006, untouched by the installed
mods):
  characters/customization/reskate/board_wheelcolor/<base>_ap            appearance preset (host's, re-pointed,
                                                                         param_region_color_0 = the colour)
  characters/customization/reskate/board_wheelcolor/<base>_role3_<slot>  art (BC7) and mask (BC1), whole
In cas_main_sharedbundle: items/board_wheelcolor/own_<base>, its two thumbnails and two CDN records; once per
mod, items/itemmastercollection and the bundle reference table (newdeck.py does the same for decks).

Texture layout (512 x 512): the outer face is the top half, a 512 x 256 ellipse (the face graphic squashed
2:1), the inner face the bottom half. Mask R is where the graphic shows; elsewhere the wheel colour
(param_region_color_0, linear rgb) shows. G and B are copied from the host's mask."""
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
import thumbs
from newdeck import (DeckPatch, SHARED, BRT, MASTER, SLOT, _clone, _ebx, _ref, _res, _template, _write,
                     djb32, new_chunk_guid, owner_hash)

HOST = 'characters/skateboard/unlicensed/wheel/common/classic/wheelgraphic/wheel_gen_graphic_00006/2024/wheel_gen_graphic_00006'
HOST_ITEM = 'items/boardwheelcolors0/own_wheelgraphic_gen_classic00006'
HOST_THUMB = 'own_wheelgraphic_gen_classic00006'
PREFIX = 'characters/customization/reskate/board_wheelcolor/'      # Studio's wheel packs use these paths
ITEMS = 'items/board_wheelcolor/'
OSK = 'c1b71b8a-395f-e8ce-30fc-c5140100a381'      # param_graphic_osk
REGION_COLOUR = '4b18a9e2-cd35-8c46-59ff-5e880100548c'   # param_region_color_0: three linear floats + u32 1
LEVELS = 10                                       # 512 x 512 down to 1 x 1
# Menu thumbnails: the wheel face-on, a disc at (378.5, 383.5) of radius 335.5 in the 768 px thumbnail;
# the face ellipse maps onto it with no flip at scale FACE_SCALE (its outer 18% wraps over the wheel's
# rounded edge). Fitted on 65 stock wheels (correlation 0.76).
DISC = (378.5, 383.5, 335.5)
FACE_SCALE = 1.22
# The bearing hides the disc's inner 35% (across 50 stock thumbnails the centre does not vary out to 0.33 of
# the disc radius, then the core ring to about 0.38, then the art); so of the square face graphic, the
# visible face is the circle of radius 1/1.22 of its half-width and the hub 0.35 of that.
HUB = 0.35
LIGHT_RIDGE, LIGHT_SMOOTH, LIGHT_EDGE = 60.0, 6.0, 6


# ---------------------------------------------------------------- colours
def srgb_to_linear(c):
    c = np.asarray(c, np.float64) / 255
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(c):
    c = np.clip(np.asarray(c, np.float64), 0, 1)
    return np.round(255 * np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055))


def parse_hex(text):
    """'ff0000' -> (255, 0, 0), or None when `text` is not six hex digits."""
    if len(text) == 6 and all(ch in '0123456789abcdefABCDEF' for ch in text):
        return tuple(int(text[i:i + 2], 16) for i in (0, 2, 4))
    return None


def _param_records(doc):
    """{ParamKey GUID text: the boxed value record} of an appearance preset's parameters."""
    records = {r.offset: r for r in doc.boxed_values}
    out = {}
    for v in doc.instances[0].object.find('ParamInfo').value.data:
        info = v.data
        key = str(info.find('ParamKey').value.data.find('Data').value.data)
        boxed = info.find('Value').value.data.find('Value').value.data
        if isinstance(boxed, ebx.BoxedRef) and boxed.data_offset in records:
            out[key] = records[boxed.data_offset]
    return out


def region_colour(doc):
    """The preset's wheel colour (linear rgb floats), or None."""
    rec = _param_records(doc).get(REGION_COLOUR)
    return None if rec is None else struct.unpack_from('<3f', rec.raw_bytes)


def set_region_colour(doc, linear):
    rec = _param_records(doc)[REGION_COLOUR]
    raw = bytearray(rec.raw_bytes)
    struct.pack_into('<3f', raw, 0, *(float(x) for x in linear))
    rec.raw_bytes = bytes(raw)


# ---------------------------------------------------------------- textures
def face_square(img):
    """The face graphic as a square RGBA image (centre crop)."""
    img = img.convert('RGBA')
    side = min(img.size)
    left, top = (img.width - side) // 2, (img.height - side) // 2
    return img.crop((left, top, left + side, top + side))


def wheel_textures(face, colour, host_mask):
    """(art RGB, mask RGB) 512 x 512 arrays: the face squashed into the top half over the wheel colour;
    mask R = the face's alpha (0 in the bottom half), G and B the host mask's."""
    squashed = np.asarray(face_square(face).resize((512, 256), Image.LANCZOS)).astype(np.float64)
    a = squashed[..., 3:] / 255
    base = np.array(colour, np.float64)
    art = np.empty((512, 512, 3))
    art[:256] = squashed[..., :3] * a + base * (1 - a)
    art[256:] = base
    mask = np.asarray(host_mask.convert('RGB')).copy()
    mask[:256, :, 0] = np.round(squashed[..., 3])
    mask[256:, :, 0] = 0
    return np.clip(np.round(art), 0, 255).astype(np.uint8), mask


def mips(img, encode, levels=LEVELS):
    out = []
    for m in range(levels):
        level = img if m == 0 else img.resize((max(1, img.width >> m), max(1, img.height >> m)), Image.LANCZOS)
        out.append(encode(np.asarray(level)))
    return b''.join(out)


def block_containing(encoded, logical):
    """Byte offset in `encoded` of the CAS block whose decoded range holds `logical`. A texture header's
    bytes 0-7 are these offsets for mips 1 and 2: in the game's own BC1 wheel masks (512 x 512) mip 2
    starts at 0x28000, inside the block that starts at mip 1, and both offsets name that block."""
    at = decoded = 0
    while at + 8 <= len(encoded):
        d = struct.unpack_from('>I', encoded, at)[0] & 0xFFFFFF
        if decoded <= logical < decoded + d:
            return at
        comp = encoded[at + 4] | (encoded[at + 5] << 8)
        at += 8 + struct.unpack_from('>H', encoded, at + 6)[0] + (((comp >> 8) & 0xF) << 16)
        decoded += d
    raise ValueError(f'no block holds {logical:#x}')


def add_texture(patch, host_name, template, name, pixels):
    """A new texture in the host bundle: whole (first mip 0), Kraken, header offsets of mips 1 and 2 (from
    the template's own mip sizes, so any size and format), name hash; chunk-metadata record firstMip 0."""
    data, _, t_hdr, t_res, t_record = template
    h = fb.texture_header(t_hdr, t_res['meta'])
    assert len(pixels) == h['chunk_size'], (name, len(pixels), h['chunk_size'])
    enc = fbw.encode_cas(pixels, patch.game.root)
    chunk = new_chunk_guid()
    hdr = bytearray(t_hdr)
    mip1 = h['mip_sizes'][0]
    struct.pack_into('<II', hdr, 0, block_containing(enc, mip1), block_containing(enc, mip1 + h['mip_sizes'][1]))
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


# ---------------------------------------------------------------- thumbnails
def map_wheel(tex, size):
    """A 512 x 512 wheel texture (RGB) as it lands on a `size` x `size` thumbnail (float RGB): the top
    half's ellipse on the disc, sampled bilinearly at 768 px and averaged down for smaller sizes."""
    cx, cy, r = DISC
    yy, xx = np.mgrid[0:768, 0:768].astype(np.float64)
    ax = np.clip(256 + (xx - cx) / r / FACE_SCALE * 256, 0, 511)
    ay = np.clip(128 + (yy - cy) / r / FACE_SCALE * 128, 0, 255)
    t = np.asarray(tex, np.float64)[:257]
    x0, y0 = np.floor(ax).astype(int), np.floor(ay).astype(int)
    x1, y1 = np.minimum(x0 + 1, 511), np.minimum(y0 + 1, 256)
    fx, fy = (ax - x0)[..., None], (ay - y0)[..., None]
    out = (t[y0, x0] * (1 - fx) + t[y0, x1] * fx) * (1 - fy) + (t[y1, x0] * (1 - fx) + t[y1, x1] * fx) * fy
    if size != 768:
        k = 768 // size
        out = out.reshape(size, k, size, k, 3).mean((1, 3))
    return out


def fit_wheel_light(pairs, size):
    """Light (multiplied) and sheen (added) per pixel, fitted over stock wheels as thumbs.fit_light is
    for decks: `pairs` [(512 x 512 RGB texture as seen in game, thumbnail image of `size`)]. Returns
    (light, sheen, median alpha)."""
    n = 0
    sa = sr = saa = sar = 0
    alphas = []
    for tex, thumb in pairs:
        a = map_wheel(tex, size)
        rgba = np.asarray(thumb.convert('RGBA')).astype(np.float64)
        r = rgba[..., :3]
        alphas.append(rgba[..., 3])
        sa, sr, saa, sar = sa + a, sr + r, saa + a * a, sar + a * r
        n += 1
    ma, mr = sa / n, sr / n
    light = (sar / n - ma * mr) / (saa / n - ma * ma + LIGHT_RIDGE)
    alpha = np.median(np.stack(alphas), 0)
    light = thumbs._smooth_inside(light, alpha, radius=LIGHT_SMOOTH * size / 768, keep_edge=max(2, round(LIGHT_EDGE * size / 768)))
    return light, mr - light * ma, alpha


def render_thumbnail(tex, light, sheen, alpha):
    rgb = np.clip(map_wheel(tex, alpha.shape[0]) * light + sheen, 0, 255)
    rgb[alpha < 1] = 0   # the game's thumbnails are black where transparent
    return Image.fromarray(np.dstack([rgb, alpha]).astype(np.uint8), 'RGBA')


def _stock_pairs(game, log):
    """[(texture as seen in game, {size: thumbnail})] of the stock graphic wheels: each wheel's art over
    its own colour where its mask's R says the graphic does not show."""
    shared = game.bundle(SHARED)
    names = {r['name'] for r in shared['manifest']['res']}
    out = []
    for n in sorted(game.toc()['bundle_by_name']):
        if '/wheelgraphic/wheel_gen_graphic_' not in n or not n.endswith('_ap_cas_main_bundlereftable'):
            continue
        num = n.split('wheel_gen_graphic_')[1][:5]
        thumb = next((t for t in (f'thumbnail/tool/own_wheelgraphic_gen_classic_{num}', f'thumbnail/tool/own_wheelgraphic_gen_classic{num}')
                      if t in names and f'{t}_lrg' in names), None)
        if thumb is None:
            continue
        b = game.bundle(n)
        res = [r['name'] for r in b['manifest']['res']]
        c = next((r for r in res if r.endswith(f'wheel_gen_graphic_{num}_c')), None)
        o = next((r for r in res if r.endswith(f'wheel_gen_graphic_{num}_osk')), None)
        ap = next((e['name'] for e in b['manifest']['ebx'] if e['name'].endswith(f'wheel_gen_graphic_{num}_ap')), None)
        if not (c and o and ap):
            continue
        art = np.asarray(game.texture_image(b, c).convert('RGB')).astype(np.float64)
        mask = np.asarray(game.texture_image(b, o).convert('RGB')).astype(np.float64)
        if art.shape[:2] != (512, 512) or mask.shape[:2] != (512, 512):
            continue
        colour = region_colour(ebx.read_document(_ebx(game, b, ap)))
        if colour is None:
            continue
        k = mask[..., :1] / 255
        seen = art * k + linear_to_srgb(colour) * (1 - k)
        out.append((seen, {768: game.texture_image(shared, f'{thumb}_lrg'), 256: game.texture_image(shared, thumb)}))
    log(f'  wheel thumbnail lighting: {len(out)} stock wheels')
    return out


def thumbnail_lighting(game, log=print):
    """{size: (light, sheen, alpha)} for wheel thumbnails, cached in %LOCALAPPDATA%\\Caveman\\cache by the
    Skate.exe it was fitted from (as newdeck.thumbnail_lighting)."""
    st = os.stat(os.path.join(game.root, 'Skate.exe'))
    cache_dir = os.path.join(os.environ.get('LOCALAPPDATA', game.root), 'Caveman', 'cache')
    cache = os.path.join(cache_dir, f'wheel-thumbnail-lighting-{st.st_size}-{int(st.st_mtime)}-{FACE_SCALE}.npz')
    keys = ('light', 'sheen', 'alpha')
    try:
        with np.load(cache) as z:
            out = {size: tuple(z[f'{k}{size}'] for k in keys) for size in (768, 256)}
        log('  wheel thumbnail lighting from the cache')
        return out
    except (OSError, KeyError, ValueError):
        pass
    pairs = _stock_pairs(game, log)
    out = {size: fit_wheel_light([(t, th[size]) for t, th in pairs], size) for size in (768, 256)}
    try:
        os.makedirs(cache_dir, exist_ok=True)
        np.savez_compressed(cache, **{f'{k}{size}': v for size, values in out.items() for k, v in zip(keys, values)})
    except OSError as error:
        log(f'  note: could not cache the wheel thumbnail lighting ({error})')
    return out


# ---------------------------------------------------------------- building
class Templates:
    """Everything a new wheel is cloned from, read once from the player's install."""

    def __init__(self, game, log=print):
        self.game = game
        self.host_name = f'win32/{HOST}_ap_cas_main_bundlereftable'
        host = game.bundle(self.host_name)
        records = ndb.read(host['manifest']['chunk_meta'])[0].children
        self.textures = {'art': _template(game, host, f'{HOST}_c', records),
                         'osk': _template(game, host, f'{HOST}_osk', records)}
        self.host_mask = game.texture_image(host, f'{HOST}_osk')
        self.ap = _ebx(game, host, f'{HOST}_ap')
        shared = game.bundle(SHARED)
        self.item = _ebx(game, shared, HOST_ITEM)
        self.thumb = {s: _ebx(game, shared, f'thumbnail/tool/{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.thumb_res = {s: _res(game, shared, f'thumbnail/tool/{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.cdn = {s: _ebx(game, shared, f'thumbnail/cdn/img_{HOST_THUMB}{s}') for s in ('', '_lrg')}
        self.cdn_refs = {s: _ref(ebx.read_document(self.cdn[s])) for s in ('', '_lrg')}
        self.thumb_refs = {s: _ref(ebx.read_document(self.thumb[s])) for s in ('', '_lrg')}
        self.lighting = thumbnail_lighting(game, log)


def add_wheel(patch, t, key, face, colour=(255, 255, 255)):
    """Adds one wheel to `patch` (a newdeck.DeckPatch): textures and preset in the host wheel's bundle,
    item, thumbnails and CDN records in the shared bundle. `t`: Templates; `key`: 'Own_...'; `face`: a PIL
    image (square-cropped); `colour`: sRGB 0-255. Returns (preset (name, host bundle) for the bundle
    reference table, the item's (file guid, instance guid) for the master list, the 768 px thumbnail)."""
    game = t.game
    base = key[len('Own_'):].lower()
    ap_name = f'{PREFIX}{base}_ap'
    item_name = f'{ITEMS}own_{base}'
    linear = srgb_to_linear(colour)
    art, mask = wheel_textures(face, colour, t.host_mask)
    tex = {'art': add_texture(patch, t.host_name, t.textures['art'], f'{PREFIX}{base}_role3_{SLOT}',
                              mips(Image.fromarray(art), bc7.encode)),
           'osk': add_texture(patch, t.host_name, t.textures['osk'], f'{PREFIX}{base}_role3_{OSK}',
                              mips(Image.fromarray(mask), bc1.encode))}
    remap = {}
    for slot, doc in tex.items():
        t_doc = t.textures[slot][1]
        remap[t_doc.file_guid] = doc.file_guid
        remap[t_doc.instances[0].instance_guid] = doc.instances[0].instance_guid
    ap = _clone(t.ap, ap_name)
    ap.remap_imports(remap)
    set_region_colour(ap, linear)
    patch.add_ebx(t.host_name, ap_name, _write(ap))
    # thumbnails: the wheel face-on, textures with TOC-only chunks, and CDN records
    cdn_refs = {}
    preview = None
    for suffix, size in (('', 256), ('_lrg', 768)):
        img = render_thumbnail(art, *t.lighting[size])
        preview = img if size == 768 else preview
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
    # the item
    item = _clone(t.item, item_name)
    item.set('Key', key)
    item.set('HashedAssetKey', djb32(key))
    mapping = {}
    for s in ('', '_lrg'):
        mapping[t.cdn_refs[s][0]], mapping[t.cdn_refs[s][1]] = cdn_refs[s]
    item.remap_imports(mapping)
    di = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
    item.set('AssetPaths[0].AssetName', f'{base}_ap', di)
    for axis, value in zip('xyz', linear):
        item.set(f'BaseColor.{axis}', float(value), di)
    patch.add_ebx(SHARED, item_name, _write(item))
    return (ap_name, t.host_name), _ref(item), preview


def build_wheels(game, wheels, mods_dir, mod_name, log=print):
    """wheels: [(display name, item key 'Own_...', face PIL image, sRGB colour)]. Writes a standalone mod
    <mods_dir>/<mod_name>. Returns (stats, [(display, base name, 768 px thumbnail)])."""
    patch = DeckPatch(game)
    t = Templates(game, log)
    shared = game.bundle(SHARED)
    master = ebx.read_document(_ebx(game, shared, MASTER))
    items_field = master.instances[0].object.find('Items')
    presets, built = [], []
    for display, key, face, colour in wheels:
        preset, item_ref, preview = add_wheel(patch, t, key, face, colour)
        presets.append(preset)
        master.imports.append(item_ref)
        items_field.value.data.append(ebx.Value(ebx.Pointer('external', len(master.imports) - 1)))
        built.append((display, key[len('Own_'):].lower(), preview))
        log(f'  built {display} ({key})')
    patch.replace_ebx(SHARED, MASTER, _write(master))
    data, meta = _res(game, shared, BRT)
    resource, new_meta = brt.add_presets(brt.Table(data, meta['meta']), [(p.lower(), h) for p, h in presets])
    patch.replace_res(SHARED, BRT, resource, new_meta)
    icon = built[0][2].resize((256, 256)) if built else None
    names = ', '.join(d for d, _, _ in built)
    lines = '\n'.join(f'- {d}' for d, _, _ in built)
    stats = patch.write(os.path.join(mods_dir, mod_name), mod_name, f'Wheels built by Caveman from PNGs: {names}.', icon=icon,
                        readme=f'# {mod_name}\n\nWheels built by Caveman from PNGs:\n\n{lines}\n')
    return stats, built
