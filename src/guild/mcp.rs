//! The guild as an MCP server (stdio, one JSON-RPC message per line), so a
//! Claude Code session can orchestrate it: plan a quest, hand its tasks to
//! the guild's members (Kiro, Codex…), wait for them, read their candidates
//! and decide. The members work in their own worktrees; nothing reaches the
//! user's checkout, and integrating only touches the quest's branch.

use crate::domain::*;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

pub const PROTOCOL: &str = "2025-06-18";

/// How the server reaches the service: a method and its params.
pub type Call<'a> = &'a dyn Fn(&str, Value) -> Result<Value, String>;

/// Serve on stdin/stdout until stdin closes.
pub fn serve_stdio(call: Call) {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() { continue; }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, call, &std::thread::sleep),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})),
        };
        if let Some(reply) = reply {
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}

/// One message in, its answer out (None for notifications).
pub fn handle(msg: &Value, call: Call, sleep: &dyn Fn(Duration)) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "guild", "version": crate::service::SERVICE_VERSION},
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            let (text, is_error) = match run_tool(name, &args, call, sleep) { Ok(t) => (t, false), Err(e) => (e, true) };
            Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        other => Err(json!({"code": -32601, "message": format!("unknown method {other}")})),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

const INSTRUCTIONS: &str = "The guild runs coding agents (its members, e.g. Kiro) in isolated git worktrees. You are its delegate: you talk with the user and lead the work: break the user's request into small tasks with verifiable acceptance criteria and dependencies, start them as a quest with guild_start_quest, then call guild_wait until something needs you. For a candidate ready, read it with guild_diff and accept (integrate) or reject with a reason the member can act on; answer permissions with guild_permission; retry or cancel failed tasks. Integrating merges into the quest's branch guild/quest/<id>, never into main: tell the user the branch when the quest completes. Visual work (art, scenes, look): every task names the reference image to match (its path), the exact files to reuse (paths, or `git show <commit>:<path>` when they were deleted), asks the member to build it inside the game's real scene and compare it side by side with the reference, and lists the images to leave in `evidence`; when the direction is uncertain, try it first as a `spike` (short, never integrated). Folders outside the repository the members must read (references, captures) go in the quest's `readPaths`. A note reaches a working member at once; restart a task to move it onto a member's new harness or model.";

