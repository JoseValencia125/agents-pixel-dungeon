//! The service: owns the store, the adapters and every running attempt.
//! Commands come in over the socket, the scheduler ticks once a second,
//! and each attempt is driven by its own thread from provisioning to a
//! candidate commit. Only this code changes a state; harnesses report.

use crate::api::{ApiError, ApiResult, Handler};
use crate::domain::*;
use crate::harness::{HarnessAdapter, HarnessEvent, SessionOptions, SessionRef, Support, TurnOutcome};
use crate::store::Store;
use crate::experiment::{Experiment, ExperimentDecision, ExperimentResult, ExperimentStatus, Hypothesis, HypothesisStatus, Observation, Playtest, PlaytestKind, Variant, OBSERVATION_KINDS};
use crate::{config, scheduler, workspaces};
use serde_json::{json, Value};
use sha2::Digest;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// How the orchestrator's instructions follow a failure, for the retry to read.
const ORCHESTRATOR_RETRY: &str = " — the orchestrator, retrying: ";

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
    /// Time spent waiting for someone to answer a permission, which the
    /// attempt's time limit does not count: the member was not working.
    waited: Duration,
    waiting_since: Option<Instant>,
    /// How long it may work: its task's limit, else the guild's.
    limit: Duration,
}

impl Running {
    /// How long the member has worked: its time less what it waited for.
    fn working_time(&self) -> Duration {
        let waiting = self.waiting_since.map(|w| w.elapsed()).unwrap_or_default();
        self.started.elapsed().saturating_sub(self.waited + waiting)
    }
}

pub struct Service {
    store: Mutex<Store>,
    adapters: Mutex<HashMap<HarnessKind, Arc<dyn HarnessAdapter>>>,
    harness_status: Mutex<Vec<HarnessStatus>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    /// Events appended inside an open unit of work, sent on commit.
    held: Mutex<Vec<Event>>,
    running: Mutex<HashMap<String, Running>>,
    /// Experiments whose playtests a thread is running.
    playtesting: Mutex<Vec<String>>,
    /// AI playtests asked to stop.
    ai_stops: Mutex<Vec<String>>,
    /// The member answering the user right now, if one is.
    orchestrating: Mutex<Option<String>>,
    /// Tasks being settled, and those asked again meanwhile: two candidates
    /// finishing together must not start two judges, or a judge and the policy.
    settling: Mutex<(HashSet<String>, HashSet<String>)>,
    /// Decisions waiting for the guild's orchestrator (attempt, what about);
    /// the scheduler's tick hands each to a thread of its own.
    decisions: Mutex<Vec<(String, crate::orchestrator::DecisionKind)>>,
    /// How many attempts run at once: the settings' until the orchestrator
    /// or the user changes it (`guild.concurrency`), then kept in the store.
    concurrency: AtomicUsize,
    pub settings: Settings,
    pub stop: Arc<AtomicBool>,
    log_file: Mutex<Option<std::fs::File>>,
    pub quiet: bool,
}

type Res<T> = Result<T, ApiError>;

