<p align="center"><img src="Resources/Icon/icon_256.png" width="128" alt="Agents Pixel Dungeon icon: a crystal orb showing a terminal prompt, on a wooden table by a burgundy dungeon wall"></p>

# Agents Pixel Dungeon

**A native pixel-art guild for your live [Herdr](https://herdr.dev) agents, for macOS and Linux.**

Each coding agent is a hero in a dungeon room that changes with its real Herdr status: working at the forge, asking for your attention, exhausted on an infirmary cot until its usage limit resets, reading while it waits, done. Click a room to answer the agent, accept or decline its permission prompts, or summon a new one. Rust, `egui`, a status item in the menu bar or tray, no browser or server.

<p align="center">
  <img src="docs/media/demo.gif" width="460" alt="The widget in demo mode: six agents working, asking for attention, trapped by the session limit, waiting and done">
</p>

<p align="center"><a href="docs/media/demo.mp4">Watch the full demo video</a> · <a href="docs/media/widget.png">Full-size screenshot</a> · <a href="docs/GUIDE.md">Guide</a></p>

## Download

Binaries for macOS (`.app`) and Linux (x86_64) are on the [Releases](https://github.com/JoseValencia125/agents-pixel-dungeon/releases) page. The macOS app is ad-hoc signed, not notarized: the first time, right-click it and choose Open.

## States and rooms

| Herdr state | | Room | Hero |
| --- | --- | --- | --- |
| `working` | <img src="docs/media/room_working.png" width="216" alt=""> | Workshop: desk, bookshelves, forge, alchemy table | Works at the station of its last tool: reads, forges, brews, summons, plans or types |
| `blocked` | <img src="docs/media/room_blocked.png" width="216" alt=""> | Sealed door, braziers, a rug | Jumps on the rug under a pulsing red "!" |
| session limit | <img src="docs/media/room_limited.png" width="216" alt=""> | Infirmary with a cot under the red cross | Exhausted, asleep on the cot under a breathing blanket, with the time left to recover until the reset |
| `idle` | <img src="docs/media/room_idle.png" width="216" alt=""> | Chamber at night, by the fireplace | Reads in an armchair by the fire, turning a page now and then |
| `done` | <img src="docs/media/room_done.png" width="216" alt=""> | Treasure chamber | Jumps for joy under a green tick, confetti flying |
| unknown | <img src="docs/media/room_unknown.png" width="216" alt=""> | Ruins with a portal | Wanders, looking around |

When the state changes the hero walks out through the door, the room changes behind it, and it walks back in.

The session limit is not a Herdr state: the dungeon reads it from Claude Code's own files when an agent or one of its background sessions hits its usage limit ("You've hit your session limit · resets 2:20pm"), and shows it until the limit resets.

## Integrated terminal

Click `>_` in an agent's chat, or choose the terminal action from its room's context menu, to open an integrated shell in that agent's folder. Each folder has its own session, with terminal colours, scrolling, selection, paste and interactive commands. Closing the terminal window keeps its session alive while the app runs; open it again from the same folder to continue. Run `exit` to end the shell, then choose **New session** to start again. Quitting the app closes its terminals. **External terminal** remains available in the terminal toolbar.

This shell is independent from the agent's chat and runs your login shell (`$SHELL`, with `/bin/bash` as fallback).

## Heroes

| Harness | Hero | |
| --- | :---: | --- |
| **claude** (Claude Code) | <img src="docs/media/hero_claude.png" width="64" alt="Coral wizard"> | Coral wizard with a staff |
| **codex** (OpenAI Codex) | <img src="docs/media/hero_codex.png" width="64" alt="Monochrome alchemist"> | Monochrome hooded alchemist with a glowing flask |
| **kiro** (Amazon Kiro) | <img src="docs/media/hero_kiro.png" width="64" alt="Purple-hooded rogue"> | Purple-hooded rogue with a ghost face |
| **gemini** (Google Gemini CLI) | <img src="docs/media/hero_gemini.png" width="64" alt="Blue star cleric"> | Blue star cleric |
| any other | <img src="docs/media/hero_hero.png" width="64" alt="Green adventurer"> | Green adventurer |

Original pixel art drawn in Aseprite (sources in `art/`), coloured after each tool's brand; no logos are reproduced.

## What it does

- **Chat with an agent**: its terminal's last lines, a box to reply (it grows to three lines, ⇧⏎ breaks a line), ↑/↓ to pick among a question's options, Accept / Decline / Stop buttons, a `>_` button that opens a terminal in the agent's folder, `/exit` from the right-click menu.
- **Background shells**: a cauldron bubbles in the room for each shell a Claude Code agent left running; the chat shows its output and can stop it, and you hear when it ends.
- **Summon agents** in any folder with a first prompt, through Herdr.
- **The guild's HP** in full screen: each harness's plan left (Claude's week and 5-hour session, Codex's limits, Kiro's monthly credits) and when it refills.
- **Session limits**: an exhausted agent rests on a cot in the infirmary with the time left until its limit resets, also shown in its chat.
- **Chimes and desktop notifications** when an agent needs help or finishes.
- **Subagents** as small heroes at the station of their own tool (Claude Code).
- **Filters, search and an activity log**; the window resizes to whole rooms, scrolls, and remembers where you left it.
- **Full screen** (⌃⌘F, F11 on Linux): its own space, one column of rooms on the left, the live console (colours and all) in the rest.
- **Event-driven**: Herdr's socket events the moment they happen, snapshots only to reconcile.
- Spanish, English, French, Italian and Portuguese.

The [guide](docs/GUIDE.md) has the details: how states are read, background jobs, the menu, configuration, privacy and the optional viewer mode without Herdr.

## Build and open

Requires [Rust](https://rustup.rs) (stable) and [Herdr](https://herdr.dev). Tested with Herdr 0.9.x.

```sh
git clone https://github.com/JoseValencia125/agents-pixel-dungeon.git
cd agents-pixel-dungeon
./scripts/build.sh
```

- **macOS**: `open 'build/Agents Pixel Dungeon.app'` (optionally copy it to `~/Applications`).
- **Linux** (X11 or Wayland): `./build/agents-pixel-dungeon`. Needs GTK 3, libayatana-appindicator, xdo and ALSA — on Debian/Ubuntu `sudo apt install libgtk-3-dev libayatana-appindicator3-dev libxdo-dev libasound2-dev`. `build/` also has a `.desktop` entry and icon.

`cargo test` and `--self-test` run the checks; `--demo` shows fictional agents without Herdr.

## Support the Guild 🧪⚔️

Agents Pixel Dungeon is free and open source, built alongside other experiments in AI agents, developer tooling and pixel-art interfaces. If it brightens your terminal, you can buy the guild a potion on [GitHub Sponsors](https://github.com/sponsors/JoseValencia125).

<p align="center"><a href="https://github.com/sponsors/JoseValencia125"><img src="https://img.shields.io/badge/Sponsor-Buy%20the%20guild%20a%20potion-db61a2?logo=githubsponsors&logoColor=white" alt="Sponsor Nacho Valencia on GitHub"></a></p>

| Potion | Price | |
| --- | --- | --- |
| 🧪 [**Small Potion**](https://github.com/sponsors/JoseValencia125/sponsorships?tier_id=665811) | $3, one-time | A small thank-you that keeps the project alive and the agents adventuring. |
| 🧪✨ [**Greater Potion**](https://github.com/sponsors/JoseValencia125/sponsorships?tier_id=665812) | $10, one-time | For those who really enjoy the project and want to give development an extra boost. |

Prefer another amount? Pick a [custom one-time or monthly sponsorship](https://github.com/sponsors/JoseValencia125). Every potion goes into new heroes and rooms, bug fixes and keeping up with Herdr releases. No paywalls: every feature stays free for everyone.

Thank you for supporting independent open-source development. ⚔️

## Credits and licenses

Created by **Nacho Valencia**: code and all pixel art. MIT, see [LICENSE](LICENSE) and [NOTICE.md](NOTICE.md).

Inspired by **[Claude Dungeon](https://github.com/thousandsky2024/claude-pixel-agent-web)** by [thousandsky2024](https://github.com/thousandsky2024), which first pictured coding agents as dungeon heroes. No code or artwork from it is included.

[Herdr](https://github.com/herdrdev/herdr) is by the Herdr contributors and is installed separately. Agents Pixel Dungeon is an independent, community-made project: it is not affiliated with, endorsed by or sponsored by Herdr, Inc. "Herdr" is used only to describe what the app connects to; Herdr and other names are trademarks of their respective owners.
