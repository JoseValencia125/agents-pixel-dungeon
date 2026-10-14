//! Without Herdr there is no terminal to read, so this finds the agents
//! running in any terminal by their processes and infers what they are up
//! to from the files they write (their transcripts). It is a viewer: states
//! are approximate and nothing can be typed into an agent. The process-name
//! table follows Herdr's (Apache-2.0, see NOTICE.md).

use crate::herdr::{abbreviate_home, home_dir, last_action, Agent};
use crate::l10n::tr;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub tty: bool,
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

/// The agent a process is, from its executable (or the script a runtime
/// such as node runs), as Herdr names them. Helpers (MCP servers, native
/// hosts, daemons, one-shot prints) are not agents.
pub fn identify(args: &[String]) -> Option<&'static str> {
    let first = args.first()?;
    let basename = |p: &str| Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default().to_lowercase();
    let lookup = |name: &str| -> Option<&'static str> {
        let name = name.trim_end_matches(".js").trim_end_matches(".mjs").trim_end_matches(".cjs");
        Some(match name {
            "pi" => "pi",
            "claude" | "claude-code" => "claude",
            "codex" => "codex",
            "gemini" => "gemini",
            "agent" | "cursor" | "cursor-agent" => "cursor",
            "devin" | "devin-cli" => "devin",
            "agy" | "antigravity" | "antigravity-cli" => "antigravity",
            "cline" | ".cline" => "cline",
            "omp" => "omp",
            "mastracode" | "mastra-code" => "mastracode",
            "opencode" | "opencode2" | "open-code" => "opencode",
            "copilot" | "github-copilot" | "ghcs" => "copilot",
            "kimi" | "kimi-code" => "kimi",
            "kiro" | "kiro-cli" => "kiro",
            "droid" => "droid",
            "amp" | "amp-local" => "amp",
            "grok" | "grok-build" => "grok",
            "hermes" | "hermes-agent" => "hermes",
            "kilo" | "kilo-code" => "kilo",
            "qodercli" | "qoderclicn" | "qoder" | "qodercn" => "qodercli",
            "qwen" | "qwen-code" => "qwen",
            "letta" | "letta-code" => "letta",
            "maki" => "maki",
            "muse" | "muse-code" | "muse-cli" => "muse",
            other if other.starts_with("muse-bin-") && other["muse-bin-".len()..].starts_with(|c: char| c.is_ascii_digit()) => "muse",
            _ => return None,
        })
    };
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    // Helpers rather than the agent itself.
    let helper = ["--chrome-native-host", "--bg-pty-host", "daemon", "mcp", "serve", "--print", "-p", "--output-format", "--version", "update", "install"];
    if rest.iter().take(3).any(|a| helper.contains(a)) { return None; }
    let name = basename(first);
    // Claude Code's launcher execs a versioned binary under .../claude/versions/<v>.
    if first.contains("/claude/versions/") { return Some("claude"); }
    if let Some(agent) = lookup(&name) { return Some(agent); }
    if ["node", "bun", "deno", "python", "python3", "sh", "bash", "zsh"].contains(&name.as_str()) {
        for arg in rest.iter().take(2) {
            if let Some(agent) = lookup(&basename(arg)) { return Some(agent); }
            if arg.contains("/cursor-agent/versions/") && basename(arg) == "index.js" { return Some("cursor"); }
            if arg.contains("/claude/versions/") { return Some("claude"); }
        }
    }
    None
}

/// Every process with a controlling terminal, with its arguments and cwd.
pub fn processes() -> Vec<Process> {
    #[cfg(target_os = "linux")]
    { linux_processes() }
    #[cfg(not(target_os = "linux"))]
    { ps_processes() }
}

