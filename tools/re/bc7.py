"""A small BC7 encoder (mode 6 only: one subset, RGBA endpoints with per-endpoint p-bits, 4-bit indices),
vectorized with numpy. RGB input is encoded opaque (alpha 254 or 255: deck textures do not use it);
RGBA input keeps its alpha (menu thumbnails are cut out)."""
import numpy as np

WEIGHTS = np.array([0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64], np.float32)
BATCH = 8192


def _blocks(px):
    """(H, W, C) uint8 -> (N, 16, C) float32, padding to multiples of 4 by repeating the edge."""
    h, w, c = px.shape
    ph, pw = max(4, (h + 3) // 4 * 4), max(4, (w + 3) // 4 * 4)
    if (ph, pw) != (h, w):
        px = np.pad(px, ((0, ph - h), (0, pw - w), (0, 0)), mode='edge')
    b = px.reshape(ph // 4, 4, pw // 4, 4, c).transpose(0, 2, 1, 3, 4).reshape(-1, 16, c)
    return b.astype(np.float32)


def _quantize(e):
    """Endpoint colours (N, C) -> 7-bit values (N, C) and a p-bit (N,), choosing the p-bit with less error."""
    best_q, best_p, best_err = None, None, None
    for p in (0, 1):
        q = np.clip(np.rint((e - p) / 2), 0, 127)
        err = (((q * 2 + p) - e) ** 2).sum(1)
        if best_err is None:
            best_q, best_p, best_err = q, np.full(len(e), p), err
        else:
            take = err < best_err
            best_q = np.where(take[:, None], q, best_q)
            best_p = np.where(take, p, best_p)
            best_err = np.minimum(err, best_err)
    return best_q.astype(np.int64), best_p.astype(np.int64)


def _palette(q0, p0, q1, p1):
    e0 = (q0 * 2 + p0[:, None]).astype(np.float32)
    e1 = (q1 * 2 + p1[:, None]).astype(np.float32)
    w = WEIGHTS[None, :, None]
    return np.floor(((64 - w) * e0[:, None, :] + w * e1[:, None, :] + 32) / 64)  # (N, 16, C)


def _fit(px, e0, e1):
    q0, p0 = _quantize(np.clip(e0, 0, 255))
    q1, p1 = _quantize(np.clip(e1, 0, 255))
    pal = _palette(q0, p0, q1, p1)
    d = ((px[:, :, None, :] - pal[:, None, :, :]) ** 2).sum(3)  # (N, 16 px, 16 entries)
    idx = d.argmin(2)
    err = np.take_along_axis(d, idx[:, :, None], 2)[:, :, 0].sum(1)
    return q0, p0, q1, p1, idx, err


def _encode_batch(px):
    n, ch = len(px), px.shape[2]
    mean = px.mean(1)
    c = px - mean[:, None, :]
    cov = np.einsum('npi,npj->nij', c, c)
    axis = np.ones((n, ch), np.float32) / np.sqrt(ch)
    for _ in range(8):  # power iteration for the principal axis
        axis = np.einsum('nij,nj->ni', cov, axis)
        axis /= np.maximum(np.linalg.norm(axis, axis=1, keepdims=True), 1e-6)
    t = np.einsum('npi,ni->np', c, axis)
    e0 = mean + t.min(1)[:, None] * axis
    e1 = mean + t.max(1)[:, None] * axis
    q0, p0, q1, p1, idx, err = _fit(px, e0, e1)
    # One least-squares refit of the endpoints to the chosen indices, kept where it helps.
    w = WEIGHTS[idx] / 64.0
    a, b = 1 - w, w
    aa, bb, ab = (a * a).sum(1), (b * b).sum(1), (a * b).sum(1)
    det = aa * bb - ab * ab
    ok = np.abs(det) > 1e-6
    ax = np.einsum('np,npc->nc', a, px)
    bx = np.einsum('np,npc->nc', b, px)
    safe = np.where(ok, det, 1)[:, None]
    r0 = (bb[:, None] * ax - ab[:, None] * bx) / safe
    r1 = (aa[:, None] * bx - ab[:, None] * ax) / safe
    r0 = np.where(ok[:, None], r0, e0)
    r1 = np.where(ok[:, None], r1, e1)
    rq0, rp0, rq1, rp1, ridx, rerr = _fit(px, r0, r1)
    better = rerr < err
    q0 = np.where(better[:, None], rq0, q0); q1 = np.where(better[:, None], rq1, q1)
    p0 = np.where(better, rp0, p0); p1 = np.where(better, rp1, p1)
    idx = np.where(better[:, None], ridx, idx)
    # The anchor (pixel 0) index must have its top bit clear: swap the endpoints and mirror otherwise.
    flip = idx[:, 0] >= 8
    q0, q1 = np.where(flip[:, None], q1, q0), np.where(flip[:, None], q0, q1)
    p0, p1 = np.where(flip, p1, p0), np.where(flip, p0, p1)
    idx = np.where(flip[:, None], 15 - idx, idx)
    return _pack(q0, p0, q1, p1, idx)


def _pack(q0, p0, q1, p1, idx):
    n = len(q0)
    lo = np.zeros(n, np.uint64)
    hi = np.zeros(n, np.uint64)
    pos = 0

    def put(value, bits):
        nonlocal lo, hi, pos
        v = value.astype(np.uint64) & np.uint64((1 << bits) - 1)
        for i in range(bits):  # bit by bit keeps the 64-bit boundary simple
            bit = (v >> np.uint64(i)) & np.uint64(1)
            at = pos + i
            if at < 64:
                lo |= bit << np.uint64(at)
            else:
                hi |= bit << np.uint64(at - 64)
        pos += bits

    put(np.full(n, 1 << 6), 7)  # mode 6
    for ch in range(3):
        put(q0[:, ch], 7)
        put(q1[:, ch], 7)
    if q0.shape[1] == 4:
        put(q0[:, 3], 7)
        put(q1[:, 3], 7)
    else:
        put(np.full(n, 127), 7)  # alpha endpoints: opaque
        put(np.full(n, 127), 7)
    put(p0, 1)
    put(p1, 1)
    put(idx[:, 0], 3)
    for k in range(1, 16):
        put(idx[:, k], 4)
    out = np.empty((n, 2), '<u8')
    out[:, 0], out[:, 1] = lo, hi
    return out.tobytes()


def encode(pixels):
    """(H, W, 3) or (H, W, 4) uint8 -> BC7 blocks, row-major by block."""
    px = _blocks(np.asarray(pixels, np.uint8))
    return b''.join(_encode_batch(px[i:i + BATCH]) for i in range(0, len(px), BATCH))
