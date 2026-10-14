# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres
to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.3.0](https://github.com/JoseValencia125/agents-pixel-dungeon/compare/v2.2.0...v2.3.0) (2026-10-13)


### Added

* demo data for the diff, PR and HP, and a full screen GIF in the README ([eb2be35](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/eb2be35eb1fbfc1186248868134f6daa17c60b2a))
* drag the bar under the rooms to size the chat ([36a39c1](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/36a39c107f7704e697bf5d53ceb366682288978c))
* keep the size you drag the widget to, and drag the bar to size the chat ([#7](https://github.com/JoseValencia125/agents-pixel-dungeon/issues/7), [#8](https://github.com/JoseValencia125/agents-pixel-dungeon/issues/8)) ([6b996ee](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/6b996ee284c32dd5d8b00026cad2637c7865f18d))
* the demo shows a diff, a pull request and the guild's HP ([5aae935](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/5aae935ffde247470a328d79d0d3ddf4a96d71a2))


### Fixed

* keep the window size the user drags to ([019a169](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/019a1696eb841fc391bacc062983dcb5a887397b))

## [2.2.0](https://github.com/JoseValencia125/agents-pixel-dungeon/compare/v2.1.2...v2.2.0) (2026-10-13)


### Added

* a warm palette for the app icon on its burgundy wall ([baf67c1](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/baf67c1e60fd341faa70eed4dcec7083f02c9da8))
* each repo's guild, with its own emblem over its room ([881b40e](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/881b40e6eb9c4b568657604db1769706f2381ff2))
* full screen keeps rooms in place and calls who waits for you ([64326bc](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/64326bcab8287dbd28bc5c3ec659249fc1ca5bfe))
* full screen's column fits many more agents ([ea784c4](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/ea784c45c59358777ea18e519d98af5981419bae))
* pick an emblem's second colour, and no button strip in full screen ([97a6d80](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/97a6d80f5ffeee3ff9913b722260662cd0696e54))
* pick the size of full screen's rooms ([c5ad194](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/c5ad194b108b9d5807dc8dcec58c32bf50d13c10))
* see an agent's changed files and pull request, and keep each repo's emblem ([74f1f0a](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/74f1f0a70ba5102fda0aa266f418b7e16262fb22))
* the app icon's crystal orb stands on a wooden table ([8c6f795](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/8c6f79574e1d68bec5ccf74fde56a638446ad0c5))
* the app is now Agents Pixel Dungeon ([f4e8a4a](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/f4e8a4a0047184739956c8244c39a634b5171420))
* the crystal orb in the menu bar too, on a burgundy wall ([fc02b82](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/fc02b82070eff660044e2084697ce9cdd6b7c317))
* the diff against the last commit, the remote or main ([28ec9ad](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/28ec9ad28cd47b9a39052e19459f74f31dea8152))
* the waiting room is an inn's bedroom at night ([123425a](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/123425a08beb343209ba1a0780b1f24e3d617291))
* the waiting room is an inn's, with timber walls and a board floor ([7dc4979](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/7dc49792070c5d5d0ba0f8b494ef556393b2b1e7))
* the waiting room's hero reads in profile ([c32562b](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/c32562b979afc50768b6308139401d683512cee4))
* the waiting room's reader sits with the hearth at its back ([43df539](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/43df539803bb2a44f27bf945be7f016efd526886))


### Fixed

* a sharp crystal orb in the menu bar ([26ed971](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/26ed97120aa2f113ca067a4e83536464be4a617c))
* the chat header's buttons read Terminal, Diff, PR ([33b2fa9](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/33b2fa97d645a3acf9a8bd9fb3ec3e23ff3c44d5))
* the chat header's buttons read Terminal, PR, Diff ([2e68816](https://github.com/JoseValencia125/agents-pixel-dungeon/commit/2e688169ba4273696b1f33d58d5d7d2842f82793))

## [2.1.2] - 2026-10-13

### Added
- An integrated terminal: `>_` (chat header, a room's right-click menu)
  opens your login shell inside the app, in the agent's folder. Each folder
  keeps its session while the app runs, even with its window closed; `exit`
  ends it and **New session** starts another. **External terminal** in its
  toolbar opens the system one, as before.
- Shells an agent left running in the background bubble as cauldrons in its
  room, even while it rests; hovering them unrolls a parchment under them,
  "Brewing in the background", with what each does, for how long and its
  last line. The chat lists them over the console: open one's output,
  or stop it (after a yes). When one ends, it rings and notifies like a
  finished agent, or like one needing help if it failed. Claude Code's shells
  for now, found from the processes and its transcript.

### Changed
- A room's card shows its repo at the top; under what the agent is doing,
  its branch on the left and its state on the right.
- The state on a room's card changes as soon as the agent's does, while the
  hero is still walking to its new room.
- When an agent's terminal title only names its harness or shell (Kiro's
  `kiro-cli`), the room shows the session's own title instead: Kiro's, from
  its session in `~/.kiro/sessions/cli`, or Codex's last prompt. Kiro's
  session is found by folder, since Herdr does not report it, so its hero
  also follows its tools.
- The full-screen console scrolls back through the agent's last 1000 lines
  (Herdr's most), up from 300.
- The `/` command lists follow Kiro 2.29 and Claude Code 2.1.

### Fixed
- A new agent in a long folder (or one with accents) failed as if its
  harness were not installed: its Herdr name went past 32 characters.
- When a new agent does not start because its shell asked something first,
  the summon panel says so and shows the question.
- Codex's menus can be answered from the chat, and messages reach it without
  stray keys.
- Kiro: a leftover draft was sent glued to the next message, and its grey
  placeholder was taken for typed text.
- Claude Code: **Accept** on "Do you trust this folder?" picked the
  highlighted "No, exit"; it now picks the "Yes". The descriptions under
  AskUserQuestion's options stay out of their text, and long lists such as
  `/model` keep their first and last options.

## [2.1.0] - 2026-10-10

### Added
- The guild's HP in full screen: how much of each harness's plan is left and
  when it refills. Claude's HP is the general weekly limit and its MP the
  5-hour session; Codex's come from its session logs; Kiro's HP is the month's
  credits against a plan you pick.
- A `>_` button (chat header, a room's right-click menu) that opens a
  terminal in the agent's folder to type a command.
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
- The guild's HP options (Claude's source, Kiro's plan) show when you click
  that hero or its name, and hide with another click.
- A room change plays half again as fast: the hero walks out and back in at
  1.5× speed and the new room fades in sooner.

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
- The last hero in the guild's HP is no longer cut off.
- Kiro's `/model` picker, whose options have no numbers, is driven from the
  chat: ↑/↓, ⏎ and esc go to it instead of the box covering its highlight.
- Kiro asking for a tool's permission mid-conversation shows all its options
  in the chat, the highlighted one included.
- Text selected in the console stays selected while ⌘ is held, so ⌘C copies
  it; ⌘-click still opens links and paths.
- esc (or Stop) on a working agent puts the message it was working on back in
  the box, to edit and send again.

### Privacy
- Claude's HP comes from the last `/usage` Claude Code cached, and no
  credential is read. Opt-in: **Live (token)**, in Claude's options (click Claude), reads
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
