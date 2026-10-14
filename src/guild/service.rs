//! The service: owns the store, the adapters and every running attempt.
//! Commands come in over the socket, the scheduler ticks once a second,
//! and each attempt is driven by its own thread from provisioning to a
//! candidate commit. Only this code changes a state; harnesses report.

use crate::api::{ApiError, ApiResult, Handler};
use crate::domain::*;
use crate::harness::{HarnessAdapter, HarnessEvent, SessionOptions, SessionRef, Support, TurnOutcome};
use crate::store::Store;
use crate::{config, scheduler, workspaces};
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Settings {
    pub concurrency: usize,
    pub attempt_timeout: Duration,
    pub cancel_timeout: Duration,
    pub setup_timeout_seconds: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { concurrency: 2, attempt_timeout: Duration::from_secs(30 * 60), cancel_timeout: Duration::from_secs(15), setup_timeout_seconds: 600 }
    }
}

struct Running {
    session: SessionRef,
    harness: HarnessKind,
    cancel_requested: bool,
    /// Stop the turn but keep the attempt: whatever is in the worktree
    /// becomes the candidate.
    interrupt_requested: bool,
    started: Instant,
}

pub struct Service {
    store: Mutex<Store>,
    adapters: Mutex<HashMap<HarnessKind, Arc<dyn HarnessAdapter>>>,
    harness_status: Mutex<Vec<HarnessStatus>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    running: Mutex<HashMap<String, Running>>,
    pub settings: Settings,
    pub stop: Arc<AtomicBool>,
    log_file: Mutex<Option<std::fs::File>>,
    pub quiet: bool,
}

type Res<T> = Result<T, ApiError>;

fn p_str<'a>(params: &'a Value, key: &str) -> Res<&'a str> {
    params.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty()).ok_or_else(|| ApiError::invalid(format!("missing {key}")))
}

fn p_opt(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn p_list(params: &Value, key: &str) -> Vec<String> {
    params.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() { out.push(c.to_ascii_lowercase()) } else if !out.ends_with('-') && !out.is_empty() { out.push('-') }
    }
    out.trim_end_matches('-').to_string()
}

impl Service {
    pub fn new(store: Store, settings: Settings) -> Arc<Service> {
        Arc::new(Service {
            store: Mutex::new(store),
            adapters: Mutex::new(HashMap::new()),
            harness_status: Mutex::new(vec![]),
            subscribers: Mutex::new(vec![]),
            running: Mutex::new(HashMap::new()),
            settings,
            stop: Arc::new(AtomicBool::new(false)),
            log_file: Mutex::new(None),
            quiet: false,
        })
    }

