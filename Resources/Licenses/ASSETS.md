# Pixel art

All bundled artwork is original and made for Agents Pixel Dungeon:

- `Sprites/rooms`: `room_working` (plus its `room_working_front` foreground layer), `room_blocked`, `room_done`, `room_idle` and `room_unknown`; each of them also 64 and 40 pixels tall (`room_<state>_medium`, `room_<state>_small`, and the workshop's `_front` layers), composed from the large rooms' layers by `scripts/room_sizes.py`, which also writes `src/room_moves.rs`.
- `Icon/`: the app icon, the guild's knight helm on a dungeon tile, drawn by `scripts/icon.py`.
- `Sprites/heroes`: one classic hero per harness, coloured after each tool's branding without reproducing logos.

They use the Endesga-32 palette, and their Aseprite sources are in the repository's `art/` folder. Copyright (c) 2026 Nacho Valencia, MIT.
