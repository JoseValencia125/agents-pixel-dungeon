//! Each repo's guild: a heraldic emblem (shape, field, partition, charge)
//! and colours, worked out from the repo itself so it never changes between
//! runs and needs no setup. Kept apart from the agent's state: an agent's
//! room and state come and go, its repo's emblem stays.
//!
//! Two pure steps: `read_meta` looks at the folder (the `origin` remote's
//! `owner/repo`, and the files that tell what kind of project it is), and
//! `identity` turns that into the emblem. An optional override from the
//! settings goes on top (`apply_override`).

use egui::Color32;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// What a folder tells about its project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectMeta {
    /// The stable name the emblem is made from: `owner/repo` from the
    /// `origin` remote (lowercase), else the repo's (or folder's) name.
    pub key: String,
    /// The remote's owner (user, organization, or group path), if any. Not
    /// used for the emblem yet: kept so related repos can share a colour
    /// family later (see `identity`).
    pub owner: Option<String>,
    /// The name shown on the card: the repo's, else the folder's.
    pub name: String,
    pub kind: Kind,
}

/// The kind of project, from its files: picks the family of the charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Game,
    Mobile,
    Docs,
    Ruby,
    Python,
    Systems,
    Web,
    Infra,
    Unknown,
}

impl Kind {
    /// The charges this kind of project draws from; the repo's hash picks one.
    pub fn family(self) -> &'static [Symbol] {
        use Symbol::*;
        match self {
            Kind::Game => &[Sword, Heart, Crown],
            Kind::Mobile => &[Phone, Bolt],
            Kind::Docs => &[Book, Quill],
            Kind::Ruby => &[Gem, Crown],
            Kind::Python => &[Snake, Flask],
            Kind::Systems => &[Gear, Anvil],
            Kind::Web => &[Scroll, Bolt, Leaf],
            Kind::Infra => &[Crate, Tower],
            Kind::Unknown => &Symbol::ALL,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Game => "game",
            Kind::Mobile => "mobile",
            Kind::Docs => "docs",
            Kind::Ruby => "ruby",
            Kind::Python => "python",
            Kind::Systems => "systems",
            Kind::Web => "web",
            Kind::Infra => "infra",
            Kind::Unknown => "",
        }
    }
}

/// The emblem's charge, drawn as 6×6 pixels (`Symbol::pixels`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Symbol {
    Gear, Anvil, Snake, Flask, Gem, Crown, Phone, Bolt, Sword, Heart,
    Scroll, Leaf, Book, Quill, Crate, Tower, Star, Moon, Eye,
}

impl Symbol {
    pub const ALL: [Symbol; 19] = [
        Symbol::Gear, Symbol::Anvil, Symbol::Snake, Symbol::Flask, Symbol::Gem, Symbol::Crown, Symbol::Phone,
        Symbol::Bolt, Symbol::Sword, Symbol::Heart, Symbol::Scroll, Symbol::Leaf, Symbol::Book, Symbol::Quill,
        Symbol::Crate, Symbol::Tower, Symbol::Star, Symbol::Moon, Symbol::Eye,
    ];

    /// Rows of the 6×6 charge, '#' inked.
    pub fn pixels(self) -> [&'static str; 6] {
        match self {
            Symbol::Gear => [".#..#.", "######", "##..##", "##..##", "######", ".#..#."],
            Symbol::Anvil => ["######", ".#####", "..##..", "..##..", ".####.", "######"],
            Symbol::Snake => [".####.", "##....", ".####.", "....##", "....##", "#####."],
            Symbol::Flask => ["..##..", "..##..", ".####.", "######", "######", ".####."],
            Symbol::Gem => [".####.", "######", "#.##.#", ".####.", "..##..", "......"],
            Symbol::Crown => ["......", "#.##.#", "######", "######", "######", "......"],
            Symbol::Phone => [".####.", ".#..#.", ".#..#.", ".####.", ".#.##.", ".####."],
            Symbol::Bolt => ["...##.", "..##..", ".#####", "...##.", "..##..", ".##..."],
            Symbol::Sword => [".....#", "....#.", "...#..", "#.#...", ".#....", "#.#..."],
            Symbol::Heart => [".#..#.", "######", "######", ".####.", "..##..", "......"],
            Symbol::Scroll => [".####.", "##..##", ".#..#.", ".#..#.", "##..##", ".####."],
            Symbol::Leaf => ["....##", "..####", ".####.", ".###..", "##....", "#....."],
            Symbol::Book => ["######", "##...#", "##.#.#", "##...#", "######", "......"],
            Symbol::Quill => [".....#", "...###", "..###.", ".###..", ".#....", "#....."],
            Symbol::Crate => ["######", "##..##", "#.##.#", "#.##.#", "##..##", "######"],
            Symbol::Tower => ["#.##.#", "######", ".####.", ".#..#.", ".#..#.", ".####."],
            Symbol::Star => ["..##..", "..##..", "######", ".####.", ".#..#.", "#....#"],
            Symbol::Moon => ["..###.", ".##...", "##....", "##....", ".##...", "..###."],
            Symbol::Eye => ["......", ".####.", "#.##.#", ".####.", "......", "......"],
        }
    }
}

