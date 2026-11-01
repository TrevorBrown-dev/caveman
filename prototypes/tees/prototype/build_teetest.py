"""Builds the T-shirt test mod (not installed) and its stencil, inputs and previews:
  <out>/Caveman-Tee-Test/        the mod
  <out>/Caveman Tee Stencil 2048.png
  <out>/art/                      the inputs (the test pattern sheet, the real graphic's front and back art)
  <out>/previews/                 per tee: the sheet, a plain front/back render, the menu thumbnail
Usage: build_teetest.py <out dir> [game root]  (test 2: the shirt colour fix)"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import numpy as np
from PIL import Image, ImageDraw, ImageFilter

import teelib
import teethumb
from gamedata import Game

out = sys.argv[1]
game = Game(sys.argv[2] if len(sys.argv) > 2 else r'E:\Games\ReSkate')
import newtee  # noqa: E402  (after gamedata: the toolkit path is set by teelib)

os.makedirs(os.path.join(out, 'art'), exist_ok=True)
os.makedirs(os.path.join(out, 'previews'), exist_ok=True)
tee = teelib.Tee(game)
print('panels:', ', '.join(f'{k} {v["m_per_u"]:.2f}x{v["m_per_v"]:.2f} m/unit' for k, v in tee.panels.items()))

# ---- the stencil (the artist's guide) and the test pattern (an input sheet drawn on it)
teelib.stencil(tee, game, 2048).save(os.path.join(out, 'Caveman Tee Stencil 2048.png'))
pattern = teelib.test_pattern(tee, 2048)
pattern.save(os.path.join(out, 'art', '000 - Tee Test Pattern.png'))


# ---- a real-looking graphic: a front wordmark (wide: exercises the fit mode) and a back print
def sunset_badge(size=1100):
    img = Image.new('RGBA', (size, int(size * 1.15)), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    cx, cy, r = size / 2, size * 0.46, size * 0.40
    sun = Image.new('RGBA', img.size, (0, 0, 0, 0))
    sd = ImageDraw.Draw(sun)
    top, bottom = np.array([255, 214, 90]), np.array([236, 64, 122])
    for y in range(int(cy - r), int(cy + r) + 1):
        t = (y - (cy - r)) / (2 * r)
        c = tuple(int(v) for v in top * (1 - t) + bottom * t) + (255,)
        half = (max(0.0, r * r - (y - cy) ** 2)) ** 0.5
        sd.line([(cx - half, y), (cx + half, y)], fill=c)
    # the sun's stripes, thicker towards the bottom
    for k in range(6):
        y = cy + r * (0.15 + 0.14 * k)
        sd.rectangle([0, y, size, y + 6 + 4 * k], fill=(0, 0, 0, 0))
    img.alpha_composite(sun)
    # mountains and a skater-ish ramp line
    d.polygon([(cx - r * 1.15, cy + r * 0.62), (cx - r * 0.45, cy - r * 0.05), (cx - r * 0.1, cy + r * 0.35), (cx + r * 0.35, cy - r * 0.25),
               (cx + r * 1.15, cy + r * 0.62)], fill=(24, 30, 52, 255))
    d.polygon([(cx - r * 0.45, cy - r * 0.05), (cx - r * 0.33, cy + r * 0.07), (cx - r * 0.52, cy + r * 0.05)], fill=(240, 240, 250, 255))
    d.polygon([(cx + r * 0.35, cy - r * 0.25), (cx + r * 0.5, cy - r * 0.07), (cx + r * 0.22, cy - r * 0.1)], fill=(240, 240, 250, 255))
    f = teelib.font(int(size * 0.115))
    for txt, y, col in (('CAVEMAN', cy + r * 0.72, (255, 214, 90)), ('SKATE CO.', cy + r * 0.98, (236, 64, 122))):
        box = d.textbbox((0, 0), txt, font=f)
        d.text((cx - (box[2] - box[0]) / 2, y), txt, font=f, fill=col + (255,), stroke_width=4, stroke_fill=(24, 30, 52, 255))
    f2 = teelib.font(int(size * 0.045))
    box = d.textbbox((0, 0), 'EST. 2026  -  BUILT FROM A PNG', font=f2)
    d.text((cx - (box[2] - box[0]) / 2, cy + r * 1.3), 'EST. 2026  -  BUILT FROM A PNG', font=f2, fill=(230, 230, 240, 255))
    return img


def wordmark():
    img = Image.new('RGBA', (1400, 420), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([20, 40, 1380, 380], radius=60, outline=(255, 214, 90, 255), width=18)
    f = teelib.font(210)
    box = d.textbbox((0, 0), 'CAVEMAN', font=f)
    d.text((700 - (box[2] - box[0]) / 2, 210 - (box[3] - box[1]) / 2 - box[1]), 'CAVEMAN', font=f, fill=(245, 245, 245, 255))
    return img


front_art, back_art = wordmark(), sunset_badge()
front_art.save(os.path.join(out, 'art', '000 - Tee Test Graphic.front.png'))
back_art.save(os.path.join(out, 'art', '000 - Tee Test Graphic.back.png'))
sheet_front = teelib.fit_art(front_art, tee, 'front')
sheet_back = teelib.fit_art(back_art, tee, 'back')
graphic = Image.alpha_composite(sheet_front, sheet_back)
# the front art is a chest print: shrink it to 45% of the panel width, centred, 9% down
pn = tee.panels['FRONT']
u0, v0, u1, v1 = pn['box']
w = (u1 - u0) * 0.45
h = w * front_art.height / front_art.width * pn['m_per_u'] / pn['m_per_v']
chest = Image.new('RGBA', (1024, 1024), (0, 0, 0, 0))
small = front_art.resize((int(w * 1024), int(h * 1024)), Image.LANCZOS)
chest.paste(small, (int(((u0 + u1) / 2 - w / 2) * 1024), int((v0 + (v1 - v0) * 0.09) * 1024)), small)
graphic = Image.alpha_composite(sheet_back, chest)
# a sleeve tag: "66" style number on the right sleeve, drawn flat (squashed along the sleeve)
sl = tee.panels['R SLEEVE']
su0, sv0, su1, sv1 = sl['box']
tag = teelib._text('C-26', 120, (255, 214, 90, 255), sl['m_per_u'] / sl['m_per_v'])
tag = tag.resize((int(tag.width * 0.6), int(tag.height * 0.6)), Image.LANCZOS)
graphic.paste(tag, (int((su0 + su1) / 2 * 1024 - tag.width / 2), int((sv0 + (sv1 - sv0) * 0.35) * 1024)), tag)
graphic.save(os.path.join(out, 'art', '000 - Tee Test Graphic (sheet).png'))

# ---- thumbnails, previews, textures
fit = teethumb.fit(game)
tees = []
RED = (190, 40, 40)
specs = [('000 - Tee Test 2 Pattern', pattern.resize((1024, 1024), Image.LANCZOS), RED, 'plain'),
         ('000 - Tee Test 2 Graphic', graphic, (38, 38, 42), 'plain'),
         ('000 - Tee Test 2 Control Red', pattern.resize((1024, 1024), Image.LANCZOS), RED, 'vans-tint')]
icon = None
for display, sheet, shirt, variant in specs:
    key = 'Own_' + display.replace(' ', '_')
    colour, mask, alpha = teelib.sheet_textures(sheet, tee)
    rgba = np.dstack([colour, alpha]).astype(np.uint8)
    # the Vans jersey is dark: its base colour times the region colour (x2, 0.5 neutral)
    thumb_shirt = shirt if variant == 'plain' else (45, 45, 48) if variant == 'vans' else tuple(int(v) for v in newtee.linear_to_srgb(newtee.srgb_to_linear(shirt) * 2 * 0.033))
    thumbs = teethumb.render(fit, thumb_shirt, rgba)
    base = display.replace(' ', '_')
    thumbs[768].save(os.path.join(out, 'previews', f'{base} thumbnail.png'))
    teelib.preview(tee, Image.fromarray(rgba, 'RGBA'), thumb_shirt).save(os.path.join(out, 'previews', f'{base} front and back.png'))
    Image.fromarray(rgba, 'RGBA').save(os.path.join(out, 'previews', f'{base} sheet.png'))
    tees.append((display, key, colour, mask, shirt, thumbs, variant))
    icon = icon or thumbs[256]
stats, built = newtee.build_tees(game, tees, out, 'Caveman-Tee-Test-2', 'caveman_tee_test_2', icon=icon)
print(stats)
