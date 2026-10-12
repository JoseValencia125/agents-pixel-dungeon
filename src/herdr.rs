//! Herdr's view of the agents (snapshots, the CLI, sessions) and what the
//! app adds from local files: git branches, Claude Code background jobs and
//! subagents, and each harness's last tool from its own session log.

use crate::l10n::{tr, trf};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub status: String,
    pub project: String,
    pub activity: String,
    pub cwd: String,
    /// agent_session id (Claude Code session UUID, or a transcript path).
    pub session: Option<String>,
    /// Subagents with recent transcript activity.
    pub subagents: usize,
    /// What each of them is doing (read, forge, brew…), from its own transcript.
    pub subagent_actions: Vec<String>,
    /// Git branch of cwd, if it is a repository.
    pub branch: Option<String>,
    /// What a working agent is doing: read, forge, brew, summon, plan, type.
    pub action: Option<String>,
    /// What a blocked background job is asking (its first line).
    pub question: Option<String>,
    /// That job's transcript, to show the whole question.
    pub question_transcript: Option<String>,
    /// When a "limited" agent's usage limit resets (unix seconds), if known.
    pub limit_resets: Option<i64>,
    /// Shells it left running in the background, and how its last ones ended.
    pub shells: Vec<crate::shells::Shell>,
    pub shell_results: Vec<crate::shells::ShellResult>,
}

impl Agent {
    pub fn new(id: &str, name: &str, status: &str, project: &str, activity: &str, cwd: &str) -> Agent {
        Agent {
            id: id.into(),
            name: name.into(),
            status: status.into(),
            project: project.into(),
            activity: activity.into(),
            cwd: cwd.into(),
            session: None,
            subagents: 0,
            subagent_actions: vec![],
            branch: None,
            action: None,
            question: None,
            question_transcript: None,
            limit_resets: None,
            shells: vec![],
            shell_results: vec![],
        }
    }

    pub fn folder(&self) -> String {
        Path::new(&self.cwd).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
    }

    pub fn label(&self) -> String {
        tr(match self.status.as_str() {
            "working" => "Trabajando",
            "blocked" => "Necesita atención",
            "limited" => "Límite de sesión",
            "idle" => "En espera",
            "done" => "Listo",
            _ => "Sin estado",
        })
    }

    pub fn rank_of(status: &str) -> usize {
        match status {
            "blocked" => 0,
            "limited" => 1,
            "working" => 2,
            "idle" => 3,
            "done" => 4,
            _ => 5,
        }
    }

    /// Seconds until a limited agent's usage limit resets (never negative).
    pub fn limit_left(&self, now: i64) -> Option<i64> {
        self.limit_resets.map(|r| (r - now).max(0))
    }

    pub fn rank(&self) -> usize {
        Agent::rank_of(&self.status)
    }

    /// A pane takes the most urgent state among itself and its background
    /// jobs: a job needing input outranks one working, which outranks idle.
    pub fn with_jobs(mut self, jobs: &[BackgroundJob]) -> Agent {
        let states: Vec<&str> = jobs.iter().map(|j| j.state.as_str()).collect();
        if let Some(urgent) = ["blocked", "working"].iter().find(|s| states.contains(s)) {
            if Agent::rank_of(urgent) < self.rank() {
                self.status = urgent.to_string();
            }
        } else if states.contains(&"limited") && self.status != "working" {
            // Only jobs stuck on the usage limit: the pane's own "blocked" is
            // the job list waiting on them, not a question for you.
            self.status = "limited".into();
        }
        if self.status == "blocked" || self.status == "limited" {
            let state = self.status.clone();
            if let Some(asking) = jobs.iter().filter(|j| j.state == state).max_by_key(|j| j.updated) {
                self.question = asking.needs.clone();
                self.question_transcript = asking.transcript.clone();
            }
        }
        if self.status == "limited" {
            self.limit_resets = jobs.iter().filter(|j| j.state == "limited").filter_map(|j| j.resets).min();
        }
        self
    }

    /// The agent's own session hit the usage limit: trapped until it resets,
    /// unless it is working again.
    pub fn with_limit(mut self, limit: Option<Option<i64>>) -> Agent {
        let Some(resets) = limit else { return self };
        if self.status == "working" { return self; }
        self.status = "limited".into();
        self.limit_resets = match (self.limit_resets, resets) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) };
        self
    }

    /// An agent whose subagents are still running is busy, even if its own
    /// pane reports idle.
    pub fn with_subagents(mut self, count: usize) -> Agent {
        self.subagents = count;
        if count > 0 && self.status == "idle" {
            self.status = "working".into();
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorError {
    Message(String),
    /// Herdr answered with an error (its JSON on stderr): a code such as
    /// agent_not_ready or agent_blocked, and its own message.
    Herdr { code: String, message: String },
}

impl MonitorError {
    pub fn text(&self) -> String {
        match self {
            MonitorError::Message(m) => m.clone(),
            MonitorError::Herdr { message, .. } => message.clone(),
        }
    }
    pub fn code(&self) -> Option<&str> {
        match self {
            MonitorError::Herdr { code, .. } => Some(code),
            _ => None,
        }
    }
}

impl std::fmt::Display for MonitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

/// The error Herdr printed on stderr (`{"error":{"code":…,"message":…}}`), if any.
pub fn herdr_error(data: &[u8]) -> Option<MonitorError> {
    let root: Value = serde_json::from_slice(data).ok()?;
    let error = root.get("error")?;
    let code = error.get("code")?.as_str()?.to_string();
    let message = error.get("message").and_then(Value::as_str).unwrap_or(&code).to_string();
    Some(MonitorError::Herdr { code, message })
}

fn str_of<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

pub fn decode_snapshot(data: &[u8]) -> Result<Vec<Agent>, MonitorError> {
    decode_snapshot_parts(data).map(|p| p.0)
}

/// A snapshot's agents (sorted, attention first) and its workspace labels.
pub fn decode_snapshot_parts(data: &[u8]) -> Result<(Vec<Agent>, HashMap<String, String>), MonitorError> {
    let invalid = || MonitorError::Message(tr("Herdr no entregó un snapshot válido."));
    let root: Value = serde_json::from_slice(data).map_err(|_| invalid())?;
    let snapshot = root.get("result").and_then(|r| r.get("snapshot")).ok_or_else(invalid)?;
    let agents = snapshot.get("agents").and_then(Value::as_array).ok_or_else(invalid)?;
    let mut names = HashMap::new();
    for space in snapshot.get("workspaces").and_then(Value::as_array).into_iter().flatten() {
        if let (Some(id), Some(label)) = (str_of(space, "workspace_id"), str_of(space, "label")) {
            names.insert(id.to_string(), label.to_string());
        }
    }
    let mut unique: HashMap<String, Agent> = HashMap::new();
    for row in agents {
        if let Some(agent) = agent_from_row(row, &names) {
            unique.insert(agent.id.clone(), agent);
        }
    }
    Ok((sort_agents(unique.into_values().collect()), names))
}

/// An agent from a Herdr pane row (a snapshot's agent or an event's pane),
/// or None when no agent runs in the pane.
pub fn agent_from_row(row: &Value, names: &HashMap<String, String>) -> Option<Agent> {
    let id = str_of(row, "pane_id")?;
    let name = str_of(row, "agent")?;
    let cwd = str_of(row, "foreground_cwd").or_else(|| str_of(row, "cwd")).unwrap_or("");
    let project = str_of(row, "workspace_id")
        .and_then(|w| names.get(w).cloned())
        .unwrap_or_else(|| Path::new(cwd).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
    let activity = str_of(row, "terminal_title_stripped")
        .or_else(|| str_of(row, "terminal_title"))
        .map(str::to_string)
        .unwrap_or_else(|| tr("Sin título de actividad"));
    let mut agent = Agent::new(id, name, &agent_status(row.get("agent_status")), &project, &activity, &abbreviate_home(cwd));
    agent.session = row.get("agent_session").and_then(|s| s.get("value")).and_then(Value::as_str).map(str::to_string);
    Some(agent)
}

pub fn agent_status(raw: Option<&Value>) -> String {
    let raw = raw.and_then(Value::as_str).unwrap_or("unknown");
    if ["working", "blocked", "idle", "done"].contains(&raw) { raw.to_string() } else { "unknown".to_string() }
}

/// Attention first, then working, waiting, done; ties by pane id.
pub fn sort_agents(mut agents: Vec<Agent>) -> Vec<Agent> {
    agents.sort_by(|a, b| a.rank().cmp(&b.rank()).then_with(|| natural_cmp(&a.id, &b.id)));
    agents
}

/// Compare like Finder: digit runs by value, so w2:p1 sorts before w10:p1.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let mut x = a.chars().peekable();
    let mut y = b.chars().peekable();
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n = 0u64;
                while let Some(ch) = x.peek().copied().filter(char::is_ascii_digit) { n = n * 10 + ch.to_digit(10).unwrap() as u64; x.next(); }
                let mut m = 0u64;
                while let Some(ch) = y.peek().copied().filter(char::is_ascii_digit) { m = m * 10 + ch.to_digit(10).unwrap() as u64; y.next(); }
                if n != m { return n.cmp(&m); }
            }
            (Some(c), Some(d)) => {
                let (lc, ld) = (c.to_lowercase().next().unwrap_or(c), d.to_lowercase().next().unwrap_or(d));
                if lc != ld { return lc.cmp(&ld); }
                x.next();
                y.next();
            }
        }
    }
}

