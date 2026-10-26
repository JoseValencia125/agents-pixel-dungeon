//! A project's evidence as one web page (FB-074): every image the members
//! left — the ones their tasks asked for, the verifiers' captures, the step
//! captures of a member still at work — grouped by quest and task, newest
//! first, with the reference each task named beside them and where each
//! version stands. The service writes it again as work comes in; the
//! designer keeps it open in a browser instead of someone building it by hand.

use crate::domain::*;
use std::path::{Path, PathBuf};

pub fn gallery_path(project_id: &str) -> PathBuf { crate::paths::home().join("gallery").join(project_id).join("index.html") }

/// The images a task's text names as references: paths to image files,
/// in the repository or absolute (`~/` allowed), that exist.
pub fn references(repo: &Path, text: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = vec![];
    for w in text.split(|c: char| c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';')) {
        let w = w.trim_end_matches(['.', ':']).trim_start_matches("./");
        if !is_image(Path::new(w)) || w.contains("://") { continue; }
        let path = if w.starts_with('/') || w.starts_with('~') { PathBuf::from(crate::policy::expand_home(w)) } else { repo.join(w) };
        if path.is_file() && !out.contains(&path) { out.push(path); }
    }
    out
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp"))
}

/// The images an attempt shows: its verifiers' and its task's evidence, or,
/// while it works, the step captures in its scratch folder.
fn shown(a: &Attempt) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = a.verifier_results.iter().flat_map(|r| r.evidence.iter().map(PathBuf::from)).collect();
    if out.is_empty() && a.state.is_active() {
        let live = crate::paths::scratch_dir(&a.id).join("evidence");
        let mut files: Vec<PathBuf> = std::fs::read_dir(&live).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        files.retain(|p| p.is_file() && is_image(p));
        files.sort();
        out = files;
    }
    out.retain(|p| p.is_file());
    out.dedup();
    out
}

/// Where a version stands, in the designer's words.
fn standing(a: &Attempt, t: &Task) -> (&'static str, &'static str) {
    let accepted = t.winner_attempt_id.as_deref() == Some(a.id.as_str()) || (a.state == AttemptState::Passed && matches!(t.state, TaskState::Accepted | TaskState::Integrating | TaskState::Completed));
    if accepted { return ("accepted", "ok"); }
    match a.state {
        AttemptState::Submitted | AttemptState::Verifying => ("waiting for a decision", "wait"),
        AttemptState::Passed => ("passed", "ok"),
        AttemptState::Failed | AttemptState::TimedOut | AttemptState::Lost => ("turned down", "no"),
        AttemptState::Cancelled => ("cancelled", "no"),
        _ => ("at work", "live"),
    }
}

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

/// A `file://` link that survives spaces and other characters in the path.
pub fn file_url(p: &Path) -> String {
    let mut out = String::from("file://");
    for b in p.display().to_string().bytes() {
        if b.is_ascii_alphanumeric() || b"/._-~".contains(&b) { out.push(b as char); } else { out.push_str(&format!("%{b:02X}")); }
    }
    out
}

