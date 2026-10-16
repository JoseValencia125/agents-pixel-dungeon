//! Kiro over the Agent Client Protocol: `kiro-cli acp --agent-engine=v3`,
//! JSON-RPC 2.0 with one object per line on stdin/stdout, stderr kept in
//! a log file. One process per session, so a crash takes one attempt
//! with it and the cwd is the attempt's worktree. Verified against
//! kiro-cli 2.31.0 (KAS 0.66.34): `session/new` announces `configOptions`
//! with `model`, and the model changes with `session/set_config_option`.

use super::*;
use crate::domain::{HarnessId, HarnessKind, ModelOption, PermissionOption};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const ADAPTER_VERSION: &str = "kiro-acp-1";
const INIT_TIMEOUT: Duration = Duration::from_secs(45);
const SESSION_TIMEOUT: Duration = Duration::from_secs(90);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine { V2, V3 }

pub struct KiroAcp {
    pub binary: PathBuf,
    pub engine: Engine,
    procs: Mutex<HashMap<String, Arc<Proc>>>,
}

/// One `kiro-cli acp` process and the bookkeeping its reader thread shares.
struct Proc {
    child: Mutex<Child>,
    pgid: i32,
    stdin: Mutex<ChildStdin>,
    native_session: Mutex<Option<String>>,
    inner: Arc<Mutex<Inner>>,
    log_path: PathBuf,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    waiting: HashMap<u64, Sender<Result<Value, Value>>>,
    /// Where `session/update` goes while a turn runs.
    turn: Option<Sender<HarnessEvent>>,
    /// Permission requests the agent is waiting on: our request id → JSON-RPC id.
    permissions: HashMap<String, Value>,
    tools: HashMap<String, (String, String)>,
    config_options: Value,
    models: Vec<ModelOption>,
    closed: bool,
}

impl KiroAcp {
    pub fn new() -> KiroAcp { KiroAcp { binary: find_binary(), engine: Engine::V3, procs: Mutex::new(HashMap::new()) } }

    fn proc_for(&self, session: &SessionRef) -> HarnessResult<Arc<Proc>> {
        self.procs.lock().unwrap().get(&session.id).cloned().ok_or_else(|| HarnessError::TransportLost("session has no process".into()))
    }

