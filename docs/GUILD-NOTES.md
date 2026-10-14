# Guild implementation notes

One entry per milestone: what changed, what was decided, and how it was checked. Newest last.

## M0 — Hardened core (2026-10-14)

- **State and events commit together.** `Store` opens nested units of work (`BEGIN IMMEDIATE`, then savepoints), and `Service::atomic` wraps every mutation that writes state and appends events: task, attempt and quest transitions, dispatch, candidate commit, candidate selection, integration, permission answers, creation of quests, tasks and members. Events appended inside a unit are held and sent to subscribers only after the outer commit; a failed unit rolls back both and sends nothing. Test: `a_failed_unit_of_work_leaves_neither_state_nor_event` (also nested units).
- **The user's checkout is never written.** The game design is no longer exported into the working tree: `design.export` commits `game-design/` onto the managed branch `refs/heads/guild/design/<project>` from a temporary detached checkout (`workspaces::publish`, compare-and-swap on the ref) and `project.add` reads the design back from that branch (`workspaces::read_ref`) before looking at the checkout. The user merges the branch when they want it.
- **Open harness ids.** `HarnessKind` became `HarnessId`, a validated slug; the registry maps ids to adapters and the core never names a harness. Adding one is a new adapter file and a `register` call.
- **Failure and recovery invariants.** `Attempt.generation` increases on every (re)drive and a driver's result is ignored when the attempt moved on (`attempt.stale_result`). Reconcile finishes an integration the service died in (idempotent: the squash finds nothing new), puts a submitted candidate's task up for a decision, and settles a race whose runners were lost. Tests cover dying while provisioning, waiting for a permission, verifying and integrating; a race losing one, two or all runners; a stale result; and three restarts in a row.
- Found on the way: two reentrant store locks (a `MutexGuard` kept alive by an `if let` scrutinee) in the stale-result and shutdown paths.
