//! Whether an agent can be closed without losing anything, and closing it:
//! nothing running or asked, a clean tree and every commit on a remote.
//! Its git worktree, when it works in one, goes with it (with its local
//! branch, whose commits a remote keeps), as AGENTS.md asks once a PR closes.

use crate::herdr::Agent;
use crate::l10n::{tr, trf};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A linked git worktree an agent works in, to remove once it is closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// The main checkout, which removes it.
    pub main: PathBuf,
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Verdict {
    /// Why closing it now would lose something; empty when it is safe.
    pub blockers: Vec<String>,
    pub worktree: Option<Worktree>,
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// What stops `agent` from closing safely; `others` are the rest of the agents.
pub fn check(agent: &Agent, others: &[Agent]) -> Verdict {
    let mut verdict = Verdict::default();
    match agent.status.as_str() {
        "working" => verdict.blockers.push(tr("Está trabajando.")),
        "blocked" => verdict.blockers.push(tr("Te está preguntando algo.")),
        _ => {}
    }
    if !agent.shells.is_empty() {
        verdict.blockers.push(trf("Tiene {} shell(s) corriendo en segundo plano.", &[&agent.shells.len().to_string()]));
    }
    let cwd = PathBuf::from(crate::herdr::expand_home(&agent.cwd));
    let Some(top) = git(&cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from) else { return verdict };
    let changed = git(&top, &["status", "--porcelain"]).map(|s| s.lines().count()).unwrap_or(0);
    if changed > 0 { verdict.blockers.push(trf("{} archivo(s) con cambios sin commit.", &[&changed.to_string()])); }
    match git(&top, &["rev-list", "--count", "HEAD", "--not", "--remotes"]).and_then(|n| n.parse::<usize>().ok()) {
        Some(0) => {}
        Some(n) => verdict.blockers.push(trf("{} commit(s) que no están en ningún remoto.", &[&n.to_string()])),
        None => verdict.blockers.push(tr("No se pudo leer el estado de git.")),
    }
    // A linked worktree: `git worktree list` names the main checkout first.
    let list = git(&top, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let main = list.lines().find_map(|l| l.strip_prefix("worktree ")).map(PathBuf::from);
    if let Some(main) = main.filter(|m| !same_path(m, &top)) {
        // Paths compared as the filesystem resolves them (/var is /private/var).
        let real = |p: PathBuf| p.canonicalize().unwrap_or(p);
        let top_real = real(top.clone());
        let sharing = others.iter().filter(|o| o.id != agent.id).any(|o| real(PathBuf::from(crate::herdr::expand_home(&o.cwd))).starts_with(&top_real));
        if sharing { verdict.blockers.push(tr("Otro agente trabaja en el mismo worktree.")); }
        let branch = git(&top, &["symbolic-ref", "--short", "-q", "HEAD"]);
        verdict.worktree = Some(Worktree { main, path: top, branch });
    }
    verdict
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.canonicalize().ok().zip(b.canonicalize().ok()).map_or(a == b, |(a, b)| a == b)
}

/// Remove a worktree whose agent has exited, and its local branch; the
/// error git gave, if any.
pub fn remove(worktree: &Worktree) -> Option<String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let out = Command::new("git").arg("-C").arg(&worktree.main).args(args).output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
    };
    let path = worktree.path.to_string_lossy();
    if let Err(e) = run(&["worktree", "remove", &path]) { return Some(trf("No se pudo borrar el worktree: {}", &[&e])); }
    if let Some(branch) = &worktree.branch {
        // Every commit of it is on a remote (checked before closing).
        if let Err(e) = run(&["branch", "-D", branch]) { return Some(trf("Se borró el worktree, pero no su rama {}: {}", &[branch, &e])); }
    }
    None
}
