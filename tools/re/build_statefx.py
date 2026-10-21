"""Builds the state-effect test decks into their own mod (prototype, 2026-10-20; knowledge/state-effects.md): board
effects fired by the skater's state instead of always on. Each deck's art names its effect and trigger.
Usage: build_statefx.py <game folder> <mods folder> [--test 1|2|3] [--prefix "000 - "] [--mod NAME]"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import newdeck  # noqa: E402
import statefx  # noqa: E402
from build_finishes import art_image  # noqa: E402
from gamedata import Game  # noqa: E402

W, O = statefx.WHILE, statefx.ONCE
# Second test (2026-10-21): the first one (copies of the root-level states in the shared bundle) never fired. These
# reference the game's states alone, as Dem Bones does for bool.state.is.wipeout.getup (in the game configuration), use
# states that are true most of the time, and one control: Dem Bones' own trigger. Every deck an aura of its own colour.
DECKS2 = [   # (name, board effect layers, trigger (state, usage, cooldown s, copy), art labels)
    ('State Test 1 Hard Bail', {'aura': (255, 40, 40)}, ('hardbail', O, 1.0, False), ['test 1', 'red aura', 'after a', 'hard bail']),
    ('State Test 2 On Board', {'aura': (0, 255, 255)}, ('onboard', W, 0, False), ['test 2', 'cyan aura', 'while', 'on board']),
    ('State Test 3 No Trick', {'aura': (40, 255, 40)}, ('notrick', W, 0, False), ['test 3', 'green aura', 'off during', 'tricks']),
    ('State Test 4 Pushing', {'aura': (255, 140, 0)}, ('pushing', W, 0, False), ['test 4', 'orange aura', 'while', 'pushing']),
    ('State Test 5 Grind', {'aura': (255, 0, 255)}, ('grind', W, 0, False), ['test 5', 'pink aura', 'while', 'grinding']),
    ('State Test 6 In Air', {'aura': (255, 255, 255)}, ('air', W, 0, False), ['test 6', 'white aura', 'while', 'in the air']),
]
# Third test (2026-10-21): test 2 also failed (root-level states cannot be imported from the shared bundle: a null view,
# the menu crash; knowledge/state-effects-runtime.md). Only states of the game configuration or the shared bundle,
# checked with statecheck.py; the control is Dem Bones' trigger exactly (its cooldown state too).
DECKS3 = [   # (name, board effect layers, trigger (state, usage, cooldown s, copy[, cooldown state]), art labels)
    ('Trigger 1 Hard Bail', {'aura': (255, 40, 40)}, ('hardbail', O, 2.0, False, 'wipeout'), ['trigger 1', 'red aura', 'after a', 'hard bail']),
    ('Trigger 2 Getup', {'aura': (255, 230, 0)}, ('getup', W, 0, False), ['trigger 2', 'yellow aura', 'while', 'getting up']),
    ('Trigger 3 Riding', {'aura': (0, 255, 255)}, ('riding', W, 0, False), ['trigger 3', 'cyan aura', 'while', 'riding']),
    ('Trigger 4 Pushing', {'aura': (255, 140, 0)}, ('push', W, 0, False), ['trigger 4', 'orange aura', 'while', 'pushing']),
    ('Trigger 5 In Air', {'aura': (255, 255, 255)}, ('airloop', W, 0, False), ['trigger 5', 'white aura', 'while', 'in the air']),
    ('Trigger 6 Landing', {'burst': 'impact'}, ('landed', O, 0.3, False), ['trigger 6', 'impact burst', 'once per', 'landing']),
    ('Trigger 7 Trick', {'aura': (40, 255, 40)}, ('doingtrick', W, 0, False), ['trigger 7', 'green aura', 'while', 'doing a trick']),
]
DECKS = [   # the first test (copies): (name, board effect layers, trigger (state, usage, cooldown s), art labels)
    ('State Grind Nyan', {'nyan': 8}, ('grind', W, 0), ['grind', 'nyan trail', 'while', 'grinding']),
    ('State Manual Aura', {'aura': (0, 255, 255)}, ('manual', W, 0), ['manual', 'cyan aura', 'while', 'manualing']),
    ('State Air Nyan', {'nyan': 8}, ('air', W, 0), ['air', 'nyan trail', 'while', 'in the air']),
    ('State Powerslide Aura', {'aura': (255, 140, 0)}, ('powerslide', W, 0), ['slide', 'orange aura', 'while', 'powersliding']),
    ('State Land Burst', {'burst': 'impact'}, ('tricklanding', O, 0.3), ['land', 'impact burst', 'once', 'per landing']),
    ('State Grind Burst', {'burst': 'impact'}, ('grindinto', O, 0.5), ['grind on', 'impact burst', 'once', 'per grind']),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('game')
    ap.add_argument('mods')
    ap.add_argument('--prefix', default='000 - ')
    ap.add_argument('--test', type=int, choices=(1, 2, 3), default=3)
    ap.add_argument('--mod', default=None, help='default Caveman-State-Test (test 1) or Caveman-State-Test-<n>')
    args = ap.parse_args()
    args.mod = args.mod or ('Caveman-State-Test' if args.test == 1 else f'Caveman-State-Test-{args.test}')
    game = Game(args.game)
    decks = []
    for name, effect, trigger, labels in {1: DECKS, 2: DECKS2, 3: DECKS3}[args.test]:
        display = f'{args.prefix}{name}'
        layers = dict(effect, trigger=trigger)
        decks.append((display, 'Own_' + display.replace(' ', '_'), art_image(labels), layers))
    stats, built = newdeck.build(game, decks, args.mods, args.mod)
    print(f'built {len(built)} decks into {os.path.join(args.mods, args.mod)} ({stats["cas"] // 1024} KB)')


if __name__ == '__main__':
    main()
