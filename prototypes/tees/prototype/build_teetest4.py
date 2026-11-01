"""Builds tee test 4 (the chocolate multi-logo host, newtee3.py): test 3's three tees plus plain tees, a fully
transparent sheet in every logo slot over a region colour (Trevor: "a plain white tee that we own").
  <out>/Caveman-Tee-Test-4/      the mod: 000 - Tee Test 3 Pattern, Graphic, Control Blue (as test 3) and
                                 000 - Tee Test 4 Plain White / Black / 00E5FF / FF6A00
  <out>/previews/                 per tee: the sheet, a plain front/back render, the menu thumbnail
Usage: build_teetest4.py <out dir> [game root]"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import numpy as np
from PIL import Image

import teelib
import teethumb
from gamedata import Game

out = sys.argv[1]
game = Game(sys.argv[2] if len(sys.argv) > 2 else r'E:\Games\ReSkate')
import newtee3  # noqa: E402

art = os.path.join(out, 'art')
os.makedirs(os.path.join(out, 'previews'), exist_ok=True)
tee = teelib.Tee(game)
pattern = Image.open(os.path.join(art, '000 - Tee Test Pattern.png')).convert('RGBA').resize((1024, 1024), Image.LANCZOS)
graphic = Image.open(os.path.join(art, '000 - Tee Test Graphic (sheet).png')).convert('RGBA')
clear = Image.new('RGBA', (1024, 1024), (0, 0, 0, 0))
RED, BLACK, BLUE = (190, 40, 40), (38, 38, 42), (40, 80, 200)
specs = [('000 - Tee Test 3 Pattern', pattern, RED, 'print'),
         ('000 - Tee Test 3 Graphic', graphic, BLACK, 'print'),
         ('000 - Tee Test 3 Control Blue', None, BLUE, 'colour'),
         # plain tees: Name.<hex>.png with a transparent PNG
         ('000 - Tee Test 4 Plain White', clear, (255, 255, 255), 'print'),
         ('000 - Tee Test 4 Plain Black', clear, (17, 17, 17), 'print'),
         ('000 - Tee Test 4 Plain 00E5FF', clear, (0, 229, 255), 'print'),
         ('000 - Tee Test 4 Plain FF6A00', clear, (255, 106, 0), 'print')]
fit = teethumb.fit(game)
tees, icon = [], None
for display, sheet, shirt, variant in specs:
    key = 'Own_' + display.replace(' ', '_')
    base = display.replace(' ', '_')
    if sheet is not None:
        colour, mask, alpha = teelib.sheet_textures(sheet, tee)
        rgba = np.dstack([colour, alpha]).astype(np.uint8)
        Image.fromarray(rgba, 'RGBA').save(os.path.join(out, 'previews', f'{base} sheet.png'))
    else:
        rgba = np.zeros((1024, 1024, 4), np.uint8)   # the control keeps the stock chocolate logos (not drawn here)
    thumbs = teethumb.render(fit, shirt, rgba)
    thumbs[768].save(os.path.join(out, 'previews', f'{base} thumbnail.png'))
    teelib.preview(tee, Image.fromarray(rgba, 'RGBA'), shirt).save(os.path.join(out, 'previews', f'{base} front and back.png'))
    tees.append((display, key, rgba if sheet is not None else None, shirt, thumbs, variant))
    icon = icon or thumbs[256]
stats, built = newtee3.build_tees(game, tees, out, 'Caveman-Tee-Test-4', icon=icon)
print(stats)
