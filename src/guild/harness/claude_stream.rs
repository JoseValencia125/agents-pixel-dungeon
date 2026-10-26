//! Claude Code headless: `claude -p --input-format stream-json
//! --output-format stream-json`, one long-lived process per session
//! that takes several turns on stdin, with permission prompts routed to
//! us as `control_request` / `can_use_tool` (`--permission-prompt-tool
//! stdio`) and answered with `control_response`. The session persists on
//! disk and is reopened with `--resume <session_id>`. Verified against
//! Claude Code 2.1.300.

use super::proc::{find_binary, scrub_env, LineProc};
use super::*;
use crate::domain::{HarnessId, HarnessKind, ModelOption, PermissionOption};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

pub const ADAPTER_VERSION: &str = "claude-stream-1";

pub struct ClaudeStream {
    pub binary: PathBuf,
    sessions: Mutex<HashMap<String, Arc<ClaudeSession>>>,
}

struct ClaudeSession {
    cwd: PathBuf,
    env: Vec<(String, String)>,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    model: Option<String>,
    /// Claude Code's session id, from `system/init`.
    native_id: Option<String>,
    proc: Option<Arc<LineProc>>,
    /// Where the turn's events and its result go while one runs.
    turn: Option<(Sender<HarnessEvent>, Sender<TurnOutcome>)>,
    interrupted: bool,
    /// Tool uses seen, by id: title, kind, paths.
    tools: HashMap<String, (String, String, Vec<String>)>,
    pending: Vec<String>,
}

impl Default for ClaudeStream {
    fn default() -> Self { ClaudeStream::new() }
}

impl ClaudeStream {
    pub fn new() -> ClaudeStream {
        ClaudeStream { binary: find_binary("CLAUDE_BIN", "claude", &["~/.local/bin/claude", "/opt/homebrew/bin/claude", "/usr/local/bin/claude"]), sessions: Mutex::new(HashMap::new()) }
    }

    fn session(&self, session: &SessionRef) -> HarnessResult<Arc<ClaudeSession>> {
        self.sessions.lock().unwrap().get(&session.id).cloned().ok_or_else(|| HarnessError::TransportLost("unknown session".into()))
    }

    /// Start the CLI for this session (new or resumed) and its reader.
    fn ensure_proc(&self, id: &str, s: &Arc<ClaudeSession>) -> HarnessResult<Arc<LineProc>> {
        if let Some(p) = s.inner.lock().unwrap().proc.clone() { return Ok(p); }
        let (model, native) = {
            let inner = s.inner.lock().unwrap();
            (inner.model.clone(), inner.native_id.clone())
        };
        let mut cmd = Command::new(&self.binary);
        cmd.args(["-p", "--output-format", "stream-json", "--input-format", "stream-json", "--verbose", "--permission-prompts", "host", "--permission-prompt-tool", "stdio"]);
        if let Some(m) = &model { cmd.args(["--model", m]); }
        if let Some(n) = &native { cmd.args(["--resume", n]); }
        scrub_env(&mut cmd);
        cmd.envs(s.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let proc = LineProc::spawn(cmd, &s.cwd, &format!("claude-{id}"))?;
        let stdout = proc.take_stdout().ok_or_else(|| HarnessError::TransportLost("no stdout".into()))?;
        s.inner.lock().unwrap().proc = Some(proc.clone());
        let reader = s.clone();
        let reader_proc = proc.clone();
        std::thread::Builder::new()
            .name("claude-stream".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                    handle_line(&reader, &reader_proc, &msg);
                }
                let mut inner = reader.inner.lock().unwrap();
                inner.proc = None;
                if let Some((_, res)) = inner.turn.take() {
                    let _ = res.send(if inner.interrupted { TurnOutcome::Cancelled } else { TurnOutcome::Failed { reason: "TRANSPORT_LOST: claude ended before the turn finished".into() } });
                }
            })
            .map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        Ok(proc)
    }
}

