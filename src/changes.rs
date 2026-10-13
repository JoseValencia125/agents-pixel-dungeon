//! What an agent changed in its folder: git's changed files and a file's
//! diff, and the pull request of its branch (GitHub's `gh`). Read on
//! threads, and only while the chat shows them.

use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};

/// A changed file: git's status letter (M, A, D, R, U, or ? when untracked),
/// its path, and the lines added and removed (None for a binary file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub status: char,
    pub path: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// OPEN, MERGED or CLOSED.
    pub state: String,
    pub draft: bool,
}

/// A command found on PATH or in a usual install folder (an app opened
/// from Finder gets a short PATH).
fn tool(name: &str) -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].iter().map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

/// Run git in `cwd` without taking the index lock an agent may need.
fn git(cwd: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new(tool("git")).arg("-C").arg(cwd).args(args).env("GIT_OPTIONAL_LOCKS", "0").env("LC_ALL", "C").output().ok()?;
    output.status.success().then_some(output.stdout)
}

/// `git status --porcelain -z`: each file's status letter and path.
pub fn parse_status(data: &[u8]) -> Vec<(char, String)> {
    let mut entries = data.split(|b| *b == 0).filter(|e| e.len() > 3);
    let mut files = vec![];
    while let Some(entry) = entries.next() {
        let (x, y) = (entry[0] as char, entry[1] as char);
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        // A rename or copy carries its old path next.
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') { entries.next(); }
        let status = match (x, y) {
            ('?', '?') => '?',
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => 'U',
            (x, ' ') => x,
            (_, y) => y,
        };
        files.push((status, path));
    }
    files
}

/// `git diff --numstat -z`: lines added and removed by path ("-" for binary).
pub fn parse_numstat(data: &[u8]) -> HashMap<String, (Option<u32>, Option<u32>)> {
    let mut counts = HashMap::new();
    let mut fields = data.split(|b| *b == 0);
    while let Some(field) = fields.next() {
        let text = String::from_utf8_lossy(field);
        let mut parts = text.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next()) else { continue };
        // A rename leaves the path empty and gives the old and new paths next.
        let path = if path.is_empty() { fields.next(); fields.next().map(|p| String::from_utf8_lossy(p).to_string()).unwrap_or_default() } else { path.to_string() };
        counts.insert(path, (added.parse().ok(), removed.parse().ok()));
    }
    counts
}

/// The folder's changed files against its last commit, untracked ones too;
/// an error when it is not a git repository.
pub fn changed_files(cwd: &str) -> Result<Vec<FileChange>, String> {
    let status = git(cwd, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]).ok_or_else(|| "not a repository".to_string())?;
    let counts = git(cwd, &["diff", "HEAD", "--numstat", "-z"]).or_else(|| git(cwd, &["diff", "--cached", "--numstat", "-z"])).map(|d| parse_numstat(&d)).unwrap_or_default();
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).map(|d| String::from_utf8_lossy(&d).trim().to_string()).unwrap_or_else(|| cwd.to_string());
    Ok(parse_status(&status).into_iter().map(|(status, path)| {
        let (added, removed) = match counts.get(&path) {
            Some(&(a, r)) => (a, r),
            // An untracked file: all its lines are new.
            None if status == '?' => (std::fs::read(std::path::Path::new(&root).join(&path)).ok().filter(|d| !d.contains(&0)).map(|d| d.split(|b| *b == b'\n').filter(|l| !l.is_empty()).count() as u32), Some(0)),
            None => (Some(0), Some(0)),
        };
        FileChange { status, path, added, removed }
    }).collect())
}

/// A file's diff against the last commit, as lines (an untracked file's
/// lines, all added). At most `limit` lines.
pub fn file_diff(cwd: &str, change: &FileChange, limit: usize) -> Vec<String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).map(|d| String::from_utf8_lossy(&d).trim().to_string()).unwrap_or_else(|| cwd.to_string());
    if change.status == '?' {
        return match std::fs::read(std::path::Path::new(&root).join(&change.path)) {
            Ok(data) if data.contains(&0) => vec!["Binary file".into()],
            Ok(data) => String::from_utf8_lossy(&data).lines().take(limit).map(|l| format!("+{l}")).collect(),
            Err(_) => vec![],
        };
    }
    let data = git(&root, &["diff", "HEAD", "--no-color", "--", &change.path]).filter(|d| !d.is_empty())
        .or_else(|| git(&root, &["diff", "--cached", "--no-color", "--", &change.path]))
        .unwrap_or_default();
    String::from_utf8_lossy(&data).lines().skip_while(|l| !l.starts_with("@@") && !l.starts_with("Binary")).take(limit).map(str::to_string).collect()
}

