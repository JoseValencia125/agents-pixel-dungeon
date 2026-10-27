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
    // Two `git worktree` commands at once read each other's half-made entries ("failed to read
    // .git/worktrees/…/commondir"): one at a time.
    static WORKTREES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = (args.first() == Some(&"worktree")).then(|| WORKTREES.lock().unwrap_or_else(|e| e.into_inner()));
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
/// integrated), so a retry goes on from it. Git merges in three ways; a
/// conflict stays in the files, marked, and says which; anything else puts
/// the worktree back as it was.
pub fn carry(workspace: &Workspace, candidate: &Candidate, union: &[String]) -> Carry {
    let path = Path::new(&workspace.path);
    let range = format!("{}..{}", candidate.base_sha, candidate.head_sha);
    let mut pick = union_args(&workspace.project_id, union);
    pick.extend(["cherry-pick".into(), "--no-commit".into(), range]);
    if git(path, &pick.iter().map(String::as_str).collect::<Vec<_>>()).is_ok() { return Carry::Applied; }
    // It clashes with what was integrated meanwhile: the worktree keeps both,
    // with the conflict markers in the files, for the member to resolve
    // (members cannot merge branches themselves).
    let conflicted: Vec<String> = git(path, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    if !conflicted.is_empty() && git(path, &["cherry-pick", "--quit"]).is_ok() && git(path, &["reset", "-q"]).is_ok() {
        return Carry::Conflicts(conflicted);
    }
    let _ = git(path, &["cherry-pick", "--abort"]);
    let _ = git(path, &["reset", "--hard", &workspace.base_sha]);
    let _ = git(path, &["clean", "-fdq"]);
    Carry::Failed
}

/// The files that only grow (registries, lists of tests), joined line by
/// line when two branches add to them (`[merge] union` in `.guild/guild.toml`,
/// FB-068): git's union driver, through an attributes file of the guild's.
pub fn union_args(project_id: &str, union: &[String]) -> Vec<String> {
    if union.is_empty() { return vec![]; }
    let file = crate::paths::worktrees_dir().join(project_id).join(".union-attributes");
    let text: String = union.iter().map(|p| format!("{} merge=union\n", p.trim())).collect();
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
        if let Some(dir) = file.parent() { let _ = std::fs::create_dir_all(dir); }
        if std::fs::write(&file, &text).is_err() { return vec![]; }
    }
    vec!["-c".into(), format!("core.attributesFile={}", file.display())]
}

/// Give a new checkout the project's ignored folders (`[project] seed`:
/// an import cache, fetched assets) as copy-on-write clones of the user's
/// checkout, so it neither rebuilds them nor takes their room again until
/// it changes them (FB-075). Where the disk cannot clone, nothing is copied
/// and the setup builds them as before. Returns the folders seeded.
pub fn seed(project: &Project, dir: &Path) -> Vec<String> {
    let repo = Path::new(&project.repo_path);
    let mut out = vec![];
    for rel in &project.seed {
        let rel = rel.trim().trim_matches('/');
        if rel.is_empty() || rel.split('/').any(|c| c == "..") { continue; }
        let (from, to) = (repo.join(rel), dir.join(rel));
        if !from.exists() || to.exists() { continue; }
        if let Some(parent) = to.parent() { let _ = std::fs::create_dir_all(parent); }
        let clone: &[&str] = if cfg!(target_os = "macos") { &["-cR"] } else if cfg!(target_os = "linux") { &["-R", "--reflink=always"] } else { continue };
        let ok = std::process::Command::new("cp").args(clone).arg(&from).arg(&to)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        if ok { out.push(rel.to_string()) } else { let _ = std::fs::remove_dir_all(&to); let _ = std::fs::remove_file(&to); }
    }
    out
}

/// Bytes free on the disk that holds `path`; `None` where it cannot be read.
pub fn free_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut probe = path.to_path_buf();
        while !probe.exists() { probe = probe.parent()?.to_path_buf(); }
        let c = std::ffi::CString::new(probe.as_os_str().as_bytes()).ok()?;
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 { return None; }
        #[allow(clippy::unnecessary_cast)]
        Some(s.f_bavail as u64 * s.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut probe = path.to_path_buf();
        while !probe.exists() { probe = probe.parent()?.to_path_buf(); }
        let wide: Vec<u16> = probe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free: u64 = 0;
        let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(free)
    }
    #[cfg(not(any(unix, windows)))]
    { let _ = path; None }
}

