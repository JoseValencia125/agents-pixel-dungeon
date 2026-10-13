#!/usr/bin/env python3
"""Every room at medium (144x64) and small (144x40) heights, composed from
the large rooms' layers with each piece of furniture moved as a block, and
the table of those moves for the code (src/room_moves.rs), so the hero and
its helpers stand at the same furniture at every size.

The walls and floor are the large room's rows: medium keeps the whole back
wall and a shorter floor; small also drops the back wall's middle band.
Rugs lose plain rows from their middle. What does not fit is left out.
The infirmary (room_limited) is made here as scripts/room_limited.py makes
the large one: the gate room recoloured, a cot, a potion rack.

Usage: export art/dungeon_rooms.aseprite's layers as one PNG per frame
(frames 1-5: working, blocked, done, idle, unknown) named room_{n}.png,
props_{n}.png and glow_{n}.png (n from 0), and frame 1's front layer as
front.png, all into LAYERS_DIR; draw the waiting room's inn walls over its
`room` layer with `python3 scripts/room_idle.py LAYERS_DIR/room_3.png`; then
    python3 scripts/room_sizes.py LAYERS_DIR OUT_DIR
OUT_DIR gets room_{state}_{size}.png (and room_working_{size}_front.png),
which go in Resources/Sprites/rooms, and room_moves.rs, which goes in src/.
These rooms have no Aseprite file of their own: their source is the large
rooms' file and this script. Needs Pillow.
"""
import os
import sys
from PIL import Image, ImageDraw

layers, out = sys.argv[1], sys.argv[2]
load = lambda name: Image.open(os.path.join(layers, name)).convert('RGBA')
FRAMES = {'working': 0, 'blocked': 1, 'done': 2, 'idle': 3, 'unknown': 4}
ROWS = {'medium': [(0, 56), (92, 100)], 'small': [(0, 6), (13, 28), (37, 52), (92, 96)]}
HEIGHT = {'medium': 64, 'small': 40}
WALL_CUT = [(0, 6), (13, 100)]  # the small room's back wall loses rows 6-13


def rows(src, bands, x0=0, x1=None):
    x1 = src.width if x1 is None else x1
    part = Image.new('RGBA', (x1 - x0, sum(b - a for a, b in bands)))
    y = 0
    for a, b in bands:
        part.paste(src.crop((x0, a, x1, b)), (0, y))
        y += b - a
    return part


class Room:
    """One room at one size: its layers, and the moves for the code."""

    def __init__(self, state, size, frame):
        self.state, self.size, self.h = state, size, HEIGHT[size]
        self.src = {'room': load(f'room_{frame}.png'), 'props': load(f'props_{frame}.png'), 'glow': load(f'glow_{frame}.png'), 'front': load('front.png')}
        self.room = rows(self.src['room'], ROWS[size])
        self.glow = rows(self.src['glow'], ROWS[size])
        self.props = Image.new('RGBA', (144, self.h))
        self.front = Image.new('RGBA', (144, self.h))
        self.moves = []

    def move(self, region, dx, dy):
        """A move for points only (where the hero stands, a door)."""
        self.moves.append((*region, dx, dy))

    def put(self, box, to, layer='props', into=None, flip=False, skip=None, code=True):
        """Paste a block of the large room at `to`; the code moves points in
        it the same way, unless `code` is False."""
        part = self.src[layer].crop(box)
        if flip:
            part = part.transpose(Image.FLIP_TOP_BOTTOM)
        if skip:
            px = part.load()
            for y in range(part.height):
                for x in range(part.width):
                    if skip(box[0] + x, box[1] + y):
                        px[x, y] = (0, 0, 0, 0)
        getattr(self, into or ('front' if layer == 'front' else 'props')).alpha_composite(part, to)
        dx, dy = to[0] - box[0], to[1] - box[1]
        if code and (dx or dy):
            self.moves.append((*box, dx, dy))

    def cut(self, box):
        """A back-wall block, without the small room's missing band."""
        x0, y0, x1, y1 = box
        bands = [(max(a, y0), min(b, y1)) for a, b in WALL_CUT if min(b, y1) > max(a, y0)]
        self.props.alpha_composite(rows(self.src['props'], bands, x0, x1), (x0, y0))

    def squeeze(self, box, top, bottom, to_y):
        """A rug: its first `top` and last `bottom` rows, stacked at `to_y`."""
        x0, y0, x1, y1 = box
        self.put((x0, y0, x1, y0 + top), (x0, to_y))
        self.put((x0, y1 - bottom, x1, y1), (x0, to_y + top))

    def image(self):
        full = self.room.copy()
        full.alpha_composite(self.props)
        full.alpha_composite(self.glow)
        return full


