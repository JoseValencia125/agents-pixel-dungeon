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
pub fn default_role_prompt(role: &str) -> String {
    match role {
        "reviewer" => "You are a code reviewer in a guild of coding agents. Review the diff against the acceptance criteria. Report findings with file and line, state which criteria pass and which fail, and give a verdict. Do not edit files.".into(),
        "planner" => "You are a planner in a guild of coding agents. Break the goal into small tasks with explicit dependencies and verifiable acceptance criteria. Do not edit files.".into(),
        _ => "You are an implementer in a guild of coding agents. Work only inside the current directory, which is your own git worktree. Make the change the task asks for, keep it small, run the project's tests when they exist, and finish with a short summary of what you changed and how you verified it. Do not commit; the guild commits your work.".into(),
    }
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
