"""The artist template: a guide layer at 2x the game's size (1024 x 4096) showing the bleed, the trim
line, the safe zone and where the trucks cover the art. Usage: make_template.py <deck_outline.json> <out png>"""
import json
import sys

from PIL import Image, ImageDraw, ImageFont

outline = json.load(open(sys.argv[1]))
S = 2                      # template scale over the game's 512 x 2048
W, H = 512 * S, 2048 * S
SAFE = 16 * S              # safe-zone inset from the trim line
# Truck baseplates and their bolt holes, in game pixels, read against the grid of the test pattern
# as it showed on the board in game (docs/deck-format.md, "Trucks").
TRUCKS = [(201, 472, 308, 638), (202, 1445, 309, 1613)]
BOLTS = [(213, 501), (297, 501), (213, 625), (297, 625), (214, 1458), (299, 1458), (214, 1582), (299, 1582)]

rows = outline['rows']
trim = [(l * S, y * S) for y, l, r in rows] + [(r * S, y * S) for y, l, r in reversed(rows)]
# The safe zone: the trim outline pulled in by SAFE at the sides and ends.
top, bottom = rows[0][0] * S, rows[-1][0] * S
safe = []
for y, l, r in rows:
    yy = y * S
    if top + SAFE <= yy <= bottom - SAFE:
        safe.append((y, l * S + SAFE, r * S - SAFE))
safe_poly = [(l, y * S) for y, l, r in safe] + [(r, y * S) for y, l, r in reversed(safe)]

img = Image.new('RGBA', (W, H), (0, 0, 0, 0))
# Bleed: everything outside the trim line, darkened.
bleed = Image.new('RGBA', (W, H), (0, 0, 0, 150))
hole = Image.new('L', (W, H), 255)
ImageDraw.Draw(hole).polygon(trim, fill=0)
img.paste(bleed, (0, 0), hole)
d = ImageDraw.Draw(img)
# Trucks.
for x0, y0, x1, y1 in TRUCKS:
    d.rounded_rectangle([x0 * S, y0 * S, x1 * S, y1 * S], radius=8 * S, fill=(255, 140, 0, 70), outline=(255, 140, 0, 230), width=3 * S)
for x, y in BOLTS:
    d.ellipse([(x - 6) * S, (y - 6) * S, (x + 6) * S, (y + 6) * S], outline=(255, 170, 60, 255), width=2 * S)
# Centre line, dotted.
for y in range(int(top), int(bottom), 24 * S):
    d.line([(W // 2, y), (W // 2, y + 8 * S)], fill=(255, 255, 255, 110), width=S)
# Safe zone, dashed.
for i in range(0, len(safe_poly) - 1, 6):
    if (i // 6) % 2 == 0:
        d.line(safe_poly[i:i + 4], fill=(0, 220, 255, 230), width=2 * S)
# Trim line.
d.line(trim + [trim[0]], fill=(255, 40, 40, 255), width=3 * S, joint='curve')

big = ImageFont.load_default(size=26 * S)
mid = ImageFont.load_default(size=15 * S)
small = ImageFont.load_default(size=11 * S)
white, cyan, red, orange = (255, 255, 255, 255), (0, 220, 255, 255), (255, 90, 90, 255), (255, 170, 60, 255)


def centred(y, text, font, fill):
    w = d.textlength(text, font=font)
    d.text(((W - w) / 2, y), text, font=font, fill=fill, stroke_width=S, stroke_fill=(0, 0, 0, 255))


centred(4 * S, 'TOP', big, white)
centred(34 * S, 'top of the board in the deck menu', small, white)
for x0, y0, x1, y1 in TRUCKS:
    centred((y0 + y1) / 2 * S - 8 * S, 'truck', mid, orange)


def legend_line(y, parts):
    """One line of coloured parts, centred, in a bleed band."""
    total = sum(d.textlength(t, font=small) for t, _ in parts)
    x = (W - total) / 2
    for text, colour in parts:
        d.text((x, y), text, font=small, fill=colour, stroke_width=S, stroke_fill=(0, 0, 0, 255))
        x += d.textlength(text, font=small)


legend_line(50 * S, [('red: trim (board edge)   ', red), ('cyan: safe zone', cyan)])
legend_line(63 * S, [('dark: bleed, extend the art to the canvas edge', white)])
centred(H - 74 * S, 'Caveman deck template, 1024 x 4096', small, white)
centred(H - 61 * S, 'any 1:4 image works; the file name is the deck name', small, white)
centred(H - 34 * S, 'BOTTOM', big, white)
img.save(sys.argv[2])
print('wrote', sys.argv[2], img.size)
