"""Tee menu thumbnails, rendered from the player's own install.

The stock tee thumbnails (105 tshirtrelaxed items, 768 px) share one mannequin, pose and camera, so:
  * the tee mesh (LOD0 of gen_shirt_tshirtrelaxed_base_mesh) is projected with a perspective camera, and
    each sleeve turned about its shoulder, fitted to the stock thumbnails' shirt silhouette (where the
    105 thumbnails differ); this gives, per thumbnail pixel, the UV0 and UV1 the shirt shows there;
  * per pixel and channel, thumbnail = colour x light + sheen, fitted over the stock tees (colour = the
    preset's region colour in sRGB) with a trimmed least squares, so logos count as outliers.
Nothing from the game is kept: the fit is cached by Skate.exe in %LOCALAPPDATA%, like the deck lighting."""
import json
import os
import struct
import sys

import numpy as np
from PIL import Image
from scipy.ndimage import distance_transform_edt
from scipy.optimize import minimize

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
sys.path.insert(0, HERE)
import ebx  # noqa: E402
import meshlib  # noqa: E402
import raster  # noqa: E402

MESH_BUNDLE = 'win32/characters/maincharacters/generic/cas/clothing/unlicensed/generic/apparel/top/shirt/tshirtrelaxed/2023/gen_shirt_tshirtrelaxed_complex_dmpreset_cas_main_bundlereftable'
SHARED = 'win32/characters/customization/configs/cas_main_sharedbundle'
REGION0 = '4b18a9e2-cd35-8c46-59ff-5e880100548c'


def srgb(lin):
    c = np.clip(np.asarray(lin, np.float64), 0, 1)
    return 255 * np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1 / 2.4) - 0.055)


def linear(s):
    c = np.asarray(s, np.float64) / 255
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def load_mesh(game):
    return meshlib.read_mesh(game, game.bundle(MESH_BUNDLE), 0)


def pose(P0, params):
    """The bind-pose mesh with each sleeve turned down about its shoulder (the thumbnails' arms hang lower)."""
    s, cx, cy, d, th, px, py, sx = params
    p = P0.copy()
    ax = np.abs(p[:, 0])
    w = np.clip((ax - sx) / 0.06, 0, 1) * (p[:, 1] > py - 0.25)
    for side in (1, -1):
        sel = (np.sign(p[:, 0]) == side) & (w > 0)
        ang = -side * th * w[sel]
        x = p[sel, 0] - side * px
        y = p[sel, 1] - py
        c, sn = np.cos(ang), np.sin(ang)
        p[sel, 0] = side * px + c * x - sn * y
        p[sel, 1] = py + sn * x + c * y
    return p


def project(params, p, res=768):
    s, cx, cy, d = params[:4]
    f = d / (d - p[:, 2])
    sc = res / 768
    return np.stack([(cx + s * p[:, 0] * f) * sc, (cy - s * p[:, 1] * f) * sc], 1)


def stock_thumbnails(game, presets):
    """[(preset name, 768 px RGBA array)] of the stock tshirtrelaxed tees that have thumbnails."""
    sh = game.bundle(SHARED)
    names = {r['name'] for r in sh['manifest']['res']}
    out = []
    for ap in sorted(presets):
        leaf = ap.split('/')[-1][:-3]
        brand, num = leaf.split('_shirt_')[0], leaf.split('_')[-1]
        t = f'thumbnail/tool/own_topshirt_{brand}_tshirtrelaxed_{num}_lrg'
        if t in names:
            out.append((ap, np.asarray(game.texture_image(sh, t))))
    return out


