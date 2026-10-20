//! The living game design: one durable object that starts as a game
//! concept (Sellers' concept and pitch template: title, concept
//! statement, genre, audience, selling points, player experience, style,
//! world fiction, monetization, platform and scope, core loops,
//! objectives, systems, interactivity) and grows, system by system,
//! into detailed design documentation as prototypes, experiments and
//! playtests settle what was uncertain.
//!
//! Two layers never mix: the **current design** here is what is true
//! now, in the present tense, with numbers where numbers are known; the
//! **design history** (questions, hypotheses, experiments, playtests,
//! decisions) says why it became so and lives in its own entities that
//! the design only points at. Agents read the current design at the end
//! of every prompt; nobody fills a gap by inventing. A gap is a question.

use crate::domain::now;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How settled a section or system is. Undefined means nobody wrote it;
/// Concept, that the intent exists; Hypothesis, that a change is proposed
/// and untested; Prototyping, that it is being tried; Validated, that
/// evidence supports the current design; Production, that it is precise
/// enough to build from; Revised, that it was validated and is changing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Maturity { #[default] Undefined, Concept, Hypothesis, Prototyping, Validated, Production, Revised }

impl Maturity {
    pub const ALL: [Maturity; 7] = [Maturity::Undefined, Maturity::Concept, Maturity::Hypothesis, Maturity::Prototyping, Maturity::Validated, Maturity::Production, Maturity::Revised];
    /// How much of the way to production-ready, for the maturity bar.
    pub fn score(self) -> f32 {
        match self { Maturity::Undefined => 0.0, Maturity::Concept => 0.25, Maturity::Hypothesis => 0.35, Maturity::Prototyping => 0.5, Maturity::Validated => 0.8, Maturity::Production => 1.0, Maturity::Revised => 0.7 }
    }
    pub fn as_str(self) -> &'static str {
        match self { Maturity::Undefined => "undefined", Maturity::Concept => "concept", Maturity::Hypothesis => "hypothesis", Maturity::Prototyping => "prototyping", Maturity::Validated => "validated", Maturity::Production => "production", Maturity::Revised => "revised" }
    }
    pub fn parse(s: &str) -> Option<Maturity> { Maturity::ALL.iter().copied().find(|m| m.as_str() == s.trim().to_lowercase()) }
}

/// The high-level concept: the game in a tweet and why it is worth making.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Concept {
    /// One or two sentences: what the game is and why it is fun.
    #[serde(default)]
    pub statement: String,
    #[serde(default)]
    pub genre: String,
    /// Motivations and interests, age, rating.
    #[serde(default)]
    pub audience: String,
    /// What makes the game stand out; nothing else matters if these are weak.
    #[serde(default)]
    pub usps: Vec<String>,
    /// The world and any narrative, as presented to the player.
    #[serde(default)]
    pub world_fiction: String,
    #[serde(default)]
    pub monetization: String,
    /// Platform, technology, team and time, major risks.
    #[serde(default)]
    pub platform_scope: String,
    /// Similar games from the player's point of view, and what sets this apart.
    #[serde(default)]
    pub competition: String,
}

/// What the game will not compromise on, falsifiable: a design choice can
/// be held against it and pass or fail.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pillar {
    pub name: String,
    #[serde(default)]
    pub statement: String,
    /// The question a choice must answer yes to, to serve this pillar.
    #[serde(default)]
    pub design_test: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Experience {
    /// Who the player is in the fiction, in one sentence.
    #[serde(default)]
    pub fantasy: String,
    /// The setting and point of view, and how long a play lasts.
    #[serde(default)]
    pub pov: String,
    #[serde(default)]
    pub player_should_feel: Vec<String>,
    #[serde(default)]
    pub desired_behaviors: Vec<String>,
    #[serde(default)]
    pub avoid: Vec<String>,
    /// The first minutes, the struggles, the victories.
    #[serde(default)]
    pub key_moments: Vec<String>,
    /// What keeps the player for the whole of their play.
    #[serde(default)]
    pub engagement: String,
}

/// Look and feel, and what it is for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Style {
    #[serde(default)]
    pub visual: String,
    #[serde(default)]
    pub audio: String,
    /// Reference and concept art, music; never used in the game itself.
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameLoop {
    pub id: String,
    pub name: String,
    /// The cycle, in order; the last step feeds the first.
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    /// What feeds back into the loop and whether it reinforces or balances.
    #[serde(default)]
    pub feedback: String,
    /// When the loop breaks or stops being interesting.
    #[serde(default)]
    pub failure: String,
    #[serde(default)]
    pub notes: String,
}

/// A game object: an entity with attributes and rules.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub name: String,
    /// `name: type or range`, one per entry.
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

/// A system of the game (combat, progression, economy…) with its own
/// maturity: what it intends, what is known, what is not, and the
/// precise current design once there is one. Links go to history and
/// implementation; the text stays the design.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameSystem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub maturity: Maturity,
    /// The "why" behind the system, in terms of the player's experience.
    #[serde(default)]
    pub intent: String,
    #[serde(default)]
    pub known: Vec<String>,
    /// Open points: each should become a question, never a guess.
    #[serde(default)]
    pub unknown: Vec<String>,
    /// How it works now, present tense, numbers where numbers are known.
    #[serde(default)]
    pub current: String,
    /// Every rule with numbers, as a formula with its variables and ranges.
    #[serde(default)]
    pub formulas: Vec<String>,
    /// `name: value (safe range) — what it changes`, one per knob.
    #[serde(default)]
    pub tuning: Vec<String>,
    /// What happens at the limits: zero, the cap, two events at once.
    #[serde(default)]
    pub edge_cases: Vec<String>,
    /// Verifiable criteria the build must meet; never "feels right".
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// Systems this one needs (ids); each should name this one back.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Player-facing or internal (simulation).
    #[serde(default)]
    pub player_facing: bool,
    #[serde(default)]
    pub loops: Vec<String>,
    #[serde(default)]
    pub parts: Vec<String>,
    /// Paths in the repository: code, tests, data, assets.
    #[serde(default)]
    pub implementation: Vec<String>,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub experiments: Vec<String>,
}