pub fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// `/Users/me/x` → `~/x`.
pub fn abbreviate_home(path: &str) -> String {
    let home = home_dir().to_string_lossy().to_string();
    if path == home {
        "~".to_string()
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        path.to_string()
    }
}

/// `~/x` → `/Users/me/x`.
pub fn expand_home(path: &str) -> String {
    if path == "~" {
        home_dir().to_string_lossy().to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home_dir().join(rest).to_string_lossy().to_string()
    } else {
        path.to_string()
    }
}

/// Locate the herdr binary: HERDR_BIN, the usual install spots, then PATH.
pub fn herdr_binary() -> Result<PathBuf, MonitorError> {
    if std::env::var("HPD_NO_HERDR").map(|v| v == "1").unwrap_or(false) {
        return Err(MonitorError::Message(tr("No se encontró Herdr. Instálalo o define HERDR_BIN.")));
    }
    let home = home_dir();
    let mut candidates: Vec<PathBuf> = vec![];
    if let Ok(bin) = std::env::var("HERDR_BIN") { candidates.push(PathBuf::from(bin)); }
    candidates.push(home.join(".local/bin/herdr"));
    candidates.push(PathBuf::from("/opt/homebrew/bin/herdr"));
    candidates.push(PathBuf::from("/usr/local/bin/herdr"));
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') { candidates.push(Path::new(dir).join("herdr")); }
    }
    candidates
        .into_iter()
        .find(|p| is_executable(p))
        .ok_or_else(|| MonitorError::Message(tr("No se encontró Herdr. Instálalo o define HERDR_BIN.")))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Run `herdr [--session s] <arguments>` and return its stdout. The process
/// is killed after `timeout` so a stuck server never hangs the UI.
pub fn run_herdr(arguments: &[&str], session: &str, timeout: Duration) -> Result<Vec<u8>, MonitorError> {
    let binary = herdr_binary()?;
    let mut command = Command::new(binary);
    if session != "default" { command.args(["--session", session]); }
    command.args(arguments).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let unreachable = || MonitorError::Message(tr("Sin conexión con Herdr. Abre tu sesión; reintentamos automáticamente."));
    let mut child = command.spawn().map_err(|_| unreachable())?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || { let mut v = vec![]; let _ = stdout.read_to_end(&mut v); v });
    let err = std::thread::spawn(move || { let mut v = vec![]; let _ = stderr.read_to_end(&mut v); v });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() > timeout => { let _ = child.kill(); let _ = child.wait(); break None; }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break None,
        }
    };
    let data = out.join().unwrap_or_default();
    let complaint = err.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => Ok(data),
        _ => Err(herdr_error(&complaint).unwrap_or_else(unreachable)),
    }
}

pub fn fetch_snapshot(session: &str) -> Result<Vec<Agent>, MonitorError> {
    let data = run_herdr(&["api", "snapshot"], session, Duration::from_secs(4))?;
    Ok(enrich(decode_snapshot(&data)?))
}

/// Add what Herdr does not know, from local files only (no processes): git
/// branch, Claude background jobs and subagents, and each harness's last
/// tool from its own session log.
pub fn enrich(agents: Vec<Agent>) -> Vec<Agent> {
    let jobs = background_jobs(Duration::from_secs(3600), live_job_ids(&default_daemon_root()));
    agents
        .into_iter()
        .map(|mut agent| {
            let path = expand_home(&agent.cwd);
            agent.branch = git_branch(&path);
            if agent.name == "claude" {
                agent = agent.with_jobs(jobs.get(&path).map(Vec::as_slice).unwrap_or(&[]));
            }
            // Herdr does not report Kiro's session: the newest one Kiro
            // keeps for this folder.
            let kiro = (harness_for(&agent.name) == "kiro").then(|| kiro_session_for(&path)).flatten();
            if agent.session.is_none() { agent.session = kiro.as_ref().map(|k| k.0.clone()); }
            // A terminal title that only names the harness or the shell says
            // nothing: the session's own title or the last prompt instead.
            if generic_title(&agent.activity, harness_for(&agent.name)) {
                let better = match harness_for(&agent.name) {
                    "kiro" => kiro.map(|k| k.1).filter(|t| !t.is_empty()),
                    "codex" => agent.session.as_deref().and_then(codex_transcript).and_then(|f| file_tail(&f, 131_072))
                        .and_then(|t| crate::standalone::codex_view(&t, Duration::ZERO, false).prompt),
                    _ => None,
                };
                if let Some(text) = better { agent.activity = text; }
            }
            let Some(session) = agent.session.clone() else { return agent };
            match harness_for(&agent.name) {
                "claude" => {
                    agent.action = current_action(&session);
                    let helpers = subagent_actions(&session, Duration::from_secs(30));
                    let count = helpers.len();
                    agent.subagent_actions = helpers;
                    agent.with_subagents(count).with_limit(session_limit(&session))
                }
                "codex" => { agent.action = recent_action(codex_transcript(&session).as_deref(), codex_action); agent }
                "kiro" => { agent.action = recent_action(kiro_transcript(&session).as_deref(), kiro_action); agent }
                _ => agent, // no readable activity: the hero makes the rounds
            }
        })
        .collect()
}

/// One classic RPG hero per harness. Any other harness gets the plain hero.
pub const HERO_ORDER: [&str; 5] = ["claude", "codex", "kiro", "gemini", "hero"];

pub fn harness_for(agent: &str) -> &'static str {
    let name = agent.to_lowercase();
    HERO_ORDER.iter().copied().find(|h| *h != "hero" && name.contains(h)).unwrap_or("hero")
}

/// Claude Code background sessions run as jobs, not in the pane: the pane
/// only shows the job list waiting for input, so Herdr reports it idle while
/// its jobs work. Each job keeps ~/.claude/jobs/<id>/state.json with its
/// state and cwd.
#[derive(Clone, Debug)]
pub struct BackgroundJob {
    pub state: String,
    pub needs: Option<String>,
    pub transcript: Option<String>,
    pub updated: SystemTime,
    /// For a job stuck on the usage limit: when it resets (unix seconds).
    pub resets: Option<i64>,
}

pub fn default_daemon_root() -> String {
    format!("/tmp/cc-daemon-{}", unsafe { libc::getuid() })
}

/// Jobs whose process is alive: Claude Code's daemon keeps one terminal
/// socket per running job, /tmp/cc-daemon-<uid>/<daemon>/pty/<job>.sock.
/// A job that died leaves its last state.json behind, frozen, so without
/// this it could ask for attention for an hour. None when that folder does
/// not exist (another Claude Code layout): then every recent job counts.
pub fn live_job_ids(root: &str) -> Option<HashSet<String>> {
    let daemons = std::fs::read_dir(root).ok()?;
    let mut live = HashSet::new();
    for daemon in daemons.flatten() {
        if let Ok(files) = std::fs::read_dir(daemon.path().join("pty")) {
            for file in files.flatten() {
                let name = file.file_name().to_string_lossy().to_string();
                if let Some(id) = name.strip_suffix(".sock") { live.insert(id.to_string()); }
            }
        }
    }
    Some(live)
}

