#!/usr/bin/env python3
"""Draw the waiting room's walls and floor as an inn's: timber beams and
posts where the other rooms have stone, a back wall of upright planks
between a beam and a wainscot, and a floor of long boards, in the same
places as before, so its furniture (the fireplace, the bed, the wardrobe),
the hero's armchair and the door stay where they are.

Usage: python3 scripts/room_idle.py OUT.png [PROPS.png ROOM.png]
OUT.png gets the bare walls and floor (art/dungeon_rooms.aseprite's
`room` layer on its idle frame); with the idle frame's `props` layer
exported as PROPS.png, ROOM.png gets the whole room. Needs Pillow."""
import sys
from PIL import Image

W, H = 144, 100
INK = (24, 20, 37)
DARK = (62, 39, 49)      # seams, shadows
BEAM = (115, 62, 57)     # timber
WOOD = (184, 111, 80)    # lit wood
PALE = (194, 133, 105)   # boards in the light
NAIL = (228, 166, 114)

im = Image.new('RGBA', (W, H))
px = im.load()
def put(x, y, c):
    if 0 <= x < W and 0 <= y < H:
        px[x, y] = c + (255,)
def box(x0, y0, x1, y1, c):
    for y in range(y0, y1 + 1):
        for x in range(x0, x1 + 1):
            put(x, y, c)

# ---- Floor: long boards, 5 rows and a seam each, their ends staggered.
for y in range(28, 94):
    board, row = (y - 28) // 6, (y - 28) % 6
    for x in range(7, 137):
        c = PALE if board % 2 == 0 else WOOD
        if row == 5:
            c = BEAM                                       # the gap between boards
        elif (x + board * 17) % 34 == 0:
            c = BEAM                                       # a board's end
        elif row == 0:
            c = NAIL if board % 2 == 0 else PALE           # its lit edge
        elif (x * 7 + y * 13 + board * 5) % 23 == 0:
            c = WOOD if board % 2 == 0 else BEAM           # grain
        put(x, y, c)
    for x in range(7, 137):
        if (x + board * 17) % 34 == 2 and row == 2:
            put(x, y, DARK)                                # nail heads by the ends

# ---- Back wall: upright planks between a beam and a wainscot.
for y in range(5, 28):
    for x in range(7, 137):
        plank = (x - 7) // 6
        c = BEAM
        if (x - 7) % 6 == 0:
            c = DARK                                       # seams
        elif (x - 7) % 6 == 1:
            c = (128, 70, 62)                              # each plank's lit edge
        elif (plank * 11 + y * 3) % 29 == 0:
            c = DARK                                       # knots
        put(x, y, c)
box(7, 5, 136, 7, DARK)                                     # the beam under the ceiling
box(7, 6, 136, 6, BEAM)
box(7, 21, 136, 26, WOOD)                                   # wainscot
for x in range(7, 137):
    if (x - 7) % 16 == 0:
        box(x, 21, x, 26, BEAM)
box(7, 21, 136, 21, PALE)
box(7, 27, 136, 27, DARK)                                   # where wall meets floor
# Corner posts.
for x0 in (7, 131):
    box(x0, 5, x0 + 5, 27, BEAM)
    box(x0, 5, x0, 27, DARK)
    box(x0 + 1, 5, x0 + 1, 27, WOOD)
    box(x0 + 5, 5, x0 + 5, 27, DARK)

# ---- Frame: timber beams where the other rooms have stone.
def beam_h(y0, y1, x0, x1):
    box(x0, y0, x1, y1, BEAM)
    box(x0, y0, x1, y0, WOOD)
    box(x0, y1, x1, y1, INK)
    for x in range(x0 + 6, x1, 24):
        put(x, y0 + 2, NAIL)
def beam_v(x0, x1):
    box(x0, 0, x1, H - 1, BEAM)
    box(x0 + 1, 0, x0 + 1, H - 1, WOOD)
    box(x1, 0, x1, H - 1, INK)
    for y in range(8, H, 20):
        put(x0 + 3, y, NAIL)
beam_v(0, 6)
beam_v(137, 143)
beam_h(0, 4, 0, 143)
beam_h(94, 99, 0, 143)
box(62, 94, 81, 99, INK)                                    # the doorway

im.save(sys.argv[1])
if len(sys.argv) > 3:
    whole = im.copy()
    whole.alpha_composite(Image.open(sys.argv[2]).convert('RGBA'))
    whole.save(sys.argv[3])
