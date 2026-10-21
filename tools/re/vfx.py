"""Item VFX for decks ("aura"): a light and ribbon trails attached to the board, the way Kichi Aura Wheels
lights its wheels (knowledge/glow.md). Both assets are cloned from the game's own oni costume effect:

  effects/characters/effectblueprints/ebp_chr_oni_sword_light   a light (PbrSphereLightEntityData) and two ribbon
                                                                trails that spawn by distance travelled
  characters/vfx/skfx_cst_oni2026vfx                            an always-on SkaterVfxEffectsAsset with two
                                                                attachments (re-aimed at the board, one per truck)

The deck item's DelMarCustomizationItemData points its `SkaterVfx` at the new asset (`PreviewVFX` True)."""
import os
import struct

import ebx

BLUEPRINT = 'effects/characters/effectblueprints/ebp_chr_oni_sword_light'
ASSET = 'characters/vfx/skfx_cst_oni2026vfx'
PREFIX = 'characters/vfx/caveman/'
TRAIL_GRAPH = ebx.Guid('b48e76d9-4a88-46c6-a49f-26b2b1ae3d70')    # effects/_global/emittergraphs/eg_ribbon_trail_distance_emit_a
HELPER_GRAPH = ebx.Guid('bf1101c3-b8cc-11ef-b925-bbfa60fddb5c')   # effects/prototype/testassets/.../eg_hlp_static_axis_loop: dropped
TRAIL_COLOUR, TRAIL_COLOUR2 = -1752990270, -1752990271            # emitter PropertyIds (0x9783DFC2, ...C1): HDR rgb
TRAIL_BOOST, TRAIL_DIM = 1.5, 0.25                                # main and second trail colour, times the aura colour
LIGHT_INTENSITY, LIGHT_RADIUS = 40.0, 0.9                         # the oni sword: 150 and 1 m; Kichi's wheels: 13 and 0.24 m
# Attach points in board space (x across, y up, z along the board, metres; from Kichi's wheel offsets): the trucks.
POINTS = [(0.0, 0.03, 0.205), (0.0, 0.03, -0.205)]
BOARD = 1                                                         # AttachType: 1 the board (0 a skater bone)
NO_BONE = 0xFFFFFFFF
# "nyan": one solid, hard-edged rainbow trail from the middle of the board, drawn by Caveman's patched ribbon program
# (nyanpatch.py: a ribbon whose colour is (-1, S, B) draws six bands, stepped every S along its length, brightness B;
# knowledge/trails.md). No light, no glow sprite: nothing soft.
NYAN_POINTS = [(0.0, 0.07, 0.0)]   # 7 cm up: at 3 cm the wide band dipped into the ground (Trevor, 2026-10-20)
NYAN_WIDTH = 0.28                                                 # PropertyId 226981187 x: ribbon width, metres (oni: 0.1)
NYAN_LIFE = 1.2                                                   # ParticleLifeSpan, seconds (oni: 0.1 and 0.15)
NYAN_STEPS = 8
WIDTH = 226981187


def linear(rgb):
    """0-255 sRGB to 0-1 linear (light and trail colours are linear)."""
    out = []
    for c in rgb:
        c = c / 255
        out.append(c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4)
    return out


def _set_vec(obj, field, values):
    vec = obj.find(field).value.data
    for name, v in zip('xyzw', values):
        vec.find(name).value.data = float(v)


def blueprint(doc, colour):
    """Edits a clone of BLUEPRINT in place: light and trails in `colour` (0-255 sRGB), trails at the attach point,
    the helper emitter dropped."""
    c = linear(colour)
    effect = doc.instances[doc.instances_of('EffectEntityData')[0][0]].object
    components = effect.find('Components').value.data
    keep = []
    for v in components:
        rec = doc.instances[v.data.index]
        if doc.type_name(rec.descriptor) == 'EmitterGraphEntityData':
            graph = rec.object.find('EmitterGraph').value.data
            graph_file = doc.imports[graph.index][0] if graph.kind == 'external' else None
            if graph_file == HELPER_GRAPH:
                continue
            if graph_file == TRAIL_GRAPH:
                _set_vec(rec.object.find('Transform').value.data, 'trans', (0, 0, 0))
                for p in rec.object.find('EmitterGraphParams').value.data:
                    pid = p.data.find('PropertyId').value.data
                    if pid == TRAIL_COLOUR:
                        _set_vec(p.data, 'Value', [x * TRAIL_BOOST for x in c] + [0])
                    elif pid == TRAIL_COLOUR2:
                        _set_vec(p.data, 'Value', [x * TRAIL_DIM for x in c] + [0])
        keep.append(v)
    components[:] = keep
    light = doc.instances[doc.instances_of('PbrSphereLightEntityData')[0][0]].object
    _set_vec(light, 'Color', c)
    light.find('Intensity').value.data = LIGHT_INTENSITY
    light.find('AttenuationRadius').value.data = LIGHT_RADIUS


