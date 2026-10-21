"""Predict stock tee thumbnails from their presets (decal placement in UV1) and compare with the real ones."""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import numpy as np
from PIL import Image
import teethumb as tt
from gamedata import Game

g = Game(r'E:\Games\ReSkate')
f = tt.fit(g)
img = os.path.join(HERE, 'img')


def decal(sheet, tex_rgba, oh, ov, scale, aspect=1.0):
    """Paint a decal into a UV1 sheet (RGBA float, NxN) at centre (0.5 + oh, 0.5 - ov), size scale (x aspect wide)."""
    n = sheet.shape[0]
    yy, xx = np.mgrid[0:n, 0:n]
    u, v = (xx + 0.5) / n, (yy + 0.5) / n
    du = (u - (0.5 + oh)) / (scale * aspect) + 0.5
    dv = (v - (0.5 - ov)) / scale + 0.5
    inside = (du >= 0) & (du < 1) & (dv >= 0) & (dv < 1)
    t = tt.sample(np.asarray(tex_rgba, np.float64), np.stack([du, dv], -1))
    a = (t[..., 3:] / 255) * inside[..., None]
    sheet[..., :3] = t[..., :3] * a + sheet[..., :3] * (1 - a)
    sheet[..., 3:] = np.maximum(sheet[..., 3:], a * 255)


def compare(name, shirt_lin, sheet, real_thumb):
    pred = tt.render(f, tt.srgb(shirt_lin), sheet)[768]
    real = Image.open(os.path.join(img, real_thumb)).convert('RGBA')
    both = Image.new('RGBA', (1536, 768))
    both.paste(pred, (0, 0))
    both.paste(real, (768, 0))
    both.save(os.path.join(img, f'pred_{name}.png'))
    p = np.asarray(pred).astype(float)[..., :3]
    r = np.asarray(real).astype(float)[..., :3]
    m = f['shirt']
    print(name, 'median abs error in the shirt', np.median(np.abs(p - r)[m]).round(2), 'p90', np.percentile(np.abs(p - r)[m], 90).round(2))


# cherries tee (00009): multilogo, graphic0 cherries (front chest), graphic1 monster ring (back)
N = 1024
sheet = np.zeros((N, N, 4))
decal(sheet, Image.open(os.path.join(img, 'logo_monsters_2x2_001_co.png')).convert('RGBA'), -0.335, 0.336, 0.11)
decal(sheet, Image.open(os.path.join(img, 'logo_monsters_2x2_002_co.png')).convert('RGBA'), 0.256, 0.109, 0.344)
compare('00009', (0.708, 0.396, 0.250), sheet, 'thumb_own_topshirt_gen_tshirtrelaxed_00009_lrg.png')
Image.fromarray(sheet.astype(np.uint8)).save(os.path.join(img, 'sheet_00009.png'))

# I heart SV tee (00008): single graphic; osk R = print, B = tint1 (red) else tint (black)
osk = np.asarray(Image.open(os.path.join(img, 'logo_ilovesv_adv_2x2_001_osk.png')).convert('RGB')).astype(float)
tint, tint1 = tt.srgb((0.036, 0.036, 0.036)), tt.srgb((0.394, 0.003, 0.005))
b = osk[..., 2:3] / 255
rgba = np.dstack([tint1 * b + tint * (1 - b), osk[..., 0]])
sheet = np.zeros((N, N, 4))
decal(sheet, rgba, -0.228, 0.234, 0.338)
compare('00008', (0.485, 0.485, 0.485), sheet, 'thumb_own_topshirt_gen_tshirtrelaxed_00008_lrg.png')
