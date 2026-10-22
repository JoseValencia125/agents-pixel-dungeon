//! A harness that follows a script, so the service can be tested without
//! a provider: it writes a file in the workspace, reports tools, asks for
//! a permission when told to, and ends how the script says.

use super::*;
use crate::domain::{HarnessId, HarnessKind, ModelOption, PermissionOption};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum MockBehaviour {
    /// Write `file` with `content`, then finish the turn.
    Succeed { file: String, content: String },
    /// Ask for a permission first; continue like Succeed once allowed.
    AskPermission { file: String, content: String },
    /// Report a failed turn.
    Fail(String),
    /// Write `file`, then fail the turn: work left in the worktree.
    WriteThenFail { file: String, content: String, reason: String },
    /// Never answer, so timeouts and cancellation can be exercised.
    Hang,
    /// Say this and finish, writing nothing.
    Reply(String),
    /// Write the JSON after `GUILD_PARAMS:` in the prompt to this file:
    /// a variant builder that applies exactly its parameters.
    WriteParams { file: String },
}

#[derive(Default)]
struct MockState {
    sessions: Vec<(String, PathBuf, Option<String>)>,
    pending: Vec<(String, String, std::sync::mpsc::Sender<bool>)>,
    interrupted: Vec<String>,
    prompts: Vec<String>,
    resumed: Vec<String>,
}

pub type PlayerFn = Arc<dyn Fn(&str) -> String + Send + Sync>;

pub struct MockHarness {
    behaviour: Mutex<MockBehaviour>,
    /// What the mock answers when the prompt is a judge's (it lists candidates).
    judge_reply: Mutex<Option<String>>,
    /// What the mock answers when the prompt is an analyst's (it lists metrics).
    analysis_reply: Mutex<Option<String>>,
    /// How the mock answers an AI player's turn (the prompt has "## Your move").
    player: Mutex<Option<PlayerFn>>,
    orchestrator_reply: Mutex<Option<String>>,
    /// How the mock rules when the guild's orchestrator is woken for a decision.
    decider: Mutex<Option<PlayerFn>>,
    /// Milliseconds the turn takes, so rooms show working for a moment.
    pub delay_ms: u64,
    state: Arc<Mutex<MockState>>,
}

