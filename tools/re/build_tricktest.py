"""Builds the per-trick test decks into their own mod (prototype, 2026-10-21; knowledge/trick-triggers.md): board
effects fired by one specific trick, through a boolean state the mod carries (tricktest.py). Each deck's art names
its effect, its trigger and how the boolean is made.
Usage: build_tricktest.py <game folder> <mods folder> [--prefix "000 - "] [--mod Caveman-Trick-Test]"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import newdeck  # noqa: E402
import statefx  # noqa: E402
import tricktest  # noqa: E402
from build_finishes import art_image  # noqa: E402
from gamedata import Game  # noqa: E402

W, O = statefx.WHILE, statefx.ONCE
DECKS = [   # (name, board effect layers, trigger (key in tricktest.TRIGGERS, usage, cooldown s), art labels)
    ('Trick Kickflip Nyan', {'nyan': 8}, ('kickflip', W, 0), ['kickflip', 'nyan trail', 'kernel == 2', 'while the trick']),
    ('Trick Heelflip Aura', {'aura': (255, 0, 255)}, ('heelflip', W, 0), ['heelflip', 'magenta aura', 'kernel == 3', 'while the trick']),
    ('Trick Shuvit Nyan', {'nyan': 8}, ('shuvits', W, 0), ['pop shuvits', 'nyan trail', 'kernel in 4', 'while the trick']),
    ('Trick 50-50 Aura', {'aura': (0, 128, 255)}, ('fiftyfifty', W, 0), ['50-50 grinds', 'blue aura', 'kernel in 4', 'while grinding']),
    ('Trick Kickflip Item', {'aura': (0, 255, 0)}, ('kickflip_item', W, 0), ['kickflip', 'green aura', 'enum item == 2', 'while the trick']),
    ('Trick Any Flip Direct', {'aura': (255, 255, 0)}, ('anyflip', W, 0), ['any flip trick', 'yellow aura', 'enum as bool', 'while the trick']),
    ('Trick Kickflip Burst', {'burst': 'impact'}, ('kickflip', O, 0.5), ['kickflip', 'impact burst', 'kernel == 2', 'once per trick']),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('game')
    ap.add_argument('mods')
    ap.add_argument('--prefix', default='000 - ')
    ap.add_argument('--mod', default='Caveman-Trick-Test')
    args = ap.parse_args()
    tricktest.register()
    game = Game(args.game)
    decks = []
    for name, effect, trigger, labels in DECKS:
        display = f'{args.prefix}{name}'
        layers = dict(effect, trigger=trigger)
        decks.append((display, 'Own_' + display.replace(' ', '_').replace('-', '_'), art_image(labels), layers))
    stats, built = newdeck.build(game, decks, args.mods, args.mod)
    print(f'built {len(built)} decks into {os.path.join(args.mods, args.mod)} ({stats["cas"] // 1024} KB)')


if __name__ == '__main__':
    main()