/// How a candidate went onto a newer base.
#[derive(Debug, PartialEq)]
pub enum Carry { Applied, Conflicts(Vec<String>), Failed }

/// Run the project's setup command in the worktree, returning its output.
pub fn setup(workspace: &Workspace, argv: &[String], timeout_seconds: u64) -> GitResult<String> {
    if argv.is_empty() { return Ok(String::new()); }
    let output = run_with_timeout(Path::new(&workspace.path), argv, timeout_seconds)?;
    if output.0 != 0 { return Err(format!("setup exited with {}: {}", output.0, output.1.chars().take(2000).collect::<String>())); }
    Ok(output.1)
}

/// (exit code, combined output)
pub fn run_with_timeout(cwd: &Path, argv: &[String], timeout_seconds: u64) -> GitResult<(i32, String)> {
    run_with_timeout_env(cwd, argv, timeout_seconds, &[])
}

/// `run_with_timeout` with extra environment variables.
pub fn run_with_timeout_env(cwd: &Path, argv: &[String], timeout_seconds: u64, env: &[(String, String)]) -> GitResult<(i32, String)> {
    use std::io::Read;
    use std::time::{Duration, Instant};
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let mut child = crate::platform::command(program)
        .args(args)
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
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

/// Keep what a worktree holds that no commit has (tracked, or untracked and not ignored): it is
/// committed onto the attempt's branch, which stays when the worktree goes. True when there was something.
pub fn rescue_uncommitted(workspace: &Workspace) -> GitResult<bool> {
    let path = Path::new(&workspace.path);
    if !path.exists() || git(path, &["status", "--porcelain"])?.trim().is_empty() { return Ok(false); }
    git(path, &["add", "-A"])?;
    git(path, &["-c", "user.name=Guild", "-c", "user.email=guild@agents-pixel-dungeon", "commit", "-q", "-m", "guild: what the worktree held when it was removed"])?;
    Ok(true)
}

/// A member that ran `git stash` in its worktree stashed in the user's repository, which shares it:
/// every stash made on one of the guild's branches is kept as a branch of its own and taken out of
/// the stash, so nothing is lost and the user's `git stash list` is the user's again. The branches made.
pub fn rescue_stashes(project: &Project) -> Vec<String> {
    let repo = Path::new(&project.repo_path);
    let Ok(list) = git(repo, &["stash", "list", "--format=%H%x09%gs"]) else { return vec![] };
    let mut made = vec![];
    // From the oldest: dropping one does not move the indexes of those below it.
    let entries: Vec<(usize, &str, &str)> = list.lines().enumerate().filter_map(|(i, l)| l.split_once('\t').map(|(sha, subject)| (i, sha, subject))).collect();
    for (index, sha, subject) in entries.into_iter().rev() {
        let on_guild_branch = subject.split_once(": ").map(|(head, _)| head).is_some_and(|head| head.rsplit(' ').next().is_some_and(|b| b.starts_with("guild/")));
        if !on_guild_branch { continue; }
        let branch = format!("guild/rescued/stash-{}", &sha[..12.min(sha.len())]);
        if !ref_exists(repo, &format!("refs/heads/{branch}")) && git(repo, &["branch", &branch, sha]).is_err() { continue; }
        if git(repo, &["stash", "drop", &format!("stash@{{{index}}}")]).is_ok() { made.push(branch); }
    }
    made
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
        if let Err(e) = git(repo, &["worktree", "remove", "--force", &path.display().to_string()]) {
            // The repository moved or git cannot: the folder is the guild's own, so it is deleted
            // anyway, or it would be retried for ever and keep its gigabytes (FB-067).
            std::fs::remove_dir_all(&path).map_err(|d| format!("{e}; and deleting the folder: {d}"))?;
        }
    }
    let _ = git(repo, &["worktree", "prune"]);
    if delete_branch {
        let _ = git(repo, &["branch", "-D", &workspace.branch]);
    }
    Ok(())
}

