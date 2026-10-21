"""Tiny z-buffer rasterizer: per pixel, the front-most triangle's barycentric-interpolated attributes."""
import numpy as np

def rasterize(xy, z, tris, W, H, attrs):
    """xy: (N,2) pixel coords, z: (N,) depth (larger = nearer), tris (M,3), attrs: dict name->(N,k).
    Returns (mask, depth, {name: (H,W,k)})."""
    depth = np.full((H, W), -np.inf)
    tri_id = np.full((H, W), -1, np.int64)
    bary = np.zeros((H, W, 3))
    P = xy[tris]            # (M,3,2)
    Z = z[tris]
    x0 = np.clip(np.floor(P[:, :, 0].min(1)).astype(int), 0, W - 1)
    x1 = np.clip(np.ceil(P[:, :, 0].max(1)).astype(int), 0, W - 1)
    y0 = np.clip(np.floor(P[:, :, 1].min(1)).astype(int), 0, H - 1)
    y1 = np.clip(np.ceil(P[:, :, 1].max(1)).astype(int), 0, H - 1)
    for t in range(len(tris)):
        if x1[t] < x0[t] or y1[t] < y0[t]:
            continue
        (ax, ay), (bx, by), (cx, cy) = P[t]
        den = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy)
        if abs(den) < 1e-12:
            continue
        ys, xs = np.mgrid[y0[t]:y1[t] + 1, x0[t]:x1[t] + 1]
        px, py = xs + 0.5, ys + 0.5
        w0 = ((by - cy) * (px - cx) + (cx - bx) * (py - cy)) / den
        w1 = ((cy - ay) * (px - cx) + (ax - cx) * (py - cy)) / den
        w2 = 1 - w0 - w1
        inside = (w0 >= -1e-6) & (w1 >= -1e-6) & (w2 >= -1e-6)
        if not inside.any():
            continue
        zz = w0 * Z[t, 0] + w1 * Z[t, 1] + w2 * Z[t, 2]
        sub = depth[y0[t]:y1[t] + 1, x0[t]:x1[t] + 1]
        upd = inside & (zz > sub)
        if not upd.any():
            continue
        sub[upd] = zz[upd]
        tri_id[y0[t]:y1[t] + 1, x0[t]:x1[t] + 1][upd] = t
        b = bary[y0[t]:y1[t] + 1, x0[t]:x1[t] + 1]
        b[upd] = np.stack([w0[upd], w1[upd], w2[upd]], -1)
    mask = tri_id >= 0
    out = {}
    tid = np.where(mask, tri_id, 0)
    for name, a in attrs.items():
        va = a[tris[tid]]          # (H,W,3,k)
        out[name] = (va * bary[..., None]).sum(2)
    return mask, depth, tri_id, bary, out