fn tools() -> Value {
    let task_id = json!({"type": "string", "description": "A task id as guild_status or guild_start_quest gives it"});
    json!([
        {"name": "guild_status", "description": "The guild at a glance: projects, members (role, harness, busy), quests with their tasks and what needs a decision.",
         "inputSchema": {"type": "object", "properties": {"questId": {"type": "string", "description": "Only this quest"}}}},
        {"name": "guild_start_quest", "description": "Create a quest with its tasks and start it. Members take the tasks by role (or the member named) in their own worktrees; dependencies run in order, the rest in parallel.",
         "inputSchema": {"type": "object", "required": ["tasks"], "properties": {
            "projectId": {"type": "string", "description": "Defaults to the guild's first project"},
            "title": {"type": "string"},
            "description": {"type": "string"},
            "questId": {"type": "string", "description": "Add these tasks to this quest instead of creating one (a completed quest opens again)"},
            "readPaths": {"type": "array", "items": {"type": "string"}, "description": "Folders outside the repository its members may read without asking (absolute or ~/…)"},
            "tasks": {"type": "array", "items": {"type": "object", "required": ["id", "title", "instructions"], "properties": {
                "id": {"type": "string", "description": "Short id, unique in this quest (t1, t2…), for dependsOn"},
                "title": {"type": "string"},
                "instructions": {"type": "string", "description": "Everything the member needs: it sees only this task, not the conversation"},
                "acceptance": {"type": "array", "items": {"type": "string"}},
                "role": {"type": "string", "description": "A role some member holds; defaults to implementer"},
                "member": {"type": "string", "description": "A member's name or id, to pin the task to it"},
                "dependsOn": {"type": "array", "items": {"type": "string"}, "description": "Ids of this plan, or a task id of another quest of the project: its quest's branch is brought in before the task starts"},
                "candidates": {"type": "integer", "description": "Run a race of this many members (default 1)"},
                "timeoutMinutes": {"type": "integer", "description": "How long an attempt may work on it (default the guild's, 30)"},
                "evidence": {"type": "array", "items": {"type": "string"}, "description": "Images the member must leave in its worktree (paths relative to it, * in the name): kept with the candidate even if git ignores them, shown to you and the user"},
                "spike": {"type": "boolean", "description": "A short trial (30 min by default) whose captures and report are the result: accepting it never integrates it"}}}}}}},
        {"name": "guild_ack", "description": "Say you have a wait the guild flagged (guild_status lists them under `waiting`): no more notices or alerts for it, it stays listed until it ends. id: the wait's id or its task's.",
         "inputSchema": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}}},
        {"name": "guild_wait", "description": "Block until a task of the quest (without questId, of any quest under way) needs you (candidate ready, permission asked, failed, blocked, or something that has waited too long and nobody moves) or the quest ends, or until the timeout. Returns the quest's status.",
         "inputSchema": {"type": "object", "properties": {"questId": {"type": "string", "description": "Without it, every quest under way"}, "timeoutSeconds": {"type": "integer", "description": "Default 300, at most 540"}}}},
        {"name": "guild_task", "description": "One task in full: instructions, notes, attempts, failures, verifier results, candidate size, pending permission (whole command, file and whether it is inside the worktree) and the end of the member's transcript. full: true gives the member's whole last reply and every verifier's whole output.",
         "inputSchema": {"type": "object", "required": ["taskId"], "properties": {"taskId": task_id, "full": {"type": "boolean"}}}},
        {"name": "guild_note", "description": "Add to a task that already started: a member at work is stopped and handed the note at once, in the same session, to correct course (later: true waits until its turn ends; the answer says which it was, and events note.delivered / note.deferred show when it arrived); a retry and the orchestrator read it with the instructions. With questId instead of taskId, a note for the whole quest: every open task reads it (members at work at once, the rest when they start, and tasks created later) until withdrawn (withdraw: its number, from 1).",
         "inputSchema": {"type": "object", "required": ["text"], "properties": {"taskId": task_id, "questId": {"type": "string"}, "text": {"type": "string"}, "later": {"type": "boolean"}, "withdraw": {"type": "integer"}}}},
        {"name": "guild_diff", "description": "The diff of a task's latest candidate.",
         "inputSchema": {"type": "object", "required": ["taskId"], "properties": {"taskId": task_id}}},
        {"name": "guild_decide", "description": "Decide on a task: accept (integrates the candidate into the quest's branch; a spike is completed without integrating), reject (with a reason; the member retries if attempts remain), retry (a candidate still waiting is rejected with the reason; a task at work starts again on its member's current harness and model, from what it wrote, without costing an attempt), or cancel (in one step; replacedBy: the tasks that waited for it wait for that task instead). accept with attemptId and override: true takes a verified attempt that was failed (by the orchestrator, a judge or a reject).",
         "inputSchema": {"type": "object", "required": ["taskId", "decision"], "properties": {"taskId": task_id, "decision": {"type": "string", "enum": ["accept", "reject", "retry", "cancel", "integrate"]}, "reason": {"type": "string"}, "attemptId": {"type": "string"}, "override": {"type": "boolean"}, "replacedBy": {"type": "string", "description": "With cancel: a task id of the same quest"}, "timeoutMinutes": {"type": "integer", "description": "With retry: a new time limit for the task, in minutes"}, "instructions": {"type": "string", "description": "With retry: new instructions for the task"}}}},
        {"name": "guild_permission", "description": "Answer a member's pending permission request with one of its option ids (from guild_task).",
         "inputSchema": {"type": "object", "required": ["taskId", "optionId"], "properties": {"taskId": task_id, "optionId": {"type": "string"}}}},
    ])
}

fn snapshot(call: Call) -> Result<Snapshot, String> {
    serde_json::from_value(call("guild.snapshot", json!({}))?).map_err(|e| e.to_string())
}

/// The attempt a task's line is about: the one at work or waiting for an
/// answer if there is one, else the latest (FB-073: a member kept waiting
/// showed in its own line and not in its task's).
fn latest_attempt<'a>(snap: &'a Snapshot, task_id: &str) -> Option<&'a Attempt> {
    let of_task = || snap.attempts.iter().filter(|a| a.task_id == task_id);
    of_task().filter(|a| matches!(a.state, AttemptState::WaitingInput))
        .chain(of_task().filter(|a| matches!(a.state, AttemptState::Provisioning | AttemptState::Running)))
        .next()
        .or_else(|| of_task().max_by(|a, b| a.created_at.cmp(&b.created_at)))
}

/// What a task wants from the orchestrator, if anything.
fn needs(snap: &Snapshot, t: &Task) -> Option<&'static str> {
    let a = latest_attempt(snap, &t.id);
    // The guild's orchestrator is on it: nothing for anyone else yet.
    if a.is_some_and(|a| a.deciding) { return None; }
    match (t.state, a.map(|a| a.state)) {
        // Git or the disk failed integrating it: the guild tries again, and `integrate` does it now (FB-077).
        (TaskState::Accepted, _) if t.block_reason.as_deref().is_some_and(|r| r.starts_with("merge failed")) => Some("integration failed: the guild tries again, or guild_decide integrate does it now"),
        (TaskState::Completed | TaskState::Cancelled | TaskState::Accepted | TaskState::Integrating, _) => None,
        (_, Some(AttemptState::WaitingInput)) => Some("permission"),
        // Submitted once its verifiers ran (they run in Verifying), Passed when they passed.
        (_, Some(AttemptState::Submitted | AttemptState::Passed)) => Some("candidate ready: read guild_diff, then accept or reject"),
        (TaskState::Failed, _) => Some("failed: retry or cancel"),
        (TaskState::Blocked, _) => Some("blocked: retry or cancel"),
        _ => None,
    }
}

