#!/usr/bin/env python3
"""Draw the waiting room: an inn's bedroom at night. Plaster walls between
dark timber, a window with the moon whose light falls across the board
floor, a small hearth in the corner, a canopy bed with a patchwork quilt
and a trunk at its foot, a coat rack, a barrel and a little table with a
mug and a candle, and a round rug. The hero reads in an armchair by the
window (the code draws the armchair, at the room style's spot).

The walls, floor and frame keep the other rooms' places (the door, the
back wall's height), so the hero walks in and out the same way.

Usage: python3 scripts/room_idle.py ROOM.png [BASE.png PROPS.png]
ROOM.png gets the whole room (Resources/Sprites/rooms/room_idle.png); BASE
and PROPS get its walls and floor and its furniture apart, the layers
scripts/room_sizes.py composes the shorter rooms from. Needs Pillow."""
import sys
from PIL import Image

W, H = 144, 100
INK = (24, 20, 37)
DARK = (62, 39, 49)        # timber in shadow, seams
BEAM = (115, 62, 57)       # timber
WOOD = (184, 111, 80)      # lit wood
PALE = (194, 133, 105)     # boards
HONEY = (228, 166, 114)
PLASTER = (139, 155, 180)  # the walls' plaster at night
PLASTER_LIT = (192, 203, 220)
PLASTER_DIM = (90, 105, 136)
NIGHT = (38, 43, 68)
NIGHT_LOW = (58, 68, 102)
MOON = (234, 212, 170)
STAR = (255, 255, 255)
CURTAIN = (162, 38, 51)
CURTAIN_DARK = (115, 30, 45)
STONE = (90, 105, 136)
STONE_LIT = (139, 155, 180)
FIRE = [(247, 118, 34), (254, 174, 52), (254, 231, 97)]
GREEN = (62, 137, 72)
GREEN_DARK = (38, 92, 66)
QUILT = [(162, 38, 51), (234, 212, 170), (58, 68, 102), (62, 137, 72)]
RUG = (58, 68, 102)
RUG_RING = (139, 155, 180)
METAL = (139, 155, 180)
GOLD = (254, 231, 97)


class Layer:
    def __init__(self):
        self.im = Image.new('RGBA', (W, H))
        self.px = self.im.load()

    def put(self, x, y, c, a=255):
        if 0 <= x < W and 0 <= y < H:
            self.px[x, y] = tuple(c) + (a,)

    def box(self, x0, y0, x1, y1, c):
        for y in range(y0, y1 + 1):
            for x in range(x0, x1 + 1):
                self.put(x, y, c)

    def get(self, x, y):
        return self.px[x, y][:3]


def mix(a, b, k):
    return tuple(round(a[i] * (1 - k) + b[i] * k) for i in range(3))


