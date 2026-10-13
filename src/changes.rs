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

/// What the files are compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffBase {
    /// The last commit: what is not committed yet.
    Uncommitted,
    /// The branch's upstream: what is not pushed yet, committed or not.
    Unpushed,
    /// Where the branch left main (or master): everything on the branch.
    Main,
}

impl DiffBase {
    pub const ALL: [DiffBase; 3] = [DiffBase::Uncommitted, DiffBase::Unpushed, DiffBase::Main];
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

/// `git diff --name-status -z`: each file's status letter and (new) path.
pub fn parse_name_status(data: &[u8]) -> Vec<(char, String)> {
    let mut fields = data.split(|b| *b == 0).filter(|f| !f.is_empty());
    let mut files = vec![];
    while let Some(code) = fields.next() {
        let status = code[0] as char;
        // A rename or copy gives the old path, then the new one.
        if matches!(status, 'R' | 'C') { fields.next(); }
        let Some(path) = fields.next() else { break };
        files.push((status, String::from_utf8_lossy(path).to_string()));
    }
    files
}

/// The commit `base` stands for in `cwd` and how to name it: None for the
/// last commit; an error when there is no upstream or no main branch.
pub fn base_commit(cwd: &str, base: DiffBase) -> Result<Option<(String, String)>, String> {
    let name = |args: &[&str]| git(cwd, args).map(|d| String::from_utf8_lossy(&d).trim().to_string()).filter(|s| !s.is_empty());
    match base {
        DiffBase::Uncommitted => Ok(None),
        DiffBase::Unpushed => {
            let upstream = name(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
                .or_else(|| name(&["rev-parse", "--abbrev-ref", "HEAD"]).map(|b| format!("origin/{b}")).filter(|r| name(&["rev-parse", "--verify", "--quiet", r]).is_some()))
                .ok_or_else(|| "no upstream".to_string())?;
            Ok(Some((upstream.clone(), upstream)))
        }
        DiffBase::Main => {
            let main = ["origin/main", "origin/master", "main", "master"].into_iter().find(|r| name(&["rev-parse", "--verify", "--quiet", r]).is_some()).ok_or_else(|| "no main".to_string())?;
            let fork = name(&["merge-base", "HEAD", main]).ok_or_else(|| "no main".to_string())?;
            Ok(Some((fork, main.to_string())))
        }
    }
}

/// The folder's changed files against `base` (the last commit, else a
/// commit), untracked ones too; an error when it is not a git repository
/// or the base cannot be found.
pub fn changed_files(cwd: &str, base: DiffBase) -> Result<Vec<FileChange>, String> {
    let status = git(cwd, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]).ok_or_else(|| "not a repository".to_string())?;
    let against = base_commit(cwd, base)?;
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).map(|d| String::from_utf8_lossy(&d).trim().to_string()).unwrap_or_else(|| cwd.to_string());
    let (files, counts) = match &against {
        None => (parse_status(&status), git(cwd, &["diff", "HEAD", "--numstat", "-z"]).or_else(|| git(cwd, &["diff", "--cached", "--numstat", "-z"])).map(|d| parse_numstat(&d)).unwrap_or_default()),
        Some((commit, _)) => {
            // The working tree against that commit, and what git does not track yet.
            let mut files = git(&root, &["diff", commit, "--name-status", "-z"]).map(|d| parse_name_status(&d)).unwrap_or_default();
            files.extend(parse_status(&status).into_iter().filter(|(s, _)| *s == '?'));
            (files, git(&root, &["diff", commit, "--numstat", "-z"]).map(|d| parse_numstat(&d)).unwrap_or_default())
        }
    };
    Ok(files.into_iter().map(|(status, path)| {
        let (added, removed) = match counts.get(&path) {
            Some(&(a, r)) => (a, r),
            // An untracked file: all its lines are new.
            None if status == '?' => (std::fs::read(std::path::Path::new(&root).join(&path)).ok().filter(|d| !d.contains(&0)).map(|d| d.split(|b| *b == b'\n').filter(|l| !l.is_empty()).count() as u32), Some(0)),
            None => (Some(0), Some(0)),
        };
        FileChange { status, path, added, removed }
    }).collect())
}