/// True right after `begin` opened the outermost unit of work.
fn store_depth_is_one(store: &Store) -> bool { store.depth() == 1 }

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
        let concurrency = store.meta("concurrency").ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(settings.concurrency);
        Arc::new(Service {
            concurrency: AtomicUsize::new(concurrency),
            store: Mutex::new(store),
            adapters: Mutex::new(HashMap::new()),
            harness_status: Mutex::new(vec![]),
            subscribers: Mutex::new(vec![]),
            held: Mutex::new(vec![]),
            playtesting: Mutex::new(vec![]),
            ai_stops: Mutex::new(vec![]),
            orchestrating: Mutex::new(None),
            settling: Mutex::new((HashSet::new(), HashSet::new())),
            decisions: Mutex::new(vec![]),
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
        self.harness_status.lock().unwrap().iter().filter(|s| s.available).map(|s| s.kind.clone()).collect()
    }

    // ----- events -----

    fn emit(&self, event: Event) -> Event {
        let store = self.store.lock().unwrap();
        self.emit_locked(&store, event)
    }

    fn emit_locked(&self, store: &Store, event: Event) -> Event {
        match store.append(event) {
            Ok(ev) => {
                if store.in_transaction() {
                    // Subscribers hear of it once it is durable.
                    self.held.lock().unwrap().push(ev.clone());
                } else {
                    self.subscribers.lock().unwrap().retain(|s| s.send(ev.clone()).is_ok());
                }
                ev
            }
            Err(e) => {
                self.log(&format!("cannot append event: {e}"));
                Event::new("error", "guild")
            }
        }
    }

    /// State and its events change together: everything `f` writes
    /// through `store` commits as one, or none of it does, and the events
    /// reach subscribers only after the commit. Nests.
    fn atomic<T>(&self, store: &Store, f: impl FnOnce() -> Res<T>) -> Res<T> {
        store.begin()?;
        let outer = !store.in_transaction() || store_depth_is_one(store);
        match f() {
            Ok(v) => {
                store.commit()?;
                if outer {
                    let held: Vec<Event> = std::mem::take(&mut *self.held.lock().unwrap());
                    if !held.is_empty() {
                        self.subscribers.lock().unwrap().retain(|s| held.iter().all(|ev| s.send(ev.clone()).is_ok()));
                    }
                }
                Ok(v)
            }
            Err(e) => {
                store.rollback();
                if outer { self.held.lock().unwrap().clear(); }
                Err(e)
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
        self.atomic(&store, || self.move_attempt_locked(&store, id, to, failure))
    }

    fn move_attempt_locked(&self, store: &Store, id: &str, to: AttemptState, failure: Option<String>) -> Res<Attempt> {
        let mut a = store.attempt(id)?.ok_or_else(|| ApiError::not_found(format!("attempt {id}")))?;
        if a.state == to { return Ok(a); }
        attempt_transition(a.state, to).map_err(ApiError::conflict)?;
        a.state = to;
        if failure.is_some() { a.failure = failure; }
        if to.is_terminal() {
            a.finished_at = Some(now());
            // A finished attempt asks for nothing any more: its harness is gone.
            a.pending_permission = None;
            a.deciding = false;
        }
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
            let _ = std::fs::remove_dir_all(crate::paths::scratch_dir(&a.id));
            // One row per finished attempt, so routing can learn from history later.
            let task = store.task(&a.task_id)?;
            let member = store.member(&a.member_id)?;
            let duration_ms = a.started_at.as_deref().zip(a.finished_at.as_deref()).and_then(|(s, f)| {
                let s = chrono::DateTime::parse_from_rfc3339(s).ok()?;
                let f = chrono::DateTime::parse_from_rfc3339(f).ok()?;
                Some((f - s).num_milliseconds())
            });
            let decision = store.decisions_of_task(&a.task_id)?.into_iter().rev().find(|d| d.winner_attempt_id.is_some());
            let label_score = decision.as_ref().and_then(|d| d.candidates.iter().find(|c| c.attempt_id == a.id)).and_then(|c| c.score);
            let record = AttemptRecord {
                attempt_id: a.id.clone(), task_id: a.task_id.clone(), quest_id: a.quest_id.clone(), project_id: a.project_id.clone(),
                task_title: task.as_ref().map(|t| t.title.clone()).unwrap_or_default(), role: task.as_ref().map(|t| t.role.clone()).unwrap_or_default(),
                strategy: task.as_ref().map(|t| t.strategy.kind).unwrap_or_default(), candidates: task.as_ref().map(|t| t.candidates).unwrap_or(1), selection: task.as_ref().map(|t| t.strategy.selection()).unwrap_or_default(),
                kind: a.kind, member_id: a.member_id.clone(), harness: a.harness.clone(), model: a.model_effective.clone().or(a.model_requested.clone()), result: to,
                attempt_number: task.as_ref().map(|t| t.attempts_made).unwrap_or(0), duration_ms, turns: a.turns, resumed: a.resumed, usage: None,
                files_changed: a.candidate.as_ref().map(|c| c.files_changed), insertions: a.candidate.as_ref().map(|c| c.insertions), deletions: a.candidate.as_ref().map(|c| c.deletions),
                verifiers_passed: a.verifier_results.iter().filter(|v| v.status == "passed").count() as u32, verifiers_failed: a.verifier_results.iter().filter(|v| v.status != "passed").count() as u32,
                judge_score: label_score, winner: decision.as_ref().map(|d| d.winner_attempt_id.as_deref() == Some(a.id.as_str())).filter(|_| a.kind == AttemptKind::Work),
                decided_by: decision.as_ref().map(|d| d.by.clone()), human_override: decision.as_ref().map(|d| d.human_override).unwrap_or(false), failure: a.failure.clone(), created_at: now(),
            };
            store.put_record(&record)?;
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
        self.atomic(store, || self.move_task_inner(store, id, to, reason))
    }

    fn move_task_inner(&self, store: &Store, id: &str, to: TaskState, reason: Option<String>) -> Res<Task> {
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
        // Decisions the orchestrator was asked for since the last tick.
        for (attempt_id, kind) in std::mem::take(&mut *self.decisions.lock().unwrap()) {
            let service = self.clone();
            std::thread::Builder::new().name("decision".into()).spawn(move || service.decide(&attempt_id, kind)).ok();
        }
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
                self.atomic(&store, || {
                    let mut q = q.clone();
                    q.state = QuestState::Completed;
                    q.updated_at = now();
                    store.put_quest(&q)?;
                    self.emit_locked(&store, Event::new("quest.completed", "guild").about_quest(&q).with(json!({"title": q.title})));
                    Ok(())
                })?;
            }
        }
        self.experiments_tick()?;
        // Dispatch.
        let running_quests: Vec<String> = quests.iter().filter(|q| q.state == QuestState::Running).map(|q| q.id.clone()).collect();
        let tasks = self.store.lock().unwrap().tasks()?;
        // A race that lost every runner (a restart, a failure) is settled here too.
        for t in tasks.iter().filter(|t| t.candidates > 1 && t.state == TaskState::Running && t.attempts_made >= t.candidates) {
            let pending = self.store.lock().unwrap().attempts_of_task(&t.id)?.into_iter().any(|a| a.state.is_active() && a.state != AttemptState::Submitted);
            if !pending { self.settle_task(&t.id)?; }
        }
        let mut slots: Vec<(&Task, u32)> = scheduler::ready_in_order(&tasks, &running_quests).into_iter().map(|t| (t, t.candidates.max(1))).collect();
        // Racing tasks already running still want their remaining runners.
        for t in tasks.iter().filter(|t| t.candidates > 1 && t.state == TaskState::Running && running_quests.contains(&t.quest_id) && t.attempts_made < t.candidates) {
            slots.push((t, t.candidates - t.attempts_made));
        }
        if slots.is_empty() { return Ok(()); }
        let available = self.available_harnesses();
        for (task, wanted) in slots { for _ in 0..wanted {
            let active = self.running.lock().unwrap().len();
            if active >= self.concurrency.load(Ordering::Relaxed) { break; }
            let attempt = {
                let store = self.store.lock().unwrap();
                let members = store.members()?;
                // Runners of one race never share a member; a retry may reuse one.
                let mut busy: Vec<String> = store.active_attempts()?.into_iter().map(|a| a.member_id).collect();
                if task.candidates > 1 { busy.extend(store.attempts_of_task(&task.id)?.into_iter().map(|a| a.member_id)); }
                let Some(member) = scheduler::pick_member(task, &members, &busy, &available).cloned() else {
                    let why = if members.iter().any(|m| m.roles.contains(&task.role)) { "no free member" } else { "no member has the role" };
                    if task.block_reason.as_deref() != Some(why) && members.iter().all(|m| !m.roles.contains(&task.role)) {
                        self.move_task_locked(&store, &task.id, TaskState::Blocked, Some(why.into()))?;
                    }
                    break;
                };
                let quest = store.quest(&task.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
                let t = store.task(&task.id)?.ok_or_else(|| ApiError::not_found("task"))?;
                let racing_more = t.candidates > 1 && t.state == TaskState::Running && t.attempts_made < t.candidates;
                if t.state != TaskState::Ready && !racing_more { break; }
                let model = t.model_id.clone().or(member.model_id.clone());
                let attempt = Attempt {
                    id: new_id(),
                    task_id: t.id.clone(),
                    task_version: t.version,
                    quest_id: quest.id.clone(),
                    project_id: quest.project_id.clone(),
                    member_id: member.id.clone(),
                    harness: member.harness.clone(),
                    model_requested: model,
                    model_effective: None,
                    kind: AttemptKind::Work,
                    state: AttemptState::Queued,
                    workspace_id: None,
                    session_id: None,
                    generation: 1,
                    candidate: None, partial: None,
                    summary: String::new(),
                    action: None,
                    pending_permission: None,
                    failure: None,
                    verifier_results: vec![],
                    turns: 0,
                    resumed: 0, deciding: false, escalation: None, worked_seconds: 0, limit_seconds: 0,
                    started_at: Some(now()),
                    finished_at: None,
                    created_at: now(),
                    updated_at: now(),
                };
                self.atomic(&store, || {
                    let mut t = t.clone();
                    t.state = TaskState::Running;
                    t.attempts_made += 1;
                    t.updated_at = now();
                    store.put_task(&t)?;
                    store.put_attempt(&attempt)?;
                    if !racing_more { self.emit_locked(&store, Event::new("task.running", "guild").about_task(&t, &quest.project_id).with(json!({"state": "running", "title": t.title, "candidates": t.candidates}))); }
                    self.emit_locked(&store, Event::new("attempt.started", "guild").about_attempt(&attempt).with(json!({"member": member.name, "harness": member.harness, "model": attempt.model_requested, "task": t.title, "runner": t.attempts_made, "candidates": t.candidates})));
                    Ok(())
                })?;
                attempt
            };
            self.spawn_driver(attempt.id.clone());
        } }
        Ok(())
    }

    /// The work attempts of a task are done (submitted or dropped out):
    /// decide what the strategy says. The hard gates come first: only
    /// candidates whose required verifiers passed can be chosen. Then the
    /// policy picks (smallest change, then the earliest), a judge is asked,
    /// or the user is left to choose on the board.
    fn settle_task(self: &Arc<Service>, task_id: &str) -> Res<()> {
        {
            let mut s = self.settling.lock().unwrap();
            if !s.0.insert(task_id.to_string()) { s.1.insert(task_id.to_string()); return Ok(()); }
        }
        // One at a time per task; a call that came meanwhile runs once more after.
        let result = loop {
            let r = self.settle_task_once(task_id);
            if r.is_err() || !self.settling.lock().unwrap().1.remove(task_id) { break r; }
        };
        let mut s = self.settling.lock().unwrap();
        s.0.remove(task_id);
        s.1.remove(task_id);
        result
    }

    fn settle_task_once(self: &Arc<Service>, task_id: &str) -> Res<()> {
        let (task, attempts) = {
            let store = self.store.lock().unwrap();
            let t = store.task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            (t.clone(), store.attempts_of_task(task_id)?)
        };
        if !matches!(task.state, TaskState::Running | TaskState::Verifying) { return Ok(()); }
        if task.state == TaskState::Verifying && (task.strategy.selection() == Selection::User || attempts.iter().any(|a| a.kind == AttemptKind::Judge && a.state.is_active())) { return Ok(()); }
        if task.state == TaskState::Verifying && self.store.lock().unwrap().decisions_of_task(task_id)?.iter().any(|d| d.winner_attempt_id.is_some()) { return Ok(()); }
        let work: Vec<&Attempt> = attempts.iter().filter(|a| a.kind == AttemptKind::Work).collect();
        if work.iter().any(|a| a.state.is_active() && a.state != AttemptState::Submitted) { return Ok(()); }
        if task.attempts_made < task.candidates { return Ok(()); }
        let submitted: Vec<&Attempt> = work.iter().copied().filter(|a| a.state == AttemptState::Submitted).collect();
        let project_id = attempts.first().map(|a| a.project_id.clone()).unwrap_or_default();
        if submitted.is_empty() {
            if task.candidates > 1 { self.emit(Event::new("race.finished", "guild").about_task(&task, &project_id).with(json!({"candidates": task.candidates, "submitted": 0}))); }
            return self.after_failed_attempt(&task, "no candidate survived");
        }
        if task.state == TaskState::Running { self.move_task(task_id, TaskState::Verifying, None)?; }
        let passing: Vec<&Attempt> = submitted.iter().copied().filter(|a| passes_gates(a)).collect();
        if task.candidates > 1 {
            self.emit(Event::new("race.finished", "guild").about_task(&task, &project_id).with(json!({"candidates": task.candidates, "submitted": submitted.len(), "passing": passing.len(), "selection": task.strategy.selection(), "autoIntegrate": task.auto_integrate})));
        }
        match task.strategy.selection() {
            Selection::User => Ok(()),
            Selection::Policy => self.decide_by_policy(task_id, "policy", None),
            Selection::Judge => {
                if passing.is_empty() { self.log(&format!("task {task_id}: nothing passed the gates; waiting for the user")); return Ok(()); }
                match self.start_judge(&task, &passing)? {
                    true => Ok(()),
                    false => self.decide_by_policy(task_id, "policy", Some("no judge available".into())),
                }
            }
        }
    }

    /// The baseline selection: among the candidates that passed the hard
    /// gates, the smallest change, then the earliest. Records the decision.
    fn decide_by_policy(self: &Arc<Service>, task_id: &str, by: &str, note: Option<String>) -> Res<()> {
        let (task, attempts, decided) = {
            let store = self.store.lock().unwrap();
            (store.task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?, store.attempts_of_task(task_id)?, store.decisions_of_task(task_id)?.iter().any(|d| d.winner_attempt_id.is_some()))
        };
        if decided || task.winner_attempt_id.is_some() { self.log(&format!("task {task_id}: already decided")); return Ok(()); }
        let submitted: Vec<&Attempt> = attempts.iter().filter(|a| a.kind == AttemptKind::Work && a.state == AttemptState::Submitted).collect();
        let passing: Vec<&Attempt> = submitted.iter().copied().filter(|a| passes_gates(a)).collect();
        let Some(winner) = passing.iter().min_by_key(|a| (a.candidate.as_ref().map(|c| c.insertions + c.deletions).unwrap_or(u32::MAX), a.finished_at.clone().unwrap_or_default(), a.id.clone())) else {
            self.log(&format!("task {task_id}: no candidate passed the verifiers; waiting for the user"));
            return Ok(());
        };
        let reason = match note { Some(n) => format!("{n}; smallest passing change, then the earliest"), None => "smallest passing change, then the earliest".into() };
        let decision = RaceDecision {
            id: new_id(), task_id: task_id.into(), quest_id: task.quest_id.clone(), project_id: winner.project_id.clone(), by: by.into(), judge_attempt_id: None,
            candidates: submitted.iter().map(|a| decision_candidate(a, &label_for(&submitted, a))).collect(),
            scores: json!({}), findings: json!({}), tradeoffs: String::new(), winner_attempt_id: Some(winner.id.clone()), confidence: None, reason, human_override: false, created_at: now(),
        };
        let winner_id = winner.id.clone();
        self.record_decision(&decision)?;
        self.select_candidate(task_id, &winner_id, by)?;
        if task.auto_integrate {
            if let Err(e) = self.task_integrate(task_id) { self.log(&format!("task {task_id}: integration failed: {}", e.message)); }
        }
        Ok(())
    }

    // ----- hypotheses and experiments -----

    fn hypothesis_create(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let store = self.store.lock().unwrap();
        store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
        let h = Hypothesis {
            id: p_opt(params, "id").unwrap_or_else(new_id), project_id: project_id.into(),
            problem: p_str(params, "problem")?.into(), evidence: p_opt(params, "evidence").unwrap_or_default(),
            level: serde_json::from_value(params.get("level").cloned().unwrap_or(json!("loop"))).map_err(|e| ApiError::invalid(format!("level: {e}")))?,
            hypothesis: p_str(params, "hypothesis")?.into(), expected_effect: p_opt(params, "expectedEffect").unwrap_or_default(), proposed_change: p_opt(params, "proposedChange").unwrap_or_default(),
            measurements: p_list(params, "measurements"), related_systems: p_list(params, "relatedSystems"), status: HypothesisStatus::Proposed, created_at: now(), updated_at: now(),
            question_id: p_opt(params, "questionId"),
        };
        self.atomic(&store, || {
            store.put_hypothesis(&h)?;
            if let Some(qid) = &h.question_id {
                let mut q = store.question(project_id, qid)?.ok_or_else(|| ApiError::not_found(format!("question {qid}")))?;
                if !q.hypothesis_ids.contains(&h.id) { q.hypothesis_ids.push(h.id.clone()); }
                store.put_question(&q)?;
            }
            self.emit_locked(&store, Event::new("hypothesis.created", "user").with(json!({"projectId": project_id, "hypothesisId": h.id, "problem": h.problem, "level": h.level})));
            Ok(())
        })?;
        Ok(serde_json::to_value(h).unwrap())
    }

    fn hypothesis_update(&self, params: &Value) -> ApiResult {
        let id = p_str(params, "hypothesisId")?;
        let store = self.store.lock().unwrap();
        let mut h = store.hypothesis(id)?.ok_or_else(|| ApiError::not_found(format!("hypothesis {id}")))?;
        if let Some(v) = p_opt(params, "problem") { h.problem = v; }
        if let Some(v) = p_opt(params, "evidence") { h.evidence = v; }
        if let Some(v) = p_opt(params, "hypothesis") { h.hypothesis = v; }
        if let Some(v) = p_opt(params, "expectedEffect") { h.expected_effect = v; }
        if let Some(v) = p_opt(params, "proposedChange") { h.proposed_change = v; }
        if params.get("measurements").is_some() { h.measurements = p_list(params, "measurements"); }
        if params.get("relatedSystems").is_some() { h.related_systems = p_list(params, "relatedSystems"); }
        if let Some(l) = params.get("level") { h.level = serde_json::from_value(l.clone()).map_err(|e| ApiError::invalid(format!("level: {e}")))?; }
        if let Some(s) = params.get("status") { h.status = serde_json::from_value(s.clone()).map_err(|e| ApiError::invalid(format!("status: {e}")))?; }
        h.updated_at = now();
        self.atomic(&store, || {
            store.put_hypothesis(&h)?;
            self.emit_locked(&store, Event::new("hypothesis.updated", "user").with(json!({"hypothesisId": h.id, "status": h.status})));
            Ok(())
        })?;
        Ok(serde_json::to_value(h).unwrap())
    }

    fn experiment_create(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let hypothesis_id = p_str(params, "hypothesisId")?;
        let store = self.store.lock().unwrap();
        store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
        let h = store.hypothesis(hypothesis_id)?.ok_or_else(|| ApiError::not_found(format!("hypothesis {hypothesis_id}")))?;
        let mut variants: Vec<Variant> = serde_json::from_value(params.get("variants").cloned().unwrap_or(json!([]))).map_err(|e| ApiError::invalid(format!("variants: {e}")))?;
        if variants.is_empty() { return Err(ApiError::invalid("an experiment needs at least one variant")); }
        for v in variants.iter_mut() {
            v.id = slug(if v.id.trim().is_empty() { &v.name } else { &v.id });
            if v.name.trim().is_empty() { v.name = v.id.clone(); }
            v.task_id = None;
            v.attempt_id = None;
        }
        let mut seen = std::collections::HashSet::new();
        if !variants.iter().all(|v| seen.insert(v.id.clone())) { return Err(ApiError::invalid("variant ids repeat")); }
        let plan = match params.get("plan") { Some(p) => serde_json::from_value(p.clone()).map_err(|e| ApiError::invalid(format!("plan: {e}")))?, None => Default::default() };
        let e = Experiment {
            id: p_opt(params, "id").unwrap_or_else(new_id), project_id: project_id.into(), hypothesis_id: hypothesis_id.into(),
            title: p_opt(params, "title").unwrap_or_else(|| format!("Experiment: {}", h.hypothesis.chars().take(60).collect::<String>())),
            status: ExperimentStatus::Draft, quest_id: None, variants, plan, analysis_task_id: None, result: None, decision: None, failure: None, created_at: now(), updated_at: now(),
        };
        self.atomic(&store, || {
            store.put_experiment(&e)?;
            self.emit_locked(&store, Event::new("experiment.created", "user").with(json!({"projectId": project_id, "experimentId": e.id, "hypothesisId": hypothesis_id, "variants": e.variants.len(), "title": e.title})));
            Ok(())
        })?;
        Ok(serde_json::to_value(e).unwrap())
    }

    /// Build the variants: a quest with one single-attempt task per
    /// variant, chosen by the policy and never integrated by itself, plus
    /// an analysis task that waits for them.
    fn experiment_start(self: &Arc<Service>, id: &str) -> ApiResult {
        let (mut e, project, hypothesis, members) = {
            let store = self.store.lock().unwrap();
            let e = store.experiment(id)?.ok_or_else(|| ApiError::not_found(format!("experiment {id}")))?;
            if e.status != ExperimentStatus::Draft { return Err(ApiError::conflict(format!("experiment is {:?}", e.status))); }
            let p = store.project(&e.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            let h = store.hypothesis(&e.hypothesis_id)?.ok_or_else(|| ApiError::not_found("hypothesis"))?;
            (e, p, h, store.members()?)
        };
        if project.playtest.is_none() { return Err(ApiError::invalid("the project has no [playtest] runner in .guild/guild.toml")); }
        let builder_role = if members.iter().any(|m| m.roles.iter().any(|r| r == "gameplay-engineer")) { "gameplay-engineer" } else { "implementer" };
        let quest = self.quest_create(&json!({"projectId": e.project_id, "title": e.title, "description": format!("Experiment on: {}", hypothesis.hypothesis)}))?;
        let quest_id = quest["id"].as_str().unwrap_or("").to_string();
        let short = e.id[e.id.len().saturating_sub(8)..].to_string();
        let mut variant_tasks = vec![];
        for v in e.variants.iter_mut() {
            let task_id = format!("{short}-{}", v.id);
            let params_json = serde_json::to_string(&v.params).unwrap_or_else(|_| "{}".into());
            let mut instructions = String::new();
            if v.id == "control" && v.params.is_empty() {
                instructions.push_str("This is the control variant: the game as it is. Change nothing. Reply that the build is ready.\n");
            } else {
                instructions.push_str(&format!("Build variant \"{}\" of the experiment \"{}\" for the hypothesis: {}\n\n", v.name, e.title, hypothesis.hypothesis));
                if !v.instructions.trim().is_empty() { instructions.push_str(v.instructions.trim()); instructions.push_str("\n\n"); }
                if !v.params.is_empty() { instructions.push_str(&format!("Apply exactly these parameters to the game's balance/configuration, changing nothing else:\nGUILD_PARAMS: {params_json}\n")); }
            }
            self.task_create(&json!({"questId": quest_id, "id": task_id, "title": format!("Variant: {}", v.name), "instructions": instructions, "role": builder_role,
                "strategy": {"type": "single", "candidates": 1, "selection": "policy", "auto_integrate": false},
                "acceptance": [format!("the build runs with parameters {params_json}")]}))?;
            v.task_id = Some(task_id.clone());
            variant_tasks.push(task_id);
        }
        let analysis_id = format!("{short}-analysis");
        self.task_create(&json!({"questId": quest_id, "id": analysis_id, "title": "Analysis", "instructions": "Analyse the playtests of the experiment's variants against the hypothesis.", "role": "design-analyst", "dependsOn": variant_tasks,
            "strategy": {"type": "single", "candidates": 1, "selection": "user", "auto_integrate": false}}))?;
        e.analysis_task_id = Some(analysis_id);
        e.quest_id = Some(quest_id.clone());
        e.status = ExperimentStatus::Building;
        e.updated_at = now();
        {
            let store = self.store.lock().unwrap();
            self.atomic(&store, || {
                store.put_experiment(&e)?;
                if let Some(qid) = &hypothesis.question_id {
                    if let Some(mut q) = store.question(&e.project_id, qid)? {
                        if !q.experiment_ids.contains(&e.id) { q.experiment_ids.push(e.id.clone()); }
                        store.put_question(&q)?;
                    }
                }
                let mut h = hypothesis.clone();
                h.status = HypothesisStatus::Experimenting;
                h.updated_at = now();
                store.put_hypothesis(&h)?;
                self.emit_locked(&store, Event::new("experiment.started", "user").with(json!({"experimentId": e.id, "questId": quest_id, "variants": e.variants.len(), "title": e.title})));
                Ok(())
            })?;
        }
        self.quest_start(&json!({"questId": quest_id}))?;
        Ok(serde_json::to_value(e).unwrap())
    }

    /// Move experiments along: builds done, then playtests; playtests
    /// done, then analysis; a lost analysis, then a new one. Runs every
    /// tick, so it also picks up where a restart left off.
    fn experiments_tick(self: &Arc<Service>) -> Res<()> {
        let experiments = self.store.lock().unwrap().experiments()?;
        for mut e in experiments.into_iter().filter(|e| matches!(e.status, ExperimentStatus::Building | ExperimentStatus::Playtesting | ExperimentStatus::Analysing)) {
            match e.status {
                ExperimentStatus::Building => {
                    let store = self.store.lock().unwrap();
                    let mut all_built = true;
                    let mut failed: Option<String> = None;
                    for v in e.variants.iter_mut() {
                        let Some(tid) = &v.task_id else { all_built = false; continue };
                        let Some(t) = store.task(tid)? else { all_built = false; continue };
                        match t.state {
                            TaskState::Accepted | TaskState::Completed => {
                                v.attempt_id = t.winner_attempt_id.clone().or_else(|| store.attempts_of_task(tid).ok().and_then(|a| a.into_iter().find(|a| a.state == AttemptState::Passed).map(|a| a.id)));
                                if v.attempt_id.is_none() { all_built = false; }
                            }
                            TaskState::Failed | TaskState::Cancelled => failed = Some(format!("variant {} {}: {}", v.name, format!("{:?}", t.state).to_lowercase(), t.block_reason.unwrap_or_default())),
                            _ => all_built = false,
                        }
                    }
                    if let Some(why) = failed {
                        e.status = ExperimentStatus::Failed;
                        e.failure = Some(why.clone());
                        e.updated_at = now();
                        self.atomic(&store, || { store.put_experiment(&e)?; self.emit_locked(&store, Event::new("experiment.failed", "guild").with(json!({"experimentId": e.id, "reason": why}))); Ok(()) })?;
                    } else if all_built {
                        e.status = ExperimentStatus::Playtesting;
                        e.updated_at = now();
                        self.atomic(&store, || { store.put_experiment(&e)?; self.emit_locked(&store, Event::new("experiment.built", "guild").with(json!({"experimentId": e.id, "variants": e.variants.len()}))); Ok(()) })?;
                    }
                }
                ExperimentStatus::Playtesting => {
                    let mut live = self.playtesting.lock().unwrap();
                    if !live.contains(&e.id) {
                        live.push(e.id.clone());
                        drop(live);
                        self.spawn_playtests(e.id.clone());
                    }
                }
                ExperimentStatus::Analysing => {
                    let active = self.store.lock().unwrap().attempts()?.into_iter().any(|a| a.kind == AttemptKind::Analysis && a.state.is_active() && e.analysis_task_id.as_deref() == Some(a.task_id.as_str()));
                    if !active { self.start_analysis(&e)?; }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Play every variant with every profile and seed the plan names, on
    /// a thread; skips playtests already finished (after a restart).
    fn spawn_playtests(self: &Arc<Service>, experiment_id: String) {
        let service = self.clone();
        std::thread::Builder::new().name("playtests".into()).spawn(move || {
            let result = service.run_playtests(&experiment_id);
            service.playtesting.lock().unwrap().retain(|id| id != &experiment_id);
            if let Err(e) = result { service.log(&format!("experiment {experiment_id}: playtests failed: {}", e.message)); }
            service.tick();
        }).ok();
    }

    fn run_playtests(self: &Arc<Service>, experiment_id: &str) -> Res<()> {
        let (e, project, done) = {
            let store = self.store.lock().unwrap();
            let e = store.experiment(experiment_id)?.ok_or_else(|| ApiError::not_found("experiment"))?;
            let p = store.project(&e.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            (e, p, store.playtests_of_experiment(experiment_id)?)
        };
        let runner = project.playtest.clone().ok_or_else(|| ApiError::invalid("no playtest runner"))?;
        let seeds: Vec<u64> = if e.plan.seeds.is_empty() { (1..=e.plan.runs_per_profile as u64).collect() } else { e.plan.seeds.clone() };
        for v in &e.variants {
            let Some(attempt_id) = &v.attempt_id else { continue };
            let workspace = {
                let store = self.store.lock().unwrap();
                let a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
                a.workspace_id.as_ref().and_then(|id| store.workspace(id).ok().flatten()).ok_or_else(|| ApiError::not_found("workspace"))?
            };
            for profile in &e.plan.profiles {
                for seed in &seeds {
                    if done.iter().any(|p| p.variant_id.as_deref() == Some(v.id.as_str()) && &p.profile == profile && p.seed == Some(*seed) && p.status == "finished") { continue; }
                    // `ai:<member>[:<persona>]`: a guild member plays this build through the agent protocol.
                    if let Some(rest) = profile.strip_prefix("ai:") {
                        let (who, persona) = rest.split_once(':').unwrap_or((rest, "player"));
                        let member = self.store.lock().unwrap().members()?.into_iter().find(|m| m.id == who || m.name.eq_ignore_ascii_case(who));
                        let Some(member) = member else { self.log(&format!("experiment {}: no member {who} for profile {profile}", e.id)); continue };
                        let pt = Playtest { id: new_id(), project_id: e.project_id.clone(), experiment_id: Some(e.id.clone()), variant_id: Some(v.id.clone()), attempt_id: Some(attempt_id.clone()), kind: PlaytestKind::AiPlayer, profile: profile.clone(), seed: Some(*seed), status: "running".into(), player: Some(member.name.clone()), build_sha: workspaces::rev_parse(Path::new(&workspace.path), "HEAD").ok(), created_at: now(), ..Default::default() };
                        {
                            let store = self.store.lock().unwrap();
                            self.atomic(&store, || { store.put_playtest(&pt)?; self.emit_locked(&store, Event::new("playtest.started", "guild").with(json!({"playtestId": pt.id, "experimentId": e.id, "variant": v.id, "profile": profile, "seed": seed, "kind": "ai_player"}))); Ok(()) })?;
                        }
                        if let Err(err) = self.run_ai_playtest(pt, &member, persona, PathBuf::from(&workspace.path), 60) { self.log(&format!("experiment {}: AI playtest failed: {}", e.id, err.message)); }
                        continue;
                    }
                    let mut pt = Playtest { id: new_id(), project_id: e.project_id.clone(), experiment_id: Some(e.id.clone()), variant_id: Some(v.id.clone()), attempt_id: Some(attempt_id.clone()), kind: PlaytestKind::Simulation, profile: profile.clone(), seed: Some(*seed), status: "running".into(), created_at: now(), ..Default::default() };
                    {
                        let store = self.store.lock().unwrap();
                        self.atomic(&store, || { store.put_playtest(&pt)?; self.emit_locked(&store, Event::new("playtest.started", "guild").with(json!({"playtestId": pt.id, "experimentId": e.id, "variant": v.id, "profile": profile, "seed": seed}))); Ok(()) })?;
                    }
                    let argv: Vec<String> = runner.argv.iter().map(|a| a.replace("{profile}", profile).replace("{seed}", &seed.to_string()).replace("{runs}", "1")).collect();
                    let started = Instant::now();
                    let outcome = workspaces::run_with_timeout(Path::new(&workspace.path), &argv, runner.timeout_seconds);
                    pt.duration_ms = Some(started.elapsed().as_millis() as i64);
                    match outcome {
                        Ok((0, out)) => {
                            let events = crate::experiment::parse_telemetry(&out);
                            let artifact = self.write_artifact(Some(attempt_id), "playtest.events", out.as_bytes())?;
                            pt.events_artifact_id = Some(artifact.id);
                            pt.event_count = events.len() as u32;
                            pt.metrics = crate::experiment::derive_metrics(&events);
                            pt.status = if events.is_empty() { "failed".into() } else { "finished".into() };
                            if events.is_empty() { pt.failure = Some("the runner printed no game.event lines".into()); }
                        }
                        Ok((code, out)) => { pt.status = "failed".into(); pt.failure = Some(format!("runner exited with {code}: {}", out.chars().rev().take(300).collect::<String>().chars().rev().collect::<String>())); }
                        Err(err) => { pt.status = "failed".into(); pt.failure = Some(err); }
                    }
                    let store = self.store.lock().unwrap();
                    self.atomic(&store, || { store.put_playtest(&pt)?; self.emit_locked(&store, Event::new("playtest.finished", "guild").with(json!({"playtestId": pt.id, "experimentId": e.id, "variant": v.id, "profile": profile, "seed": seed, "status": pt.status, "events": pt.event_count, "failure": pt.failure}))); Ok(()) })?;
                }
            }
        }
        let store = self.store.lock().unwrap();
        let mut e = store.experiment(experiment_id)?.ok_or_else(|| ApiError::not_found("experiment"))?;
        let finished = store.playtests_of_experiment(experiment_id)?.iter().filter(|p| p.status == "finished").count();
        if finished == 0 {
            e.status = ExperimentStatus::Failed;
            e.failure = Some("no playtest produced telemetry".into());
        } else {
            e.status = ExperimentStatus::Analysing;
        }
        e.updated_at = now();
        self.atomic(&store, || { store.put_experiment(&e)?; self.emit_locked(&store, Event::new("experiment.playtested", "guild").with(json!({"experimentId": e.id, "finished": finished, "status": e.status}))); Ok(()) })
    }

    /// Per variant, per profile: the aggregated metrics of its playtests.
    fn experiment_metrics(&self, e: &Experiment) -> Res<Value> {
        let playtests = self.store.lock().unwrap().playtests_of_experiment(&e.id)?;
        let mut out = serde_json::Map::new();
        for v in &e.variants {
            let mut per_profile = serde_json::Map::new();
            for profile in &e.plan.profiles {
                let ms: Vec<Value> = playtests.iter().filter(|p| p.variant_id.as_deref() == Some(v.id.as_str()) && &p.profile == profile && p.status == "finished").map(|p| p.metrics.clone()).collect();
                per_profile.insert(profile.clone(), crate::experiment::aggregate(&ms));
            }
            // Humans who played this variant from the laboratory.
            let humans: Vec<Value> = playtests.iter().filter(|p| p.variant_id.as_deref() == Some(v.id.as_str()) && p.kind == PlaytestKind::Human && p.status == "finished").map(|p| p.metrics.clone()).collect();
            if !humans.is_empty() { per_profile.insert("human".into(), crate::experiment::aggregate(&humans)); }
            out.insert(v.id.clone(), json!({"name": v.name, "params": v.params, "profiles": per_profile}));
        }
        Ok(Value::Object(out))
    }

    fn start_analysis(self: &Arc<Service>, e: &Experiment) -> Res<()> {
        let Some(task_id) = &e.analysis_task_id else { return Ok(()) };
        let attempt = {
            let store = self.store.lock().unwrap();
            let task = store.task(task_id)?.ok_or_else(|| ApiError::not_found("analysis task"))?;
            let members = store.members()?;
            let available = self.available_harnesses();
            let busy: Vec<String> = store.active_attempts()?.into_iter().map(|a| a.member_id).collect();
            let mut wanted = task.clone();
            wanted.member_id = None;
            let mut pick = |role: &str| { wanted.role = role.into(); scheduler::pick_member(&wanted, &members, &busy, &available).cloned() };
            let Some(member) = pick("design-analyst").or_else(|| pick("judge")).or_else(|| pick("reviewer")).or_else(|| pick("implementer")) else {
                self.log(&format!("experiment {}: no member free to analyse; waiting", e.id));
                return Ok(());
            };
            let quest = store.quest(&task.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
            let attempt = Attempt {
                id: new_id(), task_id: task.id.clone(), task_version: task.version, quest_id: quest.id.clone(), project_id: quest.project_id.clone(), member_id: member.id.clone(),
                harness: member.harness.clone(), model_requested: member.model_id.clone(), model_effective: None, kind: AttemptKind::Analysis, state: AttemptState::Queued, workspace_id: None, session_id: None,
                generation: 0, candidate: None, partial: None, summary: String::new(), action: None, pending_permission: None, failure: None, verifier_results: vec![], turns: 0, resumed: 0, deciding: false, escalation: None, worked_seconds: 0, limit_seconds: 0,
                started_at: Some(now()), finished_at: None, created_at: now(), updated_at: now(),
            };
            self.atomic(&store, || {
                store.put_attempt(&attempt)?;
                if task.state == TaskState::Pending { self.move_task_locked(&store, &task.id, TaskState::Ready, None)?; }
                if matches!(task.state, TaskState::Pending | TaskState::Ready) { self.move_task_locked(&store, &task.id, TaskState::Running, None)?; }
                self.emit_locked(&store, Event::new("analysis.started", "guild").about_attempt(&attempt).with(json!({"experimentId": e.id, "member": member.name, "harness": member.harness})));
                Ok(())
            })?;
            attempt
        };
        self.spawn_driver(attempt.id.clone());
        Ok(())
    }

    fn compose_analysis_prompt(&self, role_prompt: &str, task: &Task, design: Option<&crate::design::GameDesign>) -> Res<String> {
        let (e, h) = {
            let store = self.store.lock().unwrap();
            let e = store.experiments()?.into_iter().find(|e| e.analysis_task_id.as_deref() == Some(task.id.as_str())).ok_or_else(|| ApiError::not_found("the task's experiment"))?;
            let h = store.hypothesis(&e.hypothesis_id)?.ok_or_else(|| ApiError::not_found("hypothesis"))?;
            (e, h)
        };
        let metrics = self.experiment_metrics(&e)?;
        let mut s = String::new();
        s.push_str(role_prompt.trim());
        s.push_str(&format!("\n\n# Experiment: {}\n", e.title));
        s.push_str(&format!("\n## Hypothesis ({:?} level)\nProblem: {}\n", h.level, h.problem));
        if !h.evidence.trim().is_empty() { s.push_str(&format!("Evidence: {}\n", h.evidence.trim())); }
        s.push_str(&format!("Hypothesis: {}\nExpected effect: {}\nProposed change: {}\n", h.hypothesis, h.expected_effect, h.proposed_change));
        if !h.measurements.is_empty() { s.push_str(&format!("Measurements: {}\n", h.measurements.join("; "))); }
        if let Some(d) = design { s.push_str(&crate::design::prompt_context(d)); }
        s.push_str("\n## Variants\n");
        for v in &e.variants { s.push_str(&format!("- {} ({}): {}\n", v.id, v.name, serde_json::to_string(&v.params).unwrap_or_default())); }
        s.push_str(&format!("\n## Playtest plan\nprofiles: {} · runs per profile: {}\n", e.plan.profiles.join(", "), e.plan.runs_per_profile));
        s.push_str("\n## Playtest metrics\nPer variant and player profile: counts of events, means of numeric fields (as event.field), runs started/completed, and the distribution of choices (`top` is the most taken choice and `top_share` its share: a dominant strategy shows as a high top_share).\n```json\n");
        let mut text = serde_json::to_string_pretty(&metrics).unwrap_or_default();
        if cut(&mut text, 40_000) { text.push_str("\n… (truncated)"); }
        s.push_str(&text);
        s.push_str("\n```\n\nCompare the variants against the hypothesis and the intended experience and answer with the JSON block only. Use the variant ids given above in recommended_variant.\n");
        Ok(s)
    }

    fn finish_analysis(self: &Arc<Service>, attempt: &Attempt, task: &Task, outcome: TurnOutcome) -> Res<()> {
        let attempt_id = attempt.id.as_str();
        let e = self.store.lock().unwrap().experiments()?.into_iter().find(|e| e.analysis_task_id.as_deref() == Some(task.id.as_str()));
        let Some(mut e) = e else { return self.move_attempt(attempt_id, AttemptState::Failed, Some("no experiment for this analysis".into())).map(|_| ()) };
        let text: String = match &outcome {
            TurnOutcome::Completed { .. } => self.store.lock().unwrap().events_of_attempt(attempt_id, 5000)?.into_iter().filter(|ev| ev.kind == "turn.text").map(|ev| ev.payload["text"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(""),
            _ => String::new(),
        };
        let parsed = match &outcome {
            TurnOutcome::Completed { .. } => crate::experiment::parse_analysis(&text),
            TurnOutcome::Failed { reason } => Err(format!("analyst failed: {reason}")),
            TurnOutcome::Cancelled => Err("analyst cancelled".into()),
        };
        let mut result = match parsed {
            Ok(r) => r,
            Err(why) => ExperimentResult { verdict: "inconclusive".into(), evidence: vec![format!("the analyst gave no usable verdict: {why}")], ..Default::default() },
        };
        if let Some(rv) = &result.recommended_variant {
            if !e.variants.iter().any(|v| &v.id == rv) { result.unexpected.push(format!("the analyst recommended an unknown variant {rv:?}")); result.recommended_variant = None; }
        }
        result.analyst_attempt_id = Some(attempt_id.into());
        result.metrics = self.experiment_metrics(&e)?;
        result.created_at = now();
        let artifact = self.write_artifact(Some(attempt_id), "experiment.result", serde_json::to_string_pretty(&result).unwrap_or_default().as_bytes())?;
        let usable = !result.evidence.iter().any(|x| x.starts_with("the analyst gave no usable verdict"));
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            let _ = self.move_attempt_locked(&store, attempt_id, AttemptState::Submitted, None);
            if usable {
                let mut a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
                a.summary = format!("{}: {}", result.verdict, result.recommended_next.chars().take(120).collect::<String>());
                store.put_attempt(&a)?;
                self.move_attempt_locked(&store, attempt_id, AttemptState::Passed, None)?;
            } else {
                self.move_attempt_locked(&store, attempt_id, AttemptState::Failed, Some(result.evidence.join("; ")))?;
            }
            self.move_task_locked(&store, &task.id, TaskState::Completed, None)?;
            e.result = Some(result.clone());
            e.status = ExperimentStatus::Decided;
            e.updated_at = now();
            store.put_experiment(&e)?;
            if let Some(mut h) = store.hypothesis(&e.hypothesis_id)? {
                h.status = match result.verdict.as_str() { "supports" => HypothesisStatus::Supported, "rejects" => HypothesisStatus::Rejected, "not_assessed" => HypothesisStatus::NotAssessed, _ => HypothesisStatus::Inconclusive };
                h.updated_at = now();
                store.put_hypothesis(&h)?;
            }
            self.emit_locked(&store, Event::new("experiment.analysed", "guild").about_attempt(attempt).with(json!({"experimentId": e.id, "verdict": result.verdict, "recommendedVariant": result.recommended_variant, "confidence": result.confidence, "artifactId": artifact.id, "evidence": result.evidence.len()})));
            Ok(())
        })
    }

    /// The human's word: accept a variant (its candidate is integrated),
    /// reject the experiment, or iterate (a new draft from the analyst's
    /// recommendation).
    fn experiment_decide(self: &Arc<Service>, params: &Value, action: &str) -> ApiResult {
        let id = p_str(params, "experimentId")?;
        let note = p_opt(params, "note").unwrap_or_default();
        let mut e = self.store.lock().unwrap().experiment(id)?.ok_or_else(|| ApiError::not_found(format!("experiment {id}")))?;
        let settled = matches!(e.status, ExperimentStatus::Decided | ExperimentStatus::Failed | ExperimentStatus::Accepted | ExperimentStatus::Rejected);
        if action == "accept" && !matches!(e.status, ExperimentStatus::Decided | ExperimentStatus::Failed) { return Err(ApiError::conflict(format!("experiment is {:?}; wait for its analysis", e.status))); }
        if action == "iterate" && !settled { return Err(ApiError::conflict(format!("experiment is {:?}; wait for its analysis", e.status))); }
        let mut integrated: Option<Value> = None;
        let variant_id = match action {
            "accept" => {
                let vid = p_opt(params, "variantId").or_else(|| e.result.as_ref().and_then(|r| r.recommended_variant.clone())).ok_or_else(|| ApiError::invalid("say which variant to accept (variantId)"))?;
                let v = e.variants.iter().find(|v| v.id == vid).ok_or_else(|| ApiError::not_found(format!("variant {vid}")))?.clone();
                if let Some(tid) = &v.task_id {
                    let task = self.store.lock().unwrap().task(tid)?;
                    if task.as_ref().map(|t| t.state == TaskState::Accepted).unwrap_or(false) {
                        integrated = Some(self.task_integrate(tid)?);
                    }
                }
                // The decision goes on record, with its evidence, and answers the question the hypothesis came from.
                let (hypothesis, playtests) = {
                    let store = self.store.lock().unwrap();
                    (store.hypothesis(&e.hypothesis_id)?, store.playtests_of_experiment(&e.id)?)
                };
                let finished = playtests.iter().filter(|p| p.status == "finished").count();
                let human = playtests.iter().filter(|p| p.status == "finished" && p.kind == PlaytestKind::Human).count();
                let mut evidence = vec![format!("{} synthetic playtests over {} variants", finished - human, e.variants.len())];
                if human > 0 { evidence.push(format!("{human} human playtests")); }
                let notes: Vec<String> = self.store.lock().unwrap().observations_of(&e.project_id)?.into_iter().filter(|o| o.experiment_id.as_deref() == Some(e.id.as_str())).map(|o| format!("{} [{}] {}", o.id, o.kind, o.text)).collect();
                evidence.extend(notes);
                if let Some(r) = &e.result { evidence.extend(r.evidence.iter().cloned()); }
                let rejected: Vec<String> = e.variants.iter().filter(|x| x.id != vid).map(|x| format!("{} ({})", x.name, serde_json::to_string(&x.params).unwrap_or_default())).collect();
                let decision_text = match hypothesis.as_ref() { Some(h) => format!("Adopt variant \"{}\" ({}): {}", v.name, serde_json::to_string(&v.params).unwrap_or_default(), h.proposed_change), None => format!("Adopt variant \"{}\"", v.name) };
                let why = match (&note, e.result.as_ref()) { (n, _) if !n.trim().is_empty() => n.clone(), (_, Some(r)) => format!("The analyst's verdict was \"{}\": {}", r.verdict, r.recommended_next), _ => "Chosen by the human.".into() };
                let decided = self.decision_create(&json!({"projectId": e.project_id, "decision": decision_text, "why": why, "questionIds": hypothesis.as_ref().and_then(|h| h.question_id.clone()).into_iter().collect::<Vec<_>>(),
                    "hypothesisIds": [e.hypothesis_id], "experimentIds": [e.id], "playtestIds": playtests.iter().filter(|p| p.status == "finished").map(|p| p.id.clone()).collect::<Vec<_>>(),
                    "evidence": evidence, "rejectedAlternatives": rejected, "systemIds": hypothesis.as_ref().map(|h| h.related_systems.clone()).unwrap_or_default(), "decidedBy": "human"}))?;
                integrated = Some(json!({"integrated": integrated.as_ref().and_then(|i| i.get("integrated")).cloned().unwrap_or(Value::Bool(false)), "decision": decided["id"]}));
                Some(vid)
            }
            "iterate" => {
                let mut next = e.clone();
                next.id = new_id();
                next.title = format!("{} (iteration)", e.title.trim_end_matches(" (iteration)"));
                next.status = ExperimentStatus::Draft;
                next.quest_id = None;
                next.analysis_task_id = None;
                next.result = None;
                next.decision = None;
                next.failure = None;
                for v in next.variants.iter_mut() { v.task_id = None; v.attempt_id = None; }
                next.created_at = now();
                next.updated_at = now();
                let store = self.store.lock().unwrap();
                self.atomic(&store, || { store.put_experiment(&next)?; self.emit_locked(&store, Event::new("experiment.created", "user").with(json!({"projectId": next.project_id, "experimentId": next.id, "hypothesisId": next.hypothesis_id, "variants": next.variants.len(), "title": next.title, "iterates": e.id}))); Ok(()) })?;
                integrated = Some(json!({"next": next.id}));
                None
            }
            _ => None,
        };
        e.decision = Some(ExperimentDecision { action: action.into(), variant_id: variant_id.clone(), note: note.clone(), at: now() });
        e.status = match action { "accept" => ExperimentStatus::Accepted, "reject" => ExperimentStatus::Rejected, _ => e.status };
        e.updated_at = now();
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            store.put_experiment(&e)?;
            self.emit_locked(&store, Event::new("experiment.decided", "user").with(json!({"experimentId": e.id, "action": action, "variant": variant_id, "note": note})));
            Ok(())
        })?;
        Ok(json!({"experiment": e, "integrated": integrated}))
    }

    // ----- playing the game -----

    /// Open a human play session: a detached checkout of the build (the
    /// project's target, a ref, or an experiment variant's candidate), a
    /// telemetry file, and the command for a terminal to run. The client
    /// runs it; `play.finish` reads what the game wrote.
    fn play_start(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let (project, design_version) = {
            let store = self.store.lock().unwrap();
            let p = store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
            let v = store.design(project_id)?.map(|d| d.version);
            (p, v)
        };
        let runner = project.play.clone().ok_or_else(|| ApiError::invalid("the project has no [play] command in .guild/guild.toml"))?;
        let id = new_id();
        let experiment_id = p_opt(params, "experimentId");
        let variant_id = p_opt(params, "variantId");
        let (cwd, sha, attempt_id, play_dir) = self.build_to_play(&project, &id, params)?;
        let telemetry = crate::paths::artifacts_dir().join("play").join(format!("{id}.jsonl"));
        std::fs::create_dir_all(telemetry.parent().unwrap()).map_err(|e| ApiError::internal(e.to_string()))?;
        std::fs::write(&telemetry, b"").map_err(|e| ApiError::internal(e.to_string()))?;
        let seed = params.get("seed").and_then(Value::as_u64).unwrap_or_else(|| (chrono::Utc::now().timestamp_millis() % 10_000) as u64);
        let argv: Vec<String> = runner.argv.iter().map(|a| a.replace("{profile}", "human").replace("{seed}", &seed.to_string()).replace("{runs}", "1")).collect();
        let pt = Playtest {
            id: id.clone(), project_id: project_id.into(), experiment_id, variant_id, attempt_id, kind: PlaytestKind::Human, profile: "human".into(), seed: Some(seed),
            status: "running".into(), player: p_opt(params, "player"), build_sha: sha, design_version, telemetry_path: Some(telemetry.display().to_string()), play_dir, created_at: now(), ..Default::default()
        };
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            store.put_playtest(&pt)?;
            self.emit_locked(&store, Event::new("playtest.started", "user").with(json!({"playtestId": pt.id, "projectId": project_id, "kind": "human", "experimentId": pt.experiment_id, "variant": pt.variant_id, "buildSha": pt.build_sha})));
            Ok(())
        })?;
        Ok(json!({"playtest": pt, "cwd": cwd, "argv": argv, "env": {"GUILD_TELEMETRY": telemetry.display().to_string(), "GUILD_PLAYTEST": id}}))
    }

    /// Run the project's setup in a checkout made to play or replay (what
    /// an attempt's worktree gets too: dependencies, generated data); a
    /// checkout whose setup fails is removed.
    fn prepare_checkout(&self, project: &Project, dir: &Path) -> Res<()> {
        if project.setup.is_empty() { return Ok(()); }
        match workspaces::run_with_timeout(dir, &project.setup, self.settings.setup_timeout_seconds) {
            Ok((0, _)) => Ok(()),
            Ok((code, out)) => { workspaces::remove_checkout(project, dir); Err(ApiError::internal(format!("setup exited with {code}: {}", out.chars().rev().take(600).collect::<String>().chars().rev().collect::<String>()))) }
            Err(e) => { workspaces::remove_checkout(project, dir); Err(ApiError::internal(format!("setup failed: {e}"))) }
        }
    }

    /// The build a session plays: an experiment variant's worktree, or a
    /// detached checkout (made for the session `id`) of a ref, by default
    /// the project's target. Returns the directory, its commit, the
    /// variant's attempt and the checkout to remove afterwards.
    fn build_to_play(&self, project: &Project, id: &str, params: &Value) -> Res<(String, Option<String>, Option<String>, Option<String>)> {
        let experiment_id = p_opt(params, "experimentId");
        let variant_id = p_opt(params, "variantId");
        Ok(match (&experiment_id, &variant_id) {
            (Some(eid), Some(vid)) => {
                let store = self.store.lock().unwrap();
                let e = store.experiment(eid)?.ok_or_else(|| ApiError::not_found(format!("experiment {eid}")))?;
                let v = e.variants.iter().find(|v| &v.id == vid).ok_or_else(|| ApiError::not_found(format!("variant {vid}")))?;
                let aid = v.attempt_id.clone().ok_or_else(|| ApiError::conflict(format!("variant {vid} is not built yet")))?;
                let a = store.attempt(&aid)?.ok_or_else(|| ApiError::not_found("attempt"))?;
                let w = a.workspace_id.as_ref().and_then(|w| store.workspace(w).ok().flatten()).ok_or_else(|| ApiError::not_found("the variant's worktree"))?;
                if !Path::new(&w.path).exists() { return Err(ApiError::conflict("the variant's worktree is gone")); }
                let sha = a.candidate.as_ref().map(|c| c.head_sha.clone()).or_else(|| workspaces::rev_parse(Path::new(&w.path), "HEAD").ok());
                (w.path.clone(), sha, Some(aid), None)
            }
            _ => {
                let rev = p_opt(params, "ref").unwrap_or_else(|| project.target_ref.clone());
                let sha = workspaces::rev_parse(Path::new(&project.repo_path), &rev).map_err(ApiError::invalid)?;
                let dir = crate::paths::worktrees_dir().join(&project.id).join(format!("play-{}", &id[id.len() - 8..]));
                workspaces::checkout_detached(project, &sha, &dir).map_err(ApiError::internal)?;
                self.prepare_checkout(project, &dir)?;
                (dir.display().to_string(), Some(sha), None, Some(dir.display().to_string()))
            }
        })
    }

    /// Close a human session: keep its raw events, derive its metrics,
    /// remove the checkout the guild made for it. Idempotent.
    fn play_finish(&self, id: &str) -> ApiResult {
        let (mut pt, project) = {
            let store = self.store.lock().unwrap();
            let pt = store.playtest(id)?.ok_or_else(|| ApiError::not_found(format!("playtest {id}")))?;
            let p = store.project(&pt.project_id)?;
            (pt, p)
        };
        if pt.status != "running" { return Ok(serde_json::to_value(pt).unwrap()); }
        let text = pt.telemetry_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let events = crate::experiment::parse_telemetry(&text);
        if !text.is_empty() {
            let artifact = self.write_artifact(None, "playtest.events", text.as_bytes())?;
            pt.events_artifact_id = Some(artifact.id);
        }
        pt.event_count = events.len() as u32;
        pt.metrics = crate::experiment::derive_metrics(&events);
        pt.status = if events.is_empty() { "abandoned".into() } else { "finished".into() };
        pt.finished_at = Some(now());
        pt.duration_ms = chrono::DateTime::parse_from_rfc3339(&pt.created_at).ok().map(|s| (chrono::Utc::now() - s.with_timezone(&chrono::Utc)).num_milliseconds());
        if let (Some(dir), Some(p)) = (&pt.play_dir, &project) { workspaces::remove_checkout(p, Path::new(dir)); }
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            store.put_playtest(&pt)?;
            self.emit_locked(&store, Event::new("playtest.finished", "user").with(json!({"playtestId": pt.id, "projectId": pt.project_id, "kind": "human", "status": pt.status, "events": pt.event_count, "experimentId": pt.experiment_id, "variant": pt.variant_id})));
            Ok(())
        })?;
        Ok(serde_json::to_value(pt).unwrap())
    }

    // ----- AI players and the exploit hunter -----

    /// Start an AI player on the build: a member, on its own harness,
    /// plays the game through its agent protocol, one move per turn of
    /// one session. Returns the running playtest at once; the play goes
    /// on in a thread and ends with `playtest.finished`.
    fn ai_play_start(self: &Arc<Service>, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let member_ref = p_str(params, "memberId")?;
        let persona = p_opt(params, "persona").unwrap_or_else(|| "player".into());
        let (project, member, design_version) = {
            let store = self.store.lock().unwrap();
            let p = store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
            let m = store.members()?.into_iter().find(|m| m.id == member_ref || m.name.eq_ignore_ascii_case(member_ref)).ok_or_else(|| ApiError::not_found(format!("member {member_ref}")))?;
            (p, m, store.design(project_id)?.map(|d| d.version))
        };
        if project.ai_play.is_none() { return Err(ApiError::invalid("the project has no [ai_play] command in .guild/guild.toml")); }
        self.adapter(member.harness.clone())?;
        let id = new_id();
        let (cwd, sha, attempt_id, play_dir) = self.build_to_play(&project, &id, params)?;
        let seed = params.get("seed").and_then(Value::as_u64).unwrap_or_else(|| (chrono::Utc::now().timestamp_millis() % 10_000) as u64);
        let pt = Playtest {
            id: id.clone(), project_id: project_id.into(), experiment_id: p_opt(params, "experimentId"), variant_id: p_opt(params, "variantId"), attempt_id,
            kind: PlaytestKind::AiPlayer, profile: format!("ai:{persona}"), seed: Some(seed), status: "running".into(), player: Some(member.name.clone()),
            build_sha: sha, design_version, play_dir, created_at: now(), ..Default::default()
        };
        {
            let store = self.store.lock().unwrap();
            self.atomic(&store, || {
                store.put_playtest(&pt)?;
                self.emit_locked(&store, Event::new("playtest.started", "user").with(json!({"playtestId": pt.id, "projectId": project_id, "kind": "ai_player", "memberId": member.id, "persona": persona, "seed": seed, "experimentId": pt.experiment_id, "variant": pt.variant_id})));
                Ok(())
            })?;
        }
        let max_turns = params.get("maxTurns").and_then(Value::as_u64).unwrap_or(60) as u32;
        let service = self.clone();
        let started = pt.clone();
        std::thread::Builder::new().name("ai-player".into()).spawn(move || {
            if let Err(e) = service.run_ai_playtest(started, &member, &persona, PathBuf::from(cwd), max_turns) { service.log(&format!("ai playtest failed: {}", e.message)); }
        }).ok();
        Ok(serde_json::to_value(pt).unwrap())
    }

    fn ai_play_stop(&self, id: &str) -> ApiResult {
        let pt = self.store.lock().unwrap().playtest(id)?.ok_or_else(|| ApiError::not_found(format!("playtest {id}")))?;
        if pt.kind != PlaytestKind::AiPlayer { return Err(ApiError::invalid("not an AI playtest")); }
        if pt.status == "running" { self.ai_stops.lock().unwrap().push(id.to_string()); }
        Ok(json!({"stopping": pt.status == "running"}))
    }

    /// Play one AI playtest to its end and record it: the raw events, the
    /// player's decisions, what it said afterwards, and every finding it
    /// reported. Blocks; used by `ai.play` (on a thread) and by
    /// experiments whose plan names an `ai:` profile.
    fn run_ai_playtest(self: &Arc<Service>, mut pt: Playtest, member: &Member, persona: &str, cwd: PathBuf, max_turns: u32) -> Res<Playtest> {
        let (project, design) = {
            let store = self.store.lock().unwrap();
            (store.project(&pt.project_id)?.ok_or_else(|| ApiError::not_found("project"))?, store.design(&pt.project_id)?)
        };
        let runner = project.ai_play.clone().ok_or_else(|| ApiError::invalid("no [ai_play] command"))?;
        let seed = pt.seed.unwrap_or(1);
        let telemetry = crate::paths::artifacts_dir().join("play").join(format!("{}.jsonl", pt.id));
        std::fs::create_dir_all(telemetry.parent().unwrap()).map_err(|e| ApiError::internal(e.to_string()))?;
        std::fs::write(&telemetry, b"").map_err(|e| ApiError::internal(e.to_string()))?;
        pt.telemetry_path = Some(telemetry.display().to_string());
        let argv: Vec<String> = runner.argv.iter().map(|a| a.replace("{seed}", &seed.to_string()).replace("{profile}", "agent").replace("{runs}", "1")).collect();
        let env = vec![("GUILD_TELEMETRY".to_string(), telemetry.display().to_string()), ("GUILD_PLAYTEST".to_string(), pt.id.clone())];
        let started = Instant::now();
        let adapter = self.adapter(member.harness.clone())?;
        let options = SessionOptions { cwd: cwd.clone(), model_id: member.model_id.clone(), mode: member.mode.clone(), role_prompt: config::default_role_prompt(if persona == "exploit_hunter" { "exploit-hunter" } else { "ai-player" }) };
        let session = adapter.create_session(&options).map_err(|e| ApiError::new("harness", e.to_string()));
        let session = match session {
            Ok(s) => s,
            Err(e) => return self.close_ai_playtest(pt, &project, vec![], Some(e.message), String::new(), started),
        };
        let briefing = format!("{}\n\n{}", options.role_prompt, crate::aiplay::briefing(persona, design.as_ref()));
        let mut first = true;
        let mut seen = 0usize;
        let id = pt.id.clone();
        let stop = || self.ai_stops.lock().unwrap().contains(&id) || self.stop.load(Ordering::Relaxed);
        let outcome = crate::aiplay::play_game(&cwd, &argv, &env, &telemetry, max_turns, Duration::from_secs(runner.timeout_seconds), |observation, index| {
            let actions = crate::aiplay::actions_of(observation);
            let events = crate::experiment::parse_telemetry(&std::fs::read_to_string(&telemetry).unwrap_or_default());
            let since: Vec<Value> = events.get(seen.min(events.len())..).unwrap_or(&[]).to_vec();
            seen = events.len();
            let mut prompt = crate::aiplay::turn_prompt(observation, &since);
            if first { prompt = format!("{briefing}\n{prompt}"); first = false; }
            let turn = observation["turn"].as_u64().unwrap_or(0);
            self.emit(Event::new("ai.turn", "guild").with(json!({"playtestId": pt.id, "memberId": member.id, "turn": turn, "events": index})));
            let reply = self.ask_harness(&adapter, &session, &format!("{}-{turn}", pt.id), &prompt, &stop)?;
            match crate::aiplay::parse_move(&reply, &actions) {
                Ok(m) => Ok(m),
                // One more chance, with the reason.
                Err(e) => {
                    let again = self.ask_harness(&adapter, &session, &format!("{}-{turn}b", pt.id), &format!("Your answer could not be read ({e}). Answer again with only the ```json block, choosing one of: {}.", actions.iter().map(|a| a.0.as_str()).collect::<Vec<_>>().join(", ")), &stop)?;
                    crate::aiplay::parse_move(&again, &actions)
                }
            }
        }, stop);
        // What it lived, in its own words, while the session is still open.
        let mut reflection = String::new();
        // Findings the hunter only saw once the game had ended: they replay with every move.
        let mut late_findings: Vec<Value> = vec![];
        if !outcome.turns.is_empty() && !stop() {
            let events = crate::experiment::parse_telemetry(&std::fs::read_to_string(&telemetry).unwrap_or_default());
            let last = crate::aiplay::happened(events.get(seen.min(events.len())..).unwrap_or(&[]));
            let mut ask = format!("## What happened\n\n{last}\nThe game is over. In two or three sentences: what did you feel, what tempted you, what felt pointless or too strong?");
            if persona == "exploit_hunter" {
                ask.push_str(" Then, if you found something you have not reported yet, end with one ```json block: {\"findings\": [{\"title\": \"…\", \"impact\": \"…\", \"severity\": \"low|medium|high|critical\", \"steps\": \"…\"}]}");
            } else {
                ask.push_str(" No JSON this time.");
            }
            let reply = self.ask_harness(&adapter, &session, &format!("{}-end", pt.id), &ask, &stop).unwrap_or_default();
            let (prose, found) = crate::aiplay::split_reflection(&reply);
            reflection = prose.chars().take(1200).collect();
            if persona == "exploit_hunter" { late_findings = found; }
        }
        let _ = adapter.stop(&session);
        self.ai_stops.lock().unwrap().retain(|s| s != &pt.id);
        let failure = outcome.failure.clone().filter(|_| !outcome.ended || outcome.exit_code.map(|c| c != 0).unwrap_or(false));
        let decisions: String = outcome.turns.iter().map(|t| serde_json::to_string(t).unwrap() + "\n").collect();
        if !decisions.is_empty() {
            let artifact = self.write_artifact(pt.attempt_id.as_deref(), "playtest.decisions", decisions.as_bytes())?;
            self.emit(Event::new("artifact.written", "guild").with(json!({"playtestId": pt.id, "kind": "playtest.decisions", "artifactId": artifact.id})));
        }
        // The findings, numbered, with the moves that led to each.
        let events = crate::experiment::parse_telemetry(&std::fs::read_to_string(&telemetry).unwrap_or_default());
        let last_turn = outcome.turns.len().saturating_sub(1);
        let reported: Vec<(usize, &Value)> = outcome.turns.iter().enumerate().filter_map(|(i, t)| t.finding.as_ref().map(|f| (i, f))).chain(late_findings.iter().map(|f| (last_turn, f))).collect();
        for (i, raw) in reported {
            let turn = &outcome.turns[i];
            let store = self.store.lock().unwrap();
            let mut f = crate::aiplay::finding_from(raw);
            f.id = crate::design::next_id("F", store.findings_of(&pt.project_id)?.into_iter().map(|f| f.id));
            f.project_id = pt.project_id.clone();
            f.playtest_id = Some(pt.id.clone());
            f.member_id = Some(member.id.clone());
            f.build_sha = pt.build_sha.clone();
            f.design_version = pt.design_version;
            f.experiment_id = pt.experiment_id.clone();
            f.variant_id = pt.variant_id.clone();
            f.seed = Some(seed);
            f.actions = outcome.turns[..=i].iter().map(|t| t.action.clone()).collect();
            // The events up to the move after the one it reported on: what the move did.
            f.event_index = outcome.turns.get(i + 1).map(|t| t.event_index).unwrap_or(events.len() as u32);
            // The run and round it was reported in: where the game was when the move was asked for.
            let upto = &events[..(turn.event_index as usize).min(events.len())];
            f.run_id = upto.iter().rev().find_map(|e| e.get("runId").and_then(Value::as_str)).map(str::to_string);
            f.round = upto.iter().rev().find(|e| e["event"] == "round.started").and_then(|e| e["data"]["round"].as_u64());
            f.created_at = now();
            self.atomic(&store, || {
                store.put_finding(&f)?;
                self.emit_locked(&store, Event::new("finding.created", "guild").with(json!({"projectId": f.project_id, "findingId": f.id, "title": f.title, "severity": f.severity, "playtestId": pt.id})));
                Ok(())
            })?;
        }
        self.close_ai_playtest(pt, &project, outcome.turns, failure, reflection, started)
    }

    fn close_ai_playtest(&self, mut pt: Playtest, project: &Project, turns: Vec<crate::aiplay::TurnRecord>, failure: Option<String>, reflection: String, started: Instant) -> Res<Playtest> {
        let text = pt.telemetry_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let events = crate::experiment::parse_telemetry(&text);
        if !text.is_empty() { pt.events_artifact_id = Some(self.write_artifact(pt.attempt_id.as_deref(), "playtest.events", text.as_bytes())?.id); }
        pt.event_count = events.len() as u32;
        pt.metrics = crate::experiment::derive_metrics(&events);
        if let Some(m) = pt.metrics.as_object_mut() {
            m.insert("ai".into(), json!({"turns": turns.len(), "fallbacks": turns.iter().filter(|t| t.fallback).count(), "findings": turns.iter().filter(|t| t.finding.is_some()).count()}));
        }
        pt.observations = reflection;
        // A game cut short keeps its events but is no complete playtest.
        pt.status = if events.is_empty() { "failed".into() } else if failure.is_some() { "stopped".into() } else { "finished".into() };
        pt.failure = failure.or_else(|| events.is_empty().then(|| "the game wrote no game.event lines".to_string()));
        pt.duration_ms = Some(started.elapsed().as_millis() as i64);
        pt.finished_at = Some(now());
        if let Some(dir) = &pt.play_dir { workspaces::remove_checkout(project, Path::new(dir)); }
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            store.put_playtest(&pt)?;
            self.emit_locked(&store, Event::new("playtest.finished", "guild").with(json!({"playtestId": pt.id, "projectId": pt.project_id, "kind": "ai_player", "status": pt.status, "events": pt.event_count, "turns": turns.len(), "failure": pt.failure, "experimentId": pt.experiment_id, "variant": pt.variant_id})));
            Ok(())
        })?;
        Ok(pt)
    }

    /// One prompt on a live session, answered: the reply's text. Tools the
    /// player tries are declined (it only chooses moves).
    fn ask_harness(&self, adapter: &Arc<dyn HarnessAdapter>, session: &SessionRef, request_id: &str, prompt: &str, stop: &dyn Fn() -> bool) -> Result<String, String> {
        let handle = adapter.prompt(session, request_id, prompt).map_err(|e| e.to_string())?;
        let mut text = String::new();
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            while let Ok(ev) = handle.events.try_recv() {
                match ev {
                    HarnessEvent::Text(t) => text.push_str(&t),
                    HarnessEvent::PermissionRequested { request_id, .. } => { let _ = adapter.respond_permission(session, &request_id, None); }
                    _ => {}
                }
            }
            match handle.result.recv_timeout(Duration::from_millis(100)) {
                Ok(TurnOutcome::Completed { .. }) => {
                    while let Ok(ev) = handle.events.try_recv() { if let HarnessEvent::Text(t) = ev { text.push_str(&t); } }
                    return Ok(text);
                }
                Ok(TurnOutcome::Failed { reason }) => return Err(reason),
                Ok(TurnOutcome::Cancelled) => return Err("stopped".into()),
                Err(RecvTimeoutError::Disconnected) => return Err("the harness ended without a result".into()),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if stop() || Instant::now() > deadline {
                let _ = adapter.interrupt(session);
                return Err(if stop() { "stopped".into() } else { "the player took more than 5 minutes".into() });
            }
        }
    }

    /// Replay a finding on the build it was found on: same seed, the same
    /// moves, no agent. Reproduced when the game's first events are the
    /// ones the hunter saw.
    fn finding_verify(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let fid = p_str(params, "findingId")?;
        let (mut f, project, original) = {
            let store = self.store.lock().unwrap();
            let f = store.finding(project_id, fid)?.ok_or_else(|| ApiError::not_found(format!("finding {fid}")))?;
            let p = store.project(project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            let pt = f.playtest_id.as_ref().and_then(|id| store.playtest(id).ok().flatten());
            let text = pt.and_then(|pt| pt.events_artifact_id).and_then(|a| store.artifact(&a).ok().flatten()).and_then(|a| std::fs::read_to_string(a.path).ok()).unwrap_or_default();
            (f, p, crate::experiment::parse_telemetry(&text))
        };
        let runner = project.ai_play.clone().ok_or_else(|| ApiError::invalid("the project has no [ai_play] command"))?;
        // The build: the one it was found on, or another ref to see whether it still holds.
        let id = new_id();
        let rev = p_opt(params, "ref").or(f.build_sha.clone()).unwrap_or_else(|| project.target_ref.clone());
        let sha = workspaces::rev_parse(Path::new(&project.repo_path), &rev).map_err(ApiError::invalid)?;
        let dir = crate::paths::worktrees_dir().join(&project.id).join(format!("replay-{}", &id[id.len() - 8..]));
        workspaces::checkout_detached(&project, &sha, &dir).map_err(ApiError::internal)?;
        self.prepare_checkout(&project, &dir)?;
        let telemetry = crate::paths::artifacts_dir().join("play").join(format!("replay-{id}.jsonl"));
        let _ = std::fs::create_dir_all(telemetry.parent().unwrap());
        let _ = std::fs::write(&telemetry, b"");
        let argv: Vec<String> = runner.argv.iter().map(|a| a.replace("{seed}", &f.seed.unwrap_or(1).to_string()).replace("{profile}", "agent").replace("{runs}", "1")).collect();
        let env = vec![("GUILD_TELEMETRY".to_string(), telemetry.display().to_string())];
        let mut moves = f.actions.clone().into_iter();
        let outcome = crate::aiplay::play_game(&dir, &argv, &env, &telemetry, f.actions.len() as u32, Duration::from_secs(runner.timeout_seconds), |obs, _| {
            let action = moves.next().ok_or("no more moves")?;
            if !crate::aiplay::actions_of(obs).iter().any(|a| a.0 == action) { return Err(format!("move {action} is not offered any more")); }
            Ok(crate::aiplay::Move { action, ..Default::default() })
        }, || false);
        let replay = crate::experiment::parse_telemetry(&std::fs::read_to_string(&telemetry).unwrap_or_default());
        workspaces::remove_checkout(&project, &dir);
        let _ = std::fs::remove_file(&telemetry);
        let upto = (f.event_index as usize).min(original.len());
        let same = crate::aiplay::same_prefix(&original, &replay, upto) && !outcome.turns.iter().any(|t| t.fallback);
        let on_other_build = Some(&sha) != f.build_sha.as_ref();
        f.status = match (same, on_other_build) { (true, _) => "reproduced", (false, true) => "fixed", (false, false) => "not_reproduced" }.into();
        f.verification = Some(if same {
            format!("Replayed {} moves with seed {} on {}: the first {upto} events match.", f.actions.len(), f.seed.unwrap_or(1), &sha[..8])
        } else {
            let at = original.iter().zip(&replay).position(|(a, b)| a != b).unwrap_or(original.len().min(replay.len()));
            format!("Replayed {} moves with seed {} on {}: the game went another way at event {at}{}.", f.actions.len(), f.seed.unwrap_or(1), &sha[..8], outcome.turns.iter().find(|t| t.fallback).map(|t| format!(" ({})", t.why)).unwrap_or_default())
        });
        f.verified_at = Some(now());
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            store.put_finding(&f)?;
            self.emit_locked(&store, Event::new("finding.verified", "guild").with(json!({"projectId": f.project_id, "findingId": f.id, "status": f.status, "buildSha": sha})));
            Ok(())
        })?;
        Ok(serde_json::to_value(f).unwrap())
    }

    /// Turn a finding into a design question that points back to it.
    fn finding_promote(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let fid = p_str(params, "findingId")?;
        let f = self.store.lock().unwrap().finding(project_id, fid)?.ok_or_else(|| ApiError::not_found(format!("finding {fid}")))?;
        if let Some(q) = &f.question_id { return Err(ApiError::conflict(format!("{fid} already raised {q}"))); }
        let mut context = format!("{} [{} · {}]: {}", f.id, f.severity, f.status, f.title);
        if !f.impact.is_empty() { context.push_str(&format!(" · impact: {}", f.impact)); }
        if let Some(sha) = &f.build_sha { context.push_str(&format!(" · build {}", &sha[..sha.len().min(8)])); }
        context.push_str(&format!(" · replay: seed {} with moves {}", f.seed.unwrap_or(1), f.actions.join(",")));
        let question = p_opt(params, "question").unwrap_or_else(|| format!("How do we stop \"{}\"?", f.title));
        let q = self.question_create(&json!({"projectId": project_id, "question": question, "context": context, "systemId": p_opt(params, "systemId"), "raisedBy": "exploit-hunter", "source": f.id}))?;
        let store = self.store.lock().unwrap();
        let mut f = f;
        f.question_id = q["id"].as_str().map(str::to_string);
        self.atomic(&store, || { store.put_finding(&f)?; Ok(()) })?;
        Ok(json!({"finding": f, "question": q}))
    }

    fn finding_set(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let fid = p_str(params, "findingId")?;
        let status = p_str(params, "status")?;
        if !crate::aiplay::FINDING_STATUSES.contains(&status) { return Err(ApiError::invalid(format!("status must be one of {}", crate::aiplay::FINDING_STATUSES.join(", ")))); }
        let store = self.store.lock().unwrap();
        let mut f = store.finding(project_id, fid)?.ok_or_else(|| ApiError::not_found(format!("finding {fid}")))?;
        f.status = status.into();
        self.atomic(&store, || {
            store.put_finding(&f)?;
            self.emit_locked(&store, Event::new("finding.updated", "user").with(json!({"projectId": f.project_id, "findingId": f.id, "status": f.status})));
            Ok(())
        })?;
        Ok(serde_json::to_value(f).unwrap())
    }

    /// Note what helped or got in the way while orchestrating: one
    /// observation with its evidence and its cause (see `feedback`).
    fn feedback_create(&self, params: &Value) -> ApiResult {
        use crate::feedback::{Feedback, FEEDBACK_CAUSES, FEEDBACK_KINDS};
        let kind = p_str(params, "kind")?;
        if !FEEDBACK_KINDS.contains(&kind) { return Err(ApiError::invalid(format!("kind must be one of {}", FEEDBACK_KINDS.join(", ")))); }
        let cause = p_str(params, "cause")?;
        if !FEEDBACK_CAUSES.contains(&cause) { return Err(ApiError::invalid(format!("cause must be one of {}", FEEDBACK_CAUSES.join(", ")))); }
        let title = p_str(params, "title")?.trim();
        let evidence = p_str(params, "evidence")?.trim();
        if title.is_empty() || evidence.is_empty() { return Err(ApiError::invalid("title and evidence must not be empty")); }
        let project_id = p_opt(params, "projectId").filter(|p| !p.is_empty());
        let refs: Vec<String> = params.get("refs").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let store = self.store.lock().unwrap();
        if let Some(pid) = &project_id { store.project(pid)?.ok_or_else(|| ApiError::not_found(format!("project {pid}")))?; }
        let f = Feedback {
            id: crate::design::next_id("FB", store.feedback_all()?.into_iter().map(|f| f.id)),
            project_id, kind: kind.into(), cause: cause.into(), title: title.into(), evidence: evidence.into(),
            cost: p_opt(params, "cost").filter(|s| !s.trim().is_empty()),
            proposal: p_opt(params, "proposal").filter(|s| !s.trim().is_empty()),
            refs, status: "open".into(), notes: vec![], commit: None,
            author: p_opt(params, "author").unwrap_or_else(|| "orchestrator".into()),
            created_at: now(), updated_at: None,
        };
        self.atomic(&store, || {
            store.put_feedback(&f)?;
            self.emit_locked(&store, Event::new("feedback.created", "user").with(json!({"feedbackId": f.id, "projectId": f.project_id, "kind": f.kind, "cause": f.cause})));
            Ok(())
        })?;
        Ok(serde_json::to_value(f).unwrap())
    }

    /// The entries, filtered by status, cause, kind or project; as JSON, or as
    /// Markdown (`format: "markdown"`) for an agent's prompt.
    fn feedback_list(&self, params: &Value) -> ApiResult {
        let mut items = self.store.lock().unwrap().feedback_all()?;
        for (key, field) in [("status", 0), ("cause", 1), ("kind", 2), ("projectId", 3)] {
            if let Some(want) = p_opt(params, key) {
                items.retain(|f| match field { 0 => f.status == want, 1 => f.cause == want, 2 => f.kind == want, _ => f.project_id.as_deref() == Some(want.as_str()) });
            }
        }
        if p_opt(params, "format").as_deref() == Some("markdown") {
            return Ok(json!({"markdown": crate::feedback::to_markdown(&items), "count": items.len()}));
        }
        Ok(serde_json::to_value(items).unwrap())
    }

    /// Move an entry along (open → proposed → applied | discarded), with a
    /// note, a proposal or the commit that applied it.
    fn feedback_set(&self, params: &Value) -> ApiResult {
        use crate::feedback::FEEDBACK_STATUSES;
        let fid = p_str(params, "feedbackId")?;
        let store = self.store.lock().unwrap();
        let mut f = store.feedback(fid)?.ok_or_else(|| ApiError::not_found(format!("feedback {fid}")))?;
        if let Some(status) = p_opt(params, "status") {
            if !FEEDBACK_STATUSES.contains(&status.as_str()) { return Err(ApiError::invalid(format!("status must be one of {}", FEEDBACK_STATUSES.join(", ")))); }
            if status == "applied" && p_opt(params, "commit").is_none() && f.commit.is_none() { return Err(ApiError::invalid("applied needs the commit that applied it")); }
            if status == "discarded" && p_opt(params, "note").is_none() { return Err(ApiError::invalid("discarded needs a note saying why")); }
            f.status = status;
        }
        if let Some(p) = p_opt(params, "proposal").filter(|s| !s.trim().is_empty()) { f.proposal = Some(p); }
        if let Some(c) = p_opt(params, "commit").filter(|s| !s.trim().is_empty()) { f.commit = Some(c); }
        if let Some(n) = p_opt(params, "note").filter(|s| !s.trim().is_empty()) { f.notes.push(n); }
        f.updated_at = Some(now());
        self.atomic(&store, || {
            store.put_feedback(&f)?;
            self.emit_locked(&store, Event::new("feedback.updated", "user").with(json!({"feedbackId": f.id, "status": f.status})));
            Ok(())
        })?;
        Ok(serde_json::to_value(f).unwrap())
    }

    /// Note what a human noticed. Inside a session it is tied to the
    /// build, the design version, the experiment's variant, and the run,
    /// round and moment the game's telemetry was at.
    fn observation_create(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let kind = p_opt(params, "kind").unwrap_or_else(|| "comment".into());
        if !OBSERVATION_KINDS.contains(&kind.as_str()) { return Err(ApiError::invalid(format!("kind must be one of {}", OBSERVATION_KINDS.join(", ")))); }
        let text = p_opt(params, "text").unwrap_or_default();
        let store = self.store.lock().unwrap();
        store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
        // The session: the one named, or the human session running now.
        let session = match p_opt(params, "playtestId") {
            Some(pid) => Some(store.playtest(&pid)?.ok_or_else(|| ApiError::not_found(format!("playtest {pid}")))?),
            None => store.playtests()?.into_iter().filter(|p| p.project_id == project_id && p.kind == PlaytestKind::Human && p.status == "running").max_by(|a, b| a.created_at.cmp(&b.created_at)),
        };
        let mut o = Observation { id: crate::design::next_id("OBS", store.observations_of(project_id)?.into_iter().map(|o| o.id)), project_id: project_id.into(), kind, text, created_at: now(), ..Default::default() };
        if let Some(pt) = &session {
            o.playtest_id = Some(pt.id.clone());
            o.build_sha = pt.build_sha.clone();
            o.design_version = pt.design_version;
            o.experiment_id = pt.experiment_id.clone();
            o.variant_id = pt.variant_id.clone();
            let events = pt.telemetry_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| crate::experiment::parse_telemetry(&t)).unwrap_or_default();
            o.run_id = events.iter().rev().find_map(|e| e.get("runId").and_then(Value::as_str)).map(str::to_string);
            o.round = events.iter().rev().find(|e| e["event"] == "round.started").and_then(|e| e["data"]["round"].as_u64());
            o.event_index = Some(events.len() as u32);
        }
        self.atomic(&store, || {
            store.put_observation(&o)?;
            self.emit_locked(&store, Event::new("observation.created", "user").with(json!({"projectId": project_id, "observationId": o.id, "kind": o.kind, "playtestId": o.playtest_id, "round": o.round})));
            Ok(())
        })?;
        Ok(serde_json::to_value(o).unwrap())
    }

    /// Turn an observation into a design question that points back to it.
    fn observation_promote(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let oid = p_str(params, "observationId")?;
        let o = self.store.lock().unwrap().observation(project_id, oid)?.ok_or_else(|| ApiError::not_found(format!("observation {oid}")))?;
        if let Some(q) = &o.question_id { return Err(ApiError::conflict(format!("{oid} already raised {q}"))); }
        let mut context = format!("{} [{}]: {}", o.id, o.kind, o.text);
        if let Some(sha) = &o.build_sha { context.push_str(&format!(" · build {}", &sha[..sha.len().min(8)])); }
        if let Some(v) = o.design_version { context.push_str(&format!(" · design v{v}")); }
        if let (Some(e), Some(v)) = (&o.experiment_id, &o.variant_id) { context.push_str(&format!(" · experiment {} variant {v}", &e[e.len().saturating_sub(8)..])); }
        if let Some(r) = &o.run_id { context.push_str(&format!(" · run {r}")); }
        if let Some(r) = o.round { context.push_str(&format!(" · round {r}")); }
        let question = p_opt(params, "question").unwrap_or_else(|| if o.text.trim().is_empty() { format!("What made this {}?", o.kind.replace('_', " ")) } else { o.text.trim().to_string() });
        let q = self.question_create(&json!({"projectId": project_id, "question": question, "context": context, "systemId": p_opt(params, "systemId"), "raisedBy": "playtest", "source": o.id}))?;
        let store = self.store.lock().unwrap();
        let mut o = o;
        o.question_id = q["id"].as_str().map(str::to_string);
        self.atomic(&store, || { store.put_observation(&o)?; Ok(()) })?;
        Ok(json!({"observation": o, "question": q}))
    }

    /// What the finished playtests' raw events show (see
    /// `experiment::signals`), next to the experience the design wants:
    /// the comparison is laid out for the human; nothing is concluded.
    fn signals_of(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let experiment = p_opt(params, "experimentId");
        let variant = p_opt(params, "variantId");
        let kind = p_opt(params, "kind");
        let store = self.store.lock().unwrap();
        let chosen: Vec<Playtest> = store.playtests()?.into_iter().filter(|p| {
            p.project_id == project_id && p.status == "finished"
                && experiment.as_ref().map(|e| p.experiment_id.as_ref() == Some(e)).unwrap_or(true)
                && variant.as_ref().map(|v| p.variant_id.as_ref() == Some(v)).unwrap_or(true)
                && match kind.as_deref() { Some("human") => p.kind == PlaytestKind::Human, Some("synthetic") => p.kind != PlaytestKind::Human, _ => true }
        }).collect();
        let mut all = vec![];
        for p in &chosen {
            let text = p.events_artifact_id.as_ref().and_then(|a| store.artifact(a).ok().flatten()).and_then(|a| std::fs::read_to_string(&a.path).ok()).unwrap_or_default();
            all.push(crate::experiment::parse_telemetry(&text));
        }
        let design = store.design(project_id)?;
        let human = chosen.iter().filter(|p| p.kind == PlaytestKind::Human).count();
        Ok(json!({
            "signals": crate::experiment::signals(&all),
            "playtests": chosen.len(), "human": human, "synthetic": chosen.len() - human,
            "desired": design.as_ref().map(|d| d.experience.desired_behaviors.clone()).unwrap_or_default(),
            "avoid": design.as_ref().map(|d| d.experience.avoid.clone()).unwrap_or_default(),
        }))
    }

    // ----- the living design's questions and decisions -----

    fn question_create(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let store = self.store.lock().unwrap();
        store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
        let id = crate::design::next_id("Q", store.questions_of(project_id)?.into_iter().map(|q| q.id));
        let q = crate::design::DesignQuestion {
            id: id.clone(), project_id: project_id.into(), question: p_str(params, "question")?.into(), context: p_opt(params, "context").unwrap_or_default(),
            system_id: p_opt(params, "systemId"), status: "open".into(), raised_by: p_opt(params, "raisedBy").unwrap_or_else(|| "human".into()), source: p_opt(params, "source"),
            hypothesis_ids: vec![], experiment_ids: vec![], decision_id: None, answer: String::new(), created_at: now(), resolved_at: None,
        };
        self.atomic(&store, || {
            store.put_question(&q)?;
            if let Some(sid) = &q.system_id {
                if let Some(mut d) = store.design(project_id)? {
                    if let Some(sys) = d.systems.iter_mut().find(|s| &s.id == sid) {
                        if !sys.questions.contains(&q.id) { sys.questions.push(q.id.clone()); }
                        d.updated_at = now();
                        store.put_design(&d)?;
                    }
                }
            }
            self.emit_locked(&store, Event::new("question.created", &q.raised_by).with(json!({"projectId": project_id, "questionId": q.id, "question": q.question, "systemId": q.system_id})));
            Ok(())
        })?;
        Ok(serde_json::to_value(q).unwrap())
    }

    /// Answer a question with a new decision; the question stays, resolved,
    /// pointing at it.
    fn question_resolve(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let qid = p_str(params, "questionId")?;
        let q = self.store.lock().unwrap().question(project_id, qid)?.ok_or_else(|| ApiError::not_found(format!("question {qid}")))?;
        if q.status != "open" { return Err(ApiError::conflict(format!("{qid} is already resolved by {}", q.decision_id.clone().unwrap_or_default()))); }
        let mut p = params.clone();
        p["questionIds"] = json!([qid]);
        if p.get("systemIds").is_none() { if let Some(s) = &q.system_id { p["systemIds"] = json!([s]); } }
        let decided = self.decision_create(&p)?;
        Ok(json!({"question": self.store.lock().unwrap().question(project_id, qid)?, "decision": decided}))
    }

    /// A note on every task started and not yet done, in the project's
    /// quests, whose instructions or notes cite a decision `d` replaces.
    fn note_superseded(&self, project_id: &str, d: &crate::design::DesignDecision) -> Res<()> {
        let tasks: Vec<Task> = {
            let store = self.store.lock().unwrap();
            let quests: Vec<String> = store.quests()?.into_iter().filter(|q| q.project_id == project_id).map(|q| q.id).collect();
            store.tasks()?.into_iter().filter(|t| quests.contains(&t.quest_id) && t.attempts_made > 0 && !matches!(t.state, TaskState::Completed | TaskState::Cancelled | TaskState::Integrating)).collect()
        };
        for t in tasks {
            let text = format!("{}\n{}", t.instructions, t.notes.join("\n"));
            let cited: Vec<&String> = d.supersedes.iter().filter(|old| text.contains(old.as_str())).collect();
            if cited.is_empty() { continue; }
            let note = format!("{} replaces {}: {}", d.id, cited.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", "), d.decision.trim());
            self.task_note(&json!({"taskId": t.id, "text": note, "by": "guild"}))?;
        }
        Ok(())
    }

    /// Undo a resolution (FB-034): the question is open again and its
    /// decision no longer answers it; a decision that answered only it
    /// stays on record, with the reason it no longer holds.
    fn question_reopen(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let qid = p_str(params, "questionId")?;
        let why = p_opt(params, "why").unwrap_or_else(|| "reopened".into());
        let store = self.store.lock().unwrap();
        let mut q = store.question(project_id, qid)?.ok_or_else(|| ApiError::not_found(format!("question {qid}")))?;
        if q.status == "open" { return Err(ApiError::conflict(format!("{qid} is already open"))); }
        let decision = q.decision_id.take();
        self.atomic(&store, || {
            if let Some(mut d) = decision.as_deref().and_then(|id| store.design_decisions_of(project_id).ok()?.into_iter().find(|d| d.id == id)) {
                d.question_ids.retain(|x| x != qid);
                if d.question_ids.is_empty() { d.why = format!("{} (no longer answers {qid}: {why})", d.why.trim()).trim().to_string(); }
                store.put_design_decision(&d)?;
            }
            q.status = "open".into();
            q.answer.clear();
            q.resolved_at = None;
            store.put_question(&q)?;
            self.emit_locked(&store, Event::new("question.reopened", "user").with(json!({"projectId": project_id, "questionId": qid, "decisionId": decision, "why": why})));
            Ok(())
        })?;
        Ok(serde_json::to_value(q).unwrap())
    }

    fn decision_create(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let store = self.store.lock().unwrap();
        store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
        let design = store.design(project_id)?;
        let id = crate::design::next_id("DEC", store.design_decisions_of(project_id)?.into_iter().map(|d| d.id));
        let d = crate::design::DesignDecision {
            id: id.clone(), project_id: project_id.into(), decision: p_str(params, "decision")?.into(), supersedes: p_list(params, "supersedes"), why: p_opt(params, "why").unwrap_or_default(),
            question_ids: p_list(params, "questionIds"), hypothesis_ids: p_list(params, "hypothesisIds"), experiment_ids: p_list(params, "experimentIds"), playtest_ids: p_list(params, "playtestIds"),
            evidence: p_list(params, "evidence"), rejected_alternatives: p_list(params, "rejectedAlternatives"), system_ids: p_list(params, "systemIds"),
            decided_by: p_opt(params, "decidedBy").unwrap_or_else(|| "human".into()), design_version: design.as_ref().map(|x| x.version).unwrap_or(0), created_at: now(),
        };
        let answer = p_opt(params, "answer").unwrap_or_else(|| d.decision.clone());
        self.atomic(&store, || {
            store.put_design_decision(&d)?;
            for qid in &d.question_ids {
                let mut q = store.question(project_id, qid)?.ok_or_else(|| ApiError::not_found(format!("question {qid}")))?;
                q.status = "resolved".into();
                q.decision_id = Some(d.id.clone());
                q.answer = answer.clone();
                q.resolved_at = Some(now());
                store.put_question(&q)?;
                self.emit_locked(&store, Event::new("question.resolved", &d.decided_by).with(json!({"projectId": project_id, "questionId": q.id, "decisionId": d.id})));
            }
            if let Some(mut design) = design.clone() {
                let mut touched = false;
                for sid in &d.system_ids {
                    if let Some(sys) = design.systems.iter_mut().find(|s| &s.id == sid) {
                        if !sys.decisions.contains(&d.id) { sys.decisions.push(d.id.clone()); touched = true; }
                    }
                }
                if touched { design.updated_at = now(); store.put_design(&design)?; }
            }
            self.emit_locked(&store, Event::new("decision.created", &d.decided_by).with(json!({"projectId": project_id, "decisionId": d.id, "decision": d.decision, "questions": d.question_ids, "systems": d.system_ids, "supersedes": d.supersedes})));
            Ok(())
        })?;
        drop(store);
        // Work under way that cites a decision this one replaces hears of it (FB-039).
        if !d.supersedes.is_empty() { self.note_superseded(project_id, &d)?; }
        Ok(serde_json::to_value(d).unwrap())
    }

    /// Upsert one system of the design (its maturity, intent, known,
    /// unknown, current design, links) without rewriting the whole design.
    fn design_system_set(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?;
        let sys: crate::design::GameSystem = serde_json::from_value(params.get("system").cloned().ok_or_else(|| ApiError::invalid("missing system"))?).map_err(|e| ApiError::invalid(format!("system: {e}")))?;
        if sys.id.trim().is_empty() || sys.name.trim().is_empty() { return Err(ApiError::invalid("a system needs an id and a name")); }
        if sys.maturity == crate::design::Maturity::Production {
            let gaps = crate::design::production_gaps(&sys);
            if !gaps.is_empty() { return Err(ApiError::invalid(format!("system {} cannot be production without {}", sys.name, gaps.join(", ")))); }
        }
        let store = self.store.lock().unwrap();
        let mut d = store.design(project_id)?.ok_or_else(|| ApiError::not_found("the project has no design yet"))?;
        match d.systems.iter_mut().find(|s| s.id == sys.id) { Some(existing) => *existing = sys.clone(), None => d.systems.push(sys.clone()) }
        d.version += 1;
        d.updated_at = now();
        self.atomic(&store, || {
            store.put_design(&d)?;
            self.emit_locked(&store, Event::new("design.updated", "user").with(json!({"projectId": project_id, "version": d.version, "system": sys.id, "maturity": sys.maturity})));
            Ok(())
        })?;
        Ok(serde_json::to_value(d).unwrap())
    }

    fn record_decision(&self, d: &RaceDecision) -> Res<()> {
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            store.put_decision(d)?;
            for c in &d.candidates {
                if let Some(mut r) = store.record(&c.attempt_id)? {
                    r.winner = Some(d.winner_attempt_id.as_deref() == Some(c.attempt_id.as_str()));
                    r.decided_by = Some(d.by.clone());
                    r.human_override = d.human_override;
                    if c.score.is_some() { r.judge_score = c.score; }
                    store.put_record(&r)?;
                }
            }
            let task = store.task(&d.task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            self.emit_locked(&store, Event::new("race.decided", &d.by).about_task(&task, &d.project_id).with(json!({"decisionId": d.id, "by": d.by, "winner": d.winner_attempt_id, "confidence": d.confidence, "candidates": d.candidates.len(), "reason": d.reason, "humanOverride": d.human_override})));
            Ok(())
        })
    }

    /// Start a judge attempt for the task: a free member with the judge role
    /// (or reviewer), preferably not one of the candidates' authors. False
    /// when nobody can judge.
    fn start_judge(self: &Arc<Service>, task: &Task, passing: &[&Attempt]) -> Res<bool> {
        let attempt = {
            let store = self.store.lock().unwrap();
            let members = store.members()?;
            let available = self.available_harnesses();
            let busy: Vec<String> = store.active_attempts()?.into_iter().map(|a| a.member_id).collect();
            let authors: Vec<String> = passing.iter().map(|a| a.member_id.clone()).collect();
            let mut wanted = task.clone();
            wanted.member_id = None;
            let mut pick = |role: &str, busy: &[String]| { wanted.role = role.into(); scheduler::pick_member(&wanted, &members, busy, &available).cloned() };
            let mut with_authors = busy.clone();
            with_authors.extend(authors.iter().cloned());
            let member = pick("judge", &with_authors).or_else(|| pick("reviewer", &with_authors)).or_else(|| pick("judge", &busy)).or_else(|| pick("reviewer", &busy));
            let Some(member) = member else { return Ok(false) };
            let quest = store.quest(&task.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
            let attempt = Attempt {
                id: new_id(), task_id: task.id.clone(), task_version: task.version, quest_id: quest.id.clone(), project_id: quest.project_id.clone(), member_id: member.id.clone(),
                harness: member.harness.clone(), model_requested: member.model_id.clone(), model_effective: None, kind: AttemptKind::Judge, state: AttemptState::Queued, workspace_id: None, session_id: None,
                generation: 0, candidate: None, partial: None, summary: String::new(), action: None, pending_permission: None, failure: None, verifier_results: vec![], turns: 0, resumed: 0, deciding: false, escalation: None, worked_seconds: 0, limit_seconds: 0,
                started_at: Some(now()), finished_at: None, created_at: now(), updated_at: now(),
            };
            self.atomic(&store, || {
                store.put_attempt(&attempt)?;
                self.emit_locked(&store, Event::new("judge.started", "guild").about_attempt(&attempt).with(json!({"member": member.name, "harness": member.harness, "candidates": passing.len(), "task": task.title})));
                Ok(())
            })?;
            attempt
        };
        self.spawn_driver(attempt.id.clone());
        Ok(true)
    }

    /// What the judge reads: the task, its acceptance criteria, the design,
    /// and each passing candidate's diff under an anonymous label.
    fn compose_judge_prompt(&self, role_prompt: &str, task: &Task, design: Option<&crate::design::GameDesign>) -> Res<String> {
        let (attempts, diffs) = {
            let store = self.store.lock().unwrap();
            let attempts: Vec<Attempt> = store.attempts_of_task(&task.id)?.into_iter().filter(|a| a.kind == AttemptKind::Work && a.state == AttemptState::Submitted && passes_gates(a)).collect();
            let mut diffs = vec![];
            for a in &attempts {
                let text = a.candidate.as_ref().and_then(|c| c.diff_artifact_id.as_ref()).and_then(|id| store.artifact(id).ok().flatten()).and_then(|art| std::fs::read_to_string(&art.path).ok()).unwrap_or_default();
                diffs.push(text);
            }
            (attempts, diffs)
        };
        let refs: Vec<&Attempt> = attempts.iter().collect();
        let mut s = String::new();
        s.push_str(role_prompt.trim());
        s.push_str(&format!("\n\n# Task: {}\n\n{}\n", task.title, task.instructions.trim()));
        if !task.acceptance.is_empty() {
            s.push_str("\n## Acceptance criteria\n");
            for ac in &task.acceptance { s.push_str(&format!("- {}: {}\n", ac.id, ac.text)); }
        }
        if let Some(d) = design { s.push_str(&crate::design::prompt_context(d)); }
        s.push_str("\n## Candidates\n");
        for (a, diff) in attempts.iter().zip(diffs.iter()) {
            let label = label_for(&refs, a);
            let c = a.candidate.as_ref();
            let verifiers: Vec<String> = a.verifier_results.iter().map(|v| format!("{} {}", v.id, v.status)).collect();
            s.push_str(&format!("\n### Candidate {label}\nfiles changed: {} · +{} −{} · verifiers: {}\n", c.map(|c| c.files_changed).unwrap_or(0), c.map(|c| c.insertions).unwrap_or(0), c.map(|c| c.deletions).unwrap_or(0), if verifiers.is_empty() { "none".to_string() } else { verifiers.join(", ") }));
            let evidence: Vec<&String> = a.verifier_results.iter().flat_map(|v| v.evidence.iter()).collect();
            if !evidence.is_empty() {
                s.push_str("captures of the running game (open them; what the player sees is judged from these, not from the diff):\n");
                for e in evidence { s.push_str(&format!("- {e}\n")); }
            }
            let mut d: String = diff.chars().take(14_000).collect();
            if diff.chars().count() > 14_000 { d.push_str("\n… (diff truncated)"); }
            s.push_str("```diff\n");
            s.push_str(&d);
            s.push_str("\n```\n");
        }
        s.push_str("\nCompare the candidates on the rubric and answer with the JSON block only.\n");
        Ok(s)
    }

    /// The judge's turn ended: read its decision, or fall back to the policy.
    fn finish_judge(self: &Arc<Service>, attempt: &Attempt, task: &Task, outcome: TurnOutcome) -> Res<()> {
        let attempt_id = attempt.id.as_str();
        let text: String = match &outcome {
            TurnOutcome::Completed { .. } => self.store.lock().unwrap().events_of_attempt(attempt_id, 5000)?.into_iter().filter(|e| e.kind == "turn.text").map(|e| e.payload["text"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>().join(""),
            _ => String::new(),
        };
        let (attempts, labels): (Vec<Attempt>, Vec<(String, Attempt)>) = {
            let store = self.store.lock().unwrap();
            let all: Vec<Attempt> = store.attempts_of_task(&task.id)?.into_iter().filter(|a| a.kind == AttemptKind::Work && a.state == AttemptState::Submitted).collect();
            let passing: Vec<&Attempt> = all.iter().filter(|a| passes_gates(a)).collect();
            let labels = passing.iter().map(|a| (label_for(&passing, a), (*a).clone())).collect();
            (all, labels)
        };
        let parsed = match &outcome {
            TurnOutcome::Completed { .. } => parse_judge_output(&text),
            TurnOutcome::Failed { reason } => Err(format!("judge failed: {reason}")),
            TurnOutcome::Cancelled => Err("judge cancelled".into()),
        };
        match parsed {
            Ok(v) => {
                let winner_label = v["winner"].as_str().unwrap_or("").trim().to_uppercase();
                let Some((_, winner)) = labels.iter().find(|(l, _)| *l == winner_label) else {
                    self.move_attempt(attempt_id, AttemptState::Failed, Some(format!("judge named an unknown candidate: {winner_label:?}")))?;
                    return self.decide_by_policy(&task.id, "policy", Some("judge named an unknown candidate".into()));
                };
                let refs: Vec<&Attempt> = attempts.iter().collect();
                let mut candidates: Vec<DecisionCandidate> = attempts.iter().map(|a| decision_candidate(a, &label_for(&refs, a))).collect();
                for c in candidates.iter_mut() {
                    c.score = v["scores"].get(&c.label).and_then(Value::as_object).map(|o| o.values().filter_map(Value::as_f64).sum::<f64>());
                }
                let decision = RaceDecision {
                    id: new_id(), task_id: task.id.clone(), quest_id: task.quest_id.clone(), project_id: attempt.project_id.clone(), by: "judge".into(), judge_attempt_id: Some(attempt_id.into()),
                    candidates, scores: v["scores"].clone(), findings: v["findings"].clone(), tradeoffs: v["tradeoffs"].as_str().unwrap_or("").into(),
                    winner_attempt_id: Some(winner.id.clone()), confidence: v["confidence"].as_f64(), reason: v["reason"].as_str().unwrap_or("").into(), human_override: false, created_at: now(),
                };
                let winner_id = winner.id.clone();
                let artifact = self.write_artifact(Some(attempt_id), "race.decision", serde_json::to_string_pretty(&decision).unwrap_or_default().as_bytes())?;
                self.record_decision(&decision)?;
                self.update_attempt(attempt_id, |a| { a.summary = format!("Candidate {winner_label} wins"); a.failure = None; })?;
                // A judge's verdict is its candidate: submitted, then passed.
                let _ = self.move_attempt(attempt_id, AttemptState::Submitted, None);
                self.move_attempt(attempt_id, AttemptState::Passed, None)?;
                self.emit(Event::new("judge.finished", "guild").about_attempt(attempt).with(json!({"decisionId": decision.id, "artifactId": artifact.id, "winner": winner_label})));
                self.select_candidate(&task.id, &winner_id, "judge")?;
                if task.auto_integrate {
                    if let Err(e) = self.task_integrate(&task.id) { self.log(&format!("task {}: integration failed: {}", task.id, e.message)); }
                }
                Ok(())
            }
            Err(reason) => {
                let state = match &outcome { TurnOutcome::Cancelled => AttemptState::Cancelled, _ => AttemptState::Failed };
                self.move_attempt(attempt_id, state, Some(reason.clone()))?;
                self.emit(Event::new("judge.finished", "guild").about_attempt(attempt).with(json!({"failure": reason})));
                self.decide_by_policy(&task.id, "policy", Some(format!("judge unusable: {reason}")))
            }
        }
    }

    /// Accept one candidate of a task and reject its rivals.
    fn select_candidate(&self, task_id: &str, attempt_id: &str, by: &str) -> Res<Attempt> {
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            let attempts = store.attempts_of_task(task_id)?;
            let winner = attempts.iter().find(|a| a.id == attempt_id).ok_or_else(|| ApiError::not_found("attempt"))?.clone();
            if !matches!(winner.state, AttemptState::Submitted | AttemptState::Verifying) { return Err(ApiError::conflict(format!("attempt is {:?}, not submitted", winner.state))); }
            let member = store.member(&winner.member_id)?.map(|m| m.name).unwrap_or_default();
            for rival in attempts.iter().filter(|a| a.id != attempt_id && matches!(a.state, AttemptState::Submitted | AttemptState::Verifying)) {
                self.move_attempt_locked(&store, &rival.id, AttemptState::Failed, Some(format!("not selected: {member}'s candidate won")))?;
                self.emit_locked(&store, Event::new("candidate.rejected", by).about_attempt(rival).with(json!({"reason": "another candidate was selected", "winner": attempt_id})));
            }
            self.move_attempt_locked(&store, attempt_id, AttemptState::Passed, None)?;
            let mut t = store.task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            t.winner_attempt_id = Some(attempt_id.into());
            t.updated_at = now();
            store.put_task(&t)?;
            self.move_task_locked(&store, task_id, TaskState::Accepted, None)?;
            self.emit_locked(&store, Event::new("candidate.selected", by).about_attempt(&winner).with(json!({"by": by, "candidates": attempts.iter().filter(|a| a.kind == AttemptKind::Work).count()})));
            Ok(winner)
        })
    }

    fn spawn_driver(self: &Arc<Service>, attempt_id: String) {
        // Reserve the slot before the thread starts, so the next tick counts it.
        self.running.lock().unwrap().insert(
            attempt_id.clone(),
            Running { session: SessionRef { id: String::new(), native_id: None }, harness: HarnessId::named("mock"), cancel_requested: false, interrupt_requested: false, started: Instant::now(), waited: Duration::ZERO, waiting_since: None, limit: Duration::ZERO },
        );
        let service = self.clone();
        std::thread::Builder::new()
            .name(format!("attempt-{}", &attempt_id[attempt_id.len().saturating_sub(6)..]))
            .spawn(move || {
                let outcome = service.drive(&attempt_id);
                service.running.lock().unwrap().remove(&attempt_id);
                if let Err(e) = outcome {
                    service.log(&format!("attempt {attempt_id}: {e}"));
                    let generation = service.store.lock().unwrap().attempt(&attempt_id).ok().flatten().map(|a| a.generation).unwrap_or(0);
                    let _ = service.finish_attempt_of(&attempt_id, generation, TurnOutcome::Failed { reason: e.to_string() });
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
            Running { session: SessionRef { id: String::new(), native_id: None }, harness: HarnessId::named("mock"), cancel_requested: false, interrupt_requested: false, started: Instant::now(), waited: Duration::ZERO, waiting_since: None, limit: Duration::ZERO },
        );
        let service = self.clone();
        std::thread::Builder::new()
            .name(format!("resume-{}", &attempt_id[attempt_id.len().saturating_sub(6)..]))
            .spawn(move || {
                let outcome = service.resume(&attempt_id, &native_id);
                service.running.lock().unwrap().remove(&attempt_id);
                // Shutting down closes the session under a resume in flight:
                // the attempt stays as it is for the next start, as in finish_attempt.
                if let (Err(e), true) = (&outcome, service.stop.load(Ordering::Relaxed)) {
                    service.log(&format!("attempt {attempt_id}: resume interrupted by the shutdown ({e}); left for the next start"));
                } else if let Err(e) = outcome {
                    service.log(&format!("attempt {attempt_id}: resume failed: {e}"));
                    // One lock for both reads: a second lock inside the first one's
                    // statement would wait on itself forever.
                    let task = { let store = service.store.lock().unwrap(); store.attempt(&attempt_id).ok().flatten().and_then(|a| store.task(&a.task_id).ok().flatten()) };
                    let _ = service.move_attempt(&attempt_id, AttemptState::Lost, Some(format!("the service restarted and the session could not be reopened: {e}")));
                    if let Some(t) = task { let _ = service.after_runner_dropped(&t, "lost"); }
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
        // A retry learns why the earlier attempts were turned down and, when
        // the last one left a candidate (and this is not a race), goes on
        // from it: asked to fix one line, it fixes that line.
        let earlier: Vec<Attempt> = {
            let store = self.store.lock().unwrap();
            let mut v: Vec<Attempt> = store.attempts_of_task(&task.id)?.into_iter().filter(|a| a.id != attempt.id && a.kind == AttemptKind::Work && a.failure.is_some()).collect();
            v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
            v
        };
        let feedback: Vec<String> = earlier.iter().rev().take(3).rev().filter_map(|a| a.failure.clone()).collect();
        // Its candidate, else what it had written when it stopped.
        let resume_from = if attempt.kind == AttemptKind::Work && task.candidates <= 1 {
            earlier.last().and_then(|a| a.candidate.clone().map(|c| (c, false)).or_else(|| a.partial.clone().map(|c| (c, true))).map(|(c, partial)| (c, partial, workspaces::branch_name(&quest.id, &task.id, &a.id))))
        } else { None };
        let workspace = workspaces::provision(&project, &quest.id, &task.id, attempt_id, &base).map_err(ApiError::internal)?;
        // On the current base, with the last attempt's changes carried over;
        // when they do not apply, the member is told where they are.
        let continued = match &resume_from {
            None => Carried::Nothing,
            Some((c, partial, branch)) if workspaces::carry(&workspace, c) => if *partial { Carried::Partial } else { let _ = branch; Carried::Candidate },
            Some((c, _, branch)) => Carried::Left { branch: branch.clone(), head: c.head_sha.clone(), base: c.base_sha.clone() },
        };
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
        let adapter = self.adapter(member.harness.clone())?;
        let role_for_prompt = match attempt.kind { AttemptKind::Judge => "judge".to_string(), AttemptKind::Analysis => "design-analyst".to_string(), AttemptKind::Work => task.role.clone() };
        let role_prompt = if member.role_prompt.trim().is_empty() || attempt.kind != AttemptKind::Work { config::default_role_prompt(&role_for_prompt) } else { member.role_prompt.clone() };
        let _ = std::fs::create_dir_all(crate::paths::scratch_dir(attempt_id));
        let options = SessionOptions { cwd: PathBuf::from(&workspace.path), model_id: attempt.model_requested.clone(), mode: member.mode.clone(), role_prompt: role_prompt.clone() };
        let session = adapter.create_session(&options).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let design = self.store.lock().unwrap().design(&project.id)?;
        let prompt = match attempt.kind {
            AttemptKind::Judge => self.compose_judge_prompt(&role_prompt, &task, design.as_ref())?,
            AttemptKind::Analysis => self.compose_analysis_prompt(&role_prompt, &task, design.as_ref())?,
            AttemptKind::Work => compose_prompt(&role_prompt, &task, &workspace, design.as_ref(), &feedback, continued),
        };
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
        let adapter = self.adapter(member.harness.clone())?;
        let role_prompt = if member.role_prompt.trim().is_empty() { config::default_role_prompt(&task.role) } else { member.role_prompt.clone() };
        let options = SessionOptions { cwd: PathBuf::from(&workspace.path), model_id: attempt.model_effective.clone().or(attempt.model_requested.clone()), mode: member.mode.clone(), role_prompt };
        let session = adapter.resume_session(&options, native_id).map_err(|e| ApiError::new("harness", e.to_string()))?;
        self.update_attempt(attempt_id, |a| a.resumed += 1)?;
        self.emit(Event::new("session.resumed", "harness").about_attempt(&attempt).with(json!({"nativeId": native_id, "harness": member.harness.clone()})));
        let prompt = "The guild restarted while you were working. Your worktree is as you left it. Continue the task from where you stopped; if it was already complete, say so and finish with the summary.".to_string();
        self.run_turn(attempt_id, &attempt, &member, &adapter, session, &prompt)
    }

    /// Record the session, send one prompt, follow it to the end, close the
    /// session and finish the attempt. Shared by first runs and resumes.
    fn run_turn(self: &Arc<Service>, attempt_id: &str, attempt: &Attempt, member: &Member, adapter: &Arc<dyn HarnessAdapter>, session: SessionRef, prompt: &str) -> Res<()> {
        let runtime_version = self.harness_status.lock().unwrap().iter().find(|s| s.kind == member.harness).map(|s| s.runtime_version.clone()).unwrap_or_default();
        let session_row = Session { id: session.id.clone(), attempt_id: attempt_id.into(), harness: member.harness.clone(), native_id: session.native_id.clone(), runtime_version, state: "running".into(), created_at: now(), updated_at: now() };
        {
            let store = self.store.lock().unwrap();
            store.put_session(&session_row)?;
            self.emit_locked(&store, Event::new("session.started", "harness").about_attempt(attempt).with(json!({"sessionId": session.id, "nativeId": session.native_id, "harness": member.harness})));
        }
        let effective = attempt.model_effective.clone().or(attempt.model_requested.clone());
        // Its task's own time limit, else the guild's.
        let limit = self.store.lock().unwrap().task(&attempt.task_id)?.and_then(|t| t.timeout_minutes).map(|m| Duration::from_secs(m as u64 * 60)).unwrap_or(self.settings.attempt_timeout);
        if let Some(r) = self.running.lock().unwrap().get_mut(attempt_id) { r.limit = limit; }
        let attempt = self.update_attempt(attempt_id, |a| {
            a.session_id = Some(session.id.clone());
            a.model_effective = effective;
            a.turns += 1;
            a.generation += 1;
            a.pending_permission = None;
        })?;
        let attempt = &attempt;
        {
            let mut running = self.running.lock().unwrap();
            let slot = running.entry(attempt_id.to_string()).or_insert(Running { session: session.clone(), harness: member.harness.clone(), cancel_requested: false, interrupt_requested: false, started: Instant::now(), waited: Duration::ZERO, waiting_since: None, limit: Duration::ZERO });
            slot.session = session.clone();
            slot.harness = member.harness.clone();
        }
        self.move_attempt(attempt_id, AttemptState::Running, None)?;
        let notes_of = |s: &Service| s.store.lock().unwrap().task(&attempt.task_id).ok().flatten().map(|t| t.notes).unwrap_or_default();
        let mut seen = notes_of(self).len();
        let handle = adapter.prompt(&session, attempt_id, prompt).map_err(|e| ApiError::new("harness", e.to_string()))?;
        self.emit(Event::new("turn.started", "guild").about_attempt(attempt).with(json!({"turnId": handle.turn_id, "promptChars": prompt.len()})));
        let mut outcome = self.follow_turn(attempt_id, attempt, adapter, &session, handle);
        // Notes added while it worked: one more turn in the same session takes them in.
        for round in 1..=3 {
            let done = matches!(&outcome, TurnOutcome::Completed { stop_reason } if stop_reason != "interrupted");
            let notes = notes_of(self);
            if attempt.kind != AttemptKind::Work || !done || notes.len() <= seen || self.cancel_requested(attempt_id) { break; }
            let added: String = notes[seen..].iter().map(|n| format!("- {}\n", n.trim())).collect();
            seen = notes.len();
            let follow = format!("While you worked, the lead added to the task:\n{added}\nTake this into account now, in the same worktree, then finish with the updated summary.");
            let Ok(handle) = adapter.prompt(&session, &format!("{attempt_id}-note{round}"), &follow) else { break };
            let _ = self.update_attempt(attempt_id, |a| a.turns += 1);
            self.emit(Event::new("turn.started", "guild").about_attempt(attempt).with(json!({"turnId": handle.turn_id, "promptChars": follow.len(), "notes": true})));
            outcome = self.follow_turn(attempt_id, attempt, adapter, &session, handle);
        }
        let stopped = adapter.stop(&session).unwrap_or(false);
        self.emit(Event::new("session.closed", "harness").about_attempt(attempt).with(json!({"confirmed": stopped})));
        self.finish_attempt_of(attempt_id, attempt.generation, outcome)
    }

    /// A result from a driver that was superseded (the attempt was resumed
    /// by a newer generation) must not touch the attempt.
    fn finish_attempt_of(self: &Arc<Service>, attempt_id: &str, generation: u32, outcome: TurnOutcome) -> Res<()> {
        let current = self.store.lock().unwrap().attempt(attempt_id)?.map(|a| a.generation).unwrap_or(0);
        if current != generation {
            self.log(&format!("attempt {attempt_id}: result of generation {generation} ignored, now at {current}"));
            let stale = self.store.lock().unwrap().attempt(attempt_id).ok().flatten();
            if let Some(a) = stale {
                self.emit(Event::new("attempt.stale_result", "guild").about_attempt(&a).with(json!({"generation": generation, "current": current, "outcome": format!("{outcome:?}")})));
            }
            return Ok(());
        }
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
            let (cancel, interrupt, worked, limit) = {
                let running = self.running.lock().unwrap();
                running.get(attempt_id).map(|r| (r.cancel_requested, r.interrupt_requested, r.working_time(), if r.limit.is_zero() { self.settings.attempt_timeout } else { r.limit })).unwrap_or((true, false, Duration::ZERO, self.settings.attempt_timeout))
            };
            if !timed_out && worked > limit {
                timed_out = true;
                self.emit(Event::new("attempt.timeout", "guild").about_attempt(attempt).with(json!({"afterSeconds": limit.as_secs()})));
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
            HarnessEvent::SessionId(native) => {
                let store = self.store.lock().unwrap();
                let session_id = store.attempt(attempt_id).ok().flatten().and_then(|a| a.session_id);
                if let Ok(Some(mut s)) = session_id.as_deref().map(|id| store.session(id)).unwrap_or(Ok(None)) {
                    if s.native_id.as_deref() != Some(native.as_str()) {
                        s.native_id = Some(native.clone());
                        s.updated_at = now();
                        let _ = store.put_session(&s);
                        self.emit_locked(&store, Event::new("session.identified", "harness").about_attempt(attempt).with(json!({"nativeId": native})));
                    }
                }
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
                if let Some(r) = self.running.lock().unwrap().get_mut(attempt_id) { r.waiting_since.get_or_insert_with(Instant::now); }
                self.emit(Event::new("permission.requested", "harness").about_attempt(attempt).with(json!({"requestId": request_id, "title": title, "detail": detail, "options": options})));
                // Routine work in the worktree is answered at once; only the unusual waits.
                let mut programs = self.project_of_attempt(attempt).map(|p| crate::policy::project_programs(&p)).unwrap_or_default();
                let (worktree, granted) = {
                    let store = self.store.lock().unwrap();
                    let worktree = store.attempt(attempt_id).ok().flatten().and_then(|a| a.workspace_id).and_then(|w| store.workspace(&w).ok().flatten()).map(|w| w.path).unwrap_or_default();
                    (worktree, store.task(&attempt.task_id).ok().flatten().map(|t| t.granted).unwrap_or_default())
                };
                // What this task was already allowed does not ask again.
                programs.extend(granted.programs.iter().cloned());
                let verdict = match crate::policy::command_of(&title, &detail) {
                    Some(cmd) if granted.commands.contains(&cmd) && crate::policy::decide_request(&title, &detail, &programs, &worktree) != crate::policy::Verdict::Refuse => crate::policy::Verdict::Allow,
                    _ => crate::policy::decide_request(&title, &detail, &programs, &worktree),
                };
                if let Some(option) = crate::policy::option_for(&verdict, &title, &options) {
                    let by = if verdict == crate::policy::Verdict::Allow { "policy" } else { "policy-refused" };
                    if let Err(e) = self.permission_respond(&json!({"attemptId": attempt_id, "requestId": request_id, "optionId": option, "by": by})) {
                        self.log(&format!("attempt {attempt_id}: the policy could not answer {title:?}: {}", e.message));
                    }
                } else {
                    // What the policy leaves open goes to the guild's orchestrator, if it has one.
                    self.ask_orchestrator(attempt_id, crate::orchestrator::DecisionKind::Permission);
                }
            }
            HarnessEvent::Raw(_) => {}
        }
    }

    /// Record how the turn ended: commit the candidate and run the
    /// verifiers, or fail the attempt and decide the task's next state.
    fn finish_attempt(self: &Arc<Service>, attempt_id: &str, outcome: TurnOutcome) -> Res<()> {
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
        if attempt.kind == AttemptKind::Judge { return self.finish_judge(&attempt, &task, outcome); }
        if attempt.kind == AttemptKind::Analysis { return self.finish_analysis(&attempt, &task, outcome); }
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
                {
                    let store = self.store.lock().unwrap();
                    self.atomic(&store, || {
                        let mut a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
                        a.candidate = candidate.clone();
                        a.action = None;
                        a.pending_permission = None;
                        a.updated_at = now();
                        store.put_attempt(&a)?;
                        if let Some(c) = &candidate {
                            self.emit_locked(&store, Event::new("candidate.committed", "guild").about_attempt(&attempt).with(json!({"baseSha": c.base_sha, "headSha": c.head_sha, "filesChanged": c.files_changed, "insertions": c.insertions, "deletions": c.deletions})));
                        } else {
                            self.emit_locked(&store, Event::new("candidate.empty", "guild").about_attempt(&attempt).with(json!({"note": "the attempt left no changes"})));
                        }
                        self.move_attempt_locked(&store, attempt_id, AttemptState::Submitted, None)?;
                        if task.candidates <= 1 { self.move_task_locked(&store, &task.id, TaskState::Verifying, None)?; }
                        Ok(())
                    })?;
                }
                // The candidate's own .guild/guild.toml says what to run (the project's
                // record can be older); one that changes its verifiers is checked with
                // them, but never accepted without someone reading that change (FB-042).
                let own_gates = match (&workspace, &candidate) {
                    (Some(ws), Some(c)) => workspaces::git(Path::new(&ws.path), &["diff", "--name-only", &c.base_sha, &c.head_sha, "--", ".guild/guild.toml"]).is_ok_and(|out| !out.trim().is_empty()),
                    _ => false,
                };
                if let (Some(ws), Some(p), Some(_)) = (&workspace, &project, &candidate) {
                    let verifiers = config::load(Path::new(&ws.path)).ok().flatten().map(|f| f.verifiers.iter().map(|v| v.to_verifier()).collect::<Vec<_>>()).unwrap_or_else(|| p.verifiers.clone());
                    if !verifiers.is_empty() || own_gates {
                        self.move_attempt(attempt_id, AttemptState::Verifying, None)?;
                        let mut results = self.run_verifiers(attempt_id, &attempt, &verifiers, ws);
                        if own_gates {
                            results.push(VerifierResult { id: "own-gates".into(), required: true, status: "needs-review".into(), exit_code: None, duration_ms: 0, log_artifact_id: None,
                                tail: "the candidate changes .guild/guild.toml, so it was checked with its own verifiers: read that change in its diff before accepting it".into(), evidence: vec![] });
                        }
                        let all_required_pass = results.iter().filter(|r| r.required).all(|r| r.status == "passed");
                        self.update_attempt(attempt_id, |a| a.verifier_results = results.clone())?;
                        // Back to submitted: acceptance is a decision, with the results in view.
                        let store = self.store.lock().unwrap();
                        self.atomic(&store, || {
                            self.move_attempt_locked(&store, attempt_id, AttemptState::Submitted, None)?;
                            self.emit_locked(&store, Event::new("verification.finished", "verifier").about_attempt(&attempt).with(json!({"requiredPassed": all_required_pass, "results": results})));
                            Ok(())
                        })?;
                    }
                }
                // A choice left to the user goes to the orchestrator first, if the guild has one.
                if candidate.is_some() && task.candidates <= 1 && task.strategy.selection() == Selection::User {
                    if own_gates { self.update_attempt(attempt_id, |a| a.escalation = Some("it changes its own verifiers (.guild/guild.toml): only you or the delegate accept that".into()))?; }
                    else { self.ask_orchestrator(attempt_id, crate::orchestrator::DecisionKind::Candidate); }
                }
                self.settle_task(&task.id)
            }
            TurnOutcome::Failed { reason } => {
                self.keep_partial(&attempt, &task, workspace.as_ref());
                self.move_attempt(attempt_id, if reason == "timed out" { AttemptState::TimedOut } else { AttemptState::Failed }, Some(reason.clone()))?;
                if task.candidates > 1 { return self.settle_task(&task.id); }
                self.after_failed_attempt(&task, &reason)
            }
            TurnOutcome::Cancelled => {
                self.keep_partial(&attempt, &task, workspace.as_ref());
                self.move_attempt(attempt_id, AttemptState::Cancelled, Some("cancelled".into()))?;
                if task.candidates > 1 { return self.settle_task(&task.id); }
                let _ = self.move_task(&task.id, TaskState::Blocked, Some("attempt cancelled; retry to continue".into()));
                Ok(())
            }
        }
    }

    /// An attempt that stops before finishing keeps what it had written, as
    /// a commit on its branch, so the retry goes on from it (F-HD-03).
    fn keep_partial(&self, attempt: &Attempt, task: &Task, workspace: Option<&Workspace>) {
        if attempt.kind != AttemptKind::Work { return; }
        let Some(ws) = workspace.filter(|w| Path::new(&w.path).exists()) else { return };
        if let Ok(Some(partial)) = workspaces::commit_candidate(ws, &format!("guild partial: {}", task.title)) {
            let (files, added, removed) = (partial.files_changed, partial.insertions, partial.deletions);
            if self.update_attempt(&attempt.id, |a| a.partial = Some(partial)).is_ok() {
                self.emit(Event::new("partial.kept", "guild").about_attempt(attempt).with(json!({"filesChanged": files, "insertions": added, "deletions": removed})));
            }
        }
    }

    /// A runner ended without a candidate: a race settles with the rest,
    /// a single attempt follows the retry policy.
    fn after_runner_dropped(self: &Arc<Service>, task: &Task, reason: &str) -> Res<()> {
        if task.candidates > 1 { self.settle_task(&task.id) } else { self.after_failed_attempt(task, reason) }
    }

    fn after_failed_attempt(&self, task: &Task, reason: &str) -> Res<()> {
        let fresh = self.store.lock().unwrap().task(&task.id)?.unwrap_or_else(|| task.clone());
        if fresh.attempts_made < fresh.max_attempts {
            self.move_task(&task.id, TaskState::Ready, None)?;
        } else {
            self.move_task(&task.id, TaskState::Failed, Some(format!("{} attempts failed; last: {reason}", fresh.attempts_made)))?;
            let attempts: Vec<Attempt> = self.store.lock().unwrap().attempts_of_task(&task.id)?.into_iter().filter(|a| a.kind == AttemptKind::Work).collect();
            // The orchestrator retries a task once; failing again, it is the delegate's.
            let retried = attempts.iter().any(|a| a.failure.as_deref().is_some_and(|f| f.contains(ORCHESTRATOR_RETRY)));
            if let (false, Some(a)) = (retried, attempts.iter().max_by(|a, b| a.created_at.cmp(&b.created_at))) { self.ask_orchestrator(&a.id, crate::orchestrator::DecisionKind::Failure); }
        }
        Ok(())
    }

    // ----- the guild's orchestrator

    /// An artifact's text (a diff, a log).
    fn artifact_text(&self, id: &str) -> Res<String> {
        let a = self.store.lock().unwrap().artifact(id)?.ok_or_else(|| ApiError::not_found(format!("artifact {id}")))?;
        std::fs::read_to_string(&a.path).map_err(|e| ApiError::internal(e.to_string()))
    }

    /// The member that decides for the guild while the delegate talks to
    /// the user: enabled, with the orchestrator role, on an available harness.
    fn steward(&self) -> Option<Member> {
        let available = self.available_harnesses();
        self.store.lock().unwrap().members().ok()?.into_iter().find(|m| m.enabled && m.roles.iter().any(|r| r == "orchestrator") && available.contains(&m.harness))
    }

    /// Leave a decision to the guild's orchestrator, when it has one: the
    /// attempt is marked as being decided, so nobody else needs to.
    fn ask_orchestrator(&self, attempt_id: &str, kind: crate::orchestrator::DecisionKind) -> bool {
        if self.steward().is_none() { return false; }
        {
            let mut queue = self.decisions.lock().unwrap();
            if queue.iter().any(|(id, k)| id == attempt_id && *k == kind) { return true; }
            queue.push((attempt_id.to_string(), kind));
        }
        if let Ok(a) = self.update_attempt(attempt_id, |a| { a.deciding = true; a.escalation = None; }) {
            self.emit(Event::new("orchestrator.deciding", "guild").about_attempt(&a).with(json!({"kind": kind})));
        }
        true
    }

    /// Wake the orchestrator for one decision and carry it out; whatever it
    /// cannot settle (or says to escalate) is left to the delegate or the user.
    fn decide(self: &Arc<Service>, attempt_id: &str, kind: crate::orchestrator::DecisionKind) {
        let done = self.rule(attempt_id, kind).and_then(|r| { self.apply_ruling(attempt_id, kind, &r)?; Ok(r) });
        let escalation = match &done {
            Ok(r) if r.decision == "escalate" => Some(r.reason.clone()),
            Ok(_) => None,
            Err(e) => Some(format!("the orchestrator could not decide: {}", e.message)),
        };
        let Ok(a) = self.update_attempt(attempt_id, |a| { a.deciding = false; a.escalation = escalation.clone(); }) else { return };
        let (decision, reason) = done.map(|r| (r.decision, r.reason)).unwrap_or_else(|e| ("escalate".into(), e.message));
        let kind_name = if escalation.is_some() { "orchestrator.escalated" } else { "orchestrator.decided" };
        self.emit(Event::new(kind_name, "orchestrator").about_attempt(&a).with(json!({"kind": kind, "decision": decision, "reason": reason})));
    }

    /// The orchestrator's ruling on one decision, from a turn of its own.
    fn rule(&self, attempt_id: &str, kind: crate::orchestrator::DecisionKind) -> Res<crate::orchestrator::Ruling> {
        use crate::orchestrator::DecisionKind::*;
        let steward = self.steward().ok_or_else(|| ApiError::conflict("the guild has no orchestrator"))?;
        let (attempt, task, project, design, worker, workspace) = {
            let store = self.store.lock().unwrap();
            let attempt = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
            let task = store.task(&attempt.task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            let project = store.project(&attempt.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            let design = store.design(&project.id)?;
            let worker = store.member(&attempt.member_id)?.map(|m| m.name).unwrap_or_else(|| attempt.member_id.clone());
            let workspace = attempt.workspace_id.as_deref().and_then(|w| store.workspace(w).ok().flatten());
            (attempt, task, project, design, worker, workspace)
        };
        let context = match kind {
            Permission => {
                let p = attempt.pending_permission.clone().ok_or_else(|| ApiError::conflict("the permission was answered already"))?;
                let options: Vec<String> = p.options.iter().map(|o| format!("{} ({})", o.name, o.kind)).collect();
                let worktree = workspace.map(|w| w.path).unwrap_or_default();
                let what = crate::policy::describe(&p.title, &p.detail, &worktree);
                format!("{worker} asks for a permission while working in its worktree {worktree} (its scratch folder: {}):\n\n```\n{}\n```\n{what}\n{}\n\nIts harness offers: {}.", crate::paths::scratch_dir(&attempt.id).display(), p.title.trim(), p.detail.trim(), options.join(", "))
            }
            Candidate => {
                let mut diff = attempt.candidate.as_ref().and_then(|c| c.diff_artifact_id.clone()).and_then(|id| self.artifact_text(&id).ok()).unwrap_or_default();
                if crate::domain::cut(&mut diff, 40_000) { diff.push_str("\n… (cut)"); }
                // Each verifier's end, not only its last line: what it ran is the evidence.
                let verifiers: Vec<String> = attempt.verifier_results.iter().map(|v| {
                    let lines: Vec<&str> = v.tail.lines().collect();
                    let tail = lines[lines.len().saturating_sub(15)..].join("\n    ");
                    format!("- {} {}{}{}", v.id, v.status, if v.required { " (required)" } else { "" }, if tail.trim().is_empty() { String::new() } else { format!(":\n    {tail}") })
                }).collect();
                format!("{worker} left this candidate. Its summary: {}\n\nVerifiers:\n{}\n\nDiff:\n```diff\n{diff}\n```", attempt.summary.trim(), if verifiers.is_empty() { "- none".into() } else { verifiers.join("\n") })
            }
            Failure => format!("The task ran out of attempts ({} of {}). The last one, by {worker}, failed: {}\n\nIts last words: {}", task.attempts_made, task.max_attempts, attempt.failure.clone().unwrap_or_default(), attempt.summary.trim()),
        };
        let prompt = crate::orchestrator::decision_prompt(kind, &task, &worker, &context, design.as_ref());
        let text = self.one_turn(&project, &steward, &config::default_role_prompt("orchestrator"), &prompt, Duration::from_secs(120))?;
        crate::orchestrator::parse_ruling(&text, kind).ok_or_else(|| ApiError::new("orchestrator", "its answer had no decision"))
    }

    /// Carry out a ruling through the same commands the user has; a
    /// decision someone took meanwhile is left alone.
    fn apply_ruling(self: &Arc<Service>, attempt_id: &str, kind: crate::orchestrator::DecisionKind, r: &crate::orchestrator::Ruling) -> Res<()> {
        use crate::orchestrator::DecisionKind::*;
        let a = self.store.lock().unwrap().attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
        match (kind, r.decision.as_str()) {
            (_, "escalate") => {}
            (Permission, decision) => {
                let Some(p) = a.pending_permission.clone() else { return Ok(()) };
                let verdict = if decision == "allow" { crate::policy::Verdict::Allow } else { crate::policy::Verdict::Refuse };
                let option = crate::policy::option_for(&verdict, &p.title, &p.options).ok_or_else(|| ApiError::invalid("the harness offers no such option"))?;
                self.permission_respond(&json!({"attemptId": a.id, "requestId": p.request_id, "optionId": option, "by": "orchestrator"}))?;
            }
            (Candidate, decision) => {
                if a.state != AttemptState::Submitted { return Ok(()); }
                if decision == "accept" { self.task_accept(&json!({"taskId": a.task_id, "attemptId": a.id, "integrate": true, "by": "orchestrator"}))?; }
                else { self.task_reject(&json!({"taskId": a.task_id, "attemptId": a.id, "reason": format!("the orchestrator: {}", r.reason)}))?; }
            }
            (Failure, _) => {
                let task = self.store.lock().unwrap().task(&a.task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
                if task.state != TaskState::Failed { return Ok(()); }
                // The next attempt reads the last failures: the orchestrator's instructions go with this one.
                self.update_attempt(&a.id, |x| x.failure = Some(format!("{}{ORCHESTRATOR_RETRY}{}", x.failure.clone().unwrap_or_default(), r.reason)))?;
                self.task_retry(&json!({"taskId": a.task_id}))?;
            }
        }
        Ok(())
    }

    fn run_verifiers(&self, attempt_id: &str, attempt: &Attempt, verifiers: &[Verifier], workspace: &Workspace) -> Vec<VerifierResult> {
        let mut results = vec![];
        for v in verifiers {
            self.emit(Event::new("verifier.started", "verifier").about_attempt(attempt).with(json!({"verifierId": v.id, "argv": v.argv})));
            let started = Instant::now();
            // A fresh folder for what the verifier wants seen, not only read:
            // captures of the running game. Images left there become evidence.
            let evidence_dir = crate::paths::artifacts_dir().join(attempt_id).join(format!("evidence-{}-{}", v.id, new_id()));
            let _ = std::fs::create_dir_all(&evidence_dir);
            let env = vec![("GUILD_EVIDENCE".to_string(), evidence_dir.display().to_string())];
            let (status, code, output) = match workspaces::run_with_timeout_env(Path::new(&workspace.path), &v.argv, v.timeout_seconds, &env) {
                Ok((0, out)) => ("passed", Some(0), out),
                Ok((code, out)) => ("failed", Some(code), out),
                Err(e) => ("error", None, e),
            };
            let artifact = self.write_artifact(Some(attempt_id), &format!("verifier-{}", v.id), output.as_bytes()).ok();
            let tail: String = output.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            let evidence = self.keep_evidence(attempt_id, &evidence_dir);
            let result = VerifierResult { id: v.id.clone(), required: v.required, status: status.into(), exit_code: code, duration_ms: started.elapsed().as_millis() as u64, log_artifact_id: artifact.map(|a| a.id), tail, evidence };
            self.emit(Event::new("verifier.finished", "verifier").about_attempt(attempt).with(json!({"verifierId": v.id, "status": status, "exitCode": code, "durationMs": result.duration_ms, "logArtifactId": result.log_artifact_id})));
            results.push(result);
        }
        results
    }

    /// The images a verifier left in its evidence folder, registered as
    /// artifacts where they are (sorted by name); the folder goes when empty.
    fn keep_evidence(&self, attempt_id: &str, dir: &Path) -> Vec<String> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        files.retain(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).map(|e| matches!(e.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")).unwrap_or(false));
        files.sort();
        let mut kept = vec![];
        for path in files {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("evidence").to_string();
            let artifact = Artifact { id: new_id(), attempt_id: Some(attempt_id.into()), kind: format!("evidence:{name}"), path: path.display().to_string(), sha256: format!("{:x}", sha2::Sha256::digest(&bytes)), size_bytes: bytes.len() as u64, created_at: now() };
            if self.store.lock().unwrap().put_artifact(&artifact).is_ok() { kept.push(artifact.path); }
        }
        if kept.is_empty() { let _ = std::fs::remove_dir_all(dir); }
        kept
    }

    /// After a restart: attempts the store thinks are active have no
    /// driver. Adopt a candidate their worktree holds, or mark them lost.
    pub fn reconcile(self: &Arc<Service>) {
        // An AI player's game dies with the service; its playtest ends there.
        let orphans: Vec<Playtest> = self.store.lock().unwrap().playtests().unwrap_or_default().into_iter().filter(|p| p.kind == PlaytestKind::AiPlayer && p.status == "running").collect();
        for pt in orphans {
            let project = self.store.lock().unwrap().project(&pt.project_id).ok().flatten();
            if let Some(project) = project {
                let _ = self.close_ai_playtest(pt, &project, vec![], Some("the guild restarted while it played".into()), String::new(), Instant::now());
            }
        }
        // An integration the service died in the middle of: do it again; the
        // squash finds nothing new when it had already landed.
        let integrating: Vec<Task> = self.store.lock().unwrap().tasks().unwrap_or_default().into_iter().filter(|t| t.state == TaskState::Integrating).collect();
        for t in integrating {
            self.log(&format!("reconciling integration of task {}", t.id));
            if self.move_task(&t.id, TaskState::Accepted, None).is_ok() {
                if let Err(e) = self.task_integrate(&t.id) { self.log(&format!("task {}: integration after restart failed: {}", t.id, e.message)); }
            }
        }
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
                    // Already a candidate; it only needs a decision, and its task must say so.
                    let _ = self.move_attempt(&a.id, AttemptState::Submitted, None);
                    if let Some(t) = &task {
                        if t.candidates <= 1 && t.state == TaskState::Running { let _ = self.move_task(&t.id, TaskState::Verifying, None); }
                    }
                }
                (Some(candidate), _) => {
                    let _ = self.update_attempt(&a.id, |x| { x.candidate = Some(candidate.clone()); x.failure = Some("recovered after a restart".into()); });
                    let _ = self.move_attempt(&a.id, AttemptState::Submitted, None);
                    if let Some(t) = &task { let _ = self.move_task(&t.id, TaskState::Verifying, None); }
                }
                (None, _) if a.kind == AttemptKind::Analysis => {
                    let _ = self.move_attempt(&a.id, AttemptState::Lost, Some("the service restarted while it analysed".into()));
                    // The tick starts a new analysis when none is active.
                }
                (None, _) if a.kind == AttemptKind::Judge => {
                    let _ = self.move_attempt(&a.id, AttemptState::Lost, Some("the service restarted while it judged".into()));
                    let _ = self.decide_by_policy(&a.task_id, "policy", Some("judge lost in a restart".into()));
                }
                (None, _) => {
                    let _ = self.move_attempt(&a.id, AttemptState::Lost, Some("the service restarted while it ran".into()));
                    if let Some(t) = &task { let _ = self.after_runner_dropped(t, "lost"); }
                }
            }
        }
    }

    /// Close every live harness session, leaving the attempts in their
    /// state for the next start to resume.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let live: Vec<(String, HarnessKind, SessionRef)> = self.running.lock().unwrap().iter().map(|(id, r)| (id.clone(), r.harness.clone(), r.session.clone())).collect();
        for (attempt_id, harness, session) in live {
            if session.id.is_empty() { continue; }
            let Ok(adapter) = self.adapter(harness) else { continue };
            let _ = adapter.stop(&session);
            let closing = self.store.lock().unwrap().attempt(&attempt_id).ok().flatten();
            if let Some(a) = closing {
                self.emit(Event::new("session.closed", "guild").about_attempt(&a).with(json!({"confirmed": true, "reason": "service stopping"})));
            }
        }
    }

    pub fn snapshot(&self) -> Res<Snapshot> {
        let harnesses = self.harness_status.lock().unwrap().clone();
        let mut snap = self.store.lock().unwrap().snapshot(SERVICE_VERSION, harnesses)?;
        snap.roles = config::role_catalog();
        snap.conversation = self.conversation(200)?;
        snap.orchestrator = crate::orchestrator::pick(&snap.members, &self.available_harnesses()).map(|m| m.id);
        snap.orchestrator_busy = self.orchestrating.lock().unwrap().is_some();
        snap.concurrency = self.concurrency.load(Ordering::Relaxed);
        // Each running attempt's work so far against its limit, for whoever watches the time.
        {
            let running = self.running.lock().unwrap();
            for a in snap.attempts.iter_mut() {
                if let Some(r) = running.get(&a.id) { a.worked_seconds = r.working_time().as_secs(); a.limit_seconds = if r.limit.is_zero() { self.settings.attempt_timeout } else { r.limit }.as_secs(); }
            }
        }
        Ok(snap)
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
                let adapter = self.adapter(kind.clone())?;
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
            "guild.found" => self.guild_found(params),
            "roles.list" => Ok(serde_json::to_value(config::role_catalog()).unwrap()),
            "member.update" => self.member_update(params),
            "design.get" => {
                let id = p_str(params, "projectId")?;
                Ok(serde_json::to_value(self.store.lock().unwrap().design(id)?).unwrap())
            }
            "design.set" => self.design_set(params),
            "visual.add" => self.visual_add(params),
            "visual.palette" => self.visual_palette(params),
            "visual.update" => self.visual_update(params),
            "visual.remove" => self.visual_remove(params),
            "visual.notes" => self.visual_notes(params),
            "design.link" => self.design_link(params),
            "design.synced" => self.design_synced(params),
            "play.start" => self.play_start(params),
            "play.finish" => self.play_finish(p_str(params, "playtestId")?),
            "observation.create" => self.observation_create(params),
            "observation.promote" => self.observation_promote(params),
            "signals" => self.signals_of(params),
            "ai.play" => self.ai_play_start(params),
            "ai.stop" => self.ai_play_stop(p_str(params, "playtestId")?),
            "finding.verify" => self.finding_verify(params),
            "finding.promote" => self.finding_promote(params),
            "finding.set" => self.finding_set(params),
            "feedback.create" => self.feedback_create(params),
            "feedback.list" => self.feedback_list(params),
            "feedback.set" => self.feedback_set(params),
            "question.create" => self.question_create(params),
            "question.resolve" => self.question_resolve(params),
            "question.reopen" => self.question_reopen(params),
            "decision.create" => self.decision_create(params),
            "design.system.set" => self.design_system_set(params),
            "hypothesis.create" => self.hypothesis_create(params),
            "hypothesis.update" => self.hypothesis_update(params),
            "experiment.create" => self.experiment_create(params),
            "experiment.start" => self.experiment_start(p_str(params, "experimentId")?),
            "experiment.accept" => self.experiment_decide(params, "accept"),
            "experiment.reject" => self.experiment_decide(params, "reject"),
            "experiment.iterate" => self.experiment_decide(params, "iterate"),
            "records.list" => {
                let rows = self.store.lock().unwrap().records(p_opt(params, "projectId").as_deref(), p_opt(params, "taskId").as_deref())?;
                Ok(serde_json::to_value(rows).unwrap())
            }
            "design.template" => self.design_template(params),
            "design.export" => {
                let id = p_str(params, "projectId")?;
                let store = self.store.lock().unwrap();
                let project = store.project(id)?.ok_or_else(|| ApiError::not_found(format!("project {id}")))?;
                let d = store.design(id)?.ok_or_else(|| ApiError::not_found("the project has no design yet"))?;
                let bundle = crate::design::DesignBundle {
                    design: d.clone(),
                    questions: store.questions_of(id)?,
                    decisions: store.design_decisions_of(id)?,
                    hypotheses: store.hypotheses()?.into_iter().filter(|h| h.project_id == id).collect(),
                    experiments: store.experiments()?.into_iter().filter(|e| e.project_id == id).collect(),
                    observations: store.observations_of(id)?,
                };
                // Never the user's checkout: a managed branch the user merges when ready.
                let ref_name = crate::design::design_ref(&project.id);
                let (sha, files) = workspaces::publish(&project, &ref_name, &project.target_ref, &format!("guild: game design v{}", d.version), |dir| crate::design::export(dir, &bundle)).map_err(ApiError::internal)?;
                let files: Vec<String> = files.iter().map(|f| f.rsplit("game-design/").next().map(|r| format!("game-design/{r}")).unwrap_or_else(|| f.clone())).collect();
                self.emit_locked(&store, Event::new("design.exported", "user").with(json!({"projectId": id, "files": files.len(), "ref": ref_name, "sha": sha})));
                Ok(json!({"files": files, "ref": ref_name, "sha": sha}))
            }
            "orchestrator.say" => self.orchestrator_say(params),
            "member.add" => self.member_add(params),
            "guild.concurrency" => self.set_concurrency(params),
            "member.remove" => self.member_remove(p_str(params, "memberId")?),
            "guild.close" => self.guild_close(),
            "guild.reopen" => self.guild_reopen(),
            "quest.create" => self.quest_create(params),
            "task.create" => self.task_create(params),
            "task.update" => self.task_update(params),
            "task.note" => self.task_note(params),
            "quest.start" => self.quest_start(params),
            "quest.sync" => self.quest_sync(params),
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
            playtest: file.as_ref().and_then(|f| f.playtest.as_ref()).map(|p| p.to_runner()),
            play: file.as_ref().and_then(|f| f.play.as_ref()).map(|p| p.to_runner()),
            ai_play: file.as_ref().and_then(|f| f.ai_play.as_ref()).map(|p| p.to_runner()),
            permitted_programs: file.as_ref().map(|f| f.permissions.programs.clone()).unwrap_or_default(),
            created_at: existing.map(|p| p.created_at).unwrap_or_else(now),
        };
        store.put_project(&project)?;
        let mut members = 0;
        if let Some(f) = &file {
            for m in &f.members {
                let mut member = m.to_member();
                // Re-adding the project keeps who the user turned off (FB-024).
                if let Some(old) = store.member(&member.id)? { member.enabled = old.enabled; member.switched = old.switched; }
                store.put_member(&member)?;
                members += 1;
            }
        }
        // A design kept in the repo comes along, unless the guild already has one:
        // the guild's own design branch first, then the checkout's game-design/.
        let mut design_imported = false;
        if store.design(&project.id)?.is_none() {
            let from_branch = workspaces::read_ref(&project, &crate::design::design_ref(&project.id), |dir| crate::design::import(dir, &project.id)).flatten();
            if let Some(b) = from_branch.or_else(|| crate::design::import(&repo, &project.id)) {
                store.put_design(&b.design)?;
                for q in &b.questions { if store.question(&project.id, &q.id)?.is_none() { store.put_question(q)?; } }
                for d in &b.decisions { store.put_design_decision(d)?; }
                for h in &b.hypotheses { if store.hypothesis(&h.id)?.is_none() { store.put_hypothesis(h)?; } }
                for e in &b.experiments { if store.experiment(&e.id)?.is_none() { store.put_experiment(e)?; } }
                for o in &b.observations { if store.observation(&project.id, &o.id)?.is_none() { store.put_observation(o)?; } }
                design_imported = true;
            }
        }
        self.emit_locked(&store, Event::new("project.added", "user").with(json!({"projectId": project.id, "repoPath": project.repo_path, "members": members, "designImported": design_imported})));
        Ok(json!({"project": project, "membersFromConfig": members, "designImported": design_imported}))
    }

    /// Found a guild in one go: the project, its members with their
    /// roles, and a design from a template. What the dungeon's window sends.
    fn guild_found(&self, params: &Value) -> ApiResult {
        let added = self.project_add(params)?;
        let project_id = added["project"]["id"].as_str().unwrap_or("").to_string();
        let mut members = vec![];
        for m in params.get("members").and_then(Value::as_array).into_iter().flatten() {
            let name = m.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if name.is_empty() { continue; }
            let id = m.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| slug(&name));
            let role = m.get("role").and_then(Value::as_str).unwrap_or("implementer");
            let p = json!({"id": id, "name": name, "harness": m.get("harness").and_then(Value::as_str).unwrap_or("kiro"), "roles": [role], "model": m.get("model"), "mode": m.get("mode")});
            members.push(self.member_add(&p)?);
        }
        let design = match p_opt(params, "template") {
            Some(t) if !t.is_empty() && t != "none" => Some(self.design_template(&json!({"projectId": project_id, "template": t}))?),
            _ => None,
        };
        self.emit(Event::new("guild.founded", "user").with(json!({"projectId": project_id, "members": members.len(), "template": p_opt(params, "template")})));
        Ok(json!({"project": added["project"], "members": members, "design": design}))
    }

    fn member_update(&self, params: &Value) -> ApiResult {
        let id = p_str(params, "memberId")?;
        let store = self.store.lock().unwrap();
        let mut m = store.member(id)?.ok_or_else(|| ApiError::not_found(format!("member {id}")))?;
        if let Some(n) = p_opt(params, "name") { m.name = n; }
        if let Some(h) = p_opt(params, "harness") { m.harness = HarnessKind::parse(&h).ok_or_else(|| ApiError::invalid("unknown harness"))?; }
        if params.get("roles").is_some() { let roles = p_list(params, "roles"); if !roles.is_empty() { m.roles = roles; } }
        if params.get("model").is_some() { m.model_id = p_opt(params, "model"); }
        if params.get("mode").is_some() { m.mode = p_opt(params, "mode"); }
        let by = p_opt(params, "by").unwrap_or_else(|| "user".into());
        if let Some(e) = params.get("enabled").and_then(Value::as_bool).filter(|e| *e != m.enabled) {
            m.enabled = e;
            m.switched = Some(Switched { enabled: e, by: by.clone(), at: now() });
        }
        store.put_member(&m)?;
        self.emit_locked(&store, Event::new("member.updated", "user").with(json!({"memberId": m.id, "harness": m.harness, "model": m.model_id, "roles": m.roles, "enabled": m.enabled, "by": by})));
        Ok(serde_json::to_value(m).unwrap())
    }

    /// The chat with the orchestrator, from its events.
    fn conversation(&self, limit: usize) -> Res<Vec<crate::orchestrator::ChatLine>> {
        let events = self.store.lock().unwrap().events_of_kind("orchestrator.", limit)?;
        Ok(events.into_iter().filter_map(|ev| {
            let author = match ev.kind.as_str() { "orchestrator.heard" => "user", "orchestrator.replied" => "orchestrator", _ => return None };
            Some(crate::orchestrator::ChatLine { author: author.into(), text: ev.payload["text"].as_str().unwrap_or("").into(), quest_id: ev.quest_id.clone(), at: ev.occurred_at })
        }).collect())
    }

    /// The user says something to the guild. The orchestrator answers on a
    /// thread; a plan in its answer becomes a quest that starts at once.
    fn orchestrator_say(self: &Arc<Service>, params: &Value) -> ApiResult {
        let text = p_str(params, "text")?.trim().to_string();
        let (project, member) = {
            let store = self.store.lock().unwrap();
            let projects = store.projects()?;
            let project = match p_opt(params, "projectId") {
                Some(id) => projects.into_iter().find(|p| p.id == id),
                None => projects.into_iter().next(),
            }.ok_or_else(|| ApiError::not_found("the guild has no project yet"))?;
            let member = crate::orchestrator::pick(&store.members()?, &self.available_harnesses()).ok_or_else(|| ApiError::conflict("no member can orchestrate: none is enabled on an available harness"))?;
            (project, member)
        };
        {
            let mut busy = self.orchestrating.lock().unwrap();
            if busy.is_some() { return Err(ApiError::conflict("the orchestrator is still answering")); }
            *busy = Some(member.id.clone());
        }
        let history = self.conversation(40)?;
        let mut heard = Event::new("orchestrator.heard", "user").with(json!({"text": text, "member": member.name}));
        heard.project_id = Some(project.id.clone());
        self.emit(heard);
        let service = self.clone();
        let who = member.id.clone();
        std::thread::Builder::new().name("orchestrator".into()).spawn(move || {
            let reply = service.orchestrate(&project, &member, &history, &text);
            *service.orchestrating.lock().unwrap() = None;
            let (text, quest_id) = match reply {
                Ok(r) => r,
                Err(e) => (format!("(no pude responder: {e})"), None),
            };
            let mut ev = Event::new("orchestrator.replied", "harness").with(json!({"text": text, "member": member.name}));
            ev.project_id = Some(project.id.clone());
            ev.quest_id = quest_id;
            service.emit(ev);
        }).map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(json!({"orchestrator": who}))
    }

    /// One turn of a member outside any worktree, in a scratch folder of the
    /// project's (it may read the repository): its whole answer.
    fn one_turn(&self, project: &Project, member: &Member, role_prompt: &str, prompt: &str, limit: Duration) -> Res<String> {
        let cwd = crate::paths::home().join("orchestrator").join(&project.id);
        std::fs::create_dir_all(&cwd).map_err(|e| ApiError::internal(e.to_string()))?;
        let adapter = self.adapter(member.harness.clone())?;
        let options = SessionOptions { cwd, model_id: member.model_id.clone(), mode: member.mode.clone(), role_prompt: role_prompt.into() };
        let session = adapter.create_session(&options).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let handle = adapter.prompt(&session, &new_id(), prompt).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let started = Instant::now();
        let mut text = String::new();
        let outcome = loop {
            while let Ok(ev) = handle.events.try_recv() { if let HarnessEvent::Text(t) = ev { text.push_str(&t); } }
            match handle.result.recv_timeout(Duration::from_millis(200)) {
                Ok(outcome) => break outcome,
                Err(RecvTimeoutError::Disconnected) => break TurnOutcome::Failed { reason: "the harness ended without a result".into() },
                Err(RecvTimeoutError::Timeout) if started.elapsed() > limit => {
                    let _ = adapter.interrupt(&session);
                    break TurnOutcome::Failed { reason: "timed out".into() };
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        };
        while let Ok(ev) = handle.events.try_recv() { if let HarnessEvent::Text(t) = ev { text.push_str(&t); } }
        let _ = adapter.stop(&session);
        if let TurnOutcome::Failed { reason } = outcome { return Err(ApiError::new("harness", reason)); }
        Ok(text)
    }

    /// One turn of the orchestrator, outside any worktree: it reads the
    /// repository but works in a scratch folder of its own.
    fn orchestrate(self: &Arc<Service>, project: &Project, member: &Member, history: &[crate::orchestrator::ChatLine], message: &str) -> Res<(String, Option<String>)> {
        let (members, design) = {
            let store = self.store.lock().unwrap();
            (store.members()?, store.design(&project.id)?)
        };
        let snap = self.snapshot()?;
        let prompt = crate::orchestrator::compose(project, &members, &snap, design.as_ref(), history, message);
        let text = self.one_turn(project, member, crate::orchestrator::ROLE_PROMPT, &prompt, self.settings.attempt_timeout)?;
        let (reply, plan) = crate::orchestrator::parse(&text);
        let Some(plan) = plan else { return Ok((reply, None)) };
        let quest = self.quest_create(&json!({"projectId": project.id, "title": plan.title, "description": plan.description}))?;
        let quest_id = quest["id"].as_str().unwrap_or_default().to_string();
        // Task ids are global: the plan's own ids get the quest's suffix.
        let suffix = &quest_id[quest_id.len().saturating_sub(8)..];
        let id = |t: &str| format!("{suffix}-{t}");
        for t in &plan.tasks {
            let role = crate::orchestrator::role_for(&t.role, &members);
            let deps: Vec<String> = t.depends_on.iter().filter(|d| plan.tasks.iter().any(|x| &x.id == *d)).map(|d| id(d)).collect();
            self.task_create(&json!({"questId": quest_id, "id": id(&t.id), "title": t.title, "instructions": t.instructions, "acceptance": t.acceptance, "role": role, "dependsOn": deps}))?;
        }
        self.quest_start(&json!({"questId": quest_id}))?;
        Ok((reply, Some(quest_id)))
    }

    /// Close the guild without losing anything: running quests pause, the
    /// attempts at work are cancelled (candidates waiting for review stay),
    /// and the members are disabled, so its hall leaves the dungeon.
    /// `guild.reopen` brings the members back; the quests resume from the board.
    fn guild_close(self: &Arc<Service>) -> ApiResult {
        let (running, active, members) = {
            let store = self.store.lock().unwrap();
            let running: Vec<String> = store.quests()?.into_iter().filter(|q| q.state == QuestState::Running).map(|q| q.id).collect();
            let active: Vec<String> = store.active_attempts()?.into_iter().filter(|a| !matches!(a.state, AttemptState::Submitted | AttemptState::Verifying)).map(|a| a.id).collect();
            (running, active, store.members()?)
        };
        for q in &running { let _ = self.quest_set(&json!({"questId": q}), QuestState::Paused, &[QuestState::Running]); }
        for a in &active { let _ = self.attempt_cancel(a); }
        let closed: Vec<String> = members.iter().filter(|m| m.enabled).map(|m| m.id.clone()).collect();
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            for mut m in members.iter().filter(|m| m.enabled).cloned() {
                m.enabled = false;
                store.put_member(&m)?;
            }
            // Who it closed, so reopening does not bring back a member retired earlier.
            self.emit_locked(&store, Event::new("guild.closed", "user").with(json!({"members": closed, "pausedQuests": running.len(), "cancelledAttempts": active.len()})));
            Ok(())
        })?;
        Ok(json!({"pausedQuests": running, "cancelledAttempts": active}))
    }

    fn guild_reopen(&self) -> ApiResult {
        let store = self.store.lock().unwrap();
        let closed: Vec<String> = store.events_of_kind("guild.closed", 1)?.first()
            .and_then(|e| e.payload["members"].as_array().cloned()).unwrap_or_default()
            .iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let back: Vec<Member> = store.members()?.into_iter().filter(|m| !m.enabled && closed.contains(&m.id)).collect();
        self.atomic(&store, || {
            for mut m in back.iter().cloned() {
                m.enabled = true;
                store.put_member(&m)?;
            }
            self.emit_locked(&store, Event::new("guild.reopened", "user").with(json!({"members": back.len()})));
            Ok(())
        })?;
        Ok(json!({"members": back.len()}))
    }

    /// Remove a member. One with an attempt still going is retired instead
    /// (disabled, its attempts cancelled): the attempt's record and its
    /// review still need the member, and a later remove deletes it.
    fn member_remove(&self, id: &str) -> ApiResult {
        let active: Vec<String> = {
            let store = self.store.lock().unwrap();
            store.member(id)?.ok_or_else(|| ApiError::not_found(format!("member {id}")))?;
            store.active_attempts()?.into_iter().filter(|a| a.member_id == id).map(|a| a.id).collect()
        };
        if active.is_empty() {
            let store = self.store.lock().unwrap();
            store.delete_member(id)?;
            self.emit_locked(&store, Event::new("member.removed", "user").with(json!({"memberId": id})));
            return Ok(json!({"removed": id}));
        }
        for attempt in &active { let _ = self.attempt_cancel(attempt); }
        let store = self.store.lock().unwrap();
        if let Some(mut m) = store.member(id)? {
            m.enabled = false;
            store.put_member(&m)?;
        }
        self.emit_locked(&store, Event::new("member.updated", "user").with(json!({"memberId": id, "enabled": false})));
        Ok(json!({"retired": id, "cancelling": active}))
    }

    fn design_template(&self, params: &Value) -> ApiResult {
        let id = p_str(params, "projectId")?;
        let name = p_opt(params, "template").unwrap_or_else(|| "videogame".into());
        let store = self.store.lock().unwrap();
        let project = store.project(id)?.ok_or_else(|| ApiError::not_found(format!("project {id}")))?;
        let mut d = crate::design::template(id, &project.name, &name);
        d.version = store.design(id)?.map(|x| x.version + 1).unwrap_or(1);
        store.put_design(&d)?;
        self.emit_locked(&store, Event::new("design.updated", "user").with(json!({"projectId": id, "version": d.version, "template": name})));
        Ok(serde_json::to_value(d).unwrap())
    }

    fn design_set(&self, params: &Value) -> ApiResult {
        let id = p_str(params, "projectId")?;
        let mut d: crate::design::GameDesign = serde_json::from_value(params.get("design").cloned().ok_or_else(|| ApiError::invalid("missing design"))?).map_err(|e| ApiError::invalid(format!("design: {e}")))?;
        let store = self.store.lock().unwrap();
        store.project(id)?.ok_or_else(|| ApiError::not_found(format!("project {id}")))?;
        d.project_id = id.into();
        let previous = store.design(id)?;
        // A design sent without its doc keeps the one it had.
        if d.doc.is_none() { d.doc = previous.as_ref().and_then(|x| x.doc.clone()); }
        d.version = previous.map(|x| x.version + 1).unwrap_or(1);
        d.updated_at = now();
        for (i, l) in d.loops.iter_mut().enumerate() { if l.id.trim().is_empty() { l.id = format!("loop-{}", i + 1); } }
        for (i, p) in d.parts.iter_mut().enumerate() { if p.id.trim().is_empty() { p.id = format!("part-{}", i + 1); } }
        crate::design::check_production(&d).map_err(ApiError::invalid)?;
        store.put_design(&d)?;
        self.emit_locked(&store, Event::new("design.updated", "user").with(json!({"projectId": id, "version": d.version, "loops": d.loops.len(), "parts": d.parts.len()})));
        Ok(serde_json::to_value(d).unwrap())
    }

    /// How many members work at once, from 1 to 16; kept between runs.
    fn set_concurrency(&self, params: &Value) -> ApiResult {
        let n = params.get("concurrency").and_then(Value::as_u64).filter(|n| (1..=16).contains(n)).ok_or_else(|| ApiError::invalid("concurrency must be 1 to 16"))? as usize;
        let store = self.store.lock().unwrap();
        store.set_meta("concurrency", &n.to_string())?;
        self.concurrency.store(n, Ordering::Relaxed);
        self.emit_locked(&store, Event::new("guild.concurrency", "user").with(json!({"concurrency": n})));
        Ok(json!({"concurrency": n}))
    }

    /// The project an attempt works in: its task's quest's.
    fn project_of_attempt(&self, attempt: &Attempt) -> Option<Project> {
        let store = self.store.lock().unwrap();
        let task = store.task(&attempt.task_id).ok()??;
        let quest = store.quest(&task.quest_id).ok()??;
        store.project(&quest.project_id).ok()?
    }

    // ----- the visual direction

    /// Change a project's design in place (one made if it has none yet),
    /// as a new version, and say so.
    fn change_design(&self, project_id: &str, f: impl FnOnce(&mut crate::design::GameDesign) -> Res<Value>) -> ApiResult {
        let store = self.store.lock().unwrap();
        let project = store.project(project_id)?.ok_or_else(|| ApiError::not_found(format!("project {project_id}")))?;
        let mut d = store.design(project_id)?.unwrap_or_else(|| crate::design::LivingGameDesign { project_id: project_id.into(), title: project.name.clone(), ..Default::default() });
        let out = f(&mut d)?;
        d.version += 1;
        d.updated_at = now();
        store.put_design(&d)?;
        self.emit_locked(&store, Event::new("design.updated", "user").with(json!({"projectId": project_id, "version": d.version, "visual": true})));
        Ok(out)
    }

    /// Add a file to the visual direction: an image (the guild keeps a copy
    /// and its dominant colours) or a palette file (`.gpl`, `.hex`, `.pal`).
    fn visual_add(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?.to_string();
        let path = PathBuf::from(p_str(params, "path")?);
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| name.clone());
        if crate::design::PALETTE_EXTENSIONS.contains(&ext.as_str()) {
            let text = std::fs::read_to_string(&path).map_err(|e| ApiError::invalid(format!("{}: {e}", path.display())))?;
            let colors = crate::design::parse_palette(&text);
            if colors.is_empty() { return Err(ApiError::invalid(format!("{name} has no colours"))); }
            return self.visual_palette(&json!({"projectId": project_id, "name": p_opt(params, "caption").unwrap_or(stem), "colors": colors}));
        }
        if !crate::design::IMAGE_EXTENSIONS.contains(&ext.as_str()) { return Err(ApiError::invalid(format!("{name}: not an image nor a palette"))); }
        let bytes = std::fs::read(&path).map_err(|e| ApiError::invalid(format!("{}: {e}", path.display())))?;
        let decoded = image::load_from_memory(&bytes).map_err(|e| ApiError::invalid(format!("{name}: {e}")))?.to_rgba8();
        let colors = crate::design::dominant_colors(&decoded, 6);
        let caption = p_opt(params, "caption").unwrap_or_default();
        let kind = p_opt(params, "kind").unwrap_or_else(|| "reference".into());
        self.change_design(&project_id, |d| {
            let id = crate::design::next_id("IMG", d.visual.images.iter().map(|i| i.id.clone()));
            let dir = crate::paths::visual_dir(&d.project_id);
            std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
            let file = dir.join(format!("{}.{ext}", id.to_lowercase()));
            std::fs::write(&file, &bytes).map_err(|e| ApiError::internal(e.to_string()))?;
            let image = crate::design::VisualImage { id, file: file.display().to_string(), name, caption, kind, colors };
            d.visual.images.push(image.clone());
            Ok(serde_json::to_value(image).unwrap())
        })
    }

    /// Add a palette, or replace the one with that id.
    fn visual_palette(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?.to_string();
        let name = p_str(params, "name")?.to_string();
        let colors: Vec<String> = p_list(params, "colors").iter().flat_map(|c| crate::design::parse_palette(c)).collect();
        if colors.is_empty() { return Err(ApiError::invalid("a palette needs colours (#rrggbb)")); }
        let id = p_opt(params, "id");
        self.change_design(&project_id, |d| {
            let palette = crate::design::Palette { id: id.clone().unwrap_or_else(|| crate::design::next_id("PAL", d.visual.palettes.iter().map(|p| p.id.clone()))), name, colors };
            match d.visual.palettes.iter_mut().find(|p| p.id == palette.id) { Some(p) => *p = palette.clone(), None => d.visual.palettes.push(palette.clone()) }
            Ok(serde_json::to_value(palette).unwrap())
        })
    }

    /// An image's caption or kind.
    fn visual_update(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?.to_string();
        let id = p_str(params, "id")?.to_string();
        self.change_design(&project_id, |d| {
            let image = d.visual.images.iter_mut().find(|i| i.id == id).ok_or_else(|| ApiError::not_found(format!("image {id}")))?;
            if let Some(c) = params.get("caption").and_then(Value::as_str) { image.caption = c.trim().into(); }
            if let Some(k) = p_opt(params, "kind") { image.kind = k; }
            Ok(serde_json::to_value(image.clone()).unwrap())
        })
    }

    /// Take an image (and the guild's copy of it) or a palette out.
    fn visual_remove(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?.to_string();
        let id = p_str(params, "id")?.to_string();
        self.change_design(&project_id, |d| {
            if let Some(i) = d.visual.images.iter().position(|i| i.id == id) {
                let image = d.visual.images.remove(i);
                if Path::new(&image.file).starts_with(crate::paths::visual_dir(&d.project_id)) { let _ = std::fs::remove_file(&image.file); }
            } else if let Some(i) = d.visual.palettes.iter().position(|p| p.id == id) {
                d.visual.palettes.remove(i);
            } else {
                return Err(ApiError::not_found(format!("{id} in the visual direction")));
            }
            Ok(json!({"removed": id}))
        })
    }

    /// Keep the design in a Claude Doc (its link), or stop (no url): the
    /// app's Design and Visual tabs then point to it instead of editing.
    fn design_link(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?.to_string();
        let url = p_opt(params, "url").map(|u| u.trim().to_string());
        if let Some(u) = &url { if !crate::design::is_doc_url(u) { return Err(ApiError::invalid(format!("not a Claude Doc link: {u}"))); } }
        let by = p_opt(params, "by").unwrap_or_else(|| "user".into());
        self.change_design(&project_id, |d| {
            d.doc = url.map(|url| crate::design::DesignDoc { url, linked_by: by, linked_at: now(), synced_rev: None, synced_at: None, synced_version: None });
            Ok(json!({"doc": d.doc}))
        })
    }

    /// The guild's copy now reflects the doc up to this revision.
    fn design_synced(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?.to_string();
        let rev = params.get("rev").and_then(Value::as_u64).ok_or_else(|| ApiError::invalid("missing rev"))?;
        self.change_design(&project_id, |d| {
            let doc = d.doc.as_mut().ok_or_else(|| ApiError::conflict("the design has no doc: link it first"))?;
            doc.synced_rev = Some(rev);
            doc.synced_at = Some(now());
            // This change is the sync itself: the version it makes is the doc's.
            doc.synced_version = Some(d.version + 1);
            Ok(json!({"doc": d.doc}))
        })
    }

    fn visual_notes(&self, params: &Value) -> ApiResult {
        let project_id = p_str(params, "projectId")?.to_string();
        let notes = params.get("notes").and_then(Value::as_str).unwrap_or("").trim().to_string();
        self.change_design(&project_id, |d| { d.visual.notes = notes; Ok(json!({"notes": d.visual.notes})) })
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
            switched: None,
        };
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            store.put_member(&member)?;
            self.emit_locked(&store, Event::new("member.added", "user").with(json!({"memberId": member.id, "harness": member.harness, "model": member.model_id})));
            Ok(())
        })?;
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
        self.atomic(&store, || {
            store.put_quest(&quest)?;
            self.emit_locked(&store, Event::new("quest.created", "user").about_quest(&quest).with(json!({"title": quest.title})));
            Ok(())
        })?;
        Ok(serde_json::to_value(quest).unwrap())
    }

    fn task_create(&self, params: &Value) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        let store = self.store.lock().unwrap();
        let quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
        let acceptance = parse_acceptance(params).unwrap_or_default();
        let state = if quest.state == QuestState::Draft { TaskState::Draft } else { TaskState::Pending };
        let strategy = parse_strategy(params)?;
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
            // A race needs one attempt per runner before any retry counts.
            max_attempts: (params.get("maxAttempts").and_then(Value::as_u64).unwrap_or(2) as u32).max(strategy.runners()),
            candidates: strategy.runners(),
            auto_integrate: strategy.auto_integrate,
            timeout_minutes: params.get("timeoutMinutes").and_then(Value::as_u64).filter(|m| *m > 0).map(|m| m as u32),
            strategy,
            winner_attempt_id: None,
            notes: vec![],
            granted: Default::default(),
            version: 1,
            created_at: now(),
            updated_at: now(),
        };
        let mut all = store.tasks_of_quest(quest_id)?;
        all.retain(|t| t.id != task.id);
        all.push(task.clone());
        validate_dag(&all).map_err(ApiError::invalid)?;
        self.atomic(&store, || {
            store.put_task(&task)?;
            self.emit_locked(&store, Event::new("task.created", "user").about_task(&task, &quest.project_id).with(json!({"title": task.title, "dependsOn": task.depends_on})));
            Ok(())
        })?;
        Ok(serde_json::to_value(task).unwrap())
    }

    /// Change a task nobody has started yet: its title, instructions,
    /// acceptance, dependencies, role, pinned member or priority. New
    /// dependencies are checked like a new task's and make it wait for them.
    fn task_update(&self, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        let store = self.store.lock().unwrap();
        let mut task = store.task(task_id)?.ok_or_else(|| ApiError::not_found(format!("task {task_id}")))?;
        if !matches!(task.state, TaskState::Draft | TaskState::Pending | TaskState::Ready) || task.attempts_made > 0 {
            return Err(ApiError::conflict(format!("task {task_id} is {:?}: only a task not yet started can change", task.state)));
        }
        let quest = store.quest(&task.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
        if let Some(t) = p_opt(params, "title") { task.title = t; }
        if let Some(t) = params.get("instructions").and_then(Value::as_str) { task.instructions = t.trim().to_string(); }
        if let Some(a) = parse_acceptance(params) { task.acceptance = a; }
        if let Some(r) = p_opt(params, "role") { task.role = r; }
        if params.get("memberId").is_some() { task.member_id = p_opt(params, "memberId"); }
        if let Some(p) = params.get("priority").and_then(Value::as_i64) { task.priority = p as i32; }
        if let Some(m) = params.get("timeoutMinutes").and_then(Value::as_u64) { task.timeout_minutes = (m > 0).then_some(m as u32); }
        let deps_changed = params.get("dependsOn").is_some_and(|d| d.is_array()) && p_list(params, "dependsOn") != task.depends_on;
        if deps_changed {
            task.depends_on = p_list(params, "dependsOn");
            let mut all = store.tasks_of_quest(&task.quest_id)?;
            if let Some(missing) = task.depends_on.iter().find(|d| !all.iter().any(|t| &t.id == *d)) { return Err(ApiError::invalid(format!("no task {missing} in this quest"))); }
            all.retain(|t| t.id != task.id);
            all.push(task.clone());
            validate_dag(&all).map_err(ApiError::invalid)?;
            // Ready meant its old dependencies were done; the scheduler judges the new ones.
            if task.state == TaskState::Ready { task.state = TaskState::Pending; }
        }
        task.version += 1;
        task.updated_at = now();
        self.atomic(&store, || {
            store.put_task(&task)?;
            self.emit_locked(&store, Event::new("task.updated", "user").about_task(&task, &quest.project_id).with(json!({"title": task.title, "dependsOn": task.depends_on, "version": task.version})));
            Ok(())
        })?;
        Ok(serde_json::to_value(task).unwrap())
    }

    /// Add to a task that already started: the member working on it reads
    /// the note when its turn ends (one more turn in the same session), and
    /// a retry or the orchestrator read it with the instructions.
    fn task_note(&self, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        let text = p_str(params, "text")?.trim().to_string();
        let store = self.store.lock().unwrap();
        let mut task = store.task(task_id)?.ok_or_else(|| ApiError::not_found(format!("task {task_id}")))?;
        if matches!(task.state, TaskState::Completed | TaskState::Cancelled | TaskState::Integrating) {
            return Err(ApiError::conflict(format!("task {task_id} is {:?}: a note would reach nobody", task.state)));
        }
        let quest = store.quest(&task.quest_id)?.ok_or_else(|| ApiError::not_found("quest"))?;
        task.notes.push(text.clone());
        task.version += 1;
        task.updated_at = now();
        self.atomic(&store, || {
            store.put_task(&task)?;
            self.emit_locked(&store, Event::new("task.noted", p_opt(params, "by").as_deref().unwrap_or("user")).about_task(&task, &quest.project_id).with(json!({"text": text, "notes": task.notes.len()})));
            Ok(())
        })?;
        Ok(serde_json::to_value(task).unwrap())
    }

    /// Bring the project's target (or `from`) into a quest's branch, so the
    /// attempts that start from now on see what changed there — a corrected
    /// specification, say — instead of having it copied into every task.
    fn quest_sync(&self, params: &Value) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        let (quest, project) = {
            let store = self.store.lock().unwrap();
            let quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
            let project = store.project(&quest.project_id)?.ok_or_else(|| ApiError::not_found("project"))?;
            (quest, project)
        };
        let from = p_opt(params, "from").unwrap_or_else(|| project.target_ref.clone());
        let (old, new, files) = workspaces::sync_quest(&project, &quest, &from).map_err(ApiError::conflict)?;
        if old != new {
            let mut ev = Event::new("quest.synced", "user").with(json!({"questId": quest.id, "from": from, "oldSha": old, "newSha": new, "files": files}));
            ev.project_id = Some(project.id.clone());
            ev.quest_id = Some(quest.id.clone());
            self.emit(ev);
        }
        Ok(json!({"questId": quest.id, "from": from, "oldSha": old, "newSha": new, "files": files, "changed": old != new}))
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
            self.atomic(&store, || {
                quest.base_sha = Some(base.clone());
                quest.state = QuestState::Running;
                quest.updated_at = now();
                store.put_quest(&quest)?;
                for t in tasks.iter().filter(|t| t.state == TaskState::Draft) {
                    self.move_task_locked(&store, &t.id, TaskState::Pending, None)?;
                }
                self.emit_locked(&store, Event::new("quest.started", "user").about_quest(&quest).with(json!({"title": quest.title, "baseSha": base, "tasks": tasks.len()})));
                Ok(())
            })?;
        }
        self.tick();
        Ok(serde_json::to_value(self.store.lock().unwrap().quest(quest_id)?).unwrap())
    }

    fn quest_set(&self, params: &Value, to: QuestState, from: &[QuestState]) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        let store = self.store.lock().unwrap();
        let mut quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
        if !from.contains(&quest.state) { return Err(ApiError::conflict(format!("quest is {:?}", quest.state))); }
        self.atomic(&store, || {
            quest.state = to;
            quest.updated_at = now();
            store.put_quest(&quest)?;
            let kind = if to == QuestState::Paused { "quest.paused" } else { "quest.resumed" };
            self.emit_locked(&store, Event::new(kind, "user").about_quest(&quest).with(json!({"title": quest.title})));
            Ok(())
        })?;
        Ok(serde_json::to_value(quest).unwrap())
    }

    fn quest_cancel(&self, params: &Value) -> ApiResult {
        let quest_id = p_str(params, "questId")?;
        let (tasks, active) = {
            let store = self.store.lock().unwrap();
            let mut quest = store.quest(quest_id)?.ok_or_else(|| ApiError::not_found(format!("quest {quest_id}")))?;
            self.atomic(&store, || {
                quest.state = QuestState::Cancelled;
                quest.updated_at = now();
                store.put_quest(&quest)?;
                self.emit_locked(&store, Event::new("quest.cancelled", "user").about_quest(&quest).with(json!({"title": quest.title})));
                Ok(())
            })?;
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
        // A candidate still waiting keeps its member busy: retrying it is
        // rejecting it, so the new attempt can start.
        if self.latest_submitted(task_id).is_ok() {
            {
                let store = self.store.lock().unwrap();
                let mut task = store.task(task_id)?.ok_or_else(|| ApiError::not_found(format!("task {task_id}")))?;
                if task.attempts_made >= task.max_attempts { task.max_attempts = task.attempts_made + 1; task.version += 1; task.updated_at = now(); store.put_task(&task)?; }
            }
            let reason = p_opt(params, "reason").unwrap_or_else(|| "retried by user".into());
            let mut reject = json!({"taskId": task_id, "reason": reason});
            if let Some(id) = p_opt(params, "attemptId") { reject["attemptId"] = json!(id); }
            self.task_reject(&reject)?;
            return Ok(serde_json::to_value(self.store.lock().unwrap().task(task_id)?).unwrap());
        }
        {
            let store = self.store.lock().unwrap();
            let mut task = store.task(task_id)?.ok_or_else(|| ApiError::not_found(format!("task {task_id}")))?;
            if !matches!(task.state, TaskState::Failed | TaskState::Blocked | TaskState::Cancelled | TaskState::Verifying | TaskState::Accepted) {
                return Err(ApiError::conflict(format!("task is {:?}", task.state)));
            }
            self.atomic(&store, || {
                if task.attempts_made >= task.max_attempts { task.max_attempts = task.attempts_made + 1; }
                task.version += 1;
                task.updated_at = now();
                store.put_task(&task)?;
                self.move_task_locked(&store, task_id, TaskState::Ready, None)?;
                Ok(())
            })?;
        }
        self.tick();
        Ok(serde_json::to_value(self.store.lock().unwrap().task(task_id)?).unwrap())
    }

    fn task_cancel(&self, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        let active = self.store.lock().unwrap().active_attempts()?;
        for a in active.iter().filter(|a| a.task_id == task_id) {
            // A candidate still waiting goes with its task, or its member stays busy (FB-035).
            if matches!(a.state, AttemptState::Submitted | AttemptState::Verifying) && !self.running.lock().unwrap().contains_key(&a.id) {
                let _ = self.move_attempt(&a.id, AttemptState::Cancelled, Some("its task was cancelled".into()));
                continue;
            }
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
            let mut running = self.running.lock().unwrap();
            let r = running.get_mut(attempt_id).ok_or_else(|| ApiError::conflict("the attempt is not running"))?;
            if let Some(since) = r.waiting_since.take() { r.waited += since.elapsed(); }
            (r.session.clone(), r.harness.clone())
        };
        let adapter = self.adapter(harness)?;
        adapter.respond_permission(&session, request_id, option.as_deref()).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let by = p_opt(params, "by").unwrap_or_else(|| "user".into());
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            let mut a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
            // A command the orchestrator or the user allowed is allowed for the
            // rest of the task as it is; one the user allowed always, by program.
            if let (false, Some(p)) = (by.starts_with("policy"), a.pending_permission.as_ref().filter(|p| p.request_id == request_id)) {
                let kind = p.options.iter().find(|o| Some(&o.id) == option.as_ref()).map(|o| o.kind.as_str()).unwrap_or("");
                if let (true, Some(cmd), Some(mut task)) = (kind.starts_with("allow"), crate::policy::command_of(&p.title, &p.detail), store.task(&a.task_id)?) {
                    let before = task.granted.clone();
                    if !task.granted.commands.contains(&cmd) && task.granted.commands.len() < 200 { task.granted.commands.push(cmd.clone()); }
                    if kind == "allow_always" && by != "orchestrator" {
                        for program in crate::policy::trusted_programs(&cmd) { if !task.granted.programs.contains(&program) { task.granted.programs.push(program); } }
                    }
                    if task.granted != before { task.updated_at = now(); store.put_task(&task)?; }
                }
            }
            a.pending_permission = None;
            a.escalation = None;
            a.updated_at = now();
            store.put_attempt(&a)?;
            let _ = self.move_attempt_locked(&store, attempt_id, AttemptState::Running, None);
            self.emit_locked(&store, Event::new("permission.resolved", if by.starts_with("policy") { "policy" } else if by == "orchestrator" { "orchestrator" } else { "user" }).about_attempt(&a).with(json!({"requestId": request_id, "optionId": option, "by": by})));
            Ok(())
        })?;
        Ok(json!({"responded": request_id}))
    }

    fn latest_submitted(&self, task_id: &str) -> Res<Attempt> {
        let store = self.store.lock().unwrap();
        store
            .attempts_of_task(task_id)?
            .into_iter()
            .filter(|a| a.kind == AttemptKind::Work && matches!(a.state, AttemptState::Submitted | AttemptState::Verifying))
            .max_by(|a, b| a.created_at.cmp(&b.created_at))
            .ok_or_else(|| ApiError::conflict("the task has no submitted candidate"))
    }

    fn task_accept(self: &Arc<Service>, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        if params.get("override").and_then(Value::as_bool).unwrap_or(false) {
            let id = p_opt(params, "attemptId").ok_or_else(|| ApiError::invalid("override needs the attempt to accept"))?;
            self.revive_candidate(task_id, &id)?;
        }
        let attempt = match p_opt(params, "attemptId") { Some(id) => id, None => self.latest_submitted(task_id)?.id };
        {
            let store = self.store.lock().unwrap();
            let task = store.task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            let submitted: Vec<Attempt> = store.attempts_of_task(task_id)?.into_iter().filter(|a| a.kind == AttemptKind::Work && matches!(a.state, AttemptState::Submitted | AttemptState::Verifying)).collect();
            let refs: Vec<&Attempt> = submitted.iter().collect();
            let earlier = store.decisions_of_task(task_id)?;
            let override_ = earlier.iter().any(|d| d.winner_attempt_id.as_deref() != Some(attempt.as_str()));
            if let Some(w) = submitted.iter().find(|a| a.id == attempt) {
                let d = RaceDecision { id: new_id(), task_id: task_id.into(), quest_id: task.quest_id.clone(), project_id: w.project_id.clone(), by: p_opt(params, "by").unwrap_or_else(|| "user".into()), judge_attempt_id: None,
                    candidates: submitted.iter().map(|a| decision_candidate(a, &label_for(&refs, a))).collect(), scores: json!({}), findings: json!({}), tradeoffs: String::new(),
                    winner_attempt_id: Some(w.id.clone()), confidence: None, reason: p_opt(params, "reason").unwrap_or_else(|| "chosen by the user".into()), human_override: override_, created_at: now() };
                drop(store);
                self.record_decision(&d)?;
            }
        }
        let attempt = self.select_candidate(task_id, &attempt, "user")?;
        if params.get("integrate").and_then(Value::as_bool).unwrap_or(false) {
            return self.task_integrate(task_id);
        }
        Ok(json!({"accepted": task_id, "attemptId": attempt.id}))
    }

    /// The user overrides a verdict: an attempt that passed its required
    /// verifiers but was failed (by the orchestrator, a judge or a reject)
    /// is a candidate again. A retry already running is cancelled first.
    fn revive_candidate(&self, task_id: &str, attempt_id: &str) -> Res<()> {
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            let mut a = store.attempt(attempt_id)?.filter(|a| a.task_id == task_id && a.kind == AttemptKind::Work).ok_or_else(|| ApiError::not_found(format!("attempt {attempt_id} of {task_id}")))?;
            if a.state != AttemptState::Failed { return Ok(()); }
            if !passes_gates(&a) { return Err(ApiError::conflict("that attempt did not pass its required verifiers: only a verified candidate can be accepted over a failure")); }
            if store.attempts_of_task(task_id)?.iter().any(|o| o.kind == AttemptKind::Work && o.state.is_active() && !matches!(o.state, AttemptState::Submitted | AttemptState::Verifying)) {
                return Err(ApiError::conflict("a retry of this task is running: cancel it first (guild cancel <attempt>)"));
            }
            let failure = a.failure.take();
            a.state = AttemptState::Submitted;
            a.finished_at = None;
            a.updated_at = now();
            store.put_attempt(&a)?;
            self.emit_locked(&store, Event::new("candidate.revived", "user").about_attempt(&a).with(json!({"failure": failure})));
            if store.task(task_id)?.is_some_and(|t| t.state != TaskState::Verifying) { self.move_task_locked(&store, task_id, TaskState::Verifying, None)?; }
            Ok(())
        })
    }

    fn task_reject(self: &Arc<Service>, params: &Value) -> ApiResult {
        let task_id = p_str(params, "taskId")?;
        let reason = p_opt(params, "reason").unwrap_or_else(|| "rejected by user".into());
        let attempt = match p_opt(params, "attemptId") {
            Some(id) => self.store.lock().unwrap().attempt(&id)?.filter(|a| a.task_id == task_id && matches!(a.state, AttemptState::Submitted | AttemptState::Verifying)).ok_or_else(|| ApiError::conflict("that attempt has no candidate to reject"))?,
            None => self.latest_submitted(task_id)?,
        };
        self.move_attempt(&attempt.id, AttemptState::Failed, Some(reason.clone()))?;
        self.emit(Event::new("candidate.rejected", "user").about_attempt(&attempt).with(json!({"reason": reason})));
        let task = self.store.lock().unwrap().task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
        // A cancelled task stays cancelled: rejecting only lets its member go.
        if matches!(task.state, TaskState::Cancelled | TaskState::Completed) { return Ok(json!({"rejected": task_id, "attemptId": attempt.id})); }
        // In a race, other candidates may still be waiting for the user.
        if self.latest_submitted(task_id).is_ok() { return Ok(json!({"rejected": task_id, "attemptId": attempt.id, "remaining": true})); }
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
            let a = store.attempts_of_task(task_id)?.into_iter().filter(|a| a.kind == AttemptKind::Work && a.state == AttemptState::Passed).max_by(|a, b| a.created_at.cmp(&b.created_at)).ok_or_else(|| ApiError::conflict("no accepted attempt"))?;
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
                {
                    let store = self.store.lock().unwrap();
                    self.atomic(&store, || {
                        self.emit_locked(&store, Event::new("merge.completed", "guild").about_attempt(&attempt).with(json!({"integrationRef": quest.integration_ref, "sha": sha})));
                        self.move_task_locked(&store, task_id, TaskState::Completed, None)?;
                        Ok(())
                    })?;
                }
                self.tick();
                Ok(json!({"completed": task_id, "integrated": true, "sha": sha}))
            }
            Err(e) => {
                let store = self.store.lock().unwrap();
                self.atomic(&store, || {
                    self.emit_locked(&store, Event::new("merge.blocked", "guild").about_attempt(&attempt).with(json!({"reason": e})));
                    self.move_task_locked(&store, task_id, TaskState::Blocked, Some(format!("merge blocked: {e}")))?;
                    Ok(())
                })?;
                Err(ApiError::conflict(e))
            }
        }
    }
}

