//! The game design a project's guild works from, in Sellers' three
//! levels: the **whole** (the experience the player should have), the
//! **loops** (the systems that produce it) and the **parts** (the
//! entities and rules). It lives in the guild's database and, when the
//! user asks, in the repository under `game-design/` as TOML, where the
//! agents can read it and a project can carry it. Every attempt's prompt
//! ends with a summary of it, so each change can be argued from the
//! experience.

use crate::domain::now;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Experience {
    /// Who the player is in the fiction, in one sentence.
    #[serde(default)]
    pub fantasy: String,
    #[serde(default)]
    pub player_should_feel: Vec<String>,
    #[serde(default)]
    pub desired_behaviors: Vec<String>,
    #[serde(default)]
    pub avoid: Vec<String>,
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameDesign {
    pub project_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub experience: Experience,
    #[serde(default)]
    pub loops: Vec<GameLoop>,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
}

/// The branch the guild publishes a project's design to; the user merges it.
pub fn design_ref(project_id: &str) -> String { format!("refs/heads/guild/design/{project_id}") }

/// The names of the templates `template()` knows.
pub const TEMPLATES: [&str; 2] = ["videogame", "blank"];

/// A design to start from. `videogame` is a small word-combat game that
/// shows every level filled in; `blank` has the headings only.
pub fn template(project_id: &str, title: &str, name: &str) -> GameDesign {
    let mut d = GameDesign { project_id: project_id.into(), title: title.into(), version: 1, updated_at: now(), ..Default::default() };
    if name != "videogame" { return d }
    d.experience = Experience {
        fantasy: "I am a writer trying to make a name for myself, fighting with words.".into(),
        player_should_feel: ["creativity", "pressure", "risk", "satisfaction when the perfect word lands"].iter().map(|s| s.to_string()).collect(),
        desired_behaviors: ["consider several words before committing", "take risks on rare letters", "adapt the strategy to the enemy"].iter().map(|s| s.to_string()).collect(),
        avoid: ["a dominant strategy", "always choosing the longest word", "obvious decisions"].iter().map(|s| s.to_string()).collect(),
    };
    d.loops = vec![
        GameLoop {
            id: "core".into(),
            name: "Word combat".into(),
            steps: ["letters", "create word", "word properties", "effect", "enemy reaction", "board changes", "new letters"].iter().map(|s| s.to_string()).collect(),
            inputs: ["letter pool", "enemy state", "modifiers in play"].iter().map(|s| s.to_string()).collect(),
            outputs: ["damage", "board state", "rewards"].iter().map(|s| s.to_string()).collect(),
            feedback: "Rarer letters give better words but drain the pool: balancing. Rewards unlock modifiers that make riskier words viable: reinforcing.".into(),
            failure: "The longest word is always best (dominant strategy); the pool runs dry with no way to recover (dead end).".into(),
            notes: String::new(),
        },
        GameLoop {
            id: "run".into(),
            name: "The run".into(),
            steps: ["choose a path", "fight", "collect reward", "pick a modifier", "stronger enemy"].iter().map(|s| s.to_string()).collect(),
            inputs: ["rewards from fights", "player build"].iter().map(|s| s.to_string()).collect(),
            outputs: ["build diversity", "difficulty curve"].iter().map(|s| s.to_string()).collect(),
            feedback: "Modifiers compound into a build: reinforcing, capped by enemy scaling.".into(),
            failure: "One modifier combination trivialises every fight.".into(),
            notes: String::new(),
        },
    ];
    d.parts = vec![
        Part { id: "letter".into(), name: "Letter".into(), attributes: ["char", "rarity: common | uncommon | rare", "base_power: 1..5"].iter().map(|s| s.to_string()).collect(), rules: vec!["rare letters refill slower".into()], notes: String::new() },
        Part { id: "word".into(), name: "Word".into(), attributes: ["letters", "length", "rarity", "category: semantic field", "base_power"].iter().map(|s| s.to_string()).collect(), rules: vec!["power = sum(letter power) × length multiplier × semantic multiplier".into(), "a word fits the enemy's weakness when its category matches".into()], notes: String::new() },
        Part { id: "enemy".into(), name: "Enemy".into(), attributes: ["hp", "weakness: category", "reaction: attack | shield | steal letters"].iter().map(|s| s.to_string()).collect(), rules: vec!["reacts after every word".into()], notes: String::new() },
        Part { id: "modifier".into(), name: "Modifier".into(), attributes: ["trigger", "effect", "duration"].iter().map(|s| s.to_string()).collect(), rules: vec!["at most three in play".into()], notes: String::new() },
        Part { id: "round".into(), name: "Round".into(), attributes: ["number", "enemy", "letter pool"].iter().map(|s| s.to_string()).collect(), rules: vec!["ends when the enemy or the player falls".into()], notes: String::new() },
    ];
    d
}

/// The design as the agents read it at the end of their prompt: short,
/// every level present, nothing they could not act on.
pub fn prompt_context(d: &GameDesign) -> String {
    let mut s = String::new();
    s.push_str("\n## Game design (whole → loops → parts)\n");
    if !d.experience.fantasy.trim().is_empty() { s.push_str(&format!("Fantasy: {}\n", d.experience.fantasy.trim())); }
    let list = |label: &str, items: &[String]| if items.is_empty() { String::new() } else { format!("{label}: {}\n", items.join("; ")) };
    s.push_str(&list("The player should feel", &d.experience.player_should_feel));
    s.push_str(&list("Desired behaviours", &d.experience.desired_behaviors));
    s.push_str(&list("Avoid", &d.experience.avoid));
    for l in &d.loops {
        s.push_str(&format!("Loop \"{}\": {} ↺", l.name, l.steps.join(" → ")));
        if !l.failure.trim().is_empty() { s.push_str(&format!(" (fails when: {})", l.failure.trim())); }
        s.push('\n');
    }
    for p in &d.parts {
        s.push_str(&format!("Part {}: {}", p.name, p.attributes.join(", ")));
        if !p.rules.is_empty() { s.push_str(&format!(" — rules: {}", p.rules.join("; "))); }
        s.push('\n');
    }
    s.push_str("Before changing a number, say whether the problem is in a part, a loop or the whole experience, and which desired behaviour the change serves.\n");
    if s.len() > 6000 { s.truncate(6000); s.push_str("…\n"); }
    s
}

#[derive(Serialize, Deserialize)]
struct ExperienceFile { title: String, experience: Experience }

/// Write the design under `<repo>/game-design/` as TOML; returns the
/// files written. Stale loop and part files are removed.
pub fn export(repo: &Path, d: &GameDesign) -> std::io::Result<Vec<String>> {
    let root = repo.join("game-design");
    let loops = root.join("loops");
    let parts = root.join("parts");
    std::fs::create_dir_all(&loops)?;
    std::fs::create_dir_all(&parts)?;
    for dir in [&loops, &parts] {
        for entry in std::fs::read_dir(dir)?.flatten() {
            if entry.path().extension().map(|e| e == "toml").unwrap_or(false) { let _ = std::fs::remove_file(entry.path()); }
        }
    }
    let mut written = vec![];
    let io = |e: toml::ser::Error| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string());
    let exp = root.join("experience.toml");
    std::fs::write(&exp, toml::to_string_pretty(&ExperienceFile { title: d.title.clone(), experience: d.experience.clone() }).map_err(io)?)?;
    written.push(exp.display().to_string());
    // Numbered so the order survives the round trip.
    for (i, l) in d.loops.iter().enumerate() {
        let p = loops.join(format!("{:02}-{}.toml", i + 1, file_id(&l.id)));
        std::fs::write(&p, toml::to_string_pretty(l).map_err(io)?)?;
        written.push(p.display().to_string());
    }
    for (i, part) in d.parts.iter().enumerate() {
        let p = parts.join(format!("{:02}-{}.toml", i + 1, file_id(&part.id)));
        std::fs::write(&p, toml::to_string_pretty(part).map_err(io)?)?;
        written.push(p.display().to_string());
    }
    let readme = root.join("README.md");
    if !readme.exists() {
        std::fs::write(&readme, "# Game design\n\nWritten by the guild: `experience.toml` is the whole (the experience the player should have), `loops/` the systems that produce it, `parts/` the entities and rules. Edit here or in the dungeon's guild window; the guild reads it back when the project is added.\n")?;
        written.push(readme.display().to_string());
    }
    Ok(written)
}