/// A file's diff against `base`, as lines (an untracked file's lines, all
/// added). At most `limit` lines.
pub fn file_diff(cwd: &str, change: &FileChange, base: DiffBase, limit: usize) -> Vec<String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"]).map(|d| String::from_utf8_lossy(&d).trim().to_string()).unwrap_or_else(|| cwd.to_string());
    if change.status == '?' {
        return match std::fs::read(std::path::Path::new(&root).join(&change.path)) {
            Ok(data) if data.contains(&0) => vec!["Binary file".into()],
            Ok(data) => String::from_utf8_lossy(&data).lines().take(limit).map(|l| format!("+{l}")).collect(),
            Err(_) => vec![],
        };
    }
    let against = base_commit(cwd, base).ok().flatten().map(|(commit, _)| commit).unwrap_or_else(|| "HEAD".to_string());
    let data = git(&root, &["diff", &against, "--no-color", "--", &change.path]).filter(|d| !d.is_empty())
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
/// The changed files against `base`, and the name of what they are
/// compared with (an upstream, main), read on a thread.
pub fn fetch_changes(cwd: String, base: DiffBase) -> Receiver<Result<(Vec<FileChange>, Option<String>), String>> {
    let (tx, rx) = channel();
    if is_demo(&cwd) {
        let _ = tx.send(Ok((DEMO_FILES.iter().map(|(status, path, added, removed, _)| FileChange { status: *status, path: path.to_string(), added: Some(*added), removed: Some(*removed) }).collect(), (base == DiffBase::Main).then(|| "main".to_string()))));
        return rx;
    }
    let cwd = crate::herdr::expand_home(&cwd);
    std::thread::spawn(move || {
        let named = base_commit(&cwd, base).ok().flatten().map(|(_, name)| name);
        let _ = tx.send(changed_files(&cwd, base).map(|files| (files, named)));
    });
    rx
}

pub fn fetch_diff(cwd: String, change: FileChange, base: DiffBase) -> Receiver<Vec<String>> {
    let (tx, rx) = channel();
    if is_demo(&cwd) {
        let lines = DEMO_FILES.iter().find(|f| f.1 == change.path).map(|f| f.4).unwrap_or_default();
        let _ = tx.send(lines.lines().map(str::to_string).collect());
        return rx;
    }
    let cwd = crate::herdr::expand_home(&cwd);
    std::thread::spawn(move || { let _ = tx.send(file_diff(&cwd, &change, base, 4000)); });
    rx
}

pub fn fetch_pull_request(cwd: String) -> Receiver<Option<PullRequest>> {
    let (tx, rx) = channel();
    if is_demo(&cwd) {
        let _ = tx.send(Some(PullRequest { number: 42, title: "Settings page with a theme toggle".into(), url: "https://github.com/guild/website/pull/42".into(), state: "OPEN".into(), draft: false }));
        return rx;
    }
    let cwd = crate::herdr::expand_home(&cwd);
    std::thread::spawn(move || { let _ = tx.send(pull_request(&cwd)); });
    rx
}

/// The demo's agents work in folders that do not exist: they show these
/// changes, the same for every one of them.
fn is_demo(cwd: &str) -> bool {
    cwd.starts_with("~/demo/")
}