    fn spawn(&self, cwd: &PathBuf, log_name: &str) -> HarnessResult<Arc<Proc>> {
        if !self.binary.exists() {
            return Err(HarnessError::RuntimeFailed(format!("kiro-cli not found at {}", self.binary.display())));
        }
        std::fs::create_dir_all(cwd).map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        let logs = crate::paths::home().join("logs");
        let _ = std::fs::create_dir_all(&logs);
        let log_path = logs.join(format!("{log_name}.stderr.log"));
        let stderr = std::fs::File::create(&log_path).map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        let mut cmd = Command::new(&self.binary);
        cmd.arg("acp");
        if self.engine == Engine::V3 { cmd.arg("--agent-engine=v3"); }
        cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(stderr));
        crate::platform::detach(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| HarnessError::RuntimeFailed(format!("cannot start kiro-cli acp: {e}")))?;
        let pgid = child.id() as i32;
        let stdin = child.stdin.take().ok_or_else(|| HarnessError::RuntimeFailed("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| HarnessError::RuntimeFailed("no stdout".into()))?;
        let inner: Arc<Mutex<Inner>> = Arc::default();
        let reader_inner = inner.clone();
        let proc = Arc::new(Proc { child: Mutex::new(child), pgid, stdin: Mutex::new(stdin), native_session: Mutex::new(None), inner, log_path });
        let writer = proc.clone();
        std::thread::Builder::new()
            .name("kiro-acp-reader".into())
            .spawn(move || read_loop(stdout, reader_inner, writer))
            .map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        Ok(proc)
    }
}

impl Default for KiroAcp {
    fn default() -> Self { KiroAcp::new() }
}

fn find_binary() -> PathBuf {
    if let Ok(p) = std::env::var("KIRO_CLI_BIN") {
        if !p.is_empty() { return PathBuf::from(p); }
    }
    let home = dirs::home_dir().unwrap_or_default();
    for dir in [home.join(".local/bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")] {
        if let Some(found) = crate::platform::in_dir(&dir, "kiro-cli") { return found; }
    }
    crate::platform::which("kiro-cli").unwrap_or_else(|| PathBuf::from("kiro-cli"))
}

impl Proc {
    fn call(&self, method: &str, params: Value, timeout: Duration) -> HarnessResult<Value> {
        let (tx, rx) = channel();
        let id = {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed { return Err(HarnessError::TransportLost("process ended".into())); }
            inner.next_id += 1;
            let id = inner.next_id;
            inner.waiting.insert(id, tx);
            id
        };
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        match rx.recv_timeout(timeout) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(classify(&error)),
            Err(_) => {
                self.inner.lock().unwrap().waiting.remove(&id);
                if self.inner.lock().unwrap().closed { Err(HarnessError::TransportLost("process ended".into())) } else { Err(HarnessError::Timeout) }
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> HarnessResult<()> {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn write(&self, message: &Value) -> HarnessResult<()> {
        let mut line = message.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).map_err(|e| HarnessError::TransportLost(e.to_string()))
    }

    fn native(&self) -> HarnessResult<String> {
        self.native_session.lock().unwrap().clone().ok_or_else(|| HarnessError::RuntimeFailed("no native session".into()))
    }

    fn kill(&self) -> bool {
        crate::platform::terminate_tree(self.pgid, false);
        let mut child = self.child.lock().unwrap();
        for _ in 0..30 {
            if let Ok(Some(_)) = child.try_wait() { return true; }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        crate::platform::terminate_tree(self.pgid, true);
        matches!(child.try_wait(), Ok(Some(_)))
    }
}

fn classify(error: &Value) -> HarnessError {
    let message = error.get("message").and_then(Value::as_str).unwrap_or("").to_string();
    let lower = message.to_ascii_lowercase();
    if lower.contains("auth") || lower.contains("login") || lower.contains("credential") { HarnessError::AuthRequired(message) }
    else if lower.contains("rate") || lower.contains("throttl") || lower.contains("quota") { HarnessError::RateLimited { retry_after_seconds: None } }
    else if lower.contains("model") { HarnessError::ModelUnavailable(message) }
    else if lower.contains("method not found") { HarnessError::UnsupportedCapability("method") }
    else { HarnessError::RuntimeFailed(format!("{message} {}", error.get("data").map(|d| d.to_string()).unwrap_or_default()).trim().to_string()) }
}

/// Everything the process says: responses go to their caller, session
/// updates to the running turn, permission requests to the turn too (with
/// the JSON-RPC id kept so the answer can be written later), and any other
/// request from the agent is declined.
fn read_loop(stdout: std::process::ChildStdout, inner: Arc<Mutex<Inner>>, proc: Arc<Proc>) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let method = msg.get("method").and_then(Value::as_str).map(str::to_string);
        let id = msg.get("id").cloned();
        match (method, id) {
            (None, Some(id)) => {
                let Some(id) = id.as_u64() else { continue };
                let waiter = inner.lock().unwrap().waiting.remove(&id);
                if let Some(tx) = waiter {
                    let outcome = if let Some(err) = msg.get("error") { Err(err.clone()) } else { Ok(msg.get("result").cloned().unwrap_or(Value::Null)) };
                    let _ = tx.send(outcome);
                }
            }
            (Some(method), None) => handle_notification(&method, &msg, &inner),
            (Some(method), Some(id)) => handle_request(&method, id, &msg, &inner, &proc),
            (None, None) => {}
        }
    }
    let mut guard = inner.lock().unwrap();
    guard.closed = true;
    guard.waiting.clear();
    guard.turn = None;
}

fn handle_notification(method: &str, msg: &Value, inner: &Arc<Mutex<Inner>>) {
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let mut guard = inner.lock().unwrap();
    let Some(turn) = guard.turn.clone() else { return };
    if method != "session/update" {
        let _ = turn.send(HarnessEvent::Raw(msg.clone()));
        return;
    }
    let update = params.get("update").cloned().unwrap_or(Value::Null);
    let kind = update.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
    let text = || update.get("content").and_then(|c| c.get("text")).and_then(Value::as_str).unwrap_or("").to_string();
    let event = match kind {
        "agent_message_chunk" => HarnessEvent::Text(text()),
        "agent_thought_chunk" => HarnessEvent::Thought(text()),
        "tool_call" | "tool_call_update" => {
            let id = update.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_string();
            let known = guard.tools.get(&id).cloned();
            let title = update.get("title").and_then(Value::as_str).map(str::to_string).or(known.as_ref().map(|k| k.0.clone())).unwrap_or_default();
            let tool_kind = update.get("kind").and_then(Value::as_str).map(str::to_string).or(known.map(|k| k.1)).unwrap_or_else(|| "other".into());
            let status = update.get("status").and_then(Value::as_str).unwrap_or(if kind == "tool_call" { "pending" } else { "in_progress" }).to_string();
            guard.tools.insert(id.clone(), (title.clone(), tool_kind.clone()));
            let action = action_for_tool(&tool_kind, &title).to_string();
            let paths: Vec<String> = update.get("locations").and_then(Value::as_array).into_iter().flatten()
                .filter_map(|l| l.get("path").and_then(Value::as_str)).map(str::to_string).collect();
            HarnessEvent::Tool { id, title, kind: tool_kind, status, action, paths }
        }
        "plan" => HarnessEvent::Plan(update.get("entries").cloned().unwrap_or(Value::Null)),
        "config_option_update" | "current_mode_update" | "available_commands_update" => {
            if kind == "config_option_update" {
                if let Some(options) = update.get("configOptions") {
                    if let Some(model) = options.as_array().and_then(|a| a.iter().find(|o| o.get("id").and_then(Value::as_str) == Some("model"))) {
                        if let Some(current) = model.get("currentValue").and_then(Value::as_str) {
                            let _ = turn.send(HarnessEvent::ModelChanged { effective: current.into(), reason: "config_option_update".into() });
                        }
                    }
                }
            }
            HarnessEvent::Raw(msg.clone())
        }
        _ => HarnessEvent::Raw(msg.clone()),
    };
    let _ = turn.send(event);
}

fn handle_request(method: &str, id: Value, msg: &Value, inner: &Arc<Mutex<Inner>>, proc: &Arc<Proc>) {
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    if method == "session/request_permission" {
        let tool = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let title = tool.get("title").and_then(Value::as_str).unwrap_or("Permission").to_string();
        let detail = tool.get("rawInput").map(|v| v.to_string()).or_else(|| tool.get("content").map(|v| v.to_string())).unwrap_or_default();
        let options: Vec<PermissionOption> = params
            .get("options")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|o| PermissionOption {
                        id: o.get("optionId").and_then(Value::as_str).unwrap_or("").into(),
                        name: o.get("name").and_then(Value::as_str).unwrap_or("").into(),
                        kind: o.get("kind").and_then(Value::as_str).unwrap_or("").into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let request_id = format!("perm-{}", id.as_u64().map(|n| n.to_string()).unwrap_or_else(|| id.to_string()));
        let mut guard = inner.lock().unwrap();
        guard.permissions.insert(request_id.clone(), id);
        if let Some(turn) = guard.turn.clone() {
            let _ = turn.send(HarnessEvent::PermissionRequested { request_id, title, detail, options });
        } else {
            // No turn to ask: decline rather than hang the agent.
            let jsonrpc_id = guard.permissions.remove(&request_id).unwrap();
            drop(guard);
            let _ = proc.write(&json!({"jsonrpc": "2.0", "id": jsonrpc_id, "result": {"outcome": {"outcome": "cancelled"}}}));
        }
        return;
    }
    // fs/*, terminal/* and extension requests: not offered in our client capabilities.
    let _ = proc.write(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("{method} is not supported by this client")}}));
}

