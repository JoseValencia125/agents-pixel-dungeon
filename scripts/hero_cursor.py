#!/usr/bin/env python3
"""Draw Cursor's silver scout in the existing 16px hero system (Pillow).
Two idle and three work frames; insert before the generic hero, once.
Run after exporting Aseprite, like hero_codex.py.
"""
from pathlib import Path
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PATH = ROOT / 'Resources/Sprites/heroes/heroes.png'
PALETTE = {
    'k': (24, 20, 37, 255), 's': (82, 89, 102, 255),
    'd': (48, 52, 63, 255), 'w': (244, 247, 250, 255),
    'g': (174, 188, 203, 255), 'f': (232, 183, 150, 255),
    'c': (104, 220, 211, 255),
}
BODY = [
    '.....kkkk.......',
    '....kggggk......',
    '...kgwwgggk.....',
    '..kggssssggk....',
    '..ksffffffsk....',
    '..ksfkfkffsk....',
    '..ksffffffsk....',
    '...ksffffsk.....',
    '..kssssssssk....',
    '.ksssgggssssk...',
    '.ksdsscssdssk...',
    '.ksdsssssdssk...',
    '..kddgggddsk....',
    '..kssssssssk....',
    '...kkk..kkk.....',
    '...kdk..kdk.....',
]
BLADE = ['k....', 'kwk..', 'kwwk.', 'kwggk', 'kkwk.', '..kk.']


def frame(lift=0, breathe=False):
    out = Image.new('RGBA', (16, 16))
    for y, row in enumerate(BODY):
        assert len(row) == 16
        for x, color in enumerate(row):
            if color != '.':
                yy = y + (1 if breathe and y < 13 else 0)
                out.putpixel((x, yy), PALETTE[color])
    for y, row in enumerate(BLADE):
        for x, color in enumerate(row):
            if color != '.':
                out.putpixel((11 + x, 4 + lift + y), PALETTE[color])
    out.putpixel((12, 10 + lift), PALETTE['f'])
    return out


def main():
    old = Image.open(PATH).convert('RGBA')
    assert old.size in [(48, 160), (48, 192)]
    sheet = Image.new('RGBA', (48, 192))
    sheet.paste(old.crop((0, 0, 48, 128)), (0, 0))
    sheet.paste(old.crop((0, old.height - 32, 48, old.height)), (0, 160))
    for row, frames in [(8, [frame(), frame(breathe=True)]),
                        (9, [frame(-1), frame(-2), frame(1)])]:
        for i, art in enumerate(frames):
            sheet.paste(art, (16 * i, 16 * row))
    sheet.save(PATH)
    sheet.crop((0, 128, 16, 144)).resize((96, 96), Image.Resampling.NEAREST).save(ROOT / 'docs/media/hero_cursor.png')
    lineup = Image.new('RGBA', (132 * 6, 96))
    for i in range(6):
        portrait = sheet.crop((0, i * 32, 16, i * 32 + 16)).resize((96, 96), Image.Resampling.NEAREST)
        lineup.paste(portrait, (18 + 132 * i, 0))
    lineup.save(ROOT / 'docs/media/heroes.png')


if __name__ == '__main__':
    main()
