"""Menu thumbnails for a deck: the art "rendered" onto the board the way the game's own thumbnails are.

A stock thumbnail is the deck seen from below, standing up, with a transparent background. Where the
art lands on it was fitted from real stock thumbnails (fit_thumb_rows.py, assets/thumbnail_mapping.json):
columns map linearly, rows follow a curve because the nose and tail bend away from the camera.
Dividing a stock thumbnail by its own deck's art (mapped that way) leaves the lighting (shading, the
rim, the tint); multiplying new art by that lighting gives a thumbnail that matches the vanilla ones."""
import json
import os

import numpy as np
from PIL import Image, ImageFilter

MAPPING = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'assets',
                                      'thumbnail_mapping.json')))


def map_art(art, size):
    """The art as it lands on a `size` x `size` thumbnail's deck (RGB, art extended past the deck edge)."""
    k = size / MAPPING['thumb_size']
    tx0, tx1 = (round(v * k) for v in MAPPING['thumb_cols'])
    ax0, ax1 = MAPPING['art_cols']
    poly = np.array(MAPPING['poly'])
    # Columns: the art's columns scaled straight onto the deck's width, extended at the sides.
    cols = np.asarray(art.convert('RGB').crop((ax0, 0, ax1, 2048)).resize((tx1 - tx0, 2048), Image.LANCZOS)).astype(np.float64)
    pad = 8
    cols = np.pad(cols, ((0, 0), (pad, pad), (0, 0)), mode='edge')
    # Rows: each thumbnail row averages the band of art rows that lands on it.
    csum = np.concatenate([np.zeros((1,) + cols.shape[1:]), np.cumsum(cols, 0)])
    edges = np.polyval(poly, (np.arange(size + 1)) / k)  # art row at each thumbnail row boundary
    edges = np.clip(edges, 0, 2047)
    out = np.zeros((size, size, 3))
    for y in range(size):
        a0, a1 = edges[y], edges[y + 1]
        i0, i1 = int(np.floor(a0)), max(int(np.floor(a0)) + 1, int(np.ceil(a1)))
        band = (csum[min(i1, 2048)] - csum[i0]) / (min(i1, 2048) - i0)
        x0 = tx0 - pad
        lo, hi = max(0, x0), min(size, x0 + band.shape[0])
        out[y, lo:hi] = band[lo - x0:hi - x0]
    return out