/// Why a Ready task has not started: its member does not exist, is off,
/// is busy, or its harness is unavailable; or nobody has its role (FB-068).
fn why_waiting(snap: &Snapshot, t: &Task) -> Option<String> {
    if t.state != TaskState::Ready { return None; }
    let busy = |m: &Member| snap.attempts.iter().any(|a| a.member_id == m.id && a.state.is_active());
    let usable = |m: &Member| m.enabled && m.roles.contains(&t.role);
    if let Some(id) = &t.member_id {
        let Some(m) = snap.members.iter().find(|m| &m.id == id) else { return Some(format!("its member {id} does not exist (`task set --member`)")) };
        if !m.enabled { return Some(format!("its member {} is disabled", m.name)); }
        if !m.roles.contains(&t.role) { return Some(format!("its member {} does not have the role {}", m.name, t.role)); }
        if snap.harnesses.iter().any(|h| h.kind == m.harness && !h.available) { return Some(format!("{}'s harness {} is unavailable", m.name, m.harness)); }
        if snap.paused_members.iter().any(|p| p.member_id == m.id) { return Some(format!("its member {} is paused: its harness hit a usage limit", m.name)); }
        return busy(m).then(|| format!("its member {} is busy with another task", m.name));
    }
    let with_role: Vec<&Member> = snap.members.iter().filter(|m| usable(m)).collect();
    if with_role.is_empty() { return Some(format!("no enabled member has the role {}", t.role)); }
    with_role.iter().all(|m| busy(m) || snap.paused_members.iter().any(|p| p.member_id == m.id)).then(|| format!("every member with the role {} is busy or paused", t.role))
}

pub fn task_line(snap: &Snapshot, t: &Task) -> String {
    let a = latest_attempt(snap, &t.id);
    let who = a.and_then(|a| snap.members.iter().find(|m| m.id == a.member_id)).map(|m| format!(" · {} ({})", m.name, m.harness)).unwrap_or_default();
    // A member waiting for an answer says so in the task's own line (FB-073).
    let waiting = a.filter(|a| a.state == AttemptState::WaitingInput).map(|a| format!(" (waiting-input, attempt {})", a.id)).unwrap_or_default();
    let mut s = format!("- {} [{}] {:?}{waiting}{who}: {}", t.id, t.role, t.state, t.title);
    if let Some(a) = a {
        if !a.summary.is_empty() { s.push_str(&format!(" — {}", a.summary.chars().take(160).collect::<String>())); }
        if let Some(f) = &a.failure { s.push_str(&format!(" — failure: {f}")); }
    }
    if let Some(r) = &t.block_reason { s.push_str(&format!(" — {r}")); }
    if let Some(w) = why_waiting(snap, t) { s.push_str(&format!(" — waiting: {w}")); }
    if let Some(n) = needs(snap, t) { s.push_str(&format!("  ⟵ NEEDS YOU: {n}")); }
    // How long a member has been kept waiting, so the oldest is answered first.
    if let Some(p) = a.and_then(|a| a.pending_permission.as_ref()) {
        let waited = chrono::DateTime::parse_from_rfc3339(&p.asked_at).ok().map(|at| (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds().max(0));
        if let Some(secs) = waited { s.push_str(&format!(" (waiting {}m{:02}s: {})", secs / 60, secs % 60, p.title.lines().next().unwrap_or("").chars().take(80).collect::<String>())); }
    }
    if let Some(a) = a {
        // Past two thirds of its time: split the task, or let it be.
        if a.state.is_active() && a.limit_seconds > 0 && a.worked_seconds * 3 >= a.limit_seconds * 2 {
            s.push_str(&format!("  ⚠ worked {}m of its {}m", a.worked_seconds / 60, a.limit_seconds / 60));
        }
        if a.deciding { s.push_str("  (the guild's orchestrator is deciding)"); }
        if let Some(why) = &a.escalation { s.push_str(&format!(" — escalated by the orchestrator: {why}")); }
    }
    s
}

/// Attempts of the last day lost to a cause outside the task (a usage limit, a time limit, a lost harness), in one line.
fn external_failures(snap: &Snapshot) -> String {
    let day = chrono::Utc::now() - chrono::Duration::hours(24);
    let mut counts: std::collections::BTreeMap<&str, u32> = Default::default();
    for a in snap.attempts.iter().filter(|a| matches!(a.state, AttemptState::Failed | AttemptState::TimedOut | AttemptState::Lost)) {
        let recent = a.finished_at.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_some_and(|t| t.with_timezone(&chrono::Utc) > day);
        let class = if a.state == AttemptState::TimedOut { "timeout" } else { crate::service::failure_class(a.failure.as_deref().unwrap_or("")) };
        if recent && class != "work" { *counts.entry(class).or_default() += 1; }
    }
    if counts.is_empty() { return String::new(); }
    format!("attempts lost to causes outside the task, last 24 h: {}\n", counts.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", "))
}

