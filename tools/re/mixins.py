"""Deck mixins: cheap effects made from the deck's own texture slots and the game's own shader, no new
shader. A PNG's suffix picks them: `Cool Deck.foil.png`, `Cool Deck.holo-frost.spot.png`.

Learned from the stock decks (2026-10-20; 250 of them ship their own mask): the mask `param_graphic_osk`
is R occlusion (white but for the rim and truck shadows), G smoothness (gloss; stock decks about 211),
B metal (0 printed, 255 metal). A metal area takes the art's colour as its tint, so yellow art with
B = 255 is the game's gold foil decks (deck_gen_graphic_00041), and the plunger on a gold field
(jenkem 00003) is the field's mask 255 and the plunger's 0. Glitter (deathwish 00013) is speckled metal
and gloss. The normal map `param_graphic_ny` is flat (127, 127, 255) unless the print is raised (the
mosaic deck 00088). Both are 512 x 2048 BC7 (format 66, linear), like the art.

Holo foil is the deck shader's own iridescence (the stock Primitive, Carpet Co, Deathwish, There and Full
Circle holo decks; the community's method, as in the DGK Foil Vault mod): the deck's preset adds
`param_graphic_rgb`, a texture whose red channel is 255 where the deck is holo, and `param_foilmode`,
one of five patterns. Such decks live in a stock holo deck's bundle (newdeck.HOSTS['holo']) and use a
solid glossy mask.

The shader only uses the mask's metal channel when the deck's preset sets `param_graphic_metalmaskvalue`
(1.0 on the gold and plunger decks); decks with their own mask are built on the gold deck's preset for
that (newdeck.HOSTS['metal']).

Shader effects (Caveman's patched deck programs, shaderpatch.py): `cycle` (the colours cycle through the
hues) and `glow` (the print lights up). `aura` is not a shader effect: a light and trails attached to the
board (vfx.py), in a colour given as a hex suffix (`Cool Deck.aura.00ffff.png`) or taken from the art. The deck programs read the holo map (t0, only its red channel otherwise), so these
decks carry Caveman's marker in their holo map's top-left corner, off the visible deck.

One finish per deck. `spot` puts the finish only on the light parts of the art (dark ink stays plain
print); `emboss` raises the light parts. Both go with any finish, and so do the effects."""
import numpy as np
from PIL import Image, ImageFilter

import shaderpatch

STOCK_GLOSS = 211            # the default mask's mean smoothness
FINISHES = {
    # name: (smoothness, metal) of the even finishes; None for the ones made otherwise
    'gloss': (245, 0),
    'matte': (70, 0),
    'foil': (215, 255),
    'chrome': (250, 255),
    'glitter': None,         # speckled metal flakes that catch the light at different angles
}
HOLO = {                     # holo finish: the deck shader's foil mode (enumerator names in foilmode)
    'holo': 'Pearl',         # the stock Primitive holo deck's mode
    'holo-cellular': 'Cellular',
    'holo-frost': 'Frost',
    'holo-vertical': 'Vertical',
    'holo-glitter': 'Glitter',
}
MODIFIERS = ('spot', 'emboss', 'aura', 'nyan')   # aura: a light and trails on the board (vfx.py), in the deck's
# colour; nyan: a solid, stepped rainbow trail (vfx.py + nyanpatch.py). One board effect per deck: nyan wins.
FX = ('cycle', 'glow')       # shader effects (shaderpatch.py)
FX_SPEED = 32                # cycle speed (holo map G, linear): 32 / 255 x 2 = a quarter turn a second
FX_GLOW = 128                # glow strength (holo map B)
EFFECTS = tuple(FINISHES) + tuple(HOLO) + MODIFIERS + FX


def hex_colour(text):
    """(r, g, b) of a 6-digit hex colour such as 'ff00ff', or None."""
    if len(text) == 6 and all(c in '0123456789abcdefABCDEF' for c in text):
        return tuple(int(text[i:i + 2], 16) for i in (0, 2, 4))
    return None


def art_colour(art):
    """The art's most vivid colour (the mean of its most saturated, bright pixels), at full brightness: what an
    aura glows in when no colour is given. White for grey art."""
    import colorsys
    a = np.asarray(art.convert('RGB').resize((64, 256))).reshape(-1, 3).astype(np.float64) / 255
    mx, mn = a.max(1), a.min(1)
    vivid = (mx - mn) * mx                       # saturation x value
    if vivid.max() < 0.15:
        return (255, 255, 255)
    top = a[vivid >= np.quantile(vivid, 0.9)].mean(0)
    h, _, _ = colorsys.rgb_to_hsv(*top)
    return tuple(round(255 * x) for x in colorsys.hsv_to_rgb(h, 0.85, 1.0))


def luma(art):
    """0-1 lightness of an RGB image (Rec. 709 weights on the sRGB values)."""
    a = np.asarray(art.convert('RGB')).astype(np.float64) / 255
    return a @ np.array([0.2126, 0.7152, 0.0722])


def _smoothstep(lo, hi, x):
    t = np.clip((x - lo) / (hi - lo), 0, 1)
    return t * t * (3 - 2 * t)


def spot(art):
    """0-1 weight of the light parts of the art: where a `spot` finish goes."""
    return _smoothstep(0.55, 0.75, luma(art))