def region_colours(game):
    """{preset name: (region colour 0 as linear rgb, uses a graphic)} of every stock tshirtrelaxed preset."""
    out = {}
    for bn in sorted(b for b in game.toc()['bundle_by_name'] if '/top/shirt/tshirtrelaxed/' in b and b.endswith('_ap_cas_main_bundlereftable')):
        b = game.bundle(bn)
        m = b['manifest']
        ap = next(e['name'] for e in m['ebx'] if e['name'].endswith('_ap') and e['name'] in bn)
        i = next(i for i, e in enumerate(m['ebx']) if e['name'] == ap)
        doc = ebx.read_document(game.decode(b['files'][1 + i]))
        recs = {r.offset: r for r in doc.boxed_values}
        colour = None
        for v in doc.instances[0].object.find('ParamInfo').value.data:
            info = v.data
            if str(info.find('ParamKey').value.data.find('Data').value.data) == REGION0:
                boxed = info.find('Value').value.data.find('Value').value.data
                colour = struct.unpack_from('<3f', recs[boxed.data_offset].raw_bytes)
        out[ap] = colour
    return out


def fit(game, log=print):
    """Everything a tee thumbnail needs, fitted from the stock tees: {uv0, uv1 (768x768x2), shirt (mask
    where the shirt shows), light and sheen at 768 and 256, alpha at 768 and 256, camera}."""
    exe = os.path.join(game.root, 'Skate.exe')
    st = os.stat(exe)
    cache_dir = os.path.join(os.environ.get('LOCALAPPDATA', game.root), 'Caveman', 'cache')
    cache = os.path.join(cache_dir, f'tee-thumbnail-{st.st_size}-{int(st.st_mtime)}-v1.npz')
    try:
        with np.load(cache) as z:
            out = {k: z[k] for k in z.files}
        log('  tee thumbnail mapping from the cache')
        import light_v2
        return light_v2.light_fit(game, out, sys.modules[__name__], log)
    except (OSError, KeyError, ValueError):
        pass
    mesh = load_mesh(game)
    P0 = mesh['pos'].astype(np.float64)
    tris = mesh['tri']
    colours = region_colours(game)
    thumbs = stock_thumbnails(game, colours)
    stack = np.stack([t for _, t in thumbs]).astype(np.float64)
    log(f'  tee thumbnails: {len(thumbs)} stock tees')
    shirt768 = stack[..., :3].std(0).mean(-1) > 12
    # camera and sleeve angle: maximise the overlap of the mesh silhouette with the shirt region
    R = 192
    k = 768 // R
    M = shirt768.reshape(R, k, R, k).mean((1, 3)) > 0.5

    def loss(params):
        p = pose(P0, params)
        m = raster.rasterize(project(params, p, R), p[:, 2], tris, R, R, {})[0]
        return -((m & M).sum() / (m | M).sum())

    x0 = np.array([1000.0, 383.0, 1600.0, 3.0, 0.3, 0.2, 1.42, 0.19])
    cam = minimize(loss, x0, method='Nelder-Mead', options={'maxiter': 600, 'xatol': 1e-3, 'fatol': 1e-5}).x
    p = pose(P0, cam)
    m, _, _, _, attrs = raster.rasterize(project(cam, p), p[:, 2], tris, 768, 768, {'uv0': mesh['uv0'], 'uv1': mesh['uv1']})
    log(f'  tee mesh fitted to the thumbnails: overlap {(m & shirt768).sum() / (m | shirt768).sum():.3f}')
    # shirt pixels the mesh misses take the nearest rendered pixel's UVs
    _, (iy, ix) = distance_transform_edt(~m, return_indices=True)
    uv0 = attrs['uv0'][iy, ix]
    uv1 = attrs['uv1'][iy, ix]
    # light and sheen: thumbnail = colour x light + sheen, per pixel, trimmed least squares over the tees
    cols = np.array([srgb(colours[ap]) for ap, _ in thumbs])           # (n, 3)
    out = {'uv0': uv0.astype(np.float32), 'uv1': uv1.astype(np.float32), 'shirt': shirt768, 'camera': cam,
           'mesh_mask': m}
    for size in (768, 256):
        kk = 768 // size
        img = stack[..., :3].reshape(len(thumbs), size, kk, size, kk, 3).mean((2, 4))
        alpha = np.median(stack[..., 3].reshape(len(thumbs), size, kk, size, kk).mean((2, 4)), 0)
        light = np.zeros((size, size, 3))
        sheen = np.zeros((size, size, 3))
        for c in range(3):
            x = cols[:, c][:, None, None] * np.ones((1, size, size))
            y = img[..., c]
            keep = np.ones_like(y, bool)
            for _ in range(3):
                w = keep.astype(np.float64)
                n = w.sum(0) + 1e-9
                mx, my = (w * x).sum(0) / n, (w * y).sum(0) / n
                vx = (w * (x - mx) ** 2).sum(0) / n
                cxy = (w * (x - mx) * (y - my)).sum(0) / n
                a = cxy / (vx + 25.0)
                b0 = my - a * mx
                res = np.abs(y - (a * x + b0))
                cut = np.percentile(res, 70, axis=0)
                keep = res <= cut + 1e-6
            light[..., c], sheen[..., c] = a, b0
        out[f'light{size}'] = light.astype(np.float32)
        out[f'sheen{size}'] = sheen.astype(np.float32)
        out[f'alpha{size}'] = alpha.astype(np.float32)
    try:
        os.makedirs(cache_dir, exist_ok=True)
        np.savez_compressed(cache, **out)
    except OSError as error:
        log(f'  note: could not cache the tee thumbnail fit ({error})')
    import light_v2
    return light_v2.light_fit(game, out, sys.modules[__name__], log)


