"""Caveman: turns a folder of PNGs into ReSkate decks.

    python caveman.py [--game E:\\Games\\ReSkate] [--decks <folder>] [--check]

Each PNG is one deck, named after its file. `Cool Deck.png` is the deck "Cool Deck"; effect suffixes
(`Cool Deck.foil.png`, `Cool Deck.holo-frost.spot.png`) build a variant with those effects, named "Cool
Deck Foil" (one finish: gloss, matte, foil, chrome, glitter, holo, holo-cellular, holo-frost,
holo-vertical, holo-glitter; plus spot, for the light parts only, emboss, and the effects cycle, glow
and aura: a light and trails on the board, in a hex colour such as `.aura.00ffff` or the art's own).
Subfolders put
their names first: `000\\Cool Deck.png` is "000 - Cool Deck" (handy for sorting decks to the top). Images of any size
are fitted to the deck: scaled to cover it and centred (the edges that do not fit are cropped), turned
upright when drawn sideways, and flattened onto natural wood where transparent.

It writes a preview of each deck beside the PNGs, then builds them all into one mod (by default
`Caveman-Decks` in the game's Mods folder; `--out` writes it elsewhere)."""
import argparse
import os
import re
import sys
import unicodedata

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'tools', 're'))
import numpy as np  # noqa: E402
from PIL import Image, ImageDraw  # noqa: E402

import fb  # noqa: E402
import fbw  # noqa: E402
import mixins  # noqa: E402
from gamedata import Game  # noqa: E402

ART_SIZE = (512, 2048)
MIN_WIDTH = 256                      # narrower art is built, with a warning that it will look soft
MAX_SIDE = 16384
OUTLINE = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'assets', 'deck_outline.json')
WOOD = 'deck_gen_graphic_00001'      # Natural Blank: what shows through transparent parts


class Deck:
    def __init__(self, path, name, effects, colour=None):
        finish, mods, self.notes = mixins.split(effects)
        self.path, self.name, self.colour = path, name, colour
        self.effects = ([finish] if finish else []) + mods
        words = [e.replace('-', ' ').title() for e in self.effects]
        if colour:
            words.append(''.join(f'{c:02X}' for c in colour))
        self.display = ' '.join([name] + words)
        self.item_id = 'Own_' + self.display.replace(' ', '_')


def clean_name(text):
    """A file name as a deck name: plain ASCII letters, digits, hyphens and single spaces (the game's
    own item keys use hyphens, as in `sci-fi`)."""
    text = unicodedata.normalize('NFKD', text).encode('ascii', 'ignore').decode()
    text = re.sub(r'[_.]+', ' ', text)
    text = re.sub(r'[^A-Za-z0-9 \-]+', '', text)
    return re.sub(r' +', ' ', text).strip(' -')


PREVIEWS = 'previews'                # Caveman's own folder beside the PNGs, never read as decks


def read_folder(folder, prefix='', top=True):
    """Every PNG in the folder and its subfolders. A subfolder's name goes before its decks' names:
    `000\\Cool Deck.png` is "000 - Cool Deck", `000\\Anime\\Cool Deck.png` "000 - Anime - Cool Deck"."""
    decks = []
    for entry in sorted(os.listdir(folder)):
        path = os.path.join(folder, entry)
        if os.path.isdir(path):
            if entry.startswith('.') or (top and entry.lower() == PREVIEWS):
                continue
            decks += read_folder(path, f'{prefix}{entry} - ', top=False)
            continue
        if not entry.lower().endswith('.png') or not os.path.isfile(path):
            continue
        stem = entry[:-4]
        effects = []                 # suffixes, peeled off the end: `Deck.foil.emboss` -> foil, emboss
        colour = None                # and a hex colour among them: `Deck.aura.00ffff`
        while '.' in stem:
            last = stem.rsplit('.', 1)[1]
            if last.lower() in mixins.EFFECTS:
                effects.insert(0, last.lower())
            elif mixins.hex_colour(last) and colour is None:
                colour = mixins.hex_colour(last)
            else:
                break
            stem = stem.rsplit('.', 1)[0]
        name = clean_name(prefix + stem)
        if not name:
            print(f'  skipped {os.path.relpath(path, folder)}: no usable letters or digits in its name')
            continue
        decks.append(Deck(path, name, effects, colour))
    return decks


