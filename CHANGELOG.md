# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.1.0] - 2026-10-10

### Added
- The guild's HP in full screen: how much of each harness's plan is left and
  when it refills. Claude's HP is the general weekly limit and its MP the
  5-hour session; Codex's come from its session logs; Kiro's HP is the month's
  credits against a plan you pick.
- A `>_` button (chat header, the bar under the rooms, a room's right-click
  menu) that opens a terminal in the agent's folder to type a command.
- Starred folders in the summon panel, offered first.
- The chat box grows to three lines as you type and scrolls past that;
  ⇧⏎ breaks a line, ⏎ sends.

### Changed
- Codex's hero is now a hooded alchemist with a glowing flask, instead of a
  knight with sword and shield, to fit the library and the alchemy table.
- The usage-limit room is now an infirmary: the exhausted hero sleeps on a cot
  with the time left until the reset.
- The idle room: the hero reads in an armchair by the fire instead of sleeping.
- ⌘-click on a path in the console shows it in Finder (the file manager on
  Linux) instead of opening it; URLs still open in the browser.
- In full screen, lines a narrow pane wrapped are joined again, so the console
  uses its whole width.

### Fixed
- Pickers such as `/model` can be driven from the chat while the agent's empty
  prompt is still visible under them: ↑/↓, ⏎ and esc go to the picker.
- The chat box keeps the focus when new lines arrive in the terminal.
- Menus without numbers, such as Claude Code's "Do you trust this folder?",
  show all their options in the chat; ↑/↓ move the terminal's own ❯ and Accept
  or ⏎ picks it.
- In full screen, text that quotes a picker's footer no longer takes the box
  off the prompt line, and the box under the console is no longer cut off.
- ↑/↓ move through the "/" command menu again.
- In full screen, the typed text sits on the same line as the prompt's ❯.

### Privacy
- Claude's HP comes from the last `/usage` Claude Code cached, and no
  credential is read. Opt-in: **Live (token)**, under Claude's bars, reads
  Claude Code's OAuth token from the macOS keychain (or
  `~/.claude/.credentials.json`) and asks Anthropic's usage endpoint at most
  every two minutes. The guide's Privacy section describes it.

## [2.0.1] - 2026-10-09

### Added
- Full screen with the rooms in a column on the left and the live console
  (colours and all) on the right (⌃⌘F on macOS, F11 on Linux).
- Trapped room for Claude session limits: a caged hero rattles the bars over a
  countdown to the reset, also shown in the agent's chat.
- In full screen, type straight on the terminal's own prompt line, scroll back
  through its history, and set the console's text size on its own.
- A "/" menu listing the harness's commands.
- Drop files on the chat to add their paths to the message.
- In full screen, a spare "+" room to summon a new agent.
- Standalone viewer that runs without Herdr, with install/open buttons and a
  text-size setting.
- Badges count the rooms hidden above or below the scroll, and jump to them.

### Changed
- Strengthened the independence disclaimers across NOTICE, README and the guide:
  no affiliation, endorsement or sponsorship by Herdr, Inc., plus a trademarks note.
- The About dialog now shows the independent-project line in all five languages.

### Fixed
- Starting an agent retries while the new pane's shell is still booting.
- A background job's state follows its live tempo, not its sticky state field.
- Localized the "Reading project files…" status and the choose-options chat hint,
  which now uses "Enter" to match the existing localization key.

## [2.0.0]

- Initial public release of the Rust / egui rewrite.
