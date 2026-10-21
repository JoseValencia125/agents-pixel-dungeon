//! Entities, states and the transitions the service allows. Everything is
//! plain data with serde so the store keeps it as JSON and the socket
//! carries it unchanged. Ids are ULIDs; times are UTC RFC 3339.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;

pub fn new_id() -> String { ulid::Ulid::new().to_string().to_lowercase() }

fn one() -> u32 { 1 }

/// Cuts `s` to at most `max` bytes without splitting a character (agents'
/// text is Spanish, French…: a byte cut inside "é" panics). True when it cut.
pub fn cut(s: &mut String, max: usize) -> bool {
    if s.len() <= max { return false; }
    let mut end = max;
    while !s.is_char_boundary(end) { end -= 1; }
    s.truncate(end);
    true
}

pub fn now() -> String { chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true) }

/// Which harness runs an attempt: an open identifier, so the core never
/// branches on a name. The registry maps it to an adapter ("kiro",
/// "codex", "claude", "mock"…); anything the registry lacks is simply
/// unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(transparent)]
pub struct HarnessId(String);

/// The old name, kept for the code that grew up with it.
pub type HarnessKind = HarnessId;

impl HarnessId {
    /// A lowercase slug: letters, digits, `-`, `_`.
    pub fn parse(s: &str) -> Option<HarnessId> {
        let t = s.trim().to_ascii_lowercase();
        let t = if t == "herdr" { "pane".to_string() } else { t };
        if t.is_empty() || !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { return None; }
        Some(HarnessId(t))
    }
    /// An id known at compile time (adapters, tests).
    pub fn named(s: &str) -> HarnessId { HarnessId::parse(s).unwrap_or_else(|| HarnessId("unknown".into())) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for HarnessId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(&self.0) }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub repo_path: String,
    pub target_ref: String,
    /// `argv` run in each new workspace before the agent starts, if any.
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub verifiers: Vec<Verifier>,
    /// How to play the game headless, for playtests.
    #[serde(default)]
    pub playtest: Option<PlaytestRunner>,
    /// How a human plays it, in a terminal; `{seed}` is filled in and
    /// `GUILD_TELEMETRY` names the file the game appends its events to.
    #[serde(default)]
    pub play: Option<PlaytestRunner>,
    /// How an AI player plays it: the game speaks the agent protocol on
    /// its stdio (see `aiplay`); `{seed}` is filled in.
    #[serde(default)]
    pub ai_play: Option<PlaytestRunner>,
    /// Programs its members may run without asking, from `.guild/guild.toml`
    /// (`[permissions] programs`); the ones its own commands run count too.
    #[serde(default)]
    pub permitted_programs: Vec<String>,
    pub created_at: String,
}

/// `argv` run in a build's worktree; `{profile}`, `{seed}` and `{runs}`
/// are filled in. Its stdout carries the telemetry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaytestRunner {
    pub argv: Vec<String>,
    #[serde(default = "playtest_timeout")]
    pub timeout_seconds: u64,
}

