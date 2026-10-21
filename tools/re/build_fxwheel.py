"""Prototype (2026-10-21; an investigation build under the frozen-toolkit exception, knowledge/toolkit.md): board effects
fired by a game state, carried on WHEEL items instead of decks. A deck's SkaterVfx sets are built on the skateboard's
effects record, whose animation instance has none of the skater's states (the live probe,
knowledge/state-effects-runtime.md); ReSkate's native_vfx.cpp says the effects a skater's item puts on the wheels are
built on the skater (Kichi Aura Wheels: a wheel item, an always-on board-attached asset). Each test wheel carries an
aura (vfx.py) on the board, always on or fired by a state the shared bundle can reach (statefx.py, statecheck.py).
Usage: build_fxwheel.py <game folder> <mods folder> [--prefix "000 - "] [--mod Caveman-FX-Wheel-Test]"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from PIL import Image, ImageDraw, ImageFont  # noqa: E402

import ebx  # noqa: E402
import newwheel  # noqa: E402
import statefx  # noqa: E402
import vfx  # noqa: E402
from gamedata import Game  # noqa: E402
from newdeck import BRT, MASTER, SHARED, DeckPatch, _clone, _ebx, _ref, _res, _write  # noqa: E402
import brt  # noqa: E402

W, O = statefx.WHILE, statefx.ONCE
WHEELS = [   # (name, wheel and aura colour, trigger (state, usage, cooldown s, cooldown state) or None: always on)
    ('FX Wheel 1 Always', (255, 255, 255), None),
    ('FX Wheel 2 Riding', (0, 255, 255), ('riding', W, 0, None)),
    ('FX Wheel 3 Pushing', (255, 140, 0), ('push', W, 0, None)),
    ('FX Wheel 4 In Air', (255, 0, 255), ('airloop', W, 0, None)),
    ('FX Wheel 5 Hard Bail', (255, 40, 40), ('hardbail', O, 2.0, 'wipeout')),
]


def face(number, colour):
    """A wheel face: the colour with a big number, so each test wheel is told apart in the menu."""
    img = Image.new('RGBA', (512, 512), colour + (255,))
    draw = ImageDraw.Draw(img)
    try:
        font = ImageFont.load_default(size=300)
    except TypeError:   # older Pillow
        font = ImageFont.load_default()
    ink = (0, 0, 0, 255) if sum(colour) > 380 else (255, 255, 255, 255)
    draw.text((256, 256), str(number), fill=ink, font=font, anchor='mm')
    return img


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('game')
    ap.add_argument('mods')
    ap.add_argument('--prefix', default='000 - ')
    ap.add_argument('--mod', default='Caveman-FX-Wheel-Test')
    args = ap.parse_args()
    game = Game(args.game)
    patch = DeckPatch(game)
    t = newwheel.Templates(game)
    shared = game.bundle(SHARED)
    templates = (_ebx(game, shared, vfx.BLUEPRINT), _ebx(game, shared, vfx.ASSET))
    states = statefx.States(game)
    master = ebx.read_document(_ebx(game, shared, MASTER))
    items_field = master.instances[0].object.find('Items')

    # add_wheel writes the item itself: catch it on its way into the patch and point its SkaterVfx at the effect
    pending = {}
    add_ebx = patch.add_ebx

    def add_with_effect(bundle, name, data):
        if bundle == SHARED and name in pending:
            item = ebx.read_document(data)
            di = next(i for i, r in enumerate(item.instances) if item.type_name(r.descriptor) == 'DelMarCustomizationItemData')
            vfx.attach(item, di, pending.pop(name))
            data = _write(item)
        return add_ebx(bundle, name, data)
    patch.add_ebx = add_with_effect

    presets, built = [], []
    for n, (name, colour, trigger) in enumerate(WHEELS, 1):
        display = f'{args.prefix}{name}'
        key = 'Own_' + display.replace(' ', '_')
        base = key[len('Own_'):].lower()
        # the effect: an aura on the board (as a deck's), always on or fired by a state
        bp_name, va_name = f'{vfx.PREFIX}ebp_{base}_aura', f'{vfx.PREFIX}{base}_aura'
        bp = _clone(templates[0], bp_name)
        vfx.blueprint(bp, colour)
        add_ebx(SHARED, bp_name, _write(bp))
        va = _clone(templates[1], va_name)
        vfx.asset(va, _ref(bp), vfx.POINTS)
        if trigger:
            state, usage, cooldown, cooldown_state = trigger
            cooldown_ref = states.ref(statefx.COOLDOWN_STATES[cooldown_state]) if cooldown_state else None
            statefx.trigger(va, states.ref(statefx.STATES[state]), usage, cooldown, cooldown_ref)
        add_ebx(SHARED, va_name, _write(va))
        pending[f'{newwheel.ITEMS}own_{base}'] = _ref(va)
        preset, item_ref, preview = newwheel.add_wheel(patch, t, key, face(n, colour), colour)
        assert not pending, f'{display}: the wheel item was not caught'
        presets.append(preset)
        master.imports.append(item_ref)
        items_field.value.data.append(ebx.Value(ebx.Pointer('external', len(master.imports) - 1)))
        built.append((display, base, preview))
        print(f'  built {display} ({key}): ' + (f'{trigger[0]} usage {trigger[1]}' if trigger else 'always on'))
    patch.replace_ebx(SHARED, MASTER, _write(master))
    data, meta = _res(game, shared, BRT)
    resource, new_meta = brt.add_presets(brt.Table(data, meta['meta']), [(p.lower(), h) for p, h in presets])
    patch.replace_res(SHARED, BRT, resource, new_meta)
    names = ', '.join(d for d, _, _ in built)
    lines = '\n'.join(f'- {d}' for d, _, _ in built)
    stats = patch.write(os.path.join(args.mods, args.mod), args.mod, f'Test wheels with board effects: {names}.',
                        icon=built[0][2].resize((256, 256)), readme=f'# {args.mod}\n\nTest wheels with board effects:\n\n{lines}\n')
    print(f'built {len(built)} wheels into {os.path.join(args.mods, args.mod)} ({stats["cas"] // 1024} KB)')


if __name__ == '__main__':
    main()