/// The hard gates: a candidate commit whose required verifiers all passed.
fn passes_gates(a: &Attempt) -> bool {
    a.candidate.is_some() && a.verifier_results.iter().filter(|r| r.required).all(|r| r.status == "passed")
}

/// The anonymous label of a candidate among its peers: A, B, C… in an
/// order that says nothing about who made it (sorted by a hash of the id).
fn label_for(candidates: &[&Attempt], a: &Attempt) -> String {
    let key = |id: &str| { let h = <sha2::Sha256 as sha2::Digest>::digest(id.as_bytes()); format!("{:x}", h) };
    let mut keys: Vec<(String, String)> = candidates.iter().map(|c| (key(&c.id), c.id.clone())).collect();
    keys.sort();
    let pos = keys.iter().position(|(_, id)| *id == a.id).unwrap_or(0);
    ((b'A' + (pos as u8 % 26)) as char).to_string()
}

fn decision_candidate(a: &Attempt, label: &str) -> DecisionCandidate {
    DecisionCandidate { label: label.into(), attempt_id: a.id.clone(), member_id: a.member_id.clone(), verifiers_passed: passes_gates(a), insertions: a.candidate.as_ref().map(|c| c.insertions).unwrap_or(0), deletions: a.candidate.as_ref().map(|c| c.deletions).unwrap_or(0), score: None }
}