fn playtest_timeout() -> u64 { 300 }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verifier {
    pub id: String,
    pub argv: Vec<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_true() -> bool { true }
fn default_timeout() -> u64 { 600 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestState { Draft, Ready, Running, Paused, Blocked, Completed, Failed, Cancelled }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quest {
    pub id: String,
    pub project_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub state: QuestState,
    /// Managed integration branch, `guild/quest/<id>`.
    pub integration_ref: String,
    /// The commit the integration branch was created from.
    pub base_sha: Option<String>,
    pub plan_version: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState { Draft, Pending, Ready, Running, Verifying, Accepted, Integrating, Completed, Blocked, Failed, Cancelled }

impl TaskState {
    pub fn is_terminal(self) -> bool { matches!(self, TaskState::Completed | TaskState::Failed | TaskState::Cancelled) }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub quest_id: String,
    pub title: String,
    pub instructions: String,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub role: String,
    /// A member to prefer; any member with the role otherwise.
    pub member_id: Option<String>,
    /// A model to request from the member's harness; its default otherwise.
    pub model_id: Option<String>,
    pub priority: i32,
    pub state: TaskState,
    /// Why it is blocked, when it is.
    pub block_reason: Option<String>,
    pub attempts_made: u32,
    pub max_attempts: u32,
    /// How the task is executed and how its candidate is chosen.
    #[serde(default)]
    pub strategy: ExecutionStrategy,
    /// How many attempts race for this task at once (1: no race); derived
    /// from the strategy. Each runs on its own member and worktree and is
    /// verified on its own.
    #[serde(default = "one")]
    pub candidates: u32,
    /// Integrate the best passing candidate without asking.
    #[serde(default)]
    pub auto_integrate: bool,
    /// How long an attempt may work on it, when not the guild's default.
    #[serde(default)]
    pub timeout_minutes: Option<u32>,
    /// The candidate that won, once decided.
    #[serde(default)]
    pub winner_attempt_id: Option<String>,
    pub version: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub roles: Vec<String>,
    pub harness: HarnessKind,
    /// The model it asks its harness for; `None` keeps the harness default.
    pub model_id: Option<String>,
    /// Harness mode for the role (Kiro: vibe, plan, semantic_reviewer…).
    pub mode: Option<String>,
    #[serde(default)]
    pub role_prompt: String,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState { Queued, Provisioning, Running, WaitingInput, Submitted, Verifying, Passed, Failed, Cancelled, Lost, TimedOut }

impl AttemptState {
    pub fn is_terminal(self) -> bool {
        matches!(self, AttemptState::Passed | AttemptState::Failed | AttemptState::Cancelled | AttemptState::Lost | AttemptState::TimedOut)
    }
    pub fn is_active(self) -> bool { !self.is_terminal() }
}

/// What a task asks of the guild: one attempt, one attempt reviewed by a
/// judge, a race of candidates, or an experiment (a race whose variants
/// are compared by what they make the game do, not by code quality).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StrategyKind { #[default] Single, Reviewed, CandidateRace, Experiment }

/// Who picks among the candidates that passed the hard gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Selection { #[default] User, Policy, Judge }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionStrategy {
    #[serde(rename = "type", default)]
    pub kind: StrategyKind,
    #[serde(default = "one")]
    pub candidates: u32,
    #[serde(default)]
    pub selection: Selection,
    /// Integrate the chosen candidate without asking.
    #[serde(default)]
    pub auto_integrate: bool,
}

impl Default for ExecutionStrategy {
    fn default() -> Self { ExecutionStrategy { kind: StrategyKind::Single, candidates: 1, selection: Selection::User, auto_integrate: false } }
}

impl ExecutionStrategy {
    /// Runners the strategy wants at once.
    pub fn runners(&self) -> u32 {
        match self.kind { StrategyKind::CandidateRace | StrategyKind::Experiment => self.candidates.max(1), _ => 1 }
    }
    /// `reviewed` is a single attempt a judge looks at.
    pub fn selection(&self) -> Selection {
        if self.kind == StrategyKind::Reviewed { Selection::Judge } else { self.selection }
    }
}

/// Work attempts make candidates; a judge attempt reads them and decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AttemptKind { #[default] Work, Judge, Analysis }

/// One candidate as a decision saw it: anonymous to the judge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionCandidate {
    pub label: String,
    pub attempt_id: String,
    pub member_id: String,
    pub verifiers_passed: bool,
    pub insertions: u32,
    pub deletions: u32,
    #[serde(default)]
    pub score: Option<f64>,
}

/// Why a candidate was chosen: by the policy, a judge or the user. Durable,
/// one per decision, so a choice can be audited and learned from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RaceDecision {
    pub id: String,
    pub task_id: String,
    pub quest_id: String,
    pub project_id: String,
    /// policy, judge or user
    pub by: String,
    #[serde(default)]
    pub judge_attempt_id: Option<String>,
    pub candidates: Vec<DecisionCandidate>,
    /// Per label, the rubric's scores, as the judge gave them.
    #[serde(default)]
    pub scores: serde_json::Value,
    #[serde(default)]
    pub findings: serde_json::Value,
    #[serde(default)]
    pub tradeoffs: String,
    pub winner_attempt_id: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    pub reason: String,
    /// The user chose against an earlier judge or policy decision.
    #[serde(default)]
    pub human_override: bool,
    pub created_at: String,
}

/// One row per finished attempt: what was asked, who did it, how it
/// went and how it was judged. The dataset routing will learn from; no
/// learning yet, only honest rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub task_id: String,
    pub quest_id: String,
    pub project_id: String,
    pub task_title: String,
    pub role: String,
    pub strategy: StrategyKind,
    pub candidates: u32,
    pub selection: Selection,
    pub kind: AttemptKind,
    pub member_id: String,
    pub harness: HarnessId,
    pub model: Option<String>,
    pub result: AttemptState,
    pub attempt_number: u32,
    pub duration_ms: Option<i64>,
    pub turns: u32,
    pub resumed: u32,
    /// What the harness reported, when it did (tokens, cost…).
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
    pub files_changed: Option<u32>,
    pub insertions: Option<u32>,
    pub deletions: Option<u32>,
    pub verifiers_passed: u32,
    pub verifiers_failed: u32,
    pub judge_score: Option<f64>,
    /// Some(true) when chosen, Some(false) when another candidate was; None when no decision applied.
    pub winner: Option<bool>,
    pub decided_by: Option<String>,
    pub human_override: bool,
    pub failure: Option<String>,
    pub created_at: String,
}