def working(r):
    if r.size == 'medium':
        r.move((0, 36, 36, 52), 0, -6)        # anvil (forge)
        r.move((84, 66, 102, 80), -66, -25)   # cauldron
        r.move((56, 70, 84, 84), -38, -28)    # summoning, by the cauldron
        r.move((98, 62, 140, 90), 8, -27)     # potion bench (brew)
        r.move((112, 30, 140, 52), 0, -8)     # crystals
        r.move((30, 26, 112, 64), 0, -11)     # rug and desk (type, plan, read, gems)
        r.move((56, 84, 88, 130), 0, -36)     # door
        r.put((0, 0, 144, 28), (0, 0), code=False)
        r.put((28, 28, 38, 38), (28, 28), code=False)
        r.put((36, 40, 108, 63), (36, 29), code=False)
        r.put((36, 40, 108, 44), (36, 52), flip=True, code=False)
        r.put((46, 44, 98, 63), (46, 33), layer='front', code=False)
        r.put((10, 38, 28, 48), (10, 32), code=False)
        r.put((114, 32, 124, 44), (114, 24), code=False)
        r.put((124, 38, 134, 50), (124, 28), code=False)
        r.put((100, 66, 128, 83), (108, 39), skip=lambda x, y: x < 108 and y < 70, code=False)
        r.put((100, 66, 128, 83), (108, 39), layer='front', code=False)
        r.put((86, 69, 100, 79), (20, 44), code=False)
        r.put((10, 62, 18, 71), (10, 43), code=False)
    else:
        r.move((0, 36, 40, 52), 0, -14)
        r.move((84, 66, 102, 80), -58, -44)
        r.move((56, 70, 84, 84), -38, -48)
        r.move((98, 56, 140, 90), 0, -38)
        r.move((104, 28, 140, 56), 0, -13)
        r.move((30, 26, 104, 64), 0, -22)
        r.move((56, 84, 88, 130), 0, -52)
        r.cut((0, 0, 62, 28))
        r.cut((80, 0, 97, 28))
        r.put((66, 4, 78, 15), (66, 4), code=False)
        r.put((115, 8, 134, 19), (115, 6), code=False)
        r.put((46, 44, 98, 58), (46, 22), layer='front', into='props', code=False)  # the desk, without legs
        r.put((46, 44, 98, 58), (46, 22), layer='front', code=False)
        r.put((10, 38, 28, 48), (10, 24), code=False)
        r.put((114, 32, 124, 44), (114, 20), code=False)
        r.put((124, 38, 134, 50), (124, 24), code=False)
        r.put((86, 69, 100, 79), (28, 25), code=False)


def blocked(r, infirmary=False):
    """The gate room; the infirmary keeps its layout but no rug, lever or bones."""
    if r.size == 'medium':
        r.put((0, 0, 144, 30), (0, 0))
        r.put((30, 30, 37, 40), (30, 30))
        r.put((106, 30, 113, 40), (106, 30))
        r.put((12, 56, 22, 65), (12, 30))
        r.put((12, 66, 30, 83), (12, 39))
        if not infirmary:
            r.move((44, 44, 100, 82), 0, -20)  # the hero jumps on the rug
            r.squeeze((50, 48, 94, 78), 12, 12, 30)
            r.put((112, 42, 121, 51), (112, 34))
            r.put((34, 80, 39, 84), (34, 50))
    else:
        r.cut((44, 0, 100, 30))
        r.put((18, 5, 30, 16), (18, 4))
        r.put((114, 5, 126, 16), (114, 4))
        r.put((30, 30, 37, 40), (30, 21))
        r.put((106, 30, 113, 40), (106, 21))
        r.put((12, 56, 22, 65), (12, 24))
        if not infirmary:
            r.move((44, 44, 100, 82), 0, -38)
            r.squeeze((50, 48, 94, 78), 7, 7, 22)
            r.put((112, 42, 121, 51), (112, 22))


def done(r):
    if r.size == 'medium':
        r.put((0, 0, 144, 28), (0, 0), code=False)
        r.put((22, 30, 31, 43), (22, 28))
        r.put((10, 40, 20, 45), (10, 33))
        r.put((122, 40, 132, 45), (122, 33))
        r.put((100, 44, 116, 58), (100, 32))
        r.move((40, 50, 100, 84), 0, -26)      # the hero celebrates on the rug
        r.squeeze((44, 56, 96, 91), 13, 13, 30)
        r.put((30, 66, 40, 71), (30, 44))
        r.put((124, 74, 134, 79), (124, 47))
        r.put((10, 80, 20, 85), (10, 48))
        r.put((112, 82, 122, 87), (108, 50))
    else:
        for box, x in [((14, 5, 26, 16), 14), ((66, 4, 78, 15), 66), ((118, 5, 130, 16), 118)]:
            r.put(box, (x, 4))
        r.put((36, 9, 39, 15), (36, 7))
        r.put((104, 9, 107, 15), (104, 7))
        r.put((22, 30, 31, 43), (22, 22))
        r.put((100, 44, 116, 58), (100, 21))
        r.move((40, 50, 100, 84), 0, -40)
        r.squeeze((44, 56, 96, 91), 7, 7, 22)
        r.put((10, 40, 20, 45), (10, 31))
        r.put((122, 40, 132, 45), (122, 31))


