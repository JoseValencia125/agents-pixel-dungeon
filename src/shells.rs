//! Shells an agent left running in the background (Claude Code's Bash with
//! `run_in_background`). Alive or not comes from the processes: each is a
//! `zsh -c source …/shell-snapshots/… && eval '<command>'` child of the
//! agent's `claude`, itself a child of the pane's shell. What it is (its
//! description, its output file) and how it ended come from the agent's
//! transcript. Only local files and `ps` are read.

use crate::herdr::{file_tail, Agent};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::SystemTime;

/// A shell still running in the background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    pub pid: u32,
    /// The command as typed.
    pub command: String,
    /// What the agent said it does, else the command.
    pub label: String,
    /// When it started (unix seconds).
    pub started: i64,
    /// Claude Code's task ID, to find how it ended.
    pub task: Option<String>,
    /// The last line it wrote (its output file, or a log it writes to).
    pub last_line: Option<String>,
    /// Where it writes, to show more of it.
    pub output: Option<String>,
}

impl Shell {
    /// A call to the guild's command line (an orchestrator waiting on its
    /// quest): the guild's own work, which never rings.
    pub fn is_guild(&self) -> bool { self.command.contains("agents-pixel-dungeon-guild") }
}

/// How a background shell ended, from Claude Code's notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellResult {
    pub task: String,
    pub ok: bool,
    pub summary: String,
}

/// One process from `ps`.
#[derive(Clone, Debug)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub started: i64,
    pub command: String,
}

/// Every process, from one `ps` call.
pub fn processes() -> Vec<Process> {
    if cfg!(windows) { return vec![]; }
    let Ok(out) = Command::new("ps").args(["-Ao", "pid=,ppid=,lstart=,command="]).env("LC_ALL", "C").output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(parse_ps_line).collect()
}

/// "  123   45 Sun Oct 11 10:19:17 2026 /bin/zsh -c …"
pub fn parse_ps_line(line: &str) -> Option<Process> {
    let mut parts = line.split_whitespace();
    let pid = parts.next()?.parse().ok()?;
    let ppid = parts.next()?.parse().ok()?;
    let date: Vec<&str> = parts.by_ref().take(5).collect();
    if date.len() < 5 { return None; }
    let when = chrono::NaiveDateTime::parse_from_str(&date.join(" "), "%a %b %e %H:%M:%S %Y").ok()?;
    let started = when.and_local_timezone(chrono::Local).single()?.timestamp();
    let command = parts.collect::<Vec<_>>().join(" ");
    Some(Process { pid, ppid, started, command })
}

/// The command a Claude Code shell runs, if `command` is one.
pub fn shell_command(command: &str) -> Option<String> {
    if !command.contains("/shell-snapshots/snapshot-") { return None; }
    let start = command.find(" eval '")? + " eval '".len();
    let rest = &command[start..];
    let end = rest.rfind("' < /dev/null").or_else(|| rest.rfind('\''))?;
    Some(rest[..end].replace("'\\''", "'"))
}

/// What a transcript says about its background shells: each launch by its
/// command (task ID, description, output file), and how the finished ones
/// ended.
#[derive(Clone, Debug, Default)]
pub struct Launches {
    pub by_command: HashMap<String, (Option<String>, Option<String>, Option<String>)>,
    pub results: Vec<ShellResult>,
}

static LAUNCHES: Mutex<Option<HashMap<String, (SystemTime, Launches)>>> = Mutex::new(None);

/// A transcript's launches, read again only when it changes. Only lines
/// about background shells are parsed.
pub fn launches(transcript: &str) -> Launches {
    let Some(modified) = std::fs::metadata(transcript).and_then(|m| m.modified()).ok() else { return Launches::default() };
    let mut guard = LAUNCHES.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((when, known)) = cache.get(transcript) { if *when == modified { return known.clone(); } }
    let read = file_tail(transcript, 4 << 20).map(|t| parse_launches(&t)).unwrap_or_default();
    cache.insert(transcript.to_string(), (modified, read.clone()));
    read
}

