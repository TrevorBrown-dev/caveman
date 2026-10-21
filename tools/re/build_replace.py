"""Milestone 1: a mod that replaces one stock deck's art with a PNG, board texture and menu thumbnails.
Usage: build_replace.py <game root> <png> <out mods dir> <mod folder name> [deck number, default 00001]

Deck n is the bundle deck_gen_graphic_<n> (texture deck_gen_graphic_<n>_c) and the item
Own_DeckGraphic_Gen_Popsicle_<n> (thumbnails thumbnail/tool/own_deckgraphic_gen_popsicle_<n> and _lrg)."""
import os
import sys
import time

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bc7  # noqa: E402
import thumbs  # noqa: E402
from gamedata import Game  # noqa: E402
from patch import Patch  # noqa: E402

game_root, png, out_root, mod_name = sys.argv[1:5]
number = sys.argv[5] if len(sys.argv) > 5 else '00001'
SHARED = 'win32/characters/customization/configs/cas_main_sharedbundle'
LIGHT_DECKS = 40                       # stock decks the thumbnail lighting is fitted from
SKIP = {'00041', '00042'}              # stock decks whose thumbnail does not match their art


def deck_bundle_name(game, n):
    return game.find_bundle(f'/deck_gen_graphic_{n}_ap_cas_main_bundlereftable')


def deck_texture_name(bundle, n):
    return next(r['name'] for r in bundle['manifest']['res'] if r['name'].endswith(f'/deck_gen_graphic_{n}_c'))


def mips_bc7(img, levels):
    out, w, h = [], img.width, img.height
    for m in range(levels):
        level = img if m == 0 else img.resize((max(1, w >> m), max(1, h >> m)), Image.LANCZOS)
        out.append(bc7.encode(np.asarray(level)))
    return b''.join(out)


t0 = time.time()
game = Game(game_root)
patch = Patch(game)

# ---- the board texture
art = Image.open(png).convert('RGB')
if art.size != (512, 2048):
    print(f'note: {art.size} resized to 512x2048')
    art = art.resize((512, 2048), Image.LANCZOS)
deck_bundle = deck_bundle_name(game, number)
tex = deck_texture_name(game.bundle(deck_bundle), number)
h = patch.replace_texture_pixels(deck_bundle, tex, mips_bc7(art, 12))
patch.make_texture_whole(deck_bundle, tex)  # never stream a cosmetic texture from a mod
print(f'board: {tex} ({h["w"]}x{h["h"]}, {h["mips"]} mips)  {time.time() - t0:.1f}s')

# ---- the menu thumbnails: the art on the board, lit like the game's own thumbnails
shared = game.bundle(SHARED)
names = {r['name'] for r in shared['manifest']['res']}
candidates = []
for b in sorted(game.toc()['bundle_by_name']):
    if '/deck/common/generic/graphic/deck_gen_graphic_' in b and b.endswith('_ap_cas_main_bundlereftable'):
        n = b.split('deck_gen_graphic_')[-1].split('_')[0]
        if n not in SKIP and {f'thumbnail/tool/own_deckgraphic_gen_popsicle_{n}{s}' for s in ('', '_lrg')} <= names:
            candidates.append(n)
training = candidates[::max(1, len(candidates) // LIGHT_DECKS)][:LIGHT_DECKS]
arts = []
for n in training:
    b = game.bundle(deck_bundle_name(game, n))
    arts.append(game.texture_image(b, deck_texture_name(b, n)))
for suffix, size in (('_lrg', 768), ('', 256)):
    pairs = [(a, game.texture_image(shared, f'thumbnail/tool/own_deckgraphic_gen_popsicle_{n}{suffix}'))
             for a, n in zip(arts, training)]
    light, sheen, alpha = thumbs.fit_light(pairs, size)
    thumb = thumbs.render(art, light, sheen, alpha)
    name = f'thumbnail/tool/own_deckgraphic_gen_popsicle_{number}{suffix}'
    patch.replace_texture_pixels(SHARED, name, bc7.encode(np.asarray(thumb)))
    thumb.save(os.path.join(out_root, f'{mod_name}-thumb{suffix or "_sm"}.png'))
    print(f'thumbnail: {name} ({size}px, lighting from {len(pairs)} stock decks)  {time.time() - t0:.1f}s')

# ---- the mod
icon = Image.new('RGBA', (256, 256), (0, 0, 0, 0))
icon.paste(Image.open(os.path.join(out_root, f'{mod_name}-thumb_sm.png')), (0, 0))
stats = patch.write(os.path.join(out_root, mod_name), mod_name,
                    f'Caveman test: replaces the art of stock deck {number} (Natural Blank) with {os.path.basename(png)}.',
                    icon=icon)
print('wrote', os.path.join(out_root, mod_name), stats, f'{time.time() - t0:.1f}s')
