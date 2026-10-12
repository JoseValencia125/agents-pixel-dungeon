//! The slash commands a harness answers to, for the "/" menu of the chat
//! box: each harness's built-in ones, and for Claude Code also the custom
//! commands, skills and plugin skills found in the user's and the
//! project's `.claude` folders.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct SlashCommand {
    pub name: String,
    pub about: String,
}

fn built_in(harness: &str) -> &'static [(&'static str, &'static str)] {
    match harness {
        "claude" => &[
            ("help", "Show help and available commands"),
            ("clear", "Clear conversation history"),
            ("compact", "Summarize the conversation to free context"),
            ("context", "Show what is in the context window"),
            ("cost", "Show the cost and duration of the session"),
            ("usage", "Show plan usage limits"),
            ("model", "Choose the model"),
            ("effort", "Set the reasoning effort"),
            ("fast", "Toggle fast mode"),
            ("status", "Show version, model, account and connectivity"),
            ("config", "Open settings"),
            ("permissions", "Manage allowed and denied tools"),
            ("review", "Review a pull request or the current changes"),
            ("init", "Create a CLAUDE.md for the project"),
            ("memory", "Edit CLAUDE.md memory files"),
            ("doctor", "Check the installation"),
            ("bug", "Report a bug"),
            ("agents", "Manage agents and background sessions"),
            ("mcp", "Manage MCP servers"),
            ("hooks", "Manage hooks"),
            ("plugin", "Manage plugins"),
            ("resume", "Resume a previous conversation"),
            ("rename", "Rename the conversation"),
            ("export", "Export the conversation"),
            ("rewind", "Rewind the conversation or the code"),
            ("btw", "Ask a side question without interrupting"),
            ("vim", "Toggle vim editing mode"),
            ("terminal-setup", "Set up the terminal's keys"),
            ("release-notes", "Show release notes"),
            ("login", "Sign in"),
            ("logout", "Sign out"),
            ("exit", "Leave Claude Code"),
        ],
        "codex" => &[
            ("new", "Start a new chat"),
            ("init", "Create an AGENTS.md for the project"),
            ("compact", "Summarize the conversation to free context"),
            ("diff", "Show the changes so far"),
            ("mention", "Mention a file"),
            ("status", "Show session configuration and usage"),
            ("model", "Choose the model and reasoning effort"),
            ("permissions", "Choose what Codex may do without asking"),
            ("plan", "Switch to Plan mode"),
            ("resume", "Resume a saved chat"),
            ("review", "Review the current changes"),
            ("usage", "Show account usage"),
            ("mcp", "List MCP tools"),
            ("clear", "Clear the terminal and start a new chat"),
            ("logout", "Sign out"),
            ("exit", "Leave Codex"),
        ],
        "kiro" => &[
            ("help", "Show help and available commands"),
            ("clear", "Clear the conversation"),
            ("compact", "Summarize the conversation to free context"),
            ("context", "Manage the context files"),
            ("agent", "Manage agents"),
            ("tools", "Show and manage tools"),
            ("model", "Choose the model"),
            ("usage", "Show context window usage"),
            ("editor", "Write the prompt in an editor"),
            ("hooks", "Manage hooks"),
            ("mcp", "Show MCP servers"),
            ("prompts", "List MCP prompts"),
            ("knowledge", "Manage the knowledge base"),
            ("todos", "Manage to-do lists"),
            ("tangent", "Start or end a tangent"),
            ("save", "Save the conversation"),
            ("load", "Load a conversation"),
            ("subscribe", "Manage the subscription"),
            ("issue", "Report a bug"),
            ("changelog", "Show the changelog"),
            ("experiment", "Toggle experiments"),
            ("quit", "Leave Kiro"),
        ],
        "gemini" => &[
            ("help", "Show help and available commands"),
            ("clear", "Clear the screen"),
            ("compress", "Summarize the conversation to free context"),
            ("chat", "Save, resume or list conversations"),
            ("memory", "Manage GEMINI.md memory"),
            ("stats", "Show session statistics"),
            ("tools", "List tools"),
            ("mcp", "List MCP servers and tools"),
            ("extensions", "List extensions"),
            ("theme", "Choose a theme"),
            ("editor", "Choose an editor"),
            ("settings", "Open settings"),
            ("auth", "Choose how to sign in"),
            ("directory", "Manage workspace directories"),
            ("copy", "Copy the last answer"),
            ("restore", "Restore files to a checkpoint"),
            ("privacy", "Show the privacy notice"),
            ("about", "Show version information"),
            ("bug", "Report a bug"),
            ("vim", "Toggle vim editing mode"),
            ("quit", "Leave Gemini CLI"),
        ],
        _ => &[("help", "Show help"), ("clear", "Clear the conversation"), ("exit", "Leave the agent")],
    }
}