/// The page for one project; `None` when it has nothing to show yet.
pub fn render(snap: &Snapshot, project_id: &str) -> Option<String> {
    let project = snap.projects.iter().find(|p| p.id == project_id)?;
    let repo = Path::new(&project.repo_path);
    let mut quests: Vec<&Quest> = snap.quests.iter().filter(|q| q.project_id == project_id).collect();
    quests.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut body = String::new();
    let mut images = 0;
    for q in quests {
        let mut tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == q.id).collect();
        tasks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        let mut section = String::new();
        for t in tasks {
            let mut attempts: Vec<&Attempt> = snap.attempts.iter().filter(|a| a.task_id == t.id && a.kind == AttemptKind::Work).collect();
            attempts.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            let versions: Vec<(&Attempt, Vec<PathBuf>)> = attempts.into_iter().map(|a| (a, shown(a))).filter(|(_, s)| !s.is_empty()).collect();
            if versions.is_empty() { continue; }
            section.push_str(&format!("<article><h3>{} <span class=\"state\">{:?}</span></h3>", esc(&t.title), t.state));
            let refs = references(repo, &t.instructions);
            if !refs.is_empty() {
                section.push_str("<div class=\"version ref\"><div class=\"meta\"><b>Reference</b></div><div class=\"strip\">");
                for r in &refs { section.push_str(&format!("<a href=\"{u}\" target=\"_blank\"><img loading=\"lazy\" src=\"{u}\" title=\"{n}\"></a>", u = file_url(r), n = esc(&r.display().to_string()))); }
                section.push_str("</div></div>");
            }
            for (a, files) in versions {
                let who = snap.members.iter().find(|m| m.id == a.member_id).map(|m| m.name.as_str()).unwrap_or(a.member_id.as_str());
                let (label, class) = standing(a, t);
                section.push_str(&format!("<div class=\"version\"><div class=\"meta\"><span class=\"badge {class}\">{label}</span> {} · {}</div><div class=\"strip\">", esc(who), format!("{} UTC", esc(a.created_at.get(..16).unwrap_or(&a.created_at)).replace('T', " "))));
                for f in &files {
                    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    section.push_str(&format!("<a href=\"{u}\" target=\"_blank\"><img loading=\"lazy\" src=\"{u}\" title=\"{n}\"></a>", u = file_url(f), n = esc(name)));
                    images += 1;
                }
                section.push_str("</div>");
                if let Some(why) = a.failure.as_deref().filter(|_| class == "no") { section.push_str(&format!("<p class=\"why\">{}</p>", esc(why.chars().take(300).collect::<String>().as_str()))); }
                section.push_str("</div>");
            }
            section.push_str("</article>");
        }
        if !section.is_empty() {
            body.push_str(&format!("<section><h2>{} <span class=\"state\">{:?}</span></h2>{section}</section>", esc(&q.title), q.state));
        }
    }
    if images == 0 { return None; }
    Some(format!(r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="30">
<title>{name} · gallery</title>
<style>
:root {{ --bg: #f6f5f2; --fg: #1d1c1a; --muted: #6b6862; --card: #fff; --line: #e3e0da; --ok: #2f7d4f; --no: #a33a2c; --wait: #a86b00; --live: #2f5fa8; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg: #161514; --fg: #ecebe8; --muted: #9b978f; --card: #201f1d; --line: #34322e; --ok: #6cc28f; --no: #e07b6c; --wait: #e0a94a; --live: #7aa6e8; }} }}
body {{ margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 14px/1.4 system-ui, sans-serif; }}
h1 {{ font-size: 20px; margin: 0 0 4px; }} h2 {{ font-size: 17px; margin: 28px 0 8px; }} h3 {{ font-size: 15px; margin: 0 0 8px; }}
.sub, .meta, .state, .why {{ color: var(--muted); }} .state {{ font-weight: normal; font-size: 12px; }}
article {{ background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 12px; margin: 10px 0; }}
.version {{ margin: 8px 0; }} .ref .strip img {{ outline: 2px dashed var(--muted); }}
.strip {{ display: flex; gap: 8px; overflow-x: auto; padding: 4px 0; }}
.strip img {{ height: 180px; max-width: 320px; object-fit: contain; border-radius: 4px; background: #0002; }}
.badge {{ font-size: 12px; font-weight: 600; }} .badge.ok {{ color: var(--ok); }} .badge.no {{ color: var(--no); }} .badge.wait {{ color: var(--wait); }} .badge.live {{ color: var(--live); }}
.why {{ font-size: 12px; margin: 4px 0 0; }}
</style></head><body>
<h1>{name}</h1><p class="sub">Every version the members showed, newest first. Updated by the guild as work comes in.</p>
{body}
</body></html>
"#, name = esc(&project.name)))
}

/// Write the page; its path, or `None` when there is nothing to show.
pub fn write(snap: &Snapshot, project_id: &str) -> std::io::Result<Option<PathBuf>> {
    let Some(html) = render(snap, project_id) else { return Ok(None) };
    let path = gallery_path(project_id);
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    if std::fs::read_to_string(&path).ok().as_deref() != Some(html.as_str()) { std::fs::write(&path, html)?; }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_named_in_the_task_is_found_where_it_is() {
        let dir = std::env::temp_dir().join(format!("gallery-{}", new_id()));
        std::fs::create_dir_all(dir.join("docs/visual_ref")).unwrap();
        std::fs::write(dir.join("docs/visual_ref/day.png"), "png").unwrap();
        let found = references(&dir, "Match `docs/visual_ref/day.png`. Also docs/visual_ref/missing.png and tools/run.sh.");
        assert_eq!(found, vec![dir.join("docs/visual_ref/day.png")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_with_spaces_is_a_working_link() {
        assert_eq!(file_url(Path::new("/Users/a/Application Support/x#1.png")), "file:///Users/a/Application%20Support/x%231.png");
    }
}