/// A commit the attempt left in its worktree, with the shas a proof needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub base_sha: String,
    pub head_sha: String,
    pub tree_sha: String,
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub diff_artifact_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub task_id: String,
    pub task_version: u32,
    pub quest_id: String,
    pub project_id: String,
    pub member_id: String,
    pub harness: HarnessKind,
    pub model_requested: Option<String>,
    pub model_effective: Option<String>,
    #[serde(default)]
    pub kind: AttemptKind,
    pub state: AttemptState,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    /// Increases every time the attempt is re-driven after a restart, so a
    /// stale harness stream cannot finish a newer generation.
    pub generation: u32,
    pub candidate: Option<Candidate>,
    /// What an attempt that stopped before finishing (failed, timed out,
    /// cancelled) had written: committed so a retry goes on from it.
    #[serde(default)]
    pub partial: Option<Candidate>,
    /// What the agent said last, for the room and the inspector.
    pub summary: String,
    /// Last tool activity: read, forge, brew, summon, plan, type, gems.
    pub action: Option<String>,
    /// A permission the harness is waiting on.
    pub pending_permission: Option<PermissionRequest>,
    pub failure: Option<String>,
    #[serde(default)]
    pub verifier_results: Vec<VerifierResult>,
    pub turns: u32,
    /// Times the session was reopened after a restart.
    #[serde(default)]
    pub resumed: u32,
    /// The guild's orchestrator is deciding on it (a permission, the
    /// candidate, a failure): nobody else needs to.
    #[serde(default)]
    pub deciding: bool,
    /// Why the orchestrator left the decision to the delegate or the user.
    #[serde(default)]
    pub escalation: Option<String>,
    /// While it runs, in the snapshot only: how long it has worked (its
    /// permission waits left out) and how long it may.
    #[serde(default)]
    pub worked_seconds: u64,
    #[serde(default)]
    pub limit_seconds: u64,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One run of a project verifier over a candidate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerifierResult {
    pub id: String,
    pub required: bool,
    /// passed | failed | error | skipped
    pub status: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub log_artifact_id: Option<String>,
    /// The last lines of its output, for the inspector.
    pub tail: String,
    /// Images the verifier left in `GUILD_EVIDENCE` (captures of the running
    /// game), kept as artifacts: absolute paths, for whoever reviews.
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub title: String,
    pub detail: String,
    pub options: Vec<PermissionOption>,
    pub asked_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceState { Provisioning, Ready, Active, Frozen, Retained, Removed }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub project_id: String,
    pub attempt_id: String,
    pub path: String,
    pub branch: String,
    pub base_sha: String,
    pub state: WorkspaceState,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub attempt_id: String,
    pub harness: HarnessKind,
    pub native_id: Option<String>,
    pub runtime_version: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub attempt_id: Option<String>,
    pub kind: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub created_at: String,
}

/// A durable fact. `seq` is assigned by the store when it is appended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32,
    pub id: String,
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub occurred_at: String,
    pub source: String,
    pub project_id: Option<String>,
    pub quest_id: Option<String>,
    pub task_id: Option<String>,
    pub attempt_id: Option<String>,
    pub generation: Option<u32>,
    pub payload: serde_json::Value,
}

impl Event {
    pub fn new(kind: &str, source: &str) -> Event {
        Event {
            schema_version: SCHEMA_VERSION,
            id: new_id(),
            seq: 0,
            kind: kind.into(),
            occurred_at: now(),
            source: source.into(),
            project_id: None,
            quest_id: None,
            task_id: None,
            attempt_id: None,
            generation: None,
            payload: serde_json::Value::Null,
        }
    }
    pub fn about_attempt(mut self, a: &Attempt) -> Event {
        self.project_id = Some(a.project_id.clone());
        self.quest_id = Some(a.quest_id.clone());
        self.task_id = Some(a.task_id.clone());
        self.attempt_id = Some(a.id.clone());
        self.generation = Some(a.generation);
        self
    }
    pub fn about_task(mut self, t: &Task, project_id: &str) -> Event {
        self.project_id = Some(project_id.into());
        self.quest_id = Some(t.quest_id.clone());
        self.task_id = Some(t.id.clone());
        self
    }
    pub fn about_quest(mut self, q: &Quest) -> Event {
        self.project_id = Some(q.project_id.clone());
        self.quest_id = Some(q.id.clone());
        self
    }
    pub fn with(mut self, payload: serde_json::Value) -> Event {
        self.payload = payload;
        self
    }
}