def fit(img, wood, notes):
    """The image as 512 x 2048 deck art: upright, covering the deck, flattened onto wood."""
    if max(img.size) > MAX_SIDE:
        raise ValueError(f'{img.size[0]} x {img.size[1]} is too large (at most {MAX_SIDE} px a side)')
    w, h = img.size
    if w > h and 3.0 <= w / h <= 5.0:          # drawn sideways, as plenty of deck art is
        img = img.rotate(-90, expand=True)     # clockwise: its left edge becomes the top of the deck
        notes.append('turned upright (it was drawn sideways; its left edge is now the top)')
        w, h = img.size
    target = ART_SIZE[0] / ART_SIZE[1]
    if abs(w / h - target) / target > 0.03:
        scale = max(ART_SIZE[0] / w, ART_SIZE[1] / h)
        cw, ch = round(ART_SIZE[0] / scale), round(ART_SIZE[1] / scale)
        left, top = (w - cw) // 2, (h - ch) // 2
        img = img.crop((left, top, left + cw, top + ch))
        notes.append(f'cropped to fill the deck ({w} x {h} is not 1:4)')
    if img.width < MIN_WIDTH:
        notes.append(f'only {img.width} px wide: it will look soft (1024 x 4096 is ideal)')
    img = img.convert('RGBA').resize(ART_SIZE, Image.LANCZOS)
    if np.asarray(img)[..., 3].min() < 255:
        notes.append('transparent parts show natural wood')
    return Image.alpha_composite(wood.convert('RGBA'), img).convert('RGB')


def existing_items(game, mods_dir, skip):
    """Lower-case item ids already taken: the game's and every installed mod's ('own_...')."""
    taken = {}
    shared = game.bundle('win32/characters/customization/configs/cas_main_sharedbundle')
    for e in shared['manifest']['ebx']:
        if e['name'].startswith('items/'):
            taken[e['name'].rsplit('/', 1)[1]] = 'the game'
    for mod in sorted(os.listdir(mods_dir)) if os.path.isdir(mods_dir) else []:
        toc = os.path.join(mods_dir, mod, 'Win32', 'items.toc')
        if mod.startswith('.') or mod == skip or not os.path.isfile(toc):
            continue
        try:
            t = fb.read_toc(toc)
            layer = ModLayer(os.path.join(mods_dir, mod), game)
            for b in t['bundles']:
                if b['name'].endswith('cas_main_sharedbundle') and b['region'] is not None:
                    m, files = layer.bundle(b['region'])
                    for e, f in zip(m['ebx'], files):
                        if f['patch'] and e['name'].startswith('items/'):
                            taken.setdefault(e['name'].rsplit('/', 1)[1], mod)
        except Exception as error:  # a mod that cannot be read cannot collide either
            print(f'  note: could not read {mod} ({error})')
    return taken


class ModLayer(fb.Layer):
    """A mod folder whose patch files are in its own cas and the rest in the game's."""

    def __init__(self, root, game):
        super().__init__(root, game.root)
        self.game = game

    def read_raw(self, f):
        if not f['patch']:
            return self.game.raw(f)
        cands = self.cas.get(f['archive'], [])
        if len(cands) > 1:  # archives in several package folders: pick by install chunk
            want = 'initialinstallpackage' if f['install'] == fbw.MOD_INSTALL_CHUNK else 'defaultinstallpackage'
            cands = [c for c in cands if c[0].endswith(want)] or cands
        with open(cands[0][1], 'rb') as h:
            h.seek(f['offset'])
            return h.read(f['size'])


def preview(art, outline):
    """The art with everything off the board darkened: what makes it onto the deck."""
    import json
    rows = json.load(open(outline))['rows']
    poly = [(l, y) for y, l, r in rows] + [(r, y) for y, l, r in reversed(rows)]
    mask = Image.new('L', ART_SIZE, 90)
    ImageDraw.Draw(mask).polygon(poly, fill=255)
    dark = Image.new('RGB', ART_SIZE, (20, 20, 20))
    return Image.composite(art, dark, mask)


