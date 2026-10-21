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
    // The guild's orchestrator decides for the members; it never talks to the user.
    let usable = |m: &&Member| m.enabled && available.contains(&m.harness) && !m.roles.iter().all(|r| r == "orchestrator");
    members.iter().filter(usable).find(|m| m.roles.iter().any(|r| r == "planner")).or_else(|| members.iter().find(usable)).cloned()
}

pub const ROLE_PROMPT: &str = "You are the orchestrator of a game development guild. You are the only member the user talks to; the other members never hear from the user directly. Understand what the user wants, answer briefly in the user's language, and when it needs work, plan it for the members. Do not edit files: read the repository if you need to, the members do the work.";

/// How tasks are written, for the guild's own orchestrator and for an
/// agent that orchestrates from its terminal.
pub const TASK_RULES: &str = "Keep tasks small, each with verifiable acceptance. When you split one change between members, write the shared interface into every part's instructions (names, signatures, keys, where each thing lives), and end with an integration task that depends on the parts and checks them together: the whole flow tested end to end, and nothing left over from working apart (placeholders, comments about the merge, changes outside the task).";

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
    s.push_str(&format!("\nAnswer the user in a few sentences. If the message asks for work, end with one fenced ```json block, nothing after it: {{\"title\": \"…\", \"description\": \"…\", \"tasks\": [{{\"id\": \"t1\", \"title\": \"…\", \"instructions\": \"…\", \"acceptance\": [\"…\"], \"role\": \"<a member's role>\", \"dependsOn\": []}}]}}. {TASK_RULES} dependsOn names earlier task ids. With no work to do, give no block.\n"));
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

/// What the guild wakes its orchestrator for: a member waiting on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// A permission the policy left to someone.
    Permission,
    /// A candidate that went through its verifiers, waiting for a choice.
    Candidate,
    /// A task whose attempts ran out.
    Failure,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str { match self { DecisionKind::Permission => "permission", DecisionKind::Candidate => "candidate", DecisionKind::Failure => "failure" } }
    /// The answers it may give; `escalate` leaves it to the delegate or the user.
    pub fn choices(self) -> &'static [&'static str] {
        match self { DecisionKind::Permission => &["allow", "refuse", "escalate"], DecisionKind::Candidate => &["accept", "reject", "escalate"], DecisionKind::Failure => &["retry", "escalate"] }
    }
}

/// The orchestrator's answer: one of its kind's choices and why.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Ruling {
    pub decision: String,
    pub reason: String,
}

/// What the guild's orchestrator may decide alone; anything else goes up.
pub const ORCHESTRATOR_BOUNDS: &str = "Decide alone only what keeps a member's work inside its worktree and the system's temporary folder and is plainly what its task asks for. Escalate, never allow, what reaches outside: the network, credentials, the user's files, git history (push, commit, reset), installing software, deleting work. Accept a candidate when every required verifier passed and the diff and the member's report show each acceptance criterion met. Reject only for a defect you can point to — a failed required verifier, a line in the diff that breaks a criterion or the design — with a reason the member can act on (what, where). Evidence you cannot see (a test the verifiers do not run, a number the report does not give) is a reason to escalate, never to reject; so is a change that departs from the design, or criteria you cannot judge. Retry a failed task only with instructions that change what the member does; escalate when the task itself looks wrong. When unsure, escalate: a wrong yes costs more than a short wait.";

/// The prompt that wakes the orchestrator for one decision. `context` is
/// what the decision is about: the permission asked, the candidate's diff
/// and verifiers, the failure and the member's last words.
pub fn decision_prompt(kind: DecisionKind, task: &Task, member: &str, context: &str, design: Option<&crate::design::GameDesign>) -> String {
    let mut s = format!("## Decision needed: {}\n\n{ORCHESTRATOR_BOUNDS}\n\n", kind.as_str());
    s.push_str(&format!("## The task\n\n**{}** (role {}, member {member})\n\n", task.title, task.role));
    let mut instructions = task.instructions.trim().to_string();
    if crate::domain::cut(&mut instructions, 6000) { instructions.push_str("\n… (cut)"); }
    s.push_str(&instructions);
    if !task.acceptance.is_empty() {
        s.push_str("\n\nAcceptance:\n");
        for ac in &task.acceptance { s.push_str(&format!("- {}: {}\n", ac.id, ac.text)); }
    }
    if let Some(d) = design { s.push_str(&crate::design::prompt_context(d)); }
    s.push_str(&format!("\n## What to decide\n\n{}\n\n", context.trim()));
    s.push_str(&format!("Answer with one fenced ```json block and nothing after it: {{\"decision\": \"{}\", \"reason\": \"one or two sentences\"}}.\n", kind.choices().join("\" | \"")));
    s
}