/// A deeper section of the detailed documentation (world, UI, art
/// direction, objects, technical, tools, localization, ideas), present
/// only when the project has something to say in it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignSection {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub maturity: Maturity,
    #[serde(default)]
    pub body: String,
}

/// The ids and titles of the deeper sections the detailed template names.
pub const DEEP_SECTIONS: [(&str, &str); 8] = [
    ("world", "Game world"), ("ui", "User interface"), ("art", "Art direction"), ("objects", "Game objects"),
    ("technical", "Technical"), ("tools", "Tools"), ("localization", "Localization"), ("ideas", "Ideas and expansions"),
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LivingGameDesign {
    pub project_id: String,
    /// The working title.
    #[serde(default)]
    pub title: String,
    /// The game's own scrum report: what is done, in progress, blocked.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub concept: Concept,
    /// In priority order: when two conflict, the first wins.
    #[serde(default)]
    pub pillars: Vec<Pillar>,
    /// What the game deliberately is not.
    #[serde(default)]
    pub anti_pillars: Vec<String>,
    #[serde(default)]
    pub experience: Experience,
    #[serde(default)]
    pub style: Style,
    #[serde(default)]
    pub loops: Vec<GameLoop>,
    /// How the player moves through the game, short- and long-term goals.
    #[serde(default)]
    pub objectives: String,
    #[serde(default)]
    pub systems: Vec<GameSystem>,
    /// Moment-to-moment: what the player does, kinds of interactivity.
    #[serde(default)]
    pub interactivity: String,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default)]
    pub sections: Vec<DesignSection>,
    /// Maturity of the concept-level sections, set by the human.
    #[serde(default)]
    pub concept_maturity: Maturity,
    #[serde(default)]
    pub experience_maturity: Maturity,
    #[serde(default)]
    pub loops_maturity: Maturity,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
}

/// The name the rest of the guild grew up with.
pub type GameDesign = LivingGameDesign;

/// Something the design does not know yet. It is never deleted: resolved,
/// it keeps pointing at the decision that answered it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignQuestion {
    /// Q-001, Q-002… per project.
    pub id: String,
    pub project_id: String,
    pub question: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub system_id: Option<String>,
    /// open or resolved
    #[serde(default = "open")]
    pub status: String,
    /// human, guild, playtest, analyst
    #[serde(default)]
    pub raised_by: String,
    /// The observation, playtest or experiment that raised it.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub hypothesis_ids: Vec<String>,
    #[serde(default)]
    pub experiment_ids: Vec<String>,
    #[serde(default)]
    pub decision_id: Option<String>,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub resolved_at: Option<String>,
}

fn open() -> String { "open".into() }

/// Why the design became what it is: the decision, its reasons, what it
/// was based on and what it rejected. The design points at decisions;
/// decisions point at evidence.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignDecision {
    /// DEC-001, DEC-002… per project.
    pub id: String,
    pub project_id: String,
    pub decision: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub question_ids: Vec<String>,
    #[serde(default)]
    pub hypothesis_ids: Vec<String>,
    #[serde(default)]
    pub experiment_ids: Vec<String>,
    #[serde(default)]
    pub playtest_ids: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub rejected_alternatives: Vec<String>,
    #[serde(default)]
    pub system_ids: Vec<String>,
    /// human, analyst, policy
    #[serde(default)]
    pub decided_by: String,
    #[serde(default)]
    pub design_version: u32,
    #[serde(default)]
    pub created_at: String,
}

/// The branch the guild publishes a project's design to; the user merges it.
pub fn design_ref(project_id: &str) -> String { format!("refs/heads/guild/design/{project_id}") }

/// The names of the templates `template()` knows.
pub const TEMPLATES: [&str; 2] = ["videogame", "blank"];

fn strs(items: &[&str]) -> Vec<String> { items.iter().map(|s| s.to_string()).collect() }

