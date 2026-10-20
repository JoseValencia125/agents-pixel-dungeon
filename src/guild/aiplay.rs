//! AI players: a guild member, on any harness, plays the real game one
//! move at a time through the game's agent protocol, and the exploit
//! hunter files what it breaks as durable findings that carry what it
//! takes to reproduce them (the build, the seed and every move), so the
//! guild can replay them and say whether they hold.
//!
//! The agent protocol, on the game's stdio:
//! - the game prints `{"type":"agent.observation","turn":N,"text":"…",
//!   "state":{…},"actions":[{"id":"1","label":"…"},…]}` when it waits for
//!   a move, and reads one line `{"action":"<id>"}` on stdin;
//! - its telemetry goes, as for a human, to the file `GUILD_TELEMETRY`
//!   names (`game.event` lines on stdout are read too);
//! - it exits when the run ends, or when stdin closes.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

/// What the exploit hunter broke, with how to break it again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Finding {
    /// F-001, F-002… per project.
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// What it does to the game: the experience it ruins, the system it bypasses.
    #[serde(default)]
    pub impact: String,
    /// low, medium, high, critical
    #[serde(default)]
    pub severity: String,
    /// The hunter's own account of how to do it.
    #[serde(default)]
    pub steps: String,
    #[serde(default)]
    pub playtest_id: Option<String>,
    #[serde(default)]
    pub member_id: Option<String>,
    #[serde(default)]
    pub build_sha: Option<String>,
    #[serde(default)]
    pub design_version: Option<u32>,
    #[serde(default)]
    pub experiment_id: Option<String>,
    #[serde(default)]
    pub variant_id: Option<String>,
    /// The replay: same build, same seed, these moves in order.
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub actions: Vec<String>,
    /// How many game events had happened when it was reported; a replay
    /// reproduces it when its first `event_index` events are the same.
    #[serde(default)]
    pub event_index: u32,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub round: Option<u64>,
    /// unverified, reproduced, not_reproduced, fixed, wont_fix
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub verified_at: Option<String>,
    #[serde(default)]
    pub verification: Option<String>,
    #[serde(default)]
    pub question_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

pub const FINDING_STATUSES: [&str; 5] = ["unverified", "reproduced", "not_reproduced", "fixed", "wont_fix"];
pub const SEVERITIES: [&str; 4] = ["low", "medium", "high", "critical"];

/// The personas the guild knows how to brief; any other text is used as
/// the persona's own description.
pub const PERSONAS: [&str; 5] = ["player", "novice", "optimizer", "explorer", "exploit_hunter"];

/// One move the player decided, with why, and a finding when it saw one.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Move {
    pub action: String,
    pub why: String,
    pub finding: Option<Value>,
}

/// The briefing at the start of a session: who to play as, how to answer,
/// and what the design wants the player to live (so the hunter knows what
/// breaking it means).
pub fn briefing(persona: &str, design: Option<&crate::design::GameDesign>) -> String {
    let who = match persona {
        "player" => "a player who wants to win and plays the way a thoughtful person would".to_string(),
        "novice" => "a first-time player: you read the screen, you do not know the hidden numbers, you pick what looks good".to_string(),
        "optimizer" => "an optimizer: you look for the strongest move every turn and exploit any pattern you notice".to_string(),
        "explorer" => "an explorer: you try options you have not tried yet to see what they do, while still trying to win".to_string(),
        "exploit_hunter" => "the guild's exploit hunter: you try to break the design".to_string(),
        other => other.to_string(),
    };
    let mut out = format!("You are playing a game for the guild's playtests, as {who}. You do not edit files or run commands: you only choose moves.\n\n");
    if persona == "exploit_hunter" {
        out.push_str("Look for: a choice that always wins, a safe dominant strategy, a free or infinite resource, a degenerate loop, options that are never worth taking, rules you can bypass, content you cannot reach. When the game's events give you reasonable evidence of one, report it with a \"finding\" in your answer and keep playing. You do not need certainty: the guild replays every finding (same seed, same moves) to check it holds. Report each problem once.\n\n");
    }
    if let Some(d) = design {
        let e = &d.experience;
        if !e.fantasy.trim().is_empty() { out.push_str(&format!("The experience the design wants: {}\n", e.fantasy.trim())); }
        if !e.desired_behaviors.is_empty() { out.push_str(&format!("Players should: {}\n", e.desired_behaviors.join("; "))); }
        if !e.avoid.is_empty() { out.push_str(&format!("The design wants to avoid: {}\n", e.avoid.join("; "))); }
        out.push('\n');
    }
    out.push_str("Each turn you get the game's screen and the moves you can make. Answer with one fenced ```json block and nothing after it: {\"action\": \"<id of a move>\", \"why\": \"one sentence: what you saw and what tempted you\"");
    if persona == "exploit_hunter" {
        out.push_str(", \"finding\": {\"title\": \"…\", \"impact\": \"what it breaks\", \"severity\": \"low|medium|high|critical\", \"steps\": \"how to do it\"} (only when you found something)");
    }
    out.push_str("}\n");
    out
}