fn parse_models(result: &Value) -> (Value, Vec<ModelOption>) {
    // V3: configOptions[id=model].options; V2: models.availableModels.
    let config = result.get("configOptions").cloned().unwrap_or(Value::Null);
    let mut models = vec![];
    if let Some(model) = config.as_array().and_then(|a| a.iter().find(|o| o.get("id").and_then(Value::as_str) == Some("model"))) {
        for o in model.get("options").and_then(Value::as_array).into_iter().flatten() {
            models.push(ModelOption {
                id: o.get("value").and_then(Value::as_str).unwrap_or("").into(),
                label: o.get("name").and_then(Value::as_str).unwrap_or("").into(),
                description: o.get("description").and_then(Value::as_str).unwrap_or("").into(),
            });
        }
    }
    if models.is_empty() {
        for o in result.get("models").and_then(|m| m.get("availableModels")).and_then(Value::as_array).into_iter().flatten() {
            models.push(ModelOption {
                id: o.get("modelId").and_then(Value::as_str).unwrap_or("").into(),
                label: o.get("name").and_then(Value::as_str).unwrap_or("").into(),
                description: o.get("description").and_then(Value::as_str).unwrap_or("").into(),
            });
        }
    }
    (config, models)
}

fn current_model(result: &Value) -> Option<String> {
    if let Some(model) = result.get("configOptions").and_then(Value::as_array).and_then(|a| a.iter().find(|o| o.get("id").and_then(Value::as_str) == Some("model"))) {
        return model.get("currentValue").and_then(Value::as_str).map(str::to_string);
    }
    result.get("models").and_then(|m| m.get("currentModelId")).and_then(Value::as_str).map(str::to_string)
}

