# Guild vs solo Claude

Does the guild make better changes than one Claude Code session, and at what
cost? This bench answers it on a real game, HARNESS: Directive (Godot 4.7),
with tasks from its own backlog and its living design (`game-design/`).

## Arms

| Arm | What runs |
| --- | --- |
| `solo` | One Claude Code session (Opus 5.5) does the whole task. |
| `review` | The `solo` result, reviewed by a second read-only session, then fixed by the solo session itself (resumed). Separates what review adds from what the guild's division of work adds. |
| `guild` | A Claude orchestrator (Opus 5.5) plans, reviews and decides over the guild's MCP tools; Orin (Kiro, Opus 5.5) and Lira (Kiro, Sonnet 5.5) build in their worktrees, and each candidate passes the project's verifier. |

Every arm starts from the same commit of a dedicated clone
(`~/guild-bench/harness-directive`, no remote; registered in the guild as
`hd-bench`), so work on the real repository never moves the base. Every arm
reads the same living design: the solo sessions from `game-design/`, the
guild members from the end of their prompt.

## Tasks

`tasks/` holds three tasks of the game's backlog, small to large:
confirming large Cómputo spending (`t1`), a final record without a moral
verdict (`t2`, design question Q-004) and learning stage 4, Production
(`t3`).

## Measures

- **Verifies**: `verify.sh` runs the project's headless Godot tests plus every
  `tools/test_*.gd` the change added, the same way for every arm.
- **Blind judge**: a fourth session (Opus 5.5, read-only) sees the three diffs
  as P, Q, R in two shuffled orders and scores correctness, spec coverage,
  test quality, simplicity, fit with the codebase, design alignment and UX
  (0–5), with a ranking.
- **Cost and time**: Claude USD per session (`total_cost_usd`), Kiro credits
  of the guild members (`kiro_credits.py`, from Kiro's own session files in
  the run's time window), wall minutes, turns, diff size, review points,
  orchestrator rejections and permissions answered.
- **The human play test** (pending, by hand): play each build without knowing
  its arm.

## Running

```
python3 bench.py all --reps 2          # every task × replica × arm, then judges and the table
python3 bench.py run t1-confirmar 1 solo   # one arm (solo also produces review)
python3 bench.py judge t1-confirmar 1
python3 bench.py report
```

Runs are sequential (the arm that goes first alternates by task and
replica), so the arms do not compete for rate limits and their times are
comparable. Results, transcripts and diffs go to `~/guild-bench/results`;
worktrees to `~/guild-bench/work`.

## Earlier attempt

On 2026-10-19, one task on Verbrawl (wildcard and digraph letters), one run
per arm: solo won against the first orchestrator prompt and lost against the
second (`prompts/orchestrator.md` comes from that one), whose Claude cost was
$7.04 against $3.68 (Kiro credits not counted then). One run per arm cannot
separate the arms from noise; this bench repeats each task.