/// Folders under the worktrees' area that look like an attempt's worktree (a
/// 26-character id) and that no live workspace owns, untouched for `idle`:
/// what a crash, a failed removal or a moved repository left behind (FB-067).
pub fn orphan_worktrees(live: &[PathBuf], idle: std::time::Duration) -> Vec<PathBuf> {
    let root = crate::paths::worktrees_dir();
    let live: Vec<PathBuf> = live.iter().flat_map(|p| [p.clone(), p.canonicalize().unwrap_or_else(|_| p.clone())]).collect();
    let mut out = vec![];
    for project in std::fs::read_dir(&root).into_iter().flatten().flatten().filter(|e| e.path().is_dir()) {
        for dir in std::fs::read_dir(project.path()).into_iter().flatten().flatten() {
            let path = dir.path();
            let name = dir.file_name().to_string_lossy().to_string();
            let attempt_like = name.len() == 26 && name.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase());
            let old = dir.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age >= idle);
            if attempt_like && path.is_dir() && old && !live.contains(&path) && !live.contains(&path.canonicalize().unwrap_or_else(|_| path.clone())) { out.push(path); }
        }
    }
    out
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

/// Bring `from` (the project's target, typically) into a quest's branch,
/// in a temporary checkout, with compare-and-swap on the branch, so later
/// attempts start from it. Returns (old head, new head, files changed);
/// a conflict aborts and names its files.
pub fn sync_quest(project: &Project, quest: &Quest, from: &str) -> GitResult<(String, String, Vec<String>)> {
    let lock = branch_lock(&quest.integration_ref);
    let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
    let repo = Path::new(&project.repo_path);
    let expected = ensure_integration_branch(project, quest)?;
    let source = rev_parse(repo, from)?;
    let tmp = crate::paths::worktrees_dir().join(&project.id).join(format!("sync-{}", short(&quest.integration_ref)));
    if tmp.exists() { let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]); }
    let _ = git(repo, &["worktree", "prune"]);
    git(repo, &["worktree", "add", "--detach", &tmp.display().to_string(), &expected])?;
    let result = (|| -> GitResult<(String, String, Vec<String>)> {
        let ident = ["-c", "user.name=Guild", "-c", "user.email=guild@agents-pixel-dungeon"];
        let message = format!("guild: bring {from} into the quest");
        let mut merge: Vec<&str> = ident.to_vec();
        merge.extend(["merge", "--no-edit", "-m", &message, &source]);
        if let Err(e) = git(&tmp, &merge) {
            let conflicts = git(&tmp, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = git(&tmp, &["merge", "--abort"]);
            return Err(if conflicts.trim().is_empty() { e } else { format!("conflicts in: {}", conflicts.lines().collect::<Vec<_>>().join(", ")) });
        }
        let new_sha = rev_parse(&tmp, "HEAD")?;
        if new_sha == expected { return Ok((expected.clone(), new_sha, vec![])); }
        let files: Vec<String> = git(&tmp, &["diff", "--name-only", &expected, &new_sha]).unwrap_or_default().lines().map(str::to_string).collect();
        git(repo, &["update-ref", &quest.integration_ref, &new_sha, &expected])?;
        Ok((expected.clone(), new_sha, files))
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

/// One integration at a time per branch: two tasks of a quest that finish
/// together queue up instead of working in each other's checkout (FB-077).
fn branch_lock(name: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    type Locks = std::sync::Mutex<Vec<(String, std::sync::Arc<std::sync::Mutex<()>>)>>;
    static LOCKS: Locks = std::sync::Mutex::new(Vec::new());
    let mut locks = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, lock)) = locks.iter().find(|(n, _)| n == name) { return lock.clone(); }
    let lock = std::sync::Arc::new(std::sync::Mutex::new(()));
    locks.push((name.to_string(), lock.clone()));
    lock
}

/// An integration that failed because the work clashes with what landed meanwhile,
/// not because git or the disk failed.
pub fn is_clash(error: &str) -> bool { error.starts_with("conflict integrating") }

/// Integrations that fail on purpose, by branch (tests only).
#[cfg(test)]
pub static FAIL_INTEGRATIONS: std::sync::Mutex<Vec<(String, u32)>> = std::sync::Mutex::new(Vec::new());

/// Squash the candidate onto the quest's branch. One at a time per branch; a
/// failure of git or the disk is tried again (a clash is not: the work has to change).
/// What an earlier try already landed is found by its mark and returned as it is.
pub fn integrate(project: &Project, quest: &Quest, candidate: &Candidate, task: &Task) -> GitResult<String> {
    let lock = branch_lock(&quest.integration_ref);
    let _one_at_a_time = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut tries = 0;
    loop {
        match integrate_once(project, quest, candidate, task) {
            Err(e) if !is_clash(&e) && tries < 2 => {
                tries += 1;
                if !cfg!(test) { std::thread::sleep(std::time::Duration::from_millis(500 * tries)); }
            }
            other => return other,
        }
    }
}

fn integrate_once(project: &Project, quest: &Quest, candidate: &Candidate, task: &Task) -> GitResult<String> {
    #[cfg(test)]
    {
        let mut fails = FAIL_INTEGRATIONS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, n)) = fails.iter_mut().find(|(r, n)| *r == quest.integration_ref && *n > 0) {
            *n -= 1;
            return Err("git worktree add failed: injected".into());
        }
    }
    let repo = Path::new(&project.repo_path);
    let expected = ensure_integration_branch(project, quest)?;
    let mark = format!("Task {} · attempt candidate {}", task.id, &candidate.head_sha[..12.min(candidate.head_sha.len())]);
    // The service may have died after the branch moved and before it said so.
    let range = quest.base_sha.as_ref().map(|b| format!("{b}..{}", quest.integration_ref)).unwrap_or_else(|| quest.integration_ref.clone());
    if let Ok(landed) = git(repo, &["log", "-1", "--format=%H", "--fixed-strings", &format!("--grep={mark}"), &range]) {
        if !landed.is_empty() { return Ok(landed); }
    }
    let tmp = crate::paths::worktrees_dir().join(&project.id).join(format!("integrate-{}-{}", short(&quest.id), short(&task.id)));
    if tmp.exists() {
        let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]);
    }
    let _ = git(repo, &["worktree", "prune"]);
    git(repo, &["worktree", "add", "--detach", &tmp.display().to_string(), &expected])?;
    let result = (|| -> GitResult<String> {
        let mut squash = union_args(&project.id, &project.merge_union);
        // The merge may build a commit of its own (criss-cross bases), which needs an identity.
        squash.extend(["-c".into(), "user.name=Guild".into(), "-c".into(), "user.email=guild@agents-pixel-dungeon".into()]);
        squash.extend(["merge".into(), "--squash".into(), candidate.head_sha.clone()]);
        let merge = git(&tmp, &squash.iter().map(String::as_str).collect::<Vec<_>>());
        if let Err(err) = merge {
            let _ = git(&tmp, &["merge", "--abort"]);
            return Err(format!("conflict integrating {}: {err}", task.title));
        }
        let staged = git(&tmp, &["diff", "--cached", "--quiet"]);
        if staged.is_ok() {
            return Ok(expected.clone());
        }
        let message = format!("guild: {}\n\n{mark}", task.title);
        git(&tmp, &["-c", "user.name=Guild", "-c", "user.email=guild@agents-pixel-dungeon", "commit", "-q", "-m", &message])?;
        let new_sha = rev_parse(&tmp, "HEAD")?;
        git(repo, &["update-ref", &quest.integration_ref, &new_sha, &expected])?;
        // Landed means the branch says so, not that the command said ok.
        if rev_parse(repo, &quest.integration_ref)? != new_sha { return Err(format!("{} did not take the integration", quest.integration_ref)); }
        Ok(new_sha)
    })();
    let _ = git(repo, &["worktree", "remove", "--force", &tmp.display().to_string()]);
    let _ = git(repo, &["worktree", "prune"]);
    result
}

