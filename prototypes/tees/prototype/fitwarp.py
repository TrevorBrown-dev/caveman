"""Refine the thumbnail UV map: a smooth warp of where each thumbnail pixel looks up its UVs, fitted so stock
tees' logos (placed by their presets' scale and offsets in UV1) land where the real thumbnails show them."""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import numpy as np
from PIL import Image
from scipy.ndimage import map_coordinates
from scipy.optimize import minimize
import teethumb as tt
from gamedata import Game

g = Game(r'E:\Games\ReSkate')
f = tt.fit(g)
survey = json.load(open(os.path.join(HERE, 'survey_tshirtrelaxed.json')))
keys = json.load(open(os.path.join(HERE, 'tee_thumbs.json')))
stack = np.load(os.path.join(HERE, 'tee_thumbs.npy'))


def decal(sheet, tex, oh, ov, scale, aspect=1.0):
    n = sheet.shape[0]
    yy, xx = np.mgrid[0:n, 0:n]
    u, v = (xx + 0.5) / n, (yy + 0.5) / n
    du = (u - (0.5 + oh)) / (scale * aspect) + 0.5
    dv = (v - (0.5 - ov)) / scale + 0.5
    inside = (du >= 0) & (du < 1) & (dv >= 0) & (dv < 1)
    t = tt.sample(tex, np.stack([du, dv], -1))
    a = (t[..., 3:] / 255) * inside[..., None]
    sheet[..., :3] = t[..., :3] * a + sheet[..., :3] * (1 - a)
    sheet[..., 3:] = np.maximum(sheet[..., 3:], a * 255)


def texture_of(name):
    """A stock logo texture by asset name (decoded from its tee's bundle)."""
    for bn in g.toc()['bundle_by_name']:
        pass
    raise KeyError(name)


cases = []
for ap, thumb in zip(keys, stack):
    ps = survey[ap]
    if not ps.get('param_usemultilogo', (0, False))[1] or 'param_graphic_c' in ps or 'param_pattern_c' in ps:
        continue
    if any(k.startswith('param_graphic') and 'tint' in k for k in ps) or any('lookup' in k for k in ps):
        continue
    base = ps['param_base_c'][1]
    if not base.endswith('gen_shirt_tshirtrelaxed_c'):
        continue
    if 'param_graphic0_co' not in ps or ps.get('param_graphic0_scale', (0, 0))[1] < 0.15:
        continue
    cases.append((ap, thumb))
print(len(cases), 'cases')
# decode each case's logos from its bundle
cache = {}
sheets = []
for ap, thumb in cases[:12]:
    ps = survey[ap]
    bn = 'win32/' + ap + '_cas_main_bundlereftable'
    b = g.bundle(bn)
    sheet = np.zeros((1024, 1024, 4))
    for n in (0, 1, 2):
        co = ps.get(f'param_graphic{n}_co')
        if not co:
            continue
        name = co[1][3:].split(' #')[0]
        if name not in cache:
            cache[name] = np.asarray(g.texture_image(b, name)).astype(np.float64)
        decal(sheet, cache[name], ps.get(f'param_graphic{n}_offsethorizontal', (0, 0.0))[1], ps.get(f'param_graphic{n}_offsetvertical', (0, 0.0))[1],
              ps.get(f'param_graphic{n}_scale', (0, 1.0))[1], ps.get(f'param_graphic{n}_aspect', (0, 1.0))[1])
    colour = tt.srgb([float(x) for x in ps['param_region_color_0'][1][4:].split(',')])
    sheets.append((ap, thumb, sheet, colour))
    print(ap.split('/')[-1])

H = 384
k = 768 // H
uv1_lo = f['uv1'].reshape(H, k, H, k, 2).mean((1, 3))
light = f['light768'].reshape(H, k, H, k, 3).mean((1, 3))
sheen = f['sheen768'].reshape(H, k, H, k, 3).mean((1, 3))
shirt = f['shirt'].reshape(H, k, H, k).mean((1, 3)) > 0.5
real = [t[..., :3].reshape(H, k, H, k, 3).mean((1, 3)).astype(np.float64) for _, t, _, _ in sheets]
yy, xx = np.mgrid[0:H, 0:H].astype(np.float64)


def warp_uv(params, uv, size):
    """UVs looked up at warped pixel positions: x' = x + a0 + a1 (x - c), y' = y + b0 + b1 (y - c) + b2 (y - c)^2 / size."""
    a0, a1, b0, b1, b2 = params
    c = size / 2
    sc = size / 768
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float64)
    xs = xx + a0 * sc + a1 * (xx - c)
    ys = yy + b0 * sc + b1 * (yy - c) + b2 * (yy - c) ** 2 / size
    return np.stack([map_coordinates(uv[..., i], [ys, xs], order=1, mode='nearest') for i in range(2)], -1)


def loss(params):
    uv = warp_uv(params, uv1_lo, H)
    err = 0
    for (ap, thumb, sheet, colour), r in zip(sheets, real):
        s = tt.sample(sheet, uv)
        a = s[..., 3:] / 255
        col = s[..., :3] * a + colour * (1 - a)
        pred = col * light + sheen
        err += np.abs(pred - r)[shirt].mean()
    return err / len(sheets)


x0 = np.zeros(5)
print('before', loss(x0))
grid = [(b0, b1) for b0 in (-90, -60, -40, -20, 0) for b1 in (-0.15, -0.08, 0.0)]
scores = [(loss(np.array([0, 0, b0, b1, 0])), b0, b1) for b0, b1 in grid]
scores.sort()
print('grid best', scores[:3])
x0 = np.array([0, 0, scores[0][1], scores[0][2], 0], float)
simplex = np.array([x0] + [x0 + d for d in np.diag([15, 0.05, 15, 0.05, 0.15])])
best = minimize(loss, x0, method='Nelder-Mead', options={'maxiter': 300, 'xatol': 0.05, 'fatol': 1e-3, 'initial_simplex': simplex})
print('after', best.fun, best.x.round(4))
np.save(os.path.join(HERE, 'warp.npy'), best.x)