#[cfg(not(target_os = "linux"))]
fn ps_processes() -> Vec<Process> {
    let Ok(output) = std::process::Command::new("ps").args(["-axo", "pid=,ppid=,tty=,args="]).output() else { return vec![] };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found: Vec<Process> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid: u32 = parts.next()?.parse().ok()?;
            let ppid: u32 = parts.next()?.parse().ok()?;
            let tty = parts.next()? != "??";
            let args: Vec<String> = parts.map(str::to_string).collect();
            if args.is_empty() { return None; }
            Some(Process { pid, ppid, tty, args, cwd: None })
        })
        .collect();
    // Working directories of the agent candidates, in one lsof call.
    let agents: Vec<String> = found.iter().filter(|p| p.tty && identify(&p.args).is_some()).map(|p| p.pid.to_string()).collect();
    if !agents.is_empty() {
        if let Ok(output) = std::process::Command::new("lsof").args(["-a", "-d", "cwd", "-Fn", "-p", &agents.join(",")]).output() {
            let mut pid: Option<u32> = None;
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(p) = line.strip_prefix('p') { pid = p.parse().ok(); }
                else if let (Some(p), Some(path)) = (pid, line.strip_prefix('n')) {
                    if let Some(process) = found.iter_mut().find(|x| x.pid == p) { process.cwd = Some(path.to_string()); }
                }
            }
        }
    }
    found
}

#[cfg(target_os = "linux")]
fn linux_processes() -> Vec<Process> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return vec![] };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let cmdline = std::fs::read(entry.path().join("cmdline")).ok()?;
            let args: Vec<String> = cmdline.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).to_string()).collect();
            if args.is_empty() { return None; }
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let after = stat.rsplit(')').next()?;
            let fields: Vec<&str> = after.split_whitespace().collect();
            let ppid: u32 = fields.get(1)?.parse().ok()?;
            let tty = fields.get(4).and_then(|t| t.parse::<i64>().ok()).unwrap_or(0) != 0;
            let cwd = std::fs::read_link(entry.path().join("cwd")).ok().map(|p| p.to_string_lossy().to_string());
            Some(Process { pid, ppid, tty, args, cwd })
        })
        .collect()
}

/// `/Users/me/x` as Claude Code names its project folder: `-Users-me-x`.
pub fn claude_project_dir(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// What a transcript's last rows say the agent is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptView {
    /// "working", "blocked" (a tool call waits for permission), "idle".
    pub status: String,
    /// The last thing the user asked, as the room's activity line.
    pub prompt: Option<String>,
}

fn text_of(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).filter_map(|b| b.get("text").and_then(Value::as_str)).collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}

/// Read a Claude Code transcript's tail: the last turn's shape decides the
/// state. A pending tool call with the agent otherwise idle (no child shell
/// running, nothing written for a while) is a permission prompt.
pub fn claude_view(tail: &str, age: Duration, busy_child: bool) -> TranscriptView {
    let mut prompt = None;
    let mut last: Option<(String, bool)> = None; // (type, has pending tool_use)
    for line in tail.lines().rev() {
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let kind = row.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "user" && kind != "assistant" { continue; }
        let content = row.get("message").and_then(|m| m.get("content"));
        if last.is_none() {
            let tool = content.and_then(Value::as_array).map(|c| c.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))).unwrap_or(false);
            let result = content.and_then(Value::as_array).map(|c| c.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))).unwrap_or(false);
            last = Some(((if kind == "user" && result { "result" } else { kind }).to_string(), tool));
        }
        if kind == "user" && prompt.is_none() {
            if let Some(text) = content.and_then(text_of) {
                let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                if !line.is_empty() && !line.starts_with('<') { prompt = Some(line.chars().take(80).collect()); }
            }
        }
        if last.is_some() && prompt.is_some() { break; }
    }
    let status = match last.as_ref().map(|(k, t)| (k.as_str(), *t)) {
        Some(("assistant", true)) => if busy_child || age < Duration::from_secs(4) { "working" } else { "blocked" },
        Some(("assistant", false)) => if age < Duration::from_secs(3) { "working" } else { "idle" },
        Some(("result", _)) | Some(("user", _)) => if age < Duration::from_secs(180) { "working" } else { "idle" },
        _ => "idle",
    };
    TranscriptView { status: status.into(), prompt }
}