/// The shield's outline, 10×12 pixels ('#' inside the shield).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape { Heater, Round, Kite, Tower, Swallowtail, Oval }

impl Shape {
    pub const ALL: [Shape; 6] = [Shape::Heater, Shape::Round, Shape::Kite, Shape::Tower, Shape::Swallowtail, Shape::Oval];

    pub fn mask(self) -> [&'static str; 12] {
        match self {
            Shape::Heater => ["##########", "##########", "##########", "##########", "##########", "##########", "##########", "##########", ".########.", "..######..", "...####...", "....##...."],
            Shape::Round => ["##########", "##########", "##########", "##########", "##########", "##########", "##########", "##########", "##########", ".########.", "..######..", "....##...."],
            Shape::Kite => ["..######..", ".########.", "##########", "##########", "##########", "##########", ".########.", ".########.", "..######..", "..######..", "...####...", "....##...."],
            Shape::Tower => ["##.####.##", "##########", "##########", "##########", "##########", "##########", "##########", "##########", "##########", ".########.", "..######..", "...####..."],
            Shape::Swallowtail => ["##########", "##########", "##########", "##########", "##########", "##########", "##########", "##########", "##########", "##########", "####..####", "###....###"],
            Shape::Oval => ["...####...", ".########.", ".########.", "##########", "##########", "##########", "##########", "##########", "##########", ".########.", ".########.", "...####..."],
        }
    }
}

/// How the field is divided between the two tinctures. None crosses the
/// charge (columns 2–7, rows 2–7), so it always stands on the first colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pattern { Plain, Bordure, Base, Chief, Flanks, Barry }

impl Pattern {
    pub const ALL: [Pattern; 6] = [Pattern::Plain, Pattern::Bordure, Pattern::Base, Pattern::Chief, Pattern::Flanks, Pattern::Barry];

    /// Whether field pixel (x, y) of the 10×12 shield takes the second
    /// tincture; `edge` says it touches the shield's rim.
    pub fn second(self, x: usize, y: usize, edge: bool) -> bool {
        match self {
            Pattern::Plain => false,
            Pattern::Bordure => edge,
            Pattern::Base => y >= 8,
            Pattern::Chief => y <= 1,
            Pattern::Flanks => x <= 1 || x >= 8,
            Pattern::Barry => y <= 1 || y >= 9,
        }
    }
}

/// The field's colours: bright enough on the app's dark background, from
/// the Endesga-32 palette the rooms are drawn with, plus a burgundy and the
/// brand colours of Saludtech (its primary teal blue) and Proser (its blue).
/// The hash picks among the first `DRAWN`, never any RGB; the brand colours
/// after them are chosen by hand in the editor, or taken when the drawn
/// ones are all in use.
pub const COLOURS: [(&str, Color32); 15] = [
    ("red", Color32::from_rgb(228, 59, 68)),
    ("orange", Color32::from_rgb(247, 118, 34)),
    ("amber", Color32::from_rgb(254, 174, 52)),
    ("yellow", Color32::from_rgb(254, 231, 97)),
    ("green", Color32::from_rgb(99, 199, 77)),
    ("forest", Color32::from_rgb(62, 137, 72)),
    ("cyan", Color32::from_rgb(44, 232, 245)),
    ("blue", Color32::from_rgb(0, 153, 219)),
    ("magenta", Color32::from_rgb(181, 80, 136)),
    ("pink", Color32::from_rgb(246, 117, 122)),
    ("steel", Color32::from_rgb(139, 155, 180)),
    ("tan", Color32::from_rgb(228, 166, 114)),
    ("burgundy", Color32::from_rgb(128, 0, 32)),
    ("saludtech", Color32::from_rgb(40, 83, 107)),
    ("proser", Color32::from_rgb(0, 81, 160)),
];