/// A design to start from. `videogame` is a small word-combat game with
/// its concept filled in and its systems at the maturity a first
/// prototype would leave them; `blank` has the headings only.
pub fn template(project_id: &str, title: &str, name: &str) -> LivingGameDesign {
    let mut d = LivingGameDesign { project_id: project_id.into(), title: title.into(), version: 1, updated_at: now(), ..Default::default() };
    if name != "videogame" { return d }
    d.status = "Concept written; combat prototyped in a headless simulator; progression and economy still at concept level.".into();
    d.concept = Concept {
        statement: "A roguelike where you fight with words: build the right word from a hand of letters, hit the enemy where it is weak, and keep a run alive with a shrinking pool of rare letters.".into(),
        genre: "Roguelike deckbuilder with word construction".into(),
        audience: "Players of Slay the Spire and Scrabble-likes who want quick, clever runs; 12+".into(),
        usps: strs(&["semantic matches beat raw length: the right word, not the longest, wins", "a letter pool that drains and refills makes every rare letter a decision", "runs of ten minutes with a build that compounds"]),
        world_fiction: "You are a writer making a name in a city where duels are fought with words; each district has its own vocabulary and its own champion.".into(),
        monetization: "Premium purchase; no in-game purchases.".into(),
        platform_scope: "PC and tablet, 2D; a two-person team; first playable in six weeks; the biggest risk is the dictionary's quality in each language.".into(),
        competition: String::new(),
    };
    d.concept_maturity = Maturity::Concept;
    d.experience = Experience {
        fantasy: "I am a writer trying to make a name for myself, fighting with words.".into(),
        pov: "Top-down over the duel; one run lasts about ten minutes.".into(),
        player_should_feel: strs(&["creativity", "pressure", "risk", "satisfaction when the perfect word lands"]),
        desired_behaviors: strs(&["consider several words before committing", "take risks on rare letters", "adapt the strategy to the enemy"]),
        avoid: strs(&["a dominant strategy", "always choosing the longest word", "obvious decisions"]),
        key_moments: strs(&["the first time a short word beats a long one because it matched the enemy's weakness", "a run saved by spending the last rare letter", "the district champion falls to a word built over three turns"]),
        engagement: "Each run teaches a vocabulary; each build changes which words are worth it.".into(),
    };
    d.experience_maturity = Maturity::Concept;
    d.style = Style {
        visual: "Pixel art, warm paper tones for the hand, cold tones for the enemy; letters as tiles.".into(),
        audio: "Typewriter and paper for the player, low brass for the enemy.".into(),
        references: vec![],
    };
    d.loops = vec![
        GameLoop {
            id: "core".into(),
            name: "Word combat".into(),
            steps: strs(&["letters", "create word", "word properties", "effect", "enemy reaction", "board changes", "new letters"]),
            inputs: strs(&["letter pool", "enemy state", "modifiers in play"]),
            outputs: strs(&["damage", "board state", "rewards"]),
            feedback: "Rarer letters give better words but drain the pool: balancing. Rewards unlock modifiers that make riskier words viable: reinforcing.".into(),
            failure: "The longest word is always best (dominant strategy); the pool runs dry with no way to recover (dead end).".into(),
            notes: String::new(),
        },
        GameLoop {
            id: "run".into(),
            name: "The run".into(),
            steps: strs(&["choose a path", "fight", "collect reward", "pick a modifier", "stronger enemy"]),
            inputs: strs(&["rewards from fights", "player build"]),
            outputs: strs(&["build diversity", "difficulty curve"]),
            feedback: "Modifiers compound into a build: reinforcing, capped by enemy scaling.".into(),
            failure: "One modifier combination trivialises every fight.".into(),
            notes: String::new(),
        },
    ];
    d.loops_maturity = Maturity::Concept;
    d.objectives = "Short term: win the duel in front of you. Long term: clear the district, then the city. Each run is linear through districts; the build is the player's choice.".into();
    d.interactivity = "Each turn: read the hand and the enemy's weakness, build a word (short-term cognitive), weigh rare letters against the pool (long-term cognitive), play it, watch the enemy react.".into();
    d.systems = vec![
        GameSystem {
            id: "combat".into(), name: "Combat".into(), maturity: Maturity::Prototyping,
            intent: "Create tactical pressure without requiring fast reflexes: the right word, chosen under a draining pool.".into(),
            known: strs(&["turn based", "the enemy reacts after each word", "a word's power is its letter power times a length multiplier times a semantic multiplier when its category matches the weakness"]),
            unknown: strs(&["the exact length and semantic multipliers", "how fast the rare pool refills", "enemy progression across districts"]),
            current: "power = sum(letter power) × length_multiplier^(len−3) × semantic_multiplier (if category = weakness). Letter power: 3 for q z x j k v w, 1 otherwise. Rare pool: 6, refills 1 per turn.".into(),
            player_facing: true, loops: strs(&["core"]), parts: strs(&["letter", "word", "enemy"]),
            implementation: strs(&["wordfight.py", "balance.json"]), ..Default::default()
        },
        GameSystem {
            id: "progression".into(), name: "Progression".into(), maturity: Maturity::Concept,
            intent: "Make each run teach a vocabulary and each build change which words are worth playing.".into(),
            known: strs(&["districts in a fixed order", "one modifier chosen after each fight"]),
            unknown: strs(&["how many districts", "how modifiers scale"]),
            player_facing: true, loops: strs(&["run"]), parts: strs(&["modifier", "round"]), ..Default::default()
        },
        GameSystem {
            id: "economy".into(), name: "Letter economy".into(), maturity: Maturity::Concept,
            intent: "Every rare letter is a decision: spend it now or keep it for the champion.".into(),
            known: strs(&["rare letters drain a pool", "the pool refills slowly"]),
            unknown: strs(&["pool size", "refill rate", "whether rewards can refill it"]),
            player_facing: false, loops: strs(&["core", "run"]), parts: strs(&["letter"]), ..Default::default()
        },
    ];
    d.parts = vec![
        Part { id: "letter".into(), name: "Letter".into(), attributes: strs(&["char", "rarity: common | uncommon | rare", "base_power: 1..5"]), rules: vec!["rare letters refill slower".into()], notes: String::new() },
        Part { id: "word".into(), name: "Word".into(), attributes: strs(&["letters", "length", "rarity", "category: semantic field", "base_power"]), rules: strs(&["power = sum(letter power) × length multiplier × semantic multiplier", "a word fits the enemy's weakness when its category matches"]), notes: String::new() },
        Part { id: "enemy".into(), name: "Enemy".into(), attributes: strs(&["hp", "weakness: category", "reaction: attack | shield | steal letters"]), rules: vec!["reacts after every word".into()], notes: String::new() },
        Part { id: "modifier".into(), name: "Modifier".into(), attributes: strs(&["trigger", "effect", "duration"]), rules: vec!["at most three in play".into()], notes: String::new() },
        Part { id: "round".into(), name: "Round".into(), attributes: strs(&["number", "enemy", "letter pool"]), rules: vec!["ends when the enemy or the player falls".into()], notes: String::new() },
    ];
    d
}

/// What a system still lacks to be precise enough to build from: a
/// production system has its current design, its edge cases, verifiable
/// acceptance criteria and no open unknowns.
pub fn production_gaps(s: &GameSystem) -> Vec<&'static str> {
    let mut gaps = vec![];
    if s.current.trim().is_empty() { gaps.push("the current design"); }
    if s.edge_cases.is_empty() { gaps.push("edge cases"); }
    if s.acceptance.is_empty() { gaps.push("acceptance criteria"); }
    if !s.unknown.is_empty() { gaps.push("no open unknowns"); }
    gaps
}

