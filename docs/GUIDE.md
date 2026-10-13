# Agents Pixel Dungeon — guide

Everything the README leaves out: how the dungeon reads the agents, what each panel does, the menu, configuration, privacy and the standalone mode.

Agents Pixel Dungeon is an independent project, not affiliated with or endorsed by Herdr, Inc.; "Herdr" names the runtime the app connects to.

## How the rooms are kept current

The app keeps one connection to Herdr's socket subscribed to its events (`events.subscribe`: each agent pane's status, panes appearing, changing or closing, workspaces renamed or closed), so a change shows up the moment it happens. `herdr api snapshot` runs only to bootstrap, after a structural change, once a minute to reconcile, and — if the event connection drops — every second while it reconnects. Local enrichment (git branch, background jobs, subagents, tools) is read from files each second without starting processes.

When Herdr is unreachable or its data stops updating, a banner over the rooms says so with the time since the last good update; the last known rooms stay on screen, faded, while the app keeps reconnecting.

Activity means **the title reported by the terminal**, not an inferred summary. Animation represents state, not tool calls or completion percentage. Agents outside the selected Herdr session are not included.

## What a working agent does

For Claude Code agents the hero goes to the station that matches the last tool the agent called: it reads a book for `Read`, `Grep`, `Glob`, `WebSearch` and `WebFetch`, forges at the anvil for `Edit` and `Write`, brews potions for `Bash`, summons in a magic circle for `Task` (subagents), studies a scroll for todo lists and plans, and types at the laptop while thinking or writing.

Codex and Kiro agents do the same from their own session logs, matched by the session ID Herdr reports: Codex's rollout (`~/.codex/sessions/…/rollout-…-<id>.jsonl`; a shell command that only looks at files counts as reading, `apply_patch` as forging, `update_plan` as planning) and Kiro's session (`~/.kiro/sessions/cli/<id>.jsonl`; Herdr does not report Kiro's session ID, so the newest session Kiro keeps for the agent's folder is used). Only tool names and commands' first word are read. The line under a room is the agent's terminal title; when that only names the harness or the shell (Kiro's `kiro-cli`), it is Kiro's own session title, or Codex's last prompt, instead. Harnesses whose activity cannot be read make the rounds of every station rather than pretend to know.

## Background sessions

When a Claude Code pane shows its list of background sessions, Herdr reports the pane as idle, or reflects only the highlighted session, even while other sessions keep working. The app also reads each job's `~/.claude/jobs/<id>/state.json` (its live tempo, question and folder only) and gives a Claude Code pane the most urgent state among the sessions started in its folder during the last hour: one waiting for input makes the room *blocked*, one working makes it *working*. Only jobs whose process is still alive count (Claude Code keeps a terminal socket per running job under `/tmp/cc-daemon-<uid>/`), so a job that died while asking something does not keep its room asking for attention.

## Session limits

When Claude Code hits its usage limit it writes the refusal to the session's transcript ("You've hit your session limit · resets 2:20pm (…)", with the exact reset time) and a background job's state says "rate limited — wait and retry". The app reads both and puts the agent in the *exhausted* room, the infirmary: the hero asleep on a cot, the time left to recover (the countdown to the reset) at its foot and in the chat's header, and a violet chip in the HUD that appears only while someone is exhausted. A real question still wins (the room stays *blocked*), and so does an agent that is working again. Anything written to the session after the refusal ends the limit; when the reset time passes the room goes back to Herdr's state, and a background job that was waiting to retry asks for attention. Exhausted rooms sort right after those that need attention.

## Subagents

Herdr does not report subagents, so for Claude Code agents the app reads the transcripts Claude Code writes under `~/.claude/projects/<project>/<session>/subagents/`, matched by the session ID in Herdr's snapshot. A subagent counts as active while its transcript changed in the last 30 seconds. Shells a Claude Code agent left running in the background (Bash with `run_in_background`) bubble as cauldrons in its room's bottom-left corner, whatever its state; hover them for each one's description, running time and last line. In its chat, a strip over the console lists them: ▸ shows the last lines of its output (its output file, or the log its command writes to), **x** stops it after a yes. When one ends on its own, it rings like a finished agent (or like one needing help if it failed), with a notification and a line in the guild log. They are found from the processes (`ps`, a child shell of the agent's `claude`) and the agent's transcript (what each was launched as, how it ended). Active subagents appear as small heroes of the same harness, with a count next to the room's state; each goes to the station of the last tool in its own transcript and walks to a new one when it changes tool. Other harnesses show no subagents.

## Chat