/// A job's effective state. Its `state` can stay "blocked" for a whole
/// interactive session (it marks who holds the turn), so the live `tempo`
/// (active / idle / blocked) is the authority when present; a pending
/// question (`needs`) blocks unless the job is active again.
pub fn job_state(state: &str, tempo: Option<&str>, has_needs: bool) -> String {
    match tempo {
        Some("active") => "working".into(),
        Some("blocked") => "blocked".into(),
        Some("idle") => if has_needs { "blocked".into() } else { "idle".into() },
        _ => state.into(),
    }
}

pub fn background_jobs(within: Duration, live: Option<HashSet<String>>) -> HashMap<String, Vec<BackgroundJob>> {
    let mut by_cwd: HashMap<String, Vec<BackgroundJob>> = HashMap::new();
    let Ok(dirs) = std::fs::read_dir(home_dir().join(".claude/jobs")) else { return by_cwd };
    let cutoff = SystemTime::now().checked_sub(within).unwrap_or(SystemTime::UNIX_EPOCH);
    for dir in dirs.flatten() {
        let id = dir.file_name().to_string_lossy().to_string();
        if let Some(live) = &live { if !live.contains(&id) { continue; } }
        let Ok(data) = std::fs::read(dir.path().join("state.json")) else { continue };
        let Ok(job) = serde_json::from_slice::<Value>(&data) else { continue };
        let (Some(state), Some(cwd)) = (str_of(&job, "state"), str_of(&job, "originCwd").or_else(|| str_of(&job, "cwd"))) else { continue };
        let Some(updated) = str_of(&job, "updatedAt").and_then(parse_iso8601) else { continue };
        let needs = str_of(&job, "needs").filter(|n| !n.trim().is_empty()).map(str::to_string);
        let transcript = str_of(&job, "linkScanPath").map(str::to_string);
        let mut state = job_state(state, str_of(&job, "tempo"), needs.is_some());
        let mut resets = None;
        // A job stopped by the usage limit is trapped until it resets; once
        // that time has passed it waits for a retry, which is a question again.
        let said = [needs.as_deref(), str_of(&job, "detail")].into_iter().flatten().find(|t| is_limit_text(t)).map(str::to_string);
        if let (true, Some(said)) = (state == "blocked", said) {
            resets = transcript.as_deref().and_then(transcript_limit).flatten().or_else(|| resets_from_text(&said, chrono::Local::now()));
            if resets.map(|r| r > unix_now()).unwrap_or(true) { state = "limited".into(); }
        }
        // A limit can last hours without the job writing anything: keep it until it resets.
        if updated <= cutoff && !(state == "limited" && resets.is_some()) { continue; }
        by_cwd.entry(cwd.to_string()).or_default().push(BackgroundJob { state, needs, transcript, updated, resets });
    }
    by_cwd
}

pub fn unix_now() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Claude Code's usage-limit messages: "You've hit your session limit ·
/// resets 2:20pm (…)", "Claude usage limit reached…", a job's "rate limited
/// — wait and retry".
pub fn is_limit_text(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("rate limited") || (t.contains("limit") && (t.contains("hit your") || t.contains("limit reached")))
}

/// The reset time in "resets 2:20pm" (or "reset at 5pm", "resets 14:05"):
/// its next occurrence in local time, as unix seconds.
pub fn resets_from_text(text: &str, now: chrono::DateTime<chrono::Local>) -> Option<i64> {
    use chrono::TimeZone;
    let lower = text.to_lowercase();
    let at = lower.find("resets ").map(|i| i + 7).or_else(|| lower.find("reset at ").map(|i| i + 9))?;
    let rest = lower[at..].trim_start().trim_start_matches("at ").trim_start();
    let clock: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == ':').collect();
    if clock.is_empty() { return None; }
    let after = rest[clock.len()..].trim_start();
    let mut parts = clock.split(':');
    let mut hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next().map(|m| m.parse().ok()).unwrap_or(Some(0))?;
    if after.starts_with("pm") && hour < 12 { hour += 12; }
    if after.starts_with("am") && hour == 12 { hour = 0; }
    if hour > 23 || minute > 59 { return None; }
    let today = now.date_naive().and_hms_opt(hour, minute, 0)?;
    let mut when = chrono::Local.from_local_datetime(&today).earliest()?;
    if when <= now { when += chrono::Duration::days(1); }
    Some(when.timestamp())
}

/// Whether the tail of a Claude Code transcript ends on a usage limit, and
/// when it resets: Claude Code writes the refusal as a synthetic assistant
/// row with `"error":"rate_limit"` and `quotaLimits.resetsAt`. Anything the
/// user or the agent writes after it means the session moved on.
/// Some(None) is a limit whose reset time is unknown.
pub fn transcript_limit(path: &str) -> Option<Option<i64>> {
    let modified = file_modified(path)?;
    let mut guard = TRANSCRIPT_LIMITS.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((when, limit)) = cache.get(path) { if *when == modified { return *limit; } }
    let limit = file_tail(path, 131_072).and_then(|t| limit_in_tail(&t));
    cache.insert(path.to_string(), (modified, limit));
    limit
}

static TRANSCRIPT_LIMITS: Mutex<Option<HashMap<String, (SystemTime, Option<Option<i64>>)>>> = Mutex::new(None);

pub fn limit_in_tail(tail: &str) -> Option<Option<i64>> {
    for line in tail.lines().rev() {
        if !(line.contains("\"assistant\"") || line.contains("\"user\"")) { continue; }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        match str_of(&row, "type") {
            Some("assistant") => {
                let text: String = row.get("message").and_then(|m| m.get("content")).and_then(Value::as_array)
                    .map(|c| c.iter().filter_map(|b| str_of(b, "text")).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default();
                let api_error = row.get("isApiErrorMessage").and_then(Value::as_bool).unwrap_or(false);
                if str_of(&row, "error") != Some("rate_limit") && !(api_error && is_limit_text(&text)) { return None; }
                let resets = row.get("quotaLimits").and_then(|q| q.get("resetsAt")).and_then(Value::as_i64)
                    .or_else(|| resets_from_text(&text, chrono::Local::now()));
                return Some(resets);
            }
            Some("user") => return None,
            _ => continue,
        }
    }
    None
}

/// The usage limit a Claude Code session is stuck on, if it has not reset yet.
pub fn session_limit(session: &str) -> Option<Option<i64>> {
    let path = claude_transcript(session)?;
    let limit = transcript_limit(&path)?;
    if limit.map(|r| r <= unix_now()).unwrap_or(false) { return None; }
    // An unknown reset time only counts for a few hours after the refusal.
    if limit.is_none() && file_modified(&path)? < SystemTime::now().checked_sub(Duration::from_secs(6 * 3600))? { return None; }
    Some(limit)
}

/// ~/.claude/projects/<project>/<session>.jsonl for a session id.
pub fn claude_transcript(session: &str) -> Option<String> {
    if session.starts_with('/') { return Some(session.to_string()); }
    let dirs = std::fs::read_dir(home_dir().join(".claude/projects")).ok()?;
    dirs.flatten().map(|d| d.path().join(format!("{session}.jsonl"))).find(|f| f.exists()).map(|f| f.to_string_lossy().to_string())
}

fn parse_iso8601(text: &str) -> Option<SystemTime> {
    let when = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    Some(SystemTime::from(when))
}

/// Current git branch for a directory, read straight from .git/HEAD (no
/// process launch). Handles worktrees, where .git is a file pointing at the
/// real git dir. Detached HEADs show the short commit.
pub fn git_branch(path: &str) -> Option<String> {
    let mut dir = PathBuf::from(path);
    loop {
        let dot_git = dir.join(".git");
        if let Ok(meta) = std::fs::metadata(&dot_git) {
            let git_dir = if meta.is_dir() {
                dot_git
            } else {
                let text = std::fs::read_to_string(&dot_git).ok()?;
                let line = text.lines().next()?;
                let target = line.strip_prefix("gitdir:")?.trim();
                if Path::new(target).is_absolute() { PathBuf::from(target) } else { dir.join(target) }
            };
            let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
            let head = head.trim();
            if let Some(branch) = head.strip_prefix("ref: refs/heads/") { return Some(branch.to_string()); }
            return if head.is_empty() { None } else { Some(head.chars().take(7).collect()) };
        }
        if !dir.pop() || dir.as_os_str().is_empty() { return None; }
    }
}

struct ActionCache {
    cache: HashMap<String, (SystemTime, String)>,
}
static TRANSCRIPT_ACTIONS: Mutex<Option<ActionCache>> = Mutex::new(None);

fn with_action_cache<T>(f: impl FnOnce(&mut HashMap<String, (SystemTime, String)>) -> T) -> T {
    let mut guard = TRANSCRIPT_ACTIONS.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(|| ActionCache { cache: HashMap::new() });
    f(&mut cache.cache)
}

/// Herdr does not report subagents, but Claude Code writes one transcript per
/// subagent under ~/.claude/projects/<project>/<session>/subagents/. A
/// subagent counts as active while its transcript was written recently.
/// For each active subagent this returns what it is doing: the last tool in
/// its own transcript (or typing when it only wrote text), in a stable
/// order. Tails are cached by modification date.
pub fn subagent_actions(session: &str, within: Duration) -> Vec<String> {
    active_subagent_files(session, within)
        .into_iter()
        .map(|(file, modified)| {
            with_action_cache(|cache| {
                let key = file.to_string_lossy().to_string();
                if let Some((when, action)) = cache.get(&key) { if *when == modified { return action.clone(); } }
                let action = file_tail(&key, 65_536).and_then(|t| last_action(&t)).unwrap_or_else(|| "type".into());
                cache.insert(key, (modified, action.clone()));
                action
            })
        })
        .collect()
}

fn active_subagent_files(session: &str, within: Duration) -> Vec<(PathBuf, SystemTime)> {
    let Ok(dirs) = std::fs::read_dir(home_dir().join(".claude/projects")) else { return vec![] };
    let cutoff = SystemTime::now().checked_sub(within).unwrap_or(SystemTime::UNIX_EPOCH);
    for dir in dirs.flatten() {
        let folder = dir.path().join(session).join("subagents");
        let Ok(files) = std::fs::read_dir(&folder) else { continue };
        let mut found: Vec<(PathBuf, SystemTime)> = files
            .flatten()
            .filter_map(|f| {
                let path = f.path();
                if path.extension().map(|e| e != "jsonl").unwrap_or(true) { return None; }
                let modified = f.metadata().ok()?.modified().ok()?;
                (modified > cutoff).then_some((path, modified))
            })
            .collect();
        found.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));
        return found;
    }
    vec![]
}