/// Refuses a design that marks a system production before it is.
pub fn check_production(d: &LivingGameDesign) -> Result<(), String> {
    for s in d.systems.iter().filter(|s| s.maturity == Maturity::Production) {
        let gaps = production_gaps(s);
        if !gaps.is_empty() { return Err(format!("system {} cannot be production without {}", s.name, gaps.join(", "))); }
    }
    Ok(())
}

/// Every section with a maturity, for the chamber's table and the bar.
pub fn maturities(d: &LivingGameDesign) -> Vec<(String, String, Maturity)> {
    let mut out = vec![
        ("concept".to_string(), "Concept".to_string(), d.concept_maturity),
        ("experience".to_string(), "Player experience".to_string(), d.experience_maturity),
        ("loops".to_string(), "Core loops".to_string(), d.loops_maturity),
    ];
    for s in &d.systems { out.push((format!("system:{}", s.id), s.name.clone(), s.maturity)); }
    for s in &d.sections { out.push((format!("section:{}", s.id), s.title.clone(), s.maturity)); }
    out
}

/// How far the design is from production-ready, 0–100.
pub fn maturity_percent(d: &LivingGameDesign) -> u32 {
    let all = maturities(d);
    if all.is_empty() { return 0 }
    (all.iter().map(|(_, _, m)| m.score()).sum::<f32>() / all.len() as f32 * 100.0).round() as u32
}

/// The design as the agents read it at the end of their prompt: short,
/// every level present, present tense, and the open points named as
/// unknowns so nobody fills them by guessing.
pub fn prompt_context(d: &LivingGameDesign) -> String {
    let mut s = String::new();
    s.push_str("\n## Game design (current; whole → loops → parts)\n");
    if !d.concept.statement.trim().is_empty() { s.push_str(&format!("Concept: {}\n", d.concept.statement.trim())); }
    if !d.experience.fantasy.trim().is_empty() { s.push_str(&format!("Fantasy: {}\n", d.experience.fantasy.trim())); }
    if !d.pillars.is_empty() {
        let p: Vec<String> = d.pillars.iter().map(|p| if p.design_test.trim().is_empty() { format!("{} ({})", p.name.trim(), p.statement.trim()) } else { format!("{} ({}; test: {})", p.name.trim(), p.statement.trim(), p.design_test.trim()) }).collect();
        s.push_str(&format!("Pillars, first wins: {}\n", p.join("; ")));
    }
    if !d.anti_pillars.is_empty() { s.push_str(&format!("Anti-pillars (the game is not): {}\n", d.anti_pillars.join("; "))); }
    let list = |label: &str, items: &[String]| if items.is_empty() { String::new() } else { format!("{label}: {}\n", items.join("; ")) };
    s.push_str(&list("Selling points", &d.concept.usps));
    s.push_str(&list("The player should feel", &d.experience.player_should_feel));
    s.push_str(&list("Desired behaviours", &d.experience.desired_behaviors));
    s.push_str(&list("Avoid", &d.experience.avoid));
    for l in &d.loops {
        s.push_str(&format!("Loop \"{}\": {} ↺", l.name, l.steps.join(" → ")));
        if !l.failure.trim().is_empty() { s.push_str(&format!(" (fails when: {})", l.failure.trim())); }
        s.push('\n');
    }
    for sys in &d.systems {
        s.push_str(&format!("System {} [{}]: {}", sys.name, sys.maturity.as_str(), sys.intent.trim()));
        if !sys.current.trim().is_empty() { s.push_str(&format!(" — now: {}", sys.current.trim())); }
        if !sys.unknown.is_empty() { s.push_str(&format!(" — unknown: {}", sys.unknown.join("; "))); }
        s.push('\n');
    }
    for p in &d.parts {
        s.push_str(&format!("Part {}: {}", p.name, p.attributes.join(", ")));
        if !p.rules.is_empty() { s.push_str(&format!(" — rules: {}", p.rules.join("; "))); }
        s.push('\n');
    }
    // The closing rule always survives: a long design loses its last whole
    // lines, never the rule, and is never cut inside a character.
    let budget = PROMPT_BUDGET - UNKNOWN_RULE.len() - "…\n".len();
    if s.len() > budget {
        let mut end = budget;
        while !s.is_char_boundary(end) { end -= 1; }
        end = s[..end].rfind('\n').map(|i| i + 1).unwrap_or(end);
        s.truncate(end);
        s.push_str("…\n");
    }
    s.push_str(UNKNOWN_RULE);
    s
}

/// The most of the design, in bytes, that goes at the end of a prompt.
const PROMPT_BUDGET: usize = 12_000;
const UNKNOWN_RULE: &str = "What is marked unknown is not decided: do not invent it; name it as a question in your summary. Before changing a number, say whether the problem is in a part, a loop or the whole experience, and which desired behaviour the change serves.\n";

/// Everything the repository copy carries, so a project can be re-added
/// anywhere with its history intact.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DesignBundle {
    pub design: LivingGameDesign,
    #[serde(default)]
    pub questions: Vec<DesignQuestion>,
    #[serde(default)]
    pub decisions: Vec<DesignDecision>,
    #[serde(default)]
    pub hypotheses: Vec<crate::experiment::Hypothesis>,
    #[serde(default)]
    pub experiments: Vec<crate::experiment::Experiment>,
    #[serde(default)]
    pub observations: Vec<crate::experiment::Observation>,
}

fn md_list(items: &[String]) -> String { items.iter().map(|i| format!("- {i}\n")).collect() }
fn section(title: &str, body: &str) -> String { if body.trim().is_empty() { String::new() } else { format!("## {title}\n\n{}\n\n", body.trim()) } }
fn list_section(title: &str, items: &[String]) -> String { if items.is_empty() { String::new() } else { format!("## {title}\n\n{}\n", md_list(items)) } }
fn file_id(id: &str) -> String {
    let s: String = id.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    if s.is_empty() { "item".into() } else { s }
}

