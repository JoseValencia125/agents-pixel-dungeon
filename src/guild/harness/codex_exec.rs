//! Codex through its non-interactive CLI: one `codex exec --json` process
//! per turn, the thread kept by Codex on disk and continued with
//! `codex exec resume <thread> --json`. Events are its JSONL
//! (`thread.started`, `turn.started`, `item.started` / `item.completed`,
//! `turn.completed` / `turn.failed`, `error`). Commands run in Codex's
//! `workspace-write` sandbox, which is the attempt's worktree, so there
//! are no permission prompts: this adapter reports that capability as
//! unsupported and the guild shows it. Verified against codex-cli 0.162.1.

use super::proc::{command_title, find_binary, scrub_env, LineProc};
use super::*;
use crate::domain::{HarnessId, HarnessKind, ModelOption};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

pub const ADAPTER_VERSION: &str = "codex-exec-1";

pub struct CodexExec {
    pub binary: PathBuf,
    sessions: Mutex<HashMap<String, Arc<Mutex<CodexSession>>>>,
}

struct CodexSession {
    cwd: PathBuf,
    model: Option<String>,
    env: Vec<(String, String)>,
    /// Codex's thread id, known after the first turn starts.
    thread_id: Option<String>,
    proc: Option<Arc<LineProc>>,
    interrupted: bool,
}

impl Default for CodexExec {
    fn default() -> Self { CodexExec::new() }
}

impl CodexExec {
    pub fn new() -> CodexExec {
        CodexExec { binary: find_binary("CODEX_BIN", "codex", &["/opt/homebrew/bin/codex", "/usr/local/bin/codex", "~/.local/bin/codex"]), sessions: Mutex::new(HashMap::new()) }
    }

    fn session(&self, session: &SessionRef) -> HarnessResult<Arc<Mutex<CodexSession>>> {
        self.sessions.lock().unwrap().get(&session.id).cloned().ok_or_else(|| HarnessError::TransportLost("unknown session".into()))
    }

    /// The model `~/.codex/config.toml` (or `$CODEX_HOME`) names.
    fn configured_model() -> Option<String> {
        let home = std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".codex"));
        let text = std::fs::read_to_string(home.join("config.toml")).ok()?;
        text.lines().find_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("model")?.trim_start();
            let rest = rest.strip_prefix('=')?.trim();
            Some(rest.trim_matches('"').to_string())
        })
    }
}

/// Translate one Codex item into the guild's event, if it is one.
fn item_event(item: &Value, completed: bool, last_error: &mut Option<String>) -> Option<HarnessEvent> {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    let id = item.get("id").and_then(Value::as_str).unwrap_or("item").to_string();
    let status = || -> String {
        if !completed { return "in_progress".into() }
        match item.get("status").and_then(Value::as_str) {
            Some("failed") | Some("declined") => "failed".into(),
            _ => "completed".into(),
        }
    };
    match kind {
        "agent_message" => completed.then(|| HarnessEvent::Text(format!("{}\n", item.get("text").and_then(Value::as_str).unwrap_or("")))),
        "reasoning" => completed.then(|| HarnessEvent::Thought(item.get("text").and_then(Value::as_str).unwrap_or("").into())),
        "command_execution" => {
            let title = command_title(item.get("command").and_then(Value::as_str).unwrap_or(""));
            let mut st = status();
            if completed && item.get("exit_code").and_then(Value::as_i64).map(|c| c != 0).unwrap_or(false) { st = "failed".into(); }
            Some(HarnessEvent::Tool { id, title, kind: "execute".into(), status: st, action: "brew".into(), paths: vec![] })
        }
        "file_change" => {
            let paths: Vec<String> = item.get("changes").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.get("path").and_then(Value::as_str)).map(str::to_string).collect();
            let title = if paths.is_empty() { "Edit files".to_string() } else { format!("Edit {}", paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p)).collect::<Vec<_>>().join(", ")) };
            Some(HarnessEvent::Tool { id, title, kind: "edit".into(), status: status(), action: "forge".into(), paths })
        }
        "mcp_tool_call" => {
            let title = format!("{}.{}", item.get("server").and_then(Value::as_str).unwrap_or("mcp"), item.get("tool").and_then(Value::as_str).unwrap_or("tool"));
            let action = action_for_tool("other", &title).to_string();
            Some(HarnessEvent::Tool { id, title, kind: "other".into(), status: status(), action, paths: vec![] })
        }
        "web_search" => Some(HarnessEvent::Tool { id, title: format!("Search: {}", item.get("query").and_then(Value::as_str).unwrap_or("")), kind: "fetch".into(), status: status(), action: "read".into(), paths: vec![] }),
        "todo_list" => Some(HarnessEvent::Plan(item.get("items").cloned().unwrap_or(Value::Null))),
        "error" => {
            *last_error = item.get("message").and_then(Value::as_str).map(str::to_string);
            Some(HarnessEvent::Raw(item.clone()))
        }
        _ => Some(HarnessEvent::Raw(item.clone())),
    }
}