def idle(r):
    if r.size == 'medium':
        r.put((0, 0, 118, 32), (0, 0), code=False)
        r.put((30, 30, 37, 38), (30, 30), code=False)
        r.put((118, 22, 134, 46), (118, 12))
        r.put((112, 59, 130, 77), (112, 38))
        r.put((10, 46, 24, 55), (10, 34))
        r.put((12, 74, 24, 85), (12, 45))
        r.move((80, 34, 96, 46), 0, -4)        # the armchair by the fire
        r.put((41, 46, 108, 68), (41, 31))     # rug, nightstand and bed
        r.put((41, 79, 108, 82), (41, 53))     # the rug's bottom border
    else:
        r.cut((40, 0, 104, 32))
        r.put((36, 9, 39, 15), (36, 7))
        r.put((98, 9, 101, 15), (98, 7))
        r.put((18, 5, 30, 16), (18, 4))
        r.put((104, 8, 118, 19), (104, 6))
        r.put((118, 22, 134, 46), (118, 12))
        r.move((80, 34, 96, 46), 0, -20)
        r.put((52, 52, 93, 66), (44, 22))      # the bed alone
        r.put((10, 46, 24, 55), (10, 24))
        r.put((12, 74, 24, 85), (26, 25))


def unknown(r):
    if r.size == 'medium':
        r.put((0, 0, 144, 41), (0, 0), code=False)
        r.put((23, 48, 37, 59), (23, 34))
        r.put((102, 52, 112, 63), (102, 38))
        r.put((110, 76, 115, 83), (118, 46))
        r.put((44, 78, 56, 84), (44, 48))
    else:
        r.cut((44, 0, 100, 39))
        r.put((14, 5, 26, 16), (14, 4))
        r.put((118, 5, 130, 16), (118, 4))
        r.put((10, 30, 19, 40), (10, 22))
        r.put((124, 34, 130, 41), (124, 25))
        r.put((23, 48, 37, 59), (23, 24))
        r.put((102, 52, 112, 63), (102, 24))
        r.put((44, 78, 56, 84), (44, 30))


# The infirmary's colours and parts, as scripts/room_limited.py draws them.
TEAL = {(58, 20, 32): (25, 60, 62), (92, 26, 39): (38, 92, 66), (138, 42, 53): (46, 110, 92), (162, 38, 51): (46, 110, 92),
        (184, 84, 90): (38, 92, 66), (62, 39, 49): (24, 52, 56), (104, 56, 108): (38, 92, 66)}
INK, WHITE, PALE, RED, DARKRED = (24, 20, 37), (255, 255, 255), (192, 203, 220), (228, 59, 68), (162, 38, 51)
CHAIN = {(232, 160, 160, 255), (246, 117, 122, 255)}
BANNER = ((228, 59, 68), (162, 38, 51), (254, 174, 52), (254, 231, 97), (247, 118, 34))


