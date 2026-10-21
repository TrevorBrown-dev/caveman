"""Board effects fired by one specific trick (knowledge/trick-triggers.md, 2026-10-21): the current flip trick and
the current grind are enum game states, and an effect trigger (statefx.py: TriggerType 6) takes a boolean, so the
mod carries a boolean state of its own over the enum:

  kernel  a BooleanExpressionAsset "state in {values}" whose ExpressionKernel exprkernel.py writes, the way the
          game's own `ebool.*` states are built (Dem Bones' wipeout kernels are the template)
  item    a GameStateEnumerationItemAsset "state == value", the game's ready-made boolean over an enum state
  direct  the enum state itself as the trigger, to test whether the effect system takes a non-zero enum as true

plus byte copies of what they import: the enum state (from the root level) and its enum type (from the game
configuration), as the shared customization bundle carries every state Dem Bones' costume effects use.

Prototype wiring: register() adds the triggers to statefx.STATES and swaps statefx.States and newdeck.DeckPatch for
the subclasses below, so newdeck.build (unchanged) carries the authored assets and their dependencies."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ebx  # noqa: E402
import exprkernel  # noqa: E402
import newdeck  # noqa: E402
import statefx  # noqa: E402

SHARED_SOURCE = (newdeck.SHARED, 'items.toc')   # exprkernel.TEMPLATE lives there
PREFIX = 'animation/caveman/'
FLIP = 'animation/dingo/enumgs.anim.currentfliptrick'          # GameStateEnumerationAsset, root level
FLIP_ENUM = 'animation/dingo/onboardrightgesturetype'           # its AntEnumerationAsset, delmargameconfiguration
GRIND = 'animation/dingo/enum.gs.anim.currentgrindtype'         # root level
GRIND_ENUM = 'animation/dingo/enum.skategrindtype'              # delmargameconfiguration
# The flip tricks (OnBoardRightGestureType, the type of FLIP): value -> runtime name
FLIP_TRICKS = {
    0: 'None', 1: 'Ollie', 2: 'Kickflip', 3: 'Heelflip', 4: 'PopShuvit', 5: 'VarialKickflip', 6: 'InwardHeelflip',
    7: 'FSPopShuvit', 8: 'VarialHeelflip', 9: 'Hardflip', 10: '360PopShuvit', 11: '360Flip', 12: '360InwardHeelflip',
    13: 'FS360PopShuvit', 14: 'Laserflip', 15: '360Hardflip', 16: 'Nollie', 17: 'N_Kickflip', 18: 'N_Heelflip',
    19: 'N_PopShuvit', 20: 'N_Hardflip', 21: 'N_VarialHeelflip', 22: 'N_FSPopShuvit', 23: 'N_VarialKickflip',
    24: 'N_InwardHeelflip', 25: 'N_FS360PopShuvit', 26: 'N_360Hardflip', 27: 'N_Laserflip', 28: 'N_360PopShuvit',
    29: 'N_360Flip', 30: 'N_360InwardHeelflip', 31: 'OlliePop', 32: 'N_OlliePop', 33: 'Wallie', 34: 'FSWallie',
    35: 'BSWallie', 36: 'Impossible', 37: 'FrontFoot_Impossible', 38: 'N_Impossible', 39: 'N_BackFoot_Impossible',
    40: 'LateFlipTrick', 41: 'HeldFlipTrick', 42: 'BonedOllie', 43: 'KickflipUnderflip', 44: 'HeelflipUnderflip',
    45: 'BSPopShuvitUnderflip', 46: 'FSPopShuvitUnderflip', 47: 'NollieKickflipUnderflip', 48: 'NollieHeelflipUnderflip',
    49: 'NollieBSPopShuvitUnderflip', 50: 'NollieFSPopShuvitUnderflip', 51: 'HospitalFlip', 52: 'FlowerFlip',
    53: 'NollieHospitalFlip', 54: 'NollieFlowerFlip', 55: 'ImpossibleUnderflip', 56: 'FrontFootImpossibleUnderflip',
    57: 'NollieImpossibleUnderflip', 58: 'NollieBackFootImpossibleUnderflip',
}
# The grinds (Enum.SkateGrindType, the type of GRIND): 17 names in four stances (regular FS and BS, then BF_ (fakie?)
# FS and BS), then the Suskis and Darklips, and Exit.
_GRIND_NAMES = ['Lip', 'Board', '50_50', 'NoseSlide', 'TailSlide', 'NoseGrind', '5_0', 'Salad', 'Smith', 'Feeble', 'Willy',
                'OverWilly', 'NoseBlunt', 'Blunt', 'Crook', 'OverCrook', 'DarkSlide']
GRINDS = {0: 'Invalid'}
for _stance in ('FS_', 'BS_', 'BF_FS_', 'BF_BS_'):
    for _n in _GRIND_NAMES:
        GRINDS[len(GRINDS)] = f'{_stance}{_n}'
for _n in ('FS_Suski', 'BF_FS_Suski', 'BS_Suski', 'BF_BS_Suski', 'BS_Darklip', 'BF_BS_Darklip', 'FS_Darklip', 'BF_FS_Darklip', 'Exit'):
    GRINDS[len(GRINDS)] = _n
assert len(GRINDS) == 78 and GRINDS[20] == 'BS_50_50' and GRINDS[77] == 'Exit'


def flip(*names):
    return [v for v, n in FLIP_TRICKS.items() if n in names]


def grind(*names):
    return [v for v, n in GRINDS.items() if n in names]


# trigger key -> (kind, enum state, its enum type, value(s)); the key names the state in statefx.STATES
TRIGGERS = {
    'kickflip': ('kernel', FLIP, FLIP_ENUM, flip('Kickflip')),                    # == 2: the template's own shape
    'heelflip': ('kernel', FLIP, FLIP_ENUM, flip('Heelflip')),                    # == 3: a constant the game never compiled
    'shuvits': ('kernel', FLIP, FLIP_ENUM, flip('PopShuvit', 'FSPopShuvit', '360PopShuvit', 'FS360PopShuvit')),
    'fiftyfifty': ('kernel', GRIND, GRIND_ENUM, grind('FS_50_50', 'BS_50_50', 'BF_FS_50_50', 'BF_BS_50_50')),
    'kickflip_item': ('item', FLIP, FLIP_ENUM, 2),
    'anyflip': ('direct', FLIP, FLIP_ENUM, None),
}


def display_name(key, kind):
    """The asset's Name as the game cases its states ('Animation/Caveman/EBool.Trick.Kickflip'); the manifest name
    is its lower case."""
    leaf = key.replace('_', '.').title()
    return f'Animation/Caveman/EBool.Trick.{leaf}' if kind == 'kernel' else f'Animation/Caveman/Enum.GSItem.Trick.{leaf}'


def asset_name(key):
    kind, state, _, _ = TRIGGERS[key]
    return state if kind == 'direct' else display_name(key, kind).lower()


AUTHORED = {asset_name(k): (k,) + TRIGGERS[k] for k in TRIGGERS if TRIGGERS[k][0] != 'direct'}
# what each carried state needs carried with it (the kernel or item imports the state, the state its enum type)
DEPS = {name: [spec[2], spec[3]] for name, spec in AUTHORED.items()}
DEPS.update({FLIP: [FLIP_ENUM], GRIND: [GRIND_ENUM]})
_current = None


class TrickStates(statefx.States):
    """statefx.States plus the authored states, built on first use from the game's templates."""

    def __init__(self, game):
        super().__init__(game)
        global _current
        _current = self

    def ebx(self, name):
        if name in AUTHORED and name not in self._docs:
            key, kind, state, _, values = AUTHORED[name]
            if kind == 'kernel':
                doc = exprkernel.boolean_expression(super().ebx(exprkernel.TEMPLATE)[0], display_name(key, kind), self.ref(state), values)
            else:
                doc = exprkernel.enum_item(super().ebx(exprkernel.TEMPLATE_ITEM)[0], display_name(key, kind), self.ref(state), values)
            data = ebx.write_document(doc, preserve_layout=False)
            self._docs[name] = (data, ebx.read_document(data))
        return super().ebx(name)


class TrickPatch(newdeck.DeckPatch):
    """newdeck.DeckPatch that carries a state's dependencies ahead of it, each state once."""

    def add_ebx(self, bundle, name, data):
        if name.startswith('animation/'):
            if any(e['name'] == name for e in self.bundle(bundle)['manifest']['ebx']):
                return
            for dep in DEPS.get(name, []):
                self.add_ebx(bundle, dep, _current.ebx(dep)[0])
        super().add_ebx(bundle, name, data)


def register():
    """Makes newdeck.build accept the trigger keys in TRIGGERS (call before building or verifying)."""
    if SHARED_SOURCE not in statefx.SOURCES:
        statefx.SOURCES.append(SHARED_SOURCE)
    for key in TRIGGERS:
        statefx.STATES[key] = asset_name(key)
    statefx.States = TrickStates
    newdeck.DeckPatch = TrickPatch