/// Words that say a task brings back something that existed (FB-062).
const BRING_BACK: &[&str] = &["restaur", "recuper", "volver", "vuelv", "devolv", "prototip", "borrad", "eliminad", "como era", "como estaba", "restore", "recover", "bring back", "previous", "removed", "deleted", "prototype", "revert", "used to"];

/// Files that existed in `dir`'s history and are gone now, whose names
/// share words with the task's text: (the commit that deleted it, the
/// path), best matches first. Only when the text speaks of bringing
/// something back; `docs/REMOVED_*.md` notes are listed too.
pub fn deleted_matching(dir: &Path, text: &str, limit: usize) -> Vec<(String, String)> {
    let lower = text.to_lowercase();
    if !BRING_BACK.iter().any(|w| lower.contains(w)) { return vec![]; }
    let words: Vec<String> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4).map(str::to_string).collect();
    let Ok(log) = git(dir, &["log", "--diff-filter=D", "--name-only", "--format=@%H", "-n", "300"]) else { return vec![] };
    let mut sha = String::new();
    let mut found: Vec<(usize, String, String)> = vec![];
    for line in log.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(h) = line.strip_prefix('@') { sha = h.to_string(); continue; }
        if dir.join(line).exists() || found.iter().any(|(_, _, p)| p == line) { continue; }
        let parts: Vec<String> = line.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 4).map(str::to_string).collect();
        let score = parts.iter().filter(|p| words.iter().any(|w| w.starts_with(p.as_str()) || p.starts_with(w.as_str()))).count();
        if score > 0 { found.push((score, sha[..12.min(sha.len())].to_string(), line.to_string())); }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<(String, String)> = found.into_iter().take(limit).map(|(_, s, p)| (s, p)).collect();
    if let Ok(entries) = std::fs::read_dir(dir.join("docs")) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("REMOVED") && name.ends_with(".md") { out.push((String::new(), format!("docs/{name}"))); }
        }
    }
    out
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
        let project = Project { id: "p".into(), name: "p".into(), repo_path: repo.display().to_string(), target_ref: "refs/heads/main".into(), setup: vec![], verifiers: vec![], playtest: None, play: None, ai_play: None, permitted_programs: vec![], permitted_reads: vec![], permitted_commands: vec![], web: true, merge_union: vec![], threads: None, seed: vec![], created_at: now() };
        (dir, project)
    }

    #[test]
    fn a_task_bringing_something_back_is_shown_where_the_history_has_it() {
        // FB-062: the prototype's look was in git (91de469) and the members rebuilt it from scratch.
        let (_dir, project) = temp_repo();
        let repo = Path::new(&project.repo_path);
        let commit = |m: &str| git(repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", m]).unwrap();
        std::fs::create_dir_all(repo.join("shaders")).unwrap();
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(repo.join("shaders/neon_glow.gdshader"), "shader\n").unwrap();
        std::fs::write(repo.join("shaders/rain.gdshader"), "rain\n").unwrap();
        std::fs::write(repo.join("scripts.gd"), "x\n").unwrap();
        git(repo, &["add", "-A"]).unwrap();
        commit("prototype");
        git(repo, &["rm", "-q", "shaders/neon_glow.gdshader", "shaders/rain.gdshader"]).unwrap();
        std::fs::write(repo.join("docs/REMOVED_LOOK.md"), "what went\n").unwrap();
        git(repo, &["add", "-A"]).unwrap();
        commit("clean up");
        let found = deleted_matching(repo, "Restaurar el look neón del prototipo: el glow de los edificios", 10);
        assert_eq!(found[0].1, "shaders/neon_glow.gdshader", "{found:?}");
        assert_eq!(found[0].0.len(), 12);
        assert!(!found.iter().any(|f| f.1 == "shaders/rain.gdshader"), "only what the task speaks of: {found:?}");
        assert!(found.iter().any(|f| f.1 == "docs/REMOVED_LOOK.md" && f.0.is_empty()));
        assert!(deleted_matching(repo, "Add the glow to the buildings", 10).is_empty(), "a task that brings nothing back hears nothing");
    }

    #[test]
    fn files_that_only_grow_are_joined_when_two_tasks_add_to_them() {
        // FB-068: five tasks each added their profile to one registry and clashed at integration.
        let _guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (dir, mut project) = temp_repo();
        std::env::set_var("HPD_GUILD_HOME", dir.path().join("home").display().to_string());
        let repo = Path::new(&project.repo_path);
        std::fs::write(repo.join("tests.txt"), "core\n").unwrap();
        git(repo, &["add", "-A"]).unwrap();
        git(repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "tests"]).unwrap();
        let base = rev_parse(repo, "HEAD").unwrap();
        let quest = Quest { id: "q1".into(), project_id: "p".into(), title: "q".into(), description: String::new(), state: QuestState::Running, integration_ref: "refs/heads/guild/quest/q1".into(), base_sha: Some(base.clone()), read_paths: vec![], notes: vec![], plan_version: 1, created_at: now(), updated_at: now() };
        let mut cands = vec![];
        for (id, line) in [("a1", "park\n"), ("a2", "school\n")] {
            let ws = provision(&project, "q1", id, id, &base).unwrap();
            std::fs::write(Path::new(&ws.path).join("tests.txt"), format!("core\n{line}")).unwrap();
            cands.push((commit_candidate(&ws, "c").unwrap().unwrap(), id));
        }
        let task = |id: &str| Task { id: id.into(), quest_id: "q1".into(), title: id.into(), instructions: String::new(), acceptance: vec![], depends_on: vec![], role: "implementer".into(), member_id: None, model_id: None, priority: 0, state: TaskState::Accepted, block_reason: None, attempts_made: 1, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, timeout_minutes: None, winner_attempt_id: None, notes: vec![], granted: Default::default(), evidence: vec![], spike: false, version: 1, created_at: now(), updated_at: now() };
        integrate(&project, &quest, &cands[0].0, &task("a1")).unwrap();
        assert!(integrate(&project, &quest, &cands[1].0, &task("a2")).unwrap_err().starts_with("conflict integrating"), "without the rule they clash");
        project.merge_union = vec!["tests.txt".into()];
        let merged = integrate(&project, &quest, &cands[1].0, &task("a2")).unwrap();
        assert_eq!(git(repo, &["show", &format!("{merged}:tests.txt")]).unwrap(), "core\npark\nschool");
    }

    /// A quest with `n` candidates that each add a file of their own (FB-077).
    fn quest_with_candidates(n: usize) -> (tempdir::TempDir, Project, Quest, Vec<Candidate>, std::path::PathBuf) {
        let (dir, project) = temp_repo();
        std::env::set_var("HPD_GUILD_HOME", dir.path().join("home").display().to_string());
        let repo = PathBuf::from(&project.repo_path);
        let base = rev_parse(&repo, "HEAD").unwrap();
        let quest = Quest { id: "q-parallel".into(), project_id: "p".into(), title: "q".into(), description: String::new(), state: QuestState::Running, integration_ref: "refs/heads/guild/quest/qparalle".into(), base_sha: Some(base.clone()), read_paths: vec![], notes: vec![], plan_version: 1, created_at: now(), updated_at: now() };
        let mut cands = vec![];
        for i in 0..n {
            let id = format!("a{i}");
            let ws = provision(&project, "q-parallel", &id, &id, &base).unwrap();
            std::fs::write(Path::new(&ws.path).join(format!("file{i}.txt")), format!("{i}\n")).unwrap();
            cands.push(commit_candidate(&ws, "c").unwrap().unwrap());
        }
        (dir, project, quest, cands, repo)
    }

    fn a_task(id: &str) -> Task {
        Task { id: id.into(), quest_id: "q-parallel".into(), title: id.into(), instructions: String::new(), acceptance: vec![], depends_on: vec![], role: "implementer".into(), member_id: None, model_id: None, priority: 0, state: TaskState::Accepted, block_reason: None, attempts_made: 1, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, timeout_minutes: None, winner_attempt_id: None, notes: vec![], granted: Default::default(), evidence: vec![], spike: false, version: 1, created_at: now(), updated_at: now() }
    }

    #[test]
    fn two_tasks_of_a_quest_integrating_together_both_land() {
        // FB-077: the second one removed the checkout the first was working in.
        let _guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, project, quest, cands, repo) = quest_with_candidates(4);
        let handles: Vec<_> = cands.into_iter().enumerate().map(|(i, c)| {
            let (project, quest) = (project.clone(), quest.clone());
            std::thread::spawn(move || integrate(&project, &quest, &c, &a_task(&format!("t{i}"))))
        }).collect();
        for h in handles { h.join().unwrap().unwrap(); }
        let tree = git(&repo, &["ls-tree", "--name-only", &quest.integration_ref]).unwrap();
        for i in 0..4 { assert!(tree.contains(&format!("file{i}.txt")), "{tree}"); }
    }

    #[test]
    fn integrating_what_already_landed_finds_it_instead_of_doing_it_twice() {
        // The service can die after the branch moved and before it said so.
        let _guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, project, quest, cands, repo) = quest_with_candidates(1);
        let first = integrate(&project, &quest, &cands[0], &a_task("t0")).unwrap();
        let again = integrate(&project, &quest, &cands[0], &a_task("t0")).unwrap();
        assert_eq!(first, again);
        assert_eq!(git(&repo, &["rev-list", "--count", &format!("{}..{}", quest.base_sha.clone().unwrap(), quest.integration_ref)]).unwrap(), "1", "one commit, not two");
    }

    #[test]
    fn a_failure_of_git_is_tried_again_and_a_clash_is_not() {
        let _guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, project, quest, cands, _repo) = quest_with_candidates(1);
        FAIL_INTEGRATIONS.lock().unwrap().push((quest.integration_ref.clone(), 2));
        integrate(&project, &quest, &cands[0], &a_task("t0")).expect("two failures, the third try lands");
        assert!(is_clash("conflict integrating x: y") && !is_clash("git worktree add failed"));
    }

    #[test]
    fn a_stash_made_in_a_guild_worktree_is_kept_as_a_branch_and_leaves_the_users_stash() {
        let _guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (_dir, project, quest, _cands, repo) = quest_with_candidates(0);
        let base = quest.base_sha.clone().unwrap();
        let ws = provision(&project, "q-parallel", "t1", "a-stash", &base).unwrap();
        std::fs::write(Path::new(&ws.path).join("README.md"), "changed in the member's worktree\n").unwrap();
        git(Path::new(&ws.path), &["-c", "user.name=m", "-c", "user.email=m@m", "stash"]).unwrap();
        // And one the user made in their own checkout, which stays theirs.
        std::fs::write(repo.join("README.md"), "the user's own work\n").unwrap();
        git(&repo, &["-c", "user.name=u", "-c", "user.email=u@u", "stash"]).unwrap();
        assert_eq!(git(&repo, &["stash", "list"]).unwrap().lines().count(), 2);
        let made = rescue_stashes(&project);
        assert_eq!(made.len(), 1, "{made:?}");
        assert_eq!(git(&repo, &["show", &format!("refs/heads/{}:README.md", made[0])]).unwrap(), "changed in the member's worktree");
        let left = git(&repo, &["stash", "list"]).unwrap();
        assert_eq!(left.lines().count(), 1, "only the user's: {left}");
    }

    #[test]
    fn a_new_worktree_borrows_the_ignored_folders_instead_of_copying_them() {
        // FB-075: each worktree imported its own 1.3 GB .godot/ and copied assets/city.
        let _guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (dir, mut project) = temp_repo();
        std::env::set_var("HPD_GUILD_HOME", dir.path().join("home").display().to_string());
        let repo = Path::new(&project.repo_path);
        std::fs::write(repo.join(".gitignore"), ".godot/\n").unwrap();
        std::fs::create_dir_all(repo.join(".godot/imported")).unwrap();
        std::fs::write(repo.join(".godot/imported/city.ctex"), "texture").unwrap();
        git(repo, &["add", "-A"]).unwrap();
        git(repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "ignore"]).unwrap();
        project.seed = vec![".godot".into(), "../outside".into(), "missing".into()];
        let ws = provision(&project, "q1", "t1", "a1", &rev_parse(repo, "HEAD").unwrap()).unwrap();
        let seeded = seed(&project, Path::new(&ws.path));
        let copy = Path::new(&ws.path).join(".godot/imported/city.ctex");
        if seeded.is_empty() {
            assert!(!copy.exists(), "a disk that cannot clone leaves the setup to build it");
        } else {
            assert_eq!(seeded, vec![".godot".to_string()]);
            assert_eq!(std::fs::read_to_string(&copy).unwrap(), "texture");
            std::fs::write(&copy, "changed").unwrap();
            assert_eq!(std::fs::read_to_string(repo.join(".godot/imported/city.ctex")).unwrap(), "texture", "the user's checkout is untouched");
        }
        if cfg!(target_os = "macos") { assert!(!seeded.is_empty(), "APFS clones"); }
        assert!(seed(&project, Path::new(&ws.path)).is_empty(), "what is there stays");
        assert!(free_bytes(Path::new(&ws.path)).is_some_and(|b| b > 0) || cfg!(not(unix)));
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
        let quest = Quest { id: "q1".into(), project_id: "p".into(), title: "q".into(), description: String::new(), state: QuestState::Running, integration_ref: "refs/heads/guild/quest/q1".into(), base_sha: Some(base.clone()), read_paths: vec![], notes: vec![], plan_version: 1, created_at: now(), updated_at: now() };
        let task = Task { id: "t1".into(), quest_id: "q1".into(), title: "add file".into(), instructions: String::new(), acceptance: vec![], depends_on: vec![], role: "implementer".into(), member_id: None, model_id: None, priority: 0, state: TaskState::Accepted, block_reason: None, attempts_made: 1, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, timeout_minutes: None, winner_attempt_id: None, notes: vec![], granted: Default::default(), evidence: vec![], spike: false, version: 1, created_at: now(), updated_at: now() };
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