def lighting(stock_thumb, stock_art, blur=1.5):
    """Per-pixel light (H, W, 3) of a stock thumbnail: its colour over its own art's colour."""
    rgba = np.asarray(stock_thumb.convert('RGBA')).astype(np.float64)
    albedo = map_art(stock_art, stock_thumb.size[0])
    blur_img = lambda a: np.asarray(Image.fromarray(np.clip(a, 0, 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(blur))).astype(np.float64)
    light = blur_img(rgba[..., :3]) / np.maximum(blur_img(albedo), 8.0)
    return np.clip(light, 0, 3), rgba[..., 3]


def fit_light(pairs, size, ridge=60.0, smooth=6.0, edge=6):
    """Light (multiplied) and sheen (added) per pixel and channel, fitted over several stock decks:
    real = art x light + sheen. `pairs`: [(art image, thumbnail image of `size`)]. Also returns the
    median alpha of the thumbnails."""
    n = 0
    sa = sr = saa = sar = None
    alphas = []
    for art, thumb in pairs:
        a = map_art(art, size)
        rgba = np.asarray(thumb.convert('RGBA')).astype(np.float64)
        r = rgba[..., :3]
        alphas.append(rgba[..., 3])
        if sa is None:
            sa, sr, saa, sar = np.zeros_like(a), np.zeros_like(a), np.zeros_like(a), np.zeros_like(a)
        sa += a; sr += r; saa += a * a; sar += a * r
        n += 1
    ma, mr = sa / n, sr / n
    var = saa / n - ma * ma
    cov = sar / n - ma * mr
    light = cov / (var + ridge)
    alpha = np.median(np.stack(alphas), 0)
    # The game's renders carry blocky shading noise that differs from deck to deck (it averages into a
    # faint grid of squares). Light is smooth except at the rim, so smooth it away from the edge...
    light = _smooth_inside(light, alpha, radius=smooth, keep_edge=edge)
    # ...and fit the sheen (scratches, gloss) against the smoothed light.
    sheen = mr - light * ma
    return light, sheen, alpha


def _gauss(a, sigma):
    """Separable Gaussian blur of a 2-D float array (Pillow cannot blur float images)."""
    r = int(3 * sigma)
    k = np.exp(-0.5 * (np.arange(-r, r + 1) / sigma) ** 2)
    k /= k.sum()
    p = np.pad(a, r, mode='edge')
    p = sum(k[i] * p[:, i:i + a.shape[1]] for i in range(2 * r + 1))
    return sum(k[i] * p[i:i + a.shape[0], :] for i in range(2 * r + 1))


def _smooth_inside(img, alpha, radius, keep_edge):
    """Gaussian blur of `img` within the deck (normalized, so outside pixels do not bleed in), with the
    original kept within `keep_edge` px of the deck's edge."""
    if radius <= 0:
        return img
    inside = (alpha > 250).astype(np.float64)
    blur = lambda a: _gauss(a, radius)
    weight = blur(inside)
    out = np.empty_like(img)
    for c in range(img.shape[2]):
        out[..., c] = blur(img[..., c] * inside) / np.maximum(weight, 1e-6)
    core = Image.fromarray((inside * 255).astype(np.uint8)).filter(ImageFilter.MinFilter(2 * keep_edge + 1))
    core = np.asarray(core.filter(ImageFilter.GaussianBlur(2))).astype(np.float64)[..., None] / 255
    return core * out + (1 - core) * img


def fit_metal(pairs, size, smooth=6.0, edge=6):
    """Light of a metal deck (mask B 255), which mirrors its grey surroundings tinted by the art: real =
    art x light, one grey light per pixel (fitted per channel, the gold deck tints silver blue), by
    least squares through zero over the game's all-metal decks (so the bright ones count most).
    `pairs`: [(art image, thumbnail image of `size`)]. Returns (size, size, 1)."""
    num = den = 0
    alphas = []
    for art, thumb in pairs:
        a = map_art(art, size)
        rgba = np.asarray(thumb.convert('RGBA')).astype(np.float64)
        num = num + (a * rgba[..., :3]).sum(2, keepdims=True)
        den = den + (a * a).sum(2, keepdims=True)
        alphas.append(rgba[..., 3])
    alpha = np.median(np.stack(alphas), 0)
    return _smooth_inside(num / np.maximum(den, 1.0), alpha, radius=smooth, keep_edge=edge)


def render(art, light, sheen, alpha, metal=None, metal_light=None):
    """New art on the board, with fitted light and sheen. `metal`: the deck's metal mask (an image the
    size of the art, 255 metal), lit by `metal_light` (fit_metal). Returns RGBA."""
    mapped = map_art(art, alpha.shape[0])
    rgb = mapped * light + sheen
    if metal is not None:
        m = map_art(metal, alpha.shape[0])[..., :1] / 255
        rgb = (1 - m) * rgb + m * mapped * metal_light
    rgb = np.clip(rgb, 0, 255)
    rgb[alpha < 1] = 0  # the game's thumbnails are black where transparent
    return Image.fromarray(np.dstack([rgb, alpha]).astype(np.uint8), 'RGBA')


def make_thumbnail(art, stock_thumb, stock_art):
    """`art`: new 512 x 2048 deck art. `stock_thumb` / `stock_art`: a stock deck's thumbnail and its
    own art, whose lighting is reused. Returns an RGBA thumbnail the size of `stock_thumb`."""
    light, alpha = lighting(stock_thumb, stock_art)
    rgb = np.clip(map_art(art, stock_thumb.size[0]) * light, 0, 255)
    rgb[alpha == 0] = 0  # the game's thumbnails are black where transparent
    return Image.fromarray(np.dstack([rgb, alpha]).astype(np.uint8), 'RGBA')
