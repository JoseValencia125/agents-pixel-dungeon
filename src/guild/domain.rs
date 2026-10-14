//! Entities, states and the transitions the service allows. Everything is
//! plain data with serde so the store keeps it as JSON and the socket
//! carries it unchanged. Ids are ULIDs; times are UTC RFC 3339.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const SCHEMA_VERSION: u32 = 1;

pub fn new_id() -> String { ulid::Ulid::new().to_string().to_lowercase() }

pub fn now() -> String { chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true) }

/// Which coding agent runs an attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessKind { Kiro, Codex, Claude, Pane, Mock }

impl HarnessKind {
    pub fn parse(s: &str) -> Option<HarnessKind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "kiro" => HarnessKind::Kiro,
            "codex" => HarnessKind::Codex,
            "claude" => HarnessKind::Claude,
            "pane" | "herdr" => HarnessKind::Pane,
            "mock" => HarnessKind::Mock,
            _ => return None,
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessKind::Kiro => "kiro",
            HarnessKind::Codex => "codex",
            HarnessKind::Claude => "claude",
            HarnessKind::Pane => "pane",
            HarnessKind::Mock => "mock",
        }
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str(self.as_str()) }
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
    pub created_at: String,
}

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
    pub state: AttemptState,
    pub workspace_id: Option<String>,
    pub session_id: Option<String>,
    /// Increases every time the attempt is re-driven after a restart, so a
    /// stale harness stream cannot finish a newer generation.
    pub generation: u32,
    pub candidate: Option<Candidate>,
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
            | (Running, Verifying) | (Running, Ready) | (Running, Blocked) | (Running, Failed) | (Running, Cancelled)
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
            | (Verifying, Passed) | (Verifying, Failed) | (Verifying, Cancelled)
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

    fn task(id: &str, deps: &[&str]) -> Task {
        Task {
            id: id.into(), quest_id: "q".into(), title: id.into(), instructions: String::new(), acceptance: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(), role: "implementer".into(), member_id: None, model_id: None,
            priority: 0, state: TaskState::Pending, block_reason: None, attempts_made: 0, max_attempts: 2, version: 1,
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