/// The judge's answer: the last fenced JSON block, or the last balanced
/// object in the text, with a `winner`.
fn parse_judge_output(text: &str) -> Result<Value, String> {
    let mut candidates: Vec<&str> = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let body = &after[body_start..];
        let Some(end) = body.find("```") else { break };
        candidates.push(body[..end].trim());
        rest = &body[end + 3..];
    }
    if candidates.is_empty() {
        if let (Some(s), Some(e)) = (text.find('{'), text.rfind('}')) { if e > s { candidates.push(text[s..=e].trim()); } }
    }
    for c in candidates.iter().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(c) {
            if v.get("winner").and_then(Value::as_str).is_some() { return Ok(v); }
        }
    }
    Err("no JSON block with a winner".into())
}

/// `strategy: {type, candidates, selection, autoIntegrate}`, or the older
/// `candidates` / `autoIntegrate` flags, which mean a policy-selected race.
/// `acceptance` as strings or `{id, text}` objects; None when absent.
fn parse_acceptance(params: &Value) -> Option<Vec<AcceptanceCriterion>> {
    params.get("acceptance").and_then(Value::as_array).map(|a| {
        a.iter()
            .enumerate()
            .filter_map(|(i, v)| match v {
                Value::String(s) => Some(AcceptanceCriterion { id: format!("AC-{}", i + 1), text: s.clone() }),
                Value::Object(o) => Some(AcceptanceCriterion { id: o.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("AC-{}", i + 1)), text: o.get("text").and_then(Value::as_str).unwrap_or("").into() }),
                _ => None,
            })
            .collect()
    })
}