/// How many of `COLOURS`, from the first, a repo's hash draws from.
pub const DRAWN: usize = 12;

/// The partition's colours: heraldry's metals and dark tinctures, so the
/// second colour always stands apart from the first (`identity` only takes
/// one far enough in lightness).
pub const METALS: [(&str, Color32); 6] = [
    ("gold", Color32::from_rgb(254, 231, 97)),
    ("silver", Color32::from_rgb(192, 203, 220)),
    ("bone", Color32::from_rgb(234, 212, 170)),
    ("iron", Color32::from_rgb(58, 68, 102)),
    ("ink", Color32::from_rgb(38, 43, 68)),
    ("wine", Color32::from_rgb(115, 62, 57)),
];

/// A repo's emblem and colours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub key: String,
    pub name: String,
    pub kind: Kind,
    /// Index in `COLOURS`.
    pub primary: usize,
    /// Index in `METALS`.
    pub secondary: usize,
    pub symbol: Symbol,
    pub pattern: Pattern,
    pub shape: Shape,
}

impl Identity {
    pub fn primary_color(&self) -> Color32 { COLOURS[self.primary].1 }
    pub fn secondary_color(&self) -> Color32 { METALS[self.secondary].1 }

    /// The charge's ink: dark on a light field, light on a dark one.
    pub fn ink(&self) -> Color32 {
        if lightness(self.primary_color()) > 0.55 { Color32::from_rgb(24, 20, 37) } else { Color32::WHITE }
    }
}

/// A repo's emblem as the settings keep it (`emblems`, by key), so it stays
/// the same even when the repo's files or this code change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedEmblem {
    pub color: String,
    pub metal: String,
    pub symbol: Symbol,
    pub pattern: Pattern,
    pub shape: Shape,
    pub kind: Kind,
}

/// A colour for a repo that no other kept emblem has (`taken`), starting
/// from `wanted` and going round the palette from a place of its own; with
/// every colour taken, the least used one.
pub fn free_colour(key: &str, wanted: usize, taken: &[usize]) -> usize {
    if !taken.contains(&wanted) { return wanted; }
    let start = (draw(stable_hash(key), 6) % DRAWN as u64) as usize;
    let order: Vec<usize> = (0..DRAWN).map(|i| (start + i) % DRAWN).chain(DRAWN..COLOURS.len()).collect();
    order.iter().copied().find(|c| !taken.contains(c))
        .unwrap_or_else(|| order.iter().copied().min_by_key(|c| taken.iter().filter(|t| *t == c).count()).unwrap_or(wanted))
}

/// The second tincture for `primary`: `metal` when it stands apart enough,
/// else the first one that does.
pub fn metal_apart(primary: usize, metal: usize) -> usize {
    let light = lightness(COLOURS[primary].1);
    if (lightness(METALS[metal].1) - light).abs() > 0.3 { return metal; }
    (0..METALS.len()).find(|&i| (lightness(METALS[i].1) - light).abs() > 0.3).unwrap_or(metal)
}

/// Give kept emblems sharing a colour each a colour of its own (the first
/// by key keeps it). True when one changed.
pub fn spread_colours(kept: &mut std::collections::BTreeMap<String, SavedEmblem>) -> bool {
    let mut taken: Vec<usize> = vec![];
    let mut changed = false;
    for (key, emblem) in kept.iter_mut() {
        let Some(colour) = COLOURS.iter().position(|(n, _)| *n == emblem.color) else { continue };
        let free = free_colour(key, colour, &taken);
        if free != colour {
            emblem.color = COLOURS[free].0.into();
            if let Some(metal) = METALS.iter().position(|(n, _)| *n == emblem.metal) { emblem.metal = METALS[metal_apart(free, metal)].0.into(); }
            changed = true;
        }
        taken.push(free);
    }
    changed
}

