#!/usr/bin/env python3
"""Draw Codex's hero, a hooded alchemist, into rows 2-3 of heroes.png: the
same slate-and-white palette as before, a white beard, and a glowing flask
in the hand instead of sword and shield, so it fits the library and the
alchemy table. Idle: 2 frames (the second a breath lower); work: 3 frames
(the flask is shaken up and down, a bubble rises; the hand holds it).
Writes Resources/Sprites/heroes/heroes.png, docs/media/hero_codex.png and
Codex's slot in docs/media/heroes.png.
Needs Pillow."""
import os
from PIL import Image
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sheet_path = f"{root}/Resources/Sprites/heroes/heroes.png"
P = {'b': (24, 20, 37), 'i': (255, 255, 255), 'j': (192, 203, 220), 'k': (90, 105, 136), 'l': (58, 68, 102), 'm': (139, 155, 180)}

# The body, without the hand and the flask. Eyes (b) sit between face
# pixels (j) so the sleeper's head closes them.
BODY = [
    "......bbbb......",
    ".....bkkkkb.....",
    "....bkkkkkkb....",
    "...bkkllllkkb...",
    "...bkljjjjjlb...",
    "...bkljbjbjlb...",
    "...bkljiiijlb...",
    "...bkkbiiibkb...",
    "..bkkkkbibkkb...",
    "..bkkkkkbkkkb...",
    ".bkklkkkkkkkb...",
    ".bkklkkkkkkkb...",
    ".bkklkkkkkkkb...",
    ".bkkkkkkkklkb...",
    "..bbbbbbbbbbb...",
    "...bb....bb.....",
]
# The flask with the hand (j) that holds it, drawn at (9, top).
FLASK = [
    "..bmb.",
    ".bmmmb",
    "jbiiib",
    "jbiiib",
    "..bbb.",
]

def grid(rows):
    return [list(r) for r in rows]

def stamp(g, part, x0, y0):
    for y, row in enumerate(part):
        for x, c in enumerate(row):
            if c != '.' and 0 <= y0 + y < 16 and 0 <= x0 + x < 16:
                g[y0 + y][x0 + x] = c

def frame(dy_flask=0, bubble=None, breathe=False):
    # One column to the left, so the head fits the sleeper's crop (x 1-11).
    g = grid(row[1:] + "." for row in BODY)
    stamp(g, FLASK, 9, 7 + dy_flask)
    if bubble:
        x, y = bubble
        g[y][x] = 'i'
    if breathe:
        # A breath: everything one row lower, one robe row fewer.
        g = [list('.' * 16)] + g[:13] + g[14:]
    return g

idle = [frame(), frame(breathe=True)]
work = [frame(-1, (12, 4)), frame(-2, (11, 2)), frame(0, (13, 5))]

sheet = Image.open(sheet_path).convert('RGBA')
for y in range(32, 64):
    for x in range(48):
        sheet.putpixel((x, y), (0, 0, 0, 0))
for row, frames in ((2, idle), (3, work)):
    for f, g in enumerate(frames):
        for y in range(16):
            for x in range(16):
                c = g[y][x]
                if c != '.':
                    sheet.putpixel((f * 16 + x, row * 16 + y), P[c] + (255,))
sheet.save(sheet_path)

# The README's portrait and its slot in the lineup: the first idle frame, 6x
# (heroes.png places one every 132 px from x = 18; Codex is the second).
portrait = sheet.crop((0, 32, 16, 48)).resize((96, 96), Image.NEAREST)
portrait.save(f"{root}/docs/media/hero_codex.png")
lineup = Image.open(f"{root}/docs/media/heroes.png").convert('RGBA')
lineup.paste((0, 0, 0, 0), (150, 0, 246, 96))
lineup.alpha_composite(portrait, (150, 0))
lineup.save(f"{root}/docs/media/heroes.png")
print("ok")