pub fn parse_launches(tail: &str) -> Launches {
    let mut launches = Launches::default();
    // tool_use id → (command, description), then its result → task and file.
    let mut asked: HashMap<String, (String, Option<String>)> = HashMap::new();
    for line in tail.lines() {
        if !(line.contains("run_in_background") || line.contains("backgroundTaskId") || line.contains("<task-notification>")) { continue; }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let content = row.get("message").and_then(|m| m.get("content"));
        for item in content.and_then(Value::as_array).into_iter().flatten() {
            match item.get("type").and_then(Value::as_str) {
                Some("tool_use") if item.get("input").and_then(|i| i.get("run_in_background")).and_then(Value::as_bool) == Some(true) => {
                    let input = &item["input"];
                    let (Some(id), Some(command)) = (item.get("id").and_then(Value::as_str), input.get("command").and_then(Value::as_str)) else { continue };
                    let description = input.get("description").and_then(Value::as_str).map(str::to_string);
                    asked.insert(id.to_string(), (command.to_string(), description));
                }
                Some("tool_result") => {
                    let Some(id) = item.get("tool_use_id").and_then(Value::as_str) else { continue };
                    let Some((command, description)) = asked.get(id) else { continue };
                    let text = text_of(item.get("content"));
                    let task = between(&text, "with ID: ", ".").map(str::to_string)
                        .or_else(|| row.get("toolUseResult").and_then(|r| r.get("backgroundTaskId")).and_then(Value::as_str).map(str::to_string));
                    let output = between(&text, "Output is being written to: ", ". ").map(|s| s.trim_end_matches('.').to_string());
                    launches.by_command.insert(command.clone(), (task, description.clone(), output));
                }
                _ => {}
            }
        }
        // A finished shell's notification: queued for the agent, or the turn
        // it started (not a quote of one in some tool's output).
        let notice = row.get("type").and_then(Value::as_str) == Some("queue-operation")
            || row.pointer("/origin/kind").and_then(Value::as_str) == Some("task-notification");
        if !notice { continue; }
        let note = row.get("content").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| text_of(content));
        let (Some(task), Some(status)) = (between(&note, "<task-id>", "</task-id>"), between(&note, "<status>", "</status>")) else { continue };
        if launches.results.iter().any(|r| r.task == task) { continue; }
        let summary = between(&note, "<summary>", "</summary>").unwrap_or(status).to_string();
        launches.results.push(ShellResult { task: task.to_string(), ok: status == "completed", summary });
    }
    launches
}

fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items.iter().filter_map(|i| i.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let rest = &text[from..];
    Some(&rest[..rest.find(end).unwrap_or(rest.len())])
}

/// Pids already seen as background shells, so one launched before the
/// transcript in view (a resumed session) stays shown while it lives.
static KNOWN: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

/// The background shells under the agent's `claude` (the processes
/// `parent` picks): those the transcript launched in the background, and,
/// while the agent is not working, any other (a session resumed since).
pub fn shells_of(table: &[Process], parent: impl Fn(&Process) -> bool, working: bool, launches: &Launches, cwd: &str) -> Vec<Shell> {
    let claudes: HashSet<u32> = table.iter().filter(|p| parent(p)).map(|p| p.pid).collect();
    let mut guard = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    let known = guard.get_or_insert_with(HashSet::new);
    let mut shells = vec![];
    for process in table.iter().filter(|p| claudes.contains(&p.ppid)) {
        let Some(command) = shell_command(&process.command) else { continue };
        let launch = launches.by_command.get(&command).or_else(|| launches.by_command.get(command.trim()));
        // A shell the agent waits on while it works is not in the background.
        if launch.is_none() && working && !known.contains(&process.pid) { continue; }
        known.insert(process.pid);
        let (task, description, output) = launch.cloned().unwrap_or_default();
        let log = newest_output(output.as_deref(), &command, cwd);
        shells.push(Shell {
            pid: process.pid,
            label: description.unwrap_or_else(|| short_command(&command)),
            command,
            started: process.started,
            task,
            last_line: log.as_deref().and_then(last_line),
            output: log,
        });
    }
    shells.sort_by_key(|s| s.started);
    shells
}