def _flakes(shape, size, seed):
    """Random values 0-1, constant over size x size squares (one value per glitter flake)."""
    rng = np.random.default_rng(seed)
    h, w = shape
    small = rng.random(((h + size - 1) // size, (w + size - 1) // size))
    return np.repeat(np.repeat(small, size, 0), size, 1)[:h, :w]


def mask(finish, art, default, weight=None):
    """The deck's mask for a finish: (2048, 512, 3) uint8 RGB. `default`: the game's default mask;
    `weight`: where the finish goes (0-1 per pixel, None for everywhere); elsewhere stays default."""
    d = np.asarray(default.convert('RGB')).astype(np.float64)
    occlusion, grain = d[..., 0], d[..., 1] - STOCK_GLOSS   # keep the default's rim and its fine grain
    shape = occlusion.shape
    if finish == 'glitter':
        metal = 180 + 75 * _flakes(shape, 2, 1)
        gloss = 190 + 60 * _flakes(shape, 2, 2)
    elif finish in HOLO:
        gloss, metal = np.full(shape, 255.0), np.zeros(shape)   # the solid mask holo decks use
    else:
        smooth, metal = FINISHES[finish]
        gloss = smooth + grain
        metal = np.full(shape, float(metal))
    out = np.dstack([occlusion, gloss, metal])
    if weight is not None:
        out = d + (out - d) * weight[..., None]
    return np.clip(np.round(out), 0, 255).astype(np.uint8)


def holo_map(weight):
    """The `param_graphic_rgb` texture: red 255 where the deck is holo."""
    r = np.round(255 * (np.ones((2048, 512)) if weight is None else weight))
    return np.dstack([r, np.zeros_like(r), np.zeros_like(r)]).astype(np.uint8)


def mark(holo, effect, speed=FX_SPEED, glow=0):
    """The holo map with Caveman's shader marker painted into its top-left corner (off the deck)."""
    holo = holo.copy()
    for x0, y0, x1, y1, colour in shaderpatch.marker_pixels(effect, speed, glow):
        holo[y0:y1, x0:x1] = colour
    return holo


def normal_map(effects, art, default, strength=4.0, weight=None):
    """The deck's normal map for `effects`, or None when nothing raises the surface: (2048, 512, 3) uint8.
    `emboss` raises the light parts of the art; `glitter` tilts each flake a little (where `weight`)."""
    if 'emboss' not in effects and 'glitter' not in effects:
        return None
    d = np.asarray(default.convert('RGB')).astype(np.float64) / 127.5 - 1
    nx, ny = d[..., 0].copy(), d[..., 1].copy()
    if 'emboss' in effects:
        height = np.asarray(Image.fromarray((luma(art) * 255).astype(np.uint8)).filter(ImageFilter.GaussianBlur(2.0)))
        gy, gx = np.gradient(height.astype(np.float64) / 255)
        nx -= strength * gx
        ny -= strength * gy
    if 'glitter' in effects:
        shape = nx.shape
        w = 1 if weight is None else weight
        nx += 0.6 * (2 * _flakes(shape, 2, 3) - 1) * w
        ny += 0.6 * (2 * _flakes(shape, 2, 4) - 1) * w
    return encode_normal(nx, ny)


def encode_normal(nx, ny):
    """Tangent-space X and Y slopes as a unit normal in RGB (flat is 127.5, 127.5, 255)."""
    nz = np.ones_like(nx)
    n = np.dstack([nx, ny, nz])
    n /= np.linalg.norm(n, axis=2, keepdims=True)
    return np.clip(np.round((n + 1) * 127.5), 0, 255).astype(np.uint8)


def split(effects):
    """(finish or None, modifiers, notes) from suffixes in the order written. One finish per deck: a
    second one is left out with a note."""
    finish, mods, notes = None, [], []
    for e in effects:
        if e in FINISHES or e in HOLO:
            if finish is None:
                finish = e
            else:
                notes.append(f'one finish per deck: kept "{finish}", left out "{e}"')
        elif e not in mods:
            mods.append(e)
    if 'spot' in mods and finish is None:
        notes.append('"spot" needs a finish (such as foil or holo); left out')
        mods.remove('spot')
    if 'nyan' in mods and 'aura' in mods:
        notes.append('one board effect per deck: kept "nyan", left out "aura"')
        mods.remove('aura')
    return finish, mods, notes


def layers(effects, art, default_mask, default_normal, colour=None):
    """What the effects change, for newdeck.build: 'osk', 'ny', 'rgb' (RGB arrays), 'foilmode' (a foil mode
    name; a holo map makes it a holo-host deck) and 'aura' (an (r, g, b) colour: `colour`, or the art's).
    Empty for none."""
    finish, mods, _ = split(effects)
    out = {}
    if 'nyan' in mods:
        import vfx
        out['nyan'] = vfx.NYAN_STEPS
    elif 'aura' in mods:
        out['aura'] = colour or art_colour(art)
    weight = spot(art) if 'spot' in mods else None
    if finish:
        out['osk'] = mask(finish, art, default_mask, weight)
    if finish in HOLO:
        out['rgb'] = holo_map(weight)
        out['foilmode'] = HOLO[finish]
    if any(m in FX for m in mods):
        holo = out.get('rgb', np.zeros((2048, 512, 3), np.uint8))   # red 0: no holo on the deck
        effect = shaderpatch.EFFECT_CYCLE if 'cycle' in mods else shaderpatch.EFFECT_NONE
        out['rgb'] = mark(holo, effect, glow=FX_GLOW if 'glow' in mods else 0)
        out.setdefault('osk', np.asarray(default_mask.convert('RGB')).copy())   # the holo host's own mask is its graphic's
    n = normal_map(set(mods) | ({finish} if finish else set()), art, default_normal, weight=weight)
    if n is not None:
        out['ny'] = n
    return out
