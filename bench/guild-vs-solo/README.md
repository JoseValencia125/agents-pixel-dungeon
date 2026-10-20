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

## Pilot results (2026-10-20)

3 tasks × 2 replicas × 3 arms, all from commit `5d46838`; every run passed
the verifier. Judge: Opus 5.5, two shuffled orders per replica (12 verdicts
per arm).

| Tarea | Rép. | Brazo | Verifica | Min | USD | Kiro | Archivos | +/− | Puntaje juez (0–5) | Puesto medio |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| t1-confirmar | r1 | solo | sí | 10.3 | 1.53 | 0 | 4 | +342 −31 | 3.79 | 3.00 |
| t1-confirmar | r1 | review | sí | 13.9 | 4.06 | 0 | 4 | +371 −31 | 4.29 | 1.00 |
| t1-confirmar | r1 | guild | sí | 11.3 | 2.00 | 15.4 | 6 | +568 −21 | 3.86 | 2.00 |
| t1-confirmar | r2 | solo | sí | 9.4 | 1.47 | 0 | 5 | +286 −28 | 3.64 | 3.00 |
| t1-confirmar | r2 | review | sí | 13.9 | 4.42 | 0 | 5 | +320 −28 | 4.57 | 1.00 |
| t1-confirmar | r2 | guild | sí | 14.2 | 2.60 | 13.3 | 7 | +529 −11 | 3.93 | 2.00 |
| t2-registro | r1 | solo | sí | 4.1 | 1.18 | 0 | 5 | +299 −15 | 3.71 | 3.00 |
| t2-registro | r1 | review | sí | 6.6 | 3.17 | 0 | 5 | +323 −15 | 3.93 | 2.00 |
| t2-registro | r1 | guild | sí | 7.1 | 1.71 | 11.7 | 8 | +610 −15 | 4.21 | 1.00 |
| t2-registro | r2 | solo | sí | 4.3 | 1.36 | 0 | 5 | +325 −30 | 4.14 | 1.50 |
| t2-registro | r2 | review | sí | 7.2 | 3.59 | 0 | 9 | +367 −42 | 3.71 | 1.50 |
| t2-registro | r2 | guild | sí | 10.2 | 2.04 | 17.1 | 10 | +677 −15 | 3.36 | 3.00 |
| t3-etapa4 | r1 | solo | sí | 8.4 | 2.45 | 0 | 7 | +174 −38 | 3.36 | 3.00 |
| t3-etapa4 | r1 | review | sí | 13.3 | 6.44 | 0 | 7 | +201 −38 | 4.00 | 1.50 |
| t3-etapa4 | r1 | guild | sí | 23.1 | 3.75 | 22.6 | 10 | +547 −42 | 3.71 | 1.50 |
| t3-etapa4 | r2 | solo | sí | 14.3 | 2.93 | 0 | 8 | +185 −40 | 3.43 | 2.00 |
| t3-etapa4 | r2 | review | sí | 21.9 | 7.80 | 0 | 8 | +226 −40 | 3.36 | 3.00 |
| t3-etapa4 | r2 | guild | sí | 17.8 | 4.56 | 28.7 | 11 | +540 −30 | 4.07 | 1.00 |

| Brazo | Corridas | Verifica | Min medio | USD medio | Kiro medio | Puntaje medio | Puesto medio | 1.er puesto (veredictos) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| solo | 6 | 6/6 | 8.5 | 1.82 | 0.0 | 3.68 | 2.58 | 1/12 |
| review | 6 | 6/6 | 12.8 | 4.91 | 0.0 | 3.98 | 1.67 | 6/12 |
| guild | 6 | 6/6 | 14.0 | 2.78 | 18.1 | 3.86 | 1.75 | 5/12 |

Head to head, over the 12 verdicts: **guild beats solo 10/12, review beats
solo 9/12, guild beats review 5/12.**

What it says:

- **One Claude session alone is the weakest arm**: first in 1 of 12
  verdicts. Both review and the guild find what solo misses (t1: a
  confirmation label overflowing its button; t2: final values recorded after
  the faction's penalty was applied).
- **Review and the guild tie on quality** (mean rank 1.67 and 1.75) but not
  on what they cost: review costs 2.7× solo in Claude USD and is the most
  stable (never last except t3 r2); the guild costs 1.5× solo in Claude USD
  plus 18 Kiro credits per task, and swings between first and last.
- **The guild's losses come from seams**: in t2 r2 the orchestrator accepted
  a split whose parts did not fit (a value read after the penalty, an untyped
  variable left "until the merge", a change outside the task). Its wins come
  from more tests and edge cases (it writes 2–3× the lines).
- **Time**: solo 8.5 min, review 12.8, guild 14.0 on average; the guild is
  slowest on the large task (18–23 min).

Limits: two replicas per task; a Claude judge reading Claude's code (the
human blind play is pending); Kiro credits are not converted to USD. One run
(t3 r2 solo) was discarded and repeated because the bench's own files left
the disk mid-run; the bench now runs from its own worktree.

Next arms worth running: the guild with one member and the orchestrator's
review, without splitting the task (separates review from division of
work); and captures of the running game for the judge.

## Earlier attempt

On 2026-10-19, one task on Verbrawl (wildcard and digraph letters), one run
per arm: solo won against the first orchestrator prompt and lost against the
second (`prompts/orchestrator.md` comes from that one), whose Claude cost was
$7.04 against $3.68 (Kiro credits not counted then). One run per arm cannot
separate the arms from noise; this bench repeats each task.
