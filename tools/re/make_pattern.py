"""Deck outline (from the stock decks' consensus) and an orientation test pattern.
Usage: make_pattern.py <outline.npz> <out dir>"""
import json
import os
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

p = np.load(sys.argv[1])['p']
out_dir = sys.argv[2]
os.makedirs(out_dir, exist_ok=True)
W, H = 512, 2048
THR = 0.62
cols = p[:, 200:312].mean(1)
inside_rows = np.where(cols < THR)[0]
top, bottom = int(inside_rows.min()), int(inside_rows.max())
left, right = [], []
for y in range(top, bottom + 1):
    xs = np.where(p[y] < THR)[0]
    left.append(int(xs.min()) if len(xs) else 256)
    right.append(int(xs.max()) if len(xs) else 256)


def smooth(v, k=9):
    v = np.array(v, float)
    pad = np.pad(v, k, mode='edge')
    return [float(np.median(pad[i:i + 2 * k + 1])) for i in range(len(v))]


left, right = smooth(left), smooth(right)
outline = {'width': W, 'height': H, 'top': top, 'bottom': bottom,
           'rows': [[top + i, round(left[i], 1), round(right[i], 1)] for i in range(0, len(left), 4)]}
json.dump(outline, open(os.path.join(out_dir, 'deck_outline.json'), 'w'))
poly = [(left[i], top + i) for i in range(len(left))] + [(right[i], top + i) for i in reversed(range(len(right)))]

# Orientation test pattern: hue runs red (top) to blue (bottom), a grid every 128 px, labels.
img = Image.new('RGB', (W, H))
px = np.zeros((H, W, 3), np.uint8)
t = np.linspace(0, 1, H)[:, None]
px[..., 0] = (255 * (1 - t)).astype(np.uint8)
px[..., 1] = (60 + 60 * np.sin(np.linspace(0, 6.28, W)))[None, :].astype(np.uint8)
px[..., 2] = (255 * t).astype(np.uint8)
img = Image.fromarray(px)
d = ImageDraw.Draw(img)
font = ImageFont.load_default(size=56)
small = ImageFont.load_default(size=26)
for y in range(0, H, 128):
    d.line([(0, y), (W, y)], fill=(255, 255, 255), width=2)
    d.text((W // 2 - 30, y + 4), str(y), fill=(255, 255, 0), font=small)
for x in range(0, W, 128):
    d.line([(x, 0), (x, H)], fill=(255, 255, 255), width=2)
d.polygon(poly, outline=(0, 255, 0), width=6)
d.text((150, 330), 'TOP', fill=(255, 255, 255), font=font, stroke_width=3, stroke_fill=(0, 0, 0))
d.text((110, 1640), 'BOTTOM', fill=(255, 255, 255), font=font, stroke_width=3, stroke_fill=(0, 0, 0))
d.text((40, 1000), 'L', fill=(255, 255, 255), font=font, stroke_width=3, stroke_fill=(0, 0, 0))
d.text((430, 1000), 'R', fill=(255, 255, 255), font=font, stroke_width=3, stroke_fill=(0, 0, 0))
d.text((150, 900), 'CAVEMAN', fill=(0, 0, 0), font=small)
img.save(os.path.join(out_dir, 'test_pattern.png'))

# Artist template: transparent outside, outline + safe area, at native size.
tpl = Image.new('RGBA', (W, H), (0, 0, 0, 0))
d = ImageDraw.Draw(tpl)
d.polygon(poly, fill=(255, 255, 255, 255), outline=(255, 0, 0, 255), width=3)
tpl.save(os.path.join(out_dir, 'deck_template_512x2048.png'))
print('outline top', top, 'bottom', bottom, 'mid row', left[(1024 - top)], right[(1024 - top)])
