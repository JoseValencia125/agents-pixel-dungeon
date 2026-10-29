//! The sentinel: what the guild does about work that waits and nobody moves.
//!
//! The orchestrator wakes for three things (a permission, a candidate, a task
//! out of attempts). Everything else was invisible: a task Failed for hours, a
//! merge that failed, a member waiting for an answer, a quest paused with work
//! inside. The sentinel looks at the whole guild every few seconds, records
//! each wait as a [`Stall`] (what, since when, whose it is) and climbs a
//! ladder: a safe remedy or the orchestrator first, then a notice to the
//! delegate, with a growing pause between notices, then one alert to the
//! person. It never does anything destructive on its own.
//!
//! The deciding is a pure function (`observe` and `decide` take the world and
//! the clock and return what to do), so the ladder is a table of cases in the
//! tests. Ideas taken from Gas City's work ladder: the pause grows (3 min,
//! doubling, up to an hour); an alert is sent once per wait; progress of the
//! subject starts the wait over; a reading that cannot be trusted holds
//! everything instead of acting; a quest the person paused is dormant, listed
//! and never escalated; a harness at its usage limit is not a stalled member.

use crate::domain::*;
use chrono::{DateTime, Duration as Span, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StallKind {
    /// Accepted but never landed: git or the disk failed, or it stayed blocked by a failed merge.
    IntegrationStuck,
    /// A task Failed or Blocked that nobody retried, cancelled or integrated.
    FailedUnattended,
    /// A candidate that went through its verifiers and nobody decided.
    CandidateWaiting,
    /// A member waiting for an answer to a permission.
    InputWaiting,
    /// A member working that has shown nothing for a long time, and runs nothing.
    SilentRunning,
    /// Verifiers running for far longer than they should.
    VerifyingHung,
    /// A task waiting on a dependency that failed, was cancelled, or is done but did not release it.
    DependencyStalled,
    /// The orchestrator could not decide and left it to the delegate; nobody answered.
    OrchestratorUndecided,
    /// A paused quest with work inside that is neither done nor going anywhere.
    PausedWithWork,
    /// A member that keeps working but whose worktree has not changed for a long time.
    NoProgress,
}

impl StallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StallKind::IntegrationStuck => "integration_stuck",
            StallKind::FailedUnattended => "failed_unattended",
            StallKind::CandidateWaiting => "candidate_waiting",
            StallKind::InputWaiting => "input_waiting",
            StallKind::SilentRunning => "silent_running",
            StallKind::VerifyingHung => "verifying_hung",
            StallKind::DependencyStalled => "dependency_stalled",
            StallKind::OrchestratorUndecided => "orchestrator_undecided",
            StallKind::PausedWithWork => "paused_with_work",
            StallKind::NoProgress => "no_progress",
        }
    }

    pub const ALL: [StallKind; 10] = [
        StallKind::IntegrationStuck, StallKind::FailedUnattended, StallKind::CandidateWaiting, StallKind::InputWaiting, StallKind::SilentRunning,
        StallKind::VerifyingHung, StallKind::DependencyStalled, StallKind::OrchestratorUndecided, StallKind::PausedWithWork, StallKind::NoProgress,
    ];

    /// Who should move it first.
    pub fn owner(self) -> &'static str {
        match self {
            StallKind::IntegrationStuck => "guild",
            StallKind::FailedUnattended | StallKind::CandidateWaiting | StallKind::InputWaiting | StallKind::DependencyStalled | StallKind::VerifyingHung => "orchestrator",
            StallKind::SilentRunning | StallKind::NoProgress => "guild",
            StallKind::OrchestratorUndecided => "delegate",
            StallKind::PausedWithWork => "user",
        }
    }
}

/// One wait of the guild that nobody is moving.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stall {
    /// `<kind>:<subject>`: one per wait.
    pub id: String,
    pub kind: StallKind,
    /// The task (or attempt, or quest) that waits.
    pub subject: String,
    pub task_id: String,
    pub quest_id: String,
    pub project_id: String,
    pub reason: String,
    /// When the wait began (the subject's last change): a change starts a new wait.
    pub since: String,
    pub owner: String,
    /// Notices sent to the delegate so far.
    #[serde(default)]
    pub notices: u32,
    #[serde(default)]
    pub next_notice: Option<String>,
    /// The person was told once.
    #[serde(default)]
    pub alerted: bool,
    /// Someone said they have it: no more notices or alerts, still listed until it ends.
    #[serde(default)]
    pub acked: bool,
    /// Paused on purpose (its quest is paused): listed, never escalated.
    #[serde(default)]
    pub dormant: bool,
    /// The remedy or the orchestrator was tried.
    #[serde(default)]
    pub tried: bool,
    pub updated_at: String,
}