/// Everything a client needs to draw the guild, as of `last_seq`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub last_seq: u64,
    pub service_version: String,
    pub projects: Vec<Project>,
    pub quests: Vec<Quest>,
    pub tasks: Vec<Task>,
    pub members: Vec<Member>,
    pub attempts: Vec<Attempt>,
    pub workspaces: Vec<Workspace>,
    pub harnesses: Vec<HarnessStatus>,
    /// One per project that has one.
    #[serde(default)]
    pub decisions: Vec<RaceDecision>,
    #[serde(default)]
    pub hypotheses: Vec<crate::experiment::Hypothesis>,
    #[serde(default)]
    pub questions: Vec<crate::design::DesignQuestion>,
    #[serde(default)]
    pub design_decisions: Vec<crate::design::DesignDecision>,
    #[serde(default)]
    pub experiments: Vec<crate::experiment::Experiment>,
    #[serde(default)]
    pub playtests: Vec<crate::experiment::Playtest>,
    #[serde(default)]
    pub observations: Vec<crate::experiment::Observation>,
    #[serde(default)]
    pub findings: Vec<crate::aiplay::Finding>,
    /// What helped and what got in the way, for improving the guild itself.
    #[serde(default)]
    pub feedback: Vec<crate::feedback::Feedback>,
    #[serde(default)]
    pub designs: Vec<crate::design::GameDesign>,
    #[serde(default)]
    pub roles: Vec<crate::config::Role>,
    /// The guild's chat with its orchestrator, oldest first.
    #[serde(default)]
    pub conversation: Vec<crate::orchestrator::ChatLine>,
    /// The member who orchestrates, and whether it is thinking now.
    #[serde(default)]
    pub orchestrator: Option<String>,
    #[serde(default)]
    pub orchestrator_busy: bool,
    /// How many attempts may run at once.
    #[serde(default)]
    pub concurrency: usize,
}

/// What `probe` found out about an installed harness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarnessStatus {
    pub kind: HarnessKind,
    pub available: bool,
    pub runtime_version: String,
    pub detail: String,
    #[serde(default)]
    pub models: Vec<ModelOption>,
    #[serde(default)]
    pub capabilities: Option<crate::harness::Capabilities>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// Transitions the service accepts; anything else is a bug, so it is an error.
pub fn task_transition(from: TaskState, to: TaskState) -> Result<(), String> {
    use TaskState::*;
    let ok = matches!(
        (from, to),
        (Draft, Pending) | (Draft, Cancelled)
            | (Pending, Ready) | (Pending, Blocked) | (Pending, Cancelled)
            | (Ready, Running) | (Ready, Blocked) | (Ready, Pending) | (Ready, Cancelled)
            | (Running, Verifying) | (Running, Ready) | (Running, Blocked) | (Running, Failed) | (Running, Cancelled) | (Running, Completed)
            | (Verifying, Accepted) | (Verifying, Ready) | (Verifying, Failed) | (Verifying, Blocked) | (Verifying, Cancelled)
            | (Accepted, Integrating) | (Accepted, Ready) | (Accepted, Cancelled)
            | (Integrating, Completed) | (Integrating, Blocked) | (Integrating, Accepted)
            | (Blocked, Pending) | (Blocked, Ready) | (Blocked, Accepted) | (Blocked, Failed) | (Blocked, Cancelled)
            | (Failed, Ready) | (Failed, Pending)
    );
    if ok || from == to { Ok(()) } else { Err(format!("task cannot go from {from:?} to {to:?}")) }
}

pub fn attempt_transition(from: AttemptState, to: AttemptState) -> Result<(), String> {
    use AttemptState::*;
    if from.is_terminal() { return Err(format!("attempt is already {from:?}")); }
    let ok = matches!(
        (from, to),
        (Queued, Provisioning) | (Queued, Cancelled) | (Queued, Failed)
            | (Provisioning, Running) | (Provisioning, Failed) | (Provisioning, Cancelled) | (Provisioning, Lost)
            | (Running, WaitingInput) | (Running, Submitted) | (Running, Failed) | (Running, Cancelled) | (Running, Lost) | (Running, TimedOut)
            | (WaitingInput, Running) | (WaitingInput, Cancelled) | (WaitingInput, Lost) | (WaitingInput, TimedOut) | (WaitingInput, Failed)
            | (Submitted, Verifying) | (Submitted, Passed) | (Submitted, Failed) | (Submitted, Cancelled)
            | (Verifying, Passed) | (Verifying, Failed) | (Verifying, Cancelled) | (Verifying, Submitted)
    );
    if ok || from == to { Ok(()) } else { Err(format!("attempt cannot go from {from:?} to {to:?}")) }
}