/// The game events since the last move, one line each, so the player
/// sees what its move did (damage, rewards, deaths) in any game.
pub fn happened(events: &[Value]) -> String {
    let lines: Vec<String> = events.iter().map(|e| {
        let data = e.get("data").filter(|d| d.as_object().map(|o| !o.is_empty()).unwrap_or(false)).map(|d| format!(" {d}")).unwrap_or_default();
        format!("- {}{data}", e["event"].as_str().unwrap_or("?"))
    }).collect();
    let skipped = lines.len().saturating_sub(20);
    let mut out = String::new();
    if skipped > 0 { out.push_str(&format!("- … {skipped} earlier events\n")); }
    for l in &lines[skipped..] { out.push_str(l); out.push('\n'); }
    out
}

/// The prompt for one turn: what happened since the last move, the
/// screen and the moves.
pub fn turn_prompt(observation: &Value, since_last: &[Value]) -> String {
    let turn = observation["turn"].as_u64().map(|t| format!(" {t}")).unwrap_or_default();
    let mut out = String::new();
    if !since_last.is_empty() { out.push_str(&format!("## What happened\n\n{}\n", happened(since_last))); }
    out.push_str(&format!("## Your move{turn}\n\n"));
    if let Some(text) = observation["text"].as_str() { out.push_str(text.trim_end()); out.push_str("\n\n"); }
    if observation.get("state").map(|s| !s.is_null()).unwrap_or(false) { out.push_str(&format!("State: {}\n\n", observation["state"])); }
    out.push_str("Moves:\n");
    for a in actions_of(observation) { out.push_str(&format!("- {}: {}\n", a.0, a.1)); }
    out
}

/// The moves an observation offers, as (id, label).
pub fn actions_of(observation: &Value) -> Vec<(String, String)> {
    observation["actions"].as_array().map(|a| a.iter().filter_map(|x| {
        let id = x.get("id").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))?;
        Some((id.clone(), x.get("label").and_then(Value::as_str).unwrap_or(&id).to_string()))
    }).collect()).unwrap_or_default()
}

/// The move in a player's answer: its last JSON block (or bare object)
/// with an `action` that the observation offers. An action given by its
/// label instead of its id is accepted too.
pub fn parse_move(text: &str, actions: &[(String, String)]) -> Result<Move, String> {
    let mut blocks: Vec<&str> = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let body = &after[after.find('\n').map(|i| i + 1).unwrap_or(0)..];
        let Some(end) = body.find("```") else { break };
        blocks.push(body[..end].trim());
        rest = &body[end + 3..];
    }
    if blocks.is_empty() {
        if let (Some(s), Some(e)) = (text.find('{'), text.rfind('}')) { if e > s { blocks.push(&text[s..=e]); } }
    }
    for b in blocks.iter().rev() {
        let Ok(v) = serde_json::from_str::<Value>(b) else { continue };
        let Some(raw) = v.get("action").map(|a| a.as_str().map(str::to_string).unwrap_or_else(|| a.to_string())) else { continue };
        let raw = raw.trim().to_string();
        let action = actions.iter().find(|(id, _)| *id == raw).or_else(|| actions.iter().find(|(_, label)| label.eq_ignore_ascii_case(&raw) || label.split_whitespace().next().map(|w| w.eq_ignore_ascii_case(&raw)).unwrap_or(false)));
        let Some((id, _)) = action else { return Err(format!("{raw:?} is not one of the moves ({})", actions.iter().map(|a| a.0.as_str()).collect::<Vec<_>>().join(", "))) };
        let finding = v.get("finding").filter(|f| f.get("title").and_then(Value::as_str).map(|t| !t.trim().is_empty()).unwrap_or(false)).cloned();
        return Ok(Move { action: id.clone(), why: v.get("why").and_then(Value::as_str).unwrap_or("").trim().to_string(), finding });
    }
    Err("no JSON block with an action".into())
}