/// What the dungeon should say about its link to Herdr, if anything: lost
/// (an error, red) or stale (no fresh data for a while, amber), with how
/// long ago the last good update was. None while all is well.
pub fn connection_note(error: Option<&str>, updated: Option<Instant>, now: Instant, stale_after: f64) -> Option<(String, bool)> {
    let age = updated.map(|u| now.saturating_duration_since(u).as_secs_f64());
    let ago = age.map(|a| if a < 90.0 { trf("hace {} s", &[&(a as i64)]) } else { trf("hace {} min", &[&((a / 60.0) as i64)]) });
    if error.is_some() {
        let mut text = tr("Herdr desconectado");
        if let Some(ago) = ago { text += &format!(" · {}", trf("última actualización {}", &[&ago])); }
        return Some((text, true));
    }
    let (age, ago) = (age?, ago?);
    if age < stale_after { return None; }
    Some((trf("Datos sin actualizar · última actualización {}", &[&ago]), false))
}

/// Harnesses offered when creating an agent (all are `herdr agent start --kind` values).
pub const CREATABLE_KINDS: [&str; 8] = ["claude", "codex", "gemini", "kiro", "opencode", "cursor", "copilot", "amp"];

/// The link in `text` around char `index`: a URL, or something path-like
/// (has a `/`, starts with `~`, or is a `name.ext`), without the quotes,
/// brackets and trailing punctuation around it.
pub fn link_at(text: &str, index: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if index > chars.len() { return None; }
    let stop = |c: char| c.is_whitespace() || "\"'`<>()[]{}│".contains(c);
    let mut start = index.min(chars.len());
    while start > 0 && !stop(chars[start - 1]) { start -= 1; }
    let mut end = index;
    while end < chars.len() && !stop(chars[end]) { end += 1; }
    if start >= end { return None; }
    let token: String = chars[start..end].iter().collect();
    let token = token.trim_end_matches(['.', ',', ';', '!', '?', ':']).trim_start_matches(['•', '⎿', '●', '-', '*']);
    if token.is_empty() { return None; }
    let url = ["http://", "https://", "file://"].iter().any(|p| token.starts_with(p));
    let pathish = token.contains('/') || token.starts_with('~') || token.rsplit_once('.').map(|(name, ext)| !name.is_empty() && (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) && !ext.chars().all(|c| c.is_ascii_digit())).unwrap_or(false);
    (url || pathish).then(|| token.to_string())
}

/// What a link opens: a URL as it is; a path with `:line[:col]` dropped,
/// `~` expanded and a relative one taken from `cwd`, if it exists.
pub fn resolve_link(link: &str, cwd: &str) -> Option<String> {
    if ["http://", "https://", "file://"].iter().any(|p| link.starts_with(p)) { return Some(link.to_string()); }
    let mut path = link;
    for _ in 0..2 {
        if let Some((head, tail)) = path.rsplit_once(':') { if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) { path = head; } }
    }
    let expanded = expand_home(path);
    let full = if expanded.starts_with('/') { PathBuf::from(&expanded) } else { PathBuf::from(expand_home(cwd)).join(&expanded) };
    full.exists().then(|| full.to_string_lossy().to_string())
}

/// A terminal window in `cwd`, to type a command in: Terminal on macOS,
/// the first terminal emulator found on Linux.
pub fn open_terminal(cwd: &str) {
    let cwd = if cwd.trim().is_empty() { home_dir().to_string_lossy().to_string() } else { expand_home(cwd) };
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open").args(["-a", "Terminal", &cwd]).spawn();
        return;
    }
    for terminal in ["x-terminal-emulator", "gnome-terminal", "konsole", "xfce4-terminal", "alacritty", "kitty", "xterm"] {
        if std::process::Command::new(terminal).current_dir(&cwd).spawn().is_ok() { return; }
    }
}

/// What ⌘-click on a link does: a URL opens in the browser; a file or a
/// folder is shown in Finder (selected in its folder), or in the file
/// manager on Linux, instead of being opened.
pub fn reveal_target(target: &str) {
    if ["http://", "https://"].iter().any(|p| target.starts_with(p)) { return open_target(target); }
    let path = target.strip_prefix("file://").unwrap_or(target);
    if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open").args(["-R", path]).spawn();
        return;
    }
    let uri = format!("file://{path}");
    let shown = std::process::Command::new("dbus-send")
        .args(["--session", "--dest=org.freedesktop.FileManager1", "--type=method_call", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems", &format!("array:string:{uri}"), "string:"])
        .status().map(|s| s.success()).unwrap_or(false);
    if !shown {
        let folder = std::path::Path::new(path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        open_target(&folder);
    }
}

/// Open a URL or a path with the system (open / xdg-open).
pub fn open_target(target: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(target).spawn();
}

/// The harnesses installed on this machine, of CREATABLE_KINDS: their
/// command is on PATH or in a usual install folder (an app opened from
/// Finder gets a short PATH). All of them when none is found.
pub fn installed_kinds() -> Vec<&'static str> {
    let home = home_dir();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    dirs.extend([".local/bin", ".cargo/bin", ".bun/bin", ".npm-global/bin", ".volta/bin", ".opencode/bin"].iter().map(|d| home.join(d)));
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].iter().map(PathBuf::from));
    let found: Vec<&'static str> = CREATABLE_KINDS.iter().copied().filter(|kind| {
        let command = match *kind { "kiro" => "kiro-cli", "cursor" => "cursor-agent", other => other };
        dirs.iter().any(|d| d.join(command).is_file())
    }).collect();
    if found.is_empty() { CREATABLE_KINDS.to_vec() } else { found }
}

/// A unique, readable Herdr agent name: kind, folder and a short suffix.
pub fn agent_name(kind: &str, folder: &str) -> String {
    let slug: String = folder.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    let slug: String = if slug.is_empty() { "agent".into() } else { slug.chars().take(24).collect() };
    let suffix: String = format!("{:04x}", (std::time::SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) ^ std::process::id() as u128) & 0xffff);
    format!("{kind}-{slug}-{suffix}")
}