Click a room to open a chat panel for that agent: its terminal as it shows, colours and all (a question keeps its plain lines, with the options to click), a box to send a message or the answer to a question (drop files on the panel to add their paths; type `/` for the harness's commands, with Claude Code's custom commands, skills and plugin skills), and buttons to accept or decline a permission prompt, interrupt the agent (esc), or focus its pane in Herdr. The box grows to three lines as you type (⇧⏎ breaks a line, ⏎ sends) and scrolls past that. A pane narrower than the console has its wrapped lines joined again, so paragraphs fill the console's width. While the agent shows a picker of its own (`/model`, `/resume`…), ↑/↓, ⏎ and esc go to it, even when its empty prompt is still in sight under the list.

The `>_` button, in the chat's header and in a room's right-click menu, opens an integrated terminal in that agent's folder. It runs your login shell independently from the agent, with one session per folder. Closing or hiding the terminal window keeps the session alive until the app quits; use `exit` to end it and **New session** to start again. **External terminal** in its toolbar opens the system terminal instead.

While the agent is asking something with numbered options, ↑ and ↓ (or a click) highlight an option in the panel and put its text in the box; Enter then picks it — moving the agent's own menu to that option — or sends the text when the question is plain text. With nothing typed, Enter picks the option its terminal already highlights.

Right-click a room and choose **End agent** to type `/exit` into its chat after a confirmation. A working or asking agent gets esc first.

Opening a chat while the widget is one room wide widens it to two rooms until the chat closes. The chat takes the keyboard only when it opens, so leaving it open never steals typing from another app.

## Summon an agent

The **+** button (top-right on hover, and in the bar under the rooms) opens a panel under the rooms: pick a harness (claude, codex, gemini, kiro, opencode, cursor, copilot, amp), a folder — one your agents already work in, or any other — and an optional first prompt, then **Summon** (⌘⏎ / Ctrl+Enter). Herdr creates a workspace in that folder (`herdr workspace create`), starts the agent in its shell (`herdr agent start`) and sends the prompt (`herdr agent prompt`). If the agent stops at a startup question (trusting the folder, a login…), its chat opens so you can answer, and the prompt is sent once it is ready. The harness and folder are remembered; ★ next to the folder stars it, and starred folders come first in the list.

## Sounds and notifications

A soft two-note chime plays when an agent needs help and a gentle rising one when it finishes (bell-like tones synthesized in the app). Turn either off, mute everything, or hear them with **Play the sounds**, from the widget or the menu.

When an agent starts needing help the app also posts a desktop notification (Notification Center on macOS, the notification daemon on Linux) with the question when it knows it; a finished agent can notify too. On Linux, clicking the banner brings up the dungeon with that agent's chat open. The **Notifications** menu turns them on or off per moment.

## Activity log

The scroll button unrolls the guild's log on a parchment: agents joining and leaving, state changes, subagents starting and the messages you sent, newest first with the time. It keeps the last 40 lines in memory only.

## Each repo's guild

A room's top band is its repo's: an emblem, the repo's name and its colour, which never change with the agent's state (the state is the room itself, the light and word at the bottom right, and the red frame while it needs you). The emblem is worked out from the repo, with no setup and the same on every launch and machine:

- **What it is made from**: `owner/repo` from the folder's `origin` remote (any host, SSH or HTTPS); without a remote, the repo's or folder's name. Two clones of a repo share their emblem.
- **The charge** (the figure in the middle) comes from a family chosen by the project's files: games (`project.godot`, Unreal, Unity) a sword, heart or crown; mobile (Android, Flutter, Xcode) a phone or bolt; docs (MkDocs, mdBook, Docusaurus, or only Markdown) a book or quill; Ruby (`Gemfile`) a gem or crown; Python a snake or flask; Rust, Go, C/C++, JVM and .NET a gear or anvil; web (`package.json`, Deno, TypeScript) a scroll, bolt or leaf; infrastructure (Docker, Terraform, Helm) a crate or tower. Anything else draws from all nineteen charges.
- **The rest** (colour among twelve, a second tincture among six, partition among six, shield among six, and which charge of the family) comes from a hash of that name (FNV-1a), so related repos still look apart.