/// What the guild sees of a running attempt besides its state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Signals {
    /// Since the harness last did anything.
    pub quiet_for: Option<Duration>,
    /// It left something running (a build, a game).
    pub runs_something: bool,
    /// Since its worktree last changed (the health check measures it).
    pub idle_for: Option<Duration>,
}

/// How long a wait is tolerated before it is a stall, and how the notices space out.
#[derive(Clone, Debug)]
pub struct Policy {
    pub grace: HashMap<StallKind, Duration>,
    /// The pause before the second notice; it doubles up to `notice_cap`.
    pub notice_base: Duration,
    pub notice_cap: Duration,
    /// The person is told after this many notices, once.
    pub alert_after: u32,
}

impl Default for Policy {
    fn default() -> Policy {
        let m = |n: u64| Duration::from_secs(n * 60);
        Policy {
            grace: HashMap::from([
                (StallKind::IntegrationStuck, m(2)), (StallKind::FailedUnattended, m(2)), (StallKind::CandidateWaiting, m(5)), (StallKind::InputWaiting, m(5)),
                (StallKind::SilentRunning, m(10)), (StallKind::VerifyingHung, m(20)), (StallKind::DependencyStalled, m(10)), (StallKind::OrchestratorUndecided, m(5)),
                (StallKind::PausedWithWork, Duration::ZERO), (StallKind::NoProgress, m(20)),
            ]),
            notice_base: m(3),
            notice_cap: m(60),
            alert_after: 3,
        }
    }
}

impl Policy {
    pub fn grace_of(&self, kind: StallKind) -> Duration { self.grace.get(&kind).copied().unwrap_or(Duration::from_secs(600)) }

    /// A policy with the graces in `overrides` (minutes by kind name) and `notice_base` / `notice_cap` / `alert_after`.
    pub fn with_overrides(mut self, overrides: &serde_json::Value) -> Policy {
        for kind in StallKind::ALL {
            if let Some(m) = overrides.get(kind.as_str()).and_then(serde_json::Value::as_f64) { self.grace.insert(kind, Duration::from_secs_f64((m * 60.0).max(0.0))); }
        }
        if let Some(m) = overrides.get("notice_base").and_then(serde_json::Value::as_f64) { self.notice_base = Duration::from_secs_f64((m * 60.0).max(1.0)); }
        if let Some(m) = overrides.get("notice_cap").and_then(serde_json::Value::as_f64) { self.notice_cap = Duration::from_secs_f64((m * 60.0).max(1.0)); }
        if let Some(n) = overrides.get("alert_after").and_then(serde_json::Value::as_u64) { self.alert_after = n as u32; }
        self
    }
}

/// A wait the world shows right now.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub kind: StallKind,
    pub subject: String,
    pub task_id: String,
    pub quest_id: String,
    pub project_id: String,
    pub reason: String,
    pub since: DateTime<Utc>,
    /// Its quest is paused or blocked: listed, never escalated.
    pub dormant: bool,
    /// Something makes the reading unreliable now (a harness at its limit, a start in progress): nothing is counted or done.
    pub held: bool,
}

impl Seen {
    pub fn id(&self) -> String { format!("{}:{}", self.kind.as_str(), self.subject) }
}

/// What the sentinel does about a wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A safe remedy, or the orchestrator, once.
    Try,
    /// Wake the delegate: the nth notice.
    Notice(u32),
    /// Tell the person, once.
    Alert,
}

/// Everything `observe` looks at.
pub struct World<'a> {
    pub now: DateTime<Utc>,
    pub projects: &'a [Project],
    pub quests: &'a [Quest],
    pub tasks: &'a [Task],
    pub attempts: &'a [Attempt],
    pub paused_members: &'a [PausedMember],
    pub signals: &'a HashMap<String, Signals>,
}

fn at(s: &str) -> Option<DateTime<Utc>> { DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc)) }

fn is_merge_failure(reason: &Option<String>) -> bool { reason.as_deref().is_some_and(|r| r.starts_with("merge failed") || r.starts_with("merge blocked")) }