/// The new pane's id in `herdr workspace create` output (`.result.root_pane`).
pub fn root_pane(data: &[u8]) -> Option<String> {
    let root: Value = serde_json::from_slice(data).ok()?;
    let result = root.get("result")?;
    match result.get("root_pane")? {
        Value::Object(pane) => pane.get("pane_id")?.as_str().map(str::to_string),
        Value::String(id) => Some(id.clone()),
        _ => None,
    }
}

/// The workspace id in a `herdr workspace create` reply.
pub fn created_workspace(data: &[u8]) -> Option<String> {
    let root: Value = serde_json::from_slice(data).ok()?;
    root.get("result")?.get("workspace")?.get("workspace_id")?.as_str().map(str::to_string)
}

/// A Herdr session, from `herdr session list --json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HerdrSession {
    pub name: String,
    pub running: bool,
}

pub fn decode_sessions(data: &[u8]) -> Vec<HerdrSession> {
    let Ok(root) = serde_json::from_slice::<Value>(data) else { return vec![] };
    root.get("sessions")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(HerdrSession { name: str_of(row, "name")?.to_string(), running: row.get("running").and_then(Value::as_bool).unwrap_or(false) })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The sessions Herdr knows about; empty when Herdr cannot be reached.
pub fn list_sessions() -> Vec<HerdrSession> {
    run_herdr(&["session", "list", "--json"], "default", Duration::from_secs(2)).map(|d| decode_sessions(&d)).unwrap_or_default()
}

/// Which rooms the HUD's state chips let through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatusFilter {
    All,
    Blocked,
    Limited,
    Working,
    Idle,
    Done,
}

impl StatusFilter {
    pub const ALL: [StatusFilter; 6] = [StatusFilter::All, StatusFilter::Blocked, StatusFilter::Limited, StatusFilter::Working, StatusFilter::Idle, StatusFilter::Done];

    pub fn raw(self) -> &'static str {
        match self {
            StatusFilter::All => "all",
            StatusFilter::Blocked => "blocked",
            StatusFilter::Limited => "limited",
            StatusFilter::Working => "working",
            StatusFilter::Idle => "idle",
            StatusFilter::Done => "done",
        }
    }

    pub fn from_raw(raw: &str) -> StatusFilter {
        StatusFilter::ALL.into_iter().find(|f| f.raw() == raw).unwrap_or(StatusFilter::All)
    }

    pub fn admits(self, agent: &Agent) -> bool {
        self == StatusFilter::All || agent.status == self.raw()
    }
}

/// Lower-case and strip the accents of common Latin letters, so a search
/// for "carrito" finds "Cárrito".
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            other => other,
        })
        .collect()
}

/// Agents that pass the state filter and whose project, harness, branch,
/// folder or activity contain every word of the query (ignoring case and
/// accents). Order is kept, so attention stays first.
pub fn filter_agents(agents: &[Agent], status: StatusFilter, query: &str) -> Vec<Agent> {
    let words: Vec<String> = fold(query).split_whitespace().map(str::to_string).collect();
    agents
        .iter()
        .filter(|agent| {
            if !status.admits(agent) { return false; }
            let haystack = fold(&[agent.project.as_str(), agent.name.as_str(), agent.branch.as_deref().unwrap_or(""), &agent.folder(), agent.cwd.as_str(), agent.activity.as_str()].join("\n"));
            words.iter().all(|w| haystack.contains(w.as_str()))
        })
        .cloned()
        .collect()
}

pub fn demo_agents(tick: usize) -> Vec<Agent> {
    let specs: [(&str, &str, &str, &str); 6] = [
        ("claude", "Website", "working", "Construyendo la página de ajustes"),
        ("codex", "API service", ["working", "blocked", "done", "idle"][(tick / 10) % 4], "Revisando las pruebas"),
        ("kiro", "Mobile app", "blocked", "Esperando tu respuesta"),
        ("claude", "Design system", ["limited", "idle"][(tick / 12) % 2], "Listo para la próxima tarea"),
        ("gemini", "Documentation", "done", "Documentación actualizada"),
        ("codex", "Game engine", "working", "Ajustando el movimiento"),
    ];
    let subagents = [2, 0, 0, 0, 0, 5];
    let helpers: [&[&str]; 6] = [&["read", "brew"], &[], &[], &[], &[], &["forge", "read", "brew", "gems", "type"]];
    let branches: [Option<&str>; 6] = [Some("main"), Some("feature/tests"), None, Some("main"), Some("docs"), Some("physics")];
    specs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let cwd = format!("~/demo/{}", s.1.to_lowercase().replace(' ', "-"));
            let mut agent = Agent::new(&format!("demo:{i}"), s.0, s.2, s.1, &tr(s.3), &cwd);
            agent.branch = branches[i].map(str::to_string);
            if i == 0 { agent.action = Some(["read", "forge", "brew", "summon", "plan", "type"][(tick / 8) % 6].to_string()); }
            if agent.status == "limited" {
                // Fixed for the whole run, so the countdown ticks down.
                static DEMO_RESET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
                agent.limit_resets = Some(*DEMO_RESET.get_or_init(|| unix_now() + 73 * 60 + 5));
            }
            if i == 4 {
                // Done, but a shell it left in the background still brews.
                static DEMO_SHELL: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
                let started = *DEMO_SHELL.get_or_init(|| unix_now() - 6 * 60 - 23);
                agent.shells = vec![crate::shells::Shell {
                    pid: 0,
                    command: "cd pipeline && python3 fetch.py --limit 3000 > data/fetch.log 2>&1".into(),
                    label: tr("Descargar y contar 3000 artículos"),
                    started,
                    task: None,
                    last_line: Some(format!("{}/3000", 1200 + (tick * 7) % 1800)),
                    output: None,
                }];
            }
            agent.subagent_actions = helpers[i].iter().map(|s| s.to_string()).collect();
            agent.with_subagents(subagents[i])
        })
        .collect()
}

/// The station a tool sends the hero to: reading (also web search and
/// fetch), forging (editing files), brewing (shell commands), summoning
/// (subagents), planning (todo lists and plans) or typing (anything else,
/// and thinking or writing a reply).
pub fn tool_action(tool: &str) -> &'static str {
    match tool {
        "Read" | "Grep" | "Glob" | "LS" | "WebSearch" | "WebFetch" | "NotebookRead" => "read",
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => "forge",
        "Bash" | "BashOutput" | "KillShell" | "KillBash" | "Monitor" => "brew",
        "Task" | "Agent" => "summon",
        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList" | "ExitPlanMode" | "EnterPlanMode" => "plan",
        _ => "type",
    }
}

fn assistant_content(line: &str) -> Option<Vec<Value>> {
    if !line.contains("\"assistant\"") { return None; }
    let row: Value = serde_json::from_str(line).ok()?;
    if row.get("type")?.as_str()? != "assistant" { return None; }
    row.get("message")?.get("content")?.as_array().cloned()
}

/// The action of the latest assistant message in the tail of a Claude Code
/// transcript: the last tool it called, or typing when it only wrote text.
/// Only message types and tool names are looked at, never their contents.
pub fn last_action(tail: &str) -> Option<String> {
    for line in tail.lines().rev() {
        let Some(content) = assistant_content(line) else { continue };
        if let Some(tool) = content.iter().rev().find(|c| c.get("type").and_then(Value::as_str) == Some("tool_use")) {
            return Some(tool_action(tool.get("name").and_then(Value::as_str).unwrap_or("")).to_string());
        }
        return Some("type".into());
    }
    None
}

// ---- Other harnesses

fn file_modified(path: &str) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// The action in a transcript written in the last two minutes, from its
/// tail. Parsed again only when the file changes.
pub fn recent_action(file: Option<&str>, parse: fn(&str) -> Option<String>) -> Option<String> {
    let file = file?;
    let modified = file_modified(file)?;
    if modified < SystemTime::now().checked_sub(Duration::from_secs(120))? { return None; }
    with_action_cache(|cache| {
        if let Some((when, action)) = cache.get(file) { if *when == modified { return Some(action.clone()); } }
        let action = file_tail(file, 131_072).and_then(|t| parse(&t))?;
        cache.insert(file.to_string(), (modified, action.clone()));
        Some(action)
    })
}