/// The same for a Codex rollout: a function call without its output while
/// the agent sits idle is an approval prompt.
pub fn codex_view(tail: &str, age: Duration, busy_child: bool) -> TranscriptView {
    let mut prompt = None;
    let mut last: Option<&'static str> = None;
    for line in tail.lines().rev() {
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        if row.get("type").and_then(Value::as_str) != Some("response_item") { continue; }
        let Some(item) = row.get("payload") else { continue };
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
        let role = item.get("role").and_then(Value::as_str).unwrap_or("");
        if last.is_none() {
            last = Some(match kind {
                "function_call" | "custom_tool_call" | "local_shell_call" => "call",
                "function_call_output" | "custom_tool_call_output" => "output",
                "message" if role == "assistant" => "reply",
                "message" => "prompt",
                _ => continue,
            });
        }
        if kind == "message" && role == "user" && prompt.is_none() {
            if let Some(text) = item.get("content").and_then(|c| c.as_array()).and_then(|blocks| blocks.iter().find_map(|b| b.get("text").and_then(Value::as_str))) {
                let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                if !line.is_empty() && !line.starts_with('<') { prompt = Some(line.chars().take(80).collect()); }
            }
        }
        if last.is_some() && prompt.is_some() { break; }
    }
    let status = match last {
        Some("call") => if busy_child || age < Duration::from_secs(4) { "working" } else { "blocked" },
        Some("reply") => if age < Duration::from_secs(3) { "working" } else { "idle" },
        Some("output") | Some("prompt") => if age < Duration::from_secs(180) { "working" } else { "idle" },
        _ => "idle",
    };
    TranscriptView { status: status.into(), prompt }
}

/// A child of the agent that is a running tool (a shell, a build…) rather
/// than one of its helper servers.
fn busy_child(all: &[Process], pid: u32) -> bool {
    all.iter().any(|p| {
        p.ppid == pid && {
            let name = Path::new(&p.args[0]).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            let helper = p.args.iter().any(|a| a.contains("mcp") || a.ends_with(".mjs") || a == "serve" || a.contains("--chrome-native-host"));
            !helper && !["node", "bun"].contains(&name.as_str())
        }
    })
}

fn newest_jsonl(folder: &Path, taken: &HashSet<PathBuf>) -> Option<(PathBuf, SystemTime)> {
    let mut files: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|f| {
            let path = f.path();
            (path.extension().map(|e| e == "jsonl").unwrap_or(false) && !taken.contains(&path)).then(|| Some((path, f.metadata().ok()?.modified().ok()?))).flatten()
        })
        .collect();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.into_iter().next()
}