/// A reflection's prose, and the findings in its trailing ```json block
/// (`{"findings": [...]}`), if it has one.
pub fn split_reflection(text: &str) -> (String, Vec<Value>) {
    let Some(start) = text.rfind("```json").or_else(|| text.rfind("```")) else { return (text.trim().to_string(), vec![]) };
    let after = &text[start + 3..];
    let body = &after[after.find('\n').map(|i| i + 1).unwrap_or(after.len())..];
    let Some(end) = body.find("```") else { return (text.trim().to_string(), vec![]) };
    let Ok(v) = serde_json::from_str::<Value>(body[..end].trim()) else { return (text.trim().to_string(), vec![]) };
    let findings = v["findings"].as_array().map(|a| a.iter().filter(|f| f.get("title").and_then(Value::as_str).map(|t| !t.trim().is_empty()).unwrap_or(false)).cloned().collect()).unwrap_or_default();
    (text[..start].trim().to_string(), findings)
}

/// One turn as it was played: kept as the session's decisions artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TurnRecord {
    pub turn: u32,
    pub text: String,
    pub action: String,
    pub label: String,
    #[serde(default)]
    pub why: String,
    /// Events the game had written when the move was asked for.
    pub event_index: u32,
    #[serde(default)]
    pub finding: Option<Value>,
    /// The answer could not be read and the first move was played instead.
    #[serde(default)]
    pub fallback: bool,
}

pub struct GameOutcome {
    pub turns: Vec<TurnRecord>,
    pub exit_code: Option<i32>,
    /// The game ended by itself (not stopped, not out of turns).
    pub ended: bool,
    pub failure: Option<String>,
    pub stdout_events: String,
}

/// Run the game in `cwd` and play it with `decide`, which gets each
/// observation and how many game events exist so far. `stop` is asked
/// between turns; `max_turns` bounds the session.
pub fn play_game(cwd: &Path, argv: &[String], env: &[(String, String)], telemetry: &Path, max_turns: u32, turn_timeout: Duration, mut decide: impl FnMut(&Value, u32) -> Result<Move, String>, stop: impl Fn() -> bool) -> GameOutcome {
    let mut outcome = GameOutcome { turns: vec![], exit_code: None, ended: false, failure: None, stdout_events: String::new() };
    let Some((program, args)) = argv.split_first() else { outcome.failure = Some("empty argv".into()); return outcome };
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    for (k, v) in env { cmd.env(k, v); }
    let mut child = match cmd.spawn() { Ok(c) => c, Err(e) => { outcome.failure = Some(format!("{program}: {e}")); return outcome } };
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = channel::<String>();
    std::thread::spawn(move || { for line in BufReader::new(stdout).lines().map_while(Result::ok) { if tx.send(line).is_err() { break; } } });
    let events_now = |extra: &str| -> u32 {
        let file = std::fs::read_to_string(telemetry).unwrap_or_default();
        (crate::experiment::parse_telemetry(&file).len() + crate::experiment::parse_telemetry(extra).len()) as u32
    };
    let mut last = Instant::now();
    loop {
        if stop() { outcome.failure = Some("stopped".into()); break; }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                last = Instant::now();
                let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
                match v["type"].as_str() {
                    Some("game.event") => { outcome.stdout_events.push_str(line.trim()); outcome.stdout_events.push('\n'); }
                    Some("agent.observation") => {
                        if outcome.turns.len() as u32 >= max_turns { outcome.failure = Some(format!("stopped after {max_turns} turns")); break; }
                        let actions = actions_of(&v);
                        if actions.is_empty() { outcome.failure = Some("an observation offered no moves".into()); break; }
                        let index = events_now(&outcome.stdout_events);
                        let (m, fallback) = match decide(&v, index) {
                            Ok(m) => (m, false),
                            Err(e) if e == "stopped" => { outcome.failure = Some("stopped".into()); break; }
                            Err(e) => (Move { action: actions[0].0.clone(), why: format!("(answer not understood: {e})"), finding: None }, true),
                        };
                        let label = actions.iter().find(|a| a.0 == m.action).map(|a| a.1.clone()).unwrap_or_default();
                        outcome.turns.push(TurnRecord { turn: outcome.turns.len() as u32 + 1, text: v["text"].as_str().unwrap_or("").to_string(), action: m.action.clone(), label, why: m.why, event_index: index, finding: m.finding, fallback });
                        let sent = stdin.as_mut().map(|s| writeln!(s, "{}", serde_json::json!({"action": m.action})).and_then(|_| s.flush()));
                        if !matches!(sent, Some(Ok(()))) { break; }
                        last = Instant::now();
                    }
                    _ => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if last.elapsed() > turn_timeout { outcome.failure = Some(format!("the game said nothing for {} s", turn_timeout.as_secs())); break; }
            }
            Err(RecvTimeoutError::Disconnected) => { outcome.ended = true; break; }
        }
    }
    drop(stdin.take());
    if !outcome.ended {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline { if let Ok(Some(_)) = child.try_wait() { break; } std::thread::sleep(Duration::from_millis(50)); }
        let _ = child.kill();
    }
    outcome.exit_code = child.wait().ok().and_then(|s| s.code());
    if outcome.ended && outcome.exit_code.map(|c| c != 0).unwrap_or(false) { outcome.failure = Some(format!("the game exited with {}", outcome.exit_code.unwrap())); }
    outcome
}