impl SavedEmblem {
    /// The colours kept emblems use, as `COLOURS` indices.
    pub fn colours_of<'a>(kept: impl Iterator<Item = &'a SavedEmblem>) -> Vec<usize> {
        kept.filter_map(|e| COLOURS.iter().position(|(n, _)| *n == e.color)).collect()
    }

    pub fn of(emblem: &Identity) -> SavedEmblem {
        SavedEmblem { color: COLOURS[emblem.primary].0.into(), metal: METALS[emblem.secondary].0.into(), symbol: emblem.symbol, pattern: emblem.pattern, shape: emblem.shape, kind: emblem.kind }
    }

    /// The emblem back, for the repo `meta` describes (None if its colours
    /// are no longer in the palettes).
    pub fn restore(&self, meta: &ProjectMeta) -> Option<Identity> {
        Some(Identity {
            key: meta.key.clone(),
            name: meta.name.clone(),
            kind: self.kind,
            primary: COLOURS.iter().position(|(n, _)| *n == self.color)?,
            secondary: METALS.iter().position(|(n, _)| *n == self.metal)?,
            symbol: self.symbol,
            pattern: self.pattern,
            shape: self.shape,
        })
    }
}

/// A user's choice for one repo, from the settings (`projects` in
/// settings.json, keyed by the repo's `key`): every field optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectOverride {
    /// Shown instead of the repo's name.
    pub name: Option<String>,
    /// A `COLOURS` name.
    pub color: Option<String>,
    /// A charge's name ("gear", "quill"…).
    pub symbol: Option<Symbol>,
    /// The kind, when the files mislead (it changes the charge's family).
    pub kind: Option<Kind>,
}

/// FNV-1a, 64 bit: the same number for the same text on every machine and
/// every run (std's hasher is seeded per process).
pub fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// An independent draw from the hash for each feature (splitmix64), so the
/// colour does not decide the shape.
fn draw(hash: u64, salt: u64) -> u64 {
    let mut z = hash.wrapping_add(salt.wrapping_mul(0x9e37_79b9_7f4a_7c15));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn lightness(c: Color32) -> f32 {
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
}

/// The emblem of a project: a pure function of its metadata.
///
/// Everything comes from the hash of `meta.key`. To group related repos
/// later, the colour could come from `meta.owner`'s hash instead (a family
/// per owner) while the rest stays per repo.
pub fn identity(meta: &ProjectMeta) -> Identity {
    let hash = stable_hash(&meta.key);
    let primary = (draw(hash, 1) % DRAWN as u64) as usize;
    let light = lightness(COLOURS[primary].1);
    let apart: Vec<usize> = (0..METALS.len()).filter(|&i| (lightness(METALS[i].1) - light).abs() > 0.3).collect();
    let secondary = apart[(draw(hash, 2) % apart.len() as u64) as usize];
    let family = meta.kind.family();
    Identity {
        key: meta.key.clone(),
        name: meta.name.clone(),
        kind: meta.kind,
        primary,
        secondary,
        symbol: family[(draw(hash, 3) % family.len() as u64) as usize],
        pattern: Pattern::ALL[(draw(hash, 4) % Pattern::ALL.len() as u64) as usize],
        shape: Shape::ALL[(draw(hash, 5) % Shape::ALL.len() as u64) as usize],
    }
}

/// An emblem (the one kept, or worked out) with the user's choices on top;
/// choosing a kind works the emblem out again for it.
pub fn apply_override(meta: &ProjectMeta, base: Identity, choice: Option<&ProjectOverride>) -> Identity {
    let Some(choice) = choice else { return base };
    let mut emblem = match choice.kind {
        Some(kind) if kind != base.kind => identity(&ProjectMeta { kind, ..meta.clone() }),
        _ => base,
    };
    if let Some(name) = choice.name.as_ref().filter(|n| !n.trim().is_empty()) { emblem.name = name.trim().to_string(); }
    if let Some(primary) = choice.color.as_ref().and_then(|c| COLOURS.iter().position(|(n, _)| n.eq_ignore_ascii_case(c.trim()))) {
        emblem.primary = primary;
        let light = lightness(COLOURS[primary].1);
        if (lightness(METALS[emblem.secondary].1) - light).abs() <= 0.3 {
            emblem.secondary = (0..METALS.len()).find(|&i| (lightness(METALS[i].1) - light).abs() > 0.3).unwrap_or(emblem.secondary);
        }
    }
    if let Some(symbol) = choice.symbol { emblem.symbol = symbol; }
    emblem
}

// ---- Reading a folder

/// The project behind a folder: its repo root's remote and files. Reads a
/// few small files, no process.
pub fn read_meta(cwd: &str) -> ProjectMeta {
    // Agents' folders come abbreviated for display (~/…).
    let expanded = crate::herdr::expand_home(cwd);
    let cwd = expanded.as_str();
    let path = Path::new(cwd);
    let root = repo_root(path);
    let folder = |p: &Path| p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let base = root.as_deref().unwrap_or(path);
    let remote = root.as_deref().and_then(origin_url).and_then(|url| owner_repo(&url));
    let names: HashSet<String> = std::fs::read_dir(base).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    let kind = detect_kind(&names, |rel| base.join(rel).exists());
    match remote {
        Some((owner, repo)) => ProjectMeta { key: format!("{owner}/{repo}").to_lowercase(), owner: Some(owner), name: repo, kind },
        None => {
            let name = folder(base);
            let name = if name.is_empty() { cwd.to_string() } else { name };
            ProjectMeta { key: name.to_lowercase(), owner: None, name, kind }
        }
    }
}

/// The nearest folder up with a `.git` (directory, or file in a worktree).
fn repo_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// The `origin` remote's URL from the repo's config (a worktree's `.git`
/// file points at its gitdir, whose `commondir` holds the config).
fn origin_url(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let dir = root.join(text.trim().strip_prefix("gitdir:")?.trim());
        match std::fs::read_to_string(dir.join("commondir")) { Ok(common) => dir.join(common.trim()), Err(_) => dir }
    } else { dot_git };
    let config = std::fs::read_to_string(git_dir.join("config")).ok()?;
    remote_url(&config, "origin")
}

