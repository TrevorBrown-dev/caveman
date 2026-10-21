"""A small BC1 encoder (opaque four-colour blocks), vectorized with numpy: each 4 x 4 block's endpoints
are the ends of its colours along their main axis, quantized to RGB 565; each texel takes the nearest of
the four palette colours. Wheel masks (`_osk`) are BC1 (format 54)."""
import numpy as np

BATCH = 16384


def _blocks(px):
    """(H, W, 3) uint8 -> (N, 16, 3) float32, padding to multiples of 4 by repeating the edge."""
    h, w, _ = px.shape
    ph, pw = max(4, (h + 3) // 4 * 4), max(4, (w + 3) // 4 * 4)
    if (ph, pw) != (h, w):
        px = np.pad(px, ((0, ph - h), (0, pw - w), (0, 0)), mode='edge')
    return px.reshape(ph // 4, 4, pw // 4, 4, 3).transpose(0, 2, 1, 3, 4).reshape(-1, 16, 3).astype(np.float32)


def _to565(c):
    c = np.clip(np.round(c), 0, 255)
    r = np.round(c[:, 0] * 31 / 255).astype(np.uint32)
    g = np.round(c[:, 1] * 63 / 255).astype(np.uint32)
    b = np.round(c[:, 2] * 31 / 255).astype(np.uint32)
    return (r << 11) | (g << 5) | b


def _from565(v):
    r, g, b = (v >> 11) & 31, (v >> 5) & 63, v & 31
    return np.stack([(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)], 1).astype(np.float32)


def _encode_batch(px):
    n = len(px)
    mean = px.mean(1, keepdims=True)
    d = px - mean
    cov = np.einsum('nki,nkj->nij', d, d)
    axis = np.ones((n, 3), np.float32)
    for _ in range(8):                      # power iteration: the main axis of each block's colours
        axis = np.einsum('nij,nj->ni', cov, axis)
        axis /= np.maximum(np.linalg.norm(axis, axis=1, keepdims=True), 1e-6)
    t = np.einsum('nki,ni->nk', d, axis)
    e0 = mean[:, 0] + axis * t.max(1, keepdims=True)
    e1 = mean[:, 0] + axis * t.min(1, keepdims=True)
    c0, c1 = _to565(e0), _to565(e1)
    swap = c0 < c1                          # four-colour mode needs c0 > c1
    c0, c1 = np.where(swap, c1, c0), np.where(swap, c0, c1)
    p0, p1 = _from565(c0), _from565(c1)
    pal = np.stack([p0, p1, (2 * p0 + p1) / 3, (p0 + 2 * p1) / 3], 1)          # (n, 4, 3)
    dist = ((px[:, :, None, :] - pal[:, None, :, :]) ** 2).sum(3)             # (n, 16, 4)
    idx = dist.argmin(2).astype(np.uint32)
    idx[c0 == c1] = 0                       # a flat block: c0 == c1 is three-colour mode, index 0 is c0
    bits = (idx << (2 * np.arange(16, dtype=np.uint32))).sum(1).astype(np.uint32)
    out = np.empty((n, 2), '<u4')
    out[:, 0] = c0 | (c1 << 16)
    out[:, 1] = bits
    return out.tobytes()


def encode(pixels):
    """(H, W, 3) or (H, W, 4) uint8 -> BC1 blocks (8 bytes each), row-major by block; alpha ignored."""
    px = _blocks(np.asarray(pixels, np.uint8)[..., :3])
    return b''.join(_encode_batch(px[i:i + BATCH]) for i in range(0, len(px), BATCH))
