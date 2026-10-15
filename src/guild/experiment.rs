//! Design hypotheses and the experiments that test them: a hypothesis
//! names a problem at one of the three levels and what should change;
//! an experiment builds a variant of the game per proposed change (a
//! race whose runners are compared by what they make the game do, not
//! by their code), playtests each variant, and leaves the analysis and
//! the human's decision on record. Everything here is durable state the
//! agents only read or fill in.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DesignLevel { Whole, #[default] Loop, Part }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisStatus { #[default] Proposed, Experimenting, Supported, Rejected, Inconclusive }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Hypothesis {
    pub id: String,
    pub project_id: String,
    /// The design question this hypothesis tries to answer, when there is one.
    #[serde(default)]
    pub question_id: Option<String>,
    /// What is wrong, as observed.
    pub problem: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub level: DesignLevel,
    /// What we believe will fix it, and why.
    pub hypothesis: String,
    #[serde(default)]
    pub expected_effect: String,
    #[serde(default)]
    pub proposed_change: String,
    /// What to measure to know.
    #[serde(default)]
    pub measurements: Vec<String>,
    #[serde(default)]
    pub related_systems: Vec<String>,
    #[serde(default)]
    pub status: HypothesisStatus,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus { #[default] Draft, Building, Playtesting, Analysing, Decided, Accepted, Rejected, Failed }

/// One variant of the game: the parameters it changes and what to tell
/// the member who builds it. `control` is the game as it is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Variant {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    #[serde(default)]
    pub instructions: String,
    /// The task that builds it and the candidate that is its build.
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub attempt_id: Option<String>,
}

/// Which players play each build, how many times, with which seeds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaytestPlan {
    #[serde(default = "default_profiles")]
    pub profiles: Vec<String>,
    #[serde(default = "default_runs")]
    pub runs_per_profile: u32,
    #[serde(default)]
    pub seeds: Vec<u64>,
}

fn default_profiles() -> Vec<String> { vec!["novice".into(), "greedy".into(), "optimizer".into()] }
fn default_runs() -> u32 { 3 }

impl Default for PlaytestPlan {
    fn default() -> Self { PlaytestPlan { profiles: default_profiles(), runs_per_profile: 3, seeds: vec![] } }
}

/// What the analyst concluded, against the hypothesis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ExperimentResult {
    /// supports, rejects or inconclusive
    pub verdict: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub unexpected: Vec<String>,
    #[serde(default)]
    pub recommended_variant: Option<String>,
    #[serde(default)]
    pub recommended_next: String,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub analyst_attempt_id: Option<String>,
    /// Per variant, per profile: the derived metrics the analyst read.
    #[serde(default)]
    pub metrics: Value,
    #[serde(default)]
    pub created_at: String,
}