impl HarnessAdapter for CodexExec {
    fn kind(&self) -> HarnessKind { HarnessId::named("codex") }

    fn probe(&self) -> HarnessResult<Capabilities> {
        let output = Command::new(&self.binary).arg("--version").output().map_err(|e| HarnessError::RuntimeFailed(format!("codex not found ({}): {e}", self.binary.display())))?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut extensions = std::collections::BTreeMap::new();
        extensions.insert("binary".into(), json!(self.binary.display().to_string()));
        extensions.insert("sandbox".into(), json!("workspace-write"));
        if let Some(m) = Self::configured_model() { extensions.insert("configured_model".into(), json!(m)); }
        Ok(Capabilities {
            adapter_version: ADAPTER_VERSION.into(),
            runtime_version: version,
            protocol_version: Some("codex-exec-jsonl".into()),
            transport: Transport::Stdio,
            create_session: Support::Supported,
            resume_session: Support::Supported,
            cancel_turn: Support::Supported,
            structured_events: Support::Supported,
            permission_requests: Support::Unsupported,
            model_discovery: Support::Unsupported,
            model_switch: ModelSwitch::BetweenTurns,
            usage_reporting: Support::Supported,
            native_fork: Support::Supported,
            mcp: Support::Supported,
            subagents: Support::Unknown,
            plan_mode: Support::Unsupported,
            screenshots: Support::Supported,
            steering: Support::Unsupported,
            max_concurrent_sessions: None,
            extensions,
        })
    }

    fn create_session(&self, options: &SessionOptions) -> HarnessResult<SessionRef> {
        let id = crate::domain::new_id();
        let s = CodexSession { cwd: options.cwd.clone(), model: options.model_id.clone(), env: options.env.clone(), thread_id: None, proc: None, interrupted: false };
        self.sessions.lock().unwrap().insert(id.clone(), Arc::new(Mutex::new(s)));
        Ok(SessionRef { id, native_id: None })
    }

    fn resume_session(&self, options: &SessionOptions, native_id: &str) -> HarnessResult<SessionRef> {
        let id = crate::domain::new_id();
        let s = CodexSession { cwd: options.cwd.clone(), model: options.model_id.clone(), env: options.env.clone(), thread_id: Some(native_id.into()), proc: None, interrupted: false };
        self.sessions.lock().unwrap().insert(id.clone(), Arc::new(Mutex::new(s)));
        Ok(SessionRef { id, native_id: Some(native_id.into()) })
    }