/// The ruling in the orchestrator's answer, if it gave one of its choices.
pub fn parse_ruling(text: &str, kind: DecisionKind) -> Option<Ruling> {
    let start = text.rfind("```json")?;
    let body = &text[start + 7..];
    let end = body.find("```")?;
    let ruling: Ruling = serde_json::from_str(body[..end].trim()).ok()?;
    kind.choices().contains(&ruling.decision.as_str()).then_some(ruling)
}

/// When to hire each role of the catalogue, for an orchestrator sizing
/// its studio.
fn hire_when(role: &str) -> &'static str {
    match role {
        "planner" => "you are the planner; hire another only to plan a second project",
        "orchestrator" => "first, as soon as there are members: answers their permissions, candidates and failures within its bounds while you talk to the user, and leaves you only what it escalates; a fast model is enough",
        "implementer" => "any code that is not game rules: tooling, build, scaffolding, UI plumbing",
        "systems-designer" => "from the first day of a game: turns the concept into loops, systems, numbers with safe ranges and questions",
        "gameplay-engineer" => "the core of a game's build: rules exactly as designed, data apart from logic, tests that saw the bug fail; two when parts are independent",
        "judge" => "when you race candidates (--strategy race --select judge); never an author of them",
        "reviewer" => "always one once code lands: finds what is wrong with each candidate before you accept it",
        "art-director" => "when the game shows anything: ART_DIRECTION.md and asset briefs before any asset",
        "asset-artist" => "after the art director wrote briefs",
        "sprite-animator" => "characters or effects that move",
        "technical-artist" => "many assets, or an engine import: checks sizes, sheets, palettes, pivots automatically",
        "visual-qa" => "before saying an asset or a screen is done: judges it in the running game",
        "ai-player" => "once the game speaks the agent protocol ([ai_play]): plays as a persona and says what it felt",
        "exploit-hunter" => "once a loop is playable: breaks the economy, finds dominant strategies, reports reproducible findings",
        "design-analyst" => "experiments: reads the playtest metrics against a hypothesis",
        _ => "",
    }
}

