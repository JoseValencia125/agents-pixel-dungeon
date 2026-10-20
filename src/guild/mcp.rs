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

const INSTRUCTIONS: &str = "The guild runs coding agents (its members, e.g. Kiro) in isolated git worktrees. You orchestrate: break the user's request into small tasks with verifiable acceptance criteria and dependencies, start them as a quest with guild_start_quest, then call guild_wait until something needs you. For a candidate ready, read it with guild_diff and accept (integrate) or reject with a reason the member can act on; answer permissions with guild_permission; retry or cancel failed tasks. Integrating merges into the quest's branch guild/quest/<id>, never into main: tell the user the branch when the quest completes.";

fn tools() -> Value {
    let task_id = json!({"type": "string", "description": "A task id as guild_status or guild_start_quest gives it"});
    json!([
        {"name": "guild_status", "description": "The guild at a glance: projects, members (role, harness, busy), quests with their tasks and what needs a decision.",
         "inputSchema": {"type": "object", "properties": {"questId": {"type": "string", "description": "Only this quest"}}}},
        {"name": "guild_start_quest", "description": "Create a quest with its tasks and start it. Members take the tasks by role (or the member named) in their own worktrees; dependencies run in order, the rest in parallel.",
         "inputSchema": {"type": "object", "required": ["title", "tasks"], "properties": {
            "projectId": {"type": "string", "description": "Defaults to the guild's first project"},
            "title": {"type": "string"},
            "description": {"type": "string"},
            "tasks": {"type": "array", "items": {"type": "object", "required": ["id", "title", "instructions"], "properties": {
                "id": {"type": "string", "description": "Short id, unique in this quest (t1, t2…), for dependsOn"},
                "title": {"type": "string"},
                "instructions": {"type": "string", "description": "Everything the member needs: it sees only this task, not the conversation"},
                "acceptance": {"type": "array", "items": {"type": "string"}},
                "role": {"type": "string", "description": "A role some member holds; defaults to implementer"},
                "member": {"type": "string", "description": "A member's name or id, to pin the task to it"},
                "dependsOn": {"type": "array", "items": {"type": "string"}},
                "candidates": {"type": "integer", "description": "Run a race of this many members (default 1)"}}}}}}},
        {"name": "guild_wait", "description": "Block until a task of the quest needs you (candidate ready, permission asked, failed, blocked) or the quest ends, or until the timeout. Returns the quest's status.",
         "inputSchema": {"type": "object", "required": ["questId"], "properties": {"questId": {"type": "string"}, "timeoutSeconds": {"type": "integer", "description": "Default 300, at most 540"}}}},
        {"name": "guild_task", "description": "One task in full: instructions, attempts, summaries, failures, verifier results, candidate size, pending permission and the end of the member's transcript.",
         "inputSchema": {"type": "object", "required": ["taskId"], "properties": {"taskId": task_id}}},
        {"name": "guild_diff", "description": "The diff of a task's latest candidate.",
         "inputSchema": {"type": "object", "required": ["taskId"], "properties": {"taskId": task_id}}},
        {"name": "guild_decide", "description": "Decide on a task: accept (integrates the candidate into the quest's branch), reject (with a reason; the member retries if attempts remain), retry, or cancel.",
         "inputSchema": {"type": "object", "required": ["taskId", "decision"], "properties": {"taskId": task_id, "decision": {"type": "string", "enum": ["accept", "reject", "retry", "cancel"]}, "reason": {"type": "string"}}}},
        {"name": "guild_permission", "description": "Answer a member's pending permission request with one of its option ids (from guild_task).",
         "inputSchema": {"type": "object", "required": ["taskId", "optionId"], "properties": {"taskId": task_id, "optionId": {"type": "string"}}}},
    ])
}

fn snapshot(call: Call) -> Result<Snapshot, String> {
    serde_json::from_value(call("guild.snapshot", json!({}))?).map_err(|e| e.to_string())
}

