# Guild implementation notes

One entry per milestone: what changed, what was decided, and how it was checked. Newest last.

## M0 — Hardened core (2026-10-14)

- **State and events commit together.** `Store` opens nested units of work (`BEGIN IMMEDIATE`, then savepoints), and `Service::atomic` wraps every mutation that writes state and appends events: task, attempt and quest transitions, dispatch, candidate commit, candidate selection, integration, permission answers, creation of quests, tasks and members. Events appended inside a unit are held and sent to subscribers only after the outer commit; a failed unit rolls back both and sends nothing. Test: `a_failed_unit_of_work_leaves_neither_state_nor_event` (also nested units).
- **The user's checkout is never written.** The game design is no longer exported into the working tree: `design.export` commits `game-design/` onto the managed branch `refs/heads/guild/design/<project>` from a temporary detached checkout (`workspaces::publish`, compare-and-swap on the ref) and `project.add` reads the design back from that branch (`workspaces::read_ref`) before looking at the checkout. The user merges the branch when they want it.
- **Open harness ids.** `HarnessKind` became `HarnessId`, a validated slug; the registry maps ids to adapters and the core never names a harness. Adding one is a new adapter file and a `register` call.
- **Failure and recovery invariants.** `Attempt.generation` increases on every (re)drive and a driver's result is ignored when the attempt moved on (`attempt.stale_result`). Reconcile finishes an integration the service died in (idempotent: the squash finds nothing new), puts a submitted candidate's task up for a decision, and settles a race whose runners were lost. Tests cover dying while provisioning, waiting for a permission, verifying and integrating; a race losing one, two or all runners; a stale result; and three restarts in a row.
- Found on the way: two reentrant store locks (a `MutexGuard` kept alive by an `if let` scrutinee) in the stale-result and shutdown paths.

## M1 + M2 — Execution strategies, an independent judge, durable decisions (2026-10-14)

- **`ExecutionStrategy` on every task**: `{type: single | reviewed | candidate_race | experiment, candidates, selection: user | policy | judge, auto_integrate}`. `candidates` and `auto_integrate` on the task are derived from it; the older flags still map (`--candidates N` is a policy-selected race). `reviewed` is one attempt a judge looks at.
- **Hard gates before any choice.** Only candidates whose required verifiers passed (`passes_gates`) can be selected, by anyone. Then the selection: the user on the board, the policy (smallest passing change, then the earliest), or a judge.
- **The judge is a role on any harness.** A judge attempt (`AttemptKind::Judge`) goes to a free member with the `judge` role (then `reviewer`), preferably not an author of the candidates. Its prompt carries the task, the acceptance criteria, the design summary and each passing candidate's diff under an anonymous label (A, B, C in an order derived from a hash of the attempt id), never the authors. It must answer a fenced JSON block with scores on the rubric (correctness, acceptance coverage, simplicity, maintainability, architecture, unnecessary complexity, design alignment), findings, trade-offs, winner, confidence and reason; the winner is a judgement, not a sum.
- **`RaceDecision` is durable** for every selection, by the judge, the policy or the user (with `human_override` when the user chose against an earlier decision), with the anonymous candidates, scores, findings, winner, confidence and reason; also written as a `race.decision` artifact of the judge attempt. Events: `judge.started`, `judge.finished`, `race.decided`. CLI: `task new --strategy race --candidates 3 --select judge`, `decisions <task>`.
- **Fallbacks keep the guild moving**: an unusable or lost judge (no JSON, unknown label, failure, restart) falls back to the policy and the decision says why; a task never gets two decisions.
- Found on the way: integration picked "the latest passed attempt", which after a judge was the judge itself; selection and integration now consider work attempts only.

## M3 — A row per attempt (2026-10-14)

- **`AttemptRecord`**, written in the same unit of work as `attempt.finished`: task, role, strategy, candidates, selection, attempt kind, member, harness, model, result, attempt number, duration, turns, resumes, files and lines changed, verifier counts, judge score, winner or loser, who decided, human override, failure. A later decision updates the rows of attempts that had already finished. Usage (tokens, cost) has a slot and is empty until an adapter reports it.
- Read with `records.list {projectId?, taskId?}` or `guild records`. No learning yet: the point is honest rows for the routing policy to come.
- Real check: two Kiro workers raced on a small Python change, a Codex judge that had written neither scored them 32 and 35 on the rubric, named B with confidence 1.0, and B was integrated alone; `guild decisions t1` shows it.
