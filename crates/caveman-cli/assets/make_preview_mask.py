"""Writes deck_preview_mask.png: the mask caveman.preview draws (255 on the deck, 90 off it), from Caveman's measured
outline (assets/deck_outline.json), with Pillow exactly as caveman.py does. The Rust CLI embeds the PNG.
Usage: python make_preview_mask.py"""
import json
import os

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
OUTLINE = os.path.join(HERE, '..', '..', '..', 'assets', 'deck_outline.json')
rows = json.load(open(OUTLINE))['rows']
poly = [(l, y) for y, l, r in rows] + [(r, y) for y, l, r in reversed(rows)]
mask = Image.new('L', (512, 2048), 90)
ImageDraw.Draw(mask).polygon(poly, fill=255)
mask.save(os.path.join(HERE, 'deck_preview_mask.png'), optimize=True)
print(f'wrote deck_preview_mask.png ({os.path.getsize(os.path.join(HERE, "deck_preview_mask.png"))} bytes)')
