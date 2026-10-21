"""Writes the artist template for Caveman wheels: a 1024 x 1024 face graphic with the visible face, the part
that wraps over the wheel's edge, and the bearing hub marked (newwheel.FACE_SCALE, newwheel.HUB).
Usage: make_wheel_template.py [out.png ...]   (default: Caveman/assets/wheel_template_1024.png)"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from PIL import Image, ImageDraw, ImageFont  # noqa: E402
import newwheel  # noqa: E402

S = 1024
HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT = os.path.join(HERE, '..', '..', 'assets', 'wheel_template_1024.png')


def template():
    c = S / 2
    face = c / newwheel.FACE_SCALE                  # the visible face's radius
    hub = face * newwheel.HUB                        # hidden by the bearing
    core = face * 0.38                               # the core ring around the bearing: partly visible
    im = Image.new('RGBA', (S, S), (70, 70, 78, 255))                            # corners: off the wheel
    d = ImageDraw.Draw(im)
    d.ellipse((0, 0, S - 1, S - 1), fill=(150, 150, 160, 255))                  # wraps over the rounded edge
    d.ellipse((c - face, c - face, c + face, c + face), fill=(235, 235, 235, 255))  # the visible face
    d.ellipse((c - core, c - core, c + core, c + core), fill=(205, 205, 205, 255))
    d.ellipse((c - hub, c - hub, c + hub, c + hub), fill=(40, 40, 44, 255))      # the bearing
    for r, colour in ((face, (230, 40, 40)), (hub, (230, 40, 40)), (c - 1, (40, 40, 40))):
        d.ellipse((c - r, c - r, c + r, c + r), outline=colour + (255,), width=3)
    d.line((c, 0, c, S), fill=(120, 120, 120, 255), width=1)
    d.line((0, c, S, c), fill=(120, 120, 120, 255), width=1)
    big = ImageFont.truetype('arialbd.ttf', 34)
    small = ImageFont.truetype('arial.ttf', 22)
    d.text((c, c - (face + hub) / 2), 'VISIBLE FACE', font=big, fill=(30, 30, 30, 255), anchor='mm')
    d.text((c, c + (face + hub) / 2), 'keep text and detail inside the red circle', font=small, fill=(30, 30, 30, 255), anchor='mm')
    d.text((c, c), 'BEARING', font=big, fill=(220, 220, 220, 255), anchor='mm')
    d.text((c, c + 32), '(hidden)', font=small, fill=(200, 200, 200, 255), anchor='mm')
    label = Image.new('RGBA', (600, 40), (0, 0, 0, 0))   # in the band on the right, along it
    ImageDraw.Draw(label).text((300, 20), 'wraps over the rounded edge (bleed)', font=small, fill=(30, 30, 30, 255), anchor='mm')
    label = label.rotate(90, expand=True)
    im.alpha_composite(label, (round(S - (c - face) / 2 - 20), round(c - 300)))
    d.text((12, 12), 'Caveman wheel template', font=big, fill=(240, 240, 240, 255))
    d.text((12, 56), 'Name.ff0000.png = wheel colour ff0000', font=small, fill=(240, 240, 240, 255))
    d.text((12, S - 66), 'Transparent pixels show the wheel colour.', font=small, fill=(240, 240, 240, 255))
    d.text((12, S - 36), 'Top of the image = top of the wheel face.', font=small, fill=(240, 240, 240, 255))
    return im


if __name__ == '__main__':
    outs = sys.argv[1:] or [DEFAULT]
    im = template()
    for out in outs:
        os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
        im.save(out)
        print('wrote', os.path.abspath(out))