def limited(size):
    r = Room('limited', size, FRAMES['blocked'])
    blocked(r, infirmary=True)
    m = size == 'medium'
    cot_dy, rack_dy = (-16, -8) if m else (-30, -20)
    r.move((44, 40, 100, 70), 0, cot_dy)                # cot, sleeper, blanket, healing sparkles
    r.move((60, 70, 84, 80), 0, -20 if m else -37)      # the time-left tag at its foot
    r.move((106, 40, 122, 52), 0, rack_dy)              # potion rack
    im = r.room.copy()
    im.alpha_composite(r.props)
    px = im.load()
    W, H = im.size
    for _ in range(3):  # the gate's chains go
        for y in range(0, min(32, H)):
            for x in range(40, 104):
                if px[x, y] in CHAIN:
                    for dx in (-1, 1, -2, 2, -3, 3):
                        if px[x + dx, y] not in CHAIN:
                            px[x, y] = px[x + dx, y]
                            break
    for y in range(H):
        for x in range(W):
            if px[x, y][:3] in TEAL:
                px[x, y] = TEAL[px[x, y][:3]] + (px[x, y][3],)
    d = ImageDraw.Draw(im)
    # Over the gate, a white banner with the red cross.
    bx0, by0, bx1, by1 = 60, 4, 84, (27 if m else 20)
    d.rectangle([bx0, by0, bx1, by1], fill=INK)
    d.rectangle([bx0 + 1, by0 + 1, bx1 - 1, by1 - 1], fill=WHITE)
    d.rectangle([bx0 + 1, by1 - 2, bx1 - 1, by1 - 1], fill=PALE)
    cx, cy = (bx0 + bx1) // 2, (by0 + by1) // 2 - 1
    arm = 7 if m else 5
    d.rectangle([cx - 2, cy - arm, cx + 2, cy + arm], fill=RED)
    d.rectangle([cx - arm, cy - 2, cx + arm, cy + 2], fill=RED)
    d.line([cx + 2, cy - arm, cx + 2, cy - 3], fill=DARKRED)
    d.line([cx + arm, cy - 2, cx + arm, cy + 2], fill=DARKRED)
    d.line([cx + 2, cy + 3, cx + 2, cy + arm], fill=DARKRED)
    d.line([bx0 - 2, by0, bx1 + 2, by0], fill=(115, 62, 57))
    # The wall banners, white with a small red cross.
    banner_y = 6 if m else 5
    for x0 in (19, 115):
        region = [(x, y) for y in range(banner_y - 1, banner_y + 11) for x in range(x0 - 1, x0 + 12) if px[x, y][:3] in BANNER]
        for (x, y) in region:
            px[x, y] = WHITE + (255,)
        if region:
            xs, ys = [p[0] for p in region], [p[1] for p in region]
            mx, my = (min(xs) + max(xs)) // 2, (min(ys) + max(ys)) // 2 - 1
            d.rectangle([mx - 1, my - 3, mx + 1, my + 3], fill=RED)
            d.rectangle([mx - 3, my - 1, mx + 3, my + 1], fill=RED)
    # The cot: the idle room's bed in white on a steel frame.
    bed = load('room_3.png')
    bed.alpha_composite(load('props_3.png'))
    bed = bed.crop((52, 52, 93, 66))
    bp = bed.load()
    steel = {(115, 62, 57): (90, 105, 136), (184, 111, 80): (139, 155, 180), (62, 39, 49): (58, 68, 102), (194, 133, 105): (192, 203, 220)}
    for y in range(bed.height):
        for x in range(bed.width):
            rr, g, b, a = bp[x, y]
            if rr > 150 and g < 110:
                bp[x, y] = (PALE if (rr, g, b) != (228, 59, 68) else WHITE) + (a,) if g < 70 else (139, 155, 180, a)
            elif (rr, g, b) in steel:
                bp[x, y] = steel[(rr, g, b)] + (a,)
    im.paste(bed, (52, 52 + cot_dy))
    # A potion rack where the gate room's lever was.
    rack = ["..k.k.k..", ".kRkBkGk.", ".kRkBkGk.", "kkkkkkkkk", "kWWWWWWWk", "k.k...k.k"]
    cols = {"k": INK, "R": RED, "B": (0, 153, 219), "G": (99, 199, 77), "W": (115, 62, 57)}
    for j, row in enumerate(rack):
        for i, ch in enumerate(row):
            if ch in cols:
                px[110 + i, 43 + rack_dy + j] = cols[ch] + (255,)
    return r, im


BUILD = {'working': working, 'blocked': blocked, 'done': done, 'idle': idle, 'unknown': unknown}
tables = {}
for size in ('medium', 'small'):
    for state, build in BUILD.items():
        r = Room(state, size, FRAMES[state])
        build(r)
        r.image().save(f'{out}/room_{state}_{size}.png')
        if state == 'working':
            r.front.save(f'{out}/room_working_{size}_front.png')
        tables[(state, size)] = r.moves
    r, im = limited(size)
    im.save(f'{out}/room_limited_{size}.png')
    tables[('limited', size)] = r.moves

fmt = lambda v: f'{float(v):.1f}'
with open(f'{out}/room_moves.rs', 'w') as f:
    f.write("//! Generated by scripts/room_sizes.py: where each block of a room's art\n")
    f.write("//! went in its medium and small versions, for the points in it.\n\n")
    f.write('use crate::scene::{Move, RoomSize};\n\n')
    f.write("pub fn moves(state: &str, size: RoomSize) -> &'static [Move] {\n    match (state, size) {\n")
    for (state, size), moves in sorted(tables.items()):
        rust = 'RoomSize::Medium' if size == 'medium' else 'RoomSize::Small'
        items = ', '.join('(' + ', '.join(fmt(v) for v in mv) + ')' for mv in moves)
        f.write(f'        ("{state}", {rust}) => &[{items}],\n')
    f.write('        _ => &[],\n    }\n}\n')
print('ok')
