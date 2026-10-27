# Agent notes

## Clean up worktrees when their PR closes

Every worktree builds its own `target/` (1–4 GB each), and cargo never deletes old artifacts. When the PR of a worktree is merged or closed, clean it up in the same step:

- **Merged or closed, clean tree, no commits outside `origin/main`** (`git status --short` and `git log origin/main..<branch>` both empty): `git worktree remove <path>` and `git branch -d <branch>`.
- **Uncommitted changes or unmerged commits**: keep the worktree and run `cargo clean` inside it.

Worktrees keep a `target/` each rather than sharing one through `CARGO_TARGET_DIR`: agents build in parallel, and a shared target dir would make them wait on cargo's lock and overwrite each other's binaries.