/// A remote's URL in a git config's text.
pub fn remote_url(config: &str, remote: &str) -> Option<String> {
    let header = format!("[remote \"{remote}\"]");
    let mut inside = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') { inside = line == header; continue; }
        if inside {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "url" { return Some(value.trim().to_string()); }
            }
        }
    }
    None
}

/// `owner` and `repo` from a remote URL: scp-like (`git@host:owner/repo.git`),
/// ssh://, https:// or a local path. Nested groups (GitLab) stay in the owner.
pub fn owner_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/');
    let path = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        rest.split_once('/').map(|(_, p)| p)?
    } else if let Some((_, path)) = url.split_once(':').filter(|(host, _)| !host.contains('/')) {
        path
    } else {
        url
    };
    let path = path.trim_end_matches(".git").trim_matches('/');
    let (owner, repo) = path.rsplit_once('/')?;
    let owner = owner.rsplit('/').take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
    (!owner.is_empty() && !repo.is_empty()).then(|| (owner, repo.to_string()))
}

/// The kind of project from the names at its root (`names`) and a check for
/// deeper paths (`exists`). The first match wins, most telling first: a
/// Rails app also has a package.json, a game a Cargo.toml.
pub fn detect_kind(names: &HashSet<String>, exists: impl Fn(&str) -> bool) -> Kind {
    let has = |n: &str| names.contains(n);
    let ends = |suffix: &str| names.iter().any(|n| n.ends_with(suffix));
    if has("project.godot") || ends(".uproject") || exists("ProjectSettings/ProjectVersion.txt") { return Kind::Game; }
    if exists("app/src/main/AndroidManifest.xml") || has("AndroidManifest.xml") || has("pubspec.yaml") || ends(".xcodeproj") || ends(".xcworkspace") || has("Podfile") { return Kind::Mobile; }
    if has("mkdocs.yml") || has("book.toml") || names.iter().any(|n| n.starts_with("docusaurus.config.")) { return Kind::Docs; }
    if has("Gemfile") || ends(".gemspec") { return Kind::Ruby; }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") || has("Pipfile") || ends(".ipynb") { return Kind::Python; }
    if has("Cargo.toml") || has("go.mod") || has("CMakeLists.txt") || has("pom.xml") || has("build.gradle") || has("build.gradle.kts") || has("build.zig") || ends(".csproj") || ends(".sln") { return Kind::Systems; }
    if has("package.json") || has("deno.json") || has("tsconfig.json") { return Kind::Web; }
    if has("Dockerfile") || has("docker-compose.yml") || has("compose.yaml") || ends(".tf") || has("Chart.yaml") { return Kind::Infra; }
    let files: Vec<&String> = names.iter().filter(|n| !n.starts_with('.') && n.contains('.')).collect();
    if !files.is_empty() && files.iter().all(|n| n.ends_with(".md") || n.ends_with(".txt") || n.ends_with(".pdf")) { return Kind::Docs; }
    Kind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(key: &str, kind: Kind) -> ProjectMeta {
        ProjectMeta { key: key.into(), owner: key.split_once('/').map(|(o, _)| o.into()), name: key.rsplit('/').next().unwrap().into(), kind }
    }

    fn names(list: &[&str]) -> HashSet<String> { list.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn same_repo_same_emblem() {
        let a = identity(&meta("someone/verbrawl", Kind::Game));
        let b = identity(&meta("someone/verbrawl", Kind::Game));
        assert_eq!(a, b);
    }

    #[test]
    fn hash_is_fixed_across_runs_and_machines() {
        // FNV-1a's published values: a change here would change every emblem.
        assert_eq!(stable_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash("a"), 0xaf63_dc4c_8601_ec8c);
        let emblem = identity(&meta("someone/verbrawl", Kind::Unknown));
        assert_eq!((emblem.primary, emblem.secondary, emblem.symbol, emblem.pattern, emblem.shape), {
            let again = identity(&meta("someone/verbrawl", Kind::Unknown));
            (again.primary, again.secondary, again.symbol, again.pattern, again.shape)
        });
    }

    #[test]
    fn different_repos_usually_differ() {
        let keys: Vec<String> = (0..40).map(|i| format!("owner{}/repo-{i}", i % 3)).collect();
        let emblems: HashSet<_> = keys.iter().map(|k| { let e = identity(&meta(k, Kind::Unknown)); (e.primary, e.secondary, e.symbol, e.pattern, e.shape) }).collect();
        assert!(emblems.len() >= 38, "{} distinct emblems of 40", emblems.len());
        let colours: HashSet<usize> = keys.iter().map(|k| identity(&meta(k, Kind::Unknown)).primary).collect();
        assert!(colours.len() >= 8, "colours spread: {}", colours.len());
    }

    #[test]
    fn works_without_owner_or_kind() {
        let alone = ProjectMeta { key: "notes".into(), owner: None, name: "notes".into(), kind: Kind::Unknown };
        let emblem = identity(&alone);
        assert_eq!(emblem.name, "notes");
        assert!(Symbol::ALL.contains(&emblem.symbol));
    }

    #[test]
    fn second_colour_stands_apart() {
        for i in 0..200 {
            let e = identity(&meta(&format!("o/r{i}"), Kind::Unknown));
            assert!((lightness(e.primary_color()) - lightness(e.secondary_color())).abs() > 0.3);
        }
    }

    #[test]
    fn kind_picks_the_family() {
        for i in 0..30 {
            let e = identity(&meta(&format!("o/game{i}"), Kind::Game));
            assert!(Kind::Game.family().contains(&e.symbol));
        }
    }

    #[test]
    fn override_wins() {
        let m = meta("someone/verbrawl", Kind::Game);
        let choice = ProjectOverride { name: Some("Verbrawl".into()), color: Some("Yellow".into()), symbol: Some(Symbol::Quill), kind: None };
        let e = apply_override(&m, identity(&m), Some(&choice));
        assert_eq!((e.name.as_str(), COLOURS[e.primary].0, e.symbol), ("Verbrawl", "yellow", Symbol::Quill));
        assert!((lightness(e.primary_color()) - lightness(e.secondary_color())).abs() > 0.3);
        assert_eq!(apply_override(&m, identity(&m), None), identity(&m));
        let parsed: ProjectOverride = serde_json::from_str(r#"{"symbol":"gear","kind":"python"}"#).unwrap();
        let e = apply_override(&m, identity(&m), Some(&parsed));
        assert_eq!((e.symbol, e.kind), (Symbol::Gear, Kind::Python));
    }

    #[test]
    fn kept_emblems_stay() {
        let m = meta("someone/app", Kind::Web);
        let kept = SavedEmblem::of(&identity(&m));
        let json = serde_json::to_string(&kept).unwrap();
        let back: SavedEmblem = serde_json::from_str(&json).unwrap();
        // The repo later looks like another kind: the kept emblem wins.
        let now = ProjectMeta { kind: Kind::Python, ..m.clone() };
        assert_eq!(back.restore(&now).unwrap(), identity(&m));
        let gone = SavedEmblem { color: "mauve".into(), ..kept };
        assert_eq!(gone.restore(&m), None);
    }

    #[test]
    fn colours_stay_apart() {
        let mut kept = std::collections::BTreeMap::new();
        for i in 0..COLOURS.len() {
            let m = meta(&format!("o/r{i}"), Kind::Unknown);
            let e = identity(&m);
            let taken = SavedEmblem::colours_of(kept.values());
            let mut saved = SavedEmblem::of(&e);
            let free = free_colour(&m.key, e.primary, &taken);
            saved.color = COLOURS[free].0.into();
            saved.metal = METALS[metal_apart(free, e.secondary)].0.into();
            kept.insert(m.key.clone(), saved);
        }
        let colours: HashSet<String> = kept.values().map(|e| e.color.clone()).collect();
        assert_eq!(colours.len(), COLOURS.len(), "as many repos as colours, each with its own");
        // Two kept with the same colour: the second by key moves.
        let mut twins = std::collections::BTreeMap::new();
        let base = SavedEmblem::of(&identity(&meta("a/one", Kind::Unknown)));
        twins.insert("a/one".to_string(), SavedEmblem { color: "blue".into(), metal: "gold".into(), ..base.clone() });
        twins.insert("b/two".to_string(), SavedEmblem { color: "blue".into(), metal: "gold".into(), ..base });
        assert!(spread_colours(&mut twins));
        assert_eq!(twins["a/one"].color, "blue");
        assert_ne!(twins["b/two"].color, "blue");
        let two = twins["b/two"].restore(&meta("b/two", Kind::Unknown)).unwrap();
        assert!((lightness(two.primary_color()) - lightness(two.secondary_color())).abs() > 0.3);
        assert!(!spread_colours(&mut twins));
    }

    #[test]
    fn remotes() {
        assert_eq!(owner_repo("git@github.com:Someone/Verbrawl.git"), Some(("Someone".into(), "Verbrawl".into())));
        assert_eq!(owner_repo("https://github.com/someone/verbrawl"), Some(("someone".into(), "verbrawl".into())));
        assert_eq!(owner_repo("ssh://git@gitlab.com:2222/group/sub/app.git"), Some(("group/sub".into(), "app".into())));
        assert_eq!(owner_repo("/srv/git/team/tool.git"), Some(("srv/git/team".into(), "tool".into())));
        assert_eq!(owner_repo("nothing"), None);
        let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@x:a/b.git\n[remote \"origin\"]\n\turl = git@github.com:me/app.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";
        assert_eq!(remote_url(config, "origin").as_deref(), Some("git@github.com:me/app.git"));
        assert_eq!(remote_url("[core]\n", "origin"), None);
    }

    #[test]
    fn kinds() {
        let none = |_: &str| false;
        assert_eq!(detect_kind(&names(&["project.godot", "Cargo.toml"]), none), Kind::Game);
        assert_eq!(detect_kind(&names(&["app", "build.gradle.kts"]), |p| p == "app/src/main/AndroidManifest.xml"), Kind::Mobile);
        assert_eq!(detect_kind(&names(&["Gemfile", "package.json"]), none), Kind::Ruby);
        assert_eq!(detect_kind(&names(&["pyproject.toml"]), none), Kind::Python);
        assert_eq!(detect_kind(&names(&["Cargo.toml", "Dockerfile"]), none), Kind::Systems);
        assert_eq!(detect_kind(&names(&["package.json", "tsconfig.json"]), none), Kind::Web);
        assert_eq!(detect_kind(&names(&["Dockerfile", "README.md"]), none), Kind::Infra);
        assert_eq!(detect_kind(&names(&["mkdocs.yml", "package.json"]), none), Kind::Docs);
        assert_eq!(detect_kind(&names(&["README.md", "notes.md", ".git"]), none), Kind::Docs);
        assert_eq!(detect_kind(&names(&["main.c"]), none), Kind::Unknown);
        assert_eq!(detect_kind(&names(&[]), none), Kind::Unknown);
    }

    #[test]
    fn reads_a_real_repo() {
        let dir = std::env::temp_dir().join(format!("apd-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join(".git/config"), "[remote \"origin\"]\n\turl = git@github.com:Someone/Pixel-App.git\n").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        let m = read_meta(&dir.join("src/deep").to_string_lossy());
        assert_eq!((m.key.as_str(), m.owner.as_deref(), m.name.as_str(), m.kind), ("someone/pixel-app", Some("Someone"), "Pixel-App", Kind::Systems));
        let plain = std::env::temp_dir().join(format!("apd-identity-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        let m = read_meta(&plain.to_string_lossy());
        assert!(m.owner.is_none() && m.key.starts_with("apd-identity-plain-") && m.kind == Kind::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&plain);
    }
}
