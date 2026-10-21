"""Finds, for every row of the deck in a 768 px stock thumbnail, which row of the 512 x 2048 art it
shows, by matching rows of real thumbnails against their decks' art (normalized correlation, so the
render's lighting does not matter). Writes a smooth monotonic row map as JSON.
Usage: fit_thumb_rows.py <stock art dir> <thumb dir> <out json> <x0> <x1> <deck number>..."""
import json
import os
import sys

import numpy as np
from PIL import Image

art_dir, thumb_dir, out_json = sys.argv[1:4]
ax0, ax1 = int(sys.argv[4]), int(sys.argv[5])
numbers = sys.argv[6:]

decks = []
for n in numbers:
    art = np.asarray(Image.open(os.path.join(art_dir, f'deck_gen_graphic_{n}_c.png')).convert('L')).astype(np.float32)
    real = np.asarray(Image.open(os.path.join(thumb_dir, f'own_deckgraphic_gen_popsicle_{n}_lrg.png')).convert('RGBA')).astype(np.float32)
    decks.append((art, real))

alpha = decks[0][1][..., 3]
ys, xs = np.where(alpha > 128)
tx0, tx1, ty0, ty1 = xs.min(), xs.max() + 1, ys.min(), ys.max() + 1
width = tx1 - tx0
# Art columns sampled at the thumbnail's deck columns (x mapping is linear).
cols = np.linspace(ax0, ax1 - 1, width)
ci = np.clip(np.rint(cols).astype(int), 0, 511)


def zn(v):
    v = v - v.mean()
    n = np.linalg.norm(v)
    return v / n if n > 1e-6 else v


cand = np.arange(0, 2048)
scores = np.zeros((ty1 - ty0, len(cand)), np.float32)
for art, real in decks:
    lum = real[..., :3].mean(2)
    art_rows = np.stack([zn(art[a, ci]) for a in cand])  # (2048, width)
    for i, y in enumerate(range(ty0, ty1)):
        inside = real[y, tx0:tx1, 3] > 250
        if inside.sum() < width * 0.6:
            continue
        t = lum[y, tx0:tx1].copy()
        t[~inside] = t[inside].mean()
        scores[i] += art_rows @ zn(t)
# Monotonic best path (dynamic programming): row y+1 maps at or below row y's art row.
n_rows = ty1 - ty0
best = np.full((n_rows, len(cand)), -1e9, np.float32)
back = np.zeros((n_rows, len(cand)), np.int32)
best[0] = scores[0]
for i in range(1, n_rows):
    run = np.maximum.accumulate(best[i - 1])
    arg = np.zeros(len(cand), np.int32)
    cur = 0
    for a in range(len(cand)):  # index of the running maximum
        if best[i - 1][a] >= best[i - 1][cur]:
            cur = a
        arg[a] = cur
    best[i] = run + scores[i]
    back[i] = arg
path = np.zeros(n_rows, int)
path[-1] = int(best[-1].argmax())
for i in range(n_rows - 1, 0, -1):
    path[i - 1] = back[i][path[i]]
# Smooth with a low-order polynomial in thumbnail y, then report.
yy = np.arange(ty0, ty1)
valid = scores.max(1) > 0
coef = np.polyfit(yy[valid], path[valid], 5)
smooth = np.polyval(coef, yy)
resid = np.abs(smooth[valid] - path[valid])
print(f'thumbnail deck rows {ty0}..{ty1}, cols {tx0}..{tx1}; art rows at the ends: {smooth[0]:.0f} .. {smooth[-1]:.0f}')
print('median |path - fit|:', float(np.median(resid)), ' 90th pct:', float(np.percentile(resid, 90)))
for y in range(ty0, ty1, 48):
    i = y - ty0
    print(f'  thumb y {y}: art row {path[i]} (fit {smooth[i]:.0f})')
json.dump({'thumb_size': 768, 'thumb_rows': [int(ty0), int(ty1)], 'thumb_cols': [int(tx0), int(tx1)],
           'art_cols': [ax0, ax1], 'poly': [float(c) for c in coef]}, open(out_json, 'w'), indent=1)
