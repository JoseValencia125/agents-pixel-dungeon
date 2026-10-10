//! How much of each harness's plan is left: Claude's straight from
//! Anthropic with Claude Code's own login (its cached /usage in
//! ~/.claude.json when that fails), the rate limits Codex writes into its
//! session logs, and the credits Kiro's sessions record per turn, summed
//! over the month.

use crate::herdr::{file_tail, unix_now};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

/// One usage window: how much of it is used (0–100) and when it resets.
#[derive(Clone, Debug, PartialEq)]
pub struct Meter {
    /// Short name of the window: "5h", "sem" (the week), "mes".
    pub window: &'static str,
    pub used: f32,
    pub resets: Option<i64>,
}

impl Meter {
    /// What is left, 0–100: the hero's HP. A window that already reset is full.
    pub fn hp(&self, now: i64) -> f32 {
        if self.resets.map(|r| r <= now).unwrap_or(false) { 100.0 } else { (100.0 - self.used).clamp(0.0, 100.0) }
    }
}

/// A harness's meters: the session's (5h, its mana) and the week's or
/// month's (its HP).
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessUsage {
    pub harness: &'static str,
    pub meters: Vec<Meter>,
    /// Kiro only: credits spent this month, and the plan's total if known.
    pub credits: Option<(f64, Option<u32>)>,
}

/// Kiro's plans and their monthly credits.
pub const KIRO_PLANS: [(&str, u32); 4] = [("Free", 50), ("Pro", 1000), ("Pro+", 2000), ("Power", 10000)];

/// Read every harness's usage on a thread; the receiver gets them once.
pub fn fetch(home: PathBuf, kiro_plan: u32) -> Receiver<Vec<HarnessUsage>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let found = [claude(&home), codex(&home), kiro(&home, kiro_plan, chrono::Local::now())];
        let _ = tx.send(found.into_iter().flatten().collect());
    });
    rx
}

// ---- Claude Code

pub fn claude(home: &Path) -> Option<HarnessUsage> {
    if let Some(usage) = claude_live(home).as_ref().and_then(claude_from_utilization) { return Some(usage); }
    let text = std::fs::read_to_string(home.join(".claude.json")).ok()?;
    claude_from(&serde_json::from_str(&text).ok()?)
}

/// Claude's plan usage from Anthropic (what /usage shows), asked at most
/// every two minutes with Claude Code's own OAuth token: from the macOS
/// keychain ("Claude Code-credentials") or ~/.claude/.credentials.json. An
/// expired token is left alone (Claude Code refreshes it itself), and the
/// token goes to curl on stdin, never on its command line.
fn claude_live(home: &Path) -> Option<Value> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    static LAST: std::sync::Mutex<Option<(std::time::Instant, Option<Value>)>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().ok()?;
    if let Some((when, value)) = last.as_ref() { if when.elapsed().as_secs() < 120 { return value.clone(); } }
    let fetch = || -> Option<Value> {
        let keychain = Command::new("security").args(["find-generic-password", "-s", "Claude Code-credentials", "-w"]).stderr(Stdio::null()).output().ok()
            .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).to_string());
        let credentials: Value = serde_json::from_str(&keychain.or_else(|| std::fs::read_to_string(home.join(".claude").join(".credentials.json")).ok())?).ok()?;
        let oauth = credentials.get("claudeAiOauth")?;
        let token = oauth.get("accessToken")?.as_str()?;
        if oauth.get("expiresAt").and_then(Value::as_i64).map(|ms| ms / 1000 < unix_now() + 60).unwrap_or(false) { return None; }
        let mut curl = Command::new("curl").args(["-s", "-f", "-m", "10", "-H", "@-", "https://api.anthropic.com/api/oauth/usage"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        curl.stdin.take()?.write_all(format!("Authorization: Bearer {token}\nanthropic-beta: oauth-2025-04-20\n").as_bytes()).ok()?;
        let output = curl.wait_with_output().ok()?;
        serde_json::from_slice(&output.stdout).ok()
    };
    let value = fetch();
    *last = Some((std::time::Instant::now(), value.clone()));
    value
}

/// From ~/.claude.json's `cachedUsageUtilization` (Claude Code's last /usage).
pub fn claude_from(config: &Value) -> Option<HarnessUsage> {
    claude_from_utilization(config.get("cachedUsageUtilization")?.get("utilization")?)
}

/// From a /usage answer: its `limits` rows (the session's, then the week's
/// general one, `weekly_all`; a per-model weekly limit only when there is no
/// general one), or the older `five_hour` and `seven_day` fields.
pub fn claude_from_utilization(cached: &Value) -> Option<HarnessUsage> {
    let meter = |window, row: &Value, key: &str| Some(Meter { window, used: row.get(key)?.as_f64()? as f32, resets: row.get("resets_at").and_then(Value::as_str).and_then(iso_seconds) });
    let mut meters = vec![];
    if let Some(limits) = cached.get("limits").and_then(Value::as_array) {
        let group = |name: &'static str, window: &'static str| -> Vec<Meter> {
            limits.iter().filter(|l| l.get("group").and_then(Value::as_str) == Some(name)).filter_map(|l| meter(window, l, "percent")).collect()
        };
        meters.extend(group("session", "5h").into_iter().take(1));
        let general = limits.iter().find(|l| l.get("kind").and_then(Value::as_str) == Some("weekly_all")).and_then(|l| meter("sem", l, "percent"));
        meters.extend(general.or_else(|| group("weekly", "sem").into_iter().next()));
    }
    if meters.is_empty() {
        meters.extend(cached.get("five_hour").and_then(|r| meter("5h", r, "utilization")));
        meters.extend(cached.get("seven_day").and_then(|r| meter("sem", r, "utilization")));
    }
    (!meters.is_empty()).then_some(HarnessUsage { harness: "claude", meters, credits: None })
}