/// Every wait the guild shows now. Silence about something does not mean it is fine:
/// a subject whose time cannot be read is left out, and the caller keeps what it knew.
pub fn observe(w: &World) -> Vec<Seen> {
    let mut seen = vec![];
    let quest = |id: &str| w.quests.iter().find(|q| q.id == id);
    let signal = |attempt: &str| w.signals.get(attempt).copied().unwrap_or_default();
    for t in w.tasks {
        let Some(q) = quest(&t.quest_id) else { continue };
        // A quest that is over or not started has no waits.
        if !matches!(q.state, QuestState::Running | QuestState::Paused | QuestState::Blocked) { continue; }
        let dormant = q.state != QuestState::Running;
        let Some(updated) = at(&t.updated_at) else { continue };
        let mk = |kind: StallKind, subject: String, reason: String, since: DateTime<Utc>, held: bool| Seen {
            kind, subject, task_id: t.id.clone(), quest_id: t.quest_id.clone(), project_id: q.project_id.clone(), reason, since, dormant, held,
        };
        let attempts: Vec<&Attempt> = w.attempts.iter().filter(|a| a.task_id == t.id).collect();
        let latest = attempts.iter().max_by(|a, b| a.created_at.cmp(&b.created_at)).copied();
        // The orchestrator looked and left it to someone: that is the wait that climbs, the other one rests.
        let undecided = latest.filter(|a| a.escalation.is_some() && !a.deciding && !matches!(t.state, TaskState::Completed | TaskState::Cancelled | TaskState::Integrating | TaskState::Accepted));
        if let Some(a) = undecided {
            if let Some(since) = at(&a.updated_at) { seen.push(mk(StallKind::OrchestratorUndecided, a.id.clone(), a.escalation.clone().unwrap_or_default(), since, false)); }
        }

        match t.state {
            TaskState::Accepted if is_merge_failure(&t.block_reason) => seen.push(mk(StallKind::IntegrationStuck, t.id.clone(), t.block_reason.clone().unwrap_or_default(), updated, false)),
            TaskState::Blocked if is_merge_failure(&t.block_reason) => seen.push(mk(StallKind::IntegrationStuck, t.id.clone(), t.block_reason.clone().unwrap_or_default(), updated, false)),
            TaskState::Integrating => seen.push(mk(StallKind::IntegrationStuck, t.id.clone(), "it has been integrating for too long".into(), updated, false)),
            TaskState::Blocked if t.block_reason.as_deref().is_some_and(|r| r.starts_with("dependency")) => {
                seen.push(mk(StallKind::DependencyStalled, t.id.clone(), t.block_reason.clone().unwrap_or_default(), updated, false));
            }
            TaskState::Failed | TaskState::Blocked => {
                let resting = latest.is_some_and(|a| a.deciding) || undecided.is_some();
                seen.push(mk(StallKind::FailedUnattended, t.id.clone(), t.block_reason.clone().unwrap_or_else(|| format!("{:?}", t.state).to_lowercase()), updated, resting));
            }
            TaskState::Pending => {
                let done = |id: &String| w.tasks.iter().any(|d| &d.id == id && d.state == TaskState::Completed);
                if !t.depends_on.is_empty() && t.depends_on.iter().all(done) {
                    seen.push(mk(StallKind::DependencyStalled, t.id.clone(), "its dependencies are done but it has not been released".into(), updated, false));
                }
            }
            _ => {}
        }

        for a in attempts {
            let Some(since) = at(&a.updated_at) else { continue };
            let member_paused = w.paused_members.iter().any(|p| p.member_id == a.member_id);
            match a.state {
                AttemptState::Submitted if !t.state.is_terminal() => {
                    let resting = a.deciding || a.escalation.is_some();
                    seen.push(mk(StallKind::CandidateWaiting, a.id.clone(), "its verifiers ran and nobody decided".into(), since, resting));
                }
                AttemptState::WaitingInput => {
                    let what = a.pending_permission.as_ref().map(|p| p.title.trim().to_string()).unwrap_or_else(|| "an answer".into());
                    seen.push(mk(StallKind::InputWaiting, a.id.clone(), format!("a member waits for {what}"), since, a.deciding || a.escalation.is_some() || member_paused));
                }
                AttemptState::Verifying => {
                    // Verifiers may run as long as they declare, and once more with twice the time (FB-072).
                    let budget = w.projects.iter().find(|p| p.id == a.project_id).map(|p| p.verifiers.iter().map(|v| v.timeout_seconds).sum::<u64>() * 3).unwrap_or(0);
                    let waited = (w.now - since).to_std().unwrap_or_default();
                    seen.push(mk(StallKind::VerifyingHung, a.id.clone(), "its verifiers are still running".into(), since, waited < Duration::from_secs(budget)));
                }
                AttemptState::Running => {
                    let s = signal(&a.id);
                    // It works but nothing lands in its worktree: not while it runs something, nor when
                    // it is silent too (that is the other wait's).
                    if let Some(idle) = s.idle_for {
                        let busy = s.runs_something || member_paused || s.quiet_for.is_none_or(|q| q >= Duration::from_secs(300));
                        seen.push(mk(StallKind::NoProgress, a.id.clone(), format!("its worktree has not changed for {} min", idle.as_secs() / 60), w.now - Span::from_std(idle).unwrap_or_else(|_| Span::zero()), busy));
                    }
                    let Some(quiet) = s.quiet_for else { continue };
                    let limit = if a.limit_seconds > 0 { a.limit_seconds } else { 1800 };
                    // It waits for something it started: not silent, and the guild wakes it when that ends.
                    let held = s.runs_something || member_paused;
                    let silent_since = w.now - Span::from_std(quiet).unwrap_or_else(|_| Span::zero());
                    seen.push(mk(StallKind::SilentRunning, a.id.clone(), format!("nothing from it for {} min (limit {} min)", quiet.as_secs() / 60, limit / 60), silent_since, held));
                }
                _ => {}
            }
        }
    }
    // A paused quest with work inside: one line, dormant by nature.
    for q in w.quests.iter().filter(|q| q.state == QuestState::Paused) {
        let open: Vec<&Task> = w.tasks.iter().filter(|t| t.quest_id == q.id && matches!(t.state, TaskState::Accepted | TaskState::Blocked | TaskState::Failed | TaskState::Verifying)).collect();
        if open.is_empty() { continue; }
        let Some(since) = at(&q.updated_at) else { continue };
        seen.push(Seen { kind: StallKind::PausedWithWork, subject: q.id.clone(), task_id: String::new(), quest_id: q.id.clone(), project_id: q.project_id.clone(),
            reason: format!("paused with {} task(s) not done: {}", open.len(), open.iter().take(3).map(|t| t.id.as_str()).collect::<Vec<_>>().join(", ")), since, dormant: true, held: false });
    }
    seen
}