impl KiroAcp {
    /// Start a kiro-cli process and open a session in it: a new one, or
    /// (`load` = its native id) one kiro-cli persisted, through ACP's
    /// `session/load`, which replays its history first.
    fn open_session(&self, options: &SessionOptions, load: Option<&str>) -> HarnessResult<SessionRef> {
        let id = crate::domain::new_id();
        let proc = self.spawn(&options.cwd, &format!("kiro-{id}"))?;
        let init = proc.call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}}, "clientInfo": {"name": "agents-pixel-dungeon-guild", "version": env!("CARGO_PKG_VERSION")}}),
            INIT_TIMEOUT,
        );
        let init = match init {
            Ok(v) => v,
            Err(err) => {
                proc.kill();
                return Err(err);
            }
        };
        if self.engine == Engine::V3 && init.pointer("/agentCapabilities/_meta/kiro/extensionMethods").is_none() {
            proc.kill();
            return Err(HarnessError::RuntimeFailed("kiro-cli did not start the v3 engine (no extensionMethods announced)".into()));
        }
        if load.is_some() && init.pointer("/agentCapabilities/loadSession").and_then(Value::as_bool) != Some(true) {
            proc.kill();
            return Err(HarnessError::UnsupportedCapability("resume_session"));
        }
        let mut params = json!({"cwd": options.cwd.display().to_string(), "mcpServers": []});
        if let Some(native) = load { params["sessionId"] = json!(native); }
        if self.engine == Engine::V3 && load.is_none() {
            let mut meta = serde_json::Map::new();
            if let Some(m) = &options.model_id { meta.insert("modelId".into(), json!(m)); }
            if let Some(mode) = &options.mode { meta.insert("modeId".into(), json!(mode)); }
            if !meta.is_empty() { params["_meta"] = json!({"kiro": meta}); }
        }
        let method = if load.is_some() { "session/load" } else { "session/new" };
        let created = match proc.call(method, params, SESSION_TIMEOUT) {
            Ok(v) => v,
            Err(err) => {
                proc.kill();
                return Err(err);
            }
        };
        let native = created.get("sessionId").and_then(Value::as_str).map(str::to_string).or_else(|| load.map(str::to_string));
        let Some(native) = native else {
            proc.kill();
            return Err(HarnessError::InvalidOutput(format!("{method} returned no sessionId")));
        };
        *proc.native_session.lock().unwrap() = Some(native.clone());
        let (config, models) = parse_models(&created);
        {
            let mut inner = proc.inner.lock().unwrap();
            inner.config_options = config;
            inner.models = models;
        }
        self.procs.lock().unwrap().insert(id.clone(), proc.clone());
        // Make the requested model and mode effective, and fail loudly if they are not.
        let session = SessionRef { id: id.clone(), native_id: Some(native.clone()) };
        if let Some(mode) = &options.mode {
            let current = created.pointer("/modes/currentModeId").and_then(Value::as_str);
            if current != Some(mode.as_str()) {
                proc.call("session/set_mode", json!({"sessionId": native, "modeId": mode}), CALL_TIMEOUT)?;
            }
        }
        if let Some(model) = &options.model_id {
            if current_model(&created).as_deref() != Some(model.as_str()) {
                self.set_model(&session, model)?;
            }
        }
        Ok(session)
    }
}