/// A shell command that only looks at files (the way Codex reads them)
/// counts as reading; anything else is brewing.
pub fn shell_action(command: &str) -> &'static str {
    const READERS: [&str; 15] = ["cat", "rg", "grep", "sed", "ls", "find", "head", "tail", "nl", "wc", "less", "tree", "fd", "bat", "awk"];
    let first = command.split(|c| " ;|&\n".contains(c)).next().unwrap_or("");
    let base = Path::new(first).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    if READERS.contains(&base.as_str()) { "read" } else { "brew" }
}

/// The action of the latest Codex step in the tail of its rollout
/// (~/.codex/sessions/…/rollout-…-<session>.jsonl): its last tool call, or
/// typing while it reasons or writes.
pub fn codex_action(tail: &str) -> Option<String> {
    for line in tail.lines().rev() {
        if !line.contains("\"response_item\"") { continue; }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let Some(item) = row.get("payload") else { continue };
        let Some(kind) = str_of(item, "type") else { continue };
        match kind {
            "function_call" | "custom_tool_call" | "local_shell_call" => {
                let name = str_of(item, "name").unwrap_or("shell");
                return Some(match name {
                    "exec_command" | "shell" | "local_shell" | "container.exec" => {
                        let arguments = str_of(item, "arguments").and_then(|a| serde_json::from_str::<Value>(a).ok());
                        let command = arguments
                            .as_ref()
                            .and_then(|a| {
                                a.get("cmd").and_then(Value::as_str).map(str::to_string).or_else(|| {
                                    a.get("command").and_then(Value::as_array).map(|parts| parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "))
                                })
                            })
                            .unwrap_or_default();
                        shell_action(&command).to_string()
                    }
                    "apply_patch" => "forge".into(),
                    "update_plan" => "plan".into(),
                    "web_search" | "view_image" => "read".into(),
                    _ => "type".into(),
                });
            }
            "web_search_call" => return Some("read".into()),
            "message" | "reasoning" => return if str_of(item, "role") == Some("user") { None } else { Some("type".into()) },
            _ => continue,
        }
    }
    None
}

/// The action of the latest Kiro reply in the tail of its session
/// (~/.kiro/sessions/cli/<session>.jsonl): its last tool, or typing.
pub fn kiro_action(tail: &str) -> Option<String> {
    for line in tail.lines().rev() {
        if !line.contains("\"AssistantMessage\"") { continue; }
        let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
        let Some(content) = row.get("data").and_then(|d| d.get("content")).and_then(Value::as_array) else { continue };
        let tool = content.iter().rev().find(|c| str_of(c, "kind") == Some("toolUse")).and_then(|c| c.get("data"));
        let Some(name) = tool.and_then(|t| str_of(t, "name")).map(str::to_lowercase) else { return Some("type".into()) };
        let name = name.as_str();
        if ["read", "grep", "glob", "ls", "fs_read", "web_search", "web_fetch", "code"].contains(&name) { return Some("read".into()); }
        if ["write", "edit", "fs_write", "str_replace", "create"].contains(&name) { return Some("forge".into()); }
        if ["shell", "execute_bash", "bash"].contains(&name) { return Some("brew".into()); }
        if name.contains("todo") || name.contains("plan") { return Some("plan".into()); }
        if name.contains("agent") || name.contains("delegate") { return Some("summon".into()); }
        return Some("type".into());
    }
    None
}

static TRANSCRIPT_PATHS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Codex's rollout for a session: an absolute path as given, else the
/// newest `rollout-*-<session>.jsonl` in the last two weeks of day folders.
pub fn codex_transcript(session: &str) -> Option<String> {
    if session.starts_with('/') { return Some(session.to_string()); }
    let mut guard = TRANSCRIPT_PATHS.lock().unwrap_or_else(|e| e.into_inner());
    let paths = guard.get_or_insert_with(HashMap::new);
    if let Some(known) = paths.get(session) { return Some(known.clone()); }
    let root = home_dir().join(".codex/sessions");
    let today = chrono::Local::now();
    for back in 0..14 {
        let day = today - chrono::Duration::days(back);
        let folder = root.join(day.format("%Y/%m/%d").to_string());
        let Ok(files) = std::fs::read_dir(&folder) else { continue };
        let suffix = format!("-{session}.jsonl");
        let newest = files.flatten().map(|f| f.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(&suffix)).max();
        if let Some(name) = newest {
            let path = folder.join(name).to_string_lossy().to_string();
            paths.insert(session.to_string(), path.clone());
            return Some(path);
        }
    }
    None
}

/// Kiro's session file: an absolute path as given, else ~/.kiro/sessions/cli/<session>.jsonl.
/// A terminal title that tells nothing about the work: empty, the
/// harness's own name ("kiro-cli", "codex"), a shell or a path.
pub fn generic_title(title: &str, harness: &str) -> bool {
    let t = title.trim().to_lowercase();
    t.is_empty() || t == tr("Sin título de actividad").to_lowercase()
        || (t.contains(harness) && t.chars().count() <= harness.len() + 5)
        || ["zsh", "-zsh", "bash", "-bash", "fish", "sh"].contains(&t.as_str())
        || t.starts_with('~') || t.starts_with('/')
}

/// Kiro's sessions as read: file → (modified, folder, title).
static KIRO_SESSIONS: Mutex<Option<HashMap<PathBuf, (SystemTime, String, String)>>> = Mutex::new(None);

/// The newest Kiro session for a folder: its ID and the title Kiro gave it,
/// from ~/.kiro/sessions/cli/<id>.json (each file read again only when it
/// changes).
pub fn kiro_session_for(folder: &str) -> Option<(String, String)> {
    let files = std::fs::read_dir(home_dir().join(".kiro/sessions/cli")).ok()?;
    let mut guard = KIRO_SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    let known = guard.get_or_insert_with(HashMap::new);
    let mut best: Option<(SystemTime, String, String)> = None;
    for file in files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "json")) {
        let Some(modified) = file_modified(&file.to_string_lossy()) else { continue };
        if known.get(&file).map(|k| k.0) != Some(modified) {
            let Some(row) = std::fs::read(&file).ok().and_then(|d| serde_json::from_slice::<Value>(&d).ok()) else { continue };
            let read = |key| str_of(&row, key).unwrap_or("").to_string();
            known.insert(file.clone(), (modified, read("cwd"), read("title")));
        }
        let (when, cwd, title) = &known[&file];
        if cwd.trim_end_matches('/') != folder.trim_end_matches('/') || best.as_ref().is_some_and(|b| b.0 >= *when) { continue; }
        let id = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        best = Some((*when, id, title.clone()));
    }
    best.map(|(_, id, title)| (id, title))
}

pub fn kiro_transcript(session: &str) -> Option<String> {
    if session.starts_with('/') { return Some(session.to_string()); }
    let path = home_dir().join(format!(".kiro/sessions/cli/{session}.jsonl"));
    path.exists().then(|| path.to_string_lossy().to_string())
}

/// The last `bytes` of a file, as text.
pub fn file_tail(path: &str, bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(bytes))).ok()?;
    let mut data = vec![];
    file.read_to_end(&mut data).ok()?;
    Some(String::from_utf8_lossy(&data).to_string())
}

/// The text of the latest assistant message that has any, in the tail of a
/// Claude Code transcript. A message is written as one row per content
/// block, so every row sharing the last one's message id is joined.
pub fn last_assistant_text(tail: &str) -> Option<String> {
    let rows: Vec<&str> = tail.lines().collect();
    let mut last_id: Option<String> = None;
    for line in rows.iter().rev() {
        let Some(content) = assistant_content(line) else { continue };
        let text: Vec<&str> = content.iter().filter(|c| str_of(c, "type") == Some("text")).filter_map(|c| str_of(c, "text")).collect();
        if text.join("").is_empty() { continue; }
        last_id = serde_json::from_str::<Value>(line).ok().and_then(|r| r.get("message")?.get("id")?.as_str().map(str::to_string));
        if last_id.is_none() { return Some(text.join("\n")); }
        break;
    }
    let id = last_id?;
    let mut parts: Vec<String> = vec![];
    for line in rows {
        let Some(content) = assistant_content(line) else { continue };
        let same = serde_json::from_str::<Value>(line).ok().and_then(|r| r.get("message")?.get("id")?.as_str().map(str::to_string)) == Some(id.clone());
        if !same { continue; }
        for c in content.iter().filter(|c| str_of(c, "type") == Some("text")) {
            if let Some(t) = str_of(c, "text") { if !t.is_empty() { parts.push(t.to_string()); } }
        }
    }
    let joined = parts.join("\n");
    if joined.is_empty() { None } else { Some(joined) }
}

