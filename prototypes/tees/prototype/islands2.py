import sys, os
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
sys.path.insert(0, HERE)
import numpy as np
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components
from gamedata import Game
import meshlib
g = Game(r'E:\Games\ReSkate')
for kind in sys.argv[1:]:
    bn = next((b for b in g.toc()['bundle_by_name'] if b.endswith(f'/{kind}_complex_dmpreset_cas_main_bundlereftable')), None)
    if bn is None:
        print(kind, 'no dmpreset bundle'); continue
    b = g.bundle(bn)
    try:
        mesh = meshlib.read_mesh(g, b, 0)
    except Exception as e:
        print(kind, 'ERR', e); continue
    p, u1, tri = mesh['pos'], mesh['uv1'], mesh['tri']
    if u1 is None or len(u1) == 0:
        print(kind, 'no UV1'); continue
    keys = {}
    vid = np.array([keys.setdefault((round(float(a), 5), round(float(c), 5)), len(keys)) for a, c in u1])
    rows = np.concatenate([vid[tri[:, 0]], vid[tri[:, 1]], vid[tri[:, 2]]])
    cols = np.concatenate([vid[tri[:, 1]], vid[tri[:, 2]], vid[tri[:, 0]]])
    _, lab = connected_components(coo_matrix((np.ones(len(rows)), (rows, cols)), shape=(len(keys),) * 2), directed=False)
    tl = lab[vid[tri[:, 0]]]
    print(f'## {kind}: {len(tri)} tris; mesh sections {len(mesh["sections"])}')
    for c in np.unique(tl):
        ts = tri[tl == c]
        vs = np.unique(ts)
        uv = u1[vs]
        tile = int(np.floor(uv[:, 0].mean()))
        if np.ptp(uv[:, 0]) * np.ptp(uv[:, 1]) < 1e-3:
            continue
        q = p[vs]
        print(f'   tile {tile:2d} u {uv[:,0].min()-tile:.2f}-{uv[:,0].max()-tile:.2f} v {uv[:,1].min():.2f}-{uv[:,1].max():.2f} tris {len(ts):5d}  x {q[:,0].min():+.2f}..{q[:,0].max():+.2f} y {q[:,1].min():.2f}..{q[:,1].max():.2f} z {q[:,2].min():+.2f}..{q[:,2].max():+.2f}')
