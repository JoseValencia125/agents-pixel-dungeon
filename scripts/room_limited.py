#!/usr/bin/env python3
"""Draw the usage limit's room, the infirmary, from the other rooms' art:
room_blocked's walls and floor in a clean teal, its gate behind a white
banner with the red cross, and room_idle's bed made up as a white cot (at
the same spot, so the sleeper and the breathing blanket line up). Writes
Resources/Sprites/rooms/room_limited.png. Needs Pillow."""
import os
from PIL import Image, ImageDraw
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
out = f"{root}/Resources/Sprites/rooms/room_limited.png"
blocked = Image.open(f"{root}/Resources/Sprites/rooms/room_blocked.png").convert("RGBA")
idle = Image.open(f"{root}/Resources/Sprites/rooms/room_idle.png").convert("RGBA")
im = blocked.copy()
px = im.load()
W, H = im.size

# 1. Erase the gate's X chains (pale pink lines) with the colour beside them.
CHAIN = {(232, 160, 160, 255), (246, 117, 122, 255)}
for _ in range(3):
    for y in range(0, 32):
        for x in range(40, 104):
            if px[x, y] in CHAIN:
                for dx in (-1, 1, -2, 2, -3, 3):
                    c = px[x + dx, y]
                    if c not in CHAIN: px[x, y] = c; break

# 2. The rug and the axe become floor, copied from clean floor two brick periods away.
src = blocked.copy(); spx = src.load()
def floor_from(x0, x1, y0, y1, sx):
    for y in range(y0, y1):
        for x in range(x0, x1):
            px[x, y] = spx[x + sx, y]
floor_from(50, 74, 48, 78, 48)
floor_from(74, 94, 48, 78, 24)
floor_from(60, 96, 46, 52, -48)   # the axe's handle, copied in with the rug's floor
floor_from(106, 120, 39, 54, -72)
floor_from(120, 126, 39, 54, -24)
# The bones on the floor too.
floor_from(32, 40, 78, 86, 24)

# 3. Recolour: the reds become the infirmary's teal; stone, flames and wood stay.
MAP = {
    (58, 20, 32): (25, 60, 62),
    (92, 26, 39): (38, 92, 66),
    (138, 42, 53): (46, 110, 92),
    (162, 38, 51): (46, 110, 92),
    (184, 84, 90): (38, 92, 66),
    (62, 39, 49): (24, 52, 56),
    (104, 56, 108): (38, 92, 66),
    (228, 59, 68): (228, 59, 68),
}
for y in range(H):
    for x in range(W):
        r, g, b, a = px[x, y]
        if (r, g, b) in MAP: px[x, y] = MAP[(r, g, b)] + (a,)

d = ImageDraw.Draw(im)
INK = (24, 20, 37); WHITE = (255, 255, 255); PALE = (192, 203, 220); RED = (228, 59, 68); DARKRED = (162, 38, 51)

# 4. Over the gate: a white banner with the red cross.
bx0, by0, bx1, by1 = 60, 4, 84, 27
d.rectangle([bx0, by0, bx1, by1], fill=INK)
d.rectangle([bx0 + 1, by0 + 1, bx1 - 1, by1 - 1], fill=WHITE)
d.rectangle([bx0 + 1, by1 - 2, bx1 - 1, by1 - 1], fill=PALE)
cx, cy = (bx0 + bx1) // 2, (by0 + by1) // 2 - 1
d.rectangle([cx - 2, cy - 7, cx + 2, cy + 7], fill=RED)
d.rectangle([cx - 7, cy - 2, cx + 7, cy + 2], fill=RED)
d.line([cx + 2, cy - 7, cx + 2, cy - 3], fill=DARKRED); d.line([cx + 7, cy - 2, cx + 7, cy + 2], fill=DARKRED)
d.line([cx + 2, cy + 3, cx + 2, cy + 7], fill=DARKRED)
# rod
d.line([bx0 - 2, by0, bx1 + 2, by0], fill=(115, 62, 57))

# 5. The two wall banners: white with a small red cross.
for (x0, y0) in ((19, 6), (115, 6)):
    region = [(x, y) for y in range(y0 - 1, y0 + 11) for x in range(x0 - 1, x0 + 12) if px[x, y][:3] in ((228, 59, 68), (162, 38, 51), (254, 174, 52), (254, 231, 97), (247, 118, 34))]
    for (x, y) in region: px[x, y] = WHITE + (255,)
    # cross in the middle of the banner's white
    xs = [p[0] for p in region]; ys = [p[1] for p in region]
    if xs:
        mx, my = (min(xs) + max(xs)) // 2, (min(ys) + max(ys)) // 2 - 1
        d.rectangle([mx - 1, my - 3, mx + 1, my + 3], fill=RED)
        d.rectangle([mx - 3, my - 1, mx + 3, my + 1], fill=RED)

# 6. The cot: room_idle's bed with white sheets on a steel frame.
bed = idle.crop((52, 52, 93, 66))
bp = bed.load()
for y in range(bed.height):
    for x in range(bed.width):
        r, g, b, a = bp[x, y]
        if r > 150 and g < 110:            # red blanket and its dots
            bp[x, y] = (PALE if (r, g, b) != (228, 59, 68) else WHITE) + (a,) if g < 70 else (139, 155, 180, a)
        elif (r, g, b) in ((115, 62, 57), (184, 111, 80), (62, 39, 49), (194, 133, 105)):  # wood frame
            bp[x, y] = {(115, 62, 57): (90, 105, 136), (184, 111, 80): (139, 155, 180), (62, 39, 49): (58, 68, 102), (194, 133, 105): (192, 203, 220)}[(r, g, b)] + (a,)
im.paste(bed, (52, 52))

# 7. A potion rack where the axe was.
rack = ["..k.k.k..",
        ".kRkBkGk.",
        ".kRkBkGk.",
        "kkkkkkkkk",
        "kWWWWWWWk",
        "k.k...k.k"]
cols = {"k": INK, "R": RED, "B": (0, 153, 219), "G": (99, 199, 77), "W": (115, 62, 57)}
for j, row in enumerate(rack):
    for i, ch in enumerate(row):
        if ch in cols: px[110 + i, 43 + j] = cols[ch] + (255,)

im.save(out)
print(f"Wrote {out}")
