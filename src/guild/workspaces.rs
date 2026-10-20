//! Git worktrees for attempts, outside the user's repository: one branch
//! and one checkout per writer, created from a known commit, turned into a
//! candidate commit when the agent stops, and removed only when nothing
//! owns them. Never touches the user's main checkout.

use crate::domain::*;
use std::path::{Path, PathBuf};

pub type GitResult<T> = Result<T, String>;

/// Worktrees share one object store. On Windows, two of them writing the
/// same loose object at once (racing candidates that produce the same blob)
/// lose with "Permission denied" while the other holds the file; nothing was
/// written to a ref, so the command is retried.
pub fn git(repo: &Path, args: &[&str]) -> GitResult<String> {
    let mut tries = 0;
    loop {
        let output = crate::platform::command("git").arg("-C").arg(repo).args(args).output().map_err(|e| format!("git: {e}"))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        tries += 1;
        if tries < 5 && stderr.contains("Permission denied") && stderr.contains("objects") {
            std::thread::sleep(std::time::Duration::from_millis(50 * tries));
            continue;
        }
        return Err(format!("git {} failed: {}", args.join(" "), stderr.trim()));
    }
}

pub fn is_repo(path: &Path) -> bool { git(path, &["rev-parse", "--is-inside-work-tree"]).map(|s| s == "true").unwrap_or(false) }

pub fn rev_parse(repo: &Path, rev: &str) -> GitResult<String> { git(repo, &["rev-parse", "--verify", &format!("{rev}^{{commit}}")]) }

pub fn ref_exists(repo: &Path, name: &str) -> bool { git(repo, &["show-ref", "--verify", "--quiet", name]).is_ok() }

pub fn current_branch(repo: &Path) -> Option<String> { git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).ok() }

/// Where an attempt's worktree lives.
pub fn worktree_path(project_id: &str, attempt_id: &str) -> PathBuf { crate::paths::worktrees_dir().join(project_id).join(attempt_id) }

pub fn branch_name(quest_id: &str, task_id: &str, attempt_id: &str) -> String {
    format!("guild/{}/{}/{}", short(quest_id), short(task_id), short(attempt_id))
}

fn short(id: &str) -> &str { if id.len() > 8 { &id[id.len() - 8..] } else { id } }