/// Read `<repo>/game-design/` if it exists.
pub fn import(repo: &Path, project_id: &str) -> Option<GameDesign> {
    let root = repo.join("game-design");
    let exp = std::fs::read_to_string(root.join("experience.toml")).ok()?;
    let file: ExperienceFile = toml::from_str(&exp).ok()?;
    let mut d = GameDesign { project_id: project_id.into(), title: file.title, experience: file.experience, version: 1, updated_at: now(), ..Default::default() };
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
    Some(d)
}

fn file_id(id: &str) -> String {
    let s: String = id.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    if s.is_empty() { "item".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_videogame_template_round_trips_through_the_repo() {
        let d = template("p", "Verbrawl", "videogame");
        assert_eq!(d.loops.len(), 2);
        assert_eq!(d.parts.len(), 5);
        let ctx = prompt_context(&d);
        assert!(ctx.contains("letters → create word"));
        assert!(ctx.contains("Avoid: a dominant strategy"));
        let dir = std::env::temp_dir().join(format!("design-{}", crate::domain::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files = export(&dir, &d).unwrap();
        assert!(files.iter().any(|f| f.ends_with("experience.toml")));
        assert!(files.iter().any(|f| f.ends_with("loops/01-core.toml")));
        let back = import(&dir, "p").unwrap();
        assert_eq!(back.experience, d.experience);
        assert_eq!(back.loops, d.loops);
        assert_eq!(back.parts, d.parts);
        // Removing a loop and exporting again drops its file.
        let mut fewer = d.clone();
        fewer.loops.truncate(1);
        export(&dir, &fewer).unwrap();
        assert!(!dir.join("game-design/loops/02-run.toml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