impl HarnessAdapter for KiroAcp {
    fn kind(&self) -> HarnessKind { HarnessId::named("kiro") }

    fn probe(&self) -> HarnessResult<Capabilities> {
        if !self.binary.exists() {
            return Err(HarnessError::RuntimeFailed(format!("kiro-cli not found ({})", self.binary.display())));
        }
        let output = Command::new(&self.binary).arg("--version").output().map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut extensions = BTreeMap::new();
        extensions.insert("engine".into(), json!(if self.engine == Engine::V3 { "v3" } else { "v2" }));
        extensions.insert("binary".into(), json!(self.binary.display().to_string()));
        Ok(Capabilities {
            adapter_version: ADAPTER_VERSION.into(),
            runtime_version: version,
            protocol_version: Some("acp-1".into()),
            transport: Transport::Acp,
            create_session: Support::Supported,
            // v3 announces loadSession and fork in initialize; v2 is not probed.
            resume_session: if self.engine == Engine::V3 { Support::Supported } else { Support::Unknown },
            cancel_turn: Support::Supported,
            structured_events: Support::Supported,
            permission_requests: Support::Supported,
            model_discovery: Support::Supported,
            model_switch: ModelSwitch::BetweenTurns,
            usage_reporting: Support::Unknown,
            native_fork: if self.engine == Engine::V3 { Support::Supported } else { Support::Unknown },
            mcp: Support::Supported,
            subagents: Support::Unknown,
            plan_mode: if self.engine == Engine::V3 { Support::Supported } else { Support::Unknown },
            screenshots: Support::Unsupported,
            steering: Support::Unsupported,
            max_concurrent_sessions: None,
            extensions,
        })
    }

    fn create_session(&self, options: &SessionOptions) -> HarnessResult<SessionRef> {
        self.open_session(options, None)
    }

    fn resume_session(&self, options: &SessionOptions, native_id: &str) -> HarnessResult<SessionRef> {
        self.open_session(options, Some(native_id))
    }

    fn prompt(&self, session: &SessionRef, request_id: &str, text: &str) -> HarnessResult<TurnHandle> {
        let proc = self.proc_for(session)?;
        let native = proc.native()?;
        let (ev_tx, ev_rx) = channel();
        let (res_tx, res_rx) = channel();
        {
            let mut inner = proc.inner.lock().unwrap();
            if inner.turn.is_some() { return Err(HarnessError::RuntimeFailed("a turn is already running in this session".into())); }
            inner.turn = Some(ev_tx);
            inner.tools.clear();
        }
        let params = json!({"sessionId": native, "prompt": [{"type": "text", "text": text}]});
        let (tx, rx) = channel();
        let id = {
            let mut inner = proc.inner.lock().unwrap();
            inner.next_id += 1;
            let id = inner.next_id;
            inner.waiting.insert(id, tx);
            id
        };
        if let Err(err) = proc.write(&json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt", "params": params})) {
            proc.inner.lock().unwrap().turn = None;
            return Err(err);
        }
        let proc_for_turn = proc.clone();
        std::thread::Builder::new()
            .name("kiro-acp-turn".into())
            .spawn(move || {
                let outcome = match rx.recv() {
                    Ok(Ok(result)) => {
                        let stop = result.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_string();
                        if stop == "cancelled" { TurnOutcome::Cancelled } else { TurnOutcome::Completed { stop_reason: stop } }
                    }
                    Ok(Err(error)) => TurnOutcome::Failed { reason: classify(&error).to_string() },
                    Err(_) => TurnOutcome::Failed { reason: "TRANSPORT_LOST: kiro-cli ended before the turn finished".into() },
                };
                proc_for_turn.inner.lock().unwrap().turn = None;
                let _ = res_tx.send(outcome);
            })
            .map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        Ok(TurnHandle { turn_id: format!("turn-{request_id}"), events: ev_rx, result: res_rx })
    }

