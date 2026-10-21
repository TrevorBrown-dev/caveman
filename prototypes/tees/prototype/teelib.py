"""T-shirt geometry from the player's install: the print sheet's panels (UV1 islands of the tee mesh),
the artist stencil, the test pattern, and turning a PNG into the sheet, mask and normal-map pixels.

The print sheet is the tee mesh's second UV set (UV1) taken modulo 1: the stock Vans 00006 tee puts one
1024 x 1024 logo sheet there at scale 1 (param_graphic_c / _osk / _ny), and its art lines up with:
  FRONT  (UDIM tile 0)  left half, as seen from the front, top = collar
  BACK   (tile 1)       right half, as seen from behind, top = collar
  sleeves (tile 2)      bottom left: the wearer's right sleeve, then the left, each seen from outside,
                        top = shoulder; squashed about 2:1 along the sleeve
  HEM    (tile 2)       a strip under the sleeves: the lower front, wearer's left half, at the hem
Every part with no print area (shoulder tops, side strips, collar, cuffs) has its UV1 collapsed onto a
spot at about (0.87-0.93, 0.97-0.99): whatever the sheet holds there is smeared over all of them, so the
sheet must stay transparent in that corner (DEAD)."""
import os
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont
from scipy.ndimage import binary_dilation, distance_transform_edt
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
sys.path.insert(0, HERE)
import meshlib  # noqa: E402
import raster  # noqa: E402

MESH_BUNDLE = 'win32/characters/maincharacters/generic/cas/clothing/unlicensed/generic/apparel/top/shirt/tshirtrelaxed/2023/gen_shirt_tshirtrelaxed_complex_dmpreset_cas_main_bundlereftable'
BASE_C = 'characters/maincharacters/generic/cas/clothing/unlicensed/generic/apparel/top/shirt/tshirtrelaxed/2023/gen_shirt_tshirtrelaxed_c'
SHEET = 1024                 # the game's sheet size (the Vans logo sheet's)
DEAD = (0.84, 0.95, 1.0, 1.0)   # u0, v0, u1, v1 kept transparent around the collapsed spot
FONT = r'C:\Windows\Fonts\arialbd.ttf'


def font(size):
    try:
        return ImageFont.truetype(FONT, size)
    except OSError:
        return ImageFont.load_default()


