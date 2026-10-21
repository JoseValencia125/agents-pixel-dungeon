//! Feedback on the guild itself: what helped and what got in the way while
//! orchestrating a project, written by the orchestrator or the user as it
//! happens. It is the input for improving the role prompts, the
//! orchestrator's brief, the skills and the guild; an agent working on this
//! repository reads it with `guild feedback list` (or `export`), applies
//! what is worth applying between quests, and marks each entry.
//!
//! An entry is one observation with its evidence, its cause and its status.
//! Observation and change stay apart: the proposal is text, the change is a
//! commit that `feedback.set` records when it lands.

use serde::{Deserialize, Serialize};

/// Whether it helped (`works`) or got in the way (`fails`).
pub const FEEDBACK_KINDS: [&str; 2] = ["works", "fails"];
/// Where the cause lies: not everything is the member's fault.
pub const FEEDBACK_CAUSES: [&str; 6] = ["role", "brief", "orchestrator", "guild", "harness", "project"];
/// open → proposed (a change is drafted) → applied (with its commit) or discarded (with why).
pub const FEEDBACK_STATUSES: [&str; 4] = ["open", "proposed", "applied", "discarded"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Feedback {
    /// FB-001, FB-002… for the whole guild.
    pub id: String,
    /// The project it came from, if any.
    #[serde(default)]
    pub project_id: Option<String>,
    pub kind: String,
    pub cause: String,
    pub title: String,
    /// What happened, where, how many times.
    pub evidence: String,
    /// Time, attempts or risk it cost.
    #[serde(default)]
    pub cost: Option<String>,
    /// The concrete change and how to check it helps.
    #[serde(default)]
    pub proposal: Option<String>,
    /// Quest, task or attempt ids the evidence points at.
    #[serde(default)]
    pub refs: Vec<String>,
    pub status: String,
    /// Why it was applied or discarded, and anything learned since.
    #[serde(default)]
    pub notes: Vec<String>,
    /// The commit that applied it.
    #[serde(default)]
    pub commit: Option<String>,
    /// Who wrote it: "orchestrator", "user", or a member id.
    pub author: String,
    pub created_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// The entries as Markdown, for an agent's prompt or a human: one section per
/// entry, newest last, with everything it needs to act on it.
pub fn to_markdown(items: &[Feedback]) -> String {
    let mut out = String::from("# Guild feedback\n");
    if items.is_empty() {
        out.push_str("\nNo entries.\n");
        return out;
    }
    for f in items {
        let sign = if f.kind == "works" { "+" } else { "−" };
        out.push_str(&format!("\n## {} · {} {}\n\n", f.id, sign, f.title));
        let project = f.project_id.as_deref().unwrap_or("—");
        out.push_str(&format!("- kind: {} · cause: {} · status: {} · project: {} · by {} · {}\n", f.kind, f.cause, f.status, project, f.author, f.created_at));
        out.push_str(&format!("- evidence: {}\n", f.evidence));
        if let Some(c) = &f.cost { out.push_str(&format!("- cost: {c}\n")); }
        if let Some(p) = &f.proposal { out.push_str(&format!("- proposal: {p}\n")); }
        if !f.refs.is_empty() { out.push_str(&format!("- refs: {}\n", f.refs.join(", "))); }
        if let Some(c) = &f.commit { out.push_str(&format!("- commit: {c}\n")); }
        for n in &f.notes { out.push_str(&format!("- note: {n}\n")); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_carries_everything_an_agent_needs_to_act() {
        let f = Feedback {
            id: "FB-001".into(), project_id: Some("demo".into()), kind: "fails".into(), cause: "guild".into(),
            title: "Pending tasks cannot be edited".into(), evidence: "cancelled and recreated six times".into(),
            cost: Some("noise on the board".into()), proposal: Some("guild task set".into()), refs: vec!["t1".into(), "t2".into()],
            status: "open".into(), notes: vec!["seen again".into()], commit: None, author: "orchestrator".into(),
            created_at: "2026-10-21T16:30:00Z".into(), updated_at: None,
        };
        let md = to_markdown(&[f]);
        for needle in ["## FB-001 · − Pending tasks cannot be edited", "cause: guild", "status: open", "project: demo", "evidence: cancelled", "cost: noise", "proposal: guild task set", "refs: t1, t2", "note: seen again"] {
            assert!(md.contains(needle), "missing {needle:?} in\n{md}");
        }
        assert!(to_markdown(&[]).contains("No entries."));
    }
}