/// Write the design as linked Markdown under `<repo>/game-design/`
/// (broadest first, detail behind links, history apart from the current
/// design) plus `design.json`, the structured state the guild reads
/// back. Returns the files written; stale files of the generated
/// folders are removed.
pub fn export(repo: &Path, b: &DesignBundle) -> std::io::Result<Vec<String>> {
    let d = &b.design;
    let root = repo.join("game-design");
    for sub in ["concept", "loops", "systems", "objects", "sections", "questions", "decisions", "hypotheses", "experiments"] {
        let dir = root.join(sub);
        std::fs::create_dir_all(&dir)?;
        for entry in std::fs::read_dir(&dir)?.flatten() {
            if entry.path().extension().map(|e| e == "md" || e == "toml").unwrap_or(false) { let _ = std::fs::remove_file(entry.path()); }
        }
    }
    // Older exports kept TOML next to the Markdown; it is replaced.
    for stale in ["experience.toml", "parts"] { let p = root.join(stale); if p.is_dir() { let _ = std::fs::remove_dir_all(&p); } else { let _ = std::fs::remove_file(&p); } }
    let mut written = vec![];
    let mut write = |rel: &str, text: String| -> std::io::Result<()> {
        let p = root.join(rel);
        if let Some(dir) = p.parent() { std::fs::create_dir_all(dir)?; }
        std::fs::write(&p, text)?;
        written.push(p.display().to_string());
        Ok(())
    };
    // Main page: the tip of the iceberg.
    let mut o = format!("# {}\n\n", if d.title.is_empty() { "Untitled game" } else { &d.title });
    o.push_str(&format!("_Living game design, version {} · design maturity {}%_\n\n", d.version, maturity_percent(d)));
    o.push_str(&section("Current status", &d.status));
    o.push_str(&section("Concept statement", &d.concept.statement));
    o.push_str(&section("Genre", &d.concept.genre));
    o.push_str(&section("Target audience", &d.concept.audience));
    o.push_str(&list_section("Unique selling points", &d.concept.usps));
    o.push_str(&list_section("Pillars (in priority order)", &d.pillars.iter().map(|p| {
        let mut l = format!("**{}**", p.name.trim());
        if !p.statement.trim().is_empty() { l.push_str(&format!(": {}", p.statement.trim())); }
        if !p.design_test.trim().is_empty() { l.push_str(&format!(" _Test: {}_", p.design_test.trim())); }
        l
    }).collect::<Vec<_>>()));
    o.push_str(&list_section("Anti-pillars", &d.anti_pillars));
    o.push_str("## Maturity\n\n| Section | Maturity |\n| --- | --- |\n");
    for (_, name, m) in maturities(d) { o.push_str(&format!("| {name} | {} |\n", m.as_str())); }
    o.push('\n');
    o.push_str("## Sections\n\n- [Player experience](concept/player-experience.md)\n- [Visual and audio style](concept/visual-audio.md)\n- [Game world fiction](concept/world.md)\n- [Monetization, platform and scope](concept/platform-scope.md)\n- [Objectives and progression](concept/progression.md)\n- [Interactivity](concept/interactivity.md)\n");
    for l in &d.loops { o.push_str(&format!("- Loop: [{}](loops/{}.md)\n", l.name, file_id(&l.id))); }
    for s in &d.systems { o.push_str(&format!("- System: [{}](systems/{}.md) · {}\n", s.name, file_id(&s.id), s.maturity.as_str())); }
    for p in &d.parts { o.push_str(&format!("- Object: [{}](objects/{}.md)\n", p.name, file_id(&p.id))); }
    for s in &d.sections { o.push_str(&format!("- [{}](sections/{}.md) · {}\n", s.title, file_id(&s.id), s.maturity.as_str())); }
    let open: Vec<&DesignQuestion> = b.questions.iter().filter(|q| q.status == "open").collect();
    let resolved: Vec<&DesignQuestion> = b.questions.iter().filter(|q| q.status != "open").collect();
    o.push_str(&format!("\n## Unresolved questions ({})\n\n", open.len()));
    for q in &open { o.push_str(&format!("- [{}](questions/{}.md): {}\n", q.id, file_id(&q.id), q.question)); }
    o.push_str(&format!("\n## Resolved questions ({})\n\n", resolved.len()));
    for q in &resolved { o.push_str(&format!("- [{}](questions/{}.md): {} → {}\n", q.id, file_id(&q.id), q.question, q.decision_id.clone().unwrap_or_default())); }
    o.push_str(&format!("\n## Decisions ({})\n\n", b.decisions.len()));
    for dec in &b.decisions { o.push_str(&format!("- [{}](decisions/{}.md): {}\n", dec.id, file_id(&dec.id), dec.decision)); }
    if !b.hypotheses.is_empty() { o.push_str("\n## Hypotheses\n\n"); for h in &b.hypotheses { o.push_str(&format!("- [{}](hypotheses/{}.md) [{:?}]: {}\n", h.id, file_id(&h.id), h.status, h.hypothesis)); } }
    if !b.experiments.is_empty() { o.push_str("\n## Experiments\n\n"); for e in &b.experiments { o.push_str(&format!("- [{}](experiments/{}.md) [{:?}]: {}\n", e.id, file_id(&e.id), e.status, e.title)); } }
    if !b.observations.is_empty() {
        o.push_str(&format!("\n## Playtest observations ({})\n\n", b.observations.len()));
        for ob in &b.observations { o.push_str(&format!("- {} [{}] {}{}\n", ob.id, ob.kind, ob.text, ob.question_id.as_ref().map(|q| format!(" → [{q}](questions/{}.md)", file_id(q))).unwrap_or_default())); }
    }
    write("overview.md", o)?;
    let mut pe = "# Player experience\n\n".to_string();
    pe.push_str(&section("Fantasy", &d.experience.fantasy));
    pe.push_str(&section("Point of view and setting", &d.experience.pov));
    pe.push_str(&list_section("The player should feel", &d.experience.player_should_feel));
    pe.push_str(&list_section("Desired behaviours", &d.experience.desired_behaviors));
    pe.push_str(&list_section("Avoid", &d.experience.avoid));
    pe.push_str(&list_section("Key moments", &d.experience.key_moments));
    pe.push_str(&section("What keeps the player", &d.experience.engagement));
    write("concept/player-experience.md", pe)?;
    write("concept/visual-audio.md", format!("# Visual and audio style\n\n{}{}{}", section("Visual", &d.style.visual), section("Audio", &d.style.audio), list_section("References", &d.style.references)))?;
    write("concept/world.md", format!("# Game world fiction\n\n{}\n", if d.concept.world_fiction.trim().is_empty() { "_Not written yet._" } else { d.concept.world_fiction.trim() }))?;
    write("concept/platform-scope.md", format!("# Monetization, platform and scope\n\n{}{}{}", section("Monetization", &d.concept.monetization), section("Platform, technology and scope", &d.concept.platform_scope), section("Competition", &d.concept.competition)))?;
    write("concept/progression.md", format!("# Objectives and progression\n\n{}\n", if d.objectives.trim().is_empty() { "_Not written yet._" } else { d.objectives.trim() }))?;
    write("concept/interactivity.md", format!("# Interactivity\n\n{}\n", if d.interactivity.trim().is_empty() { "_Not written yet._" } else { d.interactivity.trim() }))?;
    for l in &d.loops {
        let mut t = format!("# Loop: {}\n\n{} ↺\n\n", l.name, l.steps.join(" → "));
        t.push_str(&list_section("Inputs", &l.inputs));
        t.push_str(&list_section("Outputs", &l.outputs));
        t.push_str(&section("Feedback", &l.feedback));
        t.push_str(&section("Fails when", &l.failure));
        t.push_str(&section("Notes", &l.notes));
        write(&format!("loops/{}.md", file_id(&l.id)), t)?;
    }
    for s in &d.systems {
        let mut t = format!("# {}\n\n**Maturity: {}** · {}\n\n", s.name, s.maturity.as_str(), if s.player_facing { "player-facing" } else { "internal" });
        t.push_str(&section("Intent", &s.intent));
        t.push_str(&section("Current design", &s.current));
        t.push_str(&list_section("Known", &s.known));
        t.push_str(&list_section("Unknown (open questions, not guesses)", &s.unknown));
        t.push_str(&list_section("Formulas", &s.formulas));
        t.push_str(&list_section("Tuning knobs", &s.tuning));
        t.push_str(&list_section("Edge cases", &s.edge_cases));
        t.push_str(&list_section("Acceptance criteria", &s.acceptance));
        t.push_str(&list_section("Depends on", &s.depends_on.iter().map(|x| format!("[{x}]({}.md)", file_id(x))).collect::<Vec<_>>()));
        let gaps = production_gaps(s);
        if s.maturity != Maturity::Production && !gaps.is_empty() { t.push_str(&format!("_Before production it needs {}._\n\n", gaps.join(", "))); }
        t.push_str(&list_section("Loops", &s.loops.iter().map(|l| format!("[{l}](../loops/{}.md)", file_id(l))).collect::<Vec<_>>()));
        t.push_str(&list_section("Objects", &s.parts.iter().map(|p| format!("[{p}](../objects/{}.md)", file_id(p))).collect::<Vec<_>>()));
        t.push_str(&list_section("Implementation", &s.implementation));
        t.push_str(&list_section("Assets", &s.assets));
        t.push_str(&list_section("Questions", &s.questions.iter().map(|q| format!("[{q}](../questions/{}.md)", file_id(q))).collect::<Vec<_>>()));
        t.push_str(&list_section("Decisions", &s.decisions.iter().map(|q| format!("[{q}](../decisions/{}.md)", file_id(q))).collect::<Vec<_>>()));
        t.push_str(&list_section("Experiments", &s.experiments.iter().map(|q| format!("[{q}](../experiments/{}.md)", file_id(q))).collect::<Vec<_>>()));
        write(&format!("systems/{}.md", file_id(&s.id)), t)?;
    }
    for p in &d.parts {
        write(&format!("objects/{}.md", file_id(&p.id)), format!("# {}\n\n{}{}{}", p.name, list_section("Attributes", &p.attributes), list_section("Rules", &p.rules), section("Notes", &p.notes)))?;
    }
    for s in &d.sections {
        write(&format!("sections/{}.md", file_id(&s.id)), format!("# {}\n\n**Maturity: {}**\n\n{}\n", s.title, s.maturity.as_str(), s.body.trim()))?;
    }
    for q in &b.questions {
        let mut t = format!("# {}\n\n**{}** · status: {} · raised by {}\n\n", q.id, q.question, q.status, if q.raised_by.is_empty() { "human" } else { &q.raised_by });
        t.push_str(&section("Context", &q.context));
        if let Some(s) = &q.system_id { t.push_str(&format!("System: [{s}](../systems/{}.md)\n\n", file_id(s))); }
        t.push_str(&list_section("Hypotheses", &q.hypothesis_ids));
        t.push_str(&list_section("Experiments", &q.experiment_ids));
        if let Some(dec) = &q.decision_id { t.push_str(&format!("## Decision\n\n[{dec}](../decisions/{}.md)\n\n", file_id(dec))); }
        t.push_str(&section("Answer", &q.answer));
        write(&format!("questions/{}.md", file_id(&q.id)), t)?;
    }
    for dec in &b.decisions {
        let mut t = format!("# {}\n\n**{}**\n\n", dec.id, dec.decision);
        t.push_str(&section("Why", &dec.why));
        t.push_str(&list_section("Based on", &dec.question_ids.iter().map(|q| format!("[{q}](../questions/{}.md)", file_id(q))).chain(dec.hypothesis_ids.iter().map(|h| format!("hypothesis {h}"))).chain(dec.experiment_ids.iter().map(|e| format!("experiment {e}"))).collect::<Vec<_>>()));
        t.push_str(&list_section("Evidence", &dec.evidence));
        t.push_str(&list_section("Playtests", &dec.playtest_ids));
        t.push_str(&list_section("Rejected alternatives", &dec.rejected_alternatives));
        t.push_str(&list_section("Systems", &dec.system_ids.iter().map(|s| format!("[{s}](../systems/{}.md)", file_id(s))).collect::<Vec<_>>()));
        t.push_str(&format!("_Decided by {} · design version {} · {}_\n", if dec.decided_by.is_empty() { "human" } else { &dec.decided_by }, dec.design_version, dec.created_at));
        write(&format!("decisions/{}.md", file_id(&dec.id)), t)?;
    }
    for h in &b.hypotheses {
        let mut t = format!("# Hypothesis {}\n\n**{}** · status: {:?} · level: {:?}\n\n", h.id, h.hypothesis, h.status, h.level);
        t.push_str(&section("Problem", &h.problem));
        t.push_str(&section("Evidence", &h.evidence));
        t.push_str(&section("Expected effect", &h.expected_effect));
        t.push_str(&section("Proposed change", &h.proposed_change));
        t.push_str(&list_section("Measurements", &h.measurements));
        write(&format!("hypotheses/{}.md", file_id(&h.id)), t)?;
    }
    for e in &b.experiments {
        let mut t = format!("# Experiment {}\n\n**{}** · status: {:?}\n\n", e.id, e.title, e.status);
        t.push_str("## Variants\n\n");
        for v in &e.variants { t.push_str(&format!("- {} ({}): {}\n", v.id, v.name, serde_json::to_string(&v.params).unwrap_or_default())); }
        if let Some(r) = &e.result {
            t.push_str(&format!("\n## Result\n\nVerdict: **{}** · confidence {:?} · recommended variant {:?}\n\n", r.verdict, r.confidence, r.recommended_variant));
            t.push_str(&list_section("Inputs", &r.inputs));
            t.push_str(&list_section("Evidence", &r.evidence));
            t.push_str(&list_section("Unexpected", &r.unexpected));
            t.push_str(&section("Recommended next", &r.recommended_next));
        }
        if let Some(dcs) = &e.decision { t.push_str(&format!("\n## Human decision\n\n{} {} {}\n", dcs.action, dcs.variant_id.clone().unwrap_or_default(), dcs.note)); }
        write(&format!("experiments/{}.md", file_id(&e.id)), t)?;
    }
    write("design.json", serde_json::to_string_pretty(b).unwrap_or_default())?;
    let readme = root.join("README.md");
    if !readme.exists() {
        std::fs::write(&readme, "# Game design\n\nWritten by the guild from its living game design. `overview.md` is the main page; the folders hold the current design (concept, loops, systems, objects, sections) and, apart from it, the history (questions, decisions, hypotheses, experiments). `design.json` is the structured state the guild reads back when the project is added again. Edit in the dungeon's design chamber; the guild rewrites these files.\n")?;
        written.push(readme.display().to_string());
    }
    Ok(written)
}