/// The human's word on the experiment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ExperimentDecision {
    /// accept, reject or iterate
    pub action: String,
    #[serde(default)]
    pub variant_id: Option<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Experiment {
    pub id: String,
    pub project_id: String,
    pub hypothesis_id: String,
    pub title: String,
    #[serde(default)]
    pub status: ExperimentStatus,
    #[serde(default)]
    pub quest_id: Option<String>,
    #[serde(default)]
    pub variants: Vec<Variant>,
    #[serde(default)]
    pub plan: PlaytestPlan,
    #[serde(default)]
    pub analysis_task_id: Option<String>,
    #[serde(default)]
    pub result: Option<ExperimentResult>,
    #[serde(default)]
    pub decision: Option<ExperimentDecision>,
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaytestKind { #[default] Simulation, AiPlayer, Human }

/// One play of one build by one player, with what it recorded.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Playtest {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub experiment_id: Option<String>,
    #[serde(default)]
    pub variant_id: Option<String>,
    /// The candidate whose worktree was played.
    #[serde(default)]
    pub attempt_id: Option<String>,
    #[serde(default)]
    pub kind: PlaytestKind,
    pub profile: String,
    #[serde(default)]
    pub seed: Option<u64>,
    /// queued, running, finished, failed
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub duration_ms: Option<i64>,
    /// The raw `game.event` lines, kept whole.
    #[serde(default)]
    pub events_artifact_id: Option<String>,
    #[serde(default)]
    pub event_count: u32,
    /// Derived from the raw events: counts per event and means of numeric fields.
    #[serde(default)]
    pub metrics: Value,
    #[serde(default)]
    pub observations: String,
    #[serde(default)]
    pub failure: Option<String>,
    #[serde(default)]
    pub created_at: String,
    /// A human session: who played (an anonymous id is fine).
    #[serde(default)]
    pub player: Option<String>,
    /// The commit that was played and the design version it was played against.
    #[serde(default)]
    pub build_sha: Option<String>,
    #[serde(default)]
    pub design_version: Option<u32>,
    /// Where the game appends its events while a human plays.
    #[serde(default)]
    pub telemetry_path: Option<String>,
    /// The checkout the session plays in, when the guild made one for it.
    #[serde(default)]
    pub play_dir: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

/// What a human noticed while playing, with where it happened. Kept as
/// evidence; promoted, it becomes a design question that points back here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Observation {
    /// OBS-001, OBS-002… per project.
    pub id: String,
    pub project_id: String,
    /// confusing, frustrating, interesting, great_moment, bug, comment
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub playtest_id: Option<String>,
    #[serde(default)]
    pub build_sha: Option<String>,
    #[serde(default)]
    pub design_version: Option<u32>,
    #[serde(default)]
    pub experiment_id: Option<String>,
    #[serde(default)]
    pub variant_id: Option<String>,
    /// The run and round the game was in, read from its telemetry.
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub round: Option<u64>,
    /// How many game events had happened: where in the run it was.
    #[serde(default)]
    pub event_index: Option<u32>,
    #[serde(default)]
    pub question_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

pub const OBSERVATION_KINDS: [&str; 6] = ["confusing", "frustrating", "interesting", "great_moment", "bug", "comment"];

/// A fact the raw telemetry shows, computed, never guessed: a dominant
/// choice, an option attribute that always wins, options nobody takes,
/// runs that almost never or always end well. Each comes with the
/// question it suggests; turning it into one is the human's call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Signal {
    /// Stable for the same finding: dominant_choice, max_attribute:<attr>, never_chosen, completion.
    pub key: String,
    pub statement: String,
    pub value: f64,
    /// How many observations the value rests on.
    pub n: u64,
    #[serde(default)]
    pub baseline: Option<f64>,
    pub suggested_question: String,
    #[serde(default)]
    pub playtests: u32,
}

/// The option a `choice.selected` names: `choice`, `word`, `option` or `action`.
fn selected_name(data: &Value) -> Option<String> {
    ["choice", "word", "option", "action"].iter().find_map(|k| data.get(*k).and_then(Value::as_str)).map(str::to_string)
}

/// Each presented option as a name and its numeric attributes: a string
/// option has its `length`; an object, its numeric fields.
fn option_attributes(option: &Value) -> Option<(String, BTreeMap<String, f64>)> {
    match option {
        Value::String(s) => Some((s.clone(), BTreeMap::from([("length".to_string(), s.chars().count() as f64)]))),
        Value::Object(o) => {
            let name = ["id", "name", "word", "choice"].iter().find_map(|k| o.get(*k).and_then(Value::as_str))?.to_string();
            let attrs = o.iter().filter_map(|(k, v)| v.as_f64().map(|n| (k.clone(), n))).collect();
            Some((name, attrs))
        }
        _ => None,
    }
}

/// The signals in the events of a set of playtests (one Vec per playtest).
pub fn signals(playtests: &[Vec<Value>]) -> Vec<Signal> {
    let mut out = vec![];
    let count = playtests.len() as u32;
    // Choices: how often each was taken, and how the taken one ranked.
    let mut taken: BTreeMap<String, u64> = BTreeMap::new();
    let mut offered: BTreeMap<String, u64> = BTreeMap::new();
    let mut total = 0u64;
    let mut max_hits: BTreeMap<String, (u64, u64, f64)> = BTreeMap::new();
    let (mut runs, mut completed) = (0u64, 0u64);
    for events in playtests {
        let mut presented: Option<Vec<(String, BTreeMap<String, f64>)>> = None;
        for e in events {
            let data = e.get("data").cloned().unwrap_or(Value::Null);
            match e["event"].as_str().unwrap_or("") {
                "run.started" => runs += 1,
                "run.completed" => completed += 1,
                "choice.presented" => {
                    presented = data.get("options").and_then(Value::as_array).map(|a| a.iter().filter_map(option_attributes).collect());
                    for (name, _) in presented.iter().flatten() { *offered.entry(name.clone()).or_default() += 1; }
                }
                "choice.selected" => {
                    let Some(name) = selected_name(&data) else { continue };
                    *taken.entry(name.clone()).or_default() += 1;
                    total += 1;
                    let Some(options) = presented.take() else { continue };
                    let Some((_, chosen)) = options.iter().find(|(n, _)| *n == name) else { continue };
                    for (attr, value) in chosen {
                        let values: Vec<f64> = options.iter().filter_map(|(_, a)| a.get(attr).copied()).collect();
                        let max = values.iter().cloned().fold(f64::MIN, f64::max);
                        let min = values.iter().cloned().fold(f64::MAX, f64::min);
                        if values.len() < 2 || (max - min).abs() < f64::EPSILON { continue; }
                        let ties = values.iter().filter(|v| (**v - max).abs() < f64::EPSILON).count() as f64;
                        let entry = max_hits.entry(attr.clone()).or_insert((0, 0, 0.0));
                        entry.1 += 1;
                        entry.2 += ties / values.len() as f64;
                        if (*value - max).abs() < f64::EPSILON { entry.0 += 1; }
                    }
                }
                _ => {}
            }
        }
    }
    if total >= 8 {
        if let Some((name, n)) = taken.iter().max_by_key(|(_, n)| **n) {
            let share = *n as f64 / total as f64;
            if share >= 0.5 {
                out.push(Signal { key: "dominant_choice".into(), statement: format!("\"{name}\" was chosen in {:.0}% of {total} choices", share * 100.0), value: share, n: total, baseline: None, suggested_question: format!("Is \"{name}\" a dominant choice?"), playtests: count });
            }
        }
    }
    for (attr, (hits, n, chance)) in &max_hits {
        if *n < 8 { continue; }
        let share = *hits as f64 / *n as f64;
        let baseline = chance / *n as f64;
        if share >= 0.7 && share >= baseline + 0.25 {
            out.push(Signal { key: format!("max_attribute:{attr}"), statement: format!("The chosen option had the highest {attr} in {:.0}% of {n} choices (about {:.0}% if chosen at random)", share * 100.0, baseline * 100.0), value: share, n: *n, baseline: Some(baseline), suggested_question: format!("Does the highest {attr} always win?"), playtests: count });
        }
    }
    let never: Vec<String> = offered.iter().filter(|(name, n)| **n >= 5 && !taken.contains_key(*name)).map(|(name, _)| name.clone()).collect();
    if !never.is_empty() && total >= 8 {
        let shown: Vec<String> = never.iter().take(6).cloned().collect();
        out.push(Signal { key: "never_chosen".into(), statement: format!("Offered at least 5 times and never chosen: {}", shown.join(", ")), value: never.len() as f64, n: total, baseline: None, suggested_question: format!("Why does nobody choose {}?", shown.join(", ")), playtests: count });
    }
    if runs >= 3 {
        let rate = completed as f64 / runs as f64;
        if rate <= 0.34 {
            out.push(Signal { key: "completion".into(), statement: format!("Only {:.0}% of {runs} runs were completed", rate * 100.0), value: rate, n: runs, baseline: None, suggested_question: "Is the game too hard at this point?".into(), playtests: count });
        } else if rate >= 0.999 && runs >= 5 {
            out.push(Signal { key: "completion".into(), statement: format!("All {runs} runs were completed"), value: rate, n: runs, baseline: None, suggested_question: "Is the game too easy at this point?".into(), playtests: count });
        }
    }
    out
}

/// The telemetry contract: one JSON object per line on the player's
/// stdout, `{"type":"game.event","runId":"…","event":"…","data":{…}}`.
/// Anything else on stdout is ignored. Returns the events in order.
pub fn parse_telemetry(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .filter(|v| v.get("type").and_then(Value::as_str) == Some("game.event") && v.get("event").and_then(Value::as_str).is_some())
        .collect()
}

/// What the raw events say in numbers: how many of each event, how many
/// runs started, completed and failed, and for every numeric field of
/// `data` under each event, its mean, min and max. `choice.selected`
/// also counts how often each `data.choice` (or `word`, `option`) was
/// taken, which is what a dominant strategy looks like.
pub fn derive_metrics(events: &[Value]) -> Value {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut nums: BTreeMap<String, (f64, f64, f64, u64)> = BTreeMap::new();
    let mut choices: BTreeMap<String, u64> = BTreeMap::new();
    for e in events {
        let name = e["event"].as_str().unwrap_or("").to_string();
        *counts.entry(name.clone()).or_default() += 1;
        if let Some(obj) = e.get("data").and_then(Value::as_object) {
            for (k, v) in obj {
                if let Some(n) = v.as_f64() {
                    let entry = nums.entry(format!("{name}.{k}")).or_insert((0.0, f64::MAX, f64::MIN, 0));
                    entry.0 += n;
                    entry.1 = entry.1.min(n);
                    entry.2 = entry.2.max(n);
                    entry.3 += 1;
                }
            }
            if name == "choice.selected" {
                if let Some(c) = ["choice", "word", "option", "action"].iter().find_map(|k| obj.get(*k).and_then(Value::as_str)) {
                    *choices.entry(c.to_string()).or_default() += 1;
                }
            }
        }
    }
    let runs_started = *counts.get("run.started").unwrap_or(&0);
    let runs_completed = *counts.get("run.completed").unwrap_or(&0);
    let runs_failed = *counts.get("run.failed").unwrap_or(&0);
    let mut fields = serde_json::Map::new();
    for (k, (sum, min, max, n)) in &nums {
        fields.insert(k.clone(), serde_json::json!({"mean": sum / *n as f64, "min": min, "max": max, "n": n}));
    }
    let total_choices: u64 = choices.values().sum();
    let (top_choice, top_share) = choices.iter().max_by_key(|(_, n)| **n).map(|(c, n)| (c.clone(), if total_choices > 0 { *n as f64 / total_choices as f64 } else { 0.0 })).unwrap_or_default();
    serde_json::json!({
        "events": counts,
        "runs": {"started": runs_started, "completed": runs_completed, "failed": runs_failed, "completion_rate": if runs_started > 0 { runs_completed as f64 / runs_started as f64 } else { 0.0 }},
        "fields": fields,
        "choices": {"distinct": choices.len(), "total": total_choices, "top": top_choice, "top_share": top_share, "counts": choices},
    })
}

/// Fold the metrics of several playtests of one build and profile into
/// one summary: means of means, sums of counts. A field's mean is over
/// the playtests that have it (an event that never happened in a playtest
/// is no zero), and `playtests` says how many those were.
pub fn aggregate(metrics: &[Value]) -> Value {
    if metrics.is_empty() { return Value::Null; }
    let mut runs = serde_json::json!({"started": 0, "completed": 0, "failed": 0});
    let mut fields: BTreeMap<String, (f64, f64, f64, u64, u64)> = BTreeMap::new();
    let mut choices: BTreeMap<String, u64> = BTreeMap::new();
    let mut events: BTreeMap<String, u64> = BTreeMap::new();
    for m in metrics {
        if let Some(c) = m["events"].as_object() {
            for (k, v) in c { *events.entry(k.clone()).or_default() += v.as_u64().unwrap_or(0); }
        }
        for k in ["started", "completed", "failed"] {
            runs[k] = Value::from(runs[k].as_u64().unwrap_or(0) + m["runs"][k].as_u64().unwrap_or(0));
        }
        if let Some(f) = m["fields"].as_object() {
            for (k, v) in f {
                let e = fields.entry(k.clone()).or_insert((0.0, f64::MAX, f64::MIN, 0, 0));
                e.0 += v["mean"].as_f64().unwrap_or(0.0);
                e.1 = e.1.min(v["min"].as_f64().unwrap_or(0.0));
                e.2 = e.2.max(v["max"].as_f64().unwrap_or(0.0));
                e.3 += 1;
                e.4 += v["n"].as_u64().unwrap_or(0);
            }
        }
        if let Some(c) = m["choices"]["counts"].as_object() {
            for (k, v) in c { *choices.entry(k.clone()).or_default() += v.as_u64().unwrap_or(0); }
        }
    }
    let started = runs["started"].as_u64().unwrap_or(0);
    let completed = runs["completed"].as_u64().unwrap_or(0);
    runs["completion_rate"] = Value::from(if started > 0 { completed as f64 / started as f64 } else { 0.0 });
    let fields: serde_json::Map<String, Value> = fields.into_iter().map(|(k, (sum, min, max, with, n))| (k, serde_json::json!({"mean": sum / with as f64, "min": min, "max": max, "playtests": with, "n": n}))).collect();
    let total: u64 = choices.values().sum();
    let (top, top_share) = choices.iter().max_by_key(|(_, c)| **c).map(|(c, k)| (c.clone(), if total > 0 { *k as f64 / total as f64 } else { 0.0 })).unwrap_or_default();
    serde_json::json!({"playtests": metrics.len(), "events": events, "runs": runs, "fields": fields, "choices": {"distinct": choices.len(), "total": total, "top": top, "top_share": top_share}})
}

/// The analyst's answer: a fenced JSON block (or a bare object) with a verdict.
pub fn parse_analysis(text: &str) -> Result<ExperimentResult, String> {
    let mut blocks: Vec<&str> = vec![];
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        let body = &after[body_start..];
        let Some(end) = body.find("```") else { break };
        blocks.push(body[..end].trim());
        rest = &body[end + 3..];
    }
    if blocks.is_empty() {
        if let (Some(s), Some(e)) = (text.find('{'), text.rfind('}')) { if e > s { blocks.push(text[s..=e].trim()); } }
    }
    for b in blocks.iter().rev() {
        if let Ok(v) = serde_json::from_str::<Value>(b) {
            if let Some(verdict) = v.get("verdict").and_then(Value::as_str) {
                let verdict = verdict.trim().to_lowercase();
                if !["supports", "rejects", "inconclusive"].contains(&verdict.as_str()) { return Err(format!("unknown verdict {verdict:?}")); }
                let list = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                return Ok(ExperimentResult {
                    verdict,
                    evidence: list("evidence"),
                    unexpected: list("unexpected"),
                    recommended_variant: v.get("recommended_variant").and_then(Value::as_str).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty() && s != "none"),
                    recommended_next: v.get("recommended_next").and_then(Value::as_str).unwrap_or("").into(),
                    confidence: v.get("confidence").and_then(Value::as_f64),
                    ..Default::default()
                });
            }
        }
    }
    Err("no JSON block with a verdict".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_lines_become_metrics_and_a_dominant_choice_shows() {
        let text = r#"noise
{"type":"game.event","runId":"r1","event":"run.started","data":{"profile":"greedy"}}
{"type":"game.event","runId":"r1","event":"choice.selected","data":{"word":"longest","length":7,"semantic":false}}
{"type":"game.event","runId":"r1","event":"choice.selected","data":{"word":"longest","length":7,"semantic":false}}
{"type":"game.event","runId":"r1","event":"choice.selected","data":{"word":"apt","length":3,"semantic":true}}
{"type":"game.event","runId":"r1","event":"run.completed","data":{"rounds":3,"score":42}}
{"type":"other","event":"x"}
"#;
        let events = parse_telemetry(text);
        assert_eq!(events.len(), 5);
        let m = derive_metrics(&events);
        assert_eq!(m["events"]["choice.selected"], 3);
        assert_eq!(m["runs"]["completion_rate"], 1.0);
        assert!((m["fields"]["choice.selected.length"]["mean"].as_f64().unwrap() - 17.0 / 3.0).abs() < 1e-9);
        assert_eq!(m["choices"]["top"], "longest");
        assert!((m["choices"]["top_share"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
        let agg = aggregate(&[m.clone(), m.clone()]);
        assert_eq!(agg["runs"]["started"], 2);
        // A field one playtest never recorded does not drag the mean towards zero.
        let won = derive_metrics(&parse_telemetry(r#"{"type":"game.event","runId":"a","event":"run.completed","data":{"edition":4}}"#));
        let lost = derive_metrics(&parse_telemetry(r#"{"type":"game.event","runId":"b","event":"run.failed","data":{"edition":2}}"#));
        let both = aggregate(&[won, lost]);
        assert_eq!((both["fields"]["run.completed.edition"]["mean"].as_f64(), both["fields"]["run.completed.edition"]["playtests"].as_u64()), (Some(4.0), Some(1)));
        assert_eq!((both["events"]["run.completed"].as_u64(), both["events"]["run.failed"].as_u64()), (Some(1), Some(1)), "event counts add up");
        assert_eq!(agg["choices"]["distinct"], 2);
        assert_eq!(agg["playtests"], 2);
    }

    #[test]
    fn signals_find_the_attribute_that_always_wins_and_options_nobody_takes() {
        let mut events = vec![];
        for r in 0..10 {
            events.push(serde_json::json!({"type": "game.event", "runId": format!("r{r}"), "event": "run.started", "data": {}}));
            events.push(serde_json::json!({"type": "game.event", "runId": format!("r{r}"), "event": "choice.presented", "data": {"options": ["ash", "glacier", "ore", "spark", "whirlwind"]}}));
            events.push(serde_json::json!({"type": "game.event", "runId": format!("r{r}"), "event": "choice.selected", "data": {"word": "whirlwind"}}));
            events.push(serde_json::json!({"type": "game.event", "runId": format!("r{r}"), "event": "run.completed", "data": {}}));
        }
        let found = signals(&[events]);
        let keys: Vec<&str> = found.iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"dominant_choice"), "{keys:?}");
        let longest = found.iter().find(|s| s.key == "max_attribute:length").expect("length always wins");
        assert_eq!(longest.value, 1.0);
        assert!((longest.baseline.unwrap() - 0.2).abs() < 1e-9);
        assert!(found.iter().any(|s| s.key == "never_chosen" && s.statement.contains("ash")));
        assert!(found.iter().any(|s| s.key == "completion" && s.suggested_question.contains("too easy")));
        // Too few choices say nothing.
        assert!(signals(&[vec![]]).is_empty());
    }

    #[test]
    fn the_analysts_verdict_is_read_or_refused() {
        let r = parse_analysis("Thinking.\n```json\n{\"verdict\": \"Rejects\", \"evidence\": [\"long words still win\"], \"recommended_variant\": \"B\", \"recommended_next\": \"raise it more\", \"confidence\": 0.7}\n```").unwrap();
        assert_eq!(r.verdict, "rejects");
        assert_eq!(r.recommended_variant.as_deref(), Some("b"));
        assert_eq!(r.evidence.len(), 1);
        assert!(parse_analysis("{\"verdict\": \"maybe\"}").is_err());
        assert!(parse_analysis("nothing").is_err());
    }
}
