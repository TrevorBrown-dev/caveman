"""Per-pixel thumbnail light and sheen fitted only where the stock tees' shirts show plain (no logo)."""
import os
import struct

import numpy as np
from scipy.ndimage import binary_dilation, distance_transform_edt

import ebx


def preset_params(game, bundle, ap):
    """{parameter name: value} of a preset (floats, bools, vectors, imported asset names)."""
    m = bundle['manifest']
    names = {}
    for i, e in enumerate(m['ebx']):
        names[ebx.read_file_guid(game.decode(bundle['files'][1 + i]))] = e['name']
    i = next(i for i, e in enumerate(m['ebx']) if e['name'] == ap)
    doc = ebx.read_document(game.decode(bundle['files'][1 + i]))
    recs = {r.offset: r for r in doc.boxed_values}
    imp = set(doc.import_offsets)
    out = {}
    for v in doc.instances[0].object.find('ParamInfo').value.data:
        val = v.data.find('Value').value.data
        p = val.find('Parameter').value.data
        pname = names.get(doc.imports[p.index][0], '?').split('/')[-1]
        boxed = val.find('Value').value.data
        rec = recs.get(boxed.data_offset)
        if rec is None:
            continue
        if boxed.encoded_type & 0x80000000:
            t = ((boxed.encoded_type & 0x7FFFFFFF) >> 5) & 0x1F
            if t == ebx.FLOAT32:
                out[pname] = struct.unpack_from('<f', rec.raw_bytes)[0]
            elif t == ebx.BOOL:
                out[pname] = bool(rec.raw_bytes[0])
        elif rec.offset in imp:
            out[pname] = names.get(doc.imports[struct.unpack_from('<I', rec.raw_bytes)[0] >> 1][0], '?')
        elif len(rec.raw_bytes) >= 12:
            out[pname] = struct.unpack_from('<3f', rec.raw_bytes)
    return out


def decal_boxes(ps):
    """UV1 rectangles (u0, v0, u1, v1) a preset prints into; None when it prints over everything or over
    something this model cannot place (a pattern, its own base colour texture, a sheet with no placement)."""
    if 'param_pattern_c' in ps or not str(ps.get('param_base_c', '')).endswith('gen_shirt_tshirtrelaxed_c'):
        return None
    if not ps.get('param_usegraphic', False):
        return []
    boxes = []
    multi = ps.get('param_usemultilogo', False)
    for n in (0, 1, 2):
        has = f'param_graphic{n}_co' in ps if multi else f'param_graphic{n}_scale' in ps
        if not has:
            continue
        sc = ps.get(f'param_graphic{n}_scale', 1.0)
        asp = ps.get(f'param_graphic{n}_aspect', 1.0)
        cu = 0.5 + ps.get(f'param_graphic{n}_offsethorizontal', 0.0)
        cv = 0.5 - ps.get(f'param_graphic{n}_offsetvertical', 0.0)
        boxes.append((cu - sc * asp / 2, cv - sc / 2, cu + sc * asp / 2, cv + sc / 2))
    if not multi and 'param_graphic_c' in ps and not boxes:
        return None
    return boxes


def light_fit(game, f, tt, log=print):
    """Per pixel and channel, thumbnail = colour x light + sheen, fitted over the stock tees whose shirt shows
    plain there: tees with a pattern or their own base colour texture are left out, and so is every pixel
    within 20 px of where a tee's preset puts a logo. Pixels too few tees show plainly take their light from
    the nearest pixels that do. Cached by Skate.exe."""
    st = os.stat(os.path.join(game.root, 'Skate.exe'))
    cache_dir = os.path.join(os.environ.get('LOCALAPPDATA', game.root), 'Caveman', 'cache')
    cache = os.path.join(cache_dir, f'tee-light-{st.st_size}-{int(st.st_mtime)}-v2.npz')
    try:
        with np.load(cache) as z:
            f.update({k: z[k] for k in z.files})
        log('  tee thumbnail lighting from the cache')
        return f
    except (OSError, KeyError, ValueError):
        pass
    colours = tt.region_colours(game)
    thumbs = tt.stock_thumbnails(game, colours)
    uv = tt.warped(f['uv1'])
    fu, fv = uv[..., 0] % 1.0, uv[..., 1]
    cols, imgs, clear, alphas = [], [], [], []
    for ap, t in thumbs:
        alphas.append(t[..., 3].astype(np.float64))
        b = game.bundle('win32/' + ap + '_cas_main_bundlereftable')
        boxes = decal_boxes(preset_params(game, b, ap))
        if boxes is None:
            continue
        logo = np.zeros((768, 768), bool)
        for u0, v0, u1, v1 in boxes:
            logo |= (fu >= u0) & (fu <= u1) & (fv >= v0) & (fv <= v1)
        logo = binary_dilation(logo, iterations=20)
        cols.append(tt.srgb(colours[ap]))
        imgs.append(t[..., :3].astype(np.float64))
        clear.append(~logo)
    cols, imgs, clear, alphas = np.array(cols), np.stack(imgs), np.stack(clear), np.stack(alphas)
    log(f'  tee thumbnail lighting: {len(cols)} stock tees ({clear.mean():.0%} of their pixels plain)')
    out = {}
    for size in (768, 256):
        kk = 768 // size
        img = imgs.reshape(len(cols), size, kk, size, kk, 3).mean((2, 4))
        w = clear.reshape(len(cols), size, kk, size, kk).min((2, 4)).astype(np.float64)
        alpha = np.median(alphas.reshape(len(alphas), size, kk, size, kk).mean((2, 4)), 0)
        n = w.sum(0)
        light = np.zeros((size, size, 3))
        sheen = np.zeros((size, size, 3))
        for c in range(3):
            x = cols[:, c][:, None, None] * np.ones((1, size, size))
            y = img[..., c]
            nn = n + 1e-9
            mx, my = (w * x).sum(0) / nn, (w * y).sum(0) / nn
            vx = (w * (x - mx) ** 2).sum(0) / nn
            cxy = (w * (x - mx) * (y - my)).sum(0) / nn
            a = cxy / (vx + 25.0)
            light[..., c], sheen[..., c] = a, my - a * mx
        few = n < 6
        if few.any():
            _, (iy, ix) = distance_transform_edt(few, return_indices=True)
            light[few], sheen[few] = light[iy[few], ix[few]], sheen[iy[few], ix[few]]
        out[f'light{size}'] = light.astype(np.float32)
        out[f'sheen{size}'] = sheen.astype(np.float32)
        out[f'alpha{size}'] = alpha.astype(np.float32)
        out[f'count{size}'] = n.astype(np.float32)
    try:
        os.makedirs(cache_dir, exist_ok=True)
        np.savez_compressed(cache, **out)
    except OSError as error:
        log(f'  note: could not cache the tee lighting ({error})')
    f.update(out)
    return f
