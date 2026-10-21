import sys, json
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
sys.path.insert(0, r'C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\teework')
import numpy as np
from PIL import Image
from scipy.optimize import minimize
from gamedata import Game
import meshlib, raster
g = Game(r'E:\Games\ReSkate')
b = g.bundle(g.find_bundle('gen_shirt_tshirtrelaxed_complex_dmpreset_cas_main_bundlereftable'))
mesh = meshlib.read_mesh(g, b, 0)
P0 = mesh['pos'].astype(np.float64)
tris = mesh['tri']
stack = np.load('tee_thumbs.npy')
M768 = stack[..., :3].astype(float).std(0).mean(-1) > 12
R = 192
k = 768 // R
M = M768.reshape(R, k, R, k).mean((1, 3)) > 0.5

def pose(params):
    s, cx, cy, d, th, px, py, sx = params
    p = P0.copy()
    ax = np.abs(p[:, 0])
    w = np.clip((ax - sx) / 0.06, 0, 1) * (p[:, 1] > py - 0.25)
    for side in (1, -1):
        sel = (np.sign(p[:, 0]) == side) & (w > 0)
        ang = -side * th * w[sel]
        x = p[sel, 0] - side * px
        y = p[sel, 1] - py
        c, s_ = np.cos(ang), np.sin(ang)
        p[sel, 0] = side * px + c * x - s_ * y
        p[sel, 1] = py + s_ * x + c * y
    return p

def project(params, p, res=768):
    s, cx, cy, d = params[:4]
    f = d / (d - p[:, 2])         # perspective factor (camera at +z distance d)
    sc = res / 768
    xy = np.stack([(cx + s * p[:, 0] * f) * sc, (cy - s * p[:, 1] * f) * sc], 1)
    return xy

def loss(params):
    p = pose(params)
    xy = project(params, p, R)
    m, *_ = raster.rasterize(xy, p[:, 2], tris, R, R, {})
    return -((m & M).sum() / (m | M).sum())

x0 = np.array([1000.0, 383.0, 1600.0, 3.0, 0.3, 0.2, 1.42, 0.19])
best = minimize(loss, x0, method='Nelder-Mead', options={'maxiter': 600, 'xatol': 1e-3, 'fatol': 1e-5, 'initial_simplex': None})
print(best.x.round(4), -best.fun)
np.save('camfit.npy', best.x)
p = pose(best.x)
xy = project(best.x, p)
m, *_ = raster.rasterize(xy, p[:, 2], tris, 768, 768, {})
img = np.zeros((768, 768, 3), np.uint8)
img[..., 0] = M768 * 255
img[..., 1] = m * 255
Image.fromarray(img).save('img/fit2.png')
print('iou768', (m & M768).sum() / (m | M768).sum())