To choose for yourself, add `projects` to `settings.json`, keyed by the name the emblem is made from (hover a room's band to see it): `"projects": {"someone/app": {"name": "App", "color": "yellow", "symbol": "quill", "kind": "game"}}`. Every field is optional. Colours: red, orange, amber, yellow, green, forest, cyan, blue, magenta, pink, steel, tan. Charges: gear, anvil, snake, flask, gem, crown, phone, bolt, sword, heart, scroll, leaf, book, quill, crate, tower, star, moon, eye. Kinds: game, mobile, docs, ruby, python, systems, web, infra. Restart the app after editing it.

## Many agents

Resize the widget from its edges or the grip in its bottom-right corner: rooms wrap into as many columns as fit, and the window snaps to whole rooms. It starts at two columns and up to four rows; the size you pick is remembered, the smallest is one room. With fewer agents than columns it narrows (a single agent is one square), and it grows with the agents up to your row count; past that, scroll with the trackpad or wheel — a small badge counts the rooms hidden below or above and jumps to them. An agent that starts needing attention takes the first room and the view scrolls to it.

With three or more agents a bar under the rooms shows one chip per state with its count — click one to show only those rooms (the red **!** chip stays lit while anyone needs you) — and a search box (its own row) that matches project, harness, branch, folder and activity, ignoring case and accents. The chosen chip is remembered.

## Full screen

The ⤢ button (top-right on hover), the menu's **Full screen**, or ⌃⌘F (F11 on Linux) take the dungeon to its own full-screen space (on macOS the real thing: swipe between spaces, like the green button): the agents stack in one column on the left (scroll for more) and keep their places whatever their state. Every room stays whole, whatever its state, at the size picked in the menu's **Room size** (Large, Medium — the default — or Small: each room redrawn shorter, with its furniture where the hero uses it); the one you chose keeps that size and is outlined. Click a room to open its chat. Entering full screen with nothing chosen opens the first agent at work. Hover a room's band for its repo, while **Waiting for you** over them lists who needs you or has finished (click one to open its chat; it leaves the list once you have, until its state changes again), a spare room with **+** to summon an agent, and the state bar under them; all the rest is the console — the chat of the room you click, the summon panel from **+**, or a hint while nothing is chosen — at full height, showing the terminal as it is, colours and spinners included, refreshed twice a second, with the last thousand lines of its history to scroll through; you type on its own prompt line. In full screen the chat's header has worded buttons: **>_ Terminal** (amber) and **± Diff** (cyan); esc closes the chat, and the console's text size stays on ⌘⇧+ / ⌘⇧− and the menu. **± Diff** (lit while open) opens a column on the right with the files changed in the agent's folder, against what you pick over the list: **Uncommitted** (the last commit), **Unpushed** (the branch's upstream: unpushed commits and uncommitted changes) or **Against main** (where the branch left main or master, as in a pull request); untracked files always count: each with its letter (M changed, A added, D deleted, R renamed, U untracked, ! in conflict), its path and the lines added and removed; click one to see its diff under the list. It is re-read every 3 s while open, without taking git's lock. Each repo's emblem is kept in `settings.json` (`emblems`) the first time it is seen, with a colour no other repo has (while there are colours left), so it stays the same across launches even if the repo's files change. Right-click a room's emblem (or **Edit the emblem…** in its menu) to change its name, main and second colours, charge, partition or shield; **Automatic** goes back to the one worked out from the repo. When the agent's branch has a pull request (found with GitHub's `gh`, once a minute), a **PR #N** chip in the chat's header — green open, purple merged, grey closed or draft — opens it in the browser. ⌘-click (Ctrl-click on Linux) a path in the console to show it in Finder (the file manager on Linux), or a URL to open it in the browser.

Under the rooms, full screen shows the guild's HP (usage left), closed to one line by default (each hero with what it has left; click it to open the bars, and the title to close them again; the choice is remembered): each harness's plan left, re-read every 30 s. Claude's HP is the week's general limit and its MP the 5-hour session; Codex's are its week and its 5-hour window, from the rate limits in its session logs; Click Claude (its hero or name) to pick where its numbers come from: the last `/usage` (the default) or live with its token (see Privacy). Kiro's HP is the month's credits against the plan you pick by clicking Kiro (its sessions record the credits per turn). Each bar says when it refills. Esc closes the open chat first, then leaves full screen; so do the button, the menu and the keys. The widget comes back where it was, at its size. `--fullscreen` starts there.

## Text size

⌘+ / ⌘- (Ctrl on Linux) make the panels' text bigger or smaller, ⌘0 resets it; the menu's **Text size** does the same. The size is remembered.

## The status item and its menu

Close or minimize the window to keep monitoring from the menu bar (macOS) or the tray (Linux), where the app's pixel crystal orb marks it; its tooltip counts the agents needing attention. A left click shows or hides the window; the menu has:

- **Show / hide**, **Move back to the corner**, **Always on top** (on by default), **Full screen**, **Open at login** (a LaunchAgent on macOS, an autostart entry on Linux).
- **Console text size**: the agent's terminal at its own size, apart from the panels' text (also **A+** / **A-** in the chat header, or ⌘⇧+ / ⌘⇧- / ⌘⇧0).
- **Sounds** and **Notifications**, each with per-moment switches.
- **Text size**.
- **Herdr session**: Herdr's sessions (`herdr session list`), to switch which one the dungeon watches — agents, log and selection start over — remembered; `HERDR_SESSION` still overrides it at launch. **Demo** shows fictional agents without Herdr; live mode never inserts fictional agents.
- **Open Herdr** and **Open Herdr at startup** (see below).

The widget reopens where you left it, at the size you gave it, and remembers sounds, notifications, the session, the state filter and the text size, in `settings.json` under `~/Library/Application Support/agents-pixel-dungeon` (macOS) or `~/.config/agents-pixel-dungeon` (Linux).

## Languages

Spanish, English, French, Italian and Portuguese, following the system locale (macOS: Language & Region; Linux: `LANG`/`LC_MESSAGES`); other languages fall back to English. `HPD_LANG=en` forces one. Translations live in `src/l10n.rs`.

## Configuration

Monitors the default local session. Environment variables:

| Variable | Effect |
| --- | --- |
| `HERDR_SESSION` | Watch a named Herdr session. |
| `HERDR_BIN` | Path to the `herdr` executable. Discovery otherwise: `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, then `PATH` (GUI launches may have a different `PATH` from your shell). |
| `HPD_LANG` | Force a UI language (`es`, `en`, `fr`, `it`, `pt`). |
| `HPD_REDUCE_MOTION=1` | Stop the animations (macOS Reduce Motion is honoured automatically). |
| `HPD_NO_HERDR=1` | Pretend Herdr is not installed, to try the standalone viewer. |

Command-line flags: `--demo` (fictional agents), `--fullscreen` (start in full screen), `--diagnose` (print what Herdr reports, or the standalone scan), `--self-test`.

## Verify

```sh
./scripts/build.sh
./target/release/agents-pixel-dungeon --self-test
./target/release/agents-pixel-dungeon --diagnose
open 'build/Agents Pixel Dungeon.app' --args --demo     # macOS
./build/agents-pixel-dungeon --demo                      # Linux
```

`cargo test` runs the same checks as `--self-test`: snapshot states, invalid/empty replies, monitor transitions, subagents, tool actions, translations, sound and notification settings, chat selection, question options, filters, connection notes, sessions, Herdr events and error codes, background jobs, the standalone heuristics, git branches, hero mapping and bundled PNG decoding. `--diagnose` prints live agent data; review it before sharing publicly.

## Privacy

The app reads Herdr's snapshot and events, and the process list (`ps`, to find shells agents left in the background; Herdr's `pane process-info` gives each pane's shell). It only acts on a terminal when you use the chat or summon panels: then it reads the agent's recent output (`herdr agent read`) and sends exactly what you typed or clicked (`herdr agent prompt`, `send-keys`, `pane send-text`, `agent focus`, `workspace create`, `agent start`); stopping a background shell from the chat sends it SIGTERM (`pkill -P`, then `kill`). No network listener or telemetry. One opt-in exception, for the HP viewer in full screen: by default Claude's HP comes from the last `/usage` Claude Code cached in `~/.claude.json` and no credential is read; if you pick **Live (token)** in Claude's options (click Claude in the HP viewer), the app reads Claude Code's own OAuth token (the macOS keychain item "Claude Code-credentials", or `~/.claude/.credentials.json`) and asks Anthropic for that usage (`api.anthropic.com/api/oauth/usage`, what Claude Code's `/usage` shows) at most every two minutes, with the token passed to `curl` on stdin; the token is not stored or sent anywhere else, and without it the app falls back to that cache. macOS may ask once for permission to read that keychain item. It reads the state and folder of Claude Code background jobs from `~/.claude/jobs/*/state.json`, and from Claude Code's, Codex's and Kiro's local transcripts: modification times, message types and the name of the last tool called (never tool inputs or outputs), plus — for a question or the standalone viewer — the text of the agent's last message, and the title Kiro gives a session or Codex's last prompt when the terminal title says nothing, which are only displayed. Agent data remains in memory. External links open only when clicked. Demo data contains no live user sessions.

## Without Herdr (optional)

Herdr is what lets the dungeon talk to the agents: it owns their terminals, reports each pane's state and types into it. Without it (or while its server is down) the app works as a **read-only viewer**: it finds the agents running in any terminal by their processes (the harnesses Herdr knows) and infers their state from the files they write — Claude Code and Codex transcripts give working, waiting, and a tool call waiting for permission; the last prompt becomes the room's activity line. The chat shows the agent's last reply but cannot send anything, and there is no summoning.

A banner over the rooms offers **Install Herdr** (its official installer, `curl -fsSL https://herdr.dev/install.sh | sh`), **Open Herdr** (a terminal running `herdr`, which leaves its server up) or **Carry on without Herdr**; the menu's **Open Herdr at startup** does the opening for you. Agents appear in the full dungeon only when they run inside Herdr.

## Support

Agents Pixel Dungeon is made by [Diabolo Studio](https://diabolostudio.com/). If the dungeon is useful to you, you can [sponsor the project on GitHub](https://github.com/sponsors/JoseValencia125). Sponsorships help pay for the time spent drawing new heroes and rooms and keeping up with Herdr releases.