/// Tool kind for Claude Code's tool names, as the room reads them.
fn tool_kind(name: &str) -> &'static str {
    match name {
        "Read" | "Grep" | "Glob" | "LS" | "NotebookRead" => "read",
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => "edit",
        "Bash" | "BashOutput" | "KillShell" => "execute",
        "WebFetch" | "WebSearch" => "fetch",
        "TodoWrite" | "Task" | "ExitPlanMode" | "EnterPlanMode" => "think",
        "Agent" => "other",
        _ => "other",
    }
}

/// What a tool use says in one line, and the files it touches.
fn tool_title(name: &str, input: &Value) -> (String, Vec<String>) {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match name {
        "Bash" => (super::proc::command_title(&s("command")), vec![]),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "NotebookRead" => {
            let p = s("file_path").max(s("notebook_path"));
            (format!("{name} {}", p.rsplit('/').next().unwrap_or("")), if p.is_empty() { vec![] } else { vec![p] })
        }
        "Grep" | "Glob" => (format!("{name} {}", s("pattern")), vec![]),
        "WebFetch" => (format!("Fetch {}", s("url")), vec![]),
        "WebSearch" => (format!("Search {}", s("query")), vec![]),
        "Agent" | "Task" => (format!("Subagent: {}", s("description")), vec![]),
        _ => (name.to_string(), vec![]),
    }
}

fn handle_line(s: &Arc<ClaudeSession>, proc: &Arc<LineProc>, msg: &Value) {
    let kind = msg.get("type").and_then(Value::as_str).unwrap_or("");
    let subtype = msg.get("subtype").and_then(Value::as_str).unwrap_or("");
    let mut inner = s.inner.lock().unwrap();
    let send = |inner: &Inner, ev: HarnessEvent| { if let Some((tx, _)) = &inner.turn { let _ = tx.send(ev); } };
    match (kind, subtype) {
        ("system", "init") => {
            if let Some(id) = msg.get("session_id").and_then(Value::as_str) {
                inner.native_id = Some(id.to_string());
                send(&inner, HarnessEvent::SessionId(id.to_string()));
            }
            if let Some(m) = msg.get("model").and_then(Value::as_str) {
                inner.model = Some(m.to_string());
                send(&inner, HarnessEvent::ModelChanged { effective: m.to_string(), reason: "init".into() });
            }
        }
        ("assistant", _) => {
            for block in msg.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => send(&inner, HarnessEvent::Text(format!("{}\n", block.get("text").and_then(Value::as_str).unwrap_or("")))),
                    Some("thinking") => send(&inner, HarnessEvent::Thought(block.get("thinking").and_then(Value::as_str).unwrap_or("").into())),
                    Some("tool_use") => {
                        let id = block.get("id").and_then(Value::as_str).unwrap_or("tool").to_string();
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let (title, paths) = tool_title(name, block.get("input").unwrap_or(&Value::Null));
                        let kind = tool_kind(name).to_string();
                        let action = action_for_tool(&kind, &title).to_string();
                        inner.tools.insert(id.clone(), (title.clone(), kind.clone(), paths.clone()));
                        send(&inner, HarnessEvent::Tool { id, title, kind, status: "in_progress".into(), action, paths });
                    }
                    _ => {}
                }
            }
        }
        ("user", _) => {
            for block in msg.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                if block.get("type").and_then(Value::as_str) != Some("tool_result") { continue }
                let id = block.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string();
                let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                if let Some((title, kind, paths)) = inner.tools.get(&id).cloned() {
                    let action = action_for_tool(&kind, &title).to_string();
                    send(&inner, HarnessEvent::Tool { id, title, kind, status: if failed { "failed".into() } else { "completed".into() }, action, paths });
                }
            }
        }
        ("control_request", _) => {
            let request_id = msg.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
            let req = msg.get("request").cloned().unwrap_or(Value::Null);
            if req.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                let tool = req.get("tool_name").and_then(Value::as_str).unwrap_or("tool");
                let input = req.get("input").cloned().unwrap_or(Value::Null);
                let (what, _) = tool_title(tool, &input);
                let title = if what == tool { tool.to_string() } else { format!("{tool}: {what}") };
                let detail = serde_json::to_string_pretty(&input).unwrap_or_default().chars().take(2000).collect();
                inner.pending.push(request_id.clone());
                send(&inner, HarnessEvent::PermissionRequested {
                    request_id,
                    title,
                    detail,
                    options: vec![
                        PermissionOption { id: "allow".into(), name: "Allow".into(), kind: "allow_once".into() },
                        PermissionOption { id: "deny".into(), name: "Deny".into(), kind: "reject_once".into() },
                    ],
                });
            } else {
                // Hooks, MCP and the rest are not ours to answer.
                let _ = proc.write_line(&json!({"type": "control_response", "response": {"subtype": "error", "request_id": request_id, "error": "not handled by the guild"}}).to_string());
            }
        }
        ("result", _) => {
            let outcome = if inner.interrupted {
                TurnOutcome::Cancelled
            } else if subtype == "success" {
                TurnOutcome::Completed { stop_reason: "end_turn".into() }
            } else {
                let reason = msg.get("result").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
                    .or_else(|| msg.get("errors").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| subtype.to_string());
                TurnOutcome::Failed { reason: format!("{subtype}: {reason}") }
            };
            inner.pending.clear();
            if let Some((_, res)) = inner.turn.take() { let _ = res.send(outcome); }
        }
        _ => send(&inner, HarnessEvent::Raw(msg.clone())),
    }
}