/// Create the worktree for an attempt from `base_sha`, on a fresh branch.
/// Adopts an existing checkout at the same path when it already points at
/// that branch (a crash between creating it and recording it).
pub fn provision(project: &Project, quest_id: &str, task_id: &str, attempt_id: &str, base_sha: &str) -> GitResult<Workspace> {
    let repo = Path::new(&project.repo_path);
    if !is_repo(repo) { return Err(format!("{} is not a git repository", project.repo_path)); }
    let path = worktree_path(&project.id, attempt_id);
    let branch = branch_name(quest_id, task_id, attempt_id);
    if path.exists() {
        match current_branch(&path) {
            Some(b) if b == branch => {}
            _ => return Err(format!("{} exists and is not the attempt's worktree", path.display())),
        }
    } else {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        let path_s = path.display().to_string();
        if ref_exists(repo, &format!("refs/heads/{branch}")) {
            git(repo, &["worktree", "add", &path_s, &branch])?;
        } else {
            git(repo, &["worktree", "add", "-b", &branch, &path_s, base_sha])?;
        }
    }
    let now = now();
    Ok(Workspace {
        id: new_id(),
        project_id: project.id.clone(),
        attempt_id: attempt_id.into(),
        path: path.display().to_string(),
        branch,
        base_sha: base_sha.into(),
        state: WorkspaceState::Ready,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Bring an earlier candidate's changes into a fresh worktree, uncommitted,
/// on top of its base (which may have moved on since: other tasks were
/// integrated), so a retry goes on from it. Git merges in three ways; on a
/// conflict the worktree is put back as it was and false is returned.
pub fn carry(workspace: &Workspace, candidate: &Candidate) -> bool {
    let path = Path::new(&workspace.path);
    let range = format!("{}..{}", candidate.base_sha, candidate.head_sha);
    if git(path, &["cherry-pick", "--no-commit", &range]).is_ok() { return true; }
    let _ = git(path, &["cherry-pick", "--abort"]);
    let _ = git(path, &["reset", "--hard", &workspace.base_sha]);
    let _ = git(path, &["clean", "-fdq"]);
    false
}

/// Run the project's setup command in the worktree, returning its output.
pub fn setup(workspace: &Workspace, argv: &[String], timeout_seconds: u64) -> GitResult<String> {
    if argv.is_empty() { return Ok(String::new()); }
    let output = run_with_timeout(Path::new(&workspace.path), argv, timeout_seconds)?;
    if output.0 != 0 { return Err(format!("setup exited with {}: {}", output.0, output.1.chars().take(2000).collect::<String>())); }
    Ok(output.1)
}

/// (exit code, combined output)
pub fn run_with_timeout(cwd: &Path, argv: &[String], timeout_seconds: u64) -> GitResult<(i32, String)> {
    use std::io::Read;
    use std::time::{Duration, Instant};
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let mut child = crate::platform::command(program)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? { break status; }
        if started.elapsed() > Duration::from_secs(timeout_seconds) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program} timed out after {timeout_seconds} s"));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut combined = out_thread.join().unwrap_or_default();
    let err = err_thread.join().unwrap_or_default();
    if !err.is_empty() {
        combined.push_str("\n--- stderr ---\n");
        combined.push_str(&err);
    }
    Ok((status.code().unwrap_or(-1), combined))
}

/// Commit whatever the attempt left in its worktree (tracked and untracked,
/// honouring .gitignore). `None` when the tree is unchanged.
pub fn commit_candidate(workspace: &Workspace, message: &str) -> GitResult<Option<Candidate>> {
    let path = Path::new(&workspace.path);
    git(path, &["add", "-A"])?;
    let staged = git(path, &["diff", "--cached", "--quiet"]);
    let head_before = rev_parse(path, "HEAD")?;
    if staged.is_ok() && head_before == workspace.base_sha {
        return Ok(None);
    }
    if staged.is_err() {
        git(path, &["-c", "user.name=Guild", "-c", "user.email=guild@agents-pixel-dungeon", "commit", "-q", "-m", message])?;
    }
    let head_sha = rev_parse(path, "HEAD")?;
    let tree_sha = git(path, &["rev-parse", "HEAD^{tree}"])?;
    let stat = git(path, &["diff", "--shortstat", &workspace.base_sha, &head_sha]).unwrap_or_default();
    let (files, ins, del) = parse_shortstat(&stat);
    Ok(Some(Candidate { base_sha: workspace.base_sha.clone(), head_sha, tree_sha, files_changed: files, insertions: ins, deletions: del, diff_artifact_id: None }))
}

pub fn candidate_diff(workspace: &Workspace, candidate: &Candidate) -> GitResult<String> {
    git(Path::new(&workspace.path), &["diff", &candidate.base_sha, &candidate.head_sha])
}

fn parse_shortstat(s: &str) -> (u32, u32, u32) {
    let mut out = (0, 0, 0);
    for part in s.split(',') {
        let n: u32 = part.trim().split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        if part.contains("file") { out.0 = n } else if part.contains("insertion") { out.1 = n } else if part.contains("deletion") { out.2 = n }
    }
    out
}

/// Remove an attempt's worktree and branch. Refuses paths outside the
/// managed area.
pub fn remove(project: &Project, workspace: &Workspace, delete_branch: bool) -> GitResult<()> {
    let path = PathBuf::from(&workspace.path);
    let managed = crate::paths::worktrees_dir();
    let canonical = path.canonicalize().unwrap_or(path.clone());
    let managed_c = managed.canonicalize().unwrap_or(managed);
    if !canonical.starts_with(&managed_c) { return Err(format!("{} is outside the managed worktrees", workspace.path)); }
    let repo = Path::new(&project.repo_path);
    if path.exists() {
        git(repo, &["worktree", "remove", "--force", &path.display().to_string()])?;
    }
    let _ = git(repo, &["worktree", "prune"]);
    if delete_branch {
        let _ = git(repo, &["branch", "-D", &workspace.branch]);
    }
    Ok(())
}

/// Create (or find) the quest's integration branch at `base`.
pub fn ensure_integration_branch(project: &Project, quest: &Quest) -> GitResult<String> {
    let repo = Path::new(&project.repo_path);
    if ref_exists(repo, &quest.integration_ref) {
        return rev_parse(repo, &quest.integration_ref);
    }
    let base = match &quest.base_sha {
        Some(sha) => sha.clone(),
        None => rev_parse(repo, &project.target_ref).or_else(|_| rev_parse(repo, "HEAD"))?,
    };
    git(repo, &["update-ref", &quest.integration_ref, &base])?;
    Ok(base)
}

/// Squash the candidate onto the integration branch in a temporary
/// checkout, with compare-and-swap on the branch. Returns the new sha, or
/// an error naming the conflict.
/// Commit what `write` puts into a temporary checkout onto `ref_name`,
/// which starts from `base` when it does not exist yet. The user's
/// checkout is never touched; the ref moves with compare-and-swap.
/// Returns the new commit, or the old one when nothing changed.
pub fn publish(project: &Project, ref_name: &str, base: &str, message: &str, write: impl FnOnce(&Path) -> std::io::Result<Vec<String>>) -> GitResult<(String, Vec<String>)> {
    let repo = Path::new(&project.repo_path);
    let expected = match rev_parse(repo, ref_name) {
        Ok(sha) => sha,
        Err(_) => {
            let sha = rev_parse(repo, base)?;
            git(repo, &["update-ref", ref_name, &sha])?;
            sha
        }
    };
    let tmp = crate::paths::worktrees_dir().join(&project.id).join(format!("publish-{}", short(ref_name)));
    if tmp.exists() { let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]); }
    let _ = git(repo, &["worktree", "prune"]);
    git(repo, &["worktree", "add", "--detach", &tmp.display().to_string(), &expected])?;
    let result = (|| -> GitResult<(String, Vec<String>)> {
        let files = write(&tmp).map_err(|e| e.to_string())?;
        git(&tmp, &["add", "-A"])?;
        if git(&tmp, &["diff", "--cached", "--quiet"]).is_ok() { return Ok((expected.clone(), files)); }
        git(&tmp, &["-c", "user.name=Guild", "-c", "user.email=guild@agents-pixel-dungeon", "commit", "-q", "-m", message])?;
        let new_sha = rev_parse(&tmp, "HEAD")?;
        git(repo, &["update-ref", ref_name, &new_sha, &expected])?;
        Ok((new_sha, files))
    })();
    let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]);
    let _ = git(repo, &["worktree", "prune"]);
    result
}

