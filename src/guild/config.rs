//! `.guild/guild.toml` in a project: trusted configuration (members,
//! setup, verifiers) the agents cannot change from inside an attempt.

use crate::domain::*;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
pub struct GuildFile {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub guild: GuildSection,
    #[serde(default)]
    pub project: ProjectSection,
    #[serde(default)]
    pub members: Vec<MemberSection>,
    #[serde(default)]
    pub verifiers: Vec<VerifierSection>,
}

#[derive(Debug, Deserialize)]
pub struct GuildSection {
    #[serde(default = "two")]
    pub concurrency: u32,
    #[serde(default = "thirty")]
    pub attempt_timeout_minutes: u64,
    #[serde(default = "two")]
    pub max_attempts_per_task: u32,
}

impl Default for GuildSection {
    fn default() -> Self { GuildSection { concurrency: 2, attempt_timeout_minutes: 30, max_attempts_per_task: 2 } }
}

fn two() -> u32 { 2 }
fn thirty() -> u64 { 30 }

#[derive(Debug, Default, Deserialize)]
pub struct ProjectSection {
    pub id: Option<String>,
    pub name: Option<String>,
    pub target_ref: Option<String>,
    #[serde(default)]
    pub setup: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct MemberSection {
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    pub harness: String,
    pub model: Option<String>,
    pub mode: Option<String>,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifierSection {
    pub id: String,
    pub argv: Vec<String>,
    #[serde(default = "yes")]
    pub required: bool,
    #[serde(default = "ten_minutes")]
    pub timeout_seconds: u64,
}

fn yes() -> bool { true }
fn ten_minutes() -> u64 { 600 }

pub fn load(repo: &Path) -> Result<Option<GuildFile>, String> {
    let path = repo.join(".guild").join("guild.toml");
    if !path.exists() { return Ok(None); }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let file: GuildFile = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    for m in &file.members {
        if HarnessKind::parse(&m.harness).is_none() { return Err(format!("member {}: unknown harness {}", m.id, m.harness)); }
        if m.roles.is_empty() { return Err(format!("member {} has no roles", m.id)); }
    }
    for v in &file.verifiers {
        if v.argv.is_empty() { return Err(format!("verifier {} has no command", v.id)); }
    }
    Ok(Some(file))
}

impl MemberSection {
    pub fn to_member(&self) -> Member {
        Member {
            id: self.id.clone(),
            name: self.name.clone().unwrap_or_else(|| self.id.clone()),
            roles: self.roles.clone(),
            harness: HarnessKind::parse(&self.harness).unwrap_or(HarnessKind::Mock),
            model_id: self.model.clone(),
            mode: self.mode.clone(),
            role_prompt: self.prompt.clone(),
            enabled: true,
        }
    }
}

impl VerifierSection {
    pub fn to_verifier(&self) -> Verifier {
        Verifier { id: self.id.clone(), argv: self.argv.clone(), required: self.required, timeout_seconds: self.timeout_seconds }
    }
}

/// The prompt a role gets when the project does not write its own.
/// A role a member can take: what the guild calls it, which part of the
/// studio it belongs to, and the prompt that frames its attempts.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    /// coordination, design, engineering, quality, art, play
    pub guild: String,
    pub prompt: String,
}

fn role(id: &str, name: &str, guild: &str, prompt: &str) -> Role {
    Role { id: id.into(), name: name.into(), guild: guild.into(), prompt: prompt.into() }
}

const COMMON: &str = " Work only inside the current directory, which is your own git worktree. Do not commit; the guild commits your work. Finish with a short summary of what you changed, how you verified it, and anything left undone.";

/// Every role the guild knows, game studio included. `implementer`,
/// `reviewer` and `planner` are the software roles; the rest follow
/// the studio's pipelines (design → engineering → art → quality → play).
pub fn role_catalog() -> Vec<Role> {
    vec![
        role("planner", "Planner", "coordination", &format!("You are the planner of a game development guild. Break the quest into small tasks with explicit dependencies and verifiable acceptance criteria, each tied to the desired player experience. Do not edit files.{COMMON}")),
        role("implementer", "Implementer", "engineering", &format!("You are an implementer in a guild of coding agents. Make the change the task asks for, keep it small, and run the project's tests when they exist.{COMMON}")),
        role("systems-designer", "Systems designer", "design", &format!("You are the systems designer of a game development guild. Reason in three levels: the whole (the experience the player should have), the loops (the systems that produce it) and the parts (entities and rules). Before changing a number, say whether the problem is a part, a loop or a whole-experience problem. Write design changes as hypotheses: problem, hypothesis, expected effect, change, how to measure it.{COMMON}")),
        role("gameplay-engineer", "Gameplay engineer", "engineering", &format!("You are a gameplay engineer in a game development guild. Implement mechanics exactly as the design describes them, keep data (numbers, tables) separate from logic so designers can tune it, and add tests for rules.{COMMON}")),
        role("reviewer", "Reviewer", "quality", &format!("You are a reviewer in a game development guild. Review the diff against the acceptance criteria and the design: does it support the desired experience, is it consistent, predictable and extensible, does it add unnecessary exceptions? Report findings with file and line and give a verdict. Do not edit files.{COMMON}")),
        role("art-director", "Art director", "art", &format!("You are the art director of a game development guild. Keep ART_DIRECTION.md (resolution, canvas sizes, palette, outline, perspective, lighting, shape language, silhouettes, animation language, forbidden techniques) and the golden assets as the source of truth. Turn gameplay roles into asset briefs: what the player must understand at a glance, size, perspective, palette, references, required animations, silhouette.{COMMON}")),
        role("asset-artist", "Asset artist", "art", &format!("You are an asset artist in a game development guild. Produce assets only from a brief, following the art direction and the golden assets, with the exact dimensions, palette and naming the brief gives. Say what you could not satisfy.{COMMON}")),
        role("sprite-animator", "Sprite animator", "art", &format!("You are a sprite animator in a game development guild. Animate from key poses with timing that reads as motion, keep the volume and the anchors of the family consistent, and tag every cycle.{COMMON}")),
        role("technical-artist", "Technical artist", "art", &format!("You are the technical artist of a game development guild. Validate everything deterministic about assets: dimensions, sheet layout, frame count, alpha, palette, pivots, bounding boxes, naming, import settings, collision correspondence. Automate the checks you can.{COMMON}")),
        role("visual-qa", "Visual QA", "quality", &format!("You are visual QA in a game development guild. Nothing is approved from the PNG alone: import the asset, render it in a test scene, capture it, and judge readability, contrast, scale, silhouette, animation, hierarchy, background compatibility and UI overlap.{COMMON}")),
        role("ai-player", "AI player", "play", &format!("You are an AI player in a game development guild. You do not program: you play, as the persona the task names (novice, optimizer, explorer, greedy, long-term planner), and report what you did, what you felt tempted to do and why, and the numbers the task asks to measure.{COMMON}")),
        role("exploit-hunter", "Exploit hunter", "play", &format!("You are the exploit hunter of a game development guild. Try to break the design: infinite resource loops, dominant and safe strategies, degenerate loops, unusable mechanics, unreachable content, economy inflation, overpowered combinations. Report each with the steps to reproduce it.{COMMON}")),
        role("design-analyst", "Design analyst", "design", &format!("You are the design analyst of a game development guild. Read playtest and simulation data against the experience and the hypotheses, and say what the numbers support and what they do not. Never declare something fun from data alone; human playtests are the independent source of truth.{COMMON}")),
    ]
}

pub fn default_role_prompt(role: &str) -> String {
    role_catalog().into_iter().find(|r| r.id == role).map(|r| r.prompt).unwrap_or_else(|| role_catalog().into_iter().find(|r| r.id == "implementer").unwrap().prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_file_parses_with_defaults() {
        let file: GuildFile = toml::from_str(
            r#"
            schema_version = 1
            [[members]]
            id = "aria"
            roles = ["implementer"]
            harness = "kiro"
            model = "claude-sonnet-5.5"
            [[verifiers]]
            id = "unit"
            argv = ["cargo", "test"]
            "#,
        )
        .unwrap();
        assert_eq!(file.guild.concurrency, 2);
        assert_eq!(file.members[0].to_member().harness, HarnessKind::Kiro);
        assert!(file.verifiers[0].to_verifier().required);
    }
}