/// `gh pr view --json number,title,url,state,isDraft`.
pub fn parse_pull_request(data: &[u8]) -> Option<PullRequest> {
    let v: Value = serde_json::from_slice(data).ok()?;
    Some(PullRequest {
        number: v.get("number")?.as_u64()?,
        title: v.get("title")?.as_str()?.to_string(),
        url: v.get("url")?.as_str()?.to_string(),
        state: v.get("state").and_then(Value::as_str).unwrap_or("OPEN").to_string(),
        draft: v.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// The pull request of the folder's branch, if `gh` finds one.
pub fn pull_request(cwd: &str) -> Option<PullRequest> {
    let output = Command::new(tool("gh")).current_dir(cwd).args(["pr", "view", "--json", "number,title,url,state,isDraft"]).env("GH_PROMPT_DISABLED", "1").output().ok()?;
    if !output.status.success() { return None; }
    parse_pull_request(&output.stdout)
}

// The agent's folder comes abbreviated for display (~/…): expanded here.
pub fn fetch_changes(cwd: String) -> Receiver<Result<Vec<FileChange>, String>> {
    let cwd = crate::herdr::expand_home(&cwd);
    let (tx, rx) = channel();
    std::thread::spawn(move || { let _ = tx.send(changed_files(&cwd)); });
    rx
}

pub fn fetch_diff(cwd: String, change: FileChange) -> Receiver<Vec<String>> {
    let cwd = crate::herdr::expand_home(&cwd);
    let (tx, rx) = channel();
    std::thread::spawn(move || { let _ = tx.send(file_diff(&cwd, &change, 4000)); });
    rx
}

pub fn fetch_pull_request(cwd: String) -> Receiver<Option<PullRequest>> {
    let cwd = crate::herdr::expand_home(&cwd);
    let (tx, rx) = channel();
    std::thread::spawn(move || { let _ = tx.send(pull_request(&cwd)); });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses() {
        let data = b" M src/ui.rs\0M  Cargo.toml\0?? notes/new file.md\0R  src/new.rs\0src/old.rs\0 D gone.txt\0UU both.rs\0A  added.rs\0";
        assert_eq!(parse_status(data), vec![
            ('M', "src/ui.rs".into()), ('M', "Cargo.toml".into()), ('?', "notes/new file.md".into()),
            ('R', "src/new.rs".into()), ('D', "gone.txt".into()), ('U', "both.rs".into()), ('A', "added.rs".into()),
        ]);
        assert!(parse_status(b"").is_empty());
    }

    #[test]
    fn line_counts() {
        let data = b"12\t3\tsrc/ui.rs\0-\t-\tlogo.png\x000\t0\t\0src/old.rs\0src/new.rs\0";
        let counts = parse_numstat(data);
        assert_eq!(counts.get("src/ui.rs"), Some(&(Some(12), Some(3))));
        assert_eq!(counts.get("logo.png"), Some(&(None, None)));
        assert_eq!(counts.get("src/new.rs"), Some(&(Some(0), Some(0))));
    }

    #[test]
    fn pull_requests() {
        let pr = parse_pull_request(br#"{"isDraft":false,"number":10,"state":"OPEN","title":"Guild rooms","url":"https://github.com/o/r/pull/10"}"#).unwrap();
        assert_eq!((pr.number, pr.title.as_str(), pr.state.as_str(), pr.draft), (10, "Guild rooms", "OPEN", false));
        assert_eq!(parse_pull_request(b"no pull requests found"), None);
    }

    #[test]
    fn a_real_repository() {
        let dir = std::env::temp_dir().join(format!("apd-changes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cwd = dir.to_string_lossy().to_string();
        let run = |args: &[&str]| { Command::new("git").arg("-C").arg(&cwd).args(args).output().unwrap(); };
        run(&["init", "-q"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "start"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "a"]);
        std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        let files = changed_files(&cwd).unwrap();
        assert!(files.contains(&FileChange { status: 'M', path: "a.txt".into(), added: Some(2), removed: Some(1) }), "{files:?}");
        assert!(files.contains(&FileChange { status: '?', path: "b.txt".into(), added: Some(1), removed: Some(0) }), "{files:?}");
        let diff = file_diff(&cwd, &files.iter().find(|f| f.path == "a.txt").unwrap().clone(), 100);
        assert!(diff[0].starts_with("@@") && diff.contains(&"-two".to_string()) && diff.contains(&"+three".to_string()), "{diff:?}");
        assert_eq!(file_diff(&cwd, &files.iter().find(|f| f.path == "b.txt").unwrap().clone(), 100), vec!["+new".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