/// How long ago, in the units a person reads: "12 min", "3 h", "2 days".
pub fn ago(since: &str) -> String {
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(since) else { return "?".into() };
    let secs = (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds().max(0);
    match secs { 0..=119 => format!("{secs} s"), 120..=7199 => format!("{} min", secs / 60), 7200..=172_799 => format!("{} h", secs / 3600), _ => format!("{} days", secs / 86_400) }
}

/// What waits and nobody moves (the sentinel's stalls), oldest first; the ones paused on purpose
/// in one line each at the end. `quest` limits it to one quest.
pub fn stall_lines(snap: &Snapshot, quest: Option<&str>) -> String {
    let mut stalls: Vec<&crate::sentinel::Stall> = snap.stalls.iter().filter(|s| quest.is_none_or(|q| s.quest_id == q)).collect();
    if stalls.is_empty() { return String::new(); }
    stalls.sort_by(|a, b| a.since.cmp(&b.since));
    let mut out = String::new();
    let live: Vec<_> = stalls.iter().filter(|s| !s.dormant).collect();
    if !live.is_empty() {
        out.push_str(&format!("waiting ({}):\n", live.len()));
        for s in live {
            let what = if s.task_id.is_empty() { s.quest_id.clone() } else { s.task_id.clone() };
            out.push_str(&format!("- {what} · {} for {} · {} — {}{}{}\n", s.kind.as_str().replace('_', " "), ago(&s.since), s.owner, s.reason.trim(),
                if s.acked { " (someone has it)" } else { "" }, if s.alerted && !s.acked { " ⚠ the user was told" } else { "" }));
        }
    }
    for s in stalls.iter().filter(|s| s.dormant) { out.push_str(&format!("  dormant, its quest is paused: {} {} — {}\n", s.quest_id, ago(&s.since), s.reason.trim())); }
    out
}

fn quest_report(snap: &Snapshot, q: &Quest) -> String {
    let tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == q.id).collect();
    let done = tasks.iter().filter(|t| t.state == TaskState::Completed).count();
    let mut s = format!("quest {} {:?} · {}/{} done · branch {} · {}\n", q.id, q.state, done, tasks.len(), q.integration_ref.trim_start_matches("refs/heads/"), q.title);
    for t in tasks { s.push_str(&task_line(snap, t)); s.push('\n'); }
    for (i, n) in q.notes.iter().enumerate() { s.push_str(&format!("  quest note {}: {n}\n", i + 1)); }
    // Tasks that will clash at integration unless one waits for the other (FB-068).
    for o in snap.overlaps.iter().filter(|o| o.quest_id == q.id) {
        let title = |id: &str| snap.tasks.iter().find(|t| t.id == id).map(|t| t.title.clone()).unwrap_or_else(|| id.to_string());
        s.push_str(&format!("  ⚠ «{}» and «{}» both change {}: put one after the other (`task set {} --depends {}`), or list a file that only grows under [merge] union in .guild/guild.toml\n", title(&o.tasks[0]), title(&o.tasks[1]), o.files.join(", "), o.tasks[1], o.tasks[0]));
    }
    s.push_str(&doc_line(snap, &q.project_id));
    s
}

/// What the guild settled that the design's Claude Doc does not show yet.
fn doc_line(snap: &Snapshot, project_id: &str) -> String {
    let Some(d) = snap.designs.iter().find(|d| d.project_id == project_id) else { return String::new() };
    let behind = crate::design::doc_behind(d, &snap.questions, &snap.design_decisions);
    match (&d.doc, behind.is_empty()) {
        (Some(doc), false) => format!("the design doc {} is behind the guild: {} — write them into the doc, then `design {project_id} synced <rev>`\n", doc.url, behind.join("; ")),
        _ => String::new(),
    }
}

fn find_member<'a>(snap: &'a Snapshot, who: &str) -> Option<&'a Member> {
    snap.members.iter().find(|m| m.id == who || m.name.eq_ignore_ascii_case(who))
}

/// What a visual task lacks to be done right the first time (FB-060): a
/// reference image, the files to reuse, the images to leave.
fn visual_gap(t: &Value) -> Option<String> {
    let role = t["role"].as_str().unwrap_or("");
    let text = format!("{} {}", t["title"].as_str().unwrap_or(""), t["instructions"].as_str().unwrap_or("")).to_lowercase();
    let visual = role.contains("art") || ["visual", "look", "shader", "sprite", "escena", "scene", "captura", "capture"].iter().any(|w| text.contains(w));
    if !visual { return None; }
    let mut missing = vec![];
    if ![".png", ".jpg", ".jpeg", ".webp"].iter().any(|e| text.contains(e)) { missing.push("a reference image (its path)"); }
    if !(text.contains('/') && [".gd", ".tscn", ".gdshader", ".glsl", ".rs", ".ts", ".py", "git show"].iter().any(|e| text.contains(e))) { missing.push("the exact files to reuse"); }
    if t["evidence"].as_array().is_none_or(|e| e.is_empty()) { missing.push("evidence images to leave"); }
    (!missing.is_empty()).then(|| format!("{} looks like visual work without {}: tasks that named them got it right the first time", t["id"].as_str().unwrap_or("?"), missing.join(", ")))
}