/// The `description:` (or `name:`) of a Markdown file's front matter.
fn front_matter(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" { return None; }
    for line in lines {
        if line.trim() == "---" { break; }
        if let Some(value) = line.strip_prefix(key).and_then(|r| r.strip_prefix(':')) {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() { return Some(value.to_string()); }
        }
    }
    None
}

fn short(text: String) -> String {
    let mut out: String = text.chars().take(90).collect();
    if out.len() < text.len() { out.push('…'); }
    out
}

/// Claude Code custom commands: every .md under a commands folder, named
/// after the file.
fn command_files(dir: &Path, prefix: &str, out: &mut Vec<SlashCommand>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            command_files(&path, prefix, out);
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
            let about = front_matter(&path, "description").map(short).unwrap_or_else(|| "Custom command".into());
            out.push(SlashCommand { name: format!("{prefix}{stem}"), about });
        }
    }
}

/// Claude Code skills: every folder with a SKILL.md under a skills folder.
fn skill_dirs(dir: &Path, prefix: &str, out: &mut Vec<SlashCommand>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    paths.sort();
    for path in paths {
        let skill = path.join("SKILL.md");
        if !skill.is_file() { continue; }
        let name = front_matter(&skill, "name").unwrap_or_else(|| path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        let about = front_matter(&skill, "description").map(short).unwrap_or_else(|| "Skill".into());
        out.push(SlashCommand { name: format!("{prefix}{name}"), about });
    }
}

/// Plugin skills and commands, as `/plugin:name`, from the newest cached
/// version of each plugin (~/.claude/plugins/cache/<market>/<plugin>/<version>).
fn plugin_commands(cache: &Path, out: &mut Vec<SlashCommand>) {
    let Ok(markets) = std::fs::read_dir(cache) else { return };
    for market in markets.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Ok(plugins) = std::fs::read_dir(&market) else { continue };
        for plugin in plugins.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let plugin_name = plugin.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let Ok(versions) = std::fs::read_dir(&plugin) else { continue };
            let mut versions: Vec<PathBuf> = versions.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            versions.sort();
            let Some(newest) = versions.last() else { continue };
            let prefix = format!("{plugin_name}:");
            skill_dirs(&newest.join("skills"), &prefix, out);
            command_files(&newest.join("commands"), &prefix, out);
            let root_skill = newest.join("SKILL.md");
            if root_skill.is_file() {
                let about = front_matter(&root_skill, "description").map(short).unwrap_or_else(|| "Skill".into());
                out.push(SlashCommand { name: plugin_name.clone(), about });
            }
        }
    }
}

/// The commands for a harness working in `cwd`, built-ins first.
pub fn slash_commands(harness: &str, cwd: &str, home: &Path) -> Vec<SlashCommand> {
    let mut out: Vec<SlashCommand> = built_in(harness).iter().map(|(n, a)| SlashCommand { name: n.to_string(), about: a.to_string() }).collect();
    if harness == "claude" {
        let project = Path::new(cwd);
        command_files(&project.join(".claude/commands"), "", &mut out);
        command_files(&home.join(".claude/commands"), "", &mut out);
        skill_dirs(&project.join(".claude/skills"), "", &mut out);
        skill_dirs(&home.join(".claude/skills"), "", &mut out);
        plugin_commands(&home.join(".claude/plugins/cache"), &mut out);
    }
    out.dedup_by(|a, b| a.name == b.name);
    out
}

/// The commands matching what is typed so far: a "/" and the start of a
/// name, no space yet.
pub fn matching<'a>(commands: &'a [SlashCommand], draft: &str) -> Vec<&'a SlashCommand> {
    let Some(typed) = draft.strip_prefix('/') else { return vec![] };
    if typed.contains(char::is_whitespace) { return vec![]; }
    let typed = typed.to_lowercase();
    commands.iter().filter(|c| c.name.to_lowercase().starts_with(&typed)).collect()
}
