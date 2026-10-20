# Attribution and provenance

## Agents Pixel Dungeon

Agents Pixel Dungeon is made by Diabolo Studio (https://diabolostudio.com/). All code and pixel art in this repository are original work, copyright (c) 2026 Nacho Valencia, released under the MIT license in LICENSE.

The room backgrounds (Resources/Sprites/rooms) and the heroes (Resources/Sprites/heroes, one per harness) were drawn for this project in Aseprite with the Endesga-32 palette. Their sources are in art/. Hero colours are inspired by each tool's branding; no logos are reproduced, and no affiliation is implied.

## Dependencies

The app is written in Rust and links MIT/Apache-2.0 crates (egui/eframe, tray-icon, notify-rust, rodio, rfd, auto-launch and their dependencies); `cargo tree` lists them. egui bundles the Ubuntu (UFL), Hack (MIT) and Noto Emoji (OFL) fonts.

## Inspiration

Some of the guild's review discipline follows [Claude Code Game Studios](https://github.com/donchitos/claude-code-game-studios) by Donchitos (MIT): the analyst's `not_assessed` verdict (absence of evidence is not evidence of absence), findings marked verified or unverified, the adversarial reviewer, cross-system scenario walks for the systems designer and the exploit hunter, seeing a regression test fail before trusting it, captures of the running game as evidence, and the design's pillars, anti-pillars and per-system formulas, tuning knobs, edge cases and acceptance criteria (from its game-pillars and game-design-document templates). The wording here is the guild's own; no files of that project are included.

The integrated terminal uses [egui_term](https://github.com/Harzu/egui_term) (MIT), backed by alacritty_terminal (Apache-2.0).

Inspired by [Claude Dungeon / claude-pixel-agent-web](https://github.com/thousandsky2024/claude-pixel-agent-web) by thousandsky2024, which first pictured coding agents as dungeon heroes. No code or artwork from that project is included.

## Herdr

[Herdr](https://github.com/herdrdev/herdr) provides the separately installed local runtime and CLI snapshot API. Herdr is not bundled. The standalone viewer's table of agent process names (`src/standalone.rs`) follows Herdr's `src/detect/mod.rs`, copyright the Herdr authors, Apache License 2.0. Vendor names only identify the agents Herdr reports.

Independent project; no affiliation with, endorsement by or sponsorship from Herdr, Inc., Anthropic, OpenAI, Amazon, Google or thousandsky2024. "Herdr" and the other names appear only to describe what the app connects to or displays, and are trademarks of their respective owners.