/// The question in an assistant message: from the line that asks it (the
/// job's `needs`, or a "needs input:" line) to the end, so its options come
/// along. Markdown emphasis is dropped; the panel is plain text.
pub fn question_lines(message: &str, needs: Option<&str>) -> Vec<String> {
    let mut lines: Vec<String> = message.split('\n').map(|l| l.replace("**", "").replace('`', "")).collect();
    let needle = needs.map(str::trim).filter(|n| !n.is_empty());
    let start = lines.iter().rposition(|line| needle.map(|n| line.contains(n)).unwrap_or(false) || line.starts_with("needs input:"));
    if let Some(start) = start {
        lines = lines.split_off(start);
        lines[0] = lines[0].replace("needs input:", "").trim().to_string();
    } else {
        let keep = lines.len().saturating_sub(8);
        lines = lines.split_off(keep);
    }
    lines.into_iter().filter(|l| !l.trim().is_empty()).collect()
}

/// Whether a trimmed line is a numbered option ("❯ 2. Opus", "› 1. Yes", "3) No").
pub fn is_option(trimmed: &str) -> bool {
    option_prefix(trimmed).is_some()
}

/// `^([❯›]\s*)?\d+[.)]\s+`: the option number and where its text starts.
fn option_prefix(trimmed: &str) -> Option<(bool, u32, usize)> {
    let marked = after_marker(trimmed).is_some();
    let rest = after_marker(trimmed).unwrap_or(trimmed);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() { return None; }
    let after = &rest[digits.len()..];
    if !(after.starts_with('.') || after.starts_with(')')) { return None; }
    let after = &after[1..];
    let spaces = after.len() - after.trim_start().len();
    if spaces == 0 { return None; }
    let consumed = trimmed.len() - after.len() + spaces;
    Some((marked, digits.parse().ok()?, consumed))
}

/// The numbered options of the question an agent is asking (the last run of
/// "1. …", "2. …" lines), and which one its terminal highlights with ❯, if
/// any. A highlighted option means the agent shows a menu driven by arrow
/// keys; without one the options are plain text to answer with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuOptions {
    pub options: Vec<MenuOption>,
    pub highlighted: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuOption {
    pub number: u32,
    pub text: String,
    pub line: usize,
}

/// Why a fresh shell would not run the agent Herdr types into it: a question
/// it asks first eats the command (oh-my-zsh's update prompt takes its first
/// letter, and zsh then offers to correct the rest). The warning, with what
/// to do, read from the pane's screen.
pub fn shell_blocker(screen: &str) -> Option<String> {
    if screen.contains("[oh-my-zsh] Would you like to update?") {
        return Some(tr("oh-my-zsh pregunta si actualizarse y se come el comando. Corre omz update en una terminal (o pon zstyle ':omz:update' mode auto en ~/.zshrc) y vuelve a invocar."));
    }
    let correct = screen.lines().rev().find(|l| l.contains("zsh: correct '") && l.contains("[nyae]"))?;
    let typed = correct.split('\'').nth(1).unwrap_or_default();
    Some(trf("zsh preguntó si corregir «{}»: algo en la shell nueva se comió parte del comando. Abre una terminal y revisa qué pregunta al arrancar.", &[&typed]))
}

/// The hint under a menu driven by arrow keys ("Enter to confirm · Esc to
/// cancel"; Kiro's "↑↓ to navigate · ↵ to select" and "esc to close · Enter
/// to see more options"; Codex's "enter select · esc back" and "Press enter
/// to confirm or esc to cancel"), in any case.
pub fn is_menu_footer(trimmed: &str) -> bool {
    let lower = trimmed.to_lowercase();
    ["enter to confirm", "enter to select", "esc to cancel", "enter to continue", "↑↓ to navigate", "↵ to select", "enter to see more options", "enter select · esc"].iter().any(|s| lower.contains(s))
}

/// What a terminal points at its highlighted option with: ❯ (Claude Code,
/// Kiro) or › (Codex, which also marks its prompt and past messages so).
const MARKERS: [char; 2] = ['❯', '›'];

/// The text after a leading highlight marker, if the line starts with one.
fn after_marker(trimmed: &str) -> Option<&str> {
    let first = trimmed.chars().next().filter(|c| MARKERS.contains(c))?;
    Some(trimmed[first.len_utf8()..].trim_start())
}

impl MenuOptions {
    pub fn new(lines: &[String]) -> MenuOptions {
        let numbered = MenuOptions::numbered(lines);
        if !numbered.options.is_empty() { return numbered; }
        MenuOptions::unnumbered(lines)
    }

    /// A menu whose options have no numbers ("❯ Yes, I trust this folder",
    /// "  No, exit"): the run of lines around the highlighted one whose text
    /// starts in the same column, numbered here in order. A › only counts
    /// above a menu's footer: Codex marks its past messages with it too.
    fn unnumbered(lines: &[String]) -> MenuOptions {
        let mut menu = MenuOptions::default();
        let footer_at = lines.iter().rposition(|l| is_menu_footer(l.trim()));
        let footer = footer_at.unwrap_or(lines.len());
        let Some(marked) = lines[..footer].iter().rposition(|l| {
            let t = l.trim_start();
            after_marker(t).is_some_and(|rest| !rest.trim().is_empty()) && (t.starts_with('❯') || footer_at.is_some())
        }) else { return menu };
        let marker = lines[marked].chars().take_while(|c| c.is_whitespace()).count();
        let column = marker + 1 + lines[marked].chars().skip(marker + 1).take_while(|c| c.is_whitespace()).count();
        let text_at = |line: &String| -> Option<String> {
            let lead = line.chars().take_while(|c| c.is_whitespace()).count();
            let rest: String = line.chars().skip(column).collect();
            let head: String = line.chars().take(column).collect();
            let plain = lead == column;
            let pointed = after_marker(head.trim()).is_some_and(str::is_empty) && lead == marker;
            ((plain || pointed) && !rest.trim().is_empty() && !is_menu_footer(line.trim())).then(|| rest.trim_end().to_string())
        };
        let mut first = marked;
        while first > 0 && text_at(&lines[first - 1]).is_some() { first -= 1; }
        let mut last = marked;
        while last + 1 < footer && text_at(&lines[last + 1]).is_some() { last += 1; }
        if first == last { return menu; }
        for i in first..=last {
            if i == marked { menu.highlighted = Some(menu.options.len()); }
            menu.options.push(MenuOption { number: (menu.options.len() + 1) as u32, text: text_at(&lines[i]).unwrap_or_default(), line: i });
        }
        menu
    }

    fn numbered(lines: &[String]) -> MenuOptions {
        let mut menu = MenuOptions::default();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            let Some((marked, number, consumed)) = option_prefix(trimmed) else { continue };
            // A "1." after other options starts a newer question.
            if number == 1 || menu.options.last().map(|o| number != o.number + 1).unwrap_or(false) {
                menu.options.clear();
                menu.highlighted = None;
            }
            if marked { menu.highlighted = Some(menu.options.len()); }
            menu.options.push(MenuOption { number, text: trimmed[consumed..].to_string(), line: i });
            // An option's text the terminal wrapped goes on, indented past its number.
            let indent = line.len() - line.trim_start().len();
            for next in &lines[i + 1..] {
                let lead = next.len() - next.trim_start().len();
                if lead <= indent + 2 || option_prefix(next.trim()).is_some() || is_menu_footer(next.trim()) { break; }
                let text = &mut menu.options.last_mut().unwrap().text;
                text.push(' ');
                text.push_str(next.trim());
            }
        }
        menu
    }

    pub fn is_menu(&self) -> bool {
        self.highlighted.is_some()
    }

    /// The option that turns the request down to say something instead
    /// ("No, and tell Codex what to do differently"), if there is one.
    pub fn tell_instead(&self) -> Option<usize> {
        self.options.iter().position(|o| { let t = o.text.to_lowercase(); t.contains("tell") && (t.contains("differently") || t.contains("instead")) })
    }

    /// The keys that move the terminal's highlight to option `index` and pick it.
    pub fn keys_choosing(&self, index: usize) -> Vec<String> {
        let delta = index as i64 - self.highlighted.unwrap_or(0) as i64;
        let mut keys = vec![if delta > 0 { "down".to_string() } else { "up".to_string() }; delta.unsigned_abs() as usize];
        keys.push("enter".into());
        keys
    }
}

