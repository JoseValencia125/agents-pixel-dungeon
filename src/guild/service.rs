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
    /// Events appended inside an open unit of work, sent on commit.
    held: Mutex<Vec<Event>>,
    running: Mutex<HashMap<String, Running>>,
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
        Arc::new(Service {
            store: Mutex::new(store),
            adapters: Mutex::new(HashMap::new()),
            harness_status: Mutex::new(vec![]),
            subscribers: Mutex::new(vec![]),
            held: Mutex::new(vec![]),
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
            if active >= self.settings.concurrency { break; }
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
                generation: 0, candidate: None, summary: String::new(), action: None, pending_permission: None, failure: None, verifier_results: vec![], turns: 0, resumed: 0,
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
            Running { session: SessionRef { id: String::new(), native_id: None }, harness: HarnessId::named("mock"), cancel_requested: false, interrupt_requested: false, started: Instant::now() },
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
            Running { session: SessionRef { id: String::new(), native_id: None }, harness: HarnessId::named("mock"), cancel_requested: false, interrupt_requested: false, started: Instant::now() },
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
        let adapter = self.adapter(member.harness.clone())?;
        let role_for_prompt = if attempt.kind == AttemptKind::Judge { "judge".to_string() } else { task.role.clone() };
        let role_prompt = if member.role_prompt.trim().is_empty() || attempt.kind == AttemptKind::Judge { config::default_role_prompt(&role_for_prompt) } else { member.role_prompt.clone() };
        let options = SessionOptions { cwd: PathBuf::from(&workspace.path), model_id: attempt.model_requested.clone(), mode: member.mode.clone(), role_prompt: role_prompt.clone() };
        let session = adapter.create_session(&options).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let design = self.store.lock().unwrap().design(&project.id)?;
        let prompt = if attempt.kind == AttemptKind::Judge {
            self.compose_judge_prompt(&role_prompt, &task, design.as_ref())?
        } else {
            compose_prompt(&role_prompt, &task, &workspace, design.as_ref())
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
            let slot = running.entry(attempt_id.to_string()).or_insert(Running { session: session.clone(), harness: member.harness.clone(), cancel_requested: false, interrupt_requested: false, started: Instant::now() });
            slot.session = session.clone();
            slot.harness = member.harness.clone();
        }
        self.move_attempt(attempt_id, AttemptState::Running, None)?;
        let handle = adapter.prompt(&session, attempt_id, prompt).map_err(|e| ApiError::new("harness", e.to_string()))?;
        self.emit(Event::new("turn.started", "guild").about_attempt(attempt).with(json!({"turnId": handle.turn_id, "promptChars": prompt.len()})));
        let outcome = self.follow_turn(attempt_id, attempt, adapter, &session, handle);
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
                self.emit(Event::new("permission.requested", "harness").about_attempt(attempt).with(json!({"requestId": request_id, "title": title, "detail": detail, "options": options})));
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
                if let (Some(ws), Some(p), Some(_)) = (&workspace, &project, &candidate) {
                    if !p.verifiers.is_empty() {
                        self.move_attempt(attempt_id, AttemptState::Verifying, None)?;
                        let results = self.run_verifiers(attempt_id, &attempt, p, ws);
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
                self.settle_task(&task.id)
            }
            TurnOutcome::Failed { reason } => {
                self.move_attempt(attempt_id, if reason == "timed out" { AttemptState::TimedOut } else { AttemptState::Failed }, Some(reason.clone()))?;
                if task.candidates > 1 { return self.settle_task(&task.id); }
                self.after_failed_attempt(&task, &reason)
            }
            TurnOutcome::Cancelled => {
                self.move_attempt(attempt_id, AttemptState::Cancelled, Some("cancelled".into()))?;
                if task.candidates > 1 { return self.settle_task(&task.id); }
                let _ = self.move_task(&task.id, TaskState::Blocked, Some("attempt cancelled; retry to continue".into()));
                Ok(())
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
                // Never the user's checkout: a managed branch the user merges when ready.
                let ref_name = crate::design::design_ref(&project.id);
                let (sha, files) = workspaces::publish(&project, &ref_name, &project.target_ref, &format!("guild: game design v{}", d.version), |dir| crate::design::export(dir, &d)).map_err(ApiError::internal)?;
                let files: Vec<String> = files.iter().map(|f| f.rsplit("game-design/").next().map(|r| format!("game-design/{r}")).unwrap_or_else(|| f.clone())).collect();
                self.emit_locked(&store, Event::new("design.exported", "user").with(json!({"projectId": id, "files": files.len(), "ref": ref_name, "sha": sha})));
                Ok(json!({"files": files, "ref": ref_name, "sha": sha}))
            }
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
        // A design kept in the repo comes along, unless the guild already has one:
        // the guild's own design branch first, then the checkout's game-design/.
        let mut design_imported = false;
        if store.design(&project.id)?.is_none() {
            let from_branch = workspaces::read_ref(&project, &crate::design::design_ref(&project.id), |dir| crate::design::import(dir, &project.id)).flatten();
            if let Some(d) = from_branch.or_else(|| crate::design::import(&repo, &project.id)) {
                store.put_design(&d)?;
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
        if let Some(e) = params.get("enabled").and_then(Value::as_bool) { m.enabled = e; }
        store.put_member(&m)?;
        self.emit_locked(&store, Event::new("member.updated", "user").with(json!({"memberId": m.id, "harness": m.harness, "model": m.model_id, "roles": m.roles})));
        Ok(serde_json::to_value(m).unwrap())
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
        d.version = store.design(id)?.map(|x| x.version + 1).unwrap_or(1);
        d.updated_at = now();
        for (i, l) in d.loops.iter_mut().enumerate() { if l.id.trim().is_empty() { l.id = format!("loop-{}", i + 1); } }
        for (i, p) in d.parts.iter_mut().enumerate() { if p.id.trim().is_empty() { p.id = format!("part-{}", i + 1); } }
        store.put_design(&d)?;
        self.emit_locked(&store, Event::new("design.updated", "user").with(json!({"projectId": id, "version": d.version, "loops": d.loops.len(), "parts": d.parts.len()})));
        Ok(serde_json::to_value(d).unwrap())
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
            strategy,
            winner_attempt_id: None,
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
            (r.session.clone(), r.harness.clone())
        };
        let adapter = self.adapter(harness)?;
        adapter.respond_permission(&session, request_id, option.as_deref()).map_err(|e| ApiError::new("harness", e.to_string()))?;
        let store = self.store.lock().unwrap();
        self.atomic(&store, || {
            let mut a = store.attempt(attempt_id)?.ok_or_else(|| ApiError::not_found("attempt"))?;
            a.pending_permission = None;
            a.updated_at = now();
            store.put_attempt(&a)?;
            let _ = self.move_attempt_locked(&store, attempt_id, AttemptState::Running, None);
            self.emit_locked(&store, Event::new("permission.resolved", "user").about_attempt(&a).with(json!({"requestId": request_id, "optionId": option})));
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
        let attempt = match p_opt(params, "attemptId") { Some(id) => id, None => self.latest_submitted(task_id)?.id };
        {
            let store = self.store.lock().unwrap();
            let task = store.task(task_id)?.ok_or_else(|| ApiError::not_found("task"))?;
            let submitted: Vec<Attempt> = store.attempts_of_task(task_id)?.into_iter().filter(|a| a.kind == AttemptKind::Work && matches!(a.state, AttemptState::Submitted | AttemptState::Verifying)).collect();
            let refs: Vec<&Attempt> = submitted.iter().collect();
            let earlier = store.decisions_of_task(task_id)?;
            let override_ = earlier.iter().any(|d| d.winner_attempt_id.as_deref() != Some(attempt.as_str()));
            if let Some(w) = submitted.iter().find(|a| a.id == attempt) {
                let d = RaceDecision { id: new_id(), task_id: task_id.into(), quest_id: task.quest_id.clone(), project_id: w.project_id.clone(), by: "user".into(), judge_attempt_id: None,
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
fn parse_strategy(params: &Value) -> Res<ExecutionStrategy> {
    if let Some(s) = params.get("strategy") {
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

fn compose_prompt(role_prompt: &str, task: &Task, workspace: &Workspace, design: Option<&crate::design::GameDesign>) -> String {
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
        let task = Task { id: "t".into(), quest_id: "q".into(), title: "Tune words".into(), instructions: "…".into(), acceptance: vec![], depends_on: vec![], role: "systems-designer".into(), member_id: None, model_id: None, priority: 0, state: TaskState::Ready, block_reason: None, attempts_made: 0, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, winner_attempt_id: None, version: 1, created_at: now(), updated_at: now() };
        let ws = Workspace { id: "w".into(), attempt_id: "a".into(), project_id: design.project_id.clone(), path: "/tmp/w".into(), branch: "guild/x".into(), base_sha: "0".into(), state: WorkspaceState::Ready, created_at: now(), updated_at: now() };
        let prompt = compose_prompt(&config::default_role_prompt("systems-designer"), &task, &ws, Some(&design));
        assert!(prompt.contains("Avoid: a dominant strategy; always choosing the longest word; obvious decisions; grinding"));
        assert!(prompt.contains("Loop \"Economy\": earn → spend ↺"));
        assert!(prompt.contains("part, a loop or the whole"));
        // Export writes TOML into the repo, and a fresh project add reads it back.
        let exported = service.handle("design.export", &json!({"projectId": design.project_id})).unwrap();
        assert!(exported["files"].as_array().unwrap().len() >= 9);
        assert!(!repo.join("game-design").exists(), "the user's checkout is untouched");
        let listed = workspaces::git(&repo, &["ls-tree", "-r", "--name-only", &crate::design::design_ref(&design.project_id)]).unwrap();
        assert!(listed.contains("game-design/loops/03-loop-3.toml"), "{listed}");
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
        assert!(snap.attempts.iter().filter(|a| a.id != winner).all(|a| a.state == AttemptState::Failed && a.failure.as_deref().unwrap().starts_with("not selected")));
        assert!(snap.attempts.iter().all(|a| a.candidate.is_some()), "every runner left a candidate");
        let kinds: Vec<String> = service.store.lock().unwrap().events_after(0, 2000).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds.iter().filter(|k| *k == "merge.completed").count(), 1, "one merge");
        assert_eq!(kinds.iter().filter(|k| *k == "candidate.rejected").count(), 2);
        assert!(kinds.contains(&"race.finished".to_string()));
        assert!(workspaces::git(&repo, &["ls-tree", "--name-only", &snap.quests[0].integration_ref]).unwrap().contains("hello.txt"));
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
            session_id, generation, candidate, summary: String::new(), action: None, pending_permission: None, failure: None,
            verifier_results: if verified { vec![VerifierResult { id: "v".into(), required: true, status: "passed".into(), exit_code: Some(0), duration_ms: 1, log_artifact_id: None, tail: String::new() }] } else { vec![] },
            turns: 1, resumed: 0, started_at: Some(now()), finished_at: None, created_at: now(), updated_at: now(),
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

    #[test]
    fn repeated_commands_with_a_key_do_not_run_twice() {
        let (_home, service, quest_id) = setup(MockBehaviour::Hang);
        let first = service.handle("task.create", &json!({"questId": quest_id, "title": "Once", "idempotencyKey": "k1"})).unwrap();
        let second = service.handle("task.create", &json!({"questId": quest_id, "title": "Once", "idempotencyKey": "k1"})).unwrap();
        assert_eq!(first, second);
        assert_eq!(service.snapshot().unwrap().tasks.len(), 1);
    }
}