fn iso_seconds(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|t| t.timestamp())
}

// ---- Codex

pub fn codex(home: &Path) -> Option<HarnessUsage> {
    newest_codex_logs(&home.join(".codex").join("sessions")).iter().find_map(|path| codex_from_tail(&file_tail(path.to_str()?, 262_144)?))
}

/// The newest session logs, newest first: sessions/YYYY/MM/DD/rollout-*.jsonl.
fn newest_codex_logs(root: &Path) -> Vec<PathBuf> {
    let newest_dirs = |dir: &Path| -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        dirs.sort();
        dirs.reverse();
        dirs
    };
    let mut days = vec![];
    'out: for year in newest_dirs(root) {
        for month in newest_dirs(&year) {
            for day in newest_dirs(&month) {
                days.push(day);
                if days.len() >= 2 { break 'out; }
            }
        }
    }
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = days.iter().flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path()).filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p))).collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(5).map(|(_, p)| p).collect()
}

/// The last `rate_limits` a Codex session log reports: `primary` is the
/// 5-hour window, `secondary` the week.
pub fn codex_from_tail(tail: &str) -> Option<HarnessUsage> {
    tail.lines().rev().filter(|l| l.contains("\"rate_limits\"")).find_map(|line| {
        let row: Value = serde_json::from_str(line).ok()?;
        let limits = find_key(&row, "rate_limits")?;
        let meter = |key: &str| {
            let w = limits.get(key)?;
            let minutes = w.get("window_minutes").and_then(Value::as_i64).unwrap_or(0);
            Some(Meter { window: if minutes >= 7 * 24 * 60 { "sem" } else { "5h" }, used: w.get("used_percent")?.as_f64()? as f32, resets: w.get("resets_at").and_then(Value::as_i64) })
        };
        let meters: Vec<Meter> = [meter("primary"), meter("secondary")].into_iter().flatten().collect();
        (!meters.is_empty()).then_some(HarnessUsage { harness: "codex", meters, credits: None })
    })
}

fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map.get(key).filter(|v| v.is_object()).or_else(|| map.values().find_map(|v| find_key(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

// ---- Kiro

/// Credits Kiro's sessions spent this month (local time), against the plan
/// the user picked (0: not picked yet, no HP to show).
pub fn kiro(home: &Path, plan: u32, now: chrono::DateTime<chrono::Local>) -> Option<HarnessUsage> {
    let dir = home.join(".kiro").join("sessions").join("cli");
    let entries = std::fs::read_dir(&dir).ok()?;
    let month = now.format("%Y-%m").to_string();
    let spent: f64 = entries.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
        .map(|session| kiro_month_credits(&session, &month))
        .sum();
    let start_next = next_month_start(now);
    let meters = if plan > 0 { vec![Meter { window: "mes", used: (spent / plan as f64 * 100.0) as f32, resets: start_next }] } else { vec![] };
    Some(HarnessUsage { harness: "kiro", meters, credits: Some((spent, (plan > 0).then_some(plan))) })
}

/// A session's credits whose turns ended in `month` ("2026-10", local time).
pub fn kiro_month_credits(session: &Value, month: &str) -> f64 {
    let turns = session.pointer("/session_state/conversation_metadata/user_turn_metadatas").and_then(Value::as_array);
    turns.into_iter().flatten().filter(|turn| {
        turn.get("end_timestamp").and_then(Value::as_str).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m").to_string() == month).unwrap_or(false)
    }).flat_map(|turn| turn.get("metering_usage").and_then(Value::as_array).into_iter().flatten())
        .filter_map(|m| m.get("value").and_then(Value::as_f64)).sum()
}

fn next_month_start(now: chrono::DateTime<chrono::Local>) -> Option<i64> {
    use chrono::{Datelike, TimeZone};
    let (year, month) = if now.month() == 12 { (now.year() + 1, 1) } else { (now.year(), now.month() + 1) };
    chrono::Local.with_ymd_and_hms(year, month, 1, 0, 0, 0).single().map(|t| t.timestamp())
}

/// When a window refills, in local time: the hour if it is today, else the
/// weekday, day and hour ("sáb 11 12:00"); `hour` false leaves the hour out.
pub fn reset_label(resets: i64, now: chrono::DateTime<chrono::Local>, hour: bool) -> Option<String> {
    use chrono::{Datelike, TimeZone};
    let when = chrono::Local.timestamp_opt(resets, 0).single()?;
    let time = when.format("%H:%M").to_string();
    if when.date_naive() == now.date_naive() { return Some(time); }
    let day = crate::l10n::tr(["lun", "mar", "mié", "jue", "vie", "sáb", "dom"][when.weekday().num_days_from_monday() as usize]);
    Some(if hour { format!("{day} {} {time}", when.day()) } else { format!("{day} {}", when.day()) })
}

/// Seconds until a meter resets, if it has not yet.
pub fn left(meter: &Meter) -> Option<i64> {
    meter.resets.map(|r| r - unix_now()).filter(|s| *s > 0)
}