impl MockHarness {
    pub fn new(behaviour: MockBehaviour) -> MockHarness {
        MockHarness { behaviour: Mutex::new(behaviour), judge_reply: Mutex::new(None), analysis_reply: Mutex::new(None), player: Mutex::new(None), orchestrator_reply: Mutex::new(None), decider: Mutex::new(None), delay_ms: 50, state: Arc::default() }
    }
    pub fn set_behaviour(&self, b: MockBehaviour) { *self.behaviour.lock().unwrap() = b; }
    /// Answer judge prompts with this text (a JSON block naming a winner).
    pub fn set_judge_reply(&self, text: impl Into<String>) { *self.judge_reply.lock().unwrap() = Some(text.into()); }
    /// Play AI-player turns with this: it gets the prompt, returns the answer.
    pub fn set_player(&self, f: impl Fn(&str) -> String + Send + Sync + 'static) { *self.player.lock().unwrap() = Some(Arc::new(f)); }
    pub fn set_analysis_reply(&self, text: impl Into<String>) { *self.analysis_reply.lock().unwrap() = Some(text.into()); }
    pub fn set_orchestrator_reply(&self, text: impl Into<String>) { *self.orchestrator_reply.lock().unwrap() = Some(text.into()); }
    /// Rule on the orchestrator's decisions with this: it gets the prompt, returns the answer.
    pub fn set_decider(&self, f: impl Fn(&str) -> String + Send + Sync + 'static) { *self.decider.lock().unwrap() = Some(Arc::new(f)); }
    /// Session ids that were reopened through `resume_session`.
    /// Every prompt sent, in order.
    pub fn prompts(&self) -> Vec<String> { self.state.lock().unwrap().prompts.clone() }
    pub fn resumed(&self) -> Vec<String> { self.state.lock().unwrap().resumed.clone() }
}

impl HarnessAdapter for MockHarness {
    fn kind(&self) -> HarnessKind { HarnessId::named("mock") }

    fn probe(&self) -> HarnessResult<Capabilities> {
        Ok(Capabilities {
            adapter_version: "mock-1".into(),
            runtime_version: "mock".into(),
            protocol_version: None,
            transport: Transport::Mock,
            create_session: Support::Supported,
            resume_session: Support::Supported,
            cancel_turn: Support::Supported,
            structured_events: Support::Supported,
            permission_requests: Support::Supported,
            model_discovery: Support::Supported,
            model_switch: ModelSwitch::BetweenTurns,
            usage_reporting: Support::Unsupported,
            native_fork: Support::Unsupported,
            mcp: Support::Unsupported,
            subagents: Support::Unsupported,
            plan_mode: Support::Unsupported,
            screenshots: Support::Unsupported,
            steering: Support::Unsupported,
            max_concurrent_sessions: None,
            extensions: Default::default(),
        })
    }

    fn create_session(&self, options: &SessionOptions) -> HarnessResult<SessionRef> {
        let id = crate::domain::new_id();
        self.state.lock().unwrap().sessions.push((id.clone(), options.cwd.clone(), options.model_id.clone()));
        Ok(SessionRef { id: id.clone(), native_id: Some(format!("mock-{id}")) })
    }

    fn resume_session(&self, options: &SessionOptions, native_id: &str) -> HarnessResult<SessionRef> {
        let Some(id) = native_id.strip_prefix("mock-") else { return Err(HarnessError::InvalidOutput(format!("not a mock session: {native_id}"))) };
        let id = id.to_string();
        let mut st = self.state.lock().unwrap();
        st.resumed.push(id.clone());
        st.sessions.retain(|s| s.0 != id);
        st.sessions.push((id.clone(), options.cwd.clone(), options.model_id.clone()));
        Ok(SessionRef { id, native_id: Some(native_id.into()) })
    }

    fn prompt(&self, session: &SessionRef, request_id: &str, text: &str) -> HarnessResult<TurnHandle> {
        self.state.lock().unwrap().prompts.push(text.to_string());
        let mut behaviour = self.behaviour.lock().unwrap().clone();
        if text.contains("## Candidates") {
            if let Some(reply) = self.judge_reply.lock().unwrap().clone() { behaviour = MockBehaviour::Reply(reply); }
        }
        if text.contains("## Guild conversation") {
            if let Some(reply) = self.orchestrator_reply.lock().unwrap().clone() { behaviour = MockBehaviour::Reply(reply); }
        }
        if text.contains("## Decision needed") {
            if let Some(decider) = self.decider.lock().unwrap().clone() { behaviour = MockBehaviour::Reply(decider(text)); }
        }
        if text.contains("## Playtest metrics") {
            if let Some(reply) = self.analysis_reply.lock().unwrap().clone() { behaviour = MockBehaviour::Reply(reply); }
        }
        if text.contains("## Your move") || text.contains("The game is over") || text.contains("could not be read") {
            if let Some(player) = self.player.lock().unwrap().clone() { behaviour = MockBehaviour::Reply(player(text)); }
        }
        if let MockBehaviour::WriteParams { file } = &behaviour {
            let params = text.lines().find_map(|l| l.trim().strip_prefix("GUILD_PARAMS:")).map(|s| s.trim().to_string()).unwrap_or_else(|| "{}".into());
            behaviour = MockBehaviour::Succeed { file: file.clone(), content: format!("{params}\n") };
        }
        let (cwd, _model) = {
            let st = self.state.lock().unwrap();
            let Some(s) = st.sessions.iter().find(|s| s.0 == session.id) else { return Err(HarnessError::RuntimeFailed("unknown session".into())) };
            (s.1.clone(), s.2.clone())
        };
        let (ev_tx, ev_rx) = channel();
        let (res_tx, res_rx) = channel();
        let state = self.state.clone();
        let session_id = session.id.clone();
        let delay = self.delay_ms;
        let turn_id = format!("turn-{request_id}");
        std::thread::Builder::new()
            .name("mock-harness".into())
            .spawn(move || {
                let _ = ev_tx.send(HarnessEvent::SessionId(format!("mock-{session_id}")));
                let finish = |file: &str, content: &str| {
                    let _ = ev_tx.send(HarnessEvent::Tool { id: "t1".into(), title: "read_file".into(), kind: "read".into(), status: "completed".into(), action: "read".into(), paths: vec![] });
                    std::thread::sleep(Duration::from_millis(delay));
                    let path = cwd.join(file);
                    if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
                    let _ = std::fs::write(&path, content);
                    let _ = ev_tx.send(HarnessEvent::Tool { id: "t2".into(), title: "write_file".into(), kind: "edit".into(), status: "completed".into(), action: "forge".into(), paths: vec![file.to_string()] });
                    let _ = ev_tx.send(HarnessEvent::Text(format!("Wrote {file}. Done.")));
                    let _ = res_tx.send(TurnOutcome::Completed { stop_reason: "end_turn".into() });
                };
                match behaviour {
                    MockBehaviour::Succeed { file, content } => finish(&file, &content),
                    MockBehaviour::AskPermission { file, content } => {
                        let (tx, rx) = channel();
                        state.lock().unwrap().pending.push((session_id.clone(), "perm-1".into(), tx));
                        let _ = ev_tx.send(HarnessEvent::PermissionRequested {
                            request_id: "perm-1".into(),
                            // Not routine, so the guild's policy leaves it to the user.
                            title: format!("mock-install {file}"),
                            detail: "The mock wants to write a file.".into(),
                            options: vec![
                                PermissionOption { id: "allow".into(), name: "Allow".into(), kind: "allow_once".into() },
                                PermissionOption { id: "always".into(), name: "Always allow".into(), kind: "allow_always".into() },
                                PermissionOption { id: "deny".into(), name: "Deny".into(), kind: "reject_once".into() },
                            ],
                        });
                        match rx.recv() {
                            Ok(true) => finish(&file, &content),
                            Ok(false) => { let _ = res_tx.send(TurnOutcome::Failed { reason: "permission denied".into() }); }
                            Err(_) => { let _ = res_tx.send(TurnOutcome::Cancelled); }
                        }
                    }
                    MockBehaviour::Reply(text) => {
                        std::thread::sleep(Duration::from_millis(delay));
                        let _ = ev_tx.send(HarnessEvent::Text(text));
                        let _ = res_tx.send(TurnOutcome::Completed { stop_reason: "end_turn".into() });
                    }
                    MockBehaviour::WriteParams { .. } => unreachable!("turned into Succeed above"),
                    MockBehaviour::Fail(reason) => {
                        std::thread::sleep(Duration::from_millis(delay));
                        let _ = res_tx.send(TurnOutcome::Failed { reason });
                    }
                    MockBehaviour::WriteThenFail { file, content, reason } => {
                        let _ = std::fs::write(cwd.join(&file), &content);
                        std::thread::sleep(Duration::from_millis(delay));
                        let _ = res_tx.send(TurnOutcome::Failed { reason });
                    }
                    MockBehaviour::Hang => loop {
                        std::thread::sleep(Duration::from_millis(50));
                        if state.lock().unwrap().interrupted.contains(&session_id) {
                            let _ = res_tx.send(TurnOutcome::Cancelled);
                            break;
                        }
                    },
                }
            })
            .map_err(|e| HarnessError::RuntimeFailed(e.to_string()))?;
        Ok(TurnHandle { turn_id, events: ev_rx, result: res_rx })
    }

    fn interrupt(&self, session: &SessionRef) -> HarnessResult<()> {
        let mut st = self.state.lock().unwrap();
        st.interrupted.push(session.id.clone());
        st.pending.retain(|p| p.0 != session.id);
        Ok(())
    }

    fn stop(&self, session: &SessionRef) -> HarnessResult<bool> {
        let mut st = self.state.lock().unwrap();
        st.interrupted.push(session.id.clone());
        st.sessions.retain(|s| s.0 != session.id);
        st.pending.retain(|p| p.0 != session.id);
        Ok(true)
    }

    fn list_models(&self, _session: Option<&SessionRef>) -> HarnessResult<Vec<ModelOption>> {
        Ok(vec![
            ModelOption { id: "mock-fast".into(), label: "mock-fast".into(), description: "Deterministic".into() },
            ModelOption { id: "mock-slow".into(), label: "mock-slow".into(), description: "Deterministic, slower".into() },
        ])
    }

    fn set_model(&self, session: &SessionRef, model_id: &str) -> HarnessResult<String> {
        if !model_id.starts_with("mock-") { return Err(HarnessError::ModelUnavailable(model_id.into())); }
        let mut st = self.state.lock().unwrap();
        if let Some(s) = st.sessions.iter_mut().find(|s| s.0 == session.id) { s.2 = Some(model_id.into()); }
        Ok(model_id.into())
    }

    fn respond_permission(&self, session: &SessionRef, request_id: &str, option_id: Option<&str>) -> HarnessResult<()> {
        let mut st = self.state.lock().unwrap();
        let Some(pos) = st.pending.iter().position(|p| p.0 == session.id && p.1 == request_id) else {
            return Err(HarnessError::InvalidOutput(format!("no pending permission {request_id}")));
        };
        let (_, _, tx) = st.pending.remove(pos);
        let _ = tx.send(matches!(option_id, Some("allow" | "always")));
        Ok(())
    }
}