/// Status, path, lines added and removed, and the file's diff.
const DEMO_FILES: [(char, &str, u32, u32, &str); 5] = [
    ('M', "src/pages/Settings.tsx", 38, 9, "@@ -1,9 +1,14 @@
 import { useState } from \"react\";
-import { Page } from \"../components/Page\";
+import { Page, Section } from \"../components/Page\";
+import { ThemeToggle } from \"../components/ThemeToggle\";
+import { saveSettings } from \"../api/user\";
 
 export function Settings() {
-  const [name, setName] = useState(\"\");
+  const [name, setName] = useState(user.name);
+  const [theme, setTheme] = useState(user.theme);
+  const [saving, setSaving] = useState(false);
 
   return (
     <Page title=\"Settings\">
@@ -24,6 +29,18 @@ export function Settings() {
         <input value={name} onChange={(e) => setName(e.target.value)} />
       </label>
+      <Section title=\"Appearance\">
+        <ThemeToggle value={theme} onChange={setTheme} />
+      </Section>
+      <button
+        disabled={saving}
+        onClick={async () => {
+          setSaving(true);
+          await saveSettings({ name, theme });
+          setSaving(false);
+        }}
+      >
+        {saving ? \"Saving…\" : \"Save\"}
+      </button>
     </Page>
   );
 }"),
    ('A', "src/components/ThemeToggle.tsx", 27, 0, "@@ -0,0 +1,27 @@
+type Theme = \"light\" | \"dark\" | \"system\";
+
+export function ThemeToggle({ value, onChange }: {
+  value: Theme;
+  onChange: (theme: Theme) => void;
+}) {
+  return (
+    <div role=\"radiogroup\" className=\"theme-toggle\">
+      {([\"light\", \"dark\", \"system\"] as Theme[]).map((theme) => (
+        <button
+          key={theme}
+          role=\"radio\"
+          aria-checked={value === theme}
+          onClick={() => onChange(theme)}
+        >
+          {theme}
+        </button>
+      ))}
+    </div>
+  );
+}"),
    ('M', "src/api/user.ts", 11, 2, "@@ -8,8 +8,17 @@ export async function getUser() {
   return res.json();
 }
 
-export async function saveName(name: string) {
-  await fetch(\"/api/user\", { method: \"PATCH\", body: JSON.stringify({ name }) });
+export async function saveSettings(settings: { name: string; theme: string }) {
+  const res = await fetch(\"/api/user\", {
+    method: \"PATCH\",
+    headers: { \"Content-Type\": \"application/json\" },
+    body: JSON.stringify(settings),
+  });
+  if (!res.ok) {
+    throw new Error(`Saving settings failed: ${res.status}`);
+  }
 }"),
    ('M', "src/styles/settings.css", 14, 3, "@@ -12,7 +12,18 @@
 .settings label {
   display: block;
-  margin-bottom: 8px;
+  margin-bottom: 12px;
 }
+
+.theme-toggle {
+  display: inline-flex;
+  gap: 4px;
+  padding: 4px;
+  border-radius: 8px;
+}
+
+.theme-toggle [aria-checked=\"true\"] {
+  background: var(--accent);
+}"),
    ('?', "tests/settings.spec.ts", 19, 0, "@@ -0,0 +1,19 @@
+import { test, expect } from \"@playwright/test\";
+
+test(\"saves the theme\", async ({ page }) => {
+  await page.goto(\"/settings\");
+  await page.getByRole(\"radio\", { name: \"dark\" }).click();
+  await page.getByRole(\"button\", { name: \"Save\" }).click();
+  await page.reload();
+  await expect(page.getByRole(\"radio\", { name: \"dark\" })).toBeChecked();
+});"),
];

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
    fn name_statuses() {
        assert_eq!(parse_name_status(b"M\0src/ui.rs\0R087\0old.rs\0new.rs\0A\0added.rs\0"), vec![('M', "src/ui.rs".into()), ('R', "new.rs".into()), ('A', "added.rs".into())]);
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
        let files = changed_files(&cwd, DiffBase::Uncommitted).unwrap();
        assert!(files.contains(&FileChange { status: 'M', path: "a.txt".into(), added: Some(2), removed: Some(1) }), "{files:?}");
        assert!(files.contains(&FileChange { status: '?', path: "b.txt".into(), added: Some(1), removed: Some(0) }), "{files:?}");
        let diff = file_diff(&cwd, &files.iter().find(|f| f.path == "a.txt").unwrap().clone(), DiffBase::Uncommitted, 100);
        assert!(diff[0].starts_with("@@") && diff.contains(&"-two".to_string()) && diff.contains(&"+three".to_string()), "{diff:?}");
        assert_eq!(file_diff(&cwd, &files.iter().find(|f| f.path == "b.txt").unwrap().clone(), DiffBase::Uncommitted, 100), vec!["+new".to_string()]);
        // A branch off main with a commit of its own: against main it shows
        // the committed file too; there is no upstream to compare with.
        run(&["branch", "-m", "main"]);
        run(&["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("c.txt"), "c\n").unwrap();
        run(&["add", "c.txt"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "c"]);
        let uncommitted: Vec<String> = changed_files(&cwd, DiffBase::Uncommitted).unwrap().into_iter().map(|f| f.path).collect();
        let branch: Vec<String> = changed_files(&cwd, DiffBase::Main).unwrap().into_iter().map(|f| f.path).collect();
        assert!(!uncommitted.contains(&"c.txt".to_string()) && branch.contains(&"c.txt".to_string()) && branch.contains(&"a.txt".to_string()), "{branch:?}");
        assert!(changed_files(&cwd, DiffBase::Unpushed).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