class Tee:
    """The tee mesh's print panels, read from the player's install."""

    def __init__(self, game):
        mesh = meshlib.read_mesh(game, game.bundle(MESH_BUNDLE), 0)
        self.mesh = mesh
        p, u1, tri = mesh['pos'].astype(np.float64), mesh['uv1'].astype(np.float64), mesh['tri']
        # islands: triangles joined through vertices with the same UV1
        keys = {}
        vid = np.array([keys.setdefault((round(float(a), 5), round(float(b), 5)), len(keys)) for a, b in u1])
        rows = np.concatenate([vid[tri[:, 0]], vid[tri[:, 1]], vid[tri[:, 2]]])
        cols = np.concatenate([vid[tri[:, 1]], vid[tri[:, 2]], vid[tri[:, 0]]])
        _, lab = connected_components(coo_matrix((np.ones(len(rows)), (rows, cols)), shape=(len(keys),) * 2), directed=False)
        tl = lab[vid[tri[:, 0]]]
        self.panels = {}
        for c in np.unique(tl):
            ts = tri[tl == c]
            vs = np.unique(ts)
            uv = u1[vs]
            tile = int(np.floor(uv[:, 0].mean()))
            area = np.ptp(uv[:, 0]) * np.ptp(uv[:, 1])
            if area < 1e-3:
                continue          # collapsed: no print area
            q = p[vs]
            fu = uv[:, 0] - tile
            # which panel: tile 0 front, tile 1 back, tile 2 sleeves (by side) and the hem strip
            if tile == 0:
                name = 'FRONT'
            elif tile == 1:
                name = 'BACK'
            elif uv[:, 1].min() > 0.86:
                name = 'HEM'
            else:
                name = 'R SLEEVE' if q[:, 0].mean() < 0 else 'L SLEEVE'
            # metres per UV unit along u and v (least squares of position on UV)
            A = np.c_[fu, uv[:, 1], np.ones(len(vs))]
            coef = np.linalg.lstsq(A, q, rcond=None)[0]
            self.panels[name] = {'tile': tile, 'tris': ts, 'box': (fu.min(), uv[:, 1].min(), fu.max(), uv[:, 1].max()),
                                 'm_per_u': float(np.linalg.norm(coef[0])), 'm_per_v': float(np.linalg.norm(coef[1])),
                                 'u_dir': coef[0] / np.linalg.norm(coef[0]), 'v_dir': coef[1] / np.linalg.norm(coef[1])}
        self.u1 = u1

    def mask(self, name, n):
        """The panel's area in an n x n sheet (bool)."""
        pn = self.panels[name]
        img = Image.new('L', (n, n), 0)
        d = ImageDraw.Draw(img)
        for t in pn['tris']:
            pts = [((self.u1[i, 0] - pn['tile']) * n, self.u1[i, 1] * n) for i in t]
            d.polygon(pts, fill=255)
        return np.asarray(img) > 127

    def labels(self, n, reach):
        """(names, label map): each sheet pixel within `reach` (a fraction of the sheet) of a panel takes the
        nearest panel's index (1-based; 0 = none), so bleed never crosses into a neighbour."""
        names = list(self.panels)
        lab = np.zeros((n, n), np.int32)
        for k, name in enumerate(names):
            lab[self.mask(name, n)] = k + 1
        dist, (iy, ix) = distance_transform_edt(lab == 0, return_indices=True)
        grown = lab[iy, ix]
        grown[dist > reach * n] = 0
        return names, lab, grown

    def outline(self, name, n, width=2):
        m = self.mask(name, n)
        return m & ~np.asarray(Image.fromarray(m).filter(ImageFilter.MinFilter(2 * width + 1)))

    def uv0_in_sheet(self, n):
        """Per sheet pixel, the UV0 the shirt has there (rasterising each print panel in UV1 space)."""
        uv0 = np.full((n, n, 2), np.nan)
        for name, pn in self.panels.items():
            ts = pn['tris']
            xy = np.stack([(self.u1[:, 0] - pn['tile']) * n, self.u1[:, 1] * n], 1)
            m, _, _, _, attrs = raster.rasterize(xy, np.zeros(len(xy)), ts, n, n, {'uv0': self.mesh['uv0']})
            uv0[m] = attrs['uv0'][m]
        return uv0


def sample(tex, uv):
    h, w = tex.shape[:2]
    x = np.clip(((uv[..., 0] % 1.0) * w).astype(int), 0, w - 1)
    y = np.clip(((uv[..., 1] % 1.0) * h).astype(int), 0, h - 1)
    return tex[y, x]


