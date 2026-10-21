"""Builds the calibration decks into their own mod, to compare mixins in game under the same light:
"Caveman Finishes", one band per finish down the deck (default, then each of mixins.FINISHES), and one
"Caveman Holo <mode>" per holo foil mode (the mode is per deck). Every band has its name, a colour sweep,
dots and a grey ramp; the left half has the finish everywhere, the right half only on the light parts
(the `spot` modifier). Also the shader diagnostics "Caveman FX Static / Pulse / Glow" (FX_DECKS), which
need Caveman's patched deck programs (shadermod.py).
Usage: build_finishes.py <game folder> <mods folder> [--prefix "000 - "] [--mod Caveman-Finishes-Test] [--art x.png]"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np  # noqa: E402
from PIL import Image, ImageDraw, ImageFont  # noqa: E402

import mixins  # noqa: E402
import newdeck  # noqa: E402
import shaderpatch  # noqa: E402
from gamedata import Game  # noqa: E402

TOP, BOTTOM = 79, 1970          # the visible deck (deck-format.md)
FINISH_BANDS = ['default'] + list(mixins.FINISHES)


def band_rows(count):
    edges = np.linspace(TOP, BOTTOM, count + 1).round().astype(int)
    edges[0], edges[-1] = 0, 2048
    return list(zip(edges[:-1], edges[1:]))


def art_image(labels):
    """Bands down the deck, each with its label, a colour sweep with dots, and a grey ramp."""
    px = np.zeros((2048, 512, 3), np.uint8)
    hue = np.linspace(0, 1, 512, endpoint=False)
    hsv = np.dstack([hue * 255, np.full(512, 255), np.full(512, 255)]).astype(np.uint8)   # (1, 512, 3)
    sweep = np.asarray(Image.fromarray(hsv, 'HSV').convert('RGB'))[0]
    ramp = np.repeat(np.linspace(0, 255, 512)[:, None], 3, 1)
    rows = band_rows(len(labels))
    for y0, y1 in rows:
        y0, y1 = max(y0, TOP), min(y1, BOTTOM)
        h = y1 - y0
        px[y0 + h * 22 // 100:y0 + h * 70 // 100] = sweep
        px[y0 + h * 70 // 100:y1 - 4] = ramp
    img = Image.fromarray(px)
    d = ImageDraw.Draw(img)
    font = ImageFont.truetype('arialbd.ttf', 44)
    for label, (y0, y1) in zip(labels, rows):
        y0, y1 = max(y0, TOP), min(y1, BOTTOM)
        d.text((256, y0 + (y1 - y0) * 11 // 100), label.upper(), font=font, fill=(255, 255, 255), anchor='mm')
        mid = y0 + (y1 - y0) * 46 // 100
        for x in range(64, 512, 128):                 # dots: small round shapes to judge sharpness
            d.ellipse((x - 18, mid - 18, x + 18, mid + 18), fill=(255, 255, 255), outline=(0, 0, 0), width=4)
    return img


def halves(art):
    """The finish weight: everywhere on the left half, `spot` (the light parts) on the right half."""
    w = np.ones((2048, 512))
    w[:, 256:] = mixins.spot(art)[:, 256:]
    return w


def finish_layers(art, default_mask, default_normal):
    w = halves(art)
    osk = np.asarray(default_mask.convert('RGB')).copy()
    ny = np.asarray(default_normal.convert('RGB')).copy()
    for name, (y0, y1) in zip(FINISH_BANDS, band_rows(len(FINISH_BANDS))):
        if name == 'default':
            continue
        osk[y0:y1] = mixins.mask(name, art, default_mask, w)[y0:y1]
        n = mixins.normal_map({name}, art, default_normal, weight=w)
        if n is not None:
            ny[y0:y1] = n[y0:y1]
    return {'osk': osk, 'ny': ny}


def holo_layers(finish, art, default_mask):
    w = halves(art)
    return {'osk': mixins.mask(finish, art, default_mask, w), 'rgb': mixins.holo_map(w), 'foilmode': mixins.HOLO[finish]}


# Shader diagnostics. Answered in game (2026-10-20): the marker is read (a fixed half turn of the hues) and the clock
# runs (brightness pulse); EFFECT_STATIC and EFFECT_PULSE still exist. Open: how strong the glow must be to show (four
# strengths: the emissive is the colour x 2^(12 B) - 1) and whether the emissive buffer is read at all (neon: the
# emissive alone, unlit; the deck should look flat).
FX_DECKS = [('FX Glow 7', shaderpatch.EFFECT_NONE, 64), ('FX Glow 63', shaderpatch.EFFECT_NONE, 128),
            ('FX Glow 511', shaderpatch.EFFECT_NONE, 192), ('FX Glow 4095', shaderpatch.EFFECT_NONE, 255),
            ('FX Neon', shaderpatch.EFFECT_NEON, 64)]
# The first nyan test (negative trigger, S2/S8/S32) showed no trail at all (2026-10-20). Now: a control (the same ribbon in
# plain orange: does it spawn at all?), the negative trigger again, and the ratio trigger (no negative numbers) at three
# step sizes (the unit along the ribbon is not known).
NYAN_DECKS = [('Nyan Ctrl', (8, 'control')), ('Nyan Neg S8', (8, 'negative')),
              ('Nyan Ratio S2', (2, 'ratio')), ('Nyan Ratio S8', (8, 'ratio')), ('Nyan Ratio S32', (32, 'ratio'))]


def fx_layers(effect, glow, default_mask):
    holo = mixins.mark(np.zeros((2048, 512, 3), np.uint8), effect, glow=glow)
    return {'osk': np.asarray(default_mask.convert('RGB')).copy(), 'rgb': holo}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('game')
    ap.add_argument('mods')
    ap.add_argument('--prefix', default='000 - ')
    ap.add_argument('--mod', default='Caveman-Finishes-Test')
    ap.add_argument('--art', help='also save the decks\' art here (PNGs, this path with a suffix)')
    args = ap.parse_args()
    game = Game(args.game)
    default_mask, default_normal = newdeck.default_layers(game)
    decks = []
    art = art_image(FINISH_BANDS)
    decks.append(('Caveman Finishes', art, finish_layers(art, default_mask, default_normal)))
    for finish, mode in mixins.HOLO.items():
        art = art_image([f'holo {mode}', 'full | spot', '', ''])
        decks.append((f'Caveman Holo {mode}', art, holo_layers(finish, art, default_mask)))
    for name, effect, glow in FX_DECKS:
        art = art_image([name, '', '', ''])
        decks.append((f'Caveman {name}', art, fx_layers(effect, glow, default_mask)))
    for label, nyan in NYAN_DECKS:   # the solid rainbow trail: triggers and step sizes
        art = art_image([label, '', '', ''])
        decks.append((f'Caveman {label}', art, {'nyan': nyan}))
    if args.art:
        for name, art, _ in decks:
            art.save(f'{os.path.splitext(args.art)[0]}-{name.replace(" ", "-")}.png')
    built_decks = [(f'{args.prefix}{name}', 'Own_' + f'{args.prefix}{name}'.replace(' ', '_'), art, lay) for name, art, lay in decks]
    stats, built = newdeck.build(game, built_decks, args.mods, args.mod)
    print(f'built {len(built)} decks into {os.path.join(args.mods, args.mod)} ({stats["cas"] // 1024} KB)')


if __name__ == '__main__':
    main()