    pub fn log_to(&self, path: &Path) {
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            *self.log_file.lock().unwrap() = Some(f);
        }
    }

    pub fn log(&self, line: &str) {
        use std::io::Write;
        let stamp = now();
        if let Some(f) = self.log_file.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{stamp} {line}");
        }
        if !self.quiet { eprintln!("{stamp} {line}"); }
    }

    pub fn register(&self, adapter: Arc<dyn HarnessAdapter>) {
        self.adapters.lock().unwrap().insert(adapter.kind(), adapter);
    }

    fn adapter(&self, kind: HarnessKind) -> Res<Arc<dyn HarnessAdapter>> {
        self.adapters.lock().unwrap().get(&kind).cloned().ok_or_else(|| ApiError::conflict(format!("no adapter for harness {kind}")))
    }

    /// Ask every adapter what it can do; remembered for the snapshot and dispatch.
    pub fn probe_harnesses(&self) -> Vec<HarnessStatus> {
        let adapters: Vec<Arc<dyn HarnessAdapter>> = self.adapters.lock().unwrap().values().cloned().collect();
        let mut out = vec![];
        for a in adapters {
            let status = match a.probe() {
                Ok(c) => HarnessStatus { kind: a.kind(), available: true, runtime_version: c.runtime_version.clone(), detail: format!("{:?} transport", c.transport), models: vec![], capabilities: Some(c) },
                Err(e) => HarnessStatus { kind: a.kind(), available: false, runtime_version: String::new(), detail: e.to_string(), models: vec![], capabilities: None },
            };
            out.push(status);
        }
        out.sort_by_key(|s| s.kind.as_str().to_string());
        *self.harness_status.lock().unwrap() = out.clone();
        out
    }

    fn available_harnesses(&self) -> Vec<HarnessKind> {
        self.harness_status.lock().unwrap().iter().filter(|s| s.available).map(|s| s.kind).collect()
    }

    // ----- events -----

    fn emit(&self, event: Event) -> Event {
        let store = self.store.lock().unwrap();
        self.emit_locked(&store, event)
    }

    fn emit_locked(&self, store: &Store, event: Event) -> Event {
        match store.append(event) {
            Ok(ev) => {
                self.subscribers.lock().unwrap().retain(|s| s.send(ev.clone()).is_ok());
                ev
            }
            Err(e) => {
                self.log(&format!("cannot append event: {e}"));
                Event::new("error", "guild")
            }
        }
    }

    // ----- state helpers -----

    fn update_attempt(&self, id: &str, f: impl FnOnce(&mut Attempt)) -> Res<Attempt> {
        let store = self.store.lock().unwrap();
        let mut a = store.attempt(id)?.ok_or_else(|| ApiError::not_found(format!("attempt {id}")))?;
        f(&mut a);
        a.updated_at = now();
        store.put_attempt(&a)?;
        Ok(a)
    }

    fn move_attempt(&self, id: &str, to: AttemptState, failure: Option<String>) -> Res<Attempt> {
        let store = self.store.lock().unwrap();
        let mut a = store.attempt(id)?.ok_or_else(|| ApiError::not_found(format!("attempt {id}")))?;
        if a.state == to { return Ok(a); }
        attempt_transition(a.state, to).map_err(ApiError::conflict)?;
        a.state = to;
        if failure.is_some() { a.failure = failure; }
        if to.is_terminal() { a.finished_at = Some(now()); }
        a.updated_at = now();
        store.put_attempt(&a)?;
        let kind = match to {
            AttemptState::Provisioning => "attempt.provisioning",
            AttemptState::Running => "attempt.running",
            AttemptState::WaitingInput => "attempt.waiting_input",
            AttemptState::Submitted => "attempt.submitted",
            AttemptState::Verifying => "attempt.verifying",
            AttemptState::Passed => "attempt.passed",
            AttemptState::Failed => "attempt.failed",
            AttemptState::Cancelled => "attempt.cancelled",
            AttemptState::Lost => "attempt.lost",
            AttemptState::TimedOut => "attempt.timed_out",
            AttemptState::Queued => "attempt.queued",
        };
        self.emit_locked(&store, Event::new(kind, "guild").about_attempt(&a).with(json!({"state": to, "failure": a.failure})));
        if to.is_terminal() {
            // One row per finished attempt, so routing can learn from history later.
            let task = store.task(&a.task_id)?;
            let member = store.member(&a.member_id)?;
            let duration_ms = a.started_at.as_deref().zip(a.finished_at.as_deref()).and_then(|(s, f)| {
                let s = chrono::DateTime::parse_from_rfc3339(s).ok()?;
                let f = chrono::DateTime::parse_from_rfc3339(f).ok()?;
                Some((f - s).num_milliseconds())
            });
            self.emit_locked(&store, Event::new("attempt.finished", "guild").about_attempt(&a).with(json!({
                "result": to,
                "role": task.as_ref().map(|t| t.role.clone()),
                "member": member.as_ref().map(|m| m.name.clone()),
                "harness": a.harness,
                "model": a.model_effective.clone().or(a.model_requested.clone()),
                "durationMs": duration_ms,
                "turns": a.turns,
                "resumed": a.resumed,
                "attemptNumber": task.as_ref().map(|t| t.attempts_made),
                "filesChanged": a.candidate.as_ref().map(|c| c.files_changed),
                "verifiersPassed": a.verifier_results.iter().filter(|v| v.status == "passed").count(),
                "verifiersFailed": a.verifier_results.iter().filter(|v| v.status != "passed").count(),
                "failure": a.failure,
            })));
        }
        Ok(a)
    }

    fn move_task(&self, id: &str, to: TaskState, reason: Option<String>) -> Res<Task> {
        let store = self.store.lock().unwrap();
        self.move_task_locked(&store, id, to, reason)
    }

    fn move_task_locked(&self, store: &Store, id: &str, to: TaskState, reason: Option<String>) -> Res<Task> {
        let mut t = store.task(id)?.ok_or_else(|| ApiError::not_found(format!("task {id}")))?;
        if t.state == to && t.block_reason == reason { return Ok(t); }
        task_transition(t.state, to).map_err(ApiError::conflict)?;
        t.state = to;
        t.block_reason = if matches!(to, TaskState::Blocked | TaskState::Failed) { reason.clone() } else { None };
        t.updated_at = now();
        store.put_task(&t)?;
        let project_id = store.quest(&t.quest_id)?.map(|q| q.project_id).unwrap_or_default();
        let kind = match to {
            TaskState::Pending => "task.pending",
            TaskState::Ready => "task.ready",
            TaskState::Running => "task.running",
            TaskState::Verifying => "task.verifying",
            TaskState::Accepted => "task.accepted",
            TaskState::Integrating => "task.integrating",
            TaskState::Completed => "task.completed",
            TaskState::Blocked => "task.blocked",
            TaskState::Failed => "task.failed",
            TaskState::Cancelled => "task.cancelled",
            TaskState::Draft => "task.draft",
        };
        self.emit_locked(store, Event::new(kind, "guild").about_task(&t, &project_id).with(json!({"state": to, "reason": reason, "title": t.title})));
        Ok(t)
    }

    fn write_artifact(&self, attempt_id: Option<&str>, kind: &str, bytes: &[u8]) -> Res<Artifact> {
        let dir = crate::paths::artifacts_dir().join(attempt_id.unwrap_or("guild"));
        std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
        let id = new_id();
        let path = dir.join(format!("{kind}-{id}"));
        std::fs::write(&path, bytes).map_err(|e| ApiError::internal(e.to_string()))?;
        let sha = format!("{:x}", sha2::Sha256::digest(bytes));
        let artifact = Artifact { id, attempt_id: attempt_id.map(str::to_string), kind: kind.into(), path: path.display().to_string(), sha256: sha, size_bytes: bytes.len() as u64, created_at: now() };
        self.store.lock().unwrap().put_artifact(&artifact)?;
        Ok(artifact)
    }

    // ----- the tick -----

    /// Readiness, dispatch, quest completion and timeouts. Runs once a
    /// second and after every command.
    pub fn tick(self: &Arc<Service>) {
        if let Err(e) = self.tick_inner() { self.log(&format!("tick: {e}")); }
    }

    fn tick_inner(self: &Arc<Service>) -> Res<()> {
        // Readiness and quest completion.
        let (quests, tasks) = {
            let store = self.store.lock().unwrap();
            (store.quests()?, store.tasks()?)
        };
        for (id, state, reason) in scheduler::readiness(&tasks) {
            self.move_task(&id, state, reason)?;
        }
        for q in quests.iter().filter(|q| q.state == QuestState::Running) {
            let of_quest: Vec<&Task> = tasks.iter().filter(|t| t.quest_id == q.id).collect();
            if !of_quest.is_empty() && of_quest.iter().all(|t| t.state == TaskState::Completed) {
                let store = self.store.lock().unwrap();
                let mut q = q.clone();
                q.state = QuestState::Completed;
                q.updated_at = now();
                store.put_quest(&q)?;
                self.emit_locked(&store, Event::new("quest.completed", "guild").about_quest(&q).with(json!({"title": q.title})));
            }
        }
        // Dispatch.
        let running_quests: Vec<String> = quests.iter().filter(|q| q.state == QuestState::Running).map(|q| q.id.clone()).collect();
        let tasks = self.store.lock().unwrap().tasks()?;
        let ready = scheduler::ready_in_order(&tasks, &running_quests);
        if ready.is_empty() { return Ok(()); }
        let available = self.available_harnesses();
        for task in ready {
            let active = self.running.lock().unwrap().len();
            if active >= self.settings.concurrency { break; }
            let attempt = {
                let mut store = self.store.lock().unwrap();
                let members = store.members()?;
                let busy: Vec<String> = store.active_attempts()?.into_iter().map(|a| a.member_id).collect();
                let Some(member) = scheduler::pick_member(task, &members, &busy, &available).cloned() else {
                    let why = if members.iter().any(|m| m.roles.contains(&task.role)) { "no free member" } else { "no member has the role" };
                    if task.block_reason.as_deref() != Some(why) && members.iter().all(|m| !m.roles.contains(&task.role)) {
                        self.move_task_locked(&store, &task.id, TaskState::Blocked, Some(why.into()))?;
                    }
                    continue;
                };
                let quest = store.quest(&task.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
                let t = store.task(&task.id)?.ok_or_else(|| ApiError::not_found("task"))?;
                if t.state != TaskState::Ready { continue; }
                let model = t.model_id.clone().or(member.model_id.clone());
                let attempt = Attempt {
                    id: new_id(),
                    task_id: t.id.clone(),
                    task_version: t.version,
                    quest_id: quest.id.clone(),
                    project_id: quest.project_id.clone(),
                    member_id: member.id.clone(),
                    harness: member.harness,
                    model_requested: model,
                    model_effective: None,
                    state: AttemptState::Queued,
                    workspace_id: None,
                    session_id: None,
                    generation: 1,
                    candidate: None,
                    summary: String::new(),
                    action: None,
                    pending_permission: None,
                    failure: None,
                    verifier_results: vec![],
                    turns: 0,
                    resumed: 0,
                    started_at: Some(now()),
                    finished_at: None,
                    created_at: now(),
                    updated_at: now(),
                };
                let created = store.transaction(|s| {
                    let mut t = t.clone();
                    t.state = TaskState::Running;
                    t.attempts_made += 1;
                    t.updated_at = now();
                    s.put_task(&t)?;
                    s.put_attempt(&attempt)?;
                    Ok(())
                });
                created?;
                self.emit_locked(&store, Event::new("task.running", "guild").about_task(&t, &quest.project_id).with(json!({"state": "running", "title": t.title})));
                self.emit_locked(&store, Event::new("attempt.started", "guild").about_attempt(&attempt).with(json!({"member": member.name, "harness": member.harness, "model": attempt.model_requested, "task": t.title})));
                attempt
            };
            self.spawn_driver(attempt.id.clone());
        }
        Ok(())
    }

    fn spawn_driver(self: &Arc<Service>, attempt_id: String) {
        // Reserve the slot before the thread starts, so the next tick counts it.
        self.running.lock().unwrap().insert(
            attempt_id.clone(),
            Running { session: SessionRef { id: String::new(), native_id: None }, harness: HarnessKind::Mock, cancel_requested: false, interrupt_requested: false, started: Instant::now() },
        );
        let service = self.clone();
        std::thread::Builder::new()
            .name(format!("attempt-{}", &attempt_id[attempt_id.len().saturating_sub(6)..]))
            .spawn(move || {
                let outcome = service.drive(&attempt_id);
                service.running.lock().unwrap().remove(&attempt_id);
                if let Err(e) = outcome {
                    service.log(&format!("attempt {attempt_id}: {e}"));
                    let _ = service.finish_attempt(&attempt_id, TurnOutcome::Failed { reason: e.to_string() });
                }
                service.tick();
            })
            .ok();
    }

    /// Like `spawn_driver`, for an attempt reopened after a restart. If
    /// the session cannot be reopened the attempt is lost, as it was.
    fn spawn_resumer(self: &Arc<Service>, attempt_id: String, native_id: String) {
        self.running.lock().unwrap().insert(
            attempt_id.clone(),
            Running { session: SessionRef { id: String::new(), native_id: None }, harness: HarnessKind::Mock, cancel_requested: false, interrupt_requested: false, started: Instant::now() },
        );
        let service = self.clone();
        std::thread::Builder::new()
            .name(format!("resume-{}", &attempt_id[attempt_id.len().saturating_sub(6)..]))
            .spawn(move || {
                let outcome = service.resume(&attempt_id, &native_id);
                service.running.lock().unwrap().remove(&attempt_id);
                if let Err(e) = outcome {
                    service.log(&format!("attempt {attempt_id}: resume failed: {e}"));
                    let task = service.store.lock().unwrap().attempt(&attempt_id).ok().flatten().and_then(|a| service.store.lock().unwrap().task(&a.task_id).ok().flatten());
                    let _ = service.move_attempt(&attempt_id, AttemptState::Lost, Some(format!("the service restarted and the session could not be reopened: {e}")));
                    if let Some(t) = task { let _ = service.after_failed_attempt(&t, "lost"); }
                }
                service.tick();
            })
            .ok();
    }

    /// Provision, start a session, send the task, follow the turn, and
    /// leave the attempt submitted (or failed). Runs on its own thread.
    fn drive(self: &Arc<Service>, attempt_id: &str) -> Res<()> {
        let (attempt, task, quest, project, member) = {
            let store = self.store.lock().unwrap();
            let a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
            let t = store.task(&a.task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            let q = store.quest(&a.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
            let p = store.project(&q.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            let m = store.member(&a.member_id)?.ok_or_else(|| ApiError::not_found("member"))?;
            (a, t, q, p, m)
        };
        self.move_attempt(attempt_id, AttemptState::Provisioning, None)?;
        let base = workspaces::ensure_integration_branch(&project, &quest).map_err(ApiError::internal)?;
        let workspace = workspaces::provision(&project, &quest.id, &task.id, attempt_id, &base).map_err(ApiError::internal)?;
        {
            let store = self.store.lock().unwrap();
            store.put_workspace(&workspace)?;
            self.emit_locked(&store, Event::new("workspace.ready", "guild").about_attempt(&attempt).with(json!({"path": workspace.path, "branch": workspace.branch, "baseSha": base})));
        }
        self.update_attempt(attempt_id, |a| a.workspace_id = Some(workspace.id.clone()))?;
        if !project.setup.is_empty() {
            self.emit(Event::new("setup.started", "guild").about_attempt(&attempt).with(json!({"argv": project.setup})));
            match workspaces::setup(&workspace, &project.setup, self.settings.setup_timeout_seconds) {
                Ok(output) => {
                    let artifact = self.write_artifact(Some(attempt_id), "setup", output.as_bytes())?;
                    self.emit(Event::new("setup.finished", "guild").about_attempt(&attempt).with(json!({"status": "passed", "logArtifactId": artifact.id})));
                }
                Err(e) => return Err(ApiError::internal(format!("setup failed: {e}"))),
            }
        }
        if self.cancel_requested(attempt_id) { return self.finish_attempt(attempt_id, TurnOutcome::Cancelled); }
        let adapter = self.adapter(member.harness)?;
        let role_prompt = if member.role_prompt.trim().is_empty() { config::default_role_prompt(&task.role) } else { member.role_prompt.clone() };
        let options = SessionOptions { cwd: PathBuf::from(&workspace.path), model_id: attempt.model_requested.clone(), mode: member.mode.clone(), role_prompt: role_prompt.clone() };
        let session = adapter.create_session(&options).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let prompt = compose_prompt(&role_prompt, &task, &workspace);
        self.run_turn(attempt_id, &attempt, &member, &adapter, session, &prompt)
    }

    /// Go on with an attempt whose harness session survived a restart:
    /// reopen the session in the same worktree and ask it to continue.
    fn resume(self: &Arc<Service>, attempt_id: &str, native_id: &str) -> Res<()> {
        let (attempt, task, member, workspace) = {
            let store = self.store.lock().unwrap();
            let a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
            let t = store.task(&a.task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            let m = store.member(&a.member_id)?.ok_or_else(|| ApiError::not_found("member"))?;
            let w = a.workspace_id.as_ref().and_then(|id| store.workspace(id).ok().flatten()).ok_or_else(|| ApiError::not_found("workspace"))?;
            (a, t, m, w)
        };
        if !Path::new(&workspace.path).exists() { return Err(ApiError::conflict("the worktree is gone")); }
        let adapter = self.adapter(member.harness)?;
        let role_prompt = if member.role_prompt.trim().is_empty() { config::default_role_prompt(&task.role) } else { member.role_prompt.clone() };
        let options = SessionOptions { cwd: PathBuf::from(&workspace.path), model_id: attempt.model_effective.clone().or(attempt.model_requested.clone()), mode: member.mode.clone(), role_prompt };
        let session = adapter.resume_session(&options, native_id).map_err(|e| ApiError::new("harness", e.to_string()))?;
        self.update_attempt(attempt_id, |a| a.resumed += 1)?;
        self.emit(Event::new("session.resumed", "harness").about_attempt(&attempt).with(json!({"nativeId": native_id, "harness": member.harness})));
        let prompt = "The guild restarted while you were working. Your worktree is as you left it. Continue the task from where you stopped; if it was already complete, say so and finish with the summary.".to_string();
        self.run_turn(attempt_id, &attempt, &member, &adapter, session, &prompt)
    }

    /// Record the session, send one prompt, follow it to the end, close the
    /// session and finish the attempt. Shared by first runs and resumes.
    fn run_turn(self: &Arc<Service>, attempt_id: &str, attempt: &Attempt, member: &Member, adapter: &Arc<dyn HarnessAdapter>, session: SessionRef, prompt: &str) -> Res<()> {
        let runtime_version = self.harness_status.lock().unwrap().iter().find(|s| s.kind == member.harness).map(|s| s.runtime_version.clone()).unwrap_or_default();
        let session_row = Session { id: session.id.clone(), attempt_id: attempt_id.into(), harness: member.harness, native_id: session.native_id.clone(), runtime_version, state: "running".into(), created_at: now(), updated_at: now() };
        {
            let store = self.store.lock().unwrap();
            store.put_session(&session_row)?;
            self.emit_locked(&store, Event::new("session.started", "harness").about_attempt(attempt).with(json!({"sessionId": session.id, "nativeId": session.native_id, "harness": member.harness})));
        }
        let effective = attempt.model_effective.clone().or(attempt.model_requested.clone());
        self.update_attempt(attempt_id, |a| {
            a.session_id = Some(session.id.clone());
            a.model_effective = effective;
            a.turns += 1;
            a.pending_permission = None;
        })?;
        {
            let mut running = self.running.lock().unwrap();
            let slot = running.entry(attempt_id.to_string()).or_insert(Running { session: session.clone(), harness: member.harness, cancel_requested: false, interrupt_requested: false, started: Instant::now() });
            slot.session = session.clone();
            slot.harness = member.harness;
        }
        self.move_attempt(attempt_id, AttemptState::Running, None)?;
        let handle = adapter.prompt(&session, attempt_id, prompt).map_err(|e| ApiError::new("harness", e.to_string()))?;
        self.emit(Event::new("turn.started", "guild").about_attempt(attempt).with(json!({"turnId": handle.turn_id, "promptChars": prompt.len()})));
        let outcome = self.follow_turn(attempt_id, attempt, adapter, &session, handle);
        let stopped = adapter.stop(&session).unwrap_or(false);
        self.emit(Event::new("session.closed", "harness").about_attempt(attempt).with(json!({"confirmed": stopped})));
        self.finish_attempt(attempt_id, outcome)
    }

    fn cancel_requested(&self, attempt_id: &str) -> bool {
        self.running.lock().unwrap().get(attempt_id).map(|r| r.cancel_requested).unwrap_or(false)
    }

    /// Follow the events of one turn until it ends, is cancelled or
    /// times out. Text is batched into `turn.text` events; tools update
    /// the room action; permissions park the attempt in waiting_input.
    fn follow_turn(&self, attempt_id: &str, attempt: &Attempt, adapter: &Arc<dyn HarnessAdapter>, session: &SessionRef, handle: crate::harness::TurnHandle) -> TurnOutcome {
        let mut text = String::new();
        let mut last_flush = Instant::now();
        let mut interrupted_at: Option<Instant> = None;
        let mut timed_out = false;
        let mut tools_open: HashMap<String, String> = HashMap::new();
        let mut phase: Option<&'static str> = None;
        let mut events_closed = false;
        // An interrupted turn ends as a completed one: the worktree is the candidate.
        let settle = |outcome: TurnOutcome, s: &Service| -> TurnOutcome {
            let interrupted = s.running.lock().unwrap().get(attempt_id).map(|r| r.interrupt_requested && !r.cancel_requested).unwrap_or(false);
            match outcome {
                TurnOutcome::Cancelled if interrupted => TurnOutcome::Completed { stop_reason: "interrupted".into() },
                other => other,
            }
        };
        let flush = |text: &mut String, s: &Service| {
            if text.trim().is_empty() { text.clear(); return; }
            let chunk = std::mem::take(text);
            let summary: String = chunk.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(240).collect();
            let _ = s.update_attempt(attempt_id, |a| if !summary.is_empty() { a.summary = summary.clone() });
            s.emit(Event::new("turn.text", "harness").about_attempt(attempt).with(json!({"text": chunk})));
        };
        loop {
            if let Ok(outcome) = handle.result.try_recv() {
                // Drain whatever the adapter still has queued.
                while let Ok(ev) = handle.events.try_recv() { self.turn_event(attempt_id, attempt, ev, &mut text, &mut tools_open, &mut phase); }
                flush(&mut text, self);
                return if timed_out { TurnOutcome::Failed { reason: "timed out".into() } } else { settle(outcome, self) };
            }
            if !events_closed {
                match handle.events.recv_timeout(Duration::from_millis(300)) {
                    Ok(ev) => {
                        let is_tool = matches!(ev, HarnessEvent::Tool { .. } | HarnessEvent::PermissionRequested { .. });
                        self.turn_event(attempt_id, attempt, ev, &mut text, &mut tools_open, &mut phase);
                        if is_tool { flush(&mut text, self); last_flush = Instant::now(); }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => events_closed = true,
                }
            } else {
                match handle.result.recv_timeout(Duration::from_millis(300)) {
                    Ok(outcome) => {
                        flush(&mut text, self);
                        return if timed_out { TurnOutcome::Failed { reason: "timed out".into() } } else { settle(outcome, self) };
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        flush(&mut text, self);
                        return TurnOutcome::Failed { reason: "TRANSPORT_LOST: the harness ended without a result".into() };
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            if last_flush.elapsed() > Duration::from_secs(2) { flush(&mut text, self); last_flush = Instant::now(); }
            let (cancel, interrupt, started) = {
                let running = self.running.lock().unwrap();
                running.get(attempt_id).map(|r| (r.cancel_requested, r.interrupt_requested, r.started)).unwrap_or((true, false, Instant::now()))
            };
            if !timed_out && started.elapsed() > self.settings.attempt_timeout {
                timed_out = true;
                self.emit(Event::new("attempt.timeout", "guild").about_attempt(attempt).with(json!({"afterSeconds": self.settings.attempt_timeout.as_secs()})));
            }
            if (cancel || interrupt || timed_out) && interrupted_at.is_none() {
                let _ = adapter.interrupt(session);
                interrupted_at = Some(Instant::now());
            }
            if let Some(at) = interrupted_at {
                if at.elapsed() > self.settings.cancel_timeout {
                    flush(&mut text, self);
                    let _ = adapter.stop(session);
                    return if timed_out { TurnOutcome::Failed { reason: "timed out".into() } } else { settle(TurnOutcome::Cancelled, self) };
                }
            }
        }
    }

    /// Record a change of what the agent is doing, once per change:
    /// `agent.thinking`, `agent.working` or `agent.waiting`.
    fn set_phase(&self, attempt: &Attempt, phase: &mut Option<&'static str>, to: &'static str) {
        if *phase == Some(to) { return; }
        *phase = Some(to);
        self.emit(Event::new(match to { "thinking" => "agent.thinking", "waiting" => "agent.waiting", _ => "agent.working" }, "harness").about_attempt(attempt));
    }

    fn turn_event(&self, attempt_id: &str, attempt: &Attempt, ev: HarnessEvent, text: &mut String, tools_open: &mut HashMap<String, String>, phase: &mut Option<&'static str>) {
        match ev {
            HarnessEvent::Text(chunk) => { self.set_phase(attempt, phase, "working"); text.push_str(&chunk) }
            HarnessEvent::Thought(_) => self.set_phase(attempt, phase, "thinking"),
            HarnessEvent::Tool { id, title, kind, status, action, paths } => {
                self.set_phase(attempt, phase, "working");
                let first = !tools_open.contains_key(&id);
                let finished = status == "completed" || status == "failed";
                if first {
                    tools_open.insert(id.clone(), title.clone());
                    let _ = self.update_attempt(attempt_id, |a| a.action = Some(action.clone()));
                    self.emit(Event::new("tool.started", "harness").about_attempt(attempt).with(json!({"toolId": id, "title": title, "kind": kind, "action": action, "paths": paths})));
                }
                if finished {
                    tools_open.remove(&id);
                    self.emit(Event::new("tool.completed", "harness").about_attempt(attempt).with(json!({"toolId": id, "title": title, "status": status, "paths": paths})));
                    if status == "completed" && matches!(kind.as_str(), "edit" | "delete" | "move") && !paths.is_empty() {
                        self.emit(Event::new("file.changed", "harness").about_attempt(attempt).with(json!({"paths": paths, "toolId": id, "kind": kind})));
                    }
                }
            }
            HarnessEvent::Plan(entries) => {
                let _ = self.update_attempt(attempt_id, |a| a.action = Some("plan".into()));
                self.emit(Event::new("plan.updated", "harness").about_attempt(attempt).with(json!({"entries": entries})));
            }
            HarnessEvent::ModelChanged { effective, reason } => {
                let _ = self.update_attempt(attempt_id, |a| a.model_effective = Some(effective.clone()));
                self.emit(Event::new("model.changed", "harness").about_attempt(attempt).with(json!({"effective": effective, "reason": reason})));
            }
            HarnessEvent::PermissionRequested { request_id, title, detail, options } => {
                self.set_phase(attempt, phase, "waiting");
                let request = PermissionRequest { request_id: request_id.clone(), title: title.clone(), detail: detail.clone(), options: options.clone(), asked_at: now() };
                let _ = self.update_attempt(attempt_id, |a| a.pending_permission = Some(request.clone()));
                let _ = self.move_attempt(attempt_id, AttemptState::WaitingInput, None);
                self.emit(Event::new("permission.requested", "harness").about_attempt(attempt).with(json!({"requestId": request_id, "title": title, "detail": detail, "options": options})));
            }
            HarnessEvent::Raw(_) => {}
        }
    }

    /// Record how the turn ended: commit the candidate and run the
    /// verifiers, or fail the attempt and decide the task's next state.
    fn finish_attempt(&self, attempt_id: &str, outcome: TurnOutcome) -> Res<()> {
        let (attempt, task, project, workspace) = {
            let store = self.store.lock().unwrap();
            let a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
            let t = store.task(&a.task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            let p = store.project(&a.project_id)?;
            let w = match &a.workspace_id { Some(id) => store.workspace(id)?, None => None };
            (a, t, p, w)
        };
        if attempt.state.is_terminal() { return Ok(()); }
        // While the service shuts down the attempt stays as it is: the next
        // start resumes the session or adopts the worktree as its candidate.
        if self.stop.load(Ordering::Relaxed) {
            self.log(&format!("attempt {attempt_id}: left {:?} for the next start", attempt.state));
            return Ok(());
        }
        match outcome {
            TurnOutcome::Completed { stop_reason } => {
                self.emit(Event::new("turn.completed", "harness").about_attempt(&attempt).with(json!({"stopReason": stop_reason})));
                let candidate = match &workspace {
                    Some(ws) => workspaces::commit_candidate(ws, &format!("guild candidate: {}", task.title)).map_err(ApiError::internal)?,
                    None => None,
                };
                let candidate = match candidate {
                    Some(mut c) => {
                        if let Some(ws) = &workspace {
                            if let Ok(diff) = workspaces::candidate_diff(ws, &c) {
                                let artifact = self.write_artifact(Some(attempt_id), "candidate.diff", diff.as_bytes())?;
                                c.diff_artifact_id = Some(artifact.id);
                            }
                        }
                        Some(c)
                    }
                    None => None,
                };
                if let Some(c) = &candidate {
                    self.emit(Event::new("candidate.committed", "guild").about_attempt(&attempt).with(json!({"baseSha": c.base_sha, "headSha": c.head_sha, "filesChanged": c.files_changed, "insertions": c.insertions, "deletions": c.deletions})));
                } else {
                    self.emit(Event::new("candidate.empty", "guild").about_attempt(&attempt).with(json!({"note": "the attempt left no changes"})));
                }
                self.update_attempt(attempt_id, |a| { a.candidate = candidate.clone(); a.action = None; a.pending_permission = None; })?;
                self.move_attempt(attempt_id, AttemptState::Submitted, None)?;
                self.move_task(&task.id, TaskState::Verifying, None)?;
                if let (Some(ws), Some(p), Some(_)) = (&workspace, &project, &candidate) {
                    if !p.verifiers.is_empty() {
                        self.move_attempt(attempt_id, AttemptState::Verifying, None)?;
                        let results = self.run_verifiers(attempt_id, &attempt, p, ws);
                        let all_required_pass = results.iter().filter(|r| r.required).all(|r| r.status == "passed");
                        self.update_attempt(attempt_id, |a| a.verifier_results = results.clone())?;
                        // Back to submitted: acceptance is the user's in V0, with the results in view.
                        let store = self.store.lock().unwrap();
                        let mut a = store.attempt(attempt_id)?.unwrap();
                        a.state = AttemptState::Submitted;
                        a.updated_at = now();
                        store.put_attempt(&a)?;
                        self.emit_locked(&store, Event::new("verification.finished", "verifier").about_attempt(&attempt).with(json!({"requiredPassed": all_required_pass, "results": results})));
                    }
                }
                Ok(())
            }
            TurnOutcome::Failed { reason } => {
                self.move_attempt(attempt_id, if reason == "timed out" { AttemptState::TimedOut } else { AttemptState::Failed }, Some(reason.clone()))?;
                self.after_failed_attempt(&task, &reason)
            }
            TurnOutcome::Cancelled => {
                self.move_attempt(attempt_id, AttemptState::Cancelled, Some("cancelled".into()))?;
                let _ = self.move_task(&task.id, TaskState::Blocked, Some("attempt cancelled; retry to continue".into()));
                Ok(())
            }
        }
    }

    fn after_failed_attempt(&self, task: &Task, reason: &str) -> Res<()> {
        let fresh = self.store.lock().unwrap().task(&task.id)?.unwrap_or_else(|| task.clone());
        if fresh.attempts_made < fresh.max_attempts {
            self.move_task(&task.id, TaskState::Ready, None)?;
        } else {
            self.move_task(&task.id, TaskState::Failed, Some(format!("{} attempts failed; last: {reason}", fresh.attempts_made)))?;
        }
        Ok(())
    }

    fn run_verifiers(&self, attempt_id: &str, attempt: &Attempt, project: &Project, workspace: &Workspace) -> Vec<VerifierResult> {
        let mut results = vec![];
        for v in &project.verifiers {
            self.emit(Event::new("verifier.started", "verifier").about_attempt(attempt).with(json!({"verifierId": v.id, "argv": v.argv})));
            let started = Instant::now();
            let (status, code, output) = match workspaces::run_with_timeout(Path::new(&workspace.path), &v.argv, v.timeout_seconds) {
                Ok((0, out)) => ("passed", Some(0), out),
                Ok((code, out)) => ("failed", Some(code), out),
                Err(e) => ("error", None, e),
            };
            let artifact = self.write_artifact(Some(attempt_id), &format!("verifier-{}", v.id), output.as_bytes()).ok();
            let tail: String = output.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            let result = VerifierResult { id: v.id.clone(), required: v.required, status: status.into(), exit_code: code, duration_ms: started.elapsed().as_millis() as u64, log_artifact_id: artifact.map(|a| a.id), tail };
            self.emit(Event::new("verifier.finished", "verifier").about_attempt(attempt).with(json!({"verifierId": v.id, "status": status, "exitCode": code, "durationMs": result.duration_ms, "logArtifactId": result.log_artifact_id})));
            results.push(result);
        }
        results
    }

    /// After a restart: attempts the store thinks are active have no
    /// driver. Adopt a candidate their worktree holds, or mark them lost.
    pub fn reconcile(self: &Arc<Service>) {
        let active = self.store.lock().unwrap().active_attempts().unwrap_or_default();
        for a in active {
            if self.running.lock().unwrap().contains_key(&a.id) { continue; }
            self.log(&format!("reconciling attempt {} ({:?})", a.id, a.state));
            let workspace = a.workspace_id.as_ref().and_then(|id| self.store.lock().unwrap().workspace(id).ok().flatten());
            // A harness that persists sessions gets to continue where it was.
            if matches!(a.state, AttemptState::Running | AttemptState::WaitingInput) {
                let native = a.session_id.as_ref().and_then(|id| self.store.lock().unwrap().session(id).ok().flatten()).and_then(|s| s.native_id);
                let can_resume = self.harness_status.lock().unwrap().iter().find(|s| s.kind == a.harness)
                    .and_then(|s| s.capabilities.as_ref()).map(|c| c.resume_session == Support::Supported).unwrap_or(false);
                if let (Some(native), true, true) = (native, can_resume, workspace.as_ref().map(|w| Path::new(&w.path).exists()).unwrap_or(false)) {
                    self.log(&format!("resuming attempt {} on session {native}", a.id));
                    self.spawn_resumer(a.id.clone(), native);
                    continue;
                }
            }
            let recovered = workspace.as_ref().filter(|w| Path::new(&w.path).exists()).and_then(|w| workspaces::commit_candidate(w, "guild candidate (recovered after restart)").ok().flatten());
            let task = self.store.lock().unwrap().task(&a.task_id).ok().flatten();
            match (recovered, a.state) {
                (_, AttemptState::Submitted) | (_, AttemptState::Verifying) => {
                    // Already a candidate; it only needs the user's decision.
                    let _ = self.move_attempt(&a.id, AttemptState::Submitted, None);
                }
                (Some(candidate), _) => {
                    let _ = self.update_attempt(&a.id, |x| { x.candidate = Some(candidate.clone()); x.failure = Some("recovered after a restart".into()); });
                    let _ = self.move_attempt(&a.id, AttemptState::Submitted, None);
                    if let Some(t) = &task { let _ = self.move_task(&t.id, TaskState::Verifying, None); }
                }
                (None, _) => {
                    let _ = self.move_attempt(&a.id, AttemptState::Lost, Some("the service restarted while it ran".into()));
                    if let Some(t) = &task { let _ = self.after_failed_attempt(t, "lost"); }
                }
            }
        }
    }

    /// Close every live harness session, leaving the attempts in their
    /// state for the next start to resume.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let live: Vec<(String, HarnessKind, SessionRef)> = self.running.lock().unwrap().iter().map(|(id, r)| (id.clone(), r.harness, r.session.clone())).collect();
        for (attempt_id, harness, session) in live {
            if session.id.is_empty() { continue; }
            let Ok(adapter) = self.adapter(harness) else { continue };
            let _ = adapter.stop(&session);
            if let Ok(Some(a)) = self.store.lock().unwrap().attempt(&attempt_id) {
                self.emit(Event::new("session.closed", "guild").about_attempt(&a).with(json!({"confirmed": true, "reason": "service stopping"})));
            }
        }
    }

    pub fn snapshot(&self) -> Res<Snapshot> {
        let harnesses = self.harness_status.lock().unwrap().clone();
        Ok(self.store.lock().unwrap().snapshot(SERVICE_VERSION, harnesses)?)
    }

    // ----- commands -----

    fn command(self: &Arc<Service>, method: &str, params: &Value) -> ApiResult {
        match method {
            "guild.ping" => Ok(json!({"ok": true, "version": SERVICE_VERSION, "pid": std::process::id(), "home": crate::paths::home()})),
            "guild.snapshot" => Ok(serde_json::to_value(self.snapshot()?).map_err(|e| ApiError::internal(e.to_string()))?),
            "guild.stop" => {
                self.stop.store(true, Ordering::Relaxed);
                Ok(json!({"stopping": true}))
            }
            "harness.probe" => Ok(serde_json::to_value(self.probe_harnesses()).unwrap()),
            "models.list" => {
                let kind = HarnessKind::parse(p_str(params, "harness")?).ok_or_else(|| ApiError::invalid("unknown harness"))?;
                let adapter = self.adapter(kind)?;
                let models = adapter.list_models(None).map_err(|e| ApiError::new("harness", e.to_string()))?;
                let mut statuses = self.harness_status.lock().unwrap();
                if let Some(s) = statuses.iter_mut().find(|s| s.kind == kind) { s.models = models.clone(); }
                Ok(serde_json::to_value(models).unwrap())
            }
            "events.after" => {
                let after = params.get("afterSeq").and_then(Value::as_u64).unwrap_or(0);
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
                Ok(serde_json::to_value(self.store.lock().unwrap().events_after(after, limit)?).unwrap())
            }
            "events.attempt" => {
                let id = p_str(params, "attemptId")?;
                let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(500) as usize;
                Ok(serde_json::to_value(self.store.lock().unwrap().events_of_attempt(id, limit)?).unwrap())
            }
            "attempt.get" => {
                let id = p_str(params, "attemptId")?;
                let store = self.store.lock().unwrap();
                let a = store.attempt(id)?.ok_or_else(|| ApiError::not_found(format!("attempt {id}")))?;
                let w = match &a.workspace_id { Some(id) => store.workspace(id)?, None => None };
                let artifacts = store.artifacts_of_attempt(id)?;
                Ok(json!({"attempt": a, "workspace": w, "artifacts": artifacts}))
            }
            "artifact.read" => {
                let id = p_str(params, "artifactId")?;
                let a = self.store.lock().unwrap().artifact(id)?.ok_or_else(|| ApiError::not_found(format!("artifact {id}")))?;
                let text = std::fs::read_to_string(&a.path).map_err(|e| ApiError::internal(e.to_string()))?;
                Ok(json!({"artifact": a, "text": text}))
            }
            "project.add" => self.project_add(params),
            "member.add" => self.member_add(params),
            "member.remove" => {
                let id = p_str(params, "memberId")?;
                self.store.lock().unwrap().delete_member(id)?;
                Ok(json!({"removed": id}))
            }
            "quest.create" => self.quest_create(params),
            "task.create" => self.task_create(params),
            "quest.start" => self.quest_start(params),
            "quest.pause" => self.quest_set(params, QuestState::Paused, &[QuestState::Running]),
            "quest.resume" => self.quest_set(params, QuestState::Running, &[QuestState::Paused, QuestState::Blocked]),
            "quest.cancel" => self.quest_cancel(params),
            "task.retry" => self.task_retry(params),
            "task.cancel" => self.task_cancel(params),
            "attempt.cancel" => self.attempt_cancel(p_str(params, "attemptId")?),
            "attempt.interrupt" => self.attempt_interrupt(p_str(params, "attemptId")?),
            "permission.respond" => self.permission_respond(params),
            "task.accept" => self.task_accept(params),
            "task.reject" => self.task_reject(params),
            "task.integrate" => self.task_integrate(p_str(params, "taskId")?),
            "workspace.remove" => {
                let id = p_str(params, "attemptId")?;
                let store = self.store.lock().unwrap();
                let a = store.attempt(id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
                if a.state.is_active() { return Err(ApiError::conflict("the attempt is still active")); }
                let w = a.workspace_id.as_ref().and_then(|w| store.workspace(w).ok().flatten()).ok_or_else(|| ApiError::not_found("workspace"))?;
                let p = store.project(&a.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
                workspaces::remove(&p, &w, true).map_err(ApiError::internal)?;
                let mut w = w;
                w.state = WorkspaceState::Removed;
                w.updated_at = now();
                store.put_workspace(&w)?;
                Ok(json!({"removed": w.path}))
            }
            _ => Err(ApiError::new("unknown_method", format!("unknown method {method}"))),
        }
    }

    fn project_add(&self, params: &Value) -> ApiResult {
        let repo = PathBuf::from(p_str(params, "repoPath")?);
        let repo = repo.canonicalize().map_err(|e| ApiError::invalid(format!("{}: {e}", repo.display())))?;
        if !workspaces::is_repo(&repo) { return Err(ApiError::invalid(format!("{} is not a git repository", repo.display()))); }
        let file = config::load(&repo).map_err(ApiError::invalid)?;
        let folder = repo.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
        let id = p_opt(params, "id").or_else(|| file.as_ref().and_then(|f| f.project.id.clone())).unwrap_or_else(|| slug(&folder));
        let name = p_opt(params, "name").or_else(|| file.as_ref().and_then(|f| f.project.name.clone())).unwrap_or(folder);
        let target_ref = p_opt(params, "targetRef").or_else(|| file.as_ref().and_then(|f| f.project.target_ref.clone())).unwrap_or_else(|| {
            let branch = workspaces::current_branch(&repo).unwrap_or_else(|| "main".into());
            if branch == "HEAD" { "HEAD".into() } else { format!("refs/heads/{branch}") }
        });
        let store = self.store.lock().unwrap();
        let existing = store.project(&id)?;
        let project = Project {
            id: id.clone(),
            name,
            repo_path: repo.display().to_string(),
            target_ref,
            setup: file.as_ref().map(|f| f.project.setup.clone()).unwrap_or_default(),
            verifiers: file.as_ref().map(|f| f.verifiers.iter().map(|v| v.to_verifier()).collect()).unwrap_or_default(),
            created_at: existing.map(|p| p.created_at).unwrap_or_else(now),
        };
        store.put_project(&project)?;
        let mut members = 0;
        if let Some(f) = &file {
            for m in &f.members {
                store.put_member(&m.to_member())?;
                members += 1;
            }
        }
        self.emit_locked(&store, Event::new("project.added", "user").with(json!({"projectId": project.id, "repoPath": project.repo_path, "members": members})));
        Ok(json!({"project": project, "membersFromConfig": members}))
    }

    fn member_add(&self, params: &Value) -> ApiResult {
        let harness = HarnessKind::parse(p_str(params, "harness")?).ok_or_else(|| ApiError::invalid("unknown harness"))?;
        let id = p_str(params, "id")?.to_string();
        let mut roles = p_list(params, "roles");
        if roles.is_empty() { roles.push("implementer".into()); }
        let member = Member {
            id: id.clone(),
            name: p_opt(params, "name").unwrap_or_else(|| id.clone()),
            roles,
            harness,
            model_id: p_opt(params, "model"),
            mode: p_opt(params, "mode"),
            role_prompt: p_opt(params, "prompt").unwrap_or_default(),
            enabled: true,
        };
        let store = self.store.lock().unwrap();
        store.put_member(&member)?;
        self.emit_locked(&store, Event::new("member.added", "user").with(json!({"memberId": member.id, "harness": member.harness, "model": member.model_id})));
        Ok(serde_json::to_value(member).unwrap())
    }

    fn quest_create(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let store = self.store.lock().unwrap();
        store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
        let id = new_id();
        let quest = Quest {
            id: id.clone(),
            project_id: project_id.into(),
            title: p_str(params, "title")?.into(),
            description: p_opt(params, "description").unwrap_or_default(),
            state: QuestState::Draft,
            integration_ref: format!("refs/heads/guild/quest/{}", &id[id.len() - 8..]),
            base_sha: None,
            plan_version: 1,
            created_at: now(),
            updated_at: now(),
        };
        store.put_quest(&quest)?;
        self.emit_locked(&store, Event::new("quest.created", "user").about_quest(&quest).with(json!({"title": quest.title})));
        Ok(serde_json::to_value(quest).unwrap())
    }

    fn task_create(&self, params: &Value) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        let store = self.store.lock().unwrap();
        let quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
        let acceptance: Vec<AcceptanceCriterion> = params
            .get("acceptance")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .enumerate()
                    .filter_map(|(i, v)| match v {
                        Value::String(s) => Some(AcceptanceCriterion { id: format!("AC-{}", i + 1), text: s.clone() }),
                        Value::Object(o) => Some(AcceptanceCriterion { id: o.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("AC-{}", i + 1)), text: o.get("text").and_then(Value::as_str).unwrap_or("").into() }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let state = if quest.state == QuestState::Draft { TaskState::Draft } else { TaskState::Pending };
        let task = Task {
            id: p_opt(params, "id").unwrap_or_else(new_id),
            quest_id: quest_id.into(),
            title: p_str(params, "title")?.into(),
            instructions: p_opt(params, "instructions").unwrap_or_default(),
            acceptance,
            depends_on: p_list(params, "dependsOn"),
            role: p_opt(params, "role").unwrap_or_else(|| "implementer".into()),
            member_id: p_opt(params, "memberId"),
            model_id: p_opt(params, "modelId"),
            priority: params.get("priority").and_then(Value::as_i64).unwrap_or(0) as i32,
            state,
            block_reason: None,
            attempts_made: 0,
            max_attempts: params.get("maxAttempts").and_then(Value::as_u64).unwrap_or(2) as u32,
            version: 1,
            created_at: now(),
            updated_at: now(),
        };
        let mut all = store.tasks_of_quest(quest_id)?;
        all.retain(|t| t.id != task.id);
        all.push(task.clone());
        validate_dag(&all).map_err(ApiError::invalid)?;
        store.put_task(&task)?;
        self.emit_locked(&store, Event::new("task.created", "user").about_task(&task, &quest.project_id).with(json!({"title": task.title, "dependsOn": task.depends_on})));
        Ok(serde_json::to_value(task).unwrap())
    }

    fn quest_start(self: &Arc<Service>, params: &Value) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        {
            let store = self.store.lock().unwrap();
            let mut quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
            if !matches!(quest.state, QuestState::Draft | QuestState::Ready | QuestState::Paused) {
                return Err(ApiError::conflict(format!("quest is {:?}", quest.state)));
            }
            let tasks = store.tasks_of_quest(quest_id)?;
            if tasks.is_empty() { return Err(ApiError::invalid("the quest has no tasks")); }
            validate_dag(&tasks).map_err(ApiError::invalid)?;
            let project = store.project(&quest.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            let base = workspaces::ensure_integration_branch(&project, &quest).map_err(ApiError::internal)?;
            quest.base_sha = Some(base.clone());
            quest.state = QuestState::Running;
            quest.updated_at = now();
            store.put_quest(&quest)?;
            for t in tasks.iter().filter(|t| t.state == TaskState::Draft) {
                self.move_task_locked(&store, &t.id, TaskState::Pending, None)?;
            }
            self.emit_locked(&store, Event::new("quest.started", "user").about_quest(&quest).with(json!({"title": quest.title, "baseSha": base, "tasks": tasks.len()})));
        }
        self.tick();
        Ok(serde_json::to_value(self.store.lock().unwrap().quest(quest_id)?).unwrap())
    }

    fn quest_set(&self, params: &Value, to: QuestState, from: &[QuestState]) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        let store = self.store.lock().unwrap();
        let mut quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
        if !from.contains(&quest.state) { return Err(ApiError::conflict(format!("quest is {:?}", quest.state))); }
        quest.state = to;
        quest.updated_at = now();
        store.put_quest(&quest)?;
        let kind = if to == QuestState::Paused { "quest.paused" } else { "quest.resumed" };
        self.emit_locked(&store, Event::new(kind, "user").about_quest(&quest).with(json!({"title": quest.title})));
        Ok(serde_json::to_value(quest).unwrap())
    }

    fn quest_cancel(&self, params: &Value) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        let (tasks, active) = {
            let store = self.store.lock().unwrap();
            let mut quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
            quest.state = QuestState::Cancelled;
            quest.updated_at = now();
            store.put_quest(&quest)?;
            self.emit_locked(&store, Event::new("quest.cancelled", "user").about_quest(&quest).with(json!({"title": quest.title})));
            (store.tasks_of_quest(quest_id)?, store.active_attempts()?)
        };
        for a in active.iter().filter(|a| a.quest_id == quest_id) {
            let _ = self.attempt_cancel(&a.id);
        }
        for t in tasks.iter().filter(|t| !t.state.is_terminal() && t.state != TaskState::Running) {
            let _ = self.move_task(&t.id, TaskState::Cancelled, None);
        }
        Ok(json!({"cancelled": quest_id}))
    }

    fn task_retry(self: &Arc<Service>, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        {
            let store = self.store.lock().unwrap();
            let mut task = store.task(task_id)?.ok_or_else(|| ApiError::not_found(format!("task {task_id}")))?;
            if !matches!(task.state, TaskState::Failed | TaskState::Blocked | TaskState::Cancelled | TaskState::Verifying | TaskState::Accepted) {
                return Err(ApiError::conflict(format!("task is {:?}", task.state)));
            }
            if task.attempts_made >= task.max_attempts { task.max_attempts = task.attempts_made + 1; }
            task.version += 1;
            task.updated_at = now();
            store.put_task(&task)?;
            self.move_task_locked(&store, task_id, TaskState::Ready, None)?;
        }
        self.tick();
        Ok(serde_json::to_value(self.store.lock().unwrap().task(task_id)?).unwrap())
    }

    fn task_cancel(&self, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        let active = self.store.lock().unwrap().active_attempts()?;
        for a in active.iter().filter(|a| a.task_id == task_id) {
            let _ = self.attempt_cancel(&a.id);
        }
        let task = self.store.lock().unwrap().task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
        if task.state != TaskState::Running {
            self.move_task(task_id, TaskState::Cancelled, None)?;
        }
        Ok(json!({"cancelled": task_id}))
    }

    /// End the turn now and keep what the worktree has as the candidate.
    fn attempt_interrupt(&self, attempt_id: &str) -> ApiResult {
        let attempt = self.store.lock().unwrap().attempt(attempt_id)?.ok_or_else(|| ApiError::not_found(format!("attempt {attempt_id}")))?;
        if !matches!(attempt.state, AttemptState::Running | AttemptState::WaitingInput) {
            return Err(ApiError::conflict(format!("attempt is {:?}, not running", attempt.state)));
        }
        let mut running = self.running.lock().unwrap();
        let Some(r) = running.get_mut(attempt_id) else { return Err(ApiError::conflict("the attempt has no live turn")) };
        r.interrupt_requested = true;
        drop(running);
        self.emit(Event::new("attempt.interrupt_requested", "user").about_attempt(&attempt));
        Ok(json!({"interrupting": attempt_id}))
    }

    fn attempt_cancel(&self, attempt_id: &str) -> ApiResult {
        let attempt = self.store.lock().unwrap().attempt(attempt_id)?.ok_or_else(|| ApiError::not_found(format!("attempt {attempt_id}")))?;
        if attempt.state.is_terminal() { return Err(ApiError::conflict(format!("attempt is already {:?}", attempt.state))); }
        let mut running = self.running.lock().unwrap();
        match running.get_mut(attempt_id) {
            Some(r) => {
                r.cancel_requested = true;
                drop(running);
                self.emit(Event::new("attempt.cancel_requested", "user").about_attempt(&attempt));
                Ok(json!({"cancelling": attempt_id}))
            }
            None => {
                drop(running);
                // Queued or submitted: nothing runs, decide now.
                if matches!(attempt.state, AttemptState::Submitted | AttemptState::Verifying) {
                    return Err(ApiError::conflict("the attempt already submitted a candidate; reject it instead"));
                }
                self.move_attempt(attempt_id, AttemptState::Cancelled, Some("cancelled".into()))?;
                let _ = self.move_task(&attempt.task_id, TaskState::Blocked, Some("attempt cancelled; retry to continue".into()));
                Ok(json!({"cancelled": attempt_id}))
            }
        }
    }

    fn permission_respond(&self, params: &Value) -> ApiResult {
        let attempt_id = p_str(params, "attemptId")?;
        let request_id = p_str(params, "requestId")?;
        let option = p_opt(params, "optionId");
        let (session, harness) = {
            let running = self.running.lock().unwrap();
            let r = running.get(attempt_id).ok_or_else(|| ApiError::conflict("the attempt is not running"))?;
            (r.session.clone(), r.harness)
        };
        let adapter = self.adapter(harness)?;
        adapter.respond_permission(&session, request_id, option.as_deref()).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let attempt = self.update_attempt(attempt_id, |a| a.pending_permission = None)?;
        let _ = self.move_attempt(attempt_id, AttemptState::Running, None);
        self.emit(Event::new("permission.resolved", "user").about_attempt(&attempt).with(json!({"requestId": request_id, "optionId": option})));
        Ok(json!({"responded": request_id}))
    }

    fn latest_submitted(&self, task_id: &str) -> Res<Attempt> {
        let store = self.store.lock().unwrap();
        store
            .attempts_of_task(task_id)?
            .into_iter()
            .filter(|a| matches!(a.state, AttemptState::Submitted | AttemptState::Verifying))
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
            .ok_or_else(|| ApiError::conflict("the task has no submitted candidate"))
    }

    fn task_accept(self: &Arc<Service>, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        let attempt = self.latest_submitted(task_id)?;
        self.move_attempt(&attempt.id, AttemptState::Passed, None)?;
        self.move_task(task_id, TaskState::Accepted, None)?;
        self.emit(Event::new("candidate.selected", "user").about_attempt(&attempt).with(json!({"by": "user"})));
        if params.get("integrate").and_then(Value::as_bool).unwrap_or(false) {
            return self.task_integrate(task_id);
        }
        Ok(json!({"accepted": task_id, "attemptId": attempt.id}))
    }

    fn task_reject(self: &Arc<Service>, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        let reason = p_opt(params, "reason").unwrap_or_else(|| "rejected by user".into());
        let attempt = self.latest_submitted(task_id)?;
        self.move_attempt(&attempt.id, AttemptState::Failed, Some(reason.clone()))?;
        self.emit(Event::new("candidate.rejected", "user").about_attempt(&attempt).with(json!({"reason": reason})));
        let task = self.store.lock().unwrap().task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
        self.after_failed_attempt(&task, &reason)?;
        self.tick();
        Ok(json!({"rejected": task_id, "attemptId": attempt.id}))
    }

    fn task_integrate(self: &Arc<Service>, task_id: &str) -> ApiResult {
        let (task, quest, project, attempt) = {
            let store = self.store.lock().unwrap();
            let t = store.task(task_id)?.ok_or_else(|| ApiError::not_found(format!("task {task_id}")))?;
            if !matches!(t.state, TaskState::Accepted | TaskState::Blocked) { return Err(ApiError::conflict(format!("task is {:?}, accept it first", t.state))); }
            let q = store.quest(&t.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
            let p = store.project(&q.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            let a = store.attempts_of_task(task_id)?.into_iter().filter(|a| a.state == AttemptState::Passed).max_by(|a, b| a.created_at.cmp(&b.created_at)).ok_or_else(|| ApiError::conflict("no accepted attempt"))?;
            (t, q, p, a)
        };
        self.move_task(task_id, TaskState::Integrating, None)?;
        let Some(candidate) = &attempt.candidate else {
            self.move_task(task_id, TaskState::Completed, None)?;
            self.emit(Event::new("merge.skipped", "guild").about_attempt(&attempt).with(json!({"note": "no code changes to integrate"})));
            self.tick();
            return Ok(json!({"completed": task_id, "integrated": false}));
        };
        self.emit(Event::new("merge.started", "guild").about_attempt(&attempt).with(json!({"integrationRef": quest.integration_ref, "headSha": candidate.head_sha})));
        match workspaces::integrate(&project, &quest, candidate, &task) {
            Ok(sha) => {
                self.emit(Event::new("merge.completed", "guild").about_attempt(&attempt).with(json!({"integrationRef": quest.integration_ref, "sha": sha})));
                self.move_task(task_id, TaskState::Completed, None)?;
                self.tick();
                Ok(json!({"completed": task_id, "integrated": true, "sha": sha}))
            }
            Err(e) => {
                self.emit(Event::new("merge.blocked", "guild").about_attempt(&attempt).with(json!({"reason": e})));
                self.move_task(task_id, TaskState::Blocked, Some(format!("merge blocked: {e}")))?;
                Err(ApiError::conflict(e))
            }
        }
    }
}

fn compose_prompt(role_prompt: &str, task: &Task, workspace: &Workspace) -> String {
    let mut s = String::new();
    s.push_str(role_prompt.trim());
    s.push_str("\n\n# Task: ");
    s.push_str(&task.title);
    s.push_str("\n\n");
    s.push_str(task.instructions.trim());
    if !task.acceptance.is_empty() {
        s.push_str("\n\n## Acceptance criteria\n");
        for ac in &task.acceptance {
            s.push_str(&format!("- {}: {}\n", ac.id, ac.text));
        }
    }
    s.push_str(&format!(
        "\n## Rules\n- Your working directory is a dedicated git worktree on branch `{}`. Work only inside it.\n- Do not commit, push or switch branches; the guild commits your work when you finish.\n- Do not ask whether to proceed: do the work, then reply with a short summary of what changed, how you verified it, and anything left undone.\n",
        workspace.branch
    ));
    s
}

/// Idempotent commands: a repeated `idempotencyKey` returns the first result.
impl Handler for Arc<Service> {
    fn handle(&self, method: &str, params: &Value) -> ApiResult {
        let key = params.get("idempotencyKey").and_then(Value::as_str).map(str::to_string);
        if let Some(k) = &key {
            if let Some(result) = self.store.lock().unwrap().command_result(k)? { return Ok(result); }
        }
        let result = self.command(method, params)?;
        if let Some(k) = &key {
            self.store.lock().unwrap().remember_command(k, &result)?;
        }
        if method.starts_with("quest.") || method.starts_with("task.") || method.starts_with("member.") || method.starts_with("project.") { self.tick(); }
        Ok(result)
    }

    fn subscribe(&self, after_seq: u64, sender: Sender<Event>) -> Result<u64, ApiError> {
        let store = self.store.lock().unwrap();
        let last = store.last_seq()?;
        let mut cursor = after_seq;
        loop {
            let batch = store.events_after(cursor, 500)?;
            if batch.is_empty() { break; }
            for ev in batch {
                cursor = ev.seq;
                if sender.send(ev).is_err() { return Ok(last); }
            }
        }
        self.subscribers.lock().unwrap().push(sender);
        Ok(last)
    }
}

/// Run the service in this process until `guild.stop` or a signal.
pub fn run(service: Arc<Service>, socket: &Path) -> std::io::Result<()> {
    service.probe_harnesses();
    service.reconcile();
    let ticker = service.clone();
    let stop = service.stop.clone();
    std::thread::Builder::new()
        .name("guild-tick".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                ticker.tick();
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .ok();
    let handler: Arc<dyn Handler> = Arc::new(service.clone());
    let result = crate::api::serve(socket, handler, service.stop.clone());
    service.shutdown();
    result
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send + Sync>() {}
    is_send::<Service>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::mock::{MockBehaviour, MockHarness};

    struct TempHome(PathBuf, std::sync::MutexGuard<'static, ()>);
    impl TempHome {
        fn new() -> TempHome {
            let guard = crate::paths::test_lock().lock().unwrap_or_else(|e| e.into_inner());
            let p = std::env::temp_dir().join(format!("guild-home-{}", new_id()));
            std::fs::create_dir_all(&p).unwrap();
            std::env::set_var("HPD_GUILD_HOME", &p);
            TempHome(p, guard)
        }
        fn repo_path(&self) -> PathBuf { self.0.join("repo") }
        fn repo(&self) -> PathBuf {
            let repo = self.repo_path();
            std::fs::create_dir_all(&repo).unwrap();
            workspaces::git(&repo, &["init", "-q", "-b", "main"]).unwrap();
            std::fs::write(repo.join("README.md"), "hello\n").unwrap();
            workspaces::git(&repo, &["add", "-A"]).unwrap();
            workspaces::git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "init"]).unwrap();
            repo
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn wait_for(service: &Arc<Service>, what: &str, f: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let started = Instant::now();
        loop {
            service.tick();
            let snap = service.snapshot().unwrap();
            if f(&snap) { return snap; }
            if started.elapsed() > Duration::from_secs(20) { panic!("timed out waiting for {what}: {:?}", snap.attempts.iter().map(|a| (a.state, a.failure.clone())).collect::<Vec<_>>()); }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn setup(behaviour: MockBehaviour) -> (TempHome, Arc<Service>, String) {
        let home = TempHome::new();
        let repo = home.repo();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), cancel_timeout: Duration::from_millis(500), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        service.register(Arc::new(MockHarness::new(behaviour)));
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"], "model": "mock-fast"})).unwrap();
        let quest = service.handle("quest.create", &json!({"projectId": "p", "title": "Say hello"})).unwrap();
        let quest_id = quest["id"].as_str().unwrap().to_string();
        (home, service, quest_id)
    }

    #[test]
    fn a_task_runs_in_its_own_worktree_and_waits_for_acceptance() {
        let (_home, service, quest_id) = setup(MockBehaviour::Succeed { file: "hello.txt".into(), content: "hi\n".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Write hello", "instructions": "Create hello.txt", "acceptance": ["hello.txt exists"]})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "submitted", |s| s.tasks[0].state == TaskState::Verifying);
        let attempt = &snap.attempts[0];
        assert_eq!(attempt.state, AttemptState::Submitted);
        let candidate = attempt.candidate.as_ref().expect("a candidate commit");
        assert_eq!(candidate.files_changed, 1);
        assert_eq!(attempt.model_effective.as_deref(), Some("mock-fast"));
        let ws = &snap.workspaces[0];
        assert!(ws.path.contains("worktrees"));
        // The user's checkout is untouched.
        assert!(!_home.repo_path().join("hello.txt").exists());
        // Accept and integrate: the quest completes.
        service.handle("task.accept", &json!({"taskId": "t1", "integrate": true})).unwrap();
        let snap = wait_for(&service, "completed", |s| s.quests[0].state == QuestState::Completed);
        assert_eq!(snap.tasks[0].state, TaskState::Completed);
        let kinds: Vec<String> = service.store.lock().unwrap().events_after(0, 1000).unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"merge.completed".to_string()));
        assert!(kinds.contains(&"tool.started".to_string()));
        // The normalized agent events the dungeon draws from.
        for k in ["agent.working", "file.changed", "attempt.finished", "session.started", "session.closed"] {
            assert!(kinds.contains(&k.to_string()), "missing {k} in {kinds:?}");
        }
        assert_eq!(kinds.iter().filter(|k| *k == "agent.working").count(), 1, "one phase event per change");
        assert!(!kinds.iter().any(|k| k == "task.completed") || kinds.iter().position(|k| k == "task.completed") > kinds.iter().position(|k| k == "attempt.submitted"));
    }

    #[test]
    fn a_failed_turn_retries_once_then_fails_the_task() {
        let (_home, service, quest_id) = setup(MockBehaviour::Fail("boom".into()));
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Doomed", "maxAttempts": 2})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "failed", |s| s.tasks[0].state == TaskState::Failed);
        assert_eq!(snap.attempts.len(), 2);
        assert!(snap.attempts.iter().all(|a| a.state == AttemptState::Failed));
        assert!(snap.tasks[0].block_reason.as_deref().unwrap().contains("2 attempts"));
        // Retry grants one more attempt.
        service.handle("task.retry", &json!({"taskId": "t1"})).unwrap();
        let snap = wait_for(&service, "third attempt", |s| s.attempts.len() == 3);
        assert_eq!(snap.tasks[0].max_attempts, 3);
    }

    #[test]
    fn a_permission_parks_the_attempt_until_the_user_answers() {
        let (_home, service, quest_id) = setup(MockBehaviour::AskPermission { file: "x.txt".into(), content: "x".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Ask first"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "waiting", |s| s.attempts.first().map(|a| a.state == AttemptState::WaitingInput).unwrap_or(false));
        let a = &snap.attempts[0];
        let perm = a.pending_permission.as_ref().unwrap();
        assert_eq!(room_state(None, Some(a)), "blocked");
        service.handle("permission.respond", &json!({"attemptId": a.id, "requestId": perm.request_id, "optionId": "allow"})).unwrap();
        let snap = wait_for(&service, "submitted", |s| s.attempts[0].state == AttemptState::Submitted);
        assert!(snap.attempts[0].pending_permission.is_none());
    }

    #[test]
    fn cancelling_a_hung_attempt_blocks_the_task_and_dependants_wait_for_parents() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hangs"})).unwrap();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t2", "title": "Child", "dependsOn": ["t1"]})).unwrap();
        assert!(service.handle("task.create", &json!({"questId": quest_id, "id": "t3", "title": "Cycle", "dependsOn": ["t3"]})).is_err());
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "running", |s| s.attempts.first().map(|a| a.state == AttemptState::Running).unwrap_or(false));
        assert_eq!(snap.tasks.iter().find(|t| t.id == "t2").unwrap().state, TaskState::Pending);
        service.handle("attempt.cancel", &json!({"attemptId": snap.attempts[0].id})).unwrap();
        let snap = wait_for(&service, "cancelled", |s| s.attempts[0].state == AttemptState::Cancelled);
        assert_eq!(snap.tasks.iter().find(|t| t.id == "t1").unwrap().state, TaskState::Blocked);
    }

    #[test]
    fn interrupting_keeps_the_worktree_as_the_candidate() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hangs"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "running", |s| s.attempts.first().map(|a| a.state == AttemptState::Running).unwrap_or(false));
        service.handle("attempt.interrupt", &json!({"attemptId": snap.attempts[0].id})).unwrap();
        let snap = wait_for(&service, "submitted", |s| s.attempts[0].state == AttemptState::Submitted);
        assert_eq!(snap.tasks[0].state, TaskState::Verifying);
        assert!(snap.attempts[0].candidate.is_none(), "nothing was written, so no candidate commit");
        let kinds: Vec<String> = service.store.lock().unwrap().events_after(0, 1000).unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"attempt.interrupt_requested".to_string()));
        assert!(kinds.contains(&"candidate.empty".to_string()));
    }

    #[test]
    fn a_restart_resumes_the_session_when_the_harness_can() {
        let home = TempHome::new();
        let repo = home.repo();
        let db = home.0.join("guild.db");
        // First life: an attempt is running when the service "dies".
        let first = Service::new(Store::open(&db).unwrap(), Settings { attempt_timeout: Duration::from_secs(600), ..Settings::default() });
        let hang = Arc::new(MockHarness::new(MockBehaviour::Hang));
        first.register(hang.clone());
        first.probe_harnesses();
        first.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        first.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"], "model": "mock-fast"})).unwrap();
        let quest_id = first.handle("quest.create", &json!({"projectId": "p", "title": "Survive"})).unwrap()["id"].as_str().unwrap().to_string();
        first.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Keep going"})).unwrap();
        first.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&first, "running", |s| s.attempts.first().map(|a| a.state == AttemptState::Running).unwrap_or(false));
        let attempt_id = snap.attempts[0].id.clone();
        // Second life over the same database: the quest and task are still there,
        // and the attempt goes on in a reopened session that now finishes.
        let second = Service::new(Store::open(&db).unwrap(), Settings::default());
        let mock = Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "done.txt".into(), content: "ok\n".into() }));
        second.register(mock.clone());
        second.probe_harnesses();
        let snap = second.snapshot().unwrap();
        assert_eq!(snap.quests[0].state, QuestState::Running);
        assert_eq!(snap.tasks[0].state, TaskState::Running);
        second.reconcile();
        let snap = wait_for(&second, "resumed and submitted", |s| s.attempts[0].state == AttemptState::Submitted);
        assert_eq!(snap.attempts[0].id, attempt_id, "the same attempt, not a new one");
        assert_eq!(snap.attempts[0].resumed, 1);
        assert_eq!(snap.attempts[0].turns, 2);
        assert_eq!(mock.resumed().len(), 1);
        assert_eq!(snap.attempts[0].candidate.as_ref().unwrap().files_changed, 1);
        let kinds: Vec<String> = second.store.lock().unwrap().events_after(0, 1000).unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"session.resumed".to_string()));
        // Let the first life's hung turn end so its thread stops.
        let _ = first.handle("attempt.cancel", &json!({"attemptId": attempt_id}));
    }

    #[test]
    fn repeated_commands_with_a_key_do_not_run_twice() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        let first = service.handle("task.create", &json!({"questId": quest_id, "title": "Once", "idempotencyKey": "k1"})).unwrap();
        let second = service.handle("task.create", &json!({"questId": quest_id, "title": "Once", "idempotencyKey": "k1"})).unwrap();
        assert_eq!(first, second);
        assert_eq!(service.snapshot().unwrap().tasks.len(), 1);
    }
}