/// The agents running in terminals right now, as rooms. Claude Code agents
/// get their session from `--resume`/`--session-id` or the newest transcript
/// of their folder; Codex agents the newest rollout.
pub fn agents() -> Vec<Agent> {
    let all = processes();
    let mut taken: HashSet<PathBuf> = HashSet::new();
    let mut out = vec![];
    let now = SystemTime::now();
    let mut candidates: Vec<&Process> = all.iter().filter(|p| p.tty && identify(&p.args).is_some()).collect();
    candidates.sort_by_key(|p| p.pid);
    // One agent per terminal: a wrapper (node → claude) shows up twice.
    let mut ttys_seen: HashSet<(String, &str)> = HashSet::new();
    for process in candidates {
        let kind = identify(&process.args).unwrap();
        let cwd = process.cwd.clone().unwrap_or_default();
        if !ttys_seen.insert((format!("{}:{}", cwd, process.ppid), kind)) { continue; }
        let folder = Path::new(&cwd).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| kind.to_string());
        let mut agent = Agent::new(&format!("pid:{}", process.pid), kind, "unknown", &folder, &tr("Sin título de actividad"), &abbreviate_home(&cwd));
        let busy = busy_child(&all, process.pid);
        match kind {
            "claude" => {
                let flagged = process.args.windows(2).find(|w| w[0] == "--resume" || w[0] == "--session-id").map(|w| w[1].clone());
                let project = home_dir().join(".claude/projects").join(claude_project_dir(&cwd));
                let file = match flagged {
                    Some(id) if project.join(format!("{id}.jsonl")).exists() => Some(project.join(format!("{id}.jsonl"))),
                    _ => newest_jsonl(&project, &taken).map(|f| f.0),
                };
                if let Some(file) = file {
                    taken.insert(file.clone());
                    let modified = std::fs::metadata(&file).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    let age = now.duration_since(modified).unwrap_or_default();
                    let tail = crate::herdr::file_tail(&file.to_string_lossy(), 131_072).unwrap_or_default();
                    let view = claude_view(&tail, age, busy);
                    agent.status = view.status;
                    if let Some(prompt) = view.prompt { agent.activity = prompt; }
                    agent.session = file.file_stem().map(|s| s.to_string_lossy().to_string());
                    agent.question_transcript = Some(file.to_string_lossy().to_string());
                    if agent.status == "working" { agent.action = last_action(&tail); }
                }
            }
            "codex" => {
                if let Some(file) = newest_codex_rollout(&taken) {
                    taken.insert(file.clone());
                    let modified = std::fs::metadata(&file).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    let age = now.duration_since(modified).unwrap_or_default();
                    let tail = crate::herdr::file_tail(&file.to_string_lossy(), 131_072).unwrap_or_default();
                    let view = codex_view(&tail, age, busy);
                    agent.status = view.status;
                    if let Some(prompt) = view.prompt { agent.activity = prompt; }
                    agent.session = Some(file.to_string_lossy().to_string());
                }
            }
            _ => { agent.status = if busy { "working".into() } else { "unknown".into() }; }
        }
        out.push(agent);
    }
    out
}

/// The newest Codex rollout written today or yesterday, not yet assigned.
fn newest_codex_rollout(taken: &HashSet<PathBuf>) -> Option<PathBuf> {
    let root = home_dir().join(".codex/sessions");
    let today = chrono::Local::now();
    let mut best: Option<(PathBuf, SystemTime)> = None;
    for back in 0..2 {
        let day = today - chrono::Duration::days(back);
        let folder = root.join(day.format("%Y/%m/%d").to_string());
        if let Some(found) = newest_jsonl(&folder, taken) {
            if best.as_ref().map(|b| found.1 > b.1).unwrap_or(true) { best = Some(found); }
        }
    }
    best.map(|b| b.0)
}

/// The terminal command that opens Herdr, per platform.
pub fn open_herdr() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let script = "tell application \"Terminal\"\n activate\n do script \"herdr\"\nend tell";
        return std::process::Command::new("osascript").args(["-e", script]).spawn().map(|_| ()).map_err(|e| e.to_string());
    }
    #[cfg(not(target_os = "macos"))]
    {
        for (terminal, args) in [("x-terminal-emulator", vec!["-e", "herdr"]), ("gnome-terminal", vec!["--", "herdr"]), ("konsole", vec!["-e", "herdr"]), ("xterm", vec!["-e", "herdr"])] {
            if std::process::Command::new(terminal).args(&args).spawn().is_ok() { return Ok(()); }
        }
        Err("no terminal found".into())
    }
}

/// Herdr's official installer, run to completion; Ok carries its last lines.
pub fn install_herdr() -> Result<String, String> {
    let output = std::process::Command::new("sh").args(["-c", "curl -fsSL https://herdr.dev/install.sh | sh"]).output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let last: Vec<&str> = text.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect();
    if output.status.success() { Ok(last.join("\n")) } else { Err(last.join("\n")) }
}

#[allow(dead_code)]
pub fn by_pid(all: &[Process]) -> HashMap<u32, &Process> {
    all.iter().map(|p| (p.pid, p)).collect()
}