/// Read `<repo>/game-design/`: the structured `design.json`, or the
/// experience TOML an older export left.
pub fn import(repo: &Path, project_id: &str) -> Option<DesignBundle> {
    let root = repo.join("game-design");
    if let Ok(text) = std::fs::read_to_string(root.join("design.json")) {
        if let Ok(mut b) = serde_json::from_str::<DesignBundle>(&text) {
            b.design.project_id = project_id.into();
            for q in b.questions.iter_mut() { q.project_id = project_id.into(); }
            for d in b.decisions.iter_mut() { d.project_id = project_id.into(); }
            for h in b.hypotheses.iter_mut() { h.project_id = project_id.into(); }
            for e in b.experiments.iter_mut() { e.project_id = project_id.into(); }
            return Some(b);
        }
    }
    #[derive(Deserialize)]
    struct ExperienceFile { #[serde(default)] title: String, #[serde(default)] experience: Experience }
    let exp = std::fs::read_to_string(root.join("experience.toml")).ok()?;
    let file: ExperienceFile = toml::from_str(&exp).ok()?;
    let mut d = LivingGameDesign { project_id: project_id.into(), title: file.title, experience: file.experience, version: 1, updated_at: now(), ..Default::default() };
    let read_dir = |sub: &str| -> Vec<String> {
        let mut texts = vec![];
        if let Ok(entries) = std::fs::read_dir(root.join(sub)) {
            let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "toml").unwrap_or(false)).collect();
            paths.sort();
            for p in paths { if let Ok(t) = std::fs::read_to_string(&p) { texts.push(t); } }
        }
        texts
    };
    d.loops = read_dir("loops").iter().filter_map(|t| toml::from_str(t).ok()).collect();
    d.parts = read_dir("parts").iter().filter_map(|t| toml::from_str(t).ok()).collect();
    Some(DesignBundle { design: d, ..Default::default() })
}