    fn prompt(&self, session: &SessionRef, request_id: &str, text: &str) -> HarnessResult<TurnHandle> {
        let shared = self.session(session)?;
        let (cwd, model, thread_id, env) = {
            let mut s = shared.lock().unwrap();
            if s.proc.is_some() { return Err(HarnessError::RuntimeFailed("a turn is already running in this session".into())); }
            s.interrupted = false;
            (s.cwd.clone(), s.model.clone(), s.thread_id.clone(), s.env.clone())
        };
        let mut cmd = Command::new(&self.binary);
        cmd.envs(env);
        cmd.arg("exec");
        match &thread_id {
            Some(t) => { cmd.args(["resume", t, "--json"]); }
            None => { cmd.args(["--json", "-C"]).arg(&cwd).args(["--sandbox", "workspace-write", "--skip-git-repo-check"]); }
        }
        if let Some(m) = &model { cmd.arg("-c").arg(format!("model=\"{m}\"")); }
        cmd.arg("-");
        scrub_env(&mut cmd);
        let proc = LineProc::spawn(cmd, &cwd, &format!("codex-{}-{request_id}", &session.id))?;
        proc.write_line(text)?;
        proc.close_stdin();
        shared.lock().unwrap().proc = Some(proc.clone());
        let (ev_tx, ev_rx) = channel();
        let (res_tx, res_rx) = channel();
        let stdout = proc.take_stdout().ok_or_else(|| HarnessError::TransportLost("no stdout".into()))?;
        let reader_shared = shared.clone();
        let reader_proc = proc.clone();
        let model = model.clone();
        std::thread::Builder::new()
            .name("codex-exec".into())
            .spawn(move || {
                let mut outcome: Option<TurnOutcome> = None;
                let mut last_error: Option<String> = None;
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                    let kind = msg.get("type").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "thread.started" => {
                            if let Some(t) = msg.get("thread_id").and_then(Value::as_str) {
                                reader_shared.lock().unwrap().thread_id = Some(t.to_string());
                                let _ = ev_tx.send(HarnessEvent::SessionId(t.to_string()));
                            }
                            // Codex never says which model served; the request or the config default did.
                            if let Some(m) = model.clone().or_else(CodexExec::configured_model) {
                                let _ = ev_tx.send(HarnessEvent::ModelChanged { effective: m, reason: if model.is_some() { "requested".into() } else { "config default".into() } });
                            }
                        }
                        "item.started" | "item.completed" | "item.updated" => {
                            if let Some(item) = msg.get("item") {
                                if let Some(ev) = item_event(item, kind == "item.completed", &mut last_error) { let _ = ev_tx.send(ev); }
                            }
                        }
                        "turn.completed" => outcome = Some(TurnOutcome::Completed { stop_reason: "turn.completed".into() }),
                        "turn.failed" => {
                            let reason = msg.pointer("/error/message").and_then(Value::as_str).or(last_error.as_deref()).unwrap_or("turn failed").to_string();
                            outcome = Some(TurnOutcome::Failed { reason });
                        }
                        "error" => {
                            last_error = msg.get("message").and_then(Value::as_str).map(str::to_string);
                            let _ = ev_tx.send(HarnessEvent::Raw(msg.clone()));
                        }
                        _ => { let _ = ev_tx.send(HarnessEvent::Raw(msg.clone())); }
                    }
                }
                // The process ended (stdout closed).
                for _ in 0..50 {
                    if reader_proc.exit_code().is_some() { break; }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                let interrupted = {
                    let mut s = reader_shared.lock().unwrap();
                    s.proc = None;
                    s.interrupted
                };
                let outcome = match outcome {
                    Some(o) if !interrupted => o,
                    _ if interrupted => TurnOutcome::Cancelled,
                    _ => TurnOutcome::Failed { reason: match (reader_proc.exit_code().flatten(), last_error) {
                        (_, Some(e)) => e,
                        (Some(code), None) => format!("codex exited with code {code} before the turn finished"),
                        (None, None) => "codex ended before the turn finished".into(),
                    } },
                };
                drop(ev_tx);
                let _ = res_tx.send(outcome);
            })
            .map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        Ok(TurnHandle { turn_id: format!("codex-{request_id}"), events: ev_rx, result: res_rx })
    }

    fn interrupt(&self, session: &SessionRef) -> HarnessResult<()> {
        let shared = self.session(session)?;
        let proc = {
            let mut s = shared.lock().unwrap();
            s.interrupted = true;
            s.proc.clone()
        };
        if let Some(p) = proc { p.interrupt(); }
        Ok(())
    }

    fn stop(&self, session: &SessionRef) -> HarnessResult<bool> {
        let Some(shared) = self.sessions.lock().unwrap().remove(&session.id) else { return Ok(true) };
        let proc = shared.lock().unwrap().proc.take();
        Ok(proc.map(|p| p.kill()).unwrap_or(true))
    }

    fn list_models(&self, _session: Option<&SessionRef>) -> HarnessResult<Vec<ModelOption>> {
        // Codex has no listing command; the configured default is what runs
        // without a model, and any id the account allows can be requested.
        Ok(Self::configured_model().into_iter().map(|m| ModelOption { id: m, label: "configured default".into(), description: "model = … in ~/.codex/config.toml".into() }).collect())
    }

    fn set_model(&self, session: &SessionRef, model_id: &str) -> HarnessResult<String> {
        let shared = self.session(session)?;
        let mut s = shared.lock().unwrap();
        if s.proc.is_some() { return Err(HarnessError::RuntimeFailed("a turn is running; the model changes between turns".into())); }
        s.model = Some(model_id.into());
        Ok(model_id.into())
    }

    fn respond_permission(&self, _session: &SessionRef, _request_id: &str, _option_id: Option<&str>) -> HarnessResult<()> {
        Err(HarnessError::UnsupportedCapability("permission_requests"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_items_become_guild_events() {
        let mut err = None;
        let cmd = json!({"id": "item_1", "type": "command_execution", "command": "/bin/zsh -lc \"printf 'hi' > hi.txt\ncat hi.txt\"", "exit_code": 0, "status": "completed"});
        match item_event(&cmd, true, &mut err).unwrap() {
            HarnessEvent::Tool { title, kind, status, action, .. } => {
                assert_eq!(kind, "execute");
                assert_eq!(status, "completed");
                assert_eq!(action, "brew");
                assert!(title.starts_with("printf 'hi' > hi.txt"), "{title}");
                assert!(title.ends_with('…'));
            }
            other => panic!("{other:?}"),
        }
        let failed = json!({"id": "item_2", "type": "command_execution", "command": "false", "exit_code": 1, "status": "completed"});
        assert!(matches!(item_event(&failed, true, &mut err).unwrap(), HarnessEvent::Tool { status, .. } if status == "failed"));
        let edit = json!({"id": "item_3", "type": "file_change", "status": "completed", "changes": [{"path": "/w/src/a.rs", "kind": "update"}]});
        assert!(matches!(item_event(&edit, true, &mut err).unwrap(), HarnessEvent::Tool { kind, paths, .. } if kind == "edit" && paths == vec!["/w/src/a.rs".to_string()]));
        let msg = json!({"id": "item_4", "type": "agent_message", "text": "Done."});
        assert!(item_event(&msg, false, &mut err).is_none(), "messages count once, when completed");
        assert_eq!(item_event(&msg, true, &mut err).unwrap(), HarnessEvent::Text("Done.\n".into()));
        let e = json!({"id": "item_5", "type": "error", "message": "model not supported"});
        item_event(&e, true, &mut err);
        assert_eq!(err.as_deref(), Some("model not supported"));
    }
}