/// Which attempt state a room should show, in the widget's vocabulary:
/// working, blocked, idle, done, unknown, plus guild's own verifying,
/// integrating and failed.
pub fn room_state(task: Option<&Task>, attempt: Option<&Attempt>) -> &'static str {
    if let Some(a) = attempt {
        return match a.state {
            AttemptState::Queued | AttemptState::Provisioning => "unknown",
            AttemptState::Running => "working",
            AttemptState::WaitingInput => "blocked",
            AttemptState::Submitted | AttemptState::Verifying => "verifying",
            AttemptState::Passed => "done",
            AttemptState::Failed | AttemptState::TimedOut | AttemptState::Lost => "failed",
            AttemptState::Cancelled => "idle",
        };
    }
    match task.map(|t| t.state) {
        Some(TaskState::Integrating) => "integrating",
        Some(TaskState::Blocked) => "blocked",
        Some(TaskState::Completed) | Some(TaskState::Accepted) => "done",
        _ => "idle",
    }
}

/// Dependencies must exist, stay inside the quest, never point at
/// themselves and never form a cycle.
pub fn validate_dag(tasks: &[Task]) -> Result<(), String> {
    use std::collections::{HashMap, HashSet};
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    for t in tasks {
        for d in &t.depends_on {
            if d == &t.id { return Err(format!("task {} depends on itself", t.id)); }
            match by_id.get(d.as_str()) {
                None => return Err(format!("task {} depends on unknown task {d}", t.id)),
                Some(p) if p.quest_id != t.quest_id => return Err(format!("task {} depends on {d} from another quest", t.id)),
                _ => {}
            }
        }
    }
    // Kahn's algorithm: whatever is left has a cycle.
    let mut indegree: HashMap<&str, usize> = tasks.iter().map(|t| (t.id.as_str(), t.depends_on.len())).collect();
    let mut queue: Vec<&str> = indegree.iter().filter(|(_, n)| **n == 0).map(|(id, _)| *id).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    while let Some(id) = queue.pop() {
        seen.insert(id);
        for t in tasks.iter().filter(|t| t.depends_on.iter().any(|d| d == id)) {
            let n = indegree.get_mut(t.id.as_str()).unwrap();
            *n -= 1;
            if *n == 0 { queue.push(&t.id); }
        }
    }
    if seen.len() != tasks.len() {
        let stuck: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).filter(|id| !seen.contains(id)).collect();
        return Err(format!("dependency cycle among {}", stuck.join(", ")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutting_text_never_splits_a_character() {
        let mut s = "acción".to_string();          // "ó" is bytes 4–5
        assert!(cut(&mut s, 5));
        assert_eq!(s, "acci");
        let mut short = "hola".to_string();
        assert!(!cut(&mut short, 10));
        assert_eq!(short, "hola");
    }

    fn task(id: &str, deps: &[&str]) -> Task {
        Task {
            id: id.into(), quest_id: "q".into(), title: id.into(), instructions: String::new(), acceptance: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(), role: "implementer".into(), member_id: None, model_id: None,
            priority: 0, state: TaskState::Pending, block_reason: None, attempts_made: 0, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, timeout_minutes: None, winner_attempt_id: None, version: 1,
            created_at: now(), updated_at: now(),
        }
    }

    #[test]
    fn a_diamond_is_a_valid_dag() {
        let tasks = vec![task("t1", &[]), task("t2", &["t1"]), task("t3", &["t1"]), task("t4", &["t2", "t3"])];
        assert!(validate_dag(&tasks).is_ok());
    }

    #[test]
    fn cycles_self_loops_and_unknown_parents_are_rejected() {
        assert!(validate_dag(&[task("a", &["b"]), task("b", &["a"])]).unwrap_err().contains("cycle"));
        assert!(validate_dag(&[task("a", &["a"])]).unwrap_err().contains("itself"));
        assert!(validate_dag(&[task("a", &["zz"])]).unwrap_err().contains("unknown"));
    }

    #[test]
    fn terminal_attempts_never_move_again() {
        assert!(attempt_transition(AttemptState::Passed, AttemptState::Running).is_err());
        assert!(attempt_transition(AttemptState::Running, AttemptState::Submitted).is_ok());
        assert!(task_transition(TaskState::Verifying, TaskState::Accepted).is_ok());
        assert!(task_transition(TaskState::Completed, TaskState::Running).is_err());
    }
}
