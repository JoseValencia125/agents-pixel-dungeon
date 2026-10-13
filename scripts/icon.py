#!/usr/bin/env python3
"""Draw the app icon: a crystal orb on a golden stand showing a terminal
prompt (an amber chevron and underscore), on a burgundy dungeon
wall that fills the whole square (macOS rounds its corners), as pixel
art scaled up with no smoothing. The orb is the agent's
scrying glass; the >_ says it is a coding agent in a terminal. Writes
Resources/Icon/icon.png (1024 px) and the sizes an .iconset needs. Pure
Python, no PIL."""
import os, struct, zlib

# Endesga-32 colours, as in the sprites, warm to go with the burgundy:
# a brass ring like the golden stand, a dark plum glass, and the prompt in
# an amber terminal's colour.
BG = (0x5e, 0x1c, 0x30)            # burgundy wall
BG_LIGHT = (0x7a, 0x2a, 0x40)      # its bricks' lit edges
INK = (0x18, 0x14, 0x25)
SCREEN = (0x24, 0x14, 0x22)        # dark plum orb interior
GOLD = (0xfe, 0xe7, 0x61)
GOLD_DARK = (0xc0, 0x96, 0x38)
PORTAL = (0xc2, 0x85, 0x69)        # brass ring
PORTAL_LIGHT = (0xe8, 0xb7, 0x96)
TERM = (0xfe, 0xae, 0x34)          # amber prompt
TERM_DARK = (0xf7, 0x76, 0x22)
WHITE = (0xf8, 0xe8, 0xd0)         # cream glint

SIZE = 44  # canvas in art pixels

def pixels():
    px = {}
    for y in range(SIZE):
        for x in range(SIZE):
            # The whole square: macOS masks it to its rounded shape.
            brick = ((y // 6) % 2 == 0 and x % 12 == 0) or ((y // 6) % 2 == 1 and (x + 6) % 12 == 0) or y % 6 == 0
            px[(x, y)] = BG_LIGHT if brick and (x + y) % 3 == 0 else BG

    def inside(x, y):
        return (x, y) in px

    def dot(x, y, c):
        if inside(x, y):
            px[(x, y)] = c

    def box(x0, y0, x1, y1, c):
        for yy in range(y0, y1 + 1):
            for xx in range(x0, x1 + 1):
                dot(xx, yy, c)

    # ---- Crystal orb, centred and a touch above middle ----
    ocx, ocy, orad = 22, 19, 15
    for yy in range(ocy - orad, ocy + orad + 1):
        for xx in range(ocx - orad, ocx + orad + 1):
            d = (xx - ocx) ** 2 + (yy - ocy) ** 2
            if d <= orad ** 2:
                dot(xx, yy, SCREEN)
            if (orad - 1) ** 2 < d <= orad ** 2:
                dot(xx, yy, PORTAL)
            elif (orad - 2) ** 2 < d <= (orad - 1) ** 2:
                dot(xx, yy, PORTAL_LIGHT)
    # highlight glint
    for (hx, hy) in [(-7, -9), (-6, -9), (-7, -8), (-8, -8)]:
        dot(ocx + hx, ocy + hy, WHITE)

    # ---- Big obvious >_ prompt, centred as a group in the orb ----
    # The group spans the chevron (8 px wide) + a 2 px gap + the
    # underscore (9 px wide) = 19 px. Centre it on the orb centre.
    group_w = 19
    left = ocx - group_w // 2
    # Chevron ">": symmetric, 7 rows, constant 2px stroke, tip centred.
    gx, gy = left, ocy - 4
    for dy in range(7):
        step = dy if dy <= 3 else 6 - dy   # 0,1,2,3,2,1,0
        x0 = step * 2
        dot(gx + x0, gy + dy, TERM)
        dot(gx + x0 + 1, gy + dy, TERM)
    # soft inner shadow along the lower edge for thickness
    for dy in range(4, 7):
        step = 6 - dy
        dot(gx + step * 2, gy + dy, TERM_DARK)

    # Underscore "_": wide bar sharing the chevron baseline.
    ux0 = left + 10
    uy = gy + 6
    box(ux0, uy, ux0 + 8, uy, TERM)
    box(ux0, uy - 1, ux0 + 8, uy - 1, TERM_DARK)

    # ---- Golden stand ----
    by = ocy + orad
    box(ocx - 7, by, ocx + 7, by + 1, GOLD_DARK)
    box(ocx - 4, by + 2, ocx + 4, by + 3, GOLD)
    box(ocx - 8, by + 4, ocx + 8, by + 4, GOLD_DARK)

    return px

def png(path, size, px):
    # Nearest pixel of the art for each pixel of the image, so the wall
    # reaches every edge at any size.
    rows = []
    for y in range(size):
        row = bytearray([0])
        for x in range(size):
            p = px.get((x * SIZE // size, y * SIZE // size))
            row += bytes(p) + b"\xff" if p else b"\x00\x00\x00\x00"
        rows.append(bytes(row))
    raw = b"".join(rows)
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(data)

if __name__ == "__main__":
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "Resources", "Icon")
    os.makedirs(root, exist_ok=True)
    px = pixels()
    png(os.path.join(root, "icon.png"), 1024, px)
    png(os.path.join(root, "icon_256.png"), 256, px)
    iconset = os.path.join(root, "icon.iconset")
    os.makedirs(iconset, exist_ok=True)
    for size in (16, 32, 64, 128, 256, 512, 1024):
        png(os.path.join(iconset, f"icon_{size}x{size}.png"), size, px)
        if size >= 32:
            png(os.path.join(iconset, f"icon_{size // 2}x{size // 2}@2x.png"), size, px)
    print("icons written to", root)
