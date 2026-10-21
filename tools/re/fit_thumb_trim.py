"""Fits the art rectangle that the game's thumbnail renders map onto the deck, from stock decks whose
art and real thumbnail are both known. Usage: fit_thumb_trim.py <stock art dir> <thumb dir> <deck number>...
(art: deck_gen_graphic_<n>_c.png; thumbnail: own_deckgraphic_gen_popsicle_<n>_lrg.png)"""
import os
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import thumbs  # noqa: E402

art_dir, thumb_dir, numbers = sys.argv[1], sys.argv[2], sys.argv[3:]
light_src = Image.open(os.path.join(thumb_dir, 'own_deckgraphic_gen_popsicle_00001_lrg.png'))
wood = Image.open(os.path.join(art_dir, 'deck_gen_graphic_00001_c.png'))
pairs = [(Image.open(os.path.join(art_dir, f'deck_gen_graphic_{n}_c.png')),
          np.asarray(Image.open(os.path.join(thumb_dir, f'own_deckgraphic_gen_popsicle_{n}_lrg.png')).convert('RGBA')).astype(np.float32))
         for n in numbers]


def error(trim):
    total = 0.0
    for art, real in pairs:
        ours = np.asarray(thumbs.make_thumbnail(art, light_src, wood, trim)).astype(np.float32)
        m = (real[..., 3] > 250) & (ours[..., 3] > 250)
        total += np.abs(ours[m][:, :3] - real[m][:, :3]).mean()
    return total / len(pairs)


trim = list(thumbs.TRIM)
best = error(tuple(trim))
print('start', trim, round(best, 2))
for step in (16, 8, 4, 2, 1):
    improved = True
    while improved:
        improved = False
        for i in range(4):
            for d in (-step, step):
                cand = trim.copy()
                cand[i] += d
                e = error(tuple(cand))
                if e < best - 1e-3:
                    best, trim, improved = e, cand, True
    print('step', step, trim, round(best, 2))
print('FIT', tuple(trim), round(best, 2))
