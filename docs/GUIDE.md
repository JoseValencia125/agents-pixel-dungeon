# Herdr Pixel Dungeon — guide

Everything the README leaves out: how the dungeon reads the agents, what each panel does, the menu, configuration, privacy and the standalone mode.

Herdr Pixel Dungeon is an independent project, not affiliated with or endorsed by Herdr, Inc.; "Herdr" names the runtime the app connects to.

## How the rooms are kept current

The app keeps one connection to Herdr's socket subscribed to its events (`events.subscribe`: each agent pane's status, panes appearing, changing or closing, workspaces renamed or closed), so a change shows up the moment it happens. `herdr api snapshot` runs only to bootstrap, after a structural change, once a minute to reconcile, and — if the event connection drops — every second while it reconnects. Local enrichment (git branch, background jobs, subagents, tools) is read from files each second without starting processes.

When Herdr is unreachable or its data stops updating, a banner over the rooms says so with the time since the last good update; the last known rooms stay on screen, faded, while the app keeps reconnecting.

Activity means **the title reported by the terminal**, not an inferred summary. Animation represents state, not tool calls or completion percentage. Agents outside the selected Herdr session are not included, except those of the SSH machines saved in Herdr (next section).

## Several machines

Herdr can keep SSH machines next to the local session ([Connecting machines](https://herdr.dev/docs/connecting-machines/): `herdr machine add workbox`). The dungeon reads the list Herdr saved (`herdr machine list --json`, every 30 s, a local file) and shows the agents of every machine enabled there beside the local ones, in the same rooms, sorted together (whoever needs you first). There is nothing to configure in the app and no SSH of its own: every call goes through `herdr --machine <profile id> …`, which uses the machine's saved target, session and Herdr's shared SSH connection.

- **Each room says where it is**: `@machine · project` under it, and in the chat's header. A remote agent's id in the dungeon includes the machine, so the same pane (`w1:p1`) on two machines is two rooms, and whatever you do in a room goes to that agent's own machine and pane; never to a local one with the same id.
- **Kept current by events**: Herdr streams events only from the local socket, so for each reachable machine the app keeps one `ssh <target>` open (the target Herdr saved, with your SSH configuration, `BatchMode=yes`) running a few lines of `python3` that pass requests to that machine's Herdr socket: the event subscription for its agents' status, and the chat panel's terminal reads (`agent.read`, a round trip of a few dozen milliseconds instead of a new `herdr --machine` each time; the console then reads four times a second). Each event reads the machine again with `herdr --machine <id> api snapshot` at once; while the stream is open snapshots also run every 20 seconds to reconcile. A machine without `python3`, or whose stream drops, is read every 2 seconds instead and the stream is retried with a back-off (up to every 2 minutes). One snapshot at a time per machine, off the UI thread.
- **When a machine cannot be reached** (SSH down, authentication needed, Herdr not running there), a banner says which, with the time since its last good snapshot; its rooms stay as last seen, faded and read-only, and the other machines and the local rooms carry on. It is tried again after 5, 10, 20, 40, then every 60 seconds. When it answers again, its rooms are replaced by the new snapshot as a whole (no duplicates, no leftovers) and the log says it reconnected. Herdr never asks for a password from the app: if a machine needs authentication, run `herdr machine reconnect <label>` in a terminal.
- **What a remote room cannot show**: the git branch, background jobs, subagents, tool stations and session limits come from local files (Claude Code's, Codex's and Kiro's logs, `.git`), which are on the other machine; Herdr's API does not report them. A remote room shows Herdr's state and the terminal's title, its hero makes the rounds of the stations, and nothing local is ever read for it.
- **Chat**: the terminal, replies, ↑/↓ options, Accept / Decline / Stop, focus and *End agent* all work as for a local agent. The `>_` terminal button, dropping files and ⌘-clicking paths are local-only (links to URLs still open); the `/` menu lists the harness's own commands only.
- **Summon** on a machine by picking it in the panel's `@` list: type an absolute folder on that machine (`~` is not expanded there), or pick one its agents use. All harnesses are offered, since the app cannot see what is installed there; Herdr says if one is missing.
- **Filter**: the `@` button in the bar under the rooms picks a machine (all, this computer, or one) and a project. The machine is remembered.
- **Hide a machine** without touching Herdr from the menu's **Machines**; machines disabled in Herdr are shown greyed out and are never contacted.

## What a working agent does

For Claude Code agents the hero goes to the station that matches the last tool the agent called: it reads a book for `Read`, `Grep`, `Glob`, `WebSearch` and `WebFetch`, forges at the anvil for `Edit` and `Write`, brews potions for `Bash`, summons in a magic circle for `Task` (subagents), studies a scroll for todo lists and plans, and types at the laptop while thinking or writing.

Codex and Kiro agents do the same from their own session logs, matched by the session ID Herdr reports: Codex's rollout (`~/.codex/sessions/…/rollout-…-<id>.jsonl`; a shell command that only looks at files counts as reading, `apply_patch` as forging, `update_plan` as planning) and Kiro's session (`~/.kiro/sessions/cli/<id>.jsonl`). Only tool names and commands' first word are read. Harnesses whose activity cannot be read make the rounds of every station rather than pretend to know.

## Background sessions

When a Claude Code pane shows its list of background sessions, Herdr reports the pane as idle, or reflects only the highlighted session, even while other sessions keep working. The app also reads each job's `~/.claude/jobs/<id>/state.json` (its live tempo, question and folder only) and gives a Claude Code pane the most urgent state among the sessions started in its folder during the last hour: one waiting for input makes the room *blocked*, one working makes it *working*. Only jobs whose process is still alive count (Claude Code keeps a terminal socket per running job under `/tmp/cc-daemon-<uid>/`), so a job that died while asking something does not keep its room asking for attention.

## Session limits

When Claude Code hits its usage limit it writes the refusal to the session's transcript ("You've hit your session limit · resets 2:20pm (…)", with the exact reset time) and a background job's state says "rate limited — wait and retry". The app reads both and puts the agent in the *exhausted* room, the infirmary: the hero asleep on a cot, the time left to recover (the countdown to the reset) at its foot and in the chat's header, and a violet chip in the HUD that appears only while someone is exhausted. A real question still wins (the room stays *blocked*), and so does an agent that is working again. Anything written to the session after the refusal ends the limit; when the reset time passes the room goes back to Herdr's state, and a background job that was waiting to retry asks for attention. Exhausted rooms sort right after those that need attention.

## Subagents

Herdr does not report subagents, so for Claude Code agents the app reads the transcripts Claude Code writes under `~/.claude/projects/<project>/<session>/subagents/`, matched by the session ID in Herdr's snapshot. A subagent counts as active while its transcript changed in the last 30 seconds. Active subagents appear as small heroes of the same harness, with a count in the room's title; each goes to the station of the last tool in its own transcript and walks to a new one when it changes tool. Other harnesses show no subagents.

## Chat

Click a room to open a chat panel for that agent: its terminal as it shows, colours and all (a question keeps its plain lines, with the options to click), a box to send a message or the answer to a question (drop files on the panel to add their paths; type `/` for the harness's commands, with Claude Code's custom commands, skills and plugin skills), and buttons to accept or decline a permission prompt, interrupt the agent (esc), or focus its pane in Herdr. The box grows to three lines as you type (⇧⏎ breaks a line, ⏎ sends) and scrolls past that. A pane narrower than the console has its wrapped lines joined again, so paragraphs fill the console's width. While the agent shows a picker of its own (`/model`, `/resume`…), ↑/↓, ⏎ and esc go to it, even when its empty prompt is still in sight under the list.

The `>_` button, in the chat's header and in a room's right-click menu, opens a terminal window to type a command in, in that agent's folder (Terminal on macOS, the first terminal emulator found on Linux).

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

## Many agents

Resize the widget from its edges or the grip in its bottom-right corner: rooms wrap into as many columns as fit, and the window snaps to whole rooms. It starts at two columns and up to four rows; the size you pick is remembered, the smallest is one room. With fewer agents than columns it narrows (a single agent is one square), and it grows with the agents up to your row count; past that, scroll with the trackpad or wheel — a small badge counts the rooms hidden below or above and jumps to them. An agent that starts needing attention takes the first room and the view scrolls to it.

With three or more agents (or any machine shown) a bar under the rooms shows one chip per state with its count — click one to show only those rooms (the red **!** chip stays lit while anyone needs you) — the `@` scope menu while machines are shown (see [Several machines](#several-machines)), and a search box (its own row) that matches project, harness, branch, folder, machine and activity, ignoring case and accents. The chosen chip is remembered.

## Full screen

The ⤢ button (top-right on hover), the menu's **Full screen**, or ⌃⌘F (F11 on Linux) take the dungeon to its own full-screen space (on macOS the real thing: swipe between spaces, like the green button): the rooms stack in one column on the left (scroll for more), a spare room with **+** to summon an agent, and the state bar under them; all the rest is the console — the chat of the room you click, the summon panel from **+**, or a hint while nothing is chosen — at full height, showing the terminal as it is, colours and spinners included, refreshed twice a second, with the last few hundred lines of its history to scroll through; you type on its own prompt line. ⌘-click (Ctrl-click on Linux) a path in the console to show it in Finder (the file manager on Linux), or a URL to open it in the browser.

Under the rooms, full screen shows the guild's HP (usage left): each harness's plan left, re-read every 30 s. Claude's HP is the week's general limit and its MP the 5-hour session; Codex's are its week and its 5-hour window, from the rate limits in its session logs; Click Claude (its hero or name) to pick where its numbers come from: the last `/usage` (the default) or live with its token (see Privacy). Kiro's HP is the month's credits against the plan you pick by clicking Kiro (its sessions record the credits per turn). Each bar says when it refills. Esc closes the open chat first, then leaves full screen; so do the button, the menu and the keys. The widget comes back where it was, at its size. `--fullscreen` starts there.

## Text size

⌘+ / ⌘- (Ctrl on Linux) make the panels' text bigger or smaller, ⌘0 resets it; the menu's **Text size** does the same. The size is remembered.

## The status item and its menu

Close or minimize the window to keep monitoring from the menu bar (macOS) or the tray (Linux), where a pixel knight's helm marks the app; its tooltip counts the agents needing attention. A left click shows or hides the window; the menu has:

- **Show / hide**, **Move back to the corner**, **Always on top** (on by default), **Full screen**, **Open at login** (a LaunchAgent on macOS, an autostart entry on Linux).
- **Console text size**: the agent's terminal at its own size, apart from the panels' text (also **A+** / **A-** in the chat header, or ⌘⇧+ / ⌘⇧- / ⌘⇧0).
- **Sounds** and **Notifications**, each with per-moment switches.
- **Text size**.
- **Herdr session**: Herdr's sessions (`herdr session list`), to switch which one the dungeon watches — agents, log and selection start over — remembered; `HERDR_SESSION` still overrides it at launch. **Demo** shows fictional agents without Herdr; live mode never inserts fictional agents.
- **Machines**: the SSH machines saved in Herdr, to show or hide each one in the dungeon (marked when unreachable or disabled in Herdr).
- **Open Herdr** and **Open Herdr at startup** (see below).

The widget reopens where you left it, at the size you gave it, and remembers sounds, notifications, the session, the hidden machines, the state and machine filters and the text size, in `settings.json` under `~/Library/Application Support/herdr-pixel-dungeon` (macOS) or `~/.config/herdr-pixel-dungeon` (Linux).

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
./target/release/herdr-pixel-dungeon --self-test
./target/release/herdr-pixel-dungeon --diagnose
open 'build/Herdr Pixel Dungeon.app' --args --demo     # macOS
./build/herdr-pixel-dungeon --demo                      # Linux
```

`cargo test` runs the same checks as `--self-test`: snapshot states, local and remote agents together (a fake `herdr` checks that every action reaches the right machine and pane, unreachable machines and reconnections), invalid/empty replies, monitor transitions, subagents, tool actions, translations, sound and notification settings, chat selection, question options, filters, connection notes, sessions, Herdr events and error codes, background jobs, the standalone heuristics, git branches, hero mapping and bundled PNG decoding. `--diagnose` prints live agent data, local and per saved machine; review it before sharing publicly.

## Privacy

The app reads Herdr's snapshot and events, and the snapshots of the SSH machines saved in Herdr through Herdr's own `--machine`. For each reachable machine it also runs `ssh <target>` itself (the target Herdr saved; `ssh` uses your own SSH configuration and agent, never a password prompt) to hold that machine's event stream open and read the open chat's terminal through it, running a short `python3` bridge to its Herdr socket there; the app reads no SSH keys itself. It only acts on a terminal when you use the chat or summon panels: then it reads the agent's recent output (`herdr agent read`) and sends exactly what you typed or clicked (`herdr agent prompt`, `send-keys`, `pane send-text`, `agent focus`, `workspace create`, `agent start`). No network listener or telemetry. One opt-in exception, for the HP viewer in full screen: by default Claude's HP comes from the last `/usage` Claude Code cached in `~/.claude.json` and no credential is read; if you pick **Live (token)** in Claude's options (click Claude in the HP viewer), the app reads Claude Code's own OAuth token (the macOS keychain item "Claude Code-credentials", or `~/.claude/.credentials.json`) and asks Anthropic for that usage (`api.anthropic.com/api/oauth/usage`, what Claude Code's `/usage` shows) at most every two minutes, with the token passed to `curl` on stdin; the token is not stored or sent anywhere else, and without it the app falls back to that cache. macOS may ask once for permission to read that keychain item. It reads the state and folder of Claude Code background jobs from `~/.claude/jobs/*/state.json`, and from Claude Code's, Codex's and Kiro's local transcripts: modification times, message types and the name of the last tool called (never tool inputs or outputs), plus — for a question or the standalone viewer — the text of the agent's last message, which is only displayed. Agent data remains in memory. External links open only when clicked. Demo data contains no live user sessions.

## Without Herdr (optional)

Herdr is what lets the dungeon talk to the agents: it owns their terminals, reports each pane's state and types into it. Without it (or while its server is down) the app works as a **read-only viewer**: it finds the agents running in any terminal by their processes (the harnesses Herdr knows) and infers their state from the files they write — Claude Code and Codex transcripts give working, waiting, and a tool call waiting for permission; the last prompt becomes the room's activity line. The chat shows the agent's last reply but cannot send anything, and there is no summoning.

A banner over the rooms offers **Install Herdr** (its official installer, `curl -fsSL https://herdr.dev/install.sh | sh`), **Open Herdr** (a terminal running `herdr`, which leaves its server up) or **Carry on without Herdr**; the menu's **Open Herdr at startup** does the opening for you. Agents appear in the full dungeon only when they run inside Herdr.

## Support

If the dungeon is useful to you, you can [sponsor the project on GitHub](https://github.com/sponsors/JoseValencia125). Sponsorships help pay for the time spent drawing new heroes and rooms and keeping up with Herdr releases.