def nyan_colour(steps, trigger='ratio'):
    """The ribbon colour that switches on nyanpatch's rainbow: 'ratio' (red = RATIO x green; the default), 'negative'
    (red -1: showed no trail at all in game, 2026-10-20), or 'control' (plain orange: no rainbow, for diagnosis)."""
    import nyanpatch
    if trigger == 'negative':
        return (-1, steps, 1, 0)
    if trigger == 'control':
        return (1, 0.4, 0, 0)
    return (nyanpatch.RATIO, 1, steps / nyanpatch.RATIO_STEPS, 0)


def nyan_blueprint(doc, steps=NYAN_STEPS, trigger='ratio'):
    """Edits a clone of BLUEPRINT in place: only the first ribbon trail, in nyan mode (nyan_colour), wide and
    long-lived; the light, the other emitters and the helper dropped."""
    effect = doc.instances[doc.instances_of('EffectEntityData')[0][0]].object
    components = effect.find('Components').value.data
    keep = []
    for v in components:
        rec = doc.instances[v.data.index]
        if doc.type_name(rec.descriptor) != 'EmitterGraphEntityData' or keep:
            continue
        graph = rec.object.find('EmitterGraph').value.data
        if graph.kind != 'external' or doc.imports[graph.index][0] != TRAIL_GRAPH:
            continue
        o = rec.object
        _set_vec(o.find('Transform').value.data, 'trans', (0, 0, 0))
        for p in o.find('EmitterGraphParams').value.data:
            pid = p.data.find('PropertyId').value.data
            if pid in (TRAIL_COLOUR, TRAIL_COLOUR2):
                _set_vec(p.data, 'Value', nyan_colour(steps, trigger))
            elif pid == WIDTH:
                p.data.find('Value').value.data.find('x').value.data = NYAN_WIDTH
        life = o.find('EmitterGraphOverrides').value.data.find('ParticleLifeSpan').value.data
        for q in ('Low', 'Medium', 'High', 'Ultra'):
            life.find(q).value.data = NYAN_LIFE
        keep.append(v)
    assert keep, f'no ribbon trail in {BLUEPRINT}'
    components[:] = keep


def asset(doc, blueprint_ref, points=POINTS):
    """Edits a clone of ASSET in place: attachments on the board at `points`, each playing `blueprint_ref`
    ((file, instance) GUIDs). The template has two; with fewer points the rest are left out of the effect."""
    items = doc.instances_of('SkaterVfxTriggerableItem')
    assert len(items) >= len(points), f'{ASSET} has {len(items)} attachments, {len(points)} wanted'
    effect_item = doc.instances_of('SkaterVfxEffectItem')[0][1].object.find('Effects').value.data
    used = [rec for _, rec in items[:len(points)]]
    effect_item[:] = [v for v in effect_item if doc.instances[v.data.index] in used]
    for rec, point in zip(used, points):
        o = rec.object
        doc.imports[o.find('Effect').value.data.index] = blueprint_ref
        _set_vec(o, 'Offset', point)
        _set_vec(o, 'Rotation', (0, 0, 0))
        o.find('AttachType').value.data = BOARD
        o.find('OffsetType').value.data = 1
        o.find('TargetBoneHash').value.data = NO_BONE
        o.find('MirroredTargetBoneHash').value.data = NO_BONE
    for _, rec in doc.instances_of('SkaterVfxEffectItem'):
        # an identity of the effect item (its function is unknown; the oni costume's own must not be shared)
        rec.object.find('InstanceGuidHash').value.data = struct.unpack('<q', os.urandom(8))[0]


def attach(item, item_data_index, asset_ref):
    """Points a deck item's SkaterVfx at `asset_ref` ((file, instance) GUIDs) and shows it in the menu preview."""
    item.imports.append(asset_ref)
    data = item.instances[item_data_index].object
    data.find('SkaterVfx').value.data = ebx.Pointer('external', len(item.imports) - 1)
    data.find('PreviewVFX').value.data = True