def walls_and_floor():
    base = Layer()
    # Floor: long boards, 5 rows and a seam each, their ends staggered.
    for y in range(28, 94):
        board, row = (y - 28) // 6, (y - 28) % 6
        for x in range(7, 137):
            c = PALE if board % 2 == 0 else WOOD
            if row == 5 or (x + board * 17) % 34 == 0:
                c = BEAM
            elif row == 0:
                c = HONEY if board % 2 == 0 else PALE
            elif (x * 7 + y * 13 + board * 5) % 23 == 0:
                c = WOOD if board % 2 == 0 else BEAM
            base.put(x, y, c)
    # The moon's light through the window, slanting across the boards.
    for y in range(28, 70):
        k = (y - 28) / 42
        x0, x1 = round(56 + k * 22), round(82 + k * 26)
        for x in range(x0, x1 + 1):
            if 7 <= x < 137:
                base.put(x, y, mix(base.get(x, y), PLASTER_LIT, 0.28 * (1 - k * 0.6)))
    # Back wall: plaster between timber, posts, a brace each side.
    for y in range(5, 28):
        for x in range(7, 137):
            base.put(x, y, PLASTER if (x * 3 + y * 5) % 17 else PLASTER_DIM)
    base.box(7, 5, 136, 7, DARK)
    base.box(7, 6, 136, 6, BEAM)
    base.box(7, 25, 136, 27, DARK)
    base.box(7, 25, 136, 25, BEAM)
    for x0 in (7, 40, 96, 131):
        base.box(x0, 5, x0 + 4, 27, BEAM)
        base.box(x0, 5, x0, 27, DARK)
        base.box(x0 + 1, 5, x0 + 1, 27, WOOD)
    for (xa, xb) in ((12, 39), (101, 130)):
        for i in range(xb - xa):
            y = 24 - round(i * 16 / (xb - xa))
            base.box(xa + i, y - 1, xa + i, y, BEAM)
    # Frame: timber beams and posts, the doorway in the bottom one.
    def beam_h(y0, y1):
        base.box(0, y0, W - 1, y1, BEAM)
        base.box(0, y0, W - 1, y0, WOOD)
        base.box(0, y1, W - 1, y1, INK)
        for x in range(6, W, 24):
            base.put(x, y0 + 2, HONEY)
    def beam_v(x0, x1):
        base.box(x0, 0, x1, H - 1, BEAM)
        base.box(x0 + 1, 0, x0 + 1, H - 1, WOOD)
        base.box(x1, 0, x1, H - 1, INK)
        for y in range(8, H, 20):
            base.put(x0 + 3, y, HONEY)
    beam_v(0, 6)
    beam_v(137, 143)
    beam_h(0, 4)
    beam_h(94, 99)
    base.box(62, 94, 81, 99, INK)
    return base


