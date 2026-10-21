import sys
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
sys.path.insert(0, r'C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\teework')
import numpy as np
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components
from gamedata import Game
import meshlib
g = Game(r'E:\Games\ReSkate')
mesh = meshlib.read_mesh(g, g.bundle(g.find_bundle('gen_shirt_tshirtrelaxed_complex_dmpreset_cas_main_bundlereftable')), 0)
p, u1, tri = mesh['pos'], mesh['uv1'], mesh['tri']
# islands: triangles connected through shared vertices with identical uv1
n = len(p)
key = {}
vid = np.zeros(n, int)
for i in range(n):
    k = (round(float(u1[i, 0]), 5), round(float(u1[i, 1]), 5))
    vid[i] = key.setdefault(k, len(key))
rows = np.concatenate([vid[tri[:, 0]], vid[tri[:, 1]], vid[tri[:, 2]]])
cols = np.concatenate([vid[tri[:, 1]], vid[tri[:, 2]], vid[tri[:, 0]]])
G = coo_matrix((np.ones(len(rows)), (rows, cols)), shape=(len(key), len(key)))
nc, lab = connected_components(G, directed=False)
tl = lab[vid[tri[:, 0]]]
for c in range(nc):
    ts = tri[tl == c]
    vs = np.unique(ts)
    uv = u1[vs]
    tile = int(np.floor(uv[:, 0].mean()))
    q = p[vs]
    print(f'island {c}: tris {len(ts)} tile {tile} u {uv[:,0].min()-tile:.3f}-{uv[:,0].max()-tile:.3f} v {uv[:,1].min():.3f}-{uv[:,1].max():.3f}  pos x {q[:,0].min():.2f}..{q[:,0].max():.2f} y {q[:,1].min():.2f}..{q[:,1].max():.2f} z {q[:,2].min():.2f}..{q[:,2].max():.2f}')
print()
for c, name in ((11, 'R sleeve'), (16, 'L sleeve'), (13, 'front'), (12, 'back'), (17, 'hem strip')):
    vs = np.unique(tri[tl == c])
    uv = u1[vs].copy(); uv[:, 0] -= np.floor(uv[:, 0].mean())
    q = p[vs]
    A = np.c_[uv, np.ones(len(vs))]
    coef, *_ = np.linalg.lstsq(A, q, rcond=None)   # pos = du * cu + dv * cv + c
    cu, cv = coef[0], coef[1]
    uc, vc = uv[:, 0].mean(), uv[:, 1].mean()
    near = np.argsort(np.hypot(uv[:, 0] - uc, uv[:, 1] - vc))[:10]
    print(f'{name}: +u -> {np.round(cu / np.linalg.norm(cu), 2)} ({np.linalg.norm(cu):.2f} m per unit), +v -> {np.round(cv / np.linalg.norm(cv), 2)} ({np.linalg.norm(cv):.2f} m/unit), centre pos {np.round(q[near].mean(0), 3)}')