/// One step of the ladder for one wait. `prev` is what was recorded before (if anything),
/// the answer is what to record now (None: not a stall yet) and what to do.
pub fn decide(prev: Option<&Stall>, seen: &Seen, policy: &Policy, now: DateTime<Utc>) -> (Option<Stall>, Vec<Action>) {
    // Progress of its subject starts the wait over.
    let prev = prev.filter(|p| p.since == seen.since.to_rfc3339());
    // A reading that cannot be trusted changes nothing.
    if seen.held { return (prev.cloned(), vec![]); }
    let age = (now - seen.since).to_std().unwrap_or_default();
    if prev.is_none() && age < policy.grace_of(seen.kind) { return (None, vec![]); }
    let mut s = prev.cloned().unwrap_or_else(|| Stall {
        id: seen.id(), kind: seen.kind, subject: seen.subject.clone(), task_id: seen.task_id.clone(), quest_id: seen.quest_id.clone(), project_id: seen.project_id.clone(),
        reason: String::new(), since: seen.since.to_rfc3339(), owner: seen.kind.owner().into(), notices: 0, next_notice: None, alerted: false, acked: false, dormant: false, tried: false,
        updated_at: now.to_rfc3339(),
    });
    s.reason = seen.reason.clone();
    s.dormant = seen.dormant || seen.kind == StallKind::PausedWithWork;
    s.updated_at = now.to_rfc3339();
    let mut actions = vec![];
    if s.dormant || s.acked { return (Some(s), actions); }
    if !s.tried {
        s.tried = true;
        actions.push(Action::Try);
        // The first notice comes after one pause, giving the remedy time.
        s.next_notice = Some((now + Span::from_std(policy.notice_base).unwrap_or_else(|_| Span::minutes(3))).to_rfc3339());
        return (Some(s), actions);
    }
    let due = s.next_notice.as_deref().and_then(at).is_none_or(|n| now >= n);
    if due {
        s.notices += 1;
        actions.push(Action::Notice(s.notices));
        // The pause doubles up to the cap: 3 min, 6, 12, … 60, then every hour.
        let pause = policy.notice_base.saturating_mul(1 << (s.notices - 1).min(10)).min(policy.notice_cap);
        s.next_notice = Some((now + Span::from_std(pause).unwrap_or_else(|_| Span::hours(1))).to_rfc3339());
        if s.notices >= policy.alert_after && !s.alerted {
            s.alerted = true;
            actions.push(Action::Alert);
        }
    }
    (Some(s), actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(kind: StallKind, since: DateTime<Utc>) -> Seen {
        Seen { kind, subject: "t1".into(), task_id: "t1".into(), quest_id: "q".into(), project_id: "p".into(), reason: "why".into(), since, dormant: false, held: false }
    }

    fn mins(n: i64) -> Span { Span::minutes(n) }

    #[test]
    fn a_wait_is_a_stall_only_after_its_grace() {
        let now = Utc::now();
        let policy = Policy::default();
        let (s, a) = decide(None, &seen(StallKind::FailedUnattended, now - mins(1)), &policy, now);
        assert!(s.is_none() && a.is_empty(), "two minutes of grace");
        let (s, a) = decide(None, &seen(StallKind::FailedUnattended, now - mins(3)), &policy, now);
        assert!(s.is_some());
        assert_eq!(a, vec![Action::Try], "first the remedy or the orchestrator, once");
    }

    #[test]
    fn the_notices_space_out_and_the_person_is_told_once() {
        let policy = Policy::default();
        let since = Utc::now() - mins(10);
        let mut now = since + mins(6);
        let kind = StallKind::CandidateWaiting;
        let (mut stall, a) = decide(None, &seen(kind, since), &policy, now);
        assert_eq!(a, vec![Action::Try]);
        let mut log = vec![];
        // Every minute for five hours.
        for _ in 0..300 {
            now += mins(1);
            let (s, actions) = decide(stall.as_ref(), &seen(kind, since), &policy, now);
            stall = s;
            for action in actions { log.push((now, action)); }
        }
        let notices: Vec<_> = log.iter().filter_map(|(at, a)| matches!(a, Action::Notice(_)).then_some(*at)).collect();
        let gaps: Vec<i64> = notices.windows(2).map(|w| (w[1] - w[0]).num_minutes()).collect();
        assert_eq!(&gaps[..5], &[3, 6, 12, 24, 48], "doubling up to the cap: {gaps:?}");
        assert!(gaps[5..].iter().all(|g| *g == 60), "up to the cap, then every hour: {gaps:?}");
        assert_eq!(log.iter().filter(|(_, a)| *a == Action::Alert).count(), 1, "one alert for the whole wait");
        let alert_at = log.iter().find(|(_, a)| *a == Action::Alert).unwrap().0;
        assert_eq!(notices.iter().position(|n| *n == alert_at), Some(2), "with the third notice");
    }

    #[test]
    fn a_change_of_its_subject_starts_the_wait_over() {
        let policy = Policy::default();
        let now = Utc::now();
        let since = now - mins(30);
        let (stall, _) = decide(None, &seen(StallKind::FailedUnattended, since), &policy, now);
        let mut stall = stall.unwrap();
        stall.notices = 2;
        stall.alerted = true;
        // The task moved (a retry, say) and failed again a minute ago: it is a new wait, with its grace.
        let (again, actions) = decide(Some(&stall), &seen(StallKind::FailedUnattended, now - mins(1)), &policy, now);
        assert!(again.is_none() && actions.is_empty());
    }

    #[test]
    fn a_held_reading_changes_nothing_and_a_dormant_or_acked_wait_is_never_escalated() {
        let policy = Policy::default();
        let now = Utc::now();
        let since = now - mins(30);
        let (stall, _) = decide(None, &seen(StallKind::InputWaiting, since), &policy, now);
        let stall = stall.unwrap();
        let mut held = seen(StallKind::InputWaiting, since);
        held.held = true;
        let (kept, actions) = decide(Some(&stall), &held, &policy, now + mins(100));
        assert_eq!((kept.as_ref(), actions.len()), (Some(&stall), 0), "a harness at its limit is not a member that stalls");
        let (none, _) = decide(None, &held, &policy, now);
        assert!(none.is_none(), "and nothing is created from it");
        let mut dormant = seen(StallKind::FailedUnattended, since);
        dormant.dormant = true;
        let (d, actions) = decide(None, &dormant, &policy, now);
        assert!(d.unwrap().dormant && actions.is_empty(), "a paused quest is listed, not escalated");
        let mut acked = stall.clone();
        acked.acked = true;
        acked.tried = true;
        let (_, actions) = decide(Some(&acked), &seen(StallKind::InputWaiting, since), &policy, now + mins(500));
        assert!(actions.is_empty());
    }

    fn task(id: &str, quest: &str, state: TaskState, reason: Option<&str>, updated: DateTime<Utc>) -> Task {
        let mut t: Task = serde_json::from_value(serde_json::json!({
            "id": id, "quest_id": quest, "title": id, "instructions": "", "acceptance": [], "depends_on": [], "role": "implementer", "member_id": null, "model_id": null, "priority": 0, "state": "draft", "block_reason": null, "attempts_made": 0, "max_attempts": 2,
            "created_at": updated.to_rfc3339(), "updated_at": updated.to_rfc3339(), "version": 1
        })).unwrap();
        t.state = state;
        t.block_reason = reason.map(str::to_string);
        t
    }

    fn quest(id: &str, state: QuestState, updated: DateTime<Utc>) -> Quest {
        Quest { id: id.into(), project_id: "p".into(), title: id.into(), description: String::new(), state, integration_ref: format!("refs/heads/guild/quest/{id}"), base_sha: None, read_paths: vec![], notes: vec![], plan_version: 1, created_at: updated.to_rfc3339(), updated_at: updated.to_rfc3339() }
    }

    #[test]
    fn the_guild_shows_what_it_was_blind_to() {
        // The case that found this: a task Failed for hours, one blocked by a merge, one accepted and forgotten in a paused quest.
        let now = Utc::now();
        let old = now - Span::hours(7);
        let quests = [quest("run", QuestState::Running, old), quest("paused", QuestState::Paused, old), quest("done", QuestState::Completed, old)];
        let tasks = [
            task("failed", "run", TaskState::Failed, Some("3 attempts failed; last: timed out"), old),
            task("merge", "run", TaskState::Blocked, Some("merge blocked: git worktree add failed"), old),
            task("dep", "run", TaskState::Blocked, Some("dependency failed: failed"), old),
            task("fine", "run", TaskState::Running, None, now),
            task("forgot", "paused", TaskState::Accepted, None, old),
            task("over", "done", TaskState::Failed, None, old),
        ];
        let signals = HashMap::new();
        let world = World { now, projects: &[], quests: &quests, tasks: &tasks, attempts: &[], paused_members: &[], signals: &signals };
        let found: Vec<(StallKind, String, bool)> = observe(&world).into_iter().map(|s| (s.kind, s.subject, s.dormant)).collect();
        assert!(found.contains(&(StallKind::FailedUnattended, "failed".into(), false)), "{found:?}");
        assert!(found.contains(&(StallKind::IntegrationStuck, "merge".into(), false)));
        assert!(found.contains(&(StallKind::DependencyStalled, "dep".into(), false)));
        assert!(found.contains(&(StallKind::PausedWithWork, "paused".into(), true)));
        assert_eq!(found.len(), 4, "nothing about the running task, nor about a quest that is over: {found:?}");
    }

    #[test]
    fn a_member_is_silent_only_when_it_shows_nothing_and_runs_nothing() {
        let now = Utc::now();
        let quests = [quest("q", QuestState::Running, now)];
        let tasks = [task("t", "q", TaskState::Running, None, now)];
        let mut a: Attempt = serde_json::from_value(serde_json::json!({
            "id": "a1", "task_id": "t", "task_version": 1, "quest_id": "q", "project_id": "p", "member_id": "m", "harness": "claude", "kind": "work", "state": "running",
            "generation": 1, "summary": "", "turns": 1, "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(), "limit_seconds": 1800
        })).unwrap();
        a.state = AttemptState::Running;
        let attempts = [a];
        let run = |signals: HashMap<String, Signals>, paused: &[PausedMember]| -> Vec<Seen> {
            observe(&World { now, projects: &[], quests: &quests, tasks: &tasks, attempts: &attempts, paused_members: paused, signals: &signals }).into_iter().filter(|s| s.kind == StallKind::SilentRunning).collect()
        };
        let quiet = Duration::from_secs(25 * 60);
        let silent = run(HashMap::from([("a1".into(), Signals { quiet_for: Some(quiet), runs_something: false, ..Default::default() })]), &[]);
        assert_eq!(silent.len(), 1);
        assert!(!silent[0].held && (now - silent[0].since).num_minutes() == 25, "its wait began when it went quiet");
        let building = run(HashMap::from([("a1".into(), Signals { quiet_for: Some(quiet), runs_something: true, ..Default::default() })]), &[]);
        assert!(building[0].held, "a build it waits for is not silence");
        assert!(run(HashMap::new(), &[]).is_empty(), "no signal, no verdict");
    }

    /// Reads the snapshot in `GUILD_SNAPSHOT` (`guild snapshot > file`) and prints what the sentinel would see
    /// there, to check the rules against a real guild before they act on it.
    #[test]
    #[ignore]
    fn what_the_sentinel_sees_in_a_real_snapshot() {
        let Ok(path) = std::env::var("GUILD_SNAPSHOT") else { return };
        let snap: Snapshot = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let signals = HashMap::new();
        let now = Utc::now();
        let world = World { now, projects: &snap.projects, quests: &snap.quests, tasks: &snap.tasks, attempts: &snap.attempts, paused_members: &snap.paused_members, signals: &signals };
        let policy = Policy::default();
        for s in observe(&world) {
            let (stall, _) = decide(None, &s, &policy, now);
            println!("{:<22} {:<26} {:>6} h {} {}{}", s.kind.as_str(), s.subject.chars().rev().take(24).collect::<String>().chars().rev().collect::<String>(), (now - s.since).num_hours(), if s.dormant { "dormant" } else { "       " }, if stall.is_some() { "STALL " } else { "grace " }, s.reason.chars().take(70).collect::<String>());
        }
    }

    #[test]
    fn a_member_that_works_without_changing_its_worktree_is_a_wait_of_its_own() {
        let now = Utc::now();
        let quests = [quest("q", QuestState::Running, now)];
        let tasks = [task("t", "q", TaskState::Running, None, now)];
        let a: Attempt = serde_json::from_value(serde_json::json!({
            "id": "a1", "task_id": "t", "task_version": 1, "quest_id": "q", "project_id": "p", "member_id": "m", "harness": "claude", "kind": "work", "state": "running",
            "generation": 1, "summary": "", "turns": 1, "created_at": now.to_rfc3339(), "updated_at": now.to_rfc3339(), "limit_seconds": 1800
        })).unwrap();
        let attempts = [a];
        let run = |signals: Signals| -> Vec<Seen> {
            let signals = HashMap::from([("a1".to_string(), signals)]);
            observe(&World { now, projects: &[], quests: &quests, tasks: &tasks, attempts: &attempts, paused_members: &[], signals: &signals }).into_iter().filter(|s| s.kind == StallKind::NoProgress).collect()
        };
        let idle = Some(Duration::from_secs(25 * 60));
        let active = Some(Duration::from_secs(20));
        let seen = run(Signals { quiet_for: active, runs_something: false, idle_for: idle });
        assert_eq!(seen.len(), 1);
        assert!(!seen[0].held && (now - seen[0].since).num_minutes() == 25, "it began when the worktree last changed");
        assert!(run(Signals { quiet_for: active, runs_something: true, idle_for: idle })[0].held, "a build in progress is not a stall");
        assert!(run(Signals { quiet_for: Some(Duration::from_secs(25 * 60)), runs_something: false, idle_for: idle })[0].held, "a silent member is the other wait's");
        assert!(run(Signals { quiet_for: active, runs_something: false, idle_for: None }).is_empty(), "nothing is said before the health check has measured it");
        let policy = Policy::default();
        assert!(decide(None, &seen_at(StallKind::NoProgress, now - Span::minutes(10)), &policy, now).0.is_none(), "20 minutes of grace");
        assert!(decide(None, &seen_at(StallKind::NoProgress, now - Span::minutes(25)), &policy, now).0.is_some());
    }

    fn seen_at(kind: StallKind, since: DateTime<Utc>) -> Seen { seen(kind, since) }
}