/// A detached checkout of `sha` at `dir`, for a human to play; never the
/// user's checkout.
pub fn checkout_detached(project: &Project, sha: &str, dir: &Path) -> GitResult<()> {
    let repo = Path::new(&project.repo_path);
    if dir.exists() { let _ = git(repo, &["worktree", "remove", "--force", &dir.display().to_string()]); }
    let _ = git(repo, &["worktree", "prune"]);
    if let Some(parent) = dir.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    git(repo, &["worktree", "add", "--detach", &dir.display().to_string(), sha]).map(|_| ())
}

/// Remove a checkout made by `checkout_detached`; only under the guild's worktrees.
pub fn remove_checkout(project: &Project, dir: &Path) {
    if !dir.starts_with(crate::paths::worktrees_dir()) { return; }
    let repo = Path::new(&project.repo_path);
    let _ = git(repo, &["worktree", "remove", "--force", &dir.display().to_string()]);
    let _ = git(repo, &["worktree", "prune"]);
}

/// Run `read` on a temporary checkout of `rev`; None when it does not exist.
pub fn read_ref<T>(project: &Project, rev: &str, read: impl FnOnce(&Path) -> T) -> Option<T> {
    let repo = Path::new(&project.repo_path);
    let sha = rev_parse(repo, rev).ok()?;
    let tmp = crate::paths::worktrees_dir().join(&project.id).join(format!("read-{}", short(&sha)));
    if tmp.exists() { let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]); }
    git(repo, &["worktree", "add", "--detach", &tmp.display().to_string(), &sha]).ok()?;
    let out = read(&tmp);
    let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]);
    let _ = git(repo, &["worktree", "prune"]);
    Some(out)
}

