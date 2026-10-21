"""Can a game state be a board effect's trigger (TriggerType 6) from the shared customization bundle? Checked offline,
from what the game's code does at run time (knowledge/state-effects-runtime.md):

  - the state must live in a bundle the shared bundle can import from: the game configuration (globals.toc) or the
    shared bundle itself; an import into the root level (or any other bundle) stays null and the trigger update
    dereferences the null view (Skate.exe+0x4876004);
  - the state's class must answer the bool view: BoolAsset, BooleanExpressionAsset, GameStateEnumerationItemAsset
    (an enum, integer or float state gives a null view: the same crash);
  - the state's own imports (a kernel's inputs, an enumeration item's enum state) must obey the same bundle rule;
  - a byte copy of a root-level state carried in the mod resolves but never fires: a state's value lives in a slot bound
    to that asset object, and nothing writes a copy's slot.

Usage: statecheck.py <game root> <state name or file GUID> ...     (names as in the manifest, e.g.
       animation/dingo/bool.state.onboard.riding)"""
import os
import sys
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ebx  # noqa: E402
from gamedata import Game  # noqa: E402

ROOT = ('win32/levels/game/dingolevel_root/dingolevel_root', 'levels/game/dingolevel_root/dingolevel_root.toc')
CONFIG = ('win32/configurations/gameconfigurations/delmargameconfiguration', 'globals.toc')
SHARED = ('win32/characters/customization/configs/cas_main_sharedbundle', 'items.toc')
LEGAL = {CONFIG[0], SHARED[0]}
BOOL_VIEW = {'BoolAsset', 'BooleanExpressionAsset', 'GameStateEnumerationItemAsset', 'DiceRollBoolAsset'}


class Index:
    """Name and file GUID of every EBX in the three bundles that matter."""

    def __init__(self, game):
        self.game = game
        self.by_name, self.by_guid = {}, {}
        for bundle, toc in (CONFIG, SHARED, ROOT):
            b = game.bundle(bundle, toc=toc)
            for i, e in enumerate(b['manifest']['ebx']):
                data = game.decode(b['files'][1 + i])
                guid = str(ebx.read_file_guid(data))
                entry = (bundle, e['name'], guid, b['files'][1 + i])
                self.by_name.setdefault(e['name'], entry)
                self.by_guid.setdefault(guid, entry)

    def find(self, key):
        try:
            return self.by_guid.get(str(uuid.UUID(key)))
        except ValueError:
            return self.by_name.get(key.lower())


def check(index, key, depth=0):
    """depth 0: the trigger itself (bundle and bool-view rules); deeper: its inputs (the bundle rule only)."""
    entry = index.find(key)
    pad = '    ' * depth
    if entry is None:
        print(f'{pad}{key}: NOT FOUND in the game configuration, the shared bundle or the root level')
        return False
    bundle, name, guid, f = entry
    doc = ebx.read_document(index.game.decode(f))
    t = doc.type_name(doc.instances[0].descriptor)
    short = bundle.split('/')[-1]
    ok = True
    problems = []
    if bundle not in LEGAL:
        problems.append(f'lives in {short}: an import from the shared bundle stays null (crash at the first update)')
    if depth == 0 and t not in BOOL_VIEW:
        problems.append(f'{t} has no bool view: the trigger view is null (crash at the first update)')
    verdict = 'OK' if not problems else 'NO'
    print(f'{pad}{name} ({short}, {t}, {guid}): {verdict}' + (' -- ' + '; '.join(problems) if problems else ''))
    ok = not problems
    for fg, ig in doc.imports:
        sub = index.find(str(fg))
        if sub is None:
            print(f'{pad}    import {fg}: not in the three bundles (fine if it is not a state)')
            continue
        sb, sn, sg, _ = sub
        if sn.startswith('animation/'):
            ok = check(index, sn, depth + 1) and ok
    return ok


def main():
    game = Game(sys.argv[1])
    index = Index(game)
    bad = 0
    for key in sys.argv[2:]:
        bad += 0 if check(index, key) else 1
    print(f'\n{len(sys.argv) - 2 - bad} usable, {bad} not')


if __name__ == '__main__':
    main()