fn latest_attempt<'a>(snap: &'a Snapshot, task_id: &str) -> Option<&'a Attempt> {
    snap.attempts.iter().filter(|a| a.task_id == task_id).max_by(|a, b| a.created_at.cmp(&b.created_at))
}

/// What a task wants from the orchestrator, if anything.
fn needs(snap: &Snapshot, t: &Task) -> Option<&'static str> {
    let a = latest_attempt(snap, &t.id);
    match (t.state, a.map(|a| a.state)) {
        (TaskState::Completed | TaskState::Cancelled | TaskState::Accepted | TaskState::Integrating, _) => None,
        (_, Some(AttemptState::WaitingInput)) => Some("permission"),
        // Submitted once its verifiers ran (they run in Verifying), Passed when they passed.
        (_, Some(AttemptState::Submitted | AttemptState::Passed)) => Some("candidate ready: read guild_diff, then accept or reject"),
        (TaskState::Failed, _) => Some("failed: retry or cancel"),
        (TaskState::Blocked, _) => Some("blocked: retry or cancel"),
        _ => None,
    }
}

fn task_line(snap: &Snapshot, t: &Task) -> String {
    let a = latest_attempt(snap, &t.id);
    let who = a.and_then(|a| snap.members.iter().find(|m| m.id == a.member_id)).map(|m| format!(" · {} ({})", m.name, m.harness)).unwrap_or_default();
    let mut s = format!("- {} [{}] {:?}{who}: {}", t.id, t.role, t.state, t.title);
    if let Some(a) = a {
        if !a.summary.is_empty() { s.push_str(&format!(" — {}", a.summary.chars().take(160).collect::<String>())); }
        if let Some(f) = &a.failure { s.push_str(&format!(" — failure: {f}")); }
    }
    if let Some(r) = &t.block_reason { s.push_str(&format!(" — {r}")); }
    if let Some(n) = needs(snap, t) { s.push_str(&format!("  ⟵ NEEDS YOU: {n}")); }
    s
}

fn quest_report(snap: &Snapshot, q: &Quest) -> String {
    let tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == q.id).collect();
    let done = tasks.iter().filter(|t| t.state == TaskState::Completed).count();
    let mut s = format!("quest {} {:?} · {}/{} done · branch {} · {}\n", q.id, q.state, done, tasks.len(), q.integration_ref.trim_start_matches("refs/heads/"), q.title);
    for t in tasks { s.push_str(&task_line(snap, t)); s.push('\n'); }
    s
}

fn find_member<'a>(snap: &'a Snapshot, who: &str) -> Option<&'a Member> {
    snap.members.iter().find(|m| m.id == who || m.name.eq_ignore_ascii_case(who))
}