def furniture():
    p = Layer()
    # ---- Window: night sky, the moon and stars, a cross of mullions.
    for y in range(8, 24):
        for x in range(57, 80):
            p.put(x, y, mix(NIGHT, NIGHT_LOW, (y - 8) / 16))
    for (sx, sy) in ((74, 10), (77, 13), (72, 20), (59, 19), (77, 21), (64, 22)):
        p.put(sx, sy, STAR)
    for y in range(9, 16):
        for x in range(58, 66):
            d1 = (x - 61.5) ** 2 + (y - 12) ** 2
            d2 = (x - 63.5) ** 2 + (y - 11) ** 2
            if d1 <= 9 and d2 > 7:
                p.put(x, y, MOON)
    p.box(55, 7, 81, 7, DARK)
    p.box(55, 7, 56, 25, DARK)
    p.box(80, 7, 81, 25, DARK)
    p.box(68, 7, 68, 24, DARK)
    p.box(57, 16, 79, 16, DARK)
    p.box(53, 24, 83, 25, WOOD)                 # the sill
    p.box(53, 25, 83, 25, BEAM)
    # Curtains, tied back.
    p.box(49, 6, 87, 6, HONEY)                  # the rod
    for (x0, x1) in ((49, 54), (82, 87)):
        for y in range(7, 26):
            narrow = 1 if 13 <= y <= 16 else 0
            lo, hi = (x0 + narrow, x1) if x0 == 49 else (x0, x1 - narrow)
            for x in range(lo, hi + 1):
                p.put(x, y, CURTAIN_DARK if (x - x0) % 2 == 0 else CURTAIN)
        for x in range(x0, x1 + 1):
            p.put(x, 14, GOLD)                  # the tie
    # ---- Hearth in the corner: stone, a small fire.
    p.box(10, 9, 30, 27, STONE)
    p.box(10, 9, 30, 9, STONE_LIT)
    p.box(9, 8, 31, 10, BEAM)                   # mantel
    p.box(9, 8, 31, 8, WOOD)
    p.box(14, 15, 26, 27, INK)
    p.box(15, 14, 25, 14, INK)
    for (x, y, c) in [(x, y, FIRE[0]) for x in range(16, 25) for y in range(23, 27)] + \
                     [(x, y, FIRE[1]) for x in range(17, 24) for y in range(21, 25) if (x + y) % 3] + \
                     [(x, y, FIRE[2]) for x in range(19, 22) for y in range(19, 23) if (x + y) % 2]:
        p.put(x, y, c)
    for x in range(10, 31, 5):
        p.put(x, 18, STONE_LIT)
        p.put(x + 2, 23, STONE_LIT)
    # ---- Coat rack: a pole, a hook bar, a hooded cloak and a hat on top.
    p.box(36, 12, 37, 40, BEAM)
    p.box(33, 40, 40, 41, DARK)
    p.box(32, 15, 41, 15, DARK)
    for y in range(16, 34):
        w = 4 if y < 22 else 5
        for x in range(37 - w, 37 + w):
            edge = x in (37 - w, 36 + w)
            fold = (x - (37 - w)) % 3 == 1 and y > 20
            p.put(x, y, GREEN_DARK if edge or fold else GREEN)
    p.box(34, 16, 39, 18, GREEN_DARK)           # the hood over the shoulders
    p.box(35, 17, 38, 17, GREEN)
    p.box(32, 33, 41, 33, GREEN_DARK)           # the hem
    p.box(32, 13, 41, 14, DARK)                 # the hat's brim
    p.box(34, 9, 39, 12, BEAM)                  # its crown
    p.box(34, 12, 39, 12, GOLD)                 # and band
    # ---- Canopy bed on the right: posts, drape, headboard, pillow, quilt.
    for x0 in (103, 133):
        p.box(x0, 20, x0 + 1, 67, DARK)
    p.box(103, 20, 134, 23, CURTAIN)
    for x in range(103, 135):
        if x % 4 in (0, 1):
            p.put(x, 24, CURTAIN_DARK)
            p.put(x, 25, CURTAIN_DARK)
    p.box(105, 26, 132, 31, BEAM)
    p.box(105, 26, 132, 26, WOOD)
    p.box(107, 32, 130, 36, MOON)
    p.box(107, 36, 130, 36, PALE)
    for y in range(37, 63):
        for x in range(105, 133):
            p.put(x, y, QUILT[((x - 105) // 5 + (y - 37) // 5) % 4])
    p.box(105, 37, 132, 37, DARK)
    p.box(105, 62, 132, 62, DARK)
    p.box(105, 63, 132, 67, BEAM)
    p.box(105, 63, 132, 63, WOOD)
    # ---- Trunk at the bed's foot.
    p.box(108, 70, 129, 80, BEAM)
    p.box(108, 70, 129, 72, WOOD)
    p.box(108, 73, 129, 73, DARK)
    p.box(108, 80, 129, 80, DARK)
    for x in (112, 125):
        p.box(x, 70, x, 80, METAL)
    p.box(118, 74, 119, 76, GOLD)
    # ---- Barrel, and a little table with a mug and a candle.
    p.box(11, 52, 21, 64, BEAM)
    p.box(12, 51, 20, 51, WOOD)
    for x in (13, 16, 19):
        p.box(x, 52, x, 64, DARK)
    for y in (54, 61):
        p.box(11, y, 21, y, METAL)
    p.box(24, 56, 38, 58, WOOD)                 # the table's top
    p.box(24, 58, 38, 58, BEAM)
    p.box(26, 59, 27, 66, DARK)
    p.box(35, 59, 36, 66, DARK)
    p.box(27, 52, 30, 55, METAL)                # the mug
    p.box(27, 52, 30, 52, STAR)
    p.box(31, 53, 31, 54, METAL)
    p.box(34, 51, 35, 55, MOON)                 # the candle
    p.put(34, 50, FIRE[1])
    p.put(35, 49, FIRE[2])
    # ---- Round rug.
    for y in range(60, 82):
        for x in range(46, 100):
            d = ((x - 73) / 26) ** 2 + ((y - 71) / 10.5) ** 2
            if d <= 1:
                p.put(x, y, RUG_RING if d > 0.78 or 0.4 < d <= 0.5 else RUG)
    return p


base, props = walls_and_floor(), furniture()
room = base.im.copy()
room.alpha_composite(props.im)
room.save(sys.argv[1])
if len(sys.argv) > 3:
    base.im.save(sys.argv[2])
    props.im.save(sys.argv[3])
