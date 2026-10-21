"""Board effects that follow the skater's state (knowledge/state-effects.md): a board-effect asset (vfx.py) whose effect
item fires on an animation-system game state instead of always (TriggerType 6), as the game's Dem Bones costume does for
hard wipeouts. Prototype (2026-10-20) for in-game testing before the Rust engine gets it.

  TriggerType 6, TriggerEffectsGameState.AssetRef -> the state asset (file and instance GUIDs equal),
  TriggerOutputUsage 0 (the effect lives while the state holds) or 1 (fires once each time), CooldownTime seconds.

The state assets live in the root level (levels/game/dingolevel_root); like the game does for its costume states, the
mod also carries a byte copy of each state's EBX in the shared customization bundle (copied from the player's game)."""
import ebx

SOURCES = [   # (bundle, superbundle TOC) holding the state assets
    ('win32/levels/game/dingolevel_root/dingolevel_root', 'levels/game/dingolevel_root/dingolevel_root.toc'),
    ('win32/configurations/gameconfigurations/delmargameconfiguration', 'globals.toc'),
    # the shared customization bundle: Dem Bones' own states (animation/ebool.wipeout.force.ishard, ...)
    ('win32/characters/customization/configs/cas_main_sharedbundle', 'items.toc'),
]
STATES = {   # name: the state asset (BoolAsset) under animation/dingo/
    'grind': 'animation/dingo/bool.state.is.grind',
    'grindinto': 'animation/dingo/bool.state.is.grindinto',
    'manual': 'animation/dingo/bool.state.is.manual',
    'landing': 'animation/dingo/bool.state.is.landing',
    'tricklanding': 'animation/dingo/bool.anim.is.tricklanding',
    'air': 'animation/dingo/bool.state.is.trickinair',
    'trick': 'animation/dingo/bool.state.is.trick',
    'powerslide': 'animation/dingo/bool.state.is.powersliding',
    'grab': 'animation/dingo/bool.anim.isgrabbing',
    # second test (2026-10-21): states true most of the time, a computed state, and Dem Bones' own trigger as a control
    'onboard': 'animation/dingo/bool.anim.onboard',
    'pushing': 'animation/dingo/bool.anim.ispushing',
    'notrick': 'animation/dingo/ebool.anim.isnotdoingtrick',
    'hardbail': 'animation/ebool.wipeout.force.ishard',
    # third test (2026-10-21): only states a shared-bundle item can reach (the game configuration or the shared
    # bundle itself, never the root level: knowledge/state-effects-runtime.md), checked with statecheck.py
    'getup': 'animation/dingo/bool.state.is.wipeout.getup',
    'riding': 'animation/dingo/bool.state.onboard.riding',
    'push': 'animation/dingo/bool.state.is.push',
    'airloop': 'animation/dingo/bool.state.inair.loop',
    'landed': 'animation/dingo/bool.state.is.landing',
    'doingtrick': 'animation/dingo/bool.anim.doingtrick',
}
COOLDOWN_STATES = {   # integer states for CooldownType 1
    'wipeout': 'animation/int.wipeout.state.cooldown',
}
GAME_STATE = 6
WHILE, ONCE = 0, 1   # TriggerOutputUsage (inferred from the game's use)
BURSTS = {   # stock one-shot effects in the shared bundle
    'impact': 'effects/characters/effectblueprints/ebp_chr_ctd_impact_a',
    'dust': 'effects/characters/effectblueprints/ebp_chr_impactdust_a',
}


class States:
    """The game's state assets (their EBX bytes from the root level), looked up once."""

    def __init__(self, game):
        self.game = game
        self._bundles = None
        self._docs = {}

    def ebx(self, name):
        """(EBX bytes, document) of a state, from the root level or the global configuration bundle."""
        if name not in self._docs:
            if self._bundles is None:
                self._bundles = [self.game.bundle(b, toc=t) for b, t in SOURCES]
            for b in self._bundles:
                m = b['manifest']
                i = next((i for i, e in enumerate(m['ebx']) if e['name'] == name), None)
                if i is not None:
                    data = self.game.decode(b['files'][1 + i])
                    self._docs[name] = (data, ebx.read_document(data))
                    break
            else:
                raise KeyError(f'game state {name} not found in {[b for b, _ in SOURCES]}')
        return self._docs[name]

    def ref(self, name):
        doc = self.ebx(name)[1]
        return doc.file_guid, doc.instances[0].instance_guid


def trigger(doc, state_ref, usage, cooldown=0.0, cooldown_state_ref=None):
    """Makes the effect item of a board-effect asset clone (vfx.asset) fire on a game state. With cooldown_state_ref
    (an integer state, as Dem Bones' int.wipeout.state.cooldown): CooldownType 1 on that state. CooldownType 1 without
    a cooldown state crashes the game (an unchecked view, knowledge/state-effects-runtime.md); a one-off trigger with
    no cooldown at all fires every frame."""
    _, rec = doc.instances_of('SkaterVfxEffectItem')[0]
    o = rec.object
    doc.imports.append(state_ref)
    o.find('TriggerType').value.data = GAME_STATE
    o.find('TriggerOutputUsage').value.data = usage
    o.find('CooldownTime').value.data = float(cooldown)
    o.find('TriggerEffectsGameState').value.data.find('AssetRef').value.data = ebx.Pointer('external', len(doc.imports) - 1)
    if cooldown_state_ref is not None:
        doc.imports.append(cooldown_state_ref)
        o.find('CooldownType').value.data = 1
        o.find('CooldownGameState').value.data.find('AssetRef').value.data = ebx.Pointer('external', len(doc.imports) - 1)