pub fn integrate(project: &Project, quest: &Quest, candidate: &Candidate, task: &Task) -> GitResult<String> {
    let repo = Path::new(&project.repo_path);
    let expected = ensure_integration_branch(project, quest)?;
    let tmp = crate::paths::worktrees_dir().join(&project.id).join(format!("integrate-{}", short(&quest.id)));
    if tmp.exists() {
        let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]);
    }
    let _ = git(repo, &["worktree", "prune"]);
    git(repo, &["worktree", "add", "--detach", &tmp.display().to_string(), &expected])?;
    let result = (|| -> GitResult<String> {
        let merge = git(&tmp, &["merge", "--squash", &candidate.head_sha]);
        if let Err(err) = merge {
            let _ = git(&tmp, &["merge", "--abort"]);
            return Err(format!("conflict integrating {}: {err}", task.title));
        }
        let staged = git(&tmp, &["diff", "--cached", "--quiet"]);
        if staged.is_ok() {
            return Ok(expected.clone());
        }
        let message = format!("guild: {}\n\nTask {} · attempt candidate {}", task.title, task.id, &candidate.head_sha[..12.min(candidate.head_sha.len())]);
        git(&tmp, &["-c", "user.name=Guild", "-c", "user.email=guild@agents-pixel-dungeon", "commit", "-q", "-m", &message])?;
        let new_sha = rev_parse(&tmp, "HEAD")?;
        git(repo, &["update-ref", &quest.integration_ref, &new_sha, &expected])?;
        Ok(new_sha)
    })();
    let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]);
    let _ = git(repo, &["worktree", "prune"]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo() -> (tempdir::TempDir, Project) {
        let dir = tempdir::TempDir::new("guild-repo");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        git(&repo, &["add", "-A"]).unwrap();
        git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]).unwrap();
        let project = Project { id: "p".into(), name: "p".into(), repo_path: repo.display().to_string(), target_ref: "refs/heads/main".into(), setup: vec![], verifiers: vec![], playtest: None, play: None, ai_play: None, created_at: now() };
        (dir, project)
    }

    #[test]
    fn a_worktree_is_created_committed_and_integrated() {
        let _guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (dir, project) = temp_repo();
        std::env::set_var("HPD_GUILD_HOME", dir.path().join("home").display().to_string());
        let base = rev_parse(Path::new(&project.repo_path), "HEAD").unwrap();
        let ws = provision(&project, "q1", "t1", "a1", &base).unwrap();
        assert!(Path::new(&ws.path).join("README.md").exists());
        assert!(commit_candidate(&ws, "nothing").unwrap().is_none());
        std::fs::write(Path::new(&ws.path).join("new.txt"), "x\n").unwrap();
        let candidate = commit_candidate(&ws, "candidate").unwrap().unwrap();
        assert_eq!(candidate.files_changed, 1);
        assert_ne!(candidate.head_sha, base);
        let quest = Quest { id: "q1".into(), project_id: "p".into(), title: "q".into(), description: String::new(), state: QuestState::Running, integration_ref: "refs/heads/guild/quest/q1".into(), base_sha: Some(base.clone()), plan_version: 1, created_at: now(), updated_at: now() };
        let task = Task { id: "t1".into(), quest_id: "q1".into(), title: "add file".into(), instructions: String::new(), acceptance: vec![], depends_on: vec![], role: "implementer".into(), member_id: None, model_id: None, priority: 0, state: TaskState::Accepted, block_reason: None, attempts_made: 1, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, winner_attempt_id: None, version: 1, created_at: now(), updated_at: now() };
        let merged = integrate(&project, &quest, &candidate, &task).unwrap();
        assert_ne!(merged, base);
        let files = git(Path::new(&project.repo_path), &["ls-tree", "--name-only", &merged]).unwrap();
        assert!(files.contains("new.txt"));
        // The user's checkout did not move.
        assert_eq!(rev_parse(Path::new(&project.repo_path), "HEAD").unwrap(), base);
        remove(&project, &ws, true).unwrap();
        assert!(!Path::new(&ws.path).exists());
    }
}

#[cfg(test)]
mod tempdir {
    pub struct TempDir(std::path::PathBuf);
    impl TempDir {
        pub fn new(prefix: &str) -> TempDir {
            let p = std::env::temp_dir().join(format!("{prefix}-{}", crate::domain::new_id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        pub fn path(&self) -> &std::path::Path { &self.0 }
    }
    impl Drop for TempDir {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
}