# ---------------------------------------------------------------- the stencil
def stencil(tee, game, n=2048):
    """The artist's guide (RGB, n x n): each print panel shows the plain shirt fabric (the generic tee
    colour texture, moved into the sheet's UV space), with its outline, name, which way is up, a 5 cm
    grid, the bleed (draw past the outline) and the corner that must stay empty."""
    base = np.asarray(game.texture_image(game.bundle(MESH_BUNDLE), BASE_C).convert('RGB')).astype(np.float64)
    uv0 = tee.uv0_in_sheet(n)
    inside = ~np.isnan(uv0[..., 0])
    img = np.zeros((n, n, 3)) + 34.0
    fabric = sample(base, np.nan_to_num(uv0))
    img[inside] = fabric[inside] * 1.05
    # bleed: 1.5% past each outline, shown as a lighter band
    out = Image.fromarray(img.clip(0, 255).astype(np.uint8))
    d = ImageDraw.Draw(out, 'RGBA')
    _, lab, grown = tee.labels(n, 0.015)
    band = (grown > 0) & (lab == 0)
    a = np.asarray(out).copy()
    a[band] = (a[band] * 0.5 + np.array([90, 140, 170]) * 0.5)
    out = Image.fromarray(a)
    d = ImageDraw.Draw(out, 'RGBA')
    # 5 cm grid inside each panel (cells square on the shirt), outlines, names, arrows
    for name, pn in tee.panels.items():
        m = tee.mask(name, n)
        u0, v0, u1, v1 = pn['box']
        du, dv = 0.05 / pn['m_per_u'], 0.05 / pn['m_per_v']
        grid = np.zeros((n, n), bool)
        for k in np.arange(np.floor(u0 / du), np.ceil(u1 / du) + 1):
            x = int(k * du * n)
            if 0 <= x < n:
                grid[:, x] = True
        for k in np.arange(np.floor(v0 / dv), np.ceil(v1 / dv) + 1):
            y = int(k * dv * n)
            if 0 <= y < n:
                grid[y, :] = True
        a = np.asarray(out).copy()
        sel = grid & m
        a[sel] = a[sel] * 0.75
        ol = tee.outline(name, n, max(1, n // 700))
        a[ol] = (0, 220, 255)
        out = Image.fromarray(a)
        d = ImageDraw.Draw(out, 'RGBA')
        cx, cy = (u0 + u1) / 2 * n, (v0 + v1) / 2 * n
        squash = pn['m_per_u'] / pn['m_per_v']          # sleeves: about 0.5
        label = {'FRONT': 'FRONT (seen from the front)', 'BACK': 'BACK (seen from behind)',
                 'R SLEEVE': 'RIGHT SLEEVE', 'L SLEEVE': 'LEFT SLEEVE', 'HEM': 'FRONT HEM (left side)'}[name]
        size = max(14, int(n * (0.022 if name in ('FRONT', 'BACK') else 0.016)))
        text_img = _text(label, size, (255, 255, 255, 200), squash)
        out.paste(text_img, (int(cx - text_img.width / 2), int(v0 * n + n * 0.012)), text_img)
        if name in ('FRONT', 'BACK', 'R SLEEVE', 'L SLEEVE'):
            _arrow(d, cx, v0 * n + n * 0.07, (v1 - v0) * n * 0.25 * (1 if name in ('FRONT', 'BACK') else 0.6), n)
            up = _text('UP (collar)' if name in ('FRONT', 'BACK') else 'UP (shoulder)', max(12, int(n * 0.011)), (255, 255, 255, 190), squash)
            out.paste(up, (int(cx + n * 0.012), int(v0 * n + n * 0.07)), up)
        if name in ('FRONT', 'BACK'):
            left, right = ("wearer's RIGHT", "wearer's LEFT") if name == 'FRONT' else ("wearer's LEFT", "wearer's RIGHT")
            for txt, x in ((left, u0 * n + n * 0.01), (right, u1 * n - n * 0.01)):
                t = _text(txt, max(12, int(n * 0.011)), (255, 255, 255, 170), 1.0)
                out.paste(t, (int(x - (t.width if x > cx else 0)), int((v0 + v1) / 2 * n)), t)
        if name in ('R SLEEVE', 'L SLEEVE'):
            # which way the body's front is (+u is the front on the right sleeve, the back on the left)
            towards = 'FRONT >' if name == 'R SLEEVE' else '< FRONT'
            t = _text(towards, max(12, int(n * 0.011)), (255, 255, 255, 170), squash)
            out.paste(t, (int(cx - t.width / 2), int(v1 * n - n * 0.03)), t)
    # the corner that must stay empty
    u0, v0, u1, v1 = DEAD
    d.rectangle([u0 * n, v0 * n, u1 * n - 1, v1 * n - 1], fill=(200, 30, 30, 120), outline=(255, 60, 60, 255), width=max(1, n // 700))
    t = _text('KEEP EMPTY', max(11, int(n * 0.0095)), (255, 255, 255, 230), 1.0)
    out.paste(t, (int((u0 + u1) / 2 * n - t.width / 2), int((v0 + v1) / 2 * n - t.height / 2)), t)
    # legend in the free corner
    lines = ['Caveman T-shirt stencil (skate. relaxed tee)',
             'Draw on a layer above this one; hide this layer and save a PNG.',
             'Transparent = shirt colour. Draw past the cyan lines (blue band).',
             'Grid: 5 cm squares on the shirt. Sleeves are squashed 2:1: draw them flat.',
             'Dark areas never show. Keep the red box empty.',
             'Generated from your own game; do not share it.']
    f = font(max(11, int(n * 0.0105)))
    y = int(0.74 * n)
    for line in lines:
        d.text((int(0.6 * n), y), line, font=f, fill=(230, 230, 230, 255))
        y += int(n * 0.016)
    return out


def _text(s, size, fill, squash):
    f = font(size)
    box = f.getbbox(s)
    w, h = box[2] - box[0] + 4, box[3] - box[1] + 4
    img = Image.new('RGBA', (w, h), (0, 0, 0, 0))
    ImageDraw.Draw(img).text((2 - box[0], 2 - box[1]), s, font=f, fill=fill)
    if abs(squash - 1) > 0.05:
        img = img.resize((w, max(1, int(round(h * squash)))), Image.LANCZOS)
    return img


def _arrow(d, x, y, length, n, fill=(255, 255, 255, 210)):
    w = max(2, n // 400)
    d.line([(x, y), (x, y + length)], fill=fill, width=w)
    d.polygon([(x, y - n * 0.012), (x - n * 0.008, y + n * 0.004), (x + n * 0.008, y + n * 0.004)], fill=fill)


# ---------------------------------------------------------------- the test pattern
PANEL_COLOURS = {'FRONT': (40, 110, 230), 'BACK': (235, 120, 20), 'R SLEEVE': (30, 170, 70), 'L SLEEVE': (200, 40, 170),
                 'HEM': (230, 200, 20)}


def test_pattern(tee, n=2048):
    """An RGBA sheet that makes the mapping obvious in game: each panel its own colour with its name, an
    arrow pointing up, a 5 cm grid numbered in centimetres from the panel's top left, which side is which,
    and a transparent band across the lower front (the shirt colour should show through it)."""
    names, _, grown = tee.labels(n, 0.012)
    a = np.zeros((n, n, 4), np.uint8)
    for k, name in enumerate(names):
        a[grown == k + 1] = PANEL_COLOURS[name] + (255,)
    img = Image.fromarray(a)
    for k_panel, (name, pn) in enumerate(tee.panels.items()):
        base_img = img
        img = Image.new('RGBA', (n, n), (0, 0, 0, 0))
        d = ImageDraw.Draw(img, 'RGBA')
        u0, v0, u1, v1 = pn['box']
        du, dv = 0.05 / pn['m_per_u'], 0.05 / pn['m_per_v']
        squash = pn['m_per_u'] / pn['m_per_v']
        lw = max(2, n // 512)
        for k in range(int(np.ceil((u1 - u0) / du)) + 1):
            x = (u0 + k * du) * n
            d.line([(x, v0 * n - n * 0.012), (x, v1 * n + n * 0.012)], fill=(255, 255, 255, 150), width=lw)
            if name in ('FRONT', 'BACK') and k % 2 == 0:
                t = _text(f'{5 * k}', max(12, n // 90), (255, 255, 255, 220), squash)
                img.paste(t, (int(x + 3), int(v0 * n + 3)), t)
        for k in range(int(np.ceil((v1 - v0) / dv)) + 1):
            y = (v0 + k * dv) * n
            d.line([(u0 * n - n * 0.012, y), (u1 * n + n * 0.012, y)], fill=(255, 255, 255, 150), width=lw)
            if name in ('FRONT', 'BACK') and k % 2 == 0 and k:
                t = _text(f'{5 * k}', max(12, n // 90), (255, 255, 255, 220), squash)
                img.paste(t, (int(u0 * n + 3), int(y + 3)), t)
        cx = (u0 + u1) / 2 * n
        big = {'FRONT': 0.06, 'BACK': 0.06, 'R SLEEVE': 0.05, 'L SLEEVE': 0.05, 'HEM': 0.03}[name]
        label = _text(name, int(n * big), (255, 255, 255, 255), squash)
        ly = (v0 + (v1 - v0) * (0.45 if name in ('FRONT', 'BACK') else 0.5)) * n - label.height / 2
        shadow = _text(name, int(n * big), (0, 0, 0, 200), squash)
        img.paste(shadow, (int(cx - label.width / 2) + n // 400, int(ly) + n // 400), shadow)
        img.paste(label, (int(cx - label.width / 2), int(ly)), label)
        d = ImageDraw.Draw(img, 'RGBA')
        if name != 'HEM':
            top = v0 * n + (v1 - v0) * n * 0.08
            length = (v1 - v0) * n * 0.22
            _arrow(d, cx, top, length, n, (255, 255, 0, 255))
            t = _text('UP', int(n * 0.022), (255, 255, 0, 255), squash)
            img.paste(t, (int(cx + n * 0.012), int(top)), t)
        if name in ('FRONT', 'BACK'):
            left, right = ('R', 'L') if name == 'FRONT' else ('L', 'R')
            for txt, x in ((left, u0 * n + n * 0.06), (right, u1 * n - n * 0.06)):
                t = _text(txt, int(n * 0.04), (255, 255, 0, 255), 1.0)
                img.paste(t, (int(x - t.width / 2), int((v0 + (v1 - v0) * 0.55) * n)), t)
        if name in ('R SLEEVE', 'L SLEEVE'):
            t = _text('FRONT >' if name == 'R SLEEVE' else '< FRONT', int(n * 0.016), (255, 255, 0, 255), squash)
            img.paste(t, (int(cx - t.width / 2), int((v0 + (v1 - v0) * 0.8) * n)), t)
        own = Image.fromarray(((grown == names.index(name) + 1) * 255).astype(np.uint8))
        layer = np.asarray(img).copy()
        layer[..., 3] = (layer[..., 3].astype(np.uint16) * (np.asarray(own) > 0)).astype(np.uint8)
        img = Image.alpha_composite(base_img, Image.fromarray(layer))
    # a transparent band across the lower front: the shirt colour shows through
    f = tee.panels['FRONT']['box']
    a = np.asarray(img).copy()
    y0, y1 = int((f[1] + (f[3] - f[1]) * 0.70) * n), int((f[1] + (f[3] - f[1]) * 0.80) * n)
    band = np.zeros((n, n), bool)
    band[y0:y1] = True
    a[band & (grown == names.index('FRONT') + 1), 3] = 0
    return Image.fromarray(a)


# ---------------------------------------------------------------- PNG -> textures
def fit_art(art, tee, where=None):
    """The player's PNG as an RGBA print sheet (SHEET x SHEET). A square PNG is a whole sheet drawn on the
    stencil. Anything else (or where='front'/'back') is one graphic, fitted inside that panel's print area
    (contained, centred across, 8% down from the panel's top)."""
    art = art.convert('RGBA')
    if where is None and abs(art.width / art.height - 1) <= 0.03:
        return art.resize((SHEET, SHEET), Image.LANCZOS)
    where = where or 'front'
    pn = tee.panels['FRONT' if where == 'front' else 'BACK']
    u0, v0, u1, v1 = pn['box']
    # the print box: 76% of the panel's width, aspect corrected for the panel's metres per UV unit
    bw = (u1 - u0) * 0.76
    bh = (v1 - v0) * 0.70
    phys_aspect = art.width / art.height                      # width / height on the shirt
    uv_aspect = phys_aspect * pn['m_per_v'] / pn['m_per_u']   # in sheet units
    w, h = bw, bw / uv_aspect
    if h > bh:
        w, h = bh * uv_aspect, bh
    sheet = Image.new('RGBA', (SHEET, SHEET), (0, 0, 0, 0))
    px = art.resize((max(1, int(round(w * SHEET))), max(1, int(round(h * SHEET)))), Image.LANCZOS)
    x = int(round(((u0 + u1) / 2 - w / 2) * SHEET))
    y = int(round((v0 + (v1 - v0) * 0.08) * SHEET))
    sheet.paste(px, (x, y), px)
    return sheet


def sheet_textures(sheet, tee):
    """(colour RGB, mask RGB) SHEET x SHEET arrays from an RGBA sheet: colour bled outward from the print (so
    mips and filtering show no dark fringe), mask R = alpha (where the print shows), G 179 (the mask's
    second channel as the stock tee logo sheets have it), B 0; the collapsed corner forced empty."""
    a = np.asarray(sheet.convert('RGBA')).astype(np.float64)
    alpha = a[..., 3].copy()
    u0, v0, u1, v1 = DEAD
    alpha[int(v0 * SHEET):, int(u0 * SHEET):] = 0
    rgb = a[..., :3]
    solid = alpha > 8
    if solid.any() and not solid.all():
        _, (iy, ix) = distance_transform_edt(~solid, return_indices=True)
        rgb = np.where(solid[..., None], rgb, rgb[iy, ix])
    mask = np.zeros((SHEET, SHEET, 3))
    mask[..., 0] = alpha
    mask[..., 1] = 179
    return np.clip(np.round(rgb), 0, 255).astype(np.uint8), np.clip(np.round(mask), 0, 255).astype(np.uint8), alpha


def preview(tee, sheet, shirt_srgb, n=768):
    """A plain render of the tee's front and back (Lambert from face normals, bind pose), side by side."""
    p = tee.mesh['pos'].astype(np.float64)
    tri = tee.mesh['tri']
    u1 = tee.mesh['uv1'].astype(np.float64)
    tex = np.asarray(sheet.convert('RGBA')).astype(np.float64)
    out = Image.new('RGB', (2 * n, n), (40, 40, 40))
    fn = np.cross(p[tri[:, 1]] - p[tri[:, 0]], p[tri[:, 2]] - p[tri[:, 0]])
    fn /= np.linalg.norm(fn, axis=1, keepdims=True) + 1e-12
    for k, side in enumerate((1, -1)):
        q = p.copy()
        q[:, 0] *= side      # seen from behind: mirror x so the viewer's right is the wearer's right
        q[:, 2] *= side
        s = n / 0.8
        xy = np.stack([n / 2 + s * q[:, 0], n * 0.04 + s * (q[:, 1].max() - q[:, 1])], 1)
        m, _, tid, _, attrs = raster.rasterize(xy, q[:, 2], tri, n, n, {'uv1': u1})
        light = np.abs((fn[tid] * np.array([0.3 * side, 0.4, 0.86 * side])).sum(-1))
        col = sample(tex, attrs['uv1'])
        al = col[..., 3:] / 255
        rgb = col[..., :3] * al + np.asarray(shirt_srgb, np.float64) * (1 - al)
        rgb = rgb * (0.35 + 0.65 * light[..., None])
        img = np.zeros((n, n, 3)) + 40
        img[m] = rgb[m]
        out.paste(Image.fromarray(img.clip(0, 255).astype(np.uint8)), (k * n, 0))
    d = ImageDraw.Draw(out)
    d.text((10, 10), 'front', font=font(22), fill=(230, 230, 230))
    d.text((n + 10, 10), 'back', font=font(22), fill=(230, 230, 230))
    return out