impl HarnessAdapter for ClaudeStream {
    fn kind(&self) -> HarnessKind { HarnessId::named("claude") }

    fn probe(&self) -> HarnessResult<Capabilities> {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("--version");
        scrub_env(&mut cmd);
        let output = cmd.output().map_err(|e| HarnessError::RuntimeFailed(format!("claude not found ({}): {e}", self.binary.display())))?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut extensions = std::collections::BTreeMap::new();
        extensions.insert("binary".into(), json!(self.binary.display().to_string()));
        Ok(Capabilities {
            adapter_version: ADAPTER_VERSION.into(),
            runtime_version: version,
            protocol_version: Some("stream-json".into()),
            transport: Transport::Stdio,
            create_session: Support::Supported,
            resume_session: Support::Supported,
            cancel_turn: Support::Supported,
            structured_events: Support::Supported,
            permission_requests: Support::Supported,
            model_discovery: Support::Unknown,
            model_switch: ModelSwitch::BetweenTurns,
            usage_reporting: Support::Supported,
            native_fork: Support::Supported,
            mcp: Support::Supported,
            subagents: Support::Supported,
            plan_mode: Support::Supported,
            screenshots: Support::Unsupported,
            steering: Support::Supported,
            max_concurrent_sessions: None,
            extensions,
        })
    }

    fn create_session(&self, options: &SessionOptions) -> HarnessResult<SessionRef> {
        let id = crate::domain::new_id();
        let s = Arc::new(ClaudeSession { cwd: options.cwd.clone(), env: options.env.clone(), inner: Mutex::new(Inner { model: options.model_id.clone(), ..Default::default() }) });
        self.sessions.lock().unwrap().insert(id.clone(), s);
        Ok(SessionRef { id, native_id: None })
    }

    fn resume_session(&self, options: &SessionOptions, native_id: &str) -> HarnessResult<SessionRef> {
        let id = crate::domain::new_id();
        let s = Arc::new(ClaudeSession { cwd: options.cwd.clone(), env: options.env.clone(), inner: Mutex::new(Inner { model: options.model_id.clone(), native_id: Some(native_id.into()), ..Default::default() }) });
        self.sessions.lock().unwrap().insert(id.clone(), s);
        Ok(SessionRef { id, native_id: Some(native_id.into()) })
    }

    fn prompt(&self, session: &SessionRef, request_id: &str, text: &str) -> HarnessResult<TurnHandle> {
        let s = self.session(session)?;
        let proc = self.ensure_proc(&session.id, &s)?;
        let (ev_tx, ev_rx) = channel();
        let (res_tx, res_rx) = channel();
        {
            let mut inner = s.inner.lock().unwrap();
            if inner.turn.is_some() { return Err(HarnessError::RuntimeFailed("a turn is already running in this session".into())); }
            inner.interrupted = false;
            inner.turn = Some((ev_tx, res_tx));
        }
        proc.write_line(&json!({"type": "user", "message": {"role": "user", "content": text}}).to_string())?;
        Ok(TurnHandle { turn_id: format!("claude-{request_id}"), events: ev_rx, result: res_rx })
    }