/// The command without its leading `cd …&&`, first line only, shortened.
pub fn short_command(command: &str) -> String {
    let line = command.lines().next().unwrap_or("").trim();
    let line = match line.strip_prefix("cd ").and_then(|r| r.find("&&").map(|i| r[i + 2..].trim())) { Some(rest) => rest, None => line };
    line.chars().take(80).collect()
}

/// Of the shell's output file and the files its command writes to with
/// `>`, `>>` or `tee`, the one written last.
pub fn newest_output(output: Option<&str>, command: &str, cwd: &str) -> Option<String> {
    let base = command.trim().strip_prefix("cd ").and_then(|r| r.split("&&").next()).map(|d| d.trim().trim_matches('"').to_string()).unwrap_or_else(|| cwd.to_string());
    let mut candidates: Vec<String> = output.map(|o| vec![o.to_string()]).unwrap_or_default();
    let words: Vec<&str> = command.split_whitespace().collect();
    for pair in words.windows(2) {
        if matches!(pair[0], ">" | ">>" | "tee" | "1>") && !pair[1].starts_with('&') && pair[1] != "/dev/null" {
            let target = pair[1].trim_matches(|c| c == '"' || c == '\'' || c == ';');
            candidates.push(if crate::herdr::is_absolute_path(target) { target.to_string() } else { Path::new(&base).join(target).to_string_lossy().to_string() });
        }
    }
    // The newest with text: a log just opened by a buffered program is empty.
    let mut found: Vec<(SystemTime, String)> = candidates.into_iter()
        .filter_map(|c| std::fs::metadata(&c).ok().filter(|m| m.len() > 0).and_then(|m| m.modified().ok()).map(|m| (m, c)))
        .collect();
    found.sort();
    found.into_iter().rev().map(|(_, c)| c).find(|c| last_line(c).is_some())
}

/// The last line with text in a file: after its last carriage return (a
/// progress bar), without colours, shortened.
pub fn last_line(path: &str) -> Option<String> {
    let tail = file_tail(path, 4096)?;
    let line = tail.lines().rev().map(|l| l.rsplit('\r').next().unwrap_or(l)).map(strip_ansi).find(|l| !l.trim().is_empty())?;
    Some(line.trim().chars().take(120).collect())
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') { chars.next(); while let Some(n) = chars.next() { if n.is_ascii_alphabetic() { break; } } }
            continue;
        }
        out.push(c);
    }
    out
}

/// Each pane's shell pid, asked of Herdr once per pane.
static PANE_SHELLS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

pub fn pane_shell(pane: &str, session: &str) -> Option<u32> {
    {
        let guard = PANE_SHELLS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pid) = guard.as_ref().and_then(|m| m.get(pane)) { return Some(*pid); }
    }
    let data = crate::herdr::run_herdr(&["pane", "process-info", "--pane", pane], session, std::time::Duration::from_secs(3)).ok()?;
    let row: Value = serde_json::from_slice(&data).ok()?;
    let pid = row.pointer("/result/process_info/shell_pid").and_then(Value::as_u64)? as u32;
    PANE_SHELLS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(pane.to_string(), pid);
    Some(pid)
}

/// Whether an agent Herdr cannot read the state of is still there: its
/// pane's shell runs something. Herdr keeps naming a pane after the last
/// agent it saw there (its detection falls back to it), so a Kiro that ended
/// leaves a "kiro" whose pane is only a shell again — typing /exit into it
/// reaches zsh. Known states, guild rooms and agents found by their
/// process are never in doubt.
pub fn still_running(agent: &Agent, table: &[Process], shell: Option<u32>) -> bool {
    if agent.status != "unknown" || agent.id.starts_with("guild:") || agent.id.starts_with("pid:") { return true; }
    let Some(shell) = shell else { return true };
    table.is_empty() || table.iter().any(|p| p.ppid == shell)
}