/// The next human-readable id of a kind: Q-001, DEC-007…
pub fn next_id(prefix: &str, existing: impl Iterator<Item = String>) -> String {
    let max = existing.filter_map(|id| id.strip_prefix(prefix).and_then(|n| n.trim_start_matches('-').parse::<u32>().ok())).max().unwrap_or(0);
    format!("{prefix}-{:03}", max + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_videogame_template_round_trips_through_the_repo_as_markdown_and_json() {
        let d = template("p", "Verbrawl", "videogame");
        assert_eq!(d.loops.len(), 2);
        assert_eq!(d.systems.len(), 3);
        assert!(maturity_percent(&d) > 20 && maturity_percent(&d) < 60, "{}", maturity_percent(&d));
        let ctx = prompt_context(&d);
        assert!(ctx.contains("letters → create word"));
        assert!(ctx.contains("System Combat [prototyping]"));
        assert!(ctx.contains("unknown: the exact length"));
        assert!(ctx.contains("do not invent it"));
        let q = DesignQuestion { id: "Q-001".into(), project_id: "p".into(), question: "Is length too dominant?".into(), system_id: Some("combat".into()), status: "open".into(), raised_by: "human".into(), created_at: now(), ..Default::default() };
        let dec = DesignDecision { id: "DEC-001".into(), project_id: "p".into(), decision: "Semantic category is the principal multiplier.".into(), why: "Long words dominated.".into(), question_ids: vec!["Q-001".into()], evidence: vec!["12 synthetic runs".into()], system_ids: vec!["combat".into()], decided_by: "human".into(), design_version: 1, created_at: now(), ..Default::default() };
        let bundle = DesignBundle { design: d.clone(), questions: vec![q], decisions: vec![dec], ..Default::default() };
        let dir = std::env::temp_dir().join(format!("design-{}", crate::domain::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files = export(&dir, &bundle).unwrap();
        for f in ["overview.md", "concept/player-experience.md", "loops/core.md", "systems/combat.md", "objects/word.md", "questions/q-001.md", "decisions/dec-001.md", "design.json"] {
            assert!(files.iter().any(|w| w.ends_with(f)), "missing {f} in {files:?}");
        }
        let overview = std::fs::read_to_string(dir.join("game-design/overview.md")).unwrap();
        assert!(overview.contains("| Combat | prototyping |"));
        assert!(overview.contains("## Unresolved questions (1)"));
        let combat = std::fs::read_to_string(dir.join("game-design/systems/combat.md")).unwrap();
        assert!(combat.contains("## Unknown (open questions, not guesses)"));
        let back = import(&dir, "p").unwrap();
        assert_eq!(back.design.concept, d.concept);
        assert_eq!(back.design.systems, d.systems);
        assert_eq!(back.questions.len(), 1);
        assert_eq!(back.decisions[0].question_ids, vec!["Q-001".to_string()]);
        // Removing a system and exporting again drops its page.
        let mut fewer = bundle.clone();
        fewer.design.systems.truncate(1);
        export(&dir, &fewer).unwrap();
        assert!(!dir.join("game-design/systems/economy.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_system_reaches_production_only_with_edge_cases_acceptance_and_no_unknowns() {
        let mut d = template("p", "Juego", "videogame");
        assert!(check_production(&d).is_ok());
        d.systems[0].maturity = Maturity::Production;
        let err = check_production(&d).unwrap_err();
        assert!(err.contains("edge cases") && err.contains("acceptance criteria") && err.contains("no open unknowns"), "{err}");
        d.systems[0].edge_cases = vec!["an empty rare pool: rare letters cannot be drawn".into()];
        d.systems[0].acceptance = vec!["a 3-letter word matching the weakness beats an unmatched 6-letter word".into()];
        d.systems[0].unknown.clear();
        assert!(check_production(&d).is_ok());
        // Pillars reach the agents, first one first.
        d.pillars = vec![Pillar { name: "Words over reflexes".into(), statement: "you win by choosing, never by speed".into(), design_test: "can a slow player still win?".into() }];
        d.anti_pillars = vec!["a typing game".into()];
        let ctx = prompt_context(&d);
        assert!(ctx.contains("Pillars, first wins: Words over reflexes") && ctx.contains("Anti-pillars (the game is not): a typing game"));
        // And survive the repository.
        let dir = std::env::temp_dir().join(format!("design-{}", crate::domain::new_id()));
        export(&dir, &DesignBundle { design: d.clone(), ..Default::default() }).unwrap();
        let overview = std::fs::read_to_string(dir.join("game-design/overview.md")).unwrap();
        assert!(overview.contains("**Words over reflexes**"));
        let combat = std::fs::read_to_string(dir.join("game-design/systems/combat.md")).unwrap();
        assert!(combat.contains("## Acceptance criteria") && !combat.contains("Before production"));
        let progression = std::fs::read_to_string(dir.join("game-design/systems/progression.md")).unwrap();
        assert!(progression.contains("Before production it needs"));
        assert_eq!(import(&dir, "p").unwrap().design, d);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_design_keeps_whole_lines_and_the_closing_rule() {
        let mut d = template("p", "Juego", "videogame");
        // Multibyte text everywhere, so a byte cut would land inside a character.
        for i in 0..60 {
            d.systems.push(GameSystem { id: format!("s{i}"), name: format!("Sistema {i}"), intent: "Que la decisión pese → energía, agua y señal ".repeat(4), ..Default::default() });
        }
        let ctx = prompt_context(&d);
        assert!(ctx.len() <= PROMPT_BUDGET, "{}", ctx.len());
        assert!(ctx.ends_with(UNKNOWN_RULE));
        assert!(ctx.contains("…\n"));
        let body = ctx.strip_suffix(UNKNOWN_RULE).unwrap().strip_suffix("…\n").unwrap();
        assert!(body.ends_with('\n'), "the cut fell mid-line");
        assert!(body.lines().last().unwrap().starts_with("System Sistema "));
        // A short design is not marked as cut.
        assert!(!prompt_context(&template("p", "Juego", "videogame")).contains('…'));
    }

    #[test]
    fn ids_count_per_kind() {
        assert_eq!(next_id("Q", vec![].into_iter()), "Q-001");
        assert_eq!(next_id("Q", vec!["Q-003".to_string(), "Q-001".to_string(), "DEC-009".to_string()].into_iter()), "Q-004");
        assert_eq!(next_id("DEC", vec!["DEC-009".to_string()].into_iter()), "DEC-010");
        assert_eq!(Maturity::parse("Validated"), Some(Maturity::Validated));
    }
}