/// The lines of an agent's terminal that say something, without Claude
/// Code's own UI around its input box: status lines (✻ Worked…, ※ recap and
/// their wrapped continuation), the input line and the mode footer. Numbered
/// options stay, even the highlighted one (❯ 1. Yes).
pub fn meaningful_lines(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut skipping = false;
    // A menu's ❯ (an unnumbered option, as in "Do you trust this folder?")
    // stays: it is above the menu's footer, unlike the input prompt.
    let rows: Vec<&str> = text.split('\n').collect();
    let menu_end = rows.iter().rposition(|l| is_menu_footer(l.trim()));
    // Codex's prompt is the last › line below any menu (the ones above are
    // past messages), with its model and folder on the line under it.
    let codex_prompt = rows.iter().rposition(|l| l.trim().starts_with('›')).filter(|&i| menu_end.is_none_or(|end| i > end) && option_prefix(rows[i].trim()).is_none());
    let codex_status = codex_prompt.and_then(|p| (p + 1..rows.len()).find(|&i| !rows[i].trim().is_empty())).filter(|&i| rows[i].contains(" · "));
    for (index, raw) in rows.iter().enumerate() {
        if Some(index) == codex_prompt || Some(index) == codex_status { continue; }
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.chars().all(|c| "─━═╌┄-".contains(c)) { skipping = false; continue; }
        if skipping && line.chars().next().map(char::is_whitespace).unwrap_or(false) { continue; }
        skipping = false;
        let option = option_prefix(trimmed).is_some();
        let status = ["✻", "✳", "✢", "✽", "✶", "※"].iter().any(|s| trimmed.starts_with(s));
        let footer = ["⏵", "▶▶", "►", "⏸"].iter().any(|s| trimmed.starts_with(s))
            || ["? for shortcuts", "shift+tab to cycle", "esc to interrupt", "← for agents"].iter().any(|s| trimmed.contains(s));
        if status { skipping = true; continue; }
        let in_menu = menu_end.map(|end| index < end).unwrap_or(false);
        if footer || (trimmed.starts_with('❯') && !option && !in_menu) { continue; }
        out.push(line.to_string());
    }
    out
}

/// Join the lines a terminal hard-wrapped back into paragraphs, so the panel
/// can wrap them to its own width: a line that filled the terminal's width
/// continues on the next one unless that one starts a new block (a bullet,
/// a tool line, an option, a heading…).
pub fn paragraphs(lines: &[String]) -> Vec<String> {
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let mut out: Vec<String> = vec![];
    let mut open = false; // the last paragraph may continue
    for line in lines {
        let full = widest >= 40 && line.chars().count() >= widest.saturating_sub(12);
        if open && !starts_block(line) {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        } else {
            out.push(line.trim_end().to_string());
        }
        open = full;
    }
    out
}

/// Whether a terminal line starts a block of its own rather than continuing
/// the one above: empty, a tool line, a bullet, a numbered option, a rule…
pub fn starts_block(line: &str) -> bool {
    let t = line.trim_start();
    t.is_empty()
        || ["●", "⎿", "❯", "›", "│", "╭", "╰", "├", "└", "⏺", "✻", "✳", "✢", "✽", "✶", "※", "✘", "✔", "- ", "* ", "• ", "> ", "#", "⏵"].iter().any(|m| t.starts_with(m))
        || option_prefix(t).is_some()
        || t.trim_end().chars().all(|c| matches!(c, '─' | '━' | '═' | '╌' | '┄' | '-'))
}

/// What a Claude Code session is doing right now, from the last 128 KB of
/// its transcript (~/.claude/projects/<project>/<session>.jsonl), if it was
/// written in the last two minutes.
pub fn current_action(session: &str) -> Option<String> {
    let Ok(dirs) = std::fs::read_dir(home_dir().join(".claude/projects")) else { return None };
    for dir in dirs.flatten() {
        let file = dir.path().join(format!("{session}.jsonl"));
        let Ok(meta) = std::fs::metadata(&file) else { continue };
        let modified = meta.modified().ok()?;
        if modified < SystemTime::now().checked_sub(Duration::from_secs(120))? { return None; }
        return file_tail(&file.to_string_lossy(), 131_072).and_then(|t| last_action(&t));
    }
    None
}

// ---- Events

/// What the dungeon listens to: panes appearing, changing, closing or
/// gaining an agent, workspaces being renamed or closed, and each agent
/// pane's status (Herdr subscribes to those one pane at a time).
pub fn herdr_subscriptions(panes: &[String]) -> Vec<Value> {
    let global = ["pane.created", "pane.updated", "pane.closed", "pane.exited", "pane.agent_detected",
                  "workspace.created", "workspace.renamed", "workspace.closed", "tab.closed"];
    let mut sorted = panes.to_vec();
    sorted.sort();
    global
        .iter()
        .map(|t| serde_json::json!({ "type": t }))
        .chain(sorted.iter().map(|p| serde_json::json!({ "type": "pane.agent_status_changed", "pane_id": p })))
        .collect()
}

/// The socket of a Herdr session, from `herdr session list --json`; for the
/// session this app was launched inside, HERDR_SOCKET_PATH.
pub fn herdr_socket(session: &str) -> Option<String> {
    if let Ok(path) = std::env::var("HERDR_SOCKET_PATH") {
        if std::env::var("HERDR_SESSION").unwrap_or_else(|_| "default".into()) == session { return Some(path); }
    }
    let data = run_herdr(&["session", "list", "--json"], "default", Duration::from_secs(3)).ok()?;
    let root: Value = serde_json::from_slice(&data).ok()?;
    root.get("sessions")?.as_array()?.iter().find(|r| str_of(r, "name") == Some(session))?.get("socket_path")?.as_str().map(str::to_string)
}

/// How an event changes Herdr's view of the agents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventEffect {
    /// Nothing the dungeon shows.
    None,
    /// Applied in place.
    Changed,
    /// A structural change best read from a fresh snapshot.
    Resync,
}

/// Apply one event from the stream to the agents as Herdr reports them
/// (before local enrichment). Status changes and pane updates are applied
/// in place; a new agent or a workspace change asks for a snapshot.
pub fn apply_herdr_event(envelope: &Value, agents: &mut Vec<Agent>, names: &HashMap<String, String>) -> EventEffect {
    let empty = Value::Object(Default::default());
    let data = envelope.get("data").unwrap_or(&empty);
    let kind = str_of(envelope, "event").or_else(|| str_of(data, "type")).unwrap_or("").replace('.', "_");
    match kind.as_str() {
        "pane_agent_status_changed" => {
            let Some(id) = str_of(data, "pane_id") else { return EventEffect::None };
            let Some(index) = agents.iter().position(|a| a.id == id) else { return EventEffect::Resync };
            let status = agent_status(data.get("agent_status"));
            if agents[index].status == status { return EventEffect::None; }
            agents[index].status = status;
            *agents = sort_agents(std::mem::take(agents));
            EventEffect::Changed
        }
        "pane_created" | "pane_updated" => {
            let Some(pane) = data.get("pane") else { return EventEffect::None };
            let Some(id) = str_of(pane, "pane_id") else { return EventEffect::None };
            let known = agents.iter().position(|a| a.id == id);
            let Some(agent) = agent_from_row(pane, names) else {
                // The agent left the pane (the shell remains).
                let Some(index) = known else { return EventEffect::None };
                agents.remove(index);
                return EventEffect::Changed;
            };
            // A pane that just gained an agent needs its status subscription: resync.
            let Some(index) = known else { return EventEffect::Resync };
            if agents[index] == agent { return EventEffect::None; }
            agents[index] = agent;
            *agents = sort_agents(std::mem::take(agents));
            EventEffect::Changed
        }
        "pane_closed" | "pane_exited" => {
            let Some(id) = str_of(data, "pane_id") else { return EventEffect::None };
            let Some(index) = agents.iter().position(|a| a.id == id) else { return EventEffect::None };
            agents.remove(index);
            EventEffect::Changed
        }
        "pane_agent_detected" | "workspace_created" | "workspace_renamed" | "workspace_closed" | "tab_closed" => EventEffect::Resync,
        _ => EventEffect::None,
    }
}