/// One tool, as `tools/call` runs it; also `guild tool <name>` on the command line.
pub fn run_tool(name: &str, args: &Value, call: Call, sleep: &dyn Fn(Duration)) -> Result<String, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let need = |k: &str| s(k).filter(|v| !v.trim().is_empty()).ok_or_else(|| format!("missing {k}"));
    match name {
        "guild_status" => {
            let snap = snapshot(call)?;
            let mut out = String::new();
            if let Some(free) = snap.disk_low {
                out.push_str(&format!("⚠ disk: {:.1} GB free — no work starts until there is room. Finished worktrees are removed by themselves (`guild gc` now); tell the user if it stays low.\n", free as f64 / (1u64 << 30) as f64));
            }
            if let Some(note) = &snap.disk_note { out.push_str(&format!("⚠ disk, what takes the room: {note}\n")); }
            out.push_str(&stall_lines(&snap, s("questId").as_deref()));
            out.push_str(&external_failures(&snap));
            for p in &snap.projects {
                out.push_str(&format!("project {} · {} · {}\n", p.id, p.name, p.repo_path));
                let gallery = crate::gallery::gallery_path(&p.id);
                if gallery.exists() { out.push_str(&format!("  evidence gallery (give the user this link): {}\n", crate::gallery::file_url(&gallery))); }
                out.push_str(&doc_line(&snap, &p.id));
            }
            let busy: Vec<&str> = snap.attempts.iter().filter(|a| a.state.is_active()).map(|a| a.member_id.as_str()).collect();
            out.push_str("members:\n");
            for m in snap.members.iter().filter(|m| m.enabled) {
                out.push_str(&format!("- {} [{}] ({} · {}) roles {} · {}\n", m.name, m.id, m.harness, m.model_id.as_deref().unwrap_or("default model"), m.roles.join(", "), if let Some(p) = snap.paused_members.iter().find(|p| p.member_id == m.id) { format!("paused until {} (its harness hit a usage limit; no attempt was spent)", p.until) } else if busy.contains(&m.id.as_str()) { "busy".into() } else { "free".into() }));
            }
            if snap.members.iter().all(|m| !m.enabled) { out.push_str("- none: add members in the guild window or with `guild member add`\n"); }
            // Who turned a member on or off, so a change is never a mystery.
            for m in &snap.members {
                if let Some(sw) = &m.switched { out.push_str(&format!("  {} was {} by {} at {}\n", m.name, if sw.enabled { "enabled" } else { "disabled" }, sw.by, sw.at)); }
            }
            let wanted = s("questId");
            for q in snap.quests.iter().filter(|q| wanted.as_ref().map_or(!matches!(q.state, QuestState::Completed | QuestState::Cancelled), |w| &q.id == w)) {
                out.push_str(&quest_report(&snap, q));
            }
            Ok(out)
        }
        "guild_start_quest" => {
            let snap = snapshot(call)?;
            let project = match s("projectId") { Some(p) => p, None => snap.projects.first().map(|p| p.id.clone()).ok_or("the guild has no project yet")? };
            let tasks = args.get("tasks").and_then(Value::as_array).filter(|t| !t.is_empty()).ok_or("missing tasks")?;
            let reads = args.get("readPaths").cloned().filter(Value::is_array);
            let quest_id = match s("questId") {
                Some(id) => {
                    let q = snap.quests.iter().find(|q| q.id == id).ok_or_else(|| format!("no quest {id}"))?;
                    if let Some(r) = &reads { call("quest.update", json!({"questId": q.id, "readPaths": r}))?; }
                    q.id.clone()
                }
                None => call("quest.create", json!({"projectId": project, "title": need("title")?, "description": s("description").unwrap_or_default(), "readPaths": reads}))?["id"].as_str().unwrap_or_default().to_string(),
            };
            // Task ids are global: the plan's own get the quest's suffix.
            let suffix = &quest_id[quest_id.len().saturating_sub(8)..];
            let local: Vec<String> = tasks.iter().filter_map(|t| t["id"].as_str().map(str::to_string)).collect();
            let id = |t: &str| format!("{suffix}-{t}");
            for t in tasks {
                let tid = t["id"].as_str().ok_or("every task needs an id")?;
                let deps: Vec<String> = t["dependsOn"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|d| if local.iter().any(|l| l == d) { id(d) } else { d.to_string() }).collect();
                let member = t["member"].as_str().map(|w| find_member(&snap, w).map(|m| m.id.clone()).ok_or_else(|| format!("no member {w}"))).transpose()?;
                let mut params = json!({"questId": quest_id, "id": id(tid), "title": t["title"], "instructions": t["instructions"], "acceptance": t["acceptance"], "dependsOn": deps, "memberId": member});
                // A pinned member's own role, else the one asked, else implementer.
                let role = t["role"].as_str().map(str::to_string).or_else(|| member.as_ref().and_then(|m| find_member(&snap, m)).and_then(|m| m.roles.first().cloned()));
                if let Some(role) = role { params["role"] = json!(role); }
                if let Some(n) = t["candidates"].as_u64().filter(|n| *n > 1) { params["candidates"] = json!(n); }
                if let Some(m) = t["timeoutMinutes"].as_u64() { params["timeoutMinutes"] = json!(m); }
                if t["evidence"].is_array() { params["evidence"] = t["evidence"].clone(); }
                if t["spike"].as_bool() == Some(true) { params["spike"] = json!(true); }
                call("task.create", params)?;
            }
            let fresh = snapshot(call)?;
            // A new quest starts; one that existed starts only if it had not.
            if s("questId").is_none() || fresh.quests.iter().any(|q| q.id == quest_id && matches!(q.state, QuestState::Draft | QuestState::Ready | QuestState::Paused)) { call("quest.start", json!({"questId": quest_id}))?; }
            let warnings: String = tasks.iter().filter_map(|t| visual_gap(t)).map(|w| format!("warning: {w}\n")).collect();
            let snap = snapshot(call)?;
            Ok(format!("started. task ids: {}\n{warnings}{}", local.iter().map(|l| id(l)).collect::<Vec<_>>().join(", "), quest_report(&snap, snap.quests.iter().find(|q| q.id == quest_id).ok_or("quest vanished")?)))
        }
        "guild_wait" => {
            // Without a quest, every quest under way (FB-041: it failed only once the wait was over).
            let wanted = s("questId");
            let timeout = Duration::from_secs(args["timeoutSeconds"].as_u64().unwrap_or(300).clamp(5, 540));
            let started = Instant::now();
            let mut watched: Vec<String> = vec![];
            loop {
                let snap = snapshot(call)?;
                if watched.is_empty() {
                    watched = match &wanted {
                        Some(id) => vec![snap.quests.iter().find(|q| &q.id == id).ok_or_else(|| format!("no quest {id}"))?.id.clone()],
                        None => snap.quests.iter().filter(|q| matches!(q.state, QuestState::Running | QuestState::Paused | QuestState::Blocked)).map(|q| q.id.clone()).collect(),
                    };
                    if watched.is_empty() { return Err("no quest under way: start one, or name it with questId".into()); }
                }
                let quests: Vec<&Quest> = snap.quests.iter().filter(|q| watched.contains(&q.id)).collect();
                let ended = quests.iter().all(|q| matches!(q.state, QuestState::Completed | QuestState::Cancelled));
                let waiting = snap.tasks.iter().any(|t| watched.contains(&t.quest_id) && needs(&snap, t).is_some());
                // A wait that lasts: the sentinel has already tried and sent a notice, and nobody has it.
                let stalled = snap.stalls.iter().any(|s| watched.contains(&s.quest_id) && s.notices > 0 && !s.acked && !s.dormant);
                if ended || waiting || stalled || started.elapsed() >= timeout {
                    let why = if ended { if quests.len() == 1 { "the quest ended" } else { "the quests ended" } } else if waiting { "something needs you" } else if stalled { "something has waited too long: guild_decide, or guild_ack if you have it" } else { "timeout, still working" };
                    let report: String = stall_lines(&snap, None) + &quests.iter().map(|q| quest_report(&snap, q)).collect::<String>();
                    return Ok(format!("{why}\n{report}"));
                }
                sleep(Duration::from_secs(3));
            }
        }
        "guild_task" => {
            let snap = snapshot(call)?;
            let task_id = need("taskId")?;
            let t = snap.tasks.iter().find(|t| t.id == task_id).ok_or_else(|| format!("no task {task_id}"))?;
            let full = args.get("full").and_then(Value::as_bool).unwrap_or(false);
            let mut out = task_line(&snap, t);
            out.push_str(&format!("\ninstructions: {}\n", t.instructions));
            for n in &t.notes { out.push_str(&format!("note: {n}\n")); }
            for ac in &t.acceptance { out.push_str(&format!("acceptance {}: {}\n", ac.id, ac.text)); }
            if !t.granted.programs.is_empty() { out.push_str(&format!("allowed always for this task: {}\n", t.granted.programs.join(", "))); }
            for a in snap.attempts.iter().filter(|a| a.task_id == t.id) {
                let member = snap.members.iter().find(|m| m.id == a.member_id).map(|m| m.name.as_str()).unwrap_or("?");
                out.push_str(&format!("attempt {} by {member}: {:?}", a.id, a.state));
                if let Some(c) = &a.candidate { out.push_str(&format!(" · candidate {} files +{} -{}", c.files_changed, c.insertions, c.deletions)); }
                if let Some(f) = &a.failure { out.push_str(&format!(" · failure [{}]: {f}", crate::service::failure_class(f))); }
                out.push('\n');
                // Step captures it leaves while it works (FB-059).
                if a.state.is_active() {
                    let live = crate::paths::scratch_dir(&a.id).join("evidence");
                    let mut shots: Vec<String> = std::fs::read_dir(&live).map(|r| r.flatten().map(|e| e.path().display().to_string()).filter(|p| [".png", ".jpg", ".jpeg", ".webp", ".gif"].iter().any(|x| p.to_lowercase().ends_with(x))).collect()).unwrap_or_default();
                    shots.sort();
                    for p in shots { out.push_str(&format!("  step capture {p} (open it to see where it is going)\n")); }
                }
                for v in &a.verifier_results {
                    // One line each by default; the whole output when asked (a suite prints one line per test).
                    if full { out.push_str(&format!("  verifier {} {}:\n{}\n", v.id, v.status, v.tail.trim_end())); }
                    else { out.push_str(&format!("  verifier {} {}: {}\n", v.id, v.status, v.tail.lines().last().unwrap_or(""))); }
                    for e in &v.evidence { out.push_str(&format!("    capture {e} (open it to see what the player sees)\n")); }
                }
                if let Some(p) = &a.pending_permission {
                    let worktree = a.workspace_id.as_ref().and_then(|w| snap.workspaces.iter().find(|x| &x.id == w)).map(|w| w.path.as_str()).unwrap_or("");
                    out.push_str(&format!("  PERMISSION {}\n", p.title));
                    let what = crate::policy::describe(&p.title, &p.detail, worktree);
                    if !what.is_empty() { out.push_str(&format!("    {what}\n")); }
                    if !p.detail.trim().is_empty() { out.push_str(&format!("    detail: {}\n", p.detail.trim())); }
                    for o in &p.options { out.push_str(&format!("    option {} ({}, {})\n", o.id, o.name, o.kind)); }
                }
            }
            if let Some(a) = latest_attempt(&snap, &t.id) {
                if let Ok(r) = call("events.attempt", json!({"attemptId": a.id, "limit": if full { 20_000 } else { 400 }})) {
                    let text: String = r.as_array().into_iter().flatten().filter(|e| e["type"] == "turn.text").filter_map(|e| e["payload"]["text"].as_str()).collect::<Vec<_>>().join("");
                    let keep = if full { 60_000 } else { 2000 };
                    let tail: String = text.chars().rev().take(keep).collect::<Vec<_>>().into_iter().rev().collect();
                    if !tail.trim().is_empty() { out.push_str(&format!("member's {}:\n{tail}\n", if full { "whole last reply" } else { "last words (full: true for all of it)" })); }
                }
            }
            Ok(out)
        }
        "guild_diff" => {
            let snap = snapshot(call)?;
            let task_id = need("taskId")?;
            let artifact = snap.attempts.iter().filter(|a| a.task_id == task_id).filter_map(|a| a.candidate.as_ref().and_then(|c| c.diff_artifact_id.clone()).map(|d| (a.created_at.clone(), d))).max().map(|x| x.1).ok_or("no candidate diff yet")?;
            let r = call("artifact.read", json!({"artifactId": artifact}))?;
            let mut text = r["text"].as_str().unwrap_or("").to_string();
            if crate::domain::cut(&mut text, 60_000) { text.push_str("\n… (truncated)"); }
            Ok(text)
        }
        "guild_note" if s("taskId").is_none() => {
            let quest = need("questId")?;
            let r = match args.get("withdraw").and_then(Value::as_u64) {
                Some(n) => call("quest.note", json!({"questId": quest, "withdraw": n, "by": "delegate"}))?,
                None => call("quest.note", json!({"questId": quest, "text": need("text")?, "by": "delegate", "later": args.get("later").and_then(Value::as_bool).unwrap_or(false)}))?,
            };
            let at_work = r["redirected"].as_array().map(Vec::len).unwrap_or(0);
            Ok(format!("the quest has {} notes{}", r["notes"].as_array().map(Vec::len).unwrap_or(0), if at_work > 0 { format!("; {at_work} members at work were stopped and read it now") } else { String::new() }))
        }
        "guild_note" => {
            let r = call("task.note", json!({"taskId": need("taskId")?, "text": need("text")?, "by": "delegate", "later": args.get("later").and_then(Value::as_bool).unwrap_or(false)}))?;
            let when = match r["delivery"].as_str() { Some("now") => "the member was stopped and reads it now", Some("when the task starts") => "nobody is at work on it: the member reads it when the task starts", _ => "the member reads it when its turn ends" };
            Ok(format!("noted ({} notes on the task): {when}", r["notes"].as_array().map(Vec::len).unwrap_or(0)))
        }
        "guild_decide" => {
            let task_id = need("taskId")?;
            let reason = s("reason");
            let r = match need("decision")?.as_str() {
                "accept" => call("task.accept", json!({"taskId": task_id, "integrate": true, "reason": reason, "attemptId": s("attemptId"), "override": args.get("override").and_then(Value::as_bool).unwrap_or(false)}))?,
                "reject" => call("task.reject", json!({"taskId": task_id, "reason": reason}))?,
                "retry" => call("task.retry", json!({"taskId": task_id, "reason": reason, "timeoutMinutes": args.get("timeoutMinutes").and_then(Value::as_u64), "instructions": s("instructions")}))?,
                "integrate" => call("task.integrate", json!({"taskId": task_id}))?,
                "cancel" => call("task.cancel", json!({"taskId": task_id, "replacedBy": s("replacedBy")}))?,
                other => return Err(format!("unknown decision {other}")),
            };
            Ok(format!("done: {r}"))
        }
        "guild_ack" => {
            let r = call("stall.ack", json!({"id": need("id")?, "by": "delegate"}))?;
            Ok(format!("acknowledged: {}", r["acked"]))
        }
        "guild_permission" => {
            let snap = snapshot(call)?;
            let task_id = need("taskId")?;
            let a = latest_attempt(&snap, &task_id).filter(|a| a.pending_permission.is_some()).ok_or("no permission pending on that task")?;
            let request = a.pending_permission.as_ref().unwrap().request_id.clone();
            call("permission.respond", json!({"attemptId": a.id, "requestId": request, "optionId": need("optionId")?}))?;
            Ok("answered".into())
        }
        other => Err(format!("unknown tool {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn it_introduces_itself_and_lists_its_tools() {
        let call: Call = &|_, _| Err("no service".into());
        let init = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}), call, &|_| {}).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "guild");
        assert!(handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), call, &|_| {}).is_none(), "notifications get no answer");
        let list = handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}), call, &|_| {}).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"guild_start_quest") && names.contains(&"guild_wait"));
        // A tool that fails says so in its result, not as a protocol error.
        let failed = handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "guild_status", "arguments": {}}}), call, &|_| {}).unwrap();
        assert_eq!(failed["result"]["isError"], true);
    }

    #[test]
    fn a_visual_task_without_reference_files_or_evidence_is_flagged() {
        // FB-060: tasks that named the reference and the files to reuse got it right the first time.
        let vague = json!({"id": "t1", "role": "asset-artist", "title": "Neon look", "instructions": "bring the look back"});
        let gap = visual_gap(&vague).unwrap();
        assert!(gap.contains("a reference image") && gap.contains("the exact files") && gap.contains("evidence"), "{gap}");
        let precise = json!({"id": "t2", "role": "asset-artist", "title": "Neon look", "evidence": ["captures/dia.png"],
            "instructions": "Match docs/visual_ref/02_ciudad.png reusing `git show 91de469:shaders/neon.gdshader` in scenes/city.tscn"});
        assert_eq!(visual_gap(&precise), None);
        assert_eq!(visual_gap(&json!({"id": "t3", "role": "implementer", "title": "Save game", "instructions": "persist the run"})), None);
    }

    #[test]
    fn a_quest_gets_scoped_task_ids_and_pinned_members() {
        let calls: RefCell<Vec<(String, Value)>> = RefCell::new(vec![]);
        let snap = json!({"schema_version": 1, "last_seq": 0, "service_version": "t", "projects": [], "quests": [], "tasks": [], "attempts": [], "workspaces": [], "harnesses": [],
            "members": [{"id": "kiro-1", "name": "Orin", "roles": ["gameplay-engineer"], "harness": "kiro", "model_id": null, "mode": null, "role_prompt": "", "enabled": true}]});
        let call = |m: &str, p: Value| -> Result<Value, String> {
            calls.borrow_mut().push((m.to_string(), p.clone()));
            Ok(match m { "guild.snapshot" => snap.clone(), "quest.create" => json!({"id": "QUEST0000abcdefgh"}), _ => json!({}) })
        };
        let args = json!({"projectId": "p", "title": "Poison", "tasks": [{"id": "t1", "title": "A", "instructions": "do a", "member": "orin"}, {"id": "t2", "title": "B", "instructions": "do b", "dependsOn": ["t1"]}]});
        let _ = run_tool("guild_start_quest", &args, &call, &|_| {});
        let made: Vec<Value> = calls.borrow().iter().filter(|(m, _)| m == "task.create").map(|c| c.1.clone()).collect();
        assert_eq!(made[0]["id"], "abcdefgh-t1");
        assert_eq!(made[0]["memberId"], "kiro-1");
        assert_eq!(made[0]["role"], "gameplay-engineer", "a pinned member brings its role");
        assert_eq!(made[1]["dependsOn"], json!(["abcdefgh-t1"]));
        assert!(calls.borrow().iter().any(|(m, _)| m == "quest.start"));
    }
}