/// Drop the agents whose program has exited, leaving their pane a shell.
pub fn drop_exited(agents: &mut Vec<Agent>, session: &str) {
    if !agents.iter().any(|a| a.status == "unknown" && !a.id.starts_with("guild:") && !a.id.starts_with("pid:")) { return; }
    let table = processes();
    agents.retain(|a| still_running(a, &table, if a.status == "unknown" { pane_shell(&a.id, session) } else { None }));
}

/// Add each Claude agent's background shells and how its last ones ended.
pub fn add_shells(agents: &mut [Agent], session: &str) {
    if !agents.iter().any(|a| crate::herdr::harness_for(&a.name) == "claude") { return; }
    let table = processes();
    for agent in agents.iter_mut().filter(|a| crate::herdr::harness_for(&a.name) == "claude") {
        let launches = agent.session.as_deref().and_then(crate::herdr::claude_transcript).map(|t| launches(&t)).unwrap_or_default();
        let cwd = crate::herdr::expand_home(&agent.cwd);
        let working = agent.status == "working";
        agent.shells = if let Some(pid) = agent.id.strip_prefix("pid:").and_then(|p| p.parse::<u32>().ok()) {
            shells_of(&table, |p| p.pid == pid, working, &launches, &cwd)
        } else if let Some(shell) = pane_shell(&agent.id, session) {
            shells_of(&table, |p| p.ppid == shell, working, &launches, &cwd)
        } else {
            vec![]
        };
        agent.shell_results = launches.results;
    }
}

/// Shells stopped from the chat: they end without an alert.
static STOPPED: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

/// Stop a background shell: what it runs first, then the shell itself.
pub fn stop(pid: u32) {
    STOPPED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(pid);
    if cfg!(windows) {
        std::thread::spawn(move || guild::platform::terminate_tree(pid as i32, true));
        return;
    }
    let pid = pid.to_string();
    std::thread::spawn(move || {
        let _ = Command::new("pkill").args(["-TERM", "-P", &pid]).status();
        let _ = Command::new("kill").args(["-TERM", &pid]).status();
    });
}

pub fn was_stopped(pid: u32) -> bool {
    STOPPED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&pid))
}

/// The last `count` lines of a shell's output, to read in the chat.
pub fn output_tail(path: &str, count: usize) -> Vec<String> {
    let tail = file_tail(path, 16_384).unwrap_or_default();
    let lines: Vec<String> = tail.lines().map(|l| strip_ansi(l.rsplit('\r').next().unwrap_or(l))).collect();
    lines[lines.len().saturating_sub(count)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_left_with_only_its_shell_has_no_agent_any_more() {
        let proc = |pid: u32, ppid: u32| Process { pid, ppid, started: 0, command: String::new() };
        let table = vec![proc(100, 1), proc(101, 100), proc(200, 1)];
        let kiro = |status: &str| Agent::new("w4N:p1", "kiro", status, "scheduler", "", "~/scheduler");
        // Herdr still says "kiro", state unknown; the pane's shell (200) runs nothing: it ended.
        assert!(!still_running(&kiro("unknown"), &table, Some(200)));
        // Its shell runs something: still there.
        assert!(still_running(&kiro("unknown"), &table, Some(100)));
        // A known state, a shell Herdr would not give, a table that could not be read: never dropped.
        assert!(still_running(&kiro("idle"), &table, Some(200)));
        assert!(still_running(&kiro("unknown"), &table, None));
        assert!(still_running(&kiro("unknown"), &[], Some(200)));
        assert!(still_running(&Agent::new("guild:hall", "claude", "unknown", "g", "", "~"), &table, Some(200)));
    }
}