def sample(tex, uv):
    """Bilinear sample of an (H, W, C) array at UV (wrapping, as the GPU does)."""
    h, w = tex.shape[:2]
    x = (uv[..., 0] % 1.0) * w - 0.5
    y = (uv[..., 1] % 1.0) * h - 0.5
    x0 = np.floor(x).astype(int)
    y0 = np.floor(y).astype(int)
    fx, fy = (x - x0)[..., None], (y - y0)[..., None]
    x0 %= w
    y0 %= h
    x1, y1 = (x0 + 1) % w, (y0 + 1) % h
    t = tex.astype(np.float64)
    return (t[y0, x0] * (1 - fx) + t[y0, x1] * fx) * (1 - fy) + (t[y1, x0] * (1 - fx) + t[y1, x1] * fx) * fy


# Where each thumbnail pixel looks up its UVs, warped (x' = x + a0 + a1 (x - 384), y' = y + b0 + b1 (y - 384)):
# the shirt on the thumbnails' mannequin sits lower than the bind-pose mesh; fitted so the logos of 12 stock tees
# (placed by their presets in UV1) land where the real thumbnails show them (grid search, fitwarp2.py).
WARP = (0.0, 0.05, -70.0, 0.05)


def warped(uv, warp=WARP):
    from scipy.ndimage import map_coordinates
    a0, a1, b0, b1 = warp
    yy, xx = np.mgrid[0:768, 0:768].astype(np.float64)
    xs = xx + a0 + a1 * (xx - 384)
    ys = yy + b0 + b1 * (yy - 384)
    return np.stack([map_coordinates(uv[..., i], [ys, xs], order=1, mode='nearest') for i in range(2)], -1)


def albedo(f, shirt_srgb, sheet_rgba):
    """The tee's colour as the thumbnail sees it (sRGB, 768 px): the print sheet (UV1, RGBA) over the shirt colour."""
    sheet = sample(np.asarray(sheet_rgba, np.float64), warped(f['uv1']))
    a = sheet[..., 3:] / 255
    return sheet[..., :3] * a + np.asarray(shirt_srgb, np.float64) * (1 - a)


def render(f, shirt_srgb, sheet_rgba):
    """{768: RGBA image, 256: RGBA image}: the thumbnails of a tee with this shirt colour and print sheet."""
    col768 = albedo(f, shirt_srgb, sheet_rgba)
    out = {}
    for size in (768, 256):
        kk = 768 // size
        col = col768.reshape(size, kk, size, kk, 3).mean((1, 3)) if kk > 1 else col768
        rgb = np.clip(col * f[f'light{size}'] + f[f'sheen{size}'], 0, 255)
        alpha = f[f'alpha{size}']
        rgb[alpha < 1] = 0
        out[size] = Image.fromarray(np.dstack([rgb, alpha]).round().astype(np.uint8), 'RGBA')
    return out