    fn interrupt(&self, session: &SessionRef) -> HarnessResult<()> {
        let proc = self.proc_for(session)?;
        let native = proc.native()?;
        proc.notify("session/cancel", json!({"sessionId": native}))
    }

    fn stop(&self, session: &SessionRef) -> HarnessResult<bool> {
        let Some(proc) = self.procs.lock().unwrap().remove(&session.id) else { return Ok(true) };
        if let Ok(native) = proc.native() {
            let _ = proc.call("session/close", json!({"sessionId": native}), Duration::from_secs(5));
        }
        Ok(proc.kill())
    }

    fn list_models(&self, session: Option<&SessionRef>) -> HarnessResult<Vec<ModelOption>> {
        if let Some(session) = session {
            let proc = self.proc_for(session)?;
            return Ok(proc.inner.lock().unwrap().models.clone());
        }
        // No session: open a throwaway one in the guild's own folder.
        let cwd = crate::paths::home().join("probe");
        let session = self.create_session(&SessionOptions { cwd, model_id: None, mode: None, role_prompt: String::new() })?;
        let models = self.list_models(Some(&session));
        let _ = self.stop(&session);
        models
    }

    fn set_model(&self, session: &SessionRef, model_id: &str) -> HarnessResult<String> {
        let proc = self.proc_for(session)?;
        let native = proc.native()?;
        let result = match self.engine {
            Engine::V3 => proc.call("session/set_config_option", json!({"sessionId": native, "configId": "model", "value": model_id}), CALL_TIMEOUT)?,
            Engine::V2 => proc.call("session/set_model", json!({"sessionId": native, "modelId": model_id}), CALL_TIMEOUT)?,
        };
        let effective = current_model(&result).unwrap_or_else(|| model_id.to_string());
        if effective != model_id {
            return Err(HarnessError::ModelUnavailable(format!("asked for {model_id}, provider kept {effective}")));
        }
        Ok(effective)
    }

    fn respond_permission(&self, session: &SessionRef, request_id: &str, option_id: Option<&str>) -> HarnessResult<()> {
        let proc = self.proc_for(session)?;
        let jsonrpc_id = proc.inner.lock().unwrap().permissions.remove(request_id);
        let Some(jsonrpc_id) = jsonrpc_id else { return Err(HarnessError::InvalidOutput(format!("no pending permission {request_id}"))) };
        let outcome = match option_id {
            Some(option) => json!({"outcome": "selected", "optionId": option}),
            None => json!({"outcome": "cancelled"}),
        };
        proc.write(&json!({"jsonrpc": "2.0", "id": jsonrpc_id, "result": {"outcome": outcome}}))
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.log_path.as_path();
        crate::platform::terminate_tree(self.pgid, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_come_from_v3_config_options_or_v2_model_list() {
        let v3 = json!({"configOptions": [{"id": "model", "currentValue": "auto", "options": [{"value": "auto", "name": "auto"}, {"value": "claude-sonnet-5.5", "name": "claude-sonnet-5.5"}]}]});
        let (_, models) = parse_models(&v3);
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["auto", "claude-sonnet-5.5"]);
        assert_eq!(current_model(&v3).as_deref(), Some("auto"));
        let v2 = json!({"models": {"currentModelId": "auto", "availableModels": [{"modelId": "gpt-5.6-sol", "name": "gpt-5.6-sol"}]}});
        let (_, models) = parse_models(&v2);
        assert_eq!(models[0].id, "gpt-5.6-sol");
    }

    #[test]
    fn tool_kinds_map_to_room_actions() {
        assert_eq!(action_for_tool("edit", "write_file"), "forge");
        assert_eq!(action_for_tool("execute", "bash"), "brew");
        assert_eq!(action_for_tool("other", "Delegate to subagent"), "summon");
        assert_eq!(action_for_tool("other", "something"), "type");
    }
}