    fn interrupt(&self, session: &SessionRef) -> HarnessResult<()> {
        let s = self.session(session)?;
        let proc = {
            let mut inner = s.inner.lock().unwrap();
            inner.interrupted = true;
            inner.proc.clone()
        };
        let Some(proc) = proc else { return Ok(()) };
        proc.write_line(&json!({"type": "control_request", "request_id": crate::domain::new_id(), "request": {"subtype": "interrupt"}}).to_string())
    }

    fn stop(&self, session: &SessionRef) -> HarnessResult<bool> {
        let Some(s) = self.sessions.lock().unwrap().remove(&session.id) else { return Ok(true) };
        let proc = s.inner.lock().unwrap().proc.take();
        let Some(proc) = proc else { return Ok(true) };
        // End of input ends the CLI; force it if it lingers.
        proc.close_stdin();
        for _ in 0..30 {
            if proc.exit_code().is_some() { return Ok(true); }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Ok(proc.kill())
    }

    fn list_models(&self, session: Option<&SessionRef>) -> HarnessResult<Vec<ModelOption>> {
        let mut out = vec![
            ModelOption { id: "sonnet".into(), label: "Sonnet".into(), description: "alias for the latest Sonnet".into() },
            ModelOption { id: "opus".into(), label: "Opus".into(), description: "alias for the latest Opus".into() },
            ModelOption { id: "haiku".into(), label: "Haiku".into(), description: "alias for the latest Haiku".into() },
        ];
        if let Some(session) = session {
            if let Ok(s) = self.session(session) {
                if let Some(m) = s.inner.lock().unwrap().model.clone() {
                    if !out.iter().any(|o| o.id == m) { out.insert(0, ModelOption { id: m.clone(), label: m, description: "in use".into() }); }
                }
            }
        }
        Ok(out)
    }

    fn set_model(&self, session: &SessionRef, model_id: &str) -> HarnessResult<String> {
        let s = self.session(session)?;
        let proc = {
            let mut inner = s.inner.lock().unwrap();
            if inner.turn.is_some() { return Err(HarnessError::RuntimeFailed("a turn is running; the model changes between turns".into())); }
            inner.model = Some(model_id.into());
            // The next turn reopens the session with the new model.
            inner.proc.take()
        };
        if let Some(p) = proc { p.close_stdin(); std::thread::sleep(std::time::Duration::from_millis(300)); if p.exit_code().is_none() { p.kill(); } }
        Ok(model_id.into())
    }

    fn respond_permission(&self, session: &SessionRef, request_id: &str, option_id: Option<&str>) -> HarnessResult<()> {
        let s = self.session(session)?;
        let proc = {
            let mut inner = s.inner.lock().unwrap();
            let Some(pos) = inner.pending.iter().position(|p| p == request_id) else { return Err(HarnessError::InvalidOutput(format!("no pending permission {request_id}"))) };
            inner.pending.remove(pos);
            inner.proc.clone().ok_or_else(|| HarnessError::TransportLost("session has no process".into()))?
        };
        let allow = option_id.map(|o| o == "allow" || o.starts_with("allow")).unwrap_or(false);
        let response = if allow { json!({"behavior": "allow"}) } else { json!({"behavior": "deny", "message": "The user declined this action."}) };
        proc.write_line(&json!({"type": "control_response", "response": {"subtype": "success", "request_id": request_id, "response": response}}).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_tool_uses_map_to_rooms_and_files() {
        let (title, paths) = tool_title("Write", &json!({"file_path": "/w/src/lib.rs", "content": "x"}));
        assert_eq!(title, "Write lib.rs");
        assert_eq!(paths, vec!["/w/src/lib.rs".to_string()]);
        assert_eq!(tool_kind("Write"), "edit");
        assert_eq!(action_for_tool(tool_kind("Bash"), &tool_title("Bash", &json!({"command": "cargo test"})).0), "brew");
        assert_eq!(action_for_tool(tool_kind("Grep"), "Grep foo"), "read");
        assert_eq!(action_for_tool(tool_kind("Agent"), &tool_title("Agent", &json!({"description": "explore"})).0), "summon");
    }
}