fn run_tool(name: &str, args: &Value, call: Call, sleep: &dyn Fn(Duration)) -> Result<String, String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string);
    let need = |k: &str| s(k).filter(|v| !v.trim().is_empty()).ok_or_else(|| format!("missing {k}"));
    match name {
        "guild_status" => {
            let snap = snapshot(call)?;
            let mut out = String::new();
            for p in &snap.projects { out.push_str(&format!("project {} · {} · {}\n", p.id, p.name, p.repo_path)); }
            let busy: Vec<&str> = snap.attempts.iter().filter(|a| a.state.is_active()).map(|a| a.member_id.as_str()).collect();
            out.push_str("members:\n");
            for m in snap.members.iter().filter(|m| m.enabled) {
                out.push_str(&format!("- {} ({} · {}) roles {} · {}\n", m.name, m.harness, m.model_id.as_deref().unwrap_or("default model"), m.roles.join(", "), if busy.contains(&m.id.as_str()) { "busy" } else { "free" }));
            }
            if snap.members.iter().all(|m| !m.enabled) { out.push_str("- none: add members in the guild window or with `guild member add`\n"); }
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
            let quest = call("quest.create", json!({"projectId": project, "title": need("title")?, "description": s("description").unwrap_or_default()}))?;
            let quest_id = quest["id"].as_str().unwrap_or_default().to_string();
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
                call("task.create", params)?;
            }
            call("quest.start", json!({"questId": quest_id}))?;
            Ok(format!("started. task ids: {}\n{}", local.iter().map(|l| id(l)).collect::<Vec<_>>().join(", "), quest_report(&snapshot(call)?, snapshot(call)?.quests.iter().find(|q| q.id == quest_id).ok_or("quest vanished")?)))
        }
        "guild_wait" => {
            let quest_id = need("questId")?;
            let timeout = Duration::from_secs(args["timeoutSeconds"].as_u64().unwrap_or(300).clamp(5, 540));
            let started = Instant::now();
            loop {
                let snap = snapshot(call)?;
                let q = snap.quests.iter().find(|q| q.id == quest_id).ok_or_else(|| format!("no quest {quest_id}"))?;
                let ended = matches!(q.state, QuestState::Completed | QuestState::Cancelled);
                let waiting = snap.tasks.iter().any(|t| t.quest_id == quest_id && needs(&snap, t).is_some());
                if ended || waiting || started.elapsed() >= timeout {
                    let why = if ended { "the quest ended" } else if waiting { "something needs you" } else { "timeout, still working" };
                    return Ok(format!("{why}\n{}", quest_report(&snap, q)));
                }
                sleep(Duration::from_secs(3));
            }
        }
        "guild_task" => {
            let snap = snapshot(call)?;
            let task_id = need("taskId")?;
            let t = snap.tasks.iter().find(|t| t.id == task_id).ok_or_else(|| format!("no task {task_id}"))?;
            let mut out = task_line(&snap, t);
            out.push_str(&format!("\ninstructions: {}\n", t.instructions));
            for ac in &t.acceptance { out.push_str(&format!("acceptance {}: {}\n", ac.id, ac.text)); }
            for a in snap.attempts.iter().filter(|a| a.task_id == t.id) {
                let member = snap.members.iter().find(|m| m.id == a.member_id).map(|m| m.name.as_str()).unwrap_or("?");
                out.push_str(&format!("attempt {} by {member}: {:?}", a.id, a.state));
                if let Some(c) = &a.candidate { out.push_str(&format!(" · candidate {} files +{} -{}", c.files_changed, c.insertions, c.deletions)); }
                out.push('\n');
                for v in &a.verifier_results {
                    out.push_str(&format!("  verifier {} {}: {}\n", v.id, v.status, v.tail.lines().last().unwrap_or("")));
                    for e in &v.evidence { out.push_str(&format!("    capture {e} (open it to see what the player sees)\n")); }
                }
                if let Some(p) = &a.pending_permission {
                    out.push_str(&format!("  PERMISSION {}: {}\n", p.title, p.detail));
                    for o in &p.options { out.push_str(&format!("    option {} ({}, {})\n", o.id, o.name, o.kind)); }
                }
            }
            if let Some(a) = latest_attempt(&snap, &t.id) {
                if let Ok(r) = call("events.attempt", json!({"attemptId": a.id, "limit": 400})) {
                    let text: String = r.as_array().into_iter().flatten().filter(|e| e["type"] == "turn.text").filter_map(|e| e["payload"]["text"].as_str()).collect::<Vec<_>>().join("");
                    let tail: String = text.chars().rev().take(2000).collect::<Vec<_>>().into_iter().rev().collect();
                    if !tail.trim().is_empty() { out.push_str(&format!("member's last words:\n{tail}\n")); }
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
            if text.len() > 60_000 { text.truncate(60_000); text.push_str("\n… (truncated)"); }
            Ok(text)
        }
        "guild_decide" => {
            let task_id = need("taskId")?;
            let reason = s("reason");
            let r = match need("decision")?.as_str() {
                "accept" => call("task.accept", json!({"taskId": task_id, "integrate": true, "reason": reason}))?,
                "reject" => call("task.reject", json!({"taskId": task_id, "reason": reason}))?,
                "retry" => call("task.retry", json!({"taskId": task_id}))?,
                "cancel" => call("task.cancel", json!({"taskId": task_id}))?,
                other => return Err(format!("unknown decision {other}")),
            };
            Ok(format!("done: {r}"))
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