fn parse_strategy(params: &Value) -> Res<ExecutionStrategy> {
    if let Some(s) = params.get("strategy").filter(|s| !s.is_null()) {
        if let Some(text) = s.as_str() {
            let kind = match text { "single" => StrategyKind::Single, "reviewed" => StrategyKind::Reviewed, "race" | "candidate_race" => StrategyKind::CandidateRace, "experiment" => StrategyKind::Experiment, other => return Err(ApiError::invalid(format!("unknown strategy {other}"))) };
            let candidates = params.get("candidates").and_then(Value::as_u64).unwrap_or(if kind == StrategyKind::Single || kind == StrategyKind::Reviewed { 1 } else { 3 }).clamp(1, 8) as u32;
            let selection = match params.get("selection").and_then(Value::as_str) { Some("policy") => Selection::Policy, Some("judge") => Selection::Judge, Some("user") => Selection::User, Some(other) => return Err(ApiError::invalid(format!("unknown selection {other}"))), None => if kind == StrategyKind::Single { Selection::User } else { Selection::Policy } };
            let auto_integrate = params.get("autoIntegrate").and_then(Value::as_bool).unwrap_or(false);
            return Ok(ExecutionStrategy { kind, candidates, selection, auto_integrate });
        }
        let mut st: ExecutionStrategy = serde_json::from_value(s.clone()).map_err(|e| ApiError::invalid(format!("strategy: {e}")))?;
        st.candidates = st.candidates.clamp(1, 8);
        return Ok(st);
    }
    let candidates = params.get("candidates").and_then(Value::as_u64).unwrap_or(1).clamp(1, 8) as u32;
    let auto_integrate = params.get("autoIntegrate").and_then(Value::as_bool).unwrap_or(false);
    Ok(if candidates > 1 {
        ExecutionStrategy { kind: StrategyKind::CandidateRace, candidates, selection: Selection::Policy, auto_integrate }
    } else if auto_integrate {
        ExecutionStrategy { kind: StrategyKind::Single, candidates: 1, selection: Selection::Policy, auto_integrate }
    } else {
        ExecutionStrategy::default()
    })
}

