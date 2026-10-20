//! The orchestrator: the one guild member the user talks to. It answers in
//! the guild's chat and, when the user asks for work, writes a plan the
//! service turns into a quest with tasks; the members take those tasks
//! below, through the scheduler, without the user having to address them.

use crate::domain::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One line of the conversation, as the snapshot carries it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLine {
    /// "user" or "orchestrator".
    pub author: String,
    pub text: String,
    /// The quest the orchestrator started with this reply, if any.
    #[serde(default)]
    pub quest_id: Option<String>,
    pub at: String,
}

/// A task as the orchestrator plans it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlannedTask {
    pub id: String,
    pub title: String,
    pub instructions: String,
    pub acceptance: Vec<String>,
    pub role: String,
    pub depends_on: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Plan {
    pub title: String,
    pub description: String,
    pub tasks: Vec<PlannedTask>,
}

/// The member who orchestrates: the enabled planner, else the first
/// enabled member, on a harness that is available.
pub fn pick(members: &[Member], available: &[HarnessKind]) -> Option<Member> {
    let usable = |m: &&Member| m.enabled && available.contains(&m.harness);
    members.iter().filter(usable).find(|m| m.roles.iter().any(|r| r == "planner")).or_else(|| members.iter().find(usable)).cloned()
}

pub const ROLE_PROMPT: &str = "You are the orchestrator of a game development guild. You are the only member the user talks to; the other members never hear from the user directly. Understand what the user wants, answer briefly in the user's language, and when it needs work, plan it for the members. Do not edit files: read the repository if you need to, the members do the work.";

/// What the orchestrator is told for one message: who is in the guild,
/// what is under way, the design, the conversation so far and the message.
pub fn compose(project: &Project, members: &[Member], snap: &Snapshot, design: Option<&crate::design::GameDesign>, history: &[ChatLine], message: &str) -> String {
    let mut s = String::new();
    s.push_str(ROLE_PROMPT);
    s.push_str(&format!("\n\nThe repository is at {} (read only for you).\n", project.repo_path));
    s.push_str("\n## Members (they take the tasks; a task's role must be one of theirs)\n");
    for m in members.iter().filter(|m| m.enabled) {
        s.push_str(&format!("- {} — {} ({})\n", m.name, m.roles.join(", "), m.harness));
    }
    let open: Vec<&Quest> = snap.quests.iter().filter(|q| q.project_id == project.id && matches!(q.state, QuestState::Running | QuestState::Paused | QuestState::Blocked)).collect();
    if !open.is_empty() {
        s.push_str("\n## Under way\n");
        for q in open {
            s.push_str(&format!("- {} ({:?})\n", q.title, q.state));
            for t in snap.tasks.iter().filter(|t| t.quest_id == q.id) { s.push_str(&format!("  - {} [{}] {:?}\n", t.title, t.role, t.state)); }
        }
    }
    if let Some(d) = design { s.push_str(&crate::design::prompt_context(d)); }
    s.push_str("\n## Guild conversation\n");
    for line in history.iter().rev().take(20).collect::<Vec<_>>().into_iter().rev() {
        s.push_str(&format!("{}: {}\n", if line.author == "user" { "User" } else { "You" }, line.text.trim()));
    }
    s.push_str(&format!("User: {}\n", message.trim()));
    s.push_str("\nAnswer the user in a few sentences. If the message asks for work, end with one fenced ```json block, nothing after it: {\"title\": \"…\", \"description\": \"…\", \"tasks\": [{\"id\": \"t1\", \"title\": \"…\", \"instructions\": \"…\", \"acceptance\": [\"…\"], \"role\": \"<a member's role>\", \"dependsOn\": []}]}. Keep tasks small, each with verifiable acceptance; dependsOn names earlier task ids. When you split one change between members, write the shared interface into every part's instructions (names, signatures, keys, where each thing lives), and end with an integration task that depends on the parts and checks them together: the whole flow tested end to end, and nothing left over from working apart (placeholders, comments about the merge, changes outside the task). With no work to do, give no block.\n");
    s
}

/// The reply without its plan block, and the plan if it has one.
pub fn parse(text: &str) -> (String, Option<Plan>) {
    let Some(start) = text.rfind("```json") else { return (text.trim().to_string(), None) };
    let body = &text[start + 7..];
    let Some(end) = body.find("```") else { return (text.trim().to_string(), None) };
    let plan = serde_json::from_str::<Value>(body[..end].trim()).ok().and_then(|v| serde_json::from_value::<Plan>(v).ok()).filter(|p| !p.tasks.is_empty() && !p.title.trim().is_empty());
    let reply = format!("{}{}", &text[..start], &body[end + 3..]).trim().to_string();
    match plan {
        Some(p) => (reply, Some(p)),
        None => (text.trim().to_string(), None),
    }
}

/// A role some member holds: the planned one, else implementer, else
/// the first member's, so every task can be picked up.
pub fn role_for(planned: &str, members: &[Member]) -> String {
    let held = |r: &str| members.iter().any(|m| m.enabled && m.roles.iter().any(|x| x == r));
    if held(planned) { return planned.into(); }
    if held("implementer") { return "implementer".into(); }
    if held("gameplay-engineer") { return "gameplay-engineer".into(); }
    members.iter().find(|m| m.enabled).and_then(|m| m.roles.first().cloned()).unwrap_or_else(|| "implementer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_block_leaves_the_reply_and_becomes_tasks() {
        let (reply, plan) = parse("Sure, two steps.\n```json\n{\"title\": \"Poison\", \"tasks\": [{\"id\": \"t1\", \"title\": \"Status\", \"role\": \"gameplay-engineer\"}, {\"id\": \"t2\", \"title\": \"Test\", \"dependsOn\": [\"t1\"]}]}\n```\n");
        assert_eq!(reply, "Sure, two steps.");
        let plan = plan.unwrap();
        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.tasks[1].depends_on, vec!["t1".to_string()]);
    }

    #[test]
    fn a_plain_answer_has_no_plan() {
        assert_eq!(parse("Three quests are running."), ("Three quests are running.".to_string(), None));
        assert!(parse("```json\n{\"title\": \"x\", \"tasks\": []}\n```").1.is_none(), "no tasks, no quest");
    }

    #[test]
    fn a_role_nobody_holds_falls_back() {
        let m = Member { id: "a".into(), name: "A".into(), roles: vec!["gameplay-engineer".into()], harness: HarnessId::named("mock"), model_id: None, mode: None, role_prompt: String::new(), enabled: true };
        assert_eq!(role_for("art-director", &[m.clone()]), "gameplay-engineer");
        assert_eq!(role_for("gameplay-engineer", &[m]), "gameplay-engineer");
    }
}