/// A finding from a move's `finding` object, before the guild numbers it.
pub fn finding_from(raw: &Value) -> Finding {
    let s = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let severity = s("severity").to_lowercase();
    Finding { title: s("title"), impact: s("impact"), steps: s("steps"), severity: if SEVERITIES.contains(&severity.as_str()) { severity } else { "medium".into() }, status: "unverified".into(), ..Default::default() }
}

/// Whether a replay shows the same game up to the finding: the first
/// `upto` events are equal.
pub fn same_prefix(original: &[Value], replay: &[Value], upto: usize) -> bool {
    upto > 0 && original.len() >= upto && replay.len() >= upto && original[..upto] == replay[..upto]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_is_read_by_id_or_label_and_refused_when_not_offered() {
        let actions = vec![("1".to_string(), "inferno (fire, 7 letters)".to_string()), ("2".to_string(), "ash (fire, 3 letters)".to_string())];
        let m = parse_move("I go long.\n```json\n{\"action\": \"1\", \"why\": \"longest hits hardest\"}\n```", &actions).unwrap();
        assert_eq!((m.action.as_str(), m.why.as_str(), m.finding.is_none()), ("1", "longest hits hardest", true));
        assert_eq!(parse_move("{\"action\": 2}", &actions).unwrap().action, "2");
        assert_eq!(parse_move("{\"action\": \"ash\"}", &actions).unwrap().action, "2");
        assert!(parse_move("{\"action\": \"7\"}", &actions).is_err());
        assert!(parse_move("no idea", &actions).is_err());
        let hunt = parse_move("```json\n{\"action\":\"1\",\"why\":\"x\",\"finding\":{\"title\":\"Longest word always wins\",\"severity\":\"HIGH\",\"impact\":\"no choice\"}}\n```", &actions).unwrap();
        let f = finding_from(hunt.finding.as_ref().unwrap());
        assert_eq!((f.title.as_str(), f.severity.as_str(), f.status.as_str()), ("Longest word always wins", "high", "unverified"));
        // An empty finding is no finding.
        assert!(parse_move("{\"action\":\"1\",\"finding\":{\"title\":\"\"}}", &actions).unwrap().finding.is_none());
    }

    #[test]
    fn a_reflection_keeps_its_prose_and_hands_over_late_findings() {
        let (prose, found) = split_reflection("It felt samey.\n\n```json\n{\"findings\": [{\"title\": \"Length beats weakness\", \"severity\": \"high\"}, {\"title\": \"\"}]}\n```");
        assert_eq!((prose.as_str(), found.len(), found[0]["title"].as_str()), ("It felt samey.", 1, Some("Length beats weakness")));
        assert_eq!(split_reflection("Just words."), ("Just words.".to_string(), vec![]));
    }

    #[test]
    fn the_briefing_tells_the_hunter_how_to_report() {
        let hunter = briefing("exploit_hunter", None);
        assert!(hunter.contains("\"finding\"") && hunter.contains("break the design"));
        let novice = briefing("novice", None);
        assert!(!novice.contains("\"finding\"") && novice.contains("first-time player"));
        assert!(briefing("a speedrunner who skips everything", None).contains("a speedrunner"));
        let obs = serde_json::json!({"turn": 3, "text": "Enemy HP 40", "actions": [{"id": "1", "label": "ember"}, {"id": 2, "label": "gale"}]});
        let p = turn_prompt(&obs, &[serde_json::json!({"event": "damage.dealt", "data": {"amount": 27.9}})]);
        assert!(p.contains("## What happened") && p.contains("- damage.dealt {\"amount\":27.9}"), "{p}");
        assert!(p.contains("## Your move 3") && p.contains("- 1: ember") && p.contains("- 2: gale"), "{p}");
    }
}