def main():
    ap = argparse.ArgumentParser(description='Turn a folder of PNGs into ReSkate decks.')
    ap.add_argument('--game', default=r'E:\Games\ReSkate', help='the ReSkate game folder (with Skate.exe)')
    ap.add_argument('--decks', help='the folder of PNGs (default: "Caveman Decks" in the game folder)')
    ap.add_argument('--mod', default='Caveman-Decks', help='the mod folder Caveman writes')
    ap.add_argument('--out', help='the folder the mod is written to (default: the game\'s Mods folder)')
    ap.add_argument('--prefix', default='', help='text put before every deck name, e.g. "000 - " to list them first while testing')
    ap.add_argument('--if-changed', action='store_true',
                    help='do nothing when the PNGs, the options and Caveman itself are as at the last build (for launchers)')
    args = ap.parse_args()
    decks_dir = args.decks or os.path.join(args.game, 'Caveman Decks')
    if not os.path.isdir(decks_dir):
        os.makedirs(decks_dir)
        print(f'Made {decks_dir}: put deck PNGs in it and run Caveman again.')
        return
    mods_dir = args.out or os.path.join(args.game, 'Mods')
    mod_dir = os.path.join(mods_dir, args.mod)
    stamp = fingerprint(decks_dir, args)
    stamp_file = os.path.join(mod_dir, FINGERPRINT)
    if args.if_changed and os.path.isfile(stamp_file) and open(stamp_file, encoding='utf-8').read() == stamp:
        print('Decks are up to date.')
        return
    game = Game(args.game)
    decks = read_folder(decks_dir, args.prefix)
    print(f'{len(decks)} deck image(s) in {decks_dir}')
    if not decks:
        remove_generated(mod_dir)
        return
    wood_bundle = game.bundle(game.find_bundle(f'/{WOOD}_ap_cas_main_bundlereftable'))
    wood = game.texture_image(wood_bundle, next(r['name'] for r in wood_bundle['manifest']['res'] if r['name'].endswith(f'/{WOOD}_c')))
    taken = existing_items(game, os.path.join(args.game, 'Mods'), args.mod)
    out = os.path.join(decks_dir, PREVIEWS)
    os.makedirs(out, exist_ok=True)
    import newdeck
    default_mask, default_normal = newdeck.default_layers(game)
    seen = {}
    ready = []
    for deck in decks:
        key = deck.item_id.lower()
        if key in seen:
            print(f'  WARNING {deck.display}: same name as {os.path.basename(seen[key])}; ReSkate would load only one, skipped')
            continue
        if key in taken:
            # Same item, same asset names: ReSkate would leave one of the two mods out whole.
            print(f'  WARNING {deck.display}: "{deck.display}" is already a deck in {taken[key]}; skipped (rename the PNG)')
            continue
        try:
            art = fit(Image.open(deck.path), wood, deck.notes)
        except Exception as error:
            print(f'  skipped {os.path.basename(deck.path)}: {error}')
            continue
        seen[key] = deck.path
        preview(art, OUTLINE).save(os.path.join(out, f'{deck.display}.png'))
        ready.append((deck, art, mixins.layers(deck.effects, art, default_mask, default_normal, deck.colour)))
        print(f'  {deck.display}' + (f'  ({"; ".join(deck.notes)})' if deck.notes else ''))
    print(f'{len(ready)} deck(s) ready; previews in {out}')
    if not ready:
        remove_generated(mod_dir)
        return
    stats, built = newdeck.build(game, [(d.display, d.item_id, art, layers) for d, art, layers in ready], mods_dir, args.mod)
    open(stamp_file, 'w', encoding='utf-8').write(stamp)
    print(f'Built {len(built)} deck(s) into {mod_dir} ({stats["cas"] // 1024} KB)')


FINGERPRINT = 'caveman-fingerprint.txt'


def fingerprint(decks_dir, args):
    """What a build depends on: every PNG under the deck folder (path, size, time), the options, and
    Caveman's own code. Equal fingerprints mean the mod on disk is already what a build would write."""
    import hashlib
    h = hashlib.sha256()
    h.update(repr((os.path.abspath(args.game), args.mod, args.prefix)).encode())
    for root, dirs, files in os.walk(decks_dir):
        dirs[:] = sorted(d for d in dirs if not d.startswith('.') and not (root == decks_dir and d.lower() == PREVIEWS))
        for name in sorted(files):
            if name.lower().endswith('.png'):
                st = os.stat(os.path.join(root, name))
                h.update(f'{os.path.relpath(os.path.join(root, name), decks_dir)}|{st.st_size}|{st.st_mtime_ns}\n'.encode())
    # Caveman's own code: this file and the toolkit modules a build loads (not every script in the
    # toolkit folder, so research tools added there do not force a rebuild at the next launch)
    import newdeck  # noqa: F401
    here = os.path.dirname(os.path.abspath(__file__))
    tools = os.path.join(here, 'tools', 're')
    used = sorted(m.__file__ for m in list(sys.modules.values())
                  if getattr(m, '__file__', None) and os.path.dirname(os.path.abspath(m.__file__)) == tools)
    for code in [os.path.join(here, 'caveman.py')] + used:
        h.update(open(code, 'rb').read())
    return h.hexdigest()


def remove_generated(mod_dir):
    """Removes the mod Caveman built, when there are no decks left to build; only a folder whose build
    stamp names Caveman as its tool."""
    import json
    import shutil
    try:
        tool = json.load(open(os.path.join(mod_dir, 'reskate-build.json'))).get('tool')
    except (OSError, ValueError):
        return
    if tool == 'Caveman':
        shutil.rmtree(mod_dir)
        print(f'No decks to build: removed {mod_dir}')


if __name__ == '__main__':
    main()