/// What a retry's worktree starts with from the attempt before it.
#[derive(Clone, Debug, PartialEq)]
enum Carried {
    Nothing,
    /// The last candidate, turned down.
    Candidate,
    /// What the last attempt had written when it stopped.
    Partial,
    /// Its work did not apply on the current base: where it is.
    Left { branch: String, head: String, base: String },
}

fn compose_prompt(role_prompt: &str, task: &Task, workspace: &Workspace, design: Option<&crate::design::GameDesign>, feedback: &[String], continued: Carried) -> String {
    let mut s = String::new();
    s.push_str(role_prompt.trim());
    s.push_str("\n\n# Task: ");
    s.push_str(&task.title);
    s.push_str("\n\n");
    s.push_str(task.instructions.trim());
    if !task.notes.is_empty() {
        s.push_str("\n\n## Notes the lead added after the task started\n");
        for n in &task.notes { s.push_str(&format!("- {}\n", n.trim())); }
    }
    if !task.acceptance.is_empty() {
        s.push_str("\n\n## Acceptance criteria\n");
        for ac in &task.acceptance {
            s.push_str(&format!("- {}: {}\n", ac.id, ac.text));
        }
    }
    s.push_str(&format!(
        "\n## Rules\n- Your working directory is a dedicated git worktree on branch `{}`. Work only inside it.\n- Do not commit, push or switch branches; the guild commits your work when you finish.\n- Do not ask whether to proceed: do the work, then reply with a short summary of what changed, how you verified it, and anything left undone.\n- Scratch files (throwaway scripts, logs, data) go in `{}`, never in /tmp: it is yours and the guild deletes it when you finish.\n",
        workspace.branch, crate::paths::scratch_dir(&workspace.attempt_id).display()
    ));
    if !feedback.is_empty() {
        s.push_str("\n## Earlier attempts were turned down\n");
        for f in feedback { s.push_str(&format!("- {}\n", f.trim())); }
        match &continued {
            Carried::Candidate => s.push_str("Your worktree already holds the last candidate. Keep what was not objected to and make exactly the changes asked for above.\n"),
            Carried::Partial => s.push_str("The last attempt stopped before it finished; your worktree already holds what it had written. Go on from it — check what is there first — instead of starting over.\n"),
            Carried::Left { branch, head, base } => s.push_str(&format!("The last attempt's work did not apply on the current base. It is on branch `{branch}` (commit {head}): see it with `git diff {base} {head}` and `git show {head}:<path>`, and reuse it instead of starting over.\n")),
            Carried::Nothing => s.push_str("Avoid what was objected to above.\n"),
        }
    }
    if let Some(d) = design { s.push_str(&crate::design::prompt_context(d)); }
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
    fn captures_a_verifier_leaves_become_evidence_for_the_judge_and_the_orchestrator() {
        let home = TempHome::new();
        let repo = home.repo();
        // The verifier captures "the running game" into the folder the guild hands it.
        let capture = "import os; open(os.path.join(os.environ['GUILD_EVIDENCE'], 'menu.png'), 'wb').write(b'png'); open(os.path.join(os.environ['GUILD_EVIDENCE'], 'notes.txt'), 'w').write('not an image')";
        std::fs::create_dir_all(repo.join(".guild")).unwrap();
        std::fs::write(repo.join(".guild/guild.toml"), format!("[[verifiers]]\nid = \"captures\"\nargv = [\"{PYTHON}\", \"-c\", \"{capture}\"]\nrequired = false\n")).unwrap();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), cancel_timeout: Duration::from_millis(500), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        service.register(Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "hello.txt".into(), content: "hi\n".into() })));
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"], "model": "mock-fast"})).unwrap();
        let quest = service.handle("quest.create", &json!({"projectId": "p", "title": "Menu"})).unwrap();
        service.handle("task.create", &json!({"questId": quest["id"], "id": "t1", "title": "Draw the menu"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest["id"]})).unwrap();
        let snap = wait_for(&service, "verified", |s| s.attempts.first().map(|a| !a.verifier_results.is_empty()).unwrap_or(false));
        let v = &snap.attempts[0].verifier_results[0];
        assert_eq!(v.status, "passed", "{}", v.tail);
        // Only the image is evidence, it is kept on disk, and the store knows it.
        assert_eq!(v.evidence.len(), 1, "{:?}", v.evidence);
        assert!(v.evidence[0].ends_with("menu.png") && Path::new(&v.evidence[0]).exists());
        let kinds: Vec<String> = service.store.lock().unwrap().artifacts_of_attempt(&snap.attempts[0].id).unwrap().into_iter().map(|a| a.kind).collect();
        assert!(kinds.contains(&"evidence:menu.png".to_string()), "{kinds:?}");
        // A verifier that leaves nothing leaves no empty folder behind.
        let dirs: Vec<_> = std::fs::read_dir(crate::paths::artifacts_dir().join(&snap.attempts[0].id)).unwrap().flatten().filter(|e| e.path().is_dir()).collect();
        assert_eq!(dirs.len(), 1);
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

    /// A guild with a worker that asks for a non-routine permission and an
    /// orchestrator member ruling through `decide`.
    fn with_orchestrator(decide: impl Fn(&str) -> String + Send + Sync + 'static) -> (TempHome, Arc<Service>, String, Arc<MockHarness>) {
        let home = TempHome::new();
        let repo = home.repo();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), cancel_timeout: Duration::from_millis(500), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        let mock = Arc::new(MockHarness::new(MockBehaviour::AskPermission { file: "x.txt".into(), content: "x".into() }));
        mock.set_decider(decide);
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"]})).unwrap();
        service.handle("member.add", &json!({"id": "orin", "harness": "mock", "roles": ["orchestrator"]})).unwrap();
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Q"})).unwrap()["id"].as_str().unwrap().to_string();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Write x", "acceptance": ["x.txt exists"]})).unwrap();
        (home, service, quest_id, mock)
    }

    #[test]
    fn the_guilds_orchestrator_answers_the_members_while_nobody_else_does() {
        let (_home, service, quest_id, mock) = with_orchestrator(|prompt| {
            let decision = if prompt.contains("Decision needed: permission") { "allow" } else { "accept" };
            format!("```json\n{{\"decision\": \"{decision}\", \"reason\": \"inside its worktree, what the task asks\"}}\n```")
        });
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        // Nobody answers the permission nor accepts the candidate: the orchestrator does both.
        let snap = wait_for(&service, "integrated", |s| s.tasks.iter().find(|t| t.id == "t1").is_some_and(|t| t.state == TaskState::Completed));
        assert!(snap.attempts.iter().all(|a| !a.deciding && a.escalation.is_none()));
        assert!(snap.decisions.iter().any(|d| d.by == "orchestrator"), "the acceptance says who took it");
        let prompts = mock.prompts();
        let permission = prompts.iter().find(|p| p.contains("Decision needed: permission")).expect("woken for the permission");
        assert!(permission.contains("mock-install x.txt") && permission.contains(crate::orchestrator::ORCHESTRATOR_BOUNDS) && permission.contains("x.txt exists"));
        let candidate = prompts.iter().find(|p| p.contains("Decision needed: candidate")).expect("woken for the candidate");
        assert!(candidate.contains("Diff:") && candidate.contains("x.txt"));
        let events = service.handle("events.after", &json!({"after": 0, "limit": 10_000})).unwrap();
        let kinds: Vec<&str> = events.as_array().unwrap().iter().filter_map(|e| e["type"].as_str()).collect();
        assert_eq!(kinds.iter().filter(|k| **k == "orchestrator.decided").count(), 2);
    }

    #[test]
    fn what_the_orchestrator_escalates_reaches_the_delegate_with_its_reason() {
        // Each decision escalates with its own reason: answering the permission brings a candidate, which wakes the orchestrator again.
        let (_home, service, quest_id, _mock) = with_orchestrator(|prompt| {
            let reason = if prompt.contains("Decision needed: permission") { "it installs software" } else { "the criteria are ambiguous" };
            format!("```json\n{{\"decision\": \"escalate\", \"reason\": \"{reason}\"}}\n```")
        });
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "escalated", |s| s.attempts.first().is_some_and(|a| a.escalation.is_some()));
        let a = &snap.attempts[0];
        assert_eq!((a.state, a.deciding, a.escalation.as_deref()), (AttemptState::WaitingInput, false, Some("it installs software")));
        let call = |m: &str, p: Value| service.handle(m, &p).map_err(|e| e.message);
        let status = crate::mcp::run_tool("guild_status", &json!({}), &call, &|_| {}).unwrap();
        assert!(status.contains("NEEDS YOU: permission") && status.contains("escalated by the orchestrator: it installs software"), "{status}");
        // The delegate answers; the escalation is settled.
        let perm = a.pending_permission.clone().unwrap();
        service.handle("permission.respond", &json!({"attemptId": a.id, "requestId": perm.request_id, "optionId": "allow"})).unwrap();
        // Whatever the candidate's decision has reached by now, the permission's escalation went with the answer.
        let snap = wait_for(&service, "submitted", |s| s.attempts[0].state == AttemptState::Submitted);
        assert_ne!(snap.attempts[0].escalation.as_deref(), Some("it installs software"), "the permission's escalation outlived its answer");
        let snap = wait_for(&service, "the candidate escalated", |s| s.attempts[0].escalation.is_some() && !s.attempts[0].deciding);
        assert_eq!(snap.attempts[0].escalation.as_deref(), Some("the criteria are ambiguous"), "a candidate the orchestrator would not take waits for the delegate");
    }

    #[test]
    fn the_orchestrator_retries_a_failed_task_once_with_instructions_then_leaves_it() {
        let (_home, service, quest_id, mock) = with_orchestrator(|_| "```json\n{\"decision\": \"retry\", \"reason\": \"read the error first, then change only parser.gd\"}\n```".into());
        mock.set_behaviour(MockBehaviour::Fail("parse error in parser.gd".into()));
        let mut t = service.store.lock().unwrap().task("t1").unwrap().unwrap();
        t.max_attempts = 1;
        service.store.lock().unwrap().put_task(&t).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        // Fails, the orchestrator retries once with its instructions, it fails again: the delegate's.
        let snap = wait_for(&service, "failed twice", |s| s.attempts.len() == 2 && s.tasks[0].state == TaskState::Failed && s.attempts.iter().all(|a| !a.deciding));
        assert!(mock.prompts().iter().any(|p| p.contains("read the error first, then change only parser.gd") && !p.contains("## Decision needed")), "the retry carries the instructions");
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(service.snapshot().unwrap().attempts.len(), 2, "no third attempt: one retry by the orchestrator");
        let _ = snap;
    }

    #[test]
    fn a_task_not_yet_started_can_change_and_a_missing_strategy_is_the_default() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        // The CLI sends "strategy": null when no --strategy is given (F-HD-02).
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "First", "strategy": null, "selection": null})).unwrap();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t2", "title": "Second", "instructions": "old", "strategy": null})).unwrap();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t3", "title": "Third", "dependsOn": ["t2"]})).unwrap();
        // Edit instead of cancelling and recreating (F-HD-01).
        let t = service.handle("task.update", &json!({"taskId": "t2", "instructions": "new", "acceptance": ["it works"], "dependsOn": ["t1"]})).unwrap();
        assert_eq!((t["instructions"].as_str(), t["depends_on"].clone(), t["version"].as_u64()), (Some("new"), json!(["t1"]), Some(2)));
        assert_eq!(t["acceptance"][0]["text"], "it works");
        assert!(service.handle("task.update", &json!({"taskId": "t1", "dependsOn": ["t3"]})).is_err(), "a cycle t1 → t3 → t2 → t1");
        assert!(service.handle("task.update", &json!({"taskId": "t1", "dependsOn": ["nope"]})).is_err(), "a dependency that does not exist");
        // Once started it is the attempt's, not the plan's.
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        wait_for(&service, "t1 running", |s| s.attempts.iter().any(|a| a.task_id == "t1"));
        assert!(service.handle("task.update", &json!({"taskId": "t1", "title": "Late"})).is_err());
        let snap = service.snapshot().unwrap();
        let t2 = snap.tasks.iter().find(|t| t.id == "t2").unwrap();
        assert_eq!(t2.state, TaskState::Pending, "waits for its new dependency");
        assert!(snap.attempts.iter().all(|a| a.task_id == "t1"), "t2 does not start before t1");
    }

    #[test]
    fn a_design_kept_in_a_claude_doc_keeps_its_link_and_how_far_it_is_mirrored() {
        let (_home, service, _quest) = setup(MockBehaviour::Hang);
        assert!(service.handle("design.link", &json!({"projectId": "p", "url": "https://example.com/doc"})).is_err());
        assert!(service.handle("design.synced", &json!({"projectId": "p", "rev": 3})).is_err(), "nothing to be synced with");
        let url = "https://claude.ai/artifact/VzVmi9VGcK1FLEStcbm4e6";
        service.handle("design.link", &json!({"projectId": "p", "url": url, "by": "delegate"})).unwrap();
        service.handle("design.synced", &json!({"projectId": "p", "rev": 308})).unwrap();
        // The delegate mirrors an edit with the design as `get` gives it, or without its doc.
        let mut d = service.snapshot().unwrap().designs[0].clone();
        d.doc = None;
        d.concept.statement = "Words fight".into();
        service.handle("design.set", &json!({"projectId": "p", "design": d})).unwrap();
        let d = service.snapshot().unwrap().designs[0].clone();
        let doc = d.doc.unwrap();
        assert_eq!((doc.url.as_str(), doc.linked_by.as_str(), doc.synced_rev), (url, "delegate", Some(308)));
        assert_eq!(d.concept.statement, "Words fight");
        // Only what the guild settled since the sync makes the doc behind.
        let call = |m: &str, p: Value| service.handle(m, &p).map_err(|e| e.message);
        let status = || crate::mcp::run_tool("guild_status", &json!({}), &call, &|_| {}).unwrap();
        assert!(status().contains("the design doc https://claude.ai/artifact/VzVmi9VGcK1FLEStcbm4e6 is behind the guild: design v"), "the set above came after the sync");
        service.handle("design.synced", &json!({"projectId": "p", "rev": 309})).unwrap();
        assert!(!status().contains("is behind"), "{}", status());
        std::thread::sleep(Duration::from_millis(5));
        let q = service.handle("question.create", &json!({"projectId": "p", "question": "How long is a round?"})).unwrap();
        let text = status();
        assert!(text.contains(&format!("questions raised {}", q["id"].as_str().unwrap())) && !text.contains("design v"), "{text}");
        service.handle("design.link", &json!({"projectId": "p"})).unwrap();
        assert!(service.snapshot().unwrap().designs[0].doc.is_none(), "edited in the guild again");
    }

    #[test]
    fn the_visual_direction_keeps_images_palettes_and_notes_and_reaches_the_members() {
        let (home, service, _quest) = setup(MockBehaviour::Hang);
        let dir = home.0.join("drop");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("hero sheet.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([239, 125, 87, 255])).save(&png).unwrap();
        let gpl = dir.join("Sweetie.gpl");
        std::fs::write(&gpl, "GIMP Palette\nName: Sweetie\n 26 28 44 black\n177 62 83 red\n").unwrap();
        let img = service.handle("visual.add", &json!({"projectId": "p", "path": png.display().to_string(), "caption": "The hero"})).unwrap();
        assert_eq!((img["id"].as_str(), img["colors"].clone()), (Some("IMG-001"), json!(["#ef7d57"])));
        let copy = std::path::PathBuf::from(img["file"].as_str().unwrap());
        assert!(copy.starts_with(crate::paths::visual_dir("p")) && copy.exists(), "the guild keeps its own copy");
        let pal = service.handle("visual.add", &json!({"projectId": "p", "path": gpl.display().to_string()})).unwrap();
        assert_eq!((pal["name"].as_str(), pal["colors"].clone()), (Some("Sweetie"), json!(["#1a1c2c", "#b13e53"])));
        assert!(service.handle("visual.add", &json!({"projectId": "p", "path": dir.join("notes.doc").display().to_string()})).is_err());
        service.handle("visual.update", &json!({"projectId": "p", "id": "IMG-001", "caption": "The hero, idle"})).unwrap();
        service.handle("visual.notes", &json!({"projectId": "p", "notes": "One-pixel dark outline."})).unwrap();
        let d = service.store.lock().unwrap().design("p").unwrap().unwrap();
        assert_eq!((d.visual.images[0].caption.as_str(), d.visual.notes.as_str()), ("The hero, idle", "One-pixel dark outline."));
        // The members are told, in every task's design context.
        let context = crate::design::prompt_context(&d);
        assert!(context.contains("Palette Sweetie: #1a1c2c #b13e53") && context.contains("The hero, idle") && context.contains("One-pixel dark outline."), "{context}");
        // The repository copy carries the images and a page for them.
        let out = home.0.join("export");
        let written = crate::design::export(&out, &crate::design::DesignBundle { design: d.clone(), ..Default::default() }).unwrap();
        assert!(out.join("game-design/visual/visual.md").exists() && out.join("game-design/visual/img-001.png").exists(), "{written:?}");
        // Taking an image out deletes the guild's copy; a palette goes by its id.
        service.handle("visual.remove", &json!({"projectId": "p", "id": "IMG-001"})).unwrap();
        service.handle("visual.remove", &json!({"projectId": "p", "id": "PAL-001"})).unwrap();
        let d = service.store.lock().unwrap().design("p").unwrap().unwrap();
        assert!(d.visual.images.is_empty() && d.visual.palettes.is_empty() && !copy.exists());
        // The members may look at the guild's images without asking.
        assert_eq!(crate::policy::decide(&format!("cat {}", copy.display()), &[]), crate::policy::Verdict::Allow);
        assert_eq!(crate::policy::decide(&format!("rm {}", copy.display()), &[]), crate::policy::Verdict::Refuse, "never delete them");
    }

    #[test]
    fn a_retry_goes_on_from_what_a_failed_attempt_had_written() {
        let home = TempHome::new();
        let repo = home.repo();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), cancel_timeout: Duration::from_millis(500), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        let mock = Arc::new(MockHarness::new(MockBehaviour::WriteThenFail { file: "core.gd".into(), content: "# 900 lines of core\n".into(), reason: "timed out".into() }));
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"]})).unwrap();
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Q"})).unwrap()["id"].as_str().unwrap().to_string();
        // One attempt, so nothing retries before the test says so.
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Core", "maxAttempts": 1})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        // The first attempt stops with its work in the worktree: kept, not lost.
        let snap = wait_for(&service, "first stopped", |s| s.tasks[0].state == TaskState::Failed && s.attempts.iter().any(|a| a.partial.is_some()));
        let first = snap.attempts.iter().find(|a| a.partial.is_some()).unwrap().clone();
        assert_eq!(first.partial.as_ref().unwrap().files_changed, 1);
        // The retry starts from it and is told so.
        mock.set_behaviour(MockBehaviour::Succeed { file: "tests.gd".into(), content: "# tests\n".into() });
        service.handle("task.retry", &json!({"taskId": "t1"})).unwrap();
        let snap = wait_for(&service, "second submitted", |s| s.attempts.iter().any(|a| a.id != first.id && a.state == AttemptState::Submitted));
        let second = snap.attempts.iter().find(|a| a.id != first.id).unwrap();
        assert!(mock.prompts().iter().any(|p| p.contains("stopped before it finished")), "the retry knows where it starts");
        let diff = service.handle("artifact.read", &json!({"artifactId": second.candidate.as_ref().unwrap().diff_artifact_id.clone().unwrap()})).unwrap()["text"].as_str().unwrap().to_string();
        assert!(diff.contains("core.gd") && diff.contains("tests.gd"), "the candidate has the first attempt's work and its own: {diff}");
    }

    #[test]
    fn a_quest_takes_what_changed_in_main_and_a_conflict_leaves_it_as_it_was() {
        let (home, service, quest_id) = setup(MockBehaviour::Hang);
        let repo = home.repo_path();
        let commit = |file: &str, text: &str| {
            std::fs::write(repo.join(file), text).unwrap();
            workspaces::git(&repo, &["add", "-A"]).unwrap();
            workspaces::git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", file]).unwrap();
        };
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hangs"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let quest = service.store.lock().unwrap().quest(&quest_id).unwrap().unwrap();
        // The specification is corrected in main after the quest started (F-HD-06).
        commit("SPEC.md", "money circulates\n");
        let r = service.handle("quest.sync", &json!({"questId": quest_id})).unwrap();
        assert_eq!((r["changed"].as_bool(), r["files"].clone()), (Some(true), json!(["SPEC.md"])));
        assert!(workspaces::git(&repo, &["show", &format!("{}:SPEC.md", quest.integration_ref)]).unwrap().contains("money circulates"), "the quest's branch has it");
        assert_eq!(service.handle("quest.sync", &json!({"questId": quest_id})).unwrap()["changed"], false, "nothing new: nothing moves");
        assert_eq!(workspaces::current_branch(&repo).as_deref(), Some("main"), "the user's checkout never moved");
        // A conflict aborts, names its files and leaves the branch where it was.
        let head = workspaces::rev_parse(&repo, &quest.integration_ref).unwrap();
        workspaces::git(&repo, &["checkout", "-q", "-b", "side", &head]).unwrap();
        commit("SPEC.md", "money is created by the state\n");
        let side = workspaces::rev_parse(&repo, "HEAD").unwrap();
        workspaces::git(&repo, &["update-ref", &quest.integration_ref, &side]).unwrap();
        workspaces::git(&repo, &["checkout", "-q", "main"]).unwrap();
        commit("SPEC.md", "money circulates between sectors\n");
        let err = service.handle("quest.sync", &json!({"questId": quest_id})).unwrap_err();
        assert!(err.message.contains("conflicts in: SPEC.md"), "{}", err.message);
        assert_eq!(workspaces::rev_parse(&repo, &quest.integration_ref).unwrap(), side, "untouched");
    }

    #[test]
    fn a_task_has_its_own_time_limit_and_the_orchestrator_sees_it_coming() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        // The guild's default is 5 s here; this task may work 1 minute, the other only its default.
        service.handle("task.create", &json!({"questId": quest_id, "id": "long", "title": "Calibrate", "timeoutMinutes": 1})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        wait_for(&service, "running", |s| s.attempts.first().is_some_and(|a| a.state == AttemptState::Running));
        std::thread::sleep(Duration::from_secs(6));
        let a = service.snapshot().unwrap().attempts[0].clone();
        assert_eq!(a.state, AttemptState::Running, "past the guild's 5 s, within its own minute");
        assert_eq!(a.limit_seconds, 60);
        assert!(a.worked_seconds >= 5);
        // Near the limit, guild_status says so.
        let mut fake = service.snapshot().unwrap();
        fake.attempts[0].worked_seconds = 45;
        let line = crate::mcp::task_line(&fake, &fake.tasks[0]);
        assert!(line.contains("⚠ worked 0m of its 1m"), "{line}");
        fake.attempts[0].worked_seconds = 20;
        assert!(!crate::mcp::task_line(&fake, &fake.tasks[0]).contains("⚠"), "not yet");
        service.handle("attempt.cancel", &json!({"attemptId": a.id})).unwrap();
    }

    #[test]
    fn an_attempt_that_ends_while_asking_asks_for_nothing_any_more() {
        let (_home, service, quest_id) = setup(MockBehaviour::AskPermission { file: "x.txt".into(), content: "x".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Ask first"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "waiting", |s| s.attempts.first().is_some_and(|a| a.pending_permission.is_some()));
        service.handle("attempt.cancel", &json!({"attemptId": snap.attempts[0].id})).unwrap();
        let snap = wait_for(&service, "ended", |s| s.attempts[0].state.is_terminal());
        assert!(snap.attempts[0].pending_permission.is_none(), "a cancelled attempt kept its question");
        let call = |m: &str, p: Value| service.handle(m, &p).map_err(|e| e.message);
        assert!(!crate::mcp::run_tool("guild_status", &json!({}), &call, &|_| {}).unwrap().contains("NEEDS YOU: permission"));
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
    fn what_was_allowed_for_a_task_is_not_asked_again() {
        let (_home, service, quest_id) = setup(MockBehaviour::AskPermission { file: "x.txt".into(), content: "x".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Ask first", "maxAttempts": 3})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "waiting", |s| s.attempts.first().is_some_and(|a| a.state == AttemptState::WaitingInput));
        let a = &snap.attempts[0];
        // The user allows it always (FB-018): Kiro would forget it in the next session; the guild does not.
        service.handle("permission.respond", &json!({"attemptId": a.id, "requestId": "perm-1", "optionId": "always"})).unwrap();
        let snap = wait_for(&service, "submitted", |s| s.attempts[0].state == AttemptState::Submitted);
        let granted = &snap.tasks[0].granted;
        assert_eq!((granted.commands.clone(), granted.programs.clone()), (vec!["mock-install x.txt".to_string()], vec!["mock-install".to_string()]));
        // The retry asks the same: answered at once, by the policy.
        service.handle("task.retry", &json!({"taskId": "t1", "reason": "once more"})).unwrap();
        let first = a.id.clone();
        let snap = wait_for(&service, "second submitted", |s| s.attempts.iter().any(|x| x.id != first && x.state == AttemptState::Submitted));
        let second = snap.attempts.iter().find(|x| x.id != first).unwrap();
        let events = service.handle("events.attempt", &json!({"attemptId": second.id})).unwrap();
        let resolved: Vec<&Value> = events.as_array().unwrap().iter().filter(|e| e["type"] == "permission.resolved").collect();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0]["payload"]["by"], "policy");
        assert_eq!(resolved[0]["payload"]["optionId"], "allow", "allowed once each time: the harness keeps nothing");
    }

    #[test]
    fn a_question_resolved_by_mistake_opens_again() {
        let (_home, service, _quest) = setup(MockBehaviour::Hang);
        service.handle("question.create", &json!({"projectId": "p", "question": "How long is a round?"})).unwrap();
        service.handle("question.create", &json!({"projectId": "p", "question": "How many districts?"})).unwrap();
        // Q-002 resolved by mistake, then opened again (FB-034).
        service.handle("decision.create", &json!({"projectId": "p", "decision": "Six districts.", "questionIds": ["Q-002"]})).unwrap();
        let reopened = service.handle("question.reopen", &json!({"projectId": "p", "questionId": "Q-002", "why": "answered the wrong question"})).unwrap();
        assert_eq!((reopened["status"].as_str(), reopened["answer"].as_str(), reopened["decision_id"].clone()), (Some("open"), Some(""), Value::Null));
        let snap = service.snapshot().unwrap();
        let dec = snap.design_decisions.iter().find(|d| d.decision == "Six districts.").unwrap();
        assert!(dec.question_ids.is_empty() && dec.why.contains("no longer answers Q-002"), "{dec:?}");
        assert!(service.handle("question.reopen", &json!({"projectId": "p", "questionId": "Q-002"})).is_err(), "already open");
        assert_eq!(service.snapshot().unwrap().questions.iter().filter(|q| q.status == "open").count(), 2);
    }

    #[test]
    fn the_candidate_s_guild_toml_says_what_to_check_and_changing_it_needs_a_person() {
        let home = TempHome::new();
        let repo = home.repo();
        let commit = |msg: &str| { workspaces::git(&repo, &["add", "-A"]).unwrap(); workspaces::git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", msg]).unwrap(); };
        let toml = |id: &str, check: &str| format!("schema_version = 1\n[[verifiers]]\nid = \"{id}\"\nargv = [\"sh\", \"-c\", \"{check}\"]\nrequired = true\n");
        std::fs::create_dir_all(repo.join(".guild")).unwrap();
        std::fs::write(repo.join(".guild/guild.toml"), toml("old", "true")).unwrap();
        commit("guild");
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        let mock = Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "a.txt".into(), content: "a\n".into() }));
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"]})).unwrap();
        // The project's verifiers changed in main after it was added (FB-016 needed it added again).
        std::fs::write(repo.join(".guild/guild.toml"), toml("fresh", "test -f a.txt")).unwrap();
        commit("fresh verifier");
        let quest = service.handle("quest.create", &json!({"projectId": "p", "title": "Q"})).unwrap()["id"].as_str().unwrap().to_string();
        service.handle("task.create", &json!({"questId": quest, "id": "t1", "title": "A", "memberId": "aria"})).unwrap();
        service.handle("task.create", &json!({"questId": quest, "id": "t2", "title": "B", "memberId": "aria", "dependsOn": ["t1"]})).unwrap();
        service.handle("quest.start", &json!({"questId": quest})).unwrap();
        let snap = wait_for(&service, "t1 verified", |s| s.attempts.iter().any(|a| a.task_id == "t1" && !a.verifier_results.is_empty()));
        let a = snap.attempts.iter().find(|a| a.task_id == "t1").unwrap();
        assert_eq!(a.verifier_results.iter().map(|v| (v.id.as_str(), v.status.as_str())).collect::<Vec<_>>(), [("fresh", "passed")]);
        // A candidate that changes its own verifiers (FB-042: it removed a check with the file it ran).
        mock.set_behaviour(MockBehaviour::Succeed { file: ".guild/guild.toml".into(), content: toml("lenient", "true") });
        service.handle("task.accept", &json!({"taskId": "t1", "integrate": true})).unwrap();
        let snap = wait_for(&service, "t2 verified", |s| s.attempts.iter().any(|a| a.task_id == "t2" && a.verifier_results.len() == 2));
        let a = snap.attempts.iter().find(|a| a.task_id == "t2").unwrap();
        assert_eq!(a.verifier_results.iter().map(|v| (v.id.as_str(), v.status.as_str(), v.required)).collect::<Vec<_>>(), [("lenient", "passed", true), ("own-gates", "needs-review", true)]);
        assert!(!passes_gates(a), "no policy or judge takes it");
        assert!(a.escalation.as_deref().unwrap().contains("changes its own verifiers") && !a.deciding, "the orchestrator is not asked");
        assert_eq!(snap.tasks.iter().find(|t| t.id == "t2").unwrap().state, TaskState::Verifying);
    }

    #[test]
    fn a_decision_that_replaces_another_reaches_the_work_that_cites_it() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        let old = service.handle("decision.create", &json!({"projectId": "p", "decision": "Rent is paid first."})).unwrap()["id"].as_str().unwrap().to_string();
        service.handle("task.create", &json!({"questId": quest_id, "id": "review", "title": "Review", "instructions": format!("Check the economy against {old}.")})).unwrap();
        service.handle("task.create", &json!({"questId": quest_id, "id": "other", "title": "Other", "instructions": "Nothing about rent."})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        wait_for(&service, "review under way", |s| s.tasks.iter().any(|t| t.id == "review" && t.attempts_made > 0));
        // FB-039: the reviewer counted the old rule as broken.
        let new = service.handle("decision.create", &json!({"projectId": "p", "decision": "Food is paid first, then rent.", "supersedes": [old]})).unwrap();
        let snap = service.snapshot().unwrap();
        let review = snap.tasks.iter().find(|t| t.id == "review").unwrap();
        assert_eq!(review.notes, [format!("{} replaces {old}: Food is paid first, then rent.", new["id"].as_str().unwrap())]);
        assert!(snap.tasks.iter().find(|t| t.id == "other").unwrap().notes.is_empty(), "only what cites it");
        assert_eq!(snap.design_decisions.iter().find(|d| d.id == new["id"]).unwrap().supersedes, [old]);
    }

    #[test]
    fn waiting_without_a_quest_waits_on_every_quest_under_way() {
        let (_home, service, quest_id) = setup(MockBehaviour::Succeed { file: "a.txt".into(), content: "a\n".into() });
        let call = |m: &str, p: Value| service.handle(m, &p).map_err(|e| e.message);
        // FB-041: before, no questId failed only once the wait was over.
        assert!(crate::mcp::run_tool("guild_wait", &json!({"timeoutSeconds": 5}), &call, &|_| {}).unwrap_err().contains("no quest under way"));
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "A"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let text = crate::mcp::run_tool("guild_wait", &json!({"timeoutSeconds": 30}), &call, &std::thread::sleep).unwrap();
        assert!(text.starts_with("something needs you") && text.contains(&quest_id), "{text}");
    }

    #[test]
    fn cancelling_a_task_lets_its_waiting_candidate_s_member_go() {
        let (_home, service, quest_id) = setup(MockBehaviour::Succeed { file: "a.txt".into(), content: "a\n".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "A", "memberId": "aria"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        wait_for(&service, "candidate", |s| s.tasks.iter().any(|t| t.id == "t1" && t.state == TaskState::Verifying));
        // FB-035: the candidate stayed submitted and Aria busy, so the next task sat Ready.
        service.handle("task.cancel", &json!({"taskId": "t1"})).unwrap();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t2", "title": "B", "memberId": "aria"})).unwrap();
        let snap = wait_for(&service, "t2's candidate", |s| s.tasks.iter().any(|t| t.id == "t2" && t.state == TaskState::Verifying));
        let first = snap.attempts.iter().find(|a| a.task_id == "t1").unwrap();
        assert_eq!(first.state, AttemptState::Cancelled);
        assert_eq!(snap.tasks.iter().find(|t| t.id == "t1").unwrap().state, TaskState::Cancelled);
    }

    #[test]
    fn retrying_a_waiting_candidate_rejects_it_and_starts_again() {
        let (_home, service, quest_id) = setup(MockBehaviour::Succeed { file: "a.txt".into(), content: "a\n".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "A", "maxAttempts": 1})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "candidate", |s| s.tasks[0].state == TaskState::Verifying);
        let first = snap.attempts[0].id.clone();
        // FB-019: the candidate kept its member busy, so the task sat Ready.
        service.handle("task.retry", &json!({"taskId": "t1", "reason": "use b"})).unwrap();
        let snap = wait_for(&service, "second candidate", |s| s.attempts.len() == 2 && s.tasks[0].state == TaskState::Verifying);
        let old = snap.attempts.iter().find(|a| a.id == first).unwrap();
        assert_eq!((old.state, old.failure.as_deref()), (AttemptState::Failed, Some("use b")));
        assert_eq!(snap.tasks[0].max_attempts, 2);
    }

    #[test]
    fn the_user_can_accept_a_verified_attempt_the_orchestrator_failed() {
        let (_home, service, quest_id) = setup(MockBehaviour::Succeed { file: "a.txt".into(), content: "a\n".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "A", "maxAttempts": 1})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "candidate", |s| s.tasks[0].state == TaskState::Verifying);
        let good = snap.attempts[0].id.clone();
        service.handle("task.reject", &json!({"taskId": "t1", "attemptId": good, "reason": "the orchestrator: no evidence"})).unwrap();
        wait_for(&service, "failed", |s| s.tasks[0].state == TaskState::Failed);
        // Without --override a failed attempt stays failed (FB-017).
        assert!(service.handle("task.accept", &json!({"taskId": "t1", "attemptId": good})).is_err());
        service.handle("task.accept", &json!({"taskId": "t1", "attemptId": good, "override": true, "integrate": true})).unwrap();
        let snap = wait_for(&service, "integrated", |s| s.tasks[0].state == TaskState::Completed);
        assert_eq!(snap.attempts[0].state, AttemptState::Passed);
        assert_eq!(snap.tasks[0].winner_attempt_id.as_deref(), Some(good.as_str()));
    }

    #[test]
    fn a_note_on_a_running_task_reaches_the_member_before_it_finishes() {
        let (_home, service, quest_id) = setup(MockBehaviour::AskPermission { file: "x.txt".into(), content: "x".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Ask first"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "waiting", |s| s.attempts.first().is_some_and(|a| a.state == AttemptState::WaitingInput));
        // FB-026: three new points while it works.
        service.handle("task.note", &json!({"taskId": "t1", "text": "also cover the empty case"})).unwrap();
        service.handle("permission.respond", &json!({"attemptId": snap.attempts[0].id, "requestId": "perm-1", "optionId": "allow"})).unwrap();
        let snap = wait_for(&service, "submitted", |s| s.attempts[0].state == AttemptState::Submitted);
        assert_eq!(snap.attempts[0].turns, 2, "one more turn in the same session for the note");
        assert_eq!(snap.tasks[0].notes, ["also cover the empty case"]);
        assert!(service.handle("task.note", &json!({"taskId": "t1", "text": " "})).is_err());
    }

    #[test]
    fn re_adding_the_project_keeps_a_member_the_user_turned_off() {
        let home = TempHome::new();
        let repo = home.repo();
        std::fs::create_dir_all(repo.join(".guild")).unwrap();
        std::fs::write(repo.join(".guild/guild.toml"), "schema_version = 1\n[[members]]\nid = \"oro\"\nroles = [\"orchestrator\"]\nharness = \"mock\"\n").unwrap();
        let service = Service::new(Store::open_in_memory().unwrap(), Settings::default());
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.update", &json!({"memberId": "oro", "enabled": false, "by": "delegate"})).unwrap();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        let m = service.snapshot().unwrap().members.into_iter().find(|m| m.id == "oro").unwrap();
        assert!(!m.enabled, "FB-024");
        assert_eq!(m.switched.map(|s| (s.enabled, s.by)), Some((false, "delegate".to_string())));
    }

    #[test]
    fn waiting_for_a_permission_does_not_count_against_the_time_limit() {
        let home = TempHome::new();
        let repo = home.repo();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(2), cancel_timeout: Duration::from_millis(500), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        service.register(Arc::new(MockHarness::new(MockBehaviour::AskPermission { file: "x.txt".into(), content: "x".into() })));
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"]})).unwrap();
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Q"})).unwrap()["id"].as_str().unwrap().to_string();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Ask first"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "waiting", |s| s.attempts.first().map(|a| a.state == AttemptState::WaitingInput).unwrap_or(false));
        // Nobody answers for longer than the attempt may work: it keeps waiting.
        std::thread::sleep(Duration::from_secs(3));
        let a = service.snapshot().unwrap().attempts[0].clone();
        assert_eq!(a.state, AttemptState::WaitingInput, "timed out while waiting for someone");
        // The orchestrator sees for how long, to answer the oldest first.
        let call = |m: &str, p: Value| service.handle(m, &p).map_err(|e| e.message);
        let status = crate::mcp::run_tool("guild_status", &json!({}), &call, &|_| {}).unwrap();
        assert!(status.contains("NEEDS YOU: permission") && status.contains("(waiting 0m0"), "{status}");
        let perm = snap.attempts[0].pending_permission.clone().unwrap();
        // Its own scratch folder, for what it would have left in /tmp; gone when the attempt ends.
        let scratch = crate::paths::scratch_dir(&a.id);
        assert!(scratch.is_dir());
        service.handle("permission.respond", &json!({"attemptId": a.id, "requestId": perm.request_id, "optionId": "allow"})).unwrap();
        wait_for(&service, "submitted", |s| s.attempts[0].state == AttemptState::Submitted);
        service.handle("task.accept", &json!({"taskId": "t1"})).unwrap();
        wait_for(&service, "passed", |s| s.attempts[0].state.is_terminal());
        assert!(!scratch.exists(), "the scratch folder outlived its attempt");
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
    fn a_rejected_candidate_is_retried_from_where_it_was_with_the_reason() {
        let home = TempHome::new();
        let repo = home.repo();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        let mock = Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "a.txt".into(), content: "first\n".into() }));
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"]})).unwrap();
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Q"})).unwrap()["id"].as_str().unwrap().to_string();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Two files", "maxAttempts": 3})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "first candidate", |s| s.tasks[0].state == TaskState::Verifying);
        let first = snap.attempts[0].candidate.clone().unwrap();
        mock.set_behaviour(MockBehaviour::Succeed { file: "b.txt".into(), content: "fix\n".into() });
        service.handle("task.reject", &json!({"taskId": "t1", "reason": "also add b.txt, keep a.txt"})).unwrap();
        let snap = wait_for(&service, "second candidate", |s| s.attempts.len() == 2 && s.attempts.iter().any(|a| a.state == AttemptState::Submitted));
        let prompt = mock.prompts().last().cloned().unwrap();
        assert!(prompt.contains("also add b.txt, keep a.txt") && prompt.contains("already holds the last candidate"), "{prompt}");
        let second = snap.attempts.iter().find(|a| a.state == AttemptState::Submitted).unwrap().candidate.clone().unwrap();
        // It went on from the first candidate, and is still measured against the quest's base.
        assert_eq!(second.base_sha, first.base_sha);
        assert_eq!(second.files_changed, 2, "a.txt from the first candidate plus b.txt");
    }

    #[test]
    fn a_retry_carries_its_candidate_onto_work_integrated_meanwhile() {
        let home = TempHome::new();
        let repo = home.repo();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        let mock = Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "logic.txt".into(), content: "v1\n".into() }));
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "orin", "harness": "mock", "roles": ["implementer"]})).unwrap();
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Q"})).unwrap()["id"].as_str().unwrap().to_string();
        service.handle("task.create", &json!({"questId": quest_id, "id": "logic", "title": "Logic", "memberId": "orin", "maxAttempts": 3})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        wait_for(&service, "logic's first candidate", |s| s.tasks.iter().any(|t| t.id == "logic" && t.state == TaskState::Verifying));
        // The UI is done and integrated while logic waits for review.
        mock.set_behaviour(MockBehaviour::Succeed { file: "ui.txt".into(), content: "ui\n".into() });
        // Orin's candidate waiting for review keeps Orin busy: Lira does the UI.
        service.handle("member.add", &json!({"id": "lira", "harness": "mock", "roles": ["implementer"]})).unwrap();
        service.handle("task.create", &json!({"questId": quest_id, "id": "ui", "title": "UI", "memberId": "lira"})).unwrap();
        wait_for(&service, "ui's candidate", |s| s.tasks.iter().any(|t| t.id == "ui" && t.state == TaskState::Verifying));
        service.handle("task.accept", &json!({"taskId": "ui", "integrate": true})).unwrap();
        wait_for(&service, "ui integrated", |s| s.tasks.iter().any(|t| t.id == "ui" && t.state == TaskState::Completed));
        mock.set_behaviour(MockBehaviour::Succeed { file: "logic.txt".into(), content: "v2\n".into() });
        service.handle("task.reject", &json!({"taskId": "logic", "reason": "write v2"})).unwrap();
        let snap = wait_for(&service, "logic's retry", |s| s.attempts.iter().filter(|a| a.task_id == "logic").count() == 2 && s.tasks.iter().any(|t| t.id == "logic" && t.state == TaskState::Verifying));
        let retry = snap.attempts.iter().filter(|a| a.task_id == "logic").max_by(|a, b| a.created_at.cmp(&b.created_at)).unwrap();
        let diff = service.handle("artifact.read", &json!({"artifactId": retry.candidate.as_ref().unwrap().diff_artifact_id.clone().unwrap()})).unwrap()["text"].as_str().unwrap().to_string();
        assert!(diff.contains("logic.txt") && !diff.contains("ui.txt"), "the retry's diff is its own change only, nothing of the UI reverted:\n{diff}");
        assert!(mock.prompts().last().unwrap().contains("already holds the last candidate"));
    }

    #[test]
    fn claude_orchestrates_through_mcp_from_quest_to_integration() {
        let (_home, service, _) = setup(MockBehaviour::Succeed { file: "poison.txt".into(), content: "tick\n".into() });
        let call = |m: &str, p: Value| service.handle(m, &p).map_err(|e| e.message);
        let tool = |name: &str, args: Value| {
            let r = crate::mcp::handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}), &call, &std::thread::sleep).unwrap();
            (r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string(), r["result"]["isError"] == true)
        };
        let (text, failed) = tool("guild_start_quest", json!({"title": "Poison", "tasks": [{"id": "t1", "title": "Poison tick", "instructions": "Write poison.txt", "acceptance": ["poison.txt exists"]}]}));
        assert!(!failed, "{text}");
        let quest_id = service.snapshot().unwrap().quests.iter().find(|q| q.title == "Poison").unwrap().id.clone();
        let (text, _) = tool("guild_wait", json!({"questId": quest_id, "timeoutSeconds": 30}));
        assert!(text.starts_with("something needs you") && text.contains("candidate ready"), "{text}");
        let task_id = format!("{}-t1", &quest_id[quest_id.len() - 8..]);
        let (diff, failed) = tool("guild_diff", json!({"taskId": task_id}));
        assert!(!failed && diff.contains("poison.txt"), "{diff}");
        let (text, failed) = tool("guild_decide", json!({"taskId": task_id, "decision": "accept"}));
        assert!(!failed, "{text}");
        let snap = wait_for(&service, "integrated", |s| s.tasks.iter().any(|t| t.id == task_id && t.state == TaskState::Completed));
        assert_eq!(snap.quests.iter().find(|q| q.id == quest_id).unwrap().state, QuestState::Completed);
        let (text, _) = tool("guild_status", json!({"questId": quest_id}));
        assert!(text.contains("1/1 done") && text.contains("guild/quest/"), "{text}");
    }

    #[test]
    fn the_orchestrator_answers_and_its_plan_starts_a_quest() {
        let home = TempHome::new();
        let repo = home.repo();
        let mut service = Service::new(Store::open_in_memory().unwrap(), Settings { attempt_timeout: Duration::from_secs(5), ..Settings::default() });
        Arc::get_mut(&mut service).unwrap().quiet = true;
        let mock = Arc::new(MockHarness::new(MockBehaviour::Hang));
        mock.set_orchestrator_reply("Two steps.\n```json\n{\"title\": \"Poison\", \"tasks\": [{\"id\": \"t1\", \"title\": \"Status\", \"role\": \"implementer\"}, {\"id\": \"t2\", \"title\": \"Test\", \"role\": \"nobody-has-this\", \"dependsOn\": [\"t1\"]}]}\n```\n");
        service.register(mock);
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("member.add", &json!({"id": "kael", "harness": "mock", "roles": ["planner"]})).unwrap();
        service.handle("member.add", &json!({"id": "aria", "harness": "mock", "roles": ["implementer"]})).unwrap();
        assert_eq!(service.snapshot().unwrap().orchestrator.as_deref(), Some("kael"), "the planner orchestrates");
        service.handle("orchestrator.say", &json!({"text": "Add poison"})).unwrap();
        assert!(service.handle("orchestrator.say", &json!({"text": "again"})).is_err(), "one message at a time");
        let snap = wait_for(&service, "the reply", |s| s.conversation.len() == 2);
        assert_eq!(snap.conversation[0].author, "user");
        assert_eq!(snap.conversation[1].text, "Two steps.");
        let quest_id = snap.conversation[1].quest_id.clone().expect("the plan became a quest");
        let quest = snap.quests.iter().find(|q| q.id == quest_id).unwrap();
        assert_eq!(quest.state, QuestState::Running);
        let tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == quest_id).collect();
        assert_eq!(tasks.len(), 2);
        let test = tasks.iter().find(|t| t.title == "Test").unwrap();
        assert_eq!(test.role, "implementer", "a role nobody holds falls back to one somebody does");
        assert_eq!(test.depends_on.len(), 1);
        assert!(!snap.orchestrator_busy);
    }

    #[test]
    fn closing_the_guild_pauses_cancels_and_hides_it_until_reopened() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        service.handle("member.add", &json!({"id": "bo", "harness": "mock", "roles": ["reviewer"]})).unwrap();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hangs"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        wait_for(&service, "running", |s| s.attempts.first().map(|a| a.state == AttemptState::Running).unwrap_or(false));
        // A member retired earlier stays retired across a close and a reopen.
        service.handle("member.add", &json!({"id": "old", "harness": "mock", "roles": ["reviewer"]})).unwrap();
        service.handle("member.update", &json!({"memberId": "old", "enabled": false})).unwrap();
        service.handle("guild.close", &json!({})).unwrap();
        let snap = wait_for(&service, "closed", |s| s.attempts[0].state == AttemptState::Cancelled);
        assert_eq!(snap.quests[0].state, QuestState::Paused);
        assert!(snap.members.iter().all(|m| !m.enabled), "nobody left in the hall");
        assert!(snap.orchestrator.is_none());
        service.handle("guild.reopen", &json!({})).unwrap();
        let snap = service.snapshot().unwrap();
        let enabled: Vec<&str> = snap.members.iter().filter(|m| m.enabled).map(|m| m.id.as_str()).collect();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.contains(&"aria") && enabled.contains(&"bo") && !enabled.contains(&"old"));
        assert_eq!(snap.quests[0].state, QuestState::Paused, "quests resume from the board");
    }

    #[test]
    fn removing_a_busy_member_retires_it_and_a_free_one_goes() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        service.handle("member.add", &json!({"id": "bo", "harness": "mock", "roles": ["reviewer"]})).unwrap();
        service.handle("member.remove", &json!({"memberId": "bo"})).unwrap();
        assert!(service.snapshot().unwrap().members.iter().all(|m| m.id != "bo"), "a free member is deleted");
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hangs"})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        wait_for(&service, "running", |s| s.attempts.first().map(|a| a.state == AttemptState::Running).unwrap_or(false));
        let r = service.handle("member.remove", &json!({"memberId": "aria"})).unwrap();
        assert_eq!(r["retired"], "aria");
        let snap = wait_for(&service, "cancelled", |s| s.attempts[0].state == AttemptState::Cancelled);
        assert!(!snap.members.iter().find(|m| m.id == "aria").unwrap().enabled, "kept for its attempt, but out of the dungeon");
        service.handle("member.remove", &json!({"memberId": "aria"})).unwrap();
        assert!(service.snapshot().unwrap().members.is_empty());
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
    fn founding_a_guild_gives_members_a_design_the_prompt_carries() {
        let home = TempHome::new();
        let repo = home.repo();
        let service = Service::new(Store::open_in_memory().unwrap(), Settings::default());
        service.register(Arc::new(MockHarness::new(MockBehaviour::Hang)));
        service.probe_harnesses();
        let r = service.handle("guild.found", &json!({"repoPath": repo.display().to_string(), "name": "Verbrawl", "template": "videogame", "members": [
            {"name": "Sellers", "role": "systems-designer", "harness": "mock", "model": "mock-fast"},
            {"name": "Orin", "role": "gameplay-engineer", "harness": "mock"},
        ]})).unwrap();
        assert_eq!(r["members"].as_array().unwrap().len(), 2);
        assert_eq!(r["design"]["loops"].as_array().unwrap().len(), 2);
        let snap = service.snapshot().unwrap();
        assert_eq!(snap.designs.len(), 1);
        assert!(snap.roles.iter().any(|r| r.id == "exploit-hunter"));
        assert_eq!(snap.members.iter().find(|m| m.id == "sellers").unwrap().roles, vec!["systems-designer".to_string()]);
        // Edit the design: the version moves and the prompt carries it.
        let mut d = snap.designs[0].clone();
        d.experience.avoid.push("grinding".into());
        d.loops.push(crate::design::GameLoop { id: String::new(), name: "Economy".into(), steps: vec!["earn".into(), "spend".into()], ..Default::default() });
        let saved = service.handle("design.set", &json!({"projectId": d.project_id, "design": d})).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(saved["loops"][2]["id"], "loop-3");
        let design: crate::design::GameDesign = serde_json::from_value(saved).unwrap();
        let task = Task { id: "t".into(), quest_id: "q".into(), title: "Tune words".into(), instructions: "…".into(), acceptance: vec![], depends_on: vec![], role: "systems-designer".into(), member_id: None, model_id: None, priority: 0, state: TaskState::Ready, block_reason: None, attempts_made: 0, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, timeout_minutes: None, winner_attempt_id: None, notes: vec![], granted: Default::default(), version: 1, created_at: now(), updated_at: now() };
        let ws = Workspace { id: "w".into(), attempt_id: "a".into(), project_id: design.project_id.clone(), path: "/tmp/w".into(), branch: "guild/x".into(), base_sha: "0".into(), state: WorkspaceState::Ready, created_at: now(), updated_at: now() };
        let prompt = compose_prompt(&config::default_role_prompt("systems-designer"), &task, &ws, Some(&design), &[], Carried::Nothing);
        assert!(prompt.contains("Avoid: a dominant strategy; always choosing the longest word; obvious decisions; grinding"));
        assert!(prompt.contains("Loop \"Economy\": earn → spend ↺"));
        assert!(prompt.contains("part, a loop or the whole"));
        assert!(prompt.contains("System Combat [prototyping]"));
        // Export writes TOML into the repo, and a fresh project add reads it back.
        let exported = service.handle("design.export", &json!({"projectId": design.project_id})).unwrap();
        assert!(exported["files"].as_array().unwrap().len() >= 9);
        assert!(!repo.join("game-design").exists(), "the user's checkout is untouched");
        let listed = workspaces::git(&repo, &["ls-tree", "-r", "--name-only", &crate::design::design_ref(&design.project_id)]).unwrap();
        assert!(listed.contains("game-design/loops/loop-3.md"), "{listed}");
        assert!(listed.contains("game-design/overview.md") && listed.contains("game-design/design.json"), "{listed}");
        // Exporting again without changes keeps the commit.
        let again = service.handle("design.export", &json!({"projectId": design.project_id})).unwrap();
        assert_eq!(again["sha"], exported["sha"]);
        let other = Service::new(Store::open_in_memory().unwrap(), Settings::default());
        let added = other.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": design.project_id})).unwrap();
        assert_eq!(added["designImported"], true);
        assert_eq!(other.snapshot().unwrap().designs[0].loops.len(), 3);
    }

    #[test]
    fn three_candidates_race_and_the_best_passing_one_is_integrated_alone() {
        let home = TempHome::new();
        let repo = home.repo();
        let service = Service::new(Store::open_in_memory().unwrap(), Settings { concurrency: 3, ..Settings::default() });
        let mock = Arc::new(MockHarness::new(MockBehaviour::Succeed { file: "hello.txt".into(), content: "hello\n".into() }));
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        for id in ["aria", "bram", "cora"] {
            service.handle("member.add", &json!({"id": id, "harness": "mock", "roles": ["implementer"], "model": "mock-fast"})).unwrap();
        }
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Race"})).unwrap()["id"].as_str().unwrap().to_string();
        let t = service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hello", "candidates": 3, "autoIntegrate": true})).unwrap();
        assert_eq!(t["candidates"], 3);
        assert_eq!(t["max_attempts"], 3);
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "three runners", |s| s.attempts.len() == 3);
        let members: std::collections::HashSet<String> = snap.attempts.iter().map(|a| a.member_id.clone()).collect();
        assert_eq!(members.len(), 3, "one runner per member");
        let snap = wait_for(&service, "completed", |s| s.quests[0].state == QuestState::Completed);
        let task = &snap.tasks[0];
        assert_eq!(task.state, TaskState::Completed);
        let winner = task.winner_attempt_id.clone().expect("a winner");
        assert_eq!(snap.attempts.iter().filter(|a| a.state == AttemptState::Passed).count(), 1);
        let losers: Vec<(AttemptState, Option<String>)> = snap.attempts.iter().filter(|a| a.id != winner).map(|a| (a.state, a.failure.clone())).collect();
        assert!(losers.iter().all(|(state, failure)| *state == AttemptState::Failed && failure.as_deref().is_some_and(|f| f.starts_with("not selected"))), "every loser rejected as not selected: {losers:?}");
        assert!(snap.attempts.iter().all(|a| a.candidate.is_some()), "every runner left a candidate");
        let kinds: Vec<String> = service.store.lock().unwrap().events_after(0, 2000).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds.iter().filter(|k| *k == "merge.completed").count(), 1, "one merge");
        assert_eq!(kinds.iter().filter(|k| *k == "candidate.rejected").count(), 2);
        assert!(kinds.contains(&"race.finished".to_string()));
        assert!(workspaces::git(&repo, &["ls-tree", "--name-only", &snap.quests[0].integration_ref]).unwrap().contains("hello.txt"));
    }

    #[test]
    fn concurrency_changes_while_running_and_survives_a_restart() {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let service = Service::new(Store::open(&db).unwrap(), Settings::default());
        assert_eq!(service.snapshot().unwrap().concurrency, 2, "the settings' until changed");
        assert!(service.handle("guild.concurrency", &json!({"concurrency": 0})).is_err());
        assert!(service.handle("guild.concurrency", &json!({"concurrency": 17})).is_err());
        service.handle("guild.concurrency", &json!({"concurrency": 5})).unwrap();
        assert_eq!(service.snapshot().unwrap().concurrency, 5);
        drop(service);
        let again = Service::new(Store::open(&db).unwrap(), Settings { concurrency: 2, ..Settings::default() });
        assert_eq!(again.snapshot().unwrap().concurrency, 5, "kept in the store");
    }

    #[test]
    fn a_failed_unit_of_work_leaves_neither_state_nor_event() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Atomic"})).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        service.subscribers.lock().unwrap().push(tx);
        let store = service.store.lock().unwrap();
        let before = store.events_after(0, 10_000).unwrap().len();
        let failed: Res<()> = service.atomic(&store, || {
            service.move_task_locked(&store, "t1", TaskState::Pending, None)?;
            service.emit_locked(&store, Event::new("test.boom", "test"));
            Err(ApiError::internal("boom"))
        });
        assert!(failed.is_err());
        assert!(!store.in_transaction());
        assert_eq!(store.task("t1").unwrap().unwrap().state, TaskState::Draft, "state rolled back");
        assert_eq!(store.events_after(0, 10_000).unwrap().len(), before, "events rolled back");
        assert!(rx.try_recv().is_err(), "subscribers heard nothing");
        // The same change, committed: state and event together, then the subscriber hears it.
        service.atomic(&store, || { service.move_task_locked(&store, "t1", TaskState::Pending, None)?; Ok(()) }).unwrap();
        assert_eq!(store.task("t1").unwrap().unwrap().state, TaskState::Pending);
        let events = store.events_after(0, 10_000).unwrap();
        assert_eq!(events.len(), before + 1);
        assert_eq!(events.last().unwrap().kind, "task.pending");
        assert_eq!(rx.try_recv().unwrap().kind, "task.pending");
        // Nested units: an inner failure inside an outer success rolls back only the inner part.
        let nested: Res<()> = service.atomic(&store, || {
            service.move_task_locked(&store, "t1", TaskState::Ready, None)?;
            let inner: Res<()> = service.atomic(&store, || { service.move_task_locked(&store, "t1", TaskState::Running, None)?; Err(ApiError::internal("inner")) });
            assert!(inner.is_err());
            Ok(())
        });
        nested.unwrap();
        assert_eq!(store.task("t1").unwrap().unwrap().state, TaskState::Ready);
        assert!(!store.in_transaction());
    }

    /// A guild with a project, three mock members and one started quest whose
    /// task waits for attempts: no adapter is registered, so nothing runs
    /// until a second service over the same database takes over.
    fn seed(home: &TempHome, db: &Path, candidates: u32, auto_integrate: bool) -> (Arc<Service>, String) {
        let repo = home.repo();
        let service = Service::new(Store::open(db).unwrap(), Settings { concurrency: 3, ..Settings::default() });
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        for id in ["aria", "bram", "cora"] {
            service.handle("member.add", &json!({"id": id, "harness": "mock", "roles": ["implementer"], "model": "mock-fast"})).unwrap();
        }
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Survive"})).unwrap()["id"].as_str().unwrap().to_string();
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Keep going", "candidates": candidates, "autoIntegrate": auto_integrate})).unwrap();
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        service.tick();
        assert_eq!(service.snapshot().unwrap().tasks[0].state, TaskState::Ready, "nothing dispatched without an adapter");
        (service, quest_id)
    }

    /// An attempt as the service would have left it when it died in `state`.
    fn seed_attempt(service: &Arc<Service>, member: &str, state: AttemptState, with_session: bool, change: Option<(&str, &str)>, generation: u32) -> Attempt {
        let store = service.store.lock().unwrap();
        let task = store.task("t1").unwrap().unwrap();
        let quest = store.quest(&task.quest_id).unwrap().unwrap();
        let project = store.project(&quest.project_id).unwrap().unwrap();
        let id = new_id();
        let base = workspaces::ensure_integration_branch(&project, &quest).unwrap();
        let ws = workspaces::provision(&project, &quest.id, &task.id, &id, &base).unwrap();
        store.put_workspace(&ws).unwrap();
        let candidate = change.map(|(file, content)| {
            std::fs::write(Path::new(&ws.path).join(file), content).unwrap();
            workspaces::commit_candidate(&ws, "seeded candidate").unwrap().unwrap()
        });
        let session_id = with_session.then(|| {
            let sid = new_id();
            store.put_session(&Session { id: sid.clone(), attempt_id: id.clone(), harness: HarnessId::named("mock"), native_id: Some(format!("mock-{sid}")), runtime_version: "mock".into(), state: "running".into(), created_at: now(), updated_at: now() }).unwrap();
            sid
        });
        let verified = candidate.is_some() && matches!(state, AttemptState::Submitted | AttemptState::Passed);
        let attempt = Attempt {
            id: id.clone(), task_id: task.id.clone(), task_version: task.version, quest_id: quest.id.clone(), project_id: project.id.clone(), member_id: member.into(),
            harness: HarnessId::named("mock"), model_requested: Some("mock-fast".into()), model_effective: Some("mock-fast".into()), kind: AttemptKind::Work, state, workspace_id: Some(ws.id.clone()),
            session_id, generation, candidate, partial: None, summary: String::new(), action: None, pending_permission: None, failure: None,
            verifier_results: if verified { vec![VerifierResult { id: "v".into(), required: true, status: "passed".into(), exit_code: Some(0), duration_ms: 1, log_artifact_id: None, tail: String::new(), evidence: vec![] }] } else { vec![] },
            turns: 1, resumed: 0, deciding: false, escalation: None, worked_seconds: 0, limit_seconds: 0, started_at: Some(now()), finished_at: None, created_at: now(), updated_at: now(),
        };
        store.put_attempt(&attempt).unwrap();
        let mut t = task;
        t.state = TaskState::Running;
        t.attempts_made += 1;
        store.put_task(&t).unwrap();
        attempt
    }

    fn takeover(db: &Path, behaviour: MockBehaviour) -> (Arc<Service>, Arc<MockHarness>) {
        let service = Service::new(Store::open(db).unwrap(), Settings { concurrency: 3, ..Settings::default() });
        let mock = Arc::new(MockHarness::new(behaviour));
        service.register(mock.clone());
        service.probe_harnesses();
        service.reconcile();
        (service, mock)
    }

    #[test]
    fn dying_while_provisioning_loses_the_attempt_and_retries_the_task() {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let (first, _q) = seed(&home, &db, 1, false);
        let a = seed_attempt(&first, "aria", AttemptState::Provisioning, false, None, 1);
        drop(first);
        let (second, _) = takeover(&db, MockBehaviour::Succeed { file: "done.txt".into(), content: "ok\n".into() });
        let snap = wait_for(&second, "retried and submitted", |s| s.attempts.len() == 2 && s.attempts.iter().any(|x| x.state == AttemptState::Submitted));
        let lost = snap.attempts.iter().find(|x| x.id == a.id).unwrap();
        assert_eq!(lost.state, AttemptState::Lost);
        assert_eq!(snap.tasks[0].state, TaskState::Verifying);
        assert_eq!(snap.tasks[0].attempts_made, 2);
    }

    #[test]
    fn dying_while_a_permission_waits_resumes_the_session() {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let (first, _q) = seed(&home, &db, 1, false);
        let a = seed_attempt(&first, "aria", AttemptState::WaitingInput, true, None, 1);
        drop(first);
        let (second, mock) = takeover(&db, MockBehaviour::Succeed { file: "done.txt".into(), content: "ok\n".into() });
        let snap = wait_for(&second, "resumed", |s| s.attempts[0].state == AttemptState::Submitted);
        assert_eq!(snap.attempts.len(), 1, "the same attempt went on");
        assert_eq!(snap.attempts[0].id, a.id);
        assert_eq!(snap.attempts[0].resumed, 1);
        assert_eq!(snap.attempts[0].generation, 2);
        assert_eq!(mock.resumed().len(), 1);
        assert!(snap.attempts[0].pending_permission.is_none());
    }

    #[test]
    fn dying_while_verifying_keeps_the_candidate_for_the_user() {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let (first, _q) = seed(&home, &db, 1, false);
        seed_attempt(&first, "aria", AttemptState::Verifying, true, Some(("done.txt", "ok\n")), 1);
        drop(first);
        let (second, mock) = takeover(&db, MockBehaviour::Fail("must not run".into()));
        let snap = second.snapshot().unwrap();
        assert_eq!(snap.attempts[0].state, AttemptState::Submitted);
        assert_eq!(snap.tasks[0].state, TaskState::Verifying, "a submitted candidate puts its task up for a decision");
        assert!(mock.resumed().is_empty(), "nothing was resumed");
        second.handle("task.accept", &json!({"taskId": "t1", "integrate": true})).unwrap();
        assert_eq!(second.snapshot().unwrap().tasks[0].state, TaskState::Completed);
    }

    #[test]
    fn dying_while_integrating_finishes_the_integration_once() {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let (first, _q) = seed(&home, &db, 1, false);
        let a = seed_attempt(&first, "aria", AttemptState::Passed, false, Some(("done.txt", "ok\n")), 1);
        {
            let store = first.store.lock().unwrap();
            let mut t = store.task("t1").unwrap().unwrap();
            t.state = TaskState::Integrating;
            t.winner_attempt_id = Some(a.id.clone());
            store.put_task(&t).unwrap();
        }
        let repo = home.repo_path();
        drop(first);
        let (second, _) = takeover(&db, MockBehaviour::Hang);
        let snap = second.snapshot().unwrap();
        assert_eq!(snap.tasks[0].state, TaskState::Completed);
        let tree = workspaces::git(&repo, &["ls-tree", "--name-only", &snap.quests[0].integration_ref]).unwrap();
        assert!(tree.contains("done.txt"));
        let sha = workspaces::rev_parse(&repo, &snap.quests[0].integration_ref).unwrap();
        // A third life finds nothing left to do and moves nothing.
        let (third, _) = takeover(&db, MockBehaviour::Hang);
        assert_eq!(third.snapshot().unwrap().tasks[0].state, TaskState::Completed);
        assert_eq!(workspaces::rev_parse(&repo, &snap.quests[0].integration_ref).unwrap(), sha);
        let merges = second.store.lock().unwrap().events_after(0, 10_000).unwrap().into_iter().filter(|e| e.kind == "merge.completed").count();
        assert_eq!(merges, 1);
    }

    fn race_after_restart(submitted: usize) -> Snapshot {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let (first, _q) = seed(&home, &db, 3, true);
        let members = ["aria", "bram", "cora"];
        for (i, m) in members.iter().enumerate() {
            if i < submitted {
                seed_attempt(&first, m, AttemptState::Submitted, false, Some(("done.txt", &format!("{m}\n"))), 1);
            } else {
                // Running, but its session and worktree changes are gone with the crash.
                seed_attempt(&first, m, AttemptState::Running, false, None, 1);
            }
        }
        drop(first);
        let (second, _) = takeover(&db, MockBehaviour::Hang);
        wait_for(&second, "race settled", |s| s.tasks[0].state.is_terminal())
    }

    #[test]
    fn a_race_losing_runners_in_a_crash_settles_with_what_survived() {
        let snap = race_after_restart(2);
        assert_eq!(snap.tasks[0].state, TaskState::Completed);
        assert_eq!(snap.attempts.iter().filter(|a| a.state == AttemptState::Passed).count(), 1);
        assert_eq!(snap.attempts.iter().filter(|a| a.state == AttemptState::Lost).count(), 1);
        let snap = race_after_restart(1);
        assert_eq!(snap.tasks[0].state, TaskState::Completed);
        assert_eq!(snap.attempts.iter().filter(|a| a.state == AttemptState::Lost).count(), 2);
        let snap = race_after_restart(0);
        assert_eq!(snap.tasks[0].state, TaskState::Failed);
        assert!(snap.attempts.iter().all(|a| a.state == AttemptState::Lost));
    }

    #[test]
    fn a_stale_result_after_a_resume_is_ignored() {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let (first, _q) = seed(&home, &db, 1, false);
        let a = seed_attempt(&first, "aria", AttemptState::Running, true, None, 2);
        first.finish_attempt_of(&a.id, 1, TurnOutcome::Completed { stop_reason: "late".into() }).unwrap();
        let snap = first.snapshot().unwrap();
        assert_eq!(snap.attempts[0].state, AttemptState::Running, "generation 1 may not finish generation 2");
        let kinds: Vec<String> = first.store.lock().unwrap().events_after(0, 1000).unwrap().into_iter().map(|e| e.kind).collect();
        assert!(kinds.contains(&"attempt.stale_result".to_string()));
        first.finish_attempt_of(&a.id, 2, TurnOutcome::Completed { stop_reason: "now".into() }).unwrap();
        assert_eq!(first.snapshot().unwrap().attempts[0].state, AttemptState::Submitted);
    }

    #[test]
    fn restarting_several_times_resumes_each_time() {
        let home = TempHome::new();
        let db = home.0.join("guild.db");
        let (first, _q) = seed(&home, &db, 1, false);
        let a = seed_attempt(&first, "aria", AttemptState::Running, true, None, 1);
        drop(first);
        let (second, _) = takeover(&db, MockBehaviour::Hang);
        wait_for(&second, "resumed once", |s| s.attempts[0].resumed == 1 && s.attempts[0].state == AttemptState::Running);
        // The second life ends as a real one would: sessions closed, the attempt left running.
        second.shutdown();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(second.snapshot().unwrap().attempts[0].state, AttemptState::Running);
        let (third, _) = takeover(&db, MockBehaviour::Succeed { file: "done.txt".into(), content: "ok\n".into() });
        let snap = wait_for(&third, "resumed twice and done", |s| s.attempts[0].state == AttemptState::Submitted);
        assert_eq!(snap.attempts[0].id, a.id);
        assert_eq!(snap.attempts[0].resumed, 2);
        assert_eq!(snap.attempts[0].generation, 3);
    }

    fn race_setup(behaviour: MockBehaviour) -> (TempHome, Arc<Service>, Arc<MockHarness>, String) {
        let home = TempHome::new();
        let repo = home.repo();
        let service = Service::new(Store::open_in_memory().unwrap(), Settings { concurrency: 4, ..Settings::default() });
        let mock = Arc::new(MockHarness::new(behaviour));
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        for id in ["aria", "bram", "cora"] {
            service.handle("member.add", &json!({"id": id, "harness": "mock", "roles": ["implementer"], "model": "mock-fast"})).unwrap();
        }
        service.handle("member.add", &json!({"id": "themis", "harness": "mock", "roles": ["judge"], "model": "mock-fast"})).unwrap();
        let quest_id = service.handle("quest.create", &json!({"projectId": "p", "title": "Judged"})).unwrap()["id"].as_str().unwrap().to_string();
        (home, service, mock, quest_id)
    }

    #[test]
    fn a_judge_picks_the_winner_of_a_race_and_its_decision_is_kept() {
        let (_home, service, mock, quest_id) = race_setup(MockBehaviour::Succeed { file: "hello.txt".into(), content: "hello\n".into() });
        let t = service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hello", "strategy": "race", "candidates": 3, "selection": "judge", "autoIntegrate": true})).unwrap();
        assert_eq!(t["strategy"]["type"], "candidate_race");
        assert_eq!(t["strategy"]["selection"], "judge");
        // The judge prefers B whatever B is; the labels hide the authors.
        mock.set_judge_reply("Looking at the three.\n```json\n{\"scores\": {\"A\": {\"correctness\": 4}, \"B\": {\"correctness\": 5}, \"C\": {\"correctness\": 3}}, \"findings\": {\"B\": [\"cleanest\"]}, \"tradeoffs\": \"none\", \"winner\": \"B\", \"confidence\": 0.8, \"reason\": \"B is the cleanest\"}\n```\n");
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "completed", |s| s.quests[0].state == QuestState::Completed);
        let judge = snap.attempts.iter().find(|a| a.kind == AttemptKind::Judge).expect("a judge attempt");
        assert_eq!(judge.member_id, "themis");
        assert_eq!(judge.state, AttemptState::Passed, "judge failure: {:?} · decisions {:?}", judge.failure, snap.decisions.iter().map(|d| (d.by.clone(), d.reason.clone())).collect::<Vec<_>>());
        assert_eq!(snap.decisions.len(), 1);
        let d = &snap.decisions[0];
        assert_eq!(d.by, "judge");
        assert_eq!(d.candidates.len(), 3);
        assert_eq!(d.confidence, Some(0.8));
        let winner = d.candidates.iter().find(|c| c.label == "B").unwrap();
        assert_eq!(d.winner_attempt_id.as_deref(), Some(winner.attempt_id.as_str()));
        assert_eq!(winner.score, Some(5.0));
        assert_eq!(snap.tasks[0].winner_attempt_id, d.winner_attempt_id);
        assert_eq!(snap.attempts.iter().filter(|a| a.kind == AttemptKind::Work && a.state == AttemptState::Passed).count(), 1);
        let kinds: Vec<String> = service.store.lock().unwrap().events_after(0, 5000).unwrap().into_iter().map(|e| e.kind).collect();
        for k in ["judge.started", "judge.finished", "race.decided", "merge.completed"] { assert!(kinds.contains(&k.to_string()), "missing {k}"); }
        // The dataset: one row per finished attempt, with the decision on it.
        let rows: Vec<AttemptRecord> = serde_json::from_value(service.handle("records.list", &json!({"projectId": "p"})).unwrap()).unwrap();
        assert_eq!(rows.len(), 4, "three workers and a judge");
        let winners: Vec<&AttemptRecord> = rows.iter().filter(|r| r.winner == Some(true)).collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(winners[0].judge_score, Some(5.0));
        assert_eq!(winners[0].decided_by.as_deref(), Some("judge"));
        assert_eq!(winners[0].strategy, StrategyKind::CandidateRace);
        assert!(rows.iter().filter(|r| r.kind == AttemptKind::Work).all(|r| r.winner.is_some() && r.files_changed == Some(1)));
        assert!(rows.iter().find(|r| r.kind == AttemptKind::Judge).unwrap().winner.is_none());
        // The judge saw labels, not names.
        let judge_events = service.store.lock().unwrap().events_of_attempt(&judge.id, 5000).unwrap();
        let prompt_chars = judge_events.iter().find(|e| e.kind == "turn.started").map(|e| e.payload["promptChars"].as_u64().unwrap()).unwrap();
        assert!(prompt_chars > 200);
    }

    #[test]
    fn an_unusable_judge_falls_back_to_the_policy_and_says_so() {
        let (_home, service, mock, quest_id) = race_setup(MockBehaviour::Succeed { file: "hello.txt".into(), content: "hello\n".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hello", "strategy": "race", "candidates": 2, "selection": "judge", "autoIntegrate": true})).unwrap();
        mock.set_judge_reply("I cannot decide.");
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "completed", |s| s.quests[0].state == QuestState::Completed);
        let d = &snap.decisions[0];
        assert_eq!(d.by, "policy");
        assert!(d.reason.contains("judge unusable"), "{}", d.reason);
        assert!(snap.attempts.iter().any(|a| a.kind == AttemptKind::Judge && a.state == AttemptState::Failed));
    }

    #[test]
    fn a_reviewed_task_is_one_attempt_a_judge_approves() {
        let (_home, service, mock, quest_id) = race_setup(MockBehaviour::Succeed { file: "hello.txt".into(), content: "hello\n".into() });
        let t = service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hello", "strategy": "reviewed", "autoIntegrate": true})).unwrap();
        assert_eq!(t["candidates"], 1);
        mock.set_judge_reply("```json\n{\"scores\": {\"A\": {\"correctness\": 5}}, \"winner\": \"A\", \"confidence\": 0.9, \"reason\": \"fine\"}\n```");
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "completed", |s| s.quests[0].state == QuestState::Completed);
        assert_eq!(snap.attempts.iter().filter(|a| a.kind == AttemptKind::Work).count(), 1);
        assert_eq!(snap.attempts.iter().filter(|a| a.kind == AttemptKind::Judge).count(), 1);
        assert_eq!(snap.decisions[0].by, "judge");
    }

    #[test]
    fn the_user_choosing_against_the_judge_is_an_override_on_record() {
        let (_home, service, mock, quest_id) = race_setup(MockBehaviour::Succeed { file: "hello.txt".into(), content: "hello\n".into() });
        service.handle("task.create", &json!({"questId": quest_id, "id": "t1", "title": "Hello", "strategy": "race", "candidates": 2, "selection": "user"})).unwrap();
        mock.set_judge_reply("unused");
        service.handle("quest.start", &json!({"questId": quest_id})).unwrap();
        let snap = wait_for(&service, "both submitted", |s| s.tasks[0].state == TaskState::Verifying && s.attempts.iter().filter(|a| a.state == AttemptState::Submitted).count() == 2);
        assert!(snap.decisions.is_empty(), "the user decides");
        let chosen = snap.attempts.iter().find(|a| a.member_id == "bram").unwrap().id.clone();
        service.handle("task.accept", &json!({"taskId": "t1", "attemptId": chosen, "integrate": true})).unwrap();
        let snap = wait_for(&service, "completed", |s| s.quests[0].state == QuestState::Completed);
        assert_eq!(snap.decisions.len(), 1);
        assert_eq!(snap.decisions[0].by, "user");
        assert!(!snap.decisions[0].human_override, "no earlier decision to override");
        assert_eq!(snap.tasks[0].winner_attempt_id.as_deref(), Some(chosen.as_str()));
    }

    #[test]
    fn judge_output_is_found_in_fenced_blocks_or_bare_objects() {
        assert_eq!(parse_judge_output("```json\n{\"winner\": \"A\"}\n```").unwrap()["winner"], "A");
        assert_eq!(parse_judge_output("thinking… ```json\n{\"x\": 1}\n``` then ```\n{\"winner\": \"C\", \"confidence\": 0.5}\n``` done").unwrap()["winner"], "C");
        assert_eq!(parse_judge_output("Verdict: {\"winner\": \"B\"} ok").unwrap()["winner"], "B");
        assert!(parse_judge_output("no decision").is_err());
        assert!(parse_judge_output("```json\n{\"scores\": {}}\n```").is_err());
    }

    const PYTHON: &str = if cfg!(windows) { "python" } else { "python3" };

    /// A repository with the word game and its guild config, as a project.
    fn wordfight_repo(home: &TempHome) -> PathBuf {
        let repo = home.repo_path();
        std::fs::create_dir_all(repo.join(".guild")).unwrap();
        std::fs::write(repo.join("wordfight.py"), include_str!("../../fixtures/wordfight/wordfight.py")).unwrap();
        std::fs::write(repo.join("balance.json"), include_str!("../../fixtures/wordfight/balance.json")).unwrap();
        std::fs::write(repo.join(".guild/guild.toml"), include_str!("../../fixtures/wordfight/.guild/guild.toml").replace("python3", PYTHON)).unwrap();
        workspaces::git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        workspaces::git(&repo, &["add", "-A"]).unwrap();
        workspaces::git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "wordfight"]).unwrap();
        repo
    }

    #[test]
    fn an_experiment_builds_variants_playtests_them_and_the_analyst_answers_the_hypothesis() {
        let home = TempHome::new();
        let repo = wordfight_repo(&home);
        let service = Service::new(Store::open_in_memory().unwrap(), Settings { concurrency: 3, ..Settings::default() });
        let mock = Arc::new(MockHarness::new(MockBehaviour::WriteParams { file: "balance.json".into() }));
        service.register(mock.clone());
        service.probe_harnesses();
        let added = service.handle("project.add", &json!({"repoPath": repo.display().to_string()})).unwrap();
        assert_eq!(added["project"]["id"], "wordfight");
        assert!(added["project"]["playtest"]["argv"].is_array(), "the runner comes from guild.toml");
        for id in ["orin", "lira"] { service.handle("member.add", &json!({"id": id, "harness": "mock", "roles": ["gameplay-engineer"], "model": "mock-fast"})).unwrap(); }
        service.handle("member.add", &json!({"id": "sage", "harness": "mock", "roles": ["design-analyst"], "model": "mock-fast"})).unwrap();
        let h = service.handle("hypothesis.create", &json!({"projectId": "wordfight", "problem": "Players always choose the longest word", "level": "loop",
            "hypothesis": "A stronger semantic multiplier makes short thematic words competitive", "expectedEffect": "more distinct words chosen", "proposedChange": "semantic_multiplier 1.2 -> 1.6",
            "measurements": ["choice.selected.length mean", "choices.top_share", "runs.completion_rate"]})).unwrap();
        let hid = h["id"].as_str().unwrap().to_string();
        assert_eq!(h["status"], "proposed");
        let e = service.handle("experiment.create", &json!({"projectId": "wordfight", "hypothesisId": hid, "title": "Semantic bonus", "variants": [
            {"id": "control", "name": "control", "params": {"length_multiplier": 1.35, "semantic_multiplier": 1.2}},
            {"id": "a", "name": "semantic 1.6", "params": {"length_multiplier": 1.35, "semantic_multiplier": 1.6}},
            {"id": "b", "name": "flat length", "params": {"length_multiplier": 1.0, "semantic_multiplier": 1.6}},
        ], "plan": {"profiles": ["greedy", "explorer"], "runs_per_profile": 2}})).unwrap();
        let eid = e["id"].as_str().unwrap().to_string();
        mock.set_analysis_reply("Reading the metrics.\n```json\n{\"verdict\": \"supports\", \"evidence\": [\"explorer completes more runs in b\", \"greedy's top_share stays 1.0: the greedy profile is a dominant strategy by construction\"], \"unexpected\": [\"control and a differ little for greedy\"], \"recommended_variant\": \"b\", \"recommended_next\": \"try semantic 1.8 with length 1.0\", \"confidence\": 0.6}\n```");
        service.handle("experiment.start", &json!({"experimentId": eid})).unwrap();
        let snap = service.snapshot().unwrap();
        assert_eq!(snap.experiments[0].status, crate::experiment::ExperimentStatus::Building);
        assert_eq!(snap.hypotheses[0].status, HypothesisStatus::Experimenting);
        assert_eq!(snap.tasks.len(), 4, "three variants and the analysis");
        // Builds: each variant's worktree gets its parameters, chosen by the policy, not integrated.
        let snap = wait_for(&service, "playtesting", |s| s.experiments[0].status != crate::experiment::ExperimentStatus::Building);
        assert_eq!(snap.experiments[0].status, crate::experiment::ExperimentStatus::Playtesting, "{:?}", snap.experiments[0].failure);
        for v in &snap.experiments[0].variants {
            let a = snap.attempts.iter().find(|a| Some(&a.id) == v.attempt_id.as_ref()).expect("a build per variant");
            let ws = snap.workspaces.iter().find(|w| Some(&w.id) == a.workspace_id.as_ref()).unwrap();
            let balance: Value = serde_json::from_str(&std::fs::read_to_string(Path::new(&ws.path).join("balance.json")).unwrap()).unwrap();
            assert_eq!(balance["semantic_multiplier"], v.params["semantic_multiplier"], "variant {}", v.id);
        }
        assert!(snap.tasks.iter().filter(|t| t.title.starts_with("Variant")).all(|t| t.state == TaskState::Accepted));
        // Playtests: 3 variants × 2 profiles × 2 seeds, each with telemetry and metrics.
        let snap = wait_for(&service, "analysed", |s| matches!(s.experiments[0].status, crate::experiment::ExperimentStatus::Decided | crate::experiment::ExperimentStatus::Failed));
        let e = &snap.experiments[0];
        assert_eq!(e.status, crate::experiment::ExperimentStatus::Decided, "{:?}", e.failure);
        assert_eq!(snap.playtests.len(), 12);
        assert!(snap.playtests.iter().all(|p| p.status == "finished" && p.event_count > 0 && p.events_artifact_id.is_some()), "{:?}", snap.playtests.iter().map(|p| p.failure.clone()).collect::<Vec<_>>());
        let greedy_control = snap.playtests.iter().find(|p| p.variant_id.as_deref() == Some("control") && p.profile == "greedy").unwrap();
        assert_eq!(greedy_control.metrics["choices"]["top_share"].as_f64().map(|x| x > 0.0), Some(true));
        // The analyst's verdict reached the experiment and the hypothesis; the analysis task is done.
        let r = e.result.as_ref().unwrap();
        assert_eq!(r.verdict, "supports");
        assert_eq!(r.recommended_variant.as_deref(), Some("b"));
        assert_eq!(r.metrics["b"]["profiles"]["explorer"]["playtests"], 2);
        assert_eq!(snap.hypotheses[0].status, HypothesisStatus::Supported);
        let analysis = snap.attempts.iter().find(|a| a.kind == AttemptKind::Analysis).unwrap();
        assert_eq!(analysis.member_id, "sage");
        assert_eq!(analysis.state, AttemptState::Passed);
        assert_eq!(snap.tasks.iter().find(|t| t.title == "Analysis").unwrap().state, TaskState::Completed);
        // The human accepts the recommended variant: its build is integrated, the others are not.
        let decided = service.handle("experiment.accept", &json!({"experimentId": eid, "note": "b reads best"})).unwrap();
        assert_eq!(decided["experiment"]["status"], "accepted");
        assert_eq!(decided["integrated"]["integrated"], true);
        assert_eq!(decided["integrated"]["decision"], "DEC-001", "{decided}");
        let decisions = service.snapshot().unwrap().design_decisions;
        assert_eq!(decisions[0].experiment_ids, vec![eid.clone()]);
        assert_eq!(decisions[0].playtest_ids.len(), 12);
        assert_eq!(decisions[0].rejected_alternatives.len(), 2);
        let snap = service.snapshot().unwrap();
        let b_task = snap.experiments[0].variants.iter().find(|v| v.id == "b").unwrap().task_id.clone().unwrap();
        assert_eq!(snap.tasks.iter().find(|t| t.id == b_task).unwrap().state, TaskState::Completed);
        assert_eq!(snap.tasks.iter().filter(|t| t.title.starts_with("Variant") && t.state == TaskState::Accepted).count(), 2);
        let merged: Value = serde_json::from_str(&workspaces::git(&repo, &["show", &format!("{}:balance.json", snap.quests[0].integration_ref)]).unwrap()).unwrap();
        assert_eq!(merged["length_multiplier"], 1.0);
        let kinds: Vec<String> = service.store.lock().unwrap().events_after(0, 10_000).unwrap().into_iter().map(|e| e.kind).collect();
        for k in ["hypothesis.created", "experiment.created", "experiment.started", "experiment.built", "playtest.started", "playtest.finished", "experiment.playtested", "analysis.started", "experiment.analysed", "experiment.decided", "merge.completed"] {
            assert!(kinds.contains(&k.to_string()), "missing {k}");
        }
        // Iterating makes a new draft from the same variants.
        let next = service.handle("experiment.iterate", &json!({"experimentId": eid, "note": "push semantic further"})).unwrap();
        let next_id = next["integrated"]["next"].as_str().unwrap().to_string();
        let snap = service.snapshot().unwrap();
        let draft = snap.experiments.iter().find(|e| e.id == next_id).unwrap();
        assert_eq!(draft.status, crate::experiment::ExperimentStatus::Draft);
        assert_eq!(draft.variants.len(), 3);
        assert!(draft.variants.iter().all(|v| v.task_id.is_none()));
    }

    #[test]
    fn a_question_is_resolved_by_a_decision_the_design_points_at_and_both_survive_a_restart() {
        let home = TempHome::new();
        let repo = home.repo();
        let db = home.0.join("guild.db");
        let service = Service::new(Store::open(&db).unwrap(), Settings::default());
        service.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        service.handle("design.template", &json!({"projectId": "p", "template": "videogame"})).unwrap();
        // A system is edited on its own: maturity and the precise current design.
        let d = service.handle("design.system.set", &json!({"projectId": "p", "system": {"id": "economy", "name": "Letter economy", "maturity": "hypothesis", "intent": "Every rare letter is a decision.", "unknown": ["pool size"], "current": "Pool: 6; refills 1 per turn."}})).unwrap();
        assert_eq!(d["version"], 2);
        assert_eq!(d["systems"][2]["maturity"], "hypothesis");
        // A question on that system, then a decision that answers it.
        let q = service.handle("question.create", &json!({"projectId": "p", "question": "Should the pool refill from rewards?", "context": "Players run dry by round 8.", "systemId": "economy"})).unwrap();
        assert_eq!(q["id"], "Q-001");
        assert_eq!(q["status"], "open");
        let q2 = service.handle("question.create", &json!({"projectId": "p", "question": "How many districts?", "systemId": "progression"})).unwrap();
        assert_eq!(q2["id"], "Q-002");
        let snap = service.snapshot().unwrap();
        assert_eq!(snap.questions.len(), 2);
        assert!(snap.designs[0].systems.iter().find(|s| s.id == "economy").unwrap().questions.contains(&"Q-001".to_string()));
        let r = service.handle("question.resolve", &json!({"projectId": "p", "questionId": "Q-001", "decision": "Rewards refill the pool by 2.", "why": "Runs died of starvation, not of bad words.", "evidence": ["6 human playtests"], "rejectedAlternatives": ["a bigger pool"], "answer": "Yes: +2 per reward."})).unwrap();
        assert_eq!(r["decision"]["id"], "DEC-001");
        assert_eq!(r["question"]["status"], "resolved");
        assert_eq!(r["question"]["decision_id"], "DEC-001");
        assert_eq!(r["question"]["answer"], "Yes: +2 per reward.");
        assert!(service.handle("question.resolve", &json!({"projectId": "p", "questionId": "Q-001", "decision": "again"})).is_err(), "a resolved question is not resolved twice");
        let snap = service.snapshot().unwrap();
        let economy = snap.designs[0].systems.iter().find(|s| s.id == "economy").unwrap();
        assert_eq!(economy.decisions, vec!["DEC-001".to_string()]);
        assert_eq!(snap.design_decisions[0].question_ids, vec!["Q-001".to_string()]);
        assert_eq!(snap.design_decisions[0].design_version, 2);
        // Export: Markdown pages for the question and the decision, and design.json with both.
        service.handle("design.export", &json!({"projectId": "p"})).unwrap();
        let listed = workspaces::git(&repo, &["ls-tree", "-r", "--name-only", &crate::design::design_ref("p")]).unwrap();
        for f in ["game-design/overview.md", "game-design/questions/q-001.md", "game-design/questions/q-002.md", "game-design/decisions/dec-001.md", "game-design/systems/economy.md", "game-design/design.json"] {
            assert!(listed.contains(f), "{f} missing in {listed}");
        }
        assert!(!repo.join("game-design").exists());
        let overview = workspaces::git(&repo, &["show", &format!("{}:game-design/overview.md", crate::design::design_ref("p"))]).unwrap();
        assert!(overview.contains("## Unresolved questions (1)") && overview.contains("## Resolved questions (1)"), "{overview}");
        // A restart over the same database keeps everything; a fresh guild re-adding the project reads the branch.
        drop(service);
        let again = Service::new(Store::open(&db).unwrap(), Settings::default());
        let snap = again.snapshot().unwrap();
        assert_eq!(snap.questions.len(), 2);
        assert_eq!(snap.design_decisions.len(), 1);
        assert_eq!(snap.designs[0].version, 2);
        let fresh = Service::new(Store::open_in_memory().unwrap(), Settings::default());
        let added = fresh.handle("project.add", &json!({"repoPath": repo.display().to_string(), "id": "p"})).unwrap();
        assert_eq!(added["designImported"], true);
        let snap = fresh.snapshot().unwrap();
        assert_eq!(snap.questions.iter().filter(|q| q.status == "resolved").count(), 1);
        assert_eq!(snap.design_decisions[0].id, "DEC-001");
        assert_eq!(snap.designs[0].systems.len(), 3);
        let kinds: Vec<String> = again.store.lock().unwrap().events_after(0, 10_000).unwrap().into_iter().map(|e| e.kind).collect();
        for k in ["question.created", "question.resolved", "decision.created", "design.updated", "design.exported"] { assert!(kinds.contains(&k.to_string()), "missing {k}"); }
    }

    #[test]
    fn an_ai_player_plays_the_build_and_the_exploit_hunters_finding_replays() {
        let home = TempHome::new();
        let repo = wordfight_repo(&home);
        // A setup that leaves a mark: every checkout made to play or replay must run it.
        let marks = home.repo_path().with_file_name("setup-marks.txt");
        // Forward slashes, quoted for sh: a Windows path's backslashes are
        // escapes in a TOML string (and in sh).
        let marks_sh = marks.display().to_string().trim_start_matches(r"\\?\").replace('\\', "/");
        let toml = std::fs::read_to_string(repo.join(".guild/guild.toml")).unwrap().replace("name = \"Wordfight\"", &format!("name = \"Wordfight\"\nsetup = [\"sh\", \"-c\", \"pwd >> '{marks_sh}'\"]"));
        std::fs::write(repo.join(".guild/guild.toml"), toml).unwrap();
        workspaces::git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "setup"]).unwrap();
        let service = Service::new(Store::open_in_memory().unwrap(), Settings::default());
        let mock = Arc::new(MockHarness::new(MockBehaviour::Reply("ok".into())));
        // The hunter takes the longest word and, on its second move, reports that it always wins.
        mock.set_player(|prompt| {
            if prompt.contains("The game is over") { return "Long words won every time; nothing else was worth it.".into(); }
            let moves: Vec<(String, usize)> = prompt.lines().filter_map(|l| l.strip_prefix("- ")).filter_map(|l| {
                let (id, label) = l.split_once(": ")?;
                Some((id.to_string(), label.split_whitespace().next()?.len()))
            }).collect();
            let best = moves.iter().max_by_key(|m| m.1).map(|m| m.0.clone()).unwrap_or_default();
            let finding = if prompt.contains("## Your move 2") { r#", "finding": {"title": "The longest word always wins", "impact": "word choice stops mattering", "severity": "high", "steps": "pick the longest word every round"}"# } else { "" };
            format!("```json\n{{\"action\": \"{best}\", \"why\": \"longest hits hardest\"{finding}}}\n```")
        });
        service.register(mock.clone());
        service.probe_harnesses();
        service.handle("project.add", &json!({"repoPath": repo.display().to_string()})).unwrap();
        service.handle("design.template", &json!({"projectId": "wordfight", "template": "videogame"})).unwrap();
        service.handle("member.add", &json!({"id": "vex", "harness": "mock", "roles": ["exploit-hunter"], "model": "mock-fast"})).unwrap();
        assert!(service.handle("ai.play", &json!({"projectId": "wordfight", "memberId": "nobody"})).is_err());
        let started = service.handle("ai.play", &json!({"projectId": "wordfight", "memberId": "vex", "persona": "exploit_hunter", "seed": 5})).unwrap();
        let pid = started["id"].as_str().unwrap().to_string();
        assert_eq!((started["kind"].as_str(), started["profile"].as_str(), started["status"].as_str()), (Some("ai_player"), Some("ai:exploit_hunter"), Some("running")));
        let play_dir = started["play_dir"].as_str().unwrap().to_string();
        let snap = wait_for(&service, "the AI playtest ends", |s| s.playtests.iter().any(|p| p.id == pid && p.status != "running"));
        let pt = snap.playtests.iter().find(|p| p.id == pid).unwrap();
        assert_eq!(pt.status, "finished", "{:?}", pt.failure);
        assert!(pt.event_count > 10 && pt.metrics["ai"]["turns"].as_u64().unwrap() >= 2 && pt.metrics["ai"]["fallbacks"] == 0, "{}", pt.metrics);
        assert!(pt.observations.contains("Long words won"), "its reflection is kept");
        assert!(!Path::new(&play_dir).exists(), "the checkout is removed");
        // The finding carries the replay: build, seed and the moves up to it.
        let f = snap.findings.iter().find(|f| f.id == "F-001").expect("the finding is filed");
        assert_eq!((f.title.as_str(), f.severity.as_str(), f.status.as_str(), f.seed, f.actions.len()), ("The longest word always wins", "high", "unverified", Some(5), 2));
        assert_eq!((f.round, f.member_id.as_deref(), f.playtest_id.as_deref()), (Some(2), Some("vex"), Some(pid.as_str())));
        // Replayed on its build without the agent, it holds.
        let v = service.handle("finding.verify", &json!({"projectId": "wordfight", "findingId": "F-001"})).unwrap();
        assert_eq!(v["status"], "reproduced", "{}", v["verification"]);
        // On a build whose balance changed, the same moves no longer make the same game.
        std::fs::write(repo.join("balance.json"), "{\"length_multiplier\": 1.0, \"semantic_multiplier\": 2.0}\n").unwrap();
        workspaces::git(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "flatter length"]).unwrap();
        let v = service.handle("finding.verify", &json!({"projectId": "wordfight", "findingId": "F-001", "ref": "main"})).unwrap();
        assert_eq!(v["status"], "fixed", "{}", v["verification"]);
        // It becomes a design question that points back to it.
        let p = service.handle("finding.promote", &json!({"projectId": "wordfight", "findingId": "F-001"})).unwrap();
        assert_eq!((p["question"]["id"].as_str(), p["question"]["raised_by"].as_str(), p["question"]["source"].as_str()), (Some("Q-001"), Some("exploit-hunter"), Some("F-001")));
        assert!(p["question"]["context"].as_str().unwrap().contains("seed 5"));
        assert!(service.handle("finding.set", &json!({"projectId": "wordfight", "findingId": "F-001", "status": "maybe"})).is_err());
        assert_eq!(service.handle("finding.set", &json!({"projectId": "wordfight", "findingId": "F-001", "status": "wont_fix"})).unwrap()["status"], "wont_fix");
        let prepared = std::fs::read_to_string(&marks).unwrap();
        assert_eq!(prepared.lines().count(), 3, "the play checkout and both replays ran the setup: {prepared}");
    }

    #[test]
    fn playing_the_build_records_telemetry_and_observations_that_become_questions() {
        let home = TempHome::new();
        let repo = wordfight_repo(&home);
        let service = Service::new(Store::open_in_memory().unwrap(), Settings::default());
        service.handle("project.add", &json!({"repoPath": repo.display().to_string()})).unwrap();
        service.handle("design.template", &json!({"projectId": "wordfight", "template": "videogame"})).unwrap();
        // A human session on the current build: a checkout apart from the user's, a telemetry file, the command.
        let started = service.handle("play.start", &json!({"projectId": "wordfight", "player": "nacho", "seed": 4})).unwrap();
        let cwd = started["cwd"].as_str().unwrap().to_string();
        assert!(cwd.contains("worktrees") && Path::new(&cwd).join("wordfight.py").exists());
        assert_eq!(started["argv"], json!([PYTHON, "wordfight.py", "profile=human", "seed=4"]));
        let telemetry = started["env"]["GUILD_TELEMETRY"].as_str().unwrap().to_string();
        let pid = started["playtest"]["id"].as_str().unwrap().to_string();
        assert_eq!(started["playtest"]["status"], "running");
        assert_eq!(started["playtest"]["design_version"], 1);
        // The human plays: always the first word, until the run ends.
        let argv: Vec<String> = serde_json::from_value(started["argv"].clone()).unwrap();
        let mut child = std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(&cwd).env("GUILD_TELEMETRY", &telemetry)
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
        { use std::io::Write; child.stdin.as_mut().unwrap().write_all("1\n".repeat(12).as_bytes()).unwrap(); }
        assert!(child.wait().unwrap().success());
        // Mid-session note: tied to the build, design version, run and round.
        let o = service.handle("observation.create", &json!({"projectId": "wordfight", "kind": "confusing", "text": "I don't understand why this word did so much damage"})).unwrap();
        assert_eq!(o["id"], "OBS-001");
        assert_eq!(o["playtest_id"], pid.as_str());
        assert_eq!(o["run_id"], "human-4");
        assert!(o["round"].as_u64().unwrap() >= 1);
        assert!(o["build_sha"].as_str().unwrap().len() == 40);
        assert!(service.handle("observation.create", &json!({"projectId": "wordfight", "kind": "boring"})).is_err());
        // More play in the same session, as a player who always takes the longest word.
        let out = std::process::Command::new(PYTHON).args(["wordfight.py", "profile=greedy", "runs=4", "seed=1"]).current_dir(&cwd).env("GUILD_TELEMETRY", &telemetry).output().unwrap();
        assert!(out.status.success());
        let done = service.handle("play.finish", &json!({"playtestId": pid})).unwrap();
        assert_eq!(done["status"], "finished");
        assert!(done["event_count"].as_u64().unwrap() > 20);
        assert!(!Path::new(&cwd).exists(), "the play checkout is removed");
        assert_eq!(service.handle("play.finish", &json!({"playtestId": pid})).unwrap()["status"], "finished", "finishing twice is harmless");
        // What the telemetry shows, next to what the design wants.
        let s = service.handle("signals", &json!({"projectId": "wordfight", "kind": "human"})).unwrap();
        assert_eq!(s["human"], 1);
        assert!(s["avoid"].as_array().unwrap().iter().any(|a| a == "always choosing the longest word"));
        let longest = s["signals"].as_array().unwrap().iter().find(|x| x["key"] == "max_attribute:length").expect("the longest word wins signal");
        assert!(longest["value"].as_f64().unwrap() >= 0.7, "{longest}");
        // The observation becomes a question that points back to it.
        let p = service.handle("observation.promote", &json!({"projectId": "wordfight", "observationId": "OBS-001", "systemId": "combat"})).unwrap();
        assert_eq!(p["question"]["id"], "Q-001");
        assert_eq!(p["question"]["source"], "OBS-001");
        assert_eq!(p["question"]["raised_by"], "playtest");
        assert!(p["question"]["context"].as_str().unwrap().contains("round"));
        assert_eq!(p["observation"]["question_id"], "Q-001");
        assert!(service.handle("observation.promote", &json!({"projectId": "wordfight", "observationId": "OBS-001"})).is_err());
        let snap = service.snapshot().unwrap();
        assert_eq!(snap.observations.len(), 1);
        assert!(snap.designs[0].systems.iter().find(|x| x.id == "combat").unwrap().questions.contains(&"Q-001".to_string()));
        let kinds: Vec<String> = service.store.lock().unwrap().events_after(0, 10_000).unwrap().into_iter().map(|e| e.kind).collect();
        for k in ["playtest.started", "observation.created", "playtest.finished", "question.created"] { assert!(kinds.contains(&k.to_string()), "missing {k}"); }
    }

    #[test]
    fn repeated_commands_with_a_key_do_not_run_twice() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        let first = service.handle("task.create", &json!({"questId": quest_id, "title": "Once", "idempotencyKey": "k1"})).unwrap();
        let second = service.handle("task.create", &json!({"questId": quest_id, "title": "Once", "idempotencyKey": "k1"})).unwrap();
        assert_eq!(first, second);
        assert_eq!(service.snapshot().unwrap().tasks.len(), 1);
    }

    #[test]
    fn feedback_is_written_filtered_moved_along_and_survives_the_snapshot() {
        let (_home, service, _quest) = setup(MockBehaviour::Fail("unused".into()));
        // A failure of the guild, from a project, pointing at its evidence.
        let f = service.handle("feedback.create", &json!({"projectId": "p", "kind": "fails", "cause": "guild", "title": "Pending tasks cannot be edited", "evidence": "cancelled and recreated six times", "refs": ["t1", "t2"], "proposal": "guild task set"})).unwrap();
        assert_eq!(f["id"], "FB-001");
        assert_eq!(f["status"], "open");
        assert_eq!(f["author"], "orchestrator");
        assert_eq!(f["refs"], json!(["t1", "t2"]));
        // Something that worked, from no project in particular.
        let g = service.handle("feedback.create", &json!({"kind": "works", "cause": "role", "title": "Tests that fail first", "evidence": "every member showed the test failing on the old code", "author": "user"})).unwrap();
        assert_eq!(g["id"], "FB-002");
        assert!(g["project_id"].is_null());
        // What it refuses: unknown kind, cause or project, empty evidence.
        assert!(service.handle("feedback.create", &json!({"kind": "meh", "cause": "guild", "title": "x", "evidence": "y"})).is_err());
        assert!(service.handle("feedback.create", &json!({"kind": "fails", "cause": "luck", "title": "x", "evidence": "y"})).is_err());
        assert!(service.handle("feedback.create", &json!({"projectId": "nope", "kind": "fails", "cause": "guild", "title": "x", "evidence": "y"})).is_err());
        assert!(service.handle("feedback.create", &json!({"kind": "fails", "cause": "guild", "title": "x", "evidence": "  "})).is_err());
        // Filters.
        let open = service.handle("feedback.list", &json!({"status": "open"})).unwrap();
        assert_eq!(open.as_array().unwrap().len(), 2);
        let guild = service.handle("feedback.list", &json!({"cause": "guild"})).unwrap();
        assert_eq!(guild.as_array().unwrap().len(), 1);
        assert_eq!(service.handle("feedback.list", &json!({"projectId": "p"})).unwrap().as_array().unwrap().len(), 1);
        assert_eq!(service.handle("feedback.list", &json!({"kind": "works"})).unwrap()[0]["id"], "FB-002");
        // Markdown for an agent's prompt.
        let md = service.handle("feedback.list", &json!({"status": "open", "format": "markdown"})).unwrap();
        assert_eq!(md["count"], 2);
        assert!(md["markdown"].as_str().unwrap().contains("## FB-001"));
        // Along the way: applied needs its commit, discarded needs why.
        assert!(service.handle("feedback.set", &json!({"feedbackId": "FB-001", "status": "applied"})).is_err());
        assert!(service.handle("feedback.set", &json!({"feedbackId": "FB-002", "status": "discarded"})).is_err());
        assert!(service.handle("feedback.set", &json!({"feedbackId": "FB-001", "status": "done"})).is_err());
        let p = service.handle("feedback.set", &json!({"feedbackId": "FB-001", "status": "proposed", "proposal": "guild task set <id> --instructions"})).unwrap();
        assert_eq!(p["status"], "proposed");
        assert_eq!(p["proposal"], "guild task set <id> --instructions");
        let a = service.handle("feedback.set", &json!({"feedbackId": "FB-001", "status": "applied", "commit": "abc1234", "note": "checked with the mock harness"})).unwrap();
        assert_eq!(a["status"], "applied");
        assert_eq!(a["commit"], "abc1234");
        assert_eq!(a["notes"], json!(["checked with the mock harness"]));
        assert!(a["updated_at"].is_string());
        assert!(service.handle("feedback.set", &json!({"feedbackId": "FB-404", "status": "open"})).is_err());
        // In the snapshot, and the next id keeps counting.
        let snap = service.snapshot().unwrap();
        assert_eq!(snap.feedback.len(), 2);
        assert_eq!(snap.feedback[0].status, "applied");
        assert_eq!(service.handle("feedback.create", &json!({"kind": "fails", "cause": "harness", "title": "z", "evidence": "w"})).unwrap()["id"], "FB-003");
    }
}