/// What an agent that was running on its own reads when it becomes the
/// guild's orchestrator (`guild orchestrate`): who it is now, how it
/// behaves when the user wants a game, the roles it can hire, how it
/// writes tasks, the commands that change every tab the user sees, and
/// the guild as it is.
pub fn brief(snap: &Snapshot, project: Option<&Project>, bin: &str) -> String {
    let g = "guild";
    let quoted = if bin.contains(' ') { format!("'{bin}'") } else { bin.to_string() };
    let pid = project.map(|p| p.id.clone()).unwrap_or_else(|| "<project>".into());
    let mut s = String::new();
    s.push_str("# You are the guild's orchestrator\n\n");
    s.push_str("The user switched this session to guild mode. You keep talking with the user here, in their language, as before; you now also run a guild of agents (its members: Claude, Codex, Kiro… each in its own git worktree) that do the building. The user's dungeon shows your room with tabs — Guild (members and what each does), Quests (the work and what waits for a decision), Design (the game design document) — and every tab reads the guild's state. Change them with the commands below; never write the guild's files by hand. Members never hear from the user: everything they know is in the task you write. Use your own subagents for reading and research if your harness has them; the members are for work that lands in the repository.\n\n");
    match snap.members.iter().find(|m| m.enabled && m.roles.iter().any(|r| r == "orchestrator")) {
        Some(o) => s.push_str(&format!("## You are the delegate, not the orchestrator\n\n{} (`{}`), the guild's orchestrator, answers the members' permissions, candidates and failures within its bounds, so nobody waits while you talk with the user. You see only what it escalates: the NEEDS YOU lines of `guild tool guild_status`, each with its reason. Settle those before and after each answer to the user.\n\n", o.name, o.id)),
        None => s.push_str(&format!("## Hire an orchestrator first\n\nWhile you talk with the user nobody answers the members, and they wait. Before the first quest hire the guild's orchestrator: `{g} member add orchestrator --harness claude --model haiku --role orchestrator` (any harness; a fast model is enough). It decides what it can within its bounds and leaves you the rest. Until it is there, look at `{g} tool guild_status` before and after each answer and settle what waits, the oldest first (each line says how long it has waited).\n\n")),
    }
    s.push_str("## Have an opinion\n\nYou are the studio's lead, not a form. Recommend one option and say why in a line; give the alternative only when the trade-off is real. When a request contradicts the design's pillars, say so and propose what would serve them. Ask only the questions whose answer changes the plan, and put your default next to each one, so silence means yes.\n\n");
    s.push_str("## When the user wants to make a game\n\n");
    s.push_str("1. **Pitch before you plan.** In your first answer propose: a working title, the game in one sentence, three pillars in priority order (each with the yes/no test a choice must pass), the core loop in three to five steps, the first playable slice (one loop, one screen, a few minutes of play, no menus), the stack (what the repository already uses; for an empty one, TypeScript + Vite + Canvas for a 2D web game, or the engine the user names) and the studio you would hire. At most two questions, typically the feeling they want and the platform, each with your default.\n");
    s.push_str(&format!("2. **Write it down at once.** No design yet: `{g} design {pid} template`. Then put the pitch in it: `{g} design {pid} get > /tmp/design.json`, edit, `{g} design {pid} set /tmp/design.json` (title, concept, pillars, anti-pillars, experience, the core loop, each system at maturity `concept` with what is known and unknown). Every unknown becomes `question new`; never invent a number silently — a number needs a safe range or it is a question.\n"));
    s.push_str("3. **Hire for the slice, not for the dream.** Start with the orchestrator, a systems designer, one or two gameplay engineers and a reviewer; an art director when the slice shows anything; the exploit hunter and AI players once there is something to play. Add members when tasks are truly independent, remove them when work is a chain: parallel parts that share seams are where guild work goes wrong. Raise concurrency to the number of members that should work at once.\n");
    s.push_str("4. **The first quest is the slice.** Scaffold, the core loop playable, tests as the verifier, and `[play]` (and `[ai_play]` when it can speak the agent protocol) in `.guild/guild.toml`, so the user can press Play. Decisions you take go to `decision new`; systems move up in maturity as they are built.\n");
    s.push_str("5. **Then loop.** Wait, read each candidate's diff against its acceptance and the design, accept or reject with a reason the member can act on. As soon as something is integrated, run a read-only reviewer on it in parallel with the next work, not only at the end: it finds the accounting and seam errors before you build on them. After every playable build ask the user to play it; turn what they notice into questions, questions into hypotheses, hypotheses into experiments.\n\n");
    s.push_str(&format!("## Writing tasks\n\nA member sees only its task: instructions, acceptance, role. {TASK_RULES} Pin a task to a member with `member`, or race several with `candidates`. A task has 30 minutes of work unless you give it more (`timeoutMinutes`); a member near its limit shows ⚠ in `guild_status`. Keep a long analysis and its write-up in separate tasks. A retry goes on from what the last attempt wrote, even when it timed out: say what to change, not to start over. When you correct the specification in the project's branch during a quest, bring it in with `guild quest sync <quest>` instead of copying it into every task. Before the first quest of a system with flows (money, resources, an economy), start with a read-only review of its specification with a conservation check written by hand — for each resource, what enters, what leaves, who holds it — or its errors come back later as the members' rejections. To change a task nobody has started, edit it (`task set`) instead of cancelling and recreating it.\n\n"));
    s.push_str("## Roles you can hire\n\nEach role's own prompt already frames every attempt its member makes.\n\n");
    for r in &snap.roles {
        s.push_str(&format!("- `{}` ({}) — {}\n", r.id, r.guild, hire_when(&r.id)));
    }
    let harnesses: Vec<String> = snap.harnesses.iter().filter(|h| h.available).map(|h| h.kind.to_string()).collect();
    s.push_str(&format!("\nHarnesses available on this machine: {}.\n\n", if harnesses.is_empty() { "none detected yet (run status)".into() } else { harnesses.join(", ") }));
    s.push_str(&format!("## Commands\n\n`guild` below is `{quoted}`: write that path in every command, a shell keeps no alias between your calls.\n\n"));
    s.push_str(&format!("Guild tab — members and how many work at once:\n- `{g} member add <id> --harness claude|codex|kiro --role R [--role R2] [--model M] [--name N]`\n- `{g} member set <id> [--role R]… [--model M] [--harness H] [--enable|--disable]`  ·  `{g} member remove <id>`\n- `{g} concurrency <N>` (1–16)  ·  `{g} roles`  ·  `{g} models <harness>`\n\n"));
    s.push_str(&format!("Quests tab — work for the members (JSON as the argument, or `-` to read it from stdin):\n- `{g} tool guild_start_quest '{{\"title\": \"…\", \"tasks\": [{{\"id\": \"t1\", \"title\": \"…\", \"instructions\": \"…\", \"acceptance\": [\"…\"], \"role\": \"gameplay-engineer\", \"dependsOn\": []}}]}}'`\n- `{g} tool guild_wait '{{\"questId\": \"…\"}}'` blocks until something needs you (up to 9 min; run it in the background if your harness can and keep talking to the user)\n- `{g} tool guild_task '{{\"taskId\": \"…\"}}'`  ·  `{g} tool guild_diff '{{\"taskId\": \"…\"}}'`\n- `{g} tool guild_decide '{{\"taskId\": \"…\", \"decision\": \"accept|reject|retry|cancel\", \"reason\": \"…\"}}'`  ·  `{g} tool guild_permission '{{\"taskId\": \"…\", \"optionId\": \"…\"}}'`\n- `{g} tool guild_status` the guild at a glance\n- `{g} task set <task> [--instructions …] [--accept …]… [--depends t1,t2 | --no-depends] [--role R] [--member ID]` changes a task nobody has started yet  ·  `[--timeout MIN]` on it or on `task new`\n- `{g} quest sync <quest> [--from REF]` brings the project's branch (a corrected spec) into the quest; later attempts start from it\n\n"));
    s.push_str(&format!("Design tab — the living game design:\n- `{g} design {pid} doc` reads it as the user sees it  ·  `get` as JSON  ·  `set <file|->` replaces it  ·  `template`\n- `{g} design {pid} system set <id> --name N --maturity M --intent … --current … --known … --unknown …`\n- `{g} question new {pid} --question …`  ·  `{g} question resolve {pid} <Q-id> --decision … --why …`  ·  `{g} decision new {pid} --decision … --why …`\n- `{g} design {pid} export` commits game-design/ to the branch guild/design/{pid}, never the checkout\n\nVisual tab — the game's look, which every task's context carries (the user drops images and palettes there):\n- `{g} visual {pid}` reads it  ·  `{g} visual {pid} add <file>… [--caption T] [--kind reference|concept|asset]` an image or a palette file  ·  `{g} visual {pid} palette <name> <#rrggbb>…`  ·  `{g} visual {pid} notes <text>`\n- Write the art direction's rules into its notes and palettes before the first asset task; an art director keeps them, and asset tasks name the palette to use.\n\n"));
    s.push_str("Accepting a task merges it into the quest's branch guild/quest/<id>, never into the user's branch; tell the user the branch when a quest completes.\n\n");
    s.push_str(&format!("Feedback on the guild itself — when something about the guild, a role's prompt, this brief, a harness or your own task writing helps or gets in the way, note it as it happens, with its evidence (quest, task, attempt ids) and its cause; an agent improving the guild reads these later:\n- `{g} feedback new --project {pid} --kind works|fails --cause role|brief|orchestrator|guild|harness|project --title … --evidence … [--cost …] [--proposal …] [--ref ID]…`\n- `{g} feedback list --status open`  ·  `{g} feedback export` (Markdown)  ·  `{g} feedback set <FB-id> --status proposed|applied|discarded [--commit SHA] [--note …]`\nBlame the right cause: a mistake in your own spec is `orchestrator`, not `role`. Never change the guild while a quest runs.\n\n"));
    s.push_str("## The guild now\n\n");
    match project {
        Some(p) => s.push_str(&format!("Project `{}` — {} at {} (quests start from {}).\n", p.id, p.name, p.repo_path, p.target_ref.trim_start_matches("refs/heads/"))),
        None => s.push_str("No project yet: run `guild orchestrate <repository path>` to add it.\n"),
    }
    s.push_str(&format!("Concurrency: {} at once.\n", snap.concurrency.max(1)));
    let members: Vec<&Member> = snap.members.iter().filter(|m| m.enabled).collect();
    if members.is_empty() { s.push_str("Members: none. Hire the studio the work needs.\n"); }
    for m in members { s.push_str(&format!("- {} (`{}`) — {} on {}{}\n", m.name, m.id, m.roles.join(", "), m.harness, m.model_id.as_ref().map(|x| format!(" · {x}")).unwrap_or_default())); }
    for q in snap.quests.iter().filter(|q| project.is_none_or(|p| q.project_id == p.id) && matches!(q.state, QuestState::Running | QuestState::Paused | QuestState::Blocked)) {
        s.push_str(&format!("Quest `{}` {:?}: {}\n", q.id, q.state, q.title));
        for t in snap.tasks.iter().filter(|t| t.quest_id == q.id) { s.push_str(&format!("  - `{}` [{}] {:?}: {}\n", t.id, t.role, t.state, t.title)); }
    }
    match project.and_then(|p| snap.designs.iter().find(|d| d.project_id == p.id)) {
        Some(d) => s.push_str(&crate::design::prompt_context(d)),
        None => s.push_str("Design: none yet.\n"),
    }
    s.push_str("\nNow answer the user: say in one line that you run the guild, and carry on with what they asked. If they had asked for nothing, ask what they want to make.\n");
    s
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
    fn the_brief_names_every_role_real_tools_and_the_guild_as_it_is() {
        let project = Project { id: "ember".into(), name: "Ember".into(), repo_path: "/r/ember".into(), target_ref: "refs/heads/main".into(), setup: vec![], verifiers: vec![], playtest: None, play: None, ai_play: None, permitted_programs: vec![], created_at: String::new() };
        let m = Member { id: "ada".into(), name: "Ada".into(), roles: vec!["systems-designer".into()], harness: HarnessId::named("claude"), model_id: None, mode: None, role_prompt: String::new(), enabled: true };
        let snap = Snapshot { projects: vec![project.clone()], members: vec![m], roles: crate::config::role_catalog(), concurrency: 3, ..Default::default() };
        let b = brief(&snap, Some(&project), "/Applications/A B.app/guild");
        // Structure, not wording: the binary quoted once, the shared task rules, a line per role.
        assert!(b.contains("'/Applications/A B.app/guild'"), "a path with spaces is quoted");
        assert!(b.contains(TASK_RULES));
        for r in crate::config::role_catalog() {
            assert!(b.contains(&format!("- `{}`", r.id)) && !hire_when(&r.id).is_empty(), "role {} has no line", r.id);
        }
        // Every MCP tool it tells the agent to run exists.
        let call: crate::mcp::Call = &|_, _| Err("no service".into());
        let list = crate::mcp::handle(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}), call, &|_| {}).unwrap();
        let tools: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        let named: Vec<&str> = b.split("tool ").skip(1).filter_map(|rest| rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next()).filter(|n| n.starts_with("guild_")).collect();
        assert!(!named.is_empty());
        for n in named { assert!(tools.contains(&n), "the brief names {n}, which the MCP server does not have"); }
        // The guild as it is: its project, members and concurrency.
        assert!(b.contains("/r/ember") && b.contains("`ada`") && b.contains("3 at once"));
    }

    #[test]
    fn a_ruling_is_one_of_its_kinds_choices() {
        let r = parse_ruling("Looks fine.\n```json\n{\"decision\": \"allow\", \"reason\": \"inside its worktree\"}\n```", DecisionKind::Permission).unwrap();
        assert_eq!((r.decision.as_str(), r.reason.as_str()), ("allow", "inside its worktree"));
        assert!(parse_ruling("```json\n{\"decision\": \"accept\"}\n```", DecisionKind::Permission).is_none(), "accept is not a permission's answer");
        assert!(parse_ruling("I would allow it.", DecisionKind::Permission).is_none(), "no block, no ruling");
        assert_eq!(DecisionKind::Failure.choices(), &["retry", "escalate"], "a failure is never accepted");
    }

    #[test]
    fn the_brief_makes_the_agent_a_delegate_once_the_guild_has_an_orchestrator() {
        let member = |id: &str, role: &str| Member { id: id.into(), name: id.to_uppercase(), roles: vec![role.into()], harness: HarnessId::named("claude"), model_id: None, mode: None, role_prompt: String::new(), enabled: true };
        let without = Snapshot { members: vec![member("ada", "gameplay-engineer")], roles: crate::config::role_catalog(), ..Default::default() };
        let b = brief(&without, None, "guild");
        assert!(b.contains("Hire an orchestrator first") && b.contains("--role orchestrator"));
        let with = Snapshot { members: vec![member("ada", "gameplay-engineer"), member("orin", "orchestrator")], roles: crate::config::role_catalog(), ..Default::default() };
        let b = brief(&with, None, "guild");
        assert!(b.contains("You are the delegate") && b.contains("ORIN (`orin`)") && !b.contains("Hire an orchestrator first"));
        // The hall's chat never makes the orchestrator its speaker.
        let available = vec![HarnessId::named("claude")];
        assert_eq!(pick(&[member("orin", "orchestrator"), member("ada", "gameplay-engineer")], &available).map(|m| m.id), Some("ada".into()));
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
