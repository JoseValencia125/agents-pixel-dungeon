//! The dungeon: one room per agent, drawn with egui's painter. Each room
//! shows the art of the agent's state and a hero that works at the right
//! station, jumps for attention, rests exhausted, reads, celebrates or
//! wanders; status changes walk the hero out through the door and into the new room.
//! Animation is driven by elapsed time, so Reduce Motion simply stops it.

use crate::assets::{Assets, Sprite, GIT_TOME_FRAME, GIT_TOME_HERO};
use crate::herdr::{harness_for, Agent};
use crate::identity::{Identity, ProjectMeta, ProjectOverride};
use crate::l10n::tr;
use egui::emath::Rot2;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const ART: Vec2 = vec2(144.0, 100.0);
/// The guild hall's art: a room's width, twice its height (less the wall
/// two rooms would share).
pub const HALL_ART: Vec2 = vec2(144.0, 192.0);
pub const FOOTER: f32 = 36.0; // caption band under the room: chat title + folder · branch
pub const HEADER: f32 = 28.0; // the repo's band over the room: its emblem and name
pub const ROW_H: f32 = 158.0 + HEADER + FOOTER; // room art 100 px at 1.5 pt/px + 8 pt frame, plus header and footer
pub const ROW_W: f32 = 220.0; // room art 144 px at 1.5 pt/px + 4 pt frame
pub const HERO: f32 = 32.0; // 16 px sprite at 2 pt/px
pub const GAP: f32 = 6.0;
/// The guild hall's cell: frame, band, its art at 1.5 pt/px, footer.
pub const HALL_H: f32 = 8.0 + HEADER + HALL_ART.y * 1.5 + FOOTER;
/// A room's side wall in points (8 art pixels): a double room hides the
/// two that would stand between its halves.
pub const SIDE_WALL: f32 = 12.0;
pub const LINE_H: f32 = 30.0; // full screen's spare room, as a line
/// Between full screen's rooms: their cells already leave 4 pt above and
/// below the frame, so this brings two frames 6 pt apart.
pub const COLUMN_GAP: f32 = -2.0;
pub const TOP_PAD: f32 = 14.0; // room for the drag handle
pub const BOTTOM_PAD: f32 = 6.0;

pub const BACKGROUND: Color32 = Color32::from_rgb(13, 15, 17);
const GOLD: Color32 = Color32::from_rgb(254, 231, 97);
const ORANGE: Color32 = Color32::from_rgb(247, 118, 34);
const MINT: Color32 = Color32::from_rgb(140, 230, 180);
const CYAN: Color32 = Color32::from_rgb(44, 232, 245);
const RED: Color32 = Color32::from_rgb(228, 59, 68);
const GREEN: Color32 = Color32::from_rgb(99, 199, 77);
const VIOLET: Color32 = Color32::from_rgb(181, 80, 136);
const PINK: Color32 = Color32::from_rgb(245, 153, 204);
/// The usage limit's colour.
pub const LIMITED: Color32 = Color32::from_rgb(175, 82, 222);

/// Rooms wrap like a flex row: as many columns as fit the width.
pub fn columns_fitting(width: f32) -> usize {
    (((width - GAP + 0.5) / (ROW_W + GAP)) as usize).max(1)
}

pub fn width_for(columns: usize) -> f32 {
    columns as f32 * ROW_W + (columns + 1) as f32 * GAP
}

/// Visual recipe for a room. Each agent's row renders the room matching its
/// current Herdr status; when the status changes, the row swaps rooms and
/// the hero moves to that room's spot.
#[derive(Clone, Copy)]
pub struct RoomStyle {
    pub status: &'static str,
    pub title: &'static str,
    pub background: &'static str,
    pub color: Color32,
    pub bubble: Option<&'static str>,
    pub spot: Pos2,
    pub roam: f32,
    pub lying: bool,
    pub route: &'static [Pos2],
}

pub const STYLES: [RoomStyle; 6] = [
    RoomStyle { status: "blocked", title: "ATENCIÓN", background: "room_blocked", color: Color32::from_rgb(255, 69, 58), bubble: None, spot: pos2(72.0, 64.0), roam: 8.0, lying: false, route: &[] },
    RoomStyle { status: "limited", title: "AGOTADO", background: "room_limited", color: LIMITED, bubble: None, spot: pos2(63.0, 57.0), roam: 0.0, lying: true,
                route: &[pos2(72.0, 80.0), pos2(52.0, 72.0)] },
    RoomStyle { status: "working", title: "TRABAJANDO", background: "room_working", color: Color32::from_rgb(255, 159, 10), bubble: None, spot: pos2(72.0, 34.0), roam: 0.0, lying: false,
                route: &[pos2(72.0, 80.0), pos2(36.0, 74.0), pos2(36.0, 36.0)] },
    RoomStyle { status: "idle", title: "EN ESPERA", background: "room_idle", color: Color32::from_rgb(152, 152, 157), bubble: None, spot: pos2(106.0, 40.0), roam: 0.0, lying: false,
                route: &[pos2(72.0, 80.0), pos2(90.0, 74.0), pos2(90.0, 46.0)] },
    RoomStyle { status: "done", title: "LISTO", background: "room_done", color: Color32::from_rgb(48, 209, 88), bubble: None, spot: pos2(72.0, 66.0), roam: 0.0, lying: false, route: &[] },
    RoomStyle { status: "unknown", title: "SIN ESTADO", background: "room_unknown", color: Color32::from_rgb(152, 152, 157), bubble: Some("…"), spot: pos2(72.0, 62.0), roam: 24.0, lying: false, route: &[] },
];

/// The waiting room's window, whose moonlight the reading hero sits in, and
/// the hearth at its back.
const WINDOW: Pos2 = pos2(92.0, 30.0);
const HEARTH: Pos2 = pos2(106.0, 22.0);

/// The doorway in the bottom wall that every room shares, and a point just
/// outside it (clipped away) where the hero leaves and arrives.
const DOOR: Pos2 = pos2(72.0, 88.0);
const OUTSIDE: Pos2 = pos2(72.0, 112.0);

pub fn style_for(status: &str) -> RoomStyle {
    STYLES.iter().copied().find(|s| s.status == status).unwrap_or(STYLES[STYLES.len() - 1])
}

/// Where each workshop station is in the room art, and which way a hero at
/// it faces. Typing and planning happen at the desk (the style's spot).
pub fn station(name: &str) -> Option<(Pos2, f32)> {
    match name {
        "read" => Some((pos2(53.0, 33.0), 1.0)),
        "forge" => Some((pos2(32.0, 40.0), -1.0)),
        "brew" => Some((pos2(114.0, 63.0), 1.0)),
        "gems" => Some((pos2(108.0, 38.0), 1.0)),
        "summon" => Some((pos2(72.0, 76.0), 1.0)),
        _ => None,
    }
}

/// Time left on a usage limit: 4:05, 1:13:05, or 2d 3h for weekly limits.
pub fn countdown(seconds: i64) -> String {
    let s = seconds.max(0);
    if s >= 86_400 { format!("{}d {}h", s / 86_400, s / 3600 % 24) }
    else if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) }
    else { format!("{}:{:02}", s / 60, s % 60) }
}

pub fn agent_color(status: &str) -> Color32 {
    match status {
        "working" => Color32::from_rgb(255, 159, 10),
        "blocked" => Color32::from_rgb(255, 69, 58),
        "limited" => LIMITED,
        "done" => Color32::from_rgb(48, 209, 88),
        _ => Color32::from_rgb(152, 152, 157),
    }
}

/// A small deterministic generator, so each room flickers on its own.
#[derive(Clone)]
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(seed.max(1))
    }
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() % 10_000) as f32 / 10_000.0
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u32() as usize) % n.max(1)
    }
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

fn blend(a: Color32, b: Color32, k: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

fn lerp(a: f32, b: f32, k: f32) -> f32 {
    a + (b - a) * k.clamp(0.0, 1.0)
}

fn lerp_pos(a: Pos2, b: Pos2, k: f32) -> Pos2 {
    pos2(lerp(a.x, b.x, k), lerp(a.y, b.y, k))
}

/// How tall full screen's column draws its rooms, the chosen one too.
/// Every room has art at each size: the large one, and the
/// same room 64 and 40 pixels tall, its furniture moved as blocks
/// (scripts/room_sizes.py).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomSize { Large, Medium, Small }

/// A block of a large room's art (x0, y0, x1, y1) and where it went in a
/// shorter room (dx, dy), as scripts/room_sizes.py composed it: the first
/// that holds a point moves it (src/room_moves.rs, generated).
pub type Move = (f32, f32, f32, f32, f32, f32);

impl RoomSize {
    pub const ALL: [RoomSize; 3] = [RoomSize::Large, RoomSize::Medium, RoomSize::Small];

    pub fn key(self) -> &'static str {
        match self { RoomSize::Large => "large", RoomSize::Medium => "medium", RoomSize::Small => "small" }
    }

    pub fn from_key(key: &str) -> RoomSize {
        RoomSize::ALL.into_iter().find(|s| s.key() == key).unwrap_or(RoomSize::Medium)
    }

    pub fn label(self) -> &'static str {
        match self { RoomSize::Large => "Grande", RoomSize::Medium => "Mediana", RoomSize::Small => "Chica" }
    }

    /// The room art's height in pixels (all are 144 wide).
    pub fn art_h(self) -> f32 {
        match self { RoomSize::Large => ART.y, RoomSize::Medium => 64.0, RoomSize::Small => 40.0 }
    }

    /// The bottom wall in points, which a hall's upper half drops.
    pub fn bottom_wall(self) -> f32 {
        match self { RoomSize::Small => 6.0, _ => 12.0 }
    }

    /// A room's whole cell: frame, repo band, art at 1.5 pt/px, footer.
    pub fn cell_h(self) -> f32 {
        8.0 + HEADER + self.art_h() * 1.5 + FOOTER
    }

    /// Where a point of a large room's art (the room of `state`) lands in
    /// this size's art.
    pub fn remap(self, state: &str, p: Pos2) -> Pos2 {
        if self == RoomSize::Large { return p; }
        let moves = crate::room_moves::moves(state, self);
        if let Some(&(_, _, _, _, dx, dy)) = moves.iter().find(|&&(x0, y0, x1, y1, _, _)| p.x >= x0 && p.x < x1 && p.y >= y0 && p.y < y1) {
            return pos2(p.x + dx, p.y + dy);
        }
        // Elsewhere: the back wall stays (the small room loses its middle
        // band), the floor shrinks to fit, the bottom wall moves up.
        let y = match self {
            RoomSize::Medium => if p.y < 28.0 { p.y } else if p.y < 92.0 { 28.0 + (p.y - 28.0) * 28.0 / 64.0 } else { p.y - 36.0 },
            _ => if p.y < 6.0 { p.y } else if p.y < 13.0 { 6.0 } else if p.y < 28.0 { p.y - 7.0 } else if p.y < 92.0 { 21.0 + (p.y - 28.0) * 15.0 / 64.0 } else { 36.0 + (p.y - 92.0) / 2.0 },
        };
        pos2(p.x, y)
    }
}

/// The room to open on entering full screen with none chosen: the first
/// one working, else the most pressing.
pub fn first_room(agents: &[Agent]) -> Option<String> {
    agents.iter().find(|a| a.status == "working").or_else(|| agents.iter().min_by_key(|a| a.rank())).map(|a| a.id.clone())
}

/// The spare room: an empty frame with a "+" and "Summon an agent" under
/// it. Lit while its panel is open or the pointer is over it.
fn draw_summon_room(painter: &Painter, rect: Rect, active: bool, hovered: bool) {
    let frame = rect.shrink(2.0);
    let lit = active || hovered;
    let line = if active { Color32::from_rgb(140, 217, 115) } else if hovered { Color32::from_gray(190) } else { Color32::from_gray(90) };
    painter.rect_filled(frame, CornerRadius::same(6), Color32::from_rgba_unmultiplied(255, 255, 255, if lit { 10 } else { 4 }));
    painter.rect_stroke(frame, CornerRadius::same(6), Stroke::new(if active { 2.0_f32 } else { 1.0 }, line), egui::StrokeKind::Inside);
    let ink = if lit { Color32::from_gray(230) } else { Color32::from_gray(140) };
    painter.text(frame.center() - vec2(0.0, 14.0), Align2::CENTER_CENTER, "+", FontId::monospace(56.0), ink);
    painter.text(frame.center() + vec2(0.0, 34.0), Align2::CENTER_CENTER, tr("Invocar un agente"), FontId::monospace(13.0), ink);
}

/// The spare room as a line of the full-screen column.
fn draw_summon_line(painter: &Painter, rect: Rect, hovered: bool) {
    let frame = rect.shrink2(vec2(2.0, 1.0));
    let ink = if hovered { Color32::from_gray(230) } else { Color32::from_gray(140) };
    painter.rect_filled(frame, CornerRadius::same(4), Color32::from_rgba_unmultiplied(255, 255, 255, if hovered { 10 } else { 4 }));
    painter.rect_stroke(frame, CornerRadius::same(4), Stroke::new(1.0_f32, if hovered { Color32::from_gray(190) } else { Color32::from_gray(90) }), egui::StrokeKind::Inside);
    painter.text(frame.center(), Align2::CENTER_CENTER, format!("+  {}", tr("Invocar un agente")), FontId::monospace(12.0), ink);
}

/// The spare room while it holds the summon form: lit, in summoning green.
fn draw_summon_frame(painter: &Painter, rect: Rect) {
    let frame = rect.shrink(2.0);
    painter.rect_filled(frame, CornerRadius::same(6), Color32::from_rgb(23, 20, 28));
    painter.rect_stroke(frame, CornerRadius::same(6), Stroke::new(2.0_f32, Color32::from_rgb(140, 217, 115)), egui::StrokeKind::Inside);
}

/// Draw a sprite centred at `center`, optionally mirrored and rotated.
pub fn draw_sprite(painter: &Painter, sprite: Sprite, center: Pos2, size: Vec2, flip_x: bool, angle: f32, tint: Color32) {
    draw_anchored(painter, sprite, center, size, vec2(0.5, 0.5), flip_x, angle, tint);
}

/// Draw a sprite with `anchor` (a fraction of its size) at `at`, rotated
/// around that anchor.
pub fn draw_anchored(painter: &Painter, sprite: Sprite, at: Pos2, size: Vec2, anchor: Vec2, flip_x: bool, angle: f32, tint: Color32) {
    let mut mesh = Mesh::with_texture(sprite.tex);
    let rect = Rect::from_min_size(at - vec2(size.x * anchor.x, size.y * anchor.y), size);
    let uv = if flip_x { Rect::from_min_max(pos2(sprite.uv.max.x, sprite.uv.min.y), pos2(sprite.uv.min.x, sprite.uv.max.y)) } else { sprite.uv };
    mesh.add_rect_with_uv(rect, uv, tint);
    if angle != 0.0 {
        mesh.rotate(Rot2::from_angle(angle), at);
    }
    painter.add(Shape::mesh(mesh));
}

#[derive(Clone, Copy, PartialEq)]
enum Pose {
    Idle(f32),
    Work(f32),
    Still,
}

#[derive(Clone)]
enum StepKind {
    Walk { from: Pos2, to: Pos2 },
    /// Work at a station: the pose and the particles follow the station.
    Act { station: &'static str, face: f32, pose: Pose },
    /// Read a book or a scroll at a station.
    Read { station: &'static str, scroll: bool, seconds: f32 },
    /// Keep the git ledger: set a huge tome flat on the desk, open it,
    /// write and turn its pages, then close it and carry it away.
    Git { station: &'static str },
    /// Look left and right (wandering).
    Look,
    /// Show "?" in the bubble.
    Say,
    Wait,
    /// Instant: the room changes behind the hero, who is now outside.
    Swap,
    /// Instant: the transition ends and the room's routine starts.
    Place,
}

#[derive(Clone)]
struct Step {
    kind: StepKind,
    dur: f32,
}

/// A list of steps run in order, looping from `loop_from` when set.
struct Runner {
    steps: Vec<Step>,
    loop_from: Option<usize>,
    index: usize,
    elapsed: f32,
}

impl Runner {
    fn new(steps: Vec<Step>, loop_from: Option<usize>) -> Runner {
        Runner { steps, loop_from, index: 0, elapsed: 0.0 }
    }

    /// Advance; returns the instant steps passed, in order.
    fn advance(&mut self, dt: f32) -> Vec<StepKind> {
        let mut fired = vec![];
        self.elapsed += dt;
        let mut guard = 0;
        while self.index < self.steps.len() && self.elapsed >= self.steps[self.index].dur && guard < 1000 {
            guard += 1;
            let step = &self.steps[self.index];
            if matches!(step.kind, StepKind::Swap | StepKind::Place) {
                fired.push(step.kind.clone());
            }
            self.elapsed -= step.dur;
            self.index += 1;
            if self.index >= self.steps.len() {
                match self.loop_from {
                    Some(from) => self.index = from,
                    None => { self.index = self.steps.len() - 1; self.elapsed = self.steps[self.index].dur; break; }
                }
            }
        }
        fired
    }

    fn current(&self) -> Option<(&Step, f32)> {
        self.steps.get(self.index).map(|s| (s, self.elapsed))
    }
}

#[derive(Clone, Copy)]
enum ParticleKind {
    Bit { color: Color32, size: f32 },
    Sparkle { color: Color32, size: f32 },
    Z,
    Confetti { color: Color32, drift: f32 },
}

struct Particle {
    kind: ParticleKind,
    origin: Pos2,
    delta: Vec2,
    life: f32,
    age: f32,
}

#[derive(Clone, Copy)]
enum EmitterKind {
    Station,
    Cauldron,
    Glints(&'static [Pos2], Color32, f32),
    Furnace,
    Brazier,
    Coins,
    Portal,
    Motes,
    Zs,
    Embers,
}

struct Emitter {
    kind: EmitterKind,
    every: f32,
    range: f32,
    next: f32,
    angle: f32,
}

struct Light {
    art: Pos2,
    radius: f32,
    color: Color32,
    low: f32,
    high: f32,
    pulse: Option<f32>,
    phase: f32,
    // Flicker: ease from `from` to `to` by `until`, then pick a new target.
    from: f32,
    to: f32,
    since: f32,
    until: f32,
}

struct Mini {
    /// Its own sprite: a guild member's harness, else the lead's.
    harness: &'static str,
    pos: Pos2,
    /// Where its walk to `target` started.
    from: Pos2,
    target: Pos2,
    action: String,
    facing: f32,
    front: bool,
}

const WORKING_GLINTS: [Pos2; 9] = [pos2(52.0, 12.0), pos2(88.0, 20.0), pos2(119.0, 37.0), pos2(129.0, 41.0), pos2(76.0, 46.0), pos2(114.0, 71.0), pos2(26.0, 16.0), pos2(72.0, 8.0), pos2(124.0, 12.0)];
const COINS: [Pos2; 14] = [pos2(16.0, 41.0), pos2(32.0, 66.0), pos2(46.0, 75.0), pos2(20.0, 81.0), pos2(64.0, 86.0), pos2(86.0, 89.0), pos2(116.0, 83.0), pos2(126.0, 41.0), pos2(127.0, 75.0), pos2(107.0, 46.0), pos2(26.0, 32.0), pos2(72.0, 8.0), pos2(20.0, 8.0), pos2(124.0, 8.0)];
const COT_GLINTS: [Pos2; 6] = [pos2(58.0, 47.0), pos2(70.0, 45.0), pos2(84.0, 47.0), pos2(90.0, 51.0), pos2(64.0, 43.0), pos2(78.0, 43.0)];
const RUIN_GLINTS: [Pos2; 5] = [pos2(21.0, 9.0), pos2(124.0, 9.0), pos2(112.0, 79.0), pos2(70.0, 28.0), pos2(14.0, 34.0)];

/// One agent living in a room that reflects its state. Positions inside
/// the room are in points from the cell's top-left corner.
pub struct Room {
    pub pos: Pos2,
    pub shown: Option<Pos2>,
    move_from: Pos2,
    move_at: f32,
    style: RoomStyle,
    status: String,
    harness: &'static str,
    name: String,
    activity: String,
    /// The agent's folder, and its repo's emblem (worked out again only
    /// when the folder or the user's choice for it changes).
    cwd: String,
    identity: Identity,
    /// The repo's own name and whether it is the demo's (never kept).
    repo_name: String,
    demo: bool,
    /// How tall it is drawn; the animation always runs in the large room's
    /// coordinates, and drawing moves each point to this size's art.
    pub size: RoomSize,
    branch: String,
    /// Shells the agent left running in the background: a cauldron each.
    shells: Vec<crate::shells::Shell>,
    action: Option<String>,
    subagents: usize,
    subagent_actions: Vec<String>,
    /// When the usage limit resets (unix seconds), for the countdown.
    limit_resets: Option<i64>,
    clock: f32,
    routine_clock: f32,
    in_transition: bool,
    first_room: bool,
    swap_at: Option<f32>,
    /// The transition's target style, kept until the hero is outside.
    pending_style: Option<RoomStyle>,
    runner: Option<Runner>,
    hero: Pos2,
    facing: f32,
    pose: Pose,
    frame_clock: f32,
    station: &'static str,
    bubble: Option<String>,
    particles: Vec<Particle>,
    emitters: Vec<Emitter>,
    lights: Vec<Light>,
    party: Vec<Mini>,
    party_visible: bool,
    done_last: f32,
    shuffle_at: f32,
    shuffle_period: f32,
    rng: Rng,
    subagent_harnesses: Vec<String>,
    /// A double room's other half, with its own heroes and state.
    annex: Option<Box<Room>>,
    /// Whether the other half stands beside this one (else under it).
    pub beside: bool,
    /// The state the band shows when it is not the hero's room.
    band: Option<RoomStyle>,
    /// The guild's members, when this is the orchestrator's room.
    training: bool,
    crew: Vec<Trainee>,
}

/// One of the guild's members in the orchestrator's room, in room-art pixels.
struct Trainee {
    name: String,
    harness: &'static str,
    status: String,
    action: String,
    pos: Pos2,
    target: Pos2,
    facing: f32,
    /// When its station next throws sparks.
    next_spark: f32,
}

impl Room {
    fn new(agent: &Agent, seed: u32) -> Room {
        let mut room = Room {
            pos: pos2(0.0, 0.0),
            shown: None,
            move_from: pos2(0.0, 0.0),
            move_at: -10.0,
            style: style_for("unknown"),
            status: String::new(),
            harness: harness_for(&agent.name),
            name: agent.name.clone(),
            activity: String::new(),
            cwd: String::new(),
            identity: crate::identity::identity(&ProjectMeta { key: String::new(), owner: None, name: String::new(), kind: crate::identity::Kind::Unknown }),
            size: RoomSize::Large,
            repo_name: String::new(),
            demo: false,
            branch: String::new(),
            shells: vec![],
            action: None,
            subagents: usize::MAX,
            subagent_actions: vec![],
            limit_resets: None,
            clock: 0.0,
            routine_clock: 0.0,
            in_transition: false,
            first_room: true,
            swap_at: None,
            pending_style: None,
            runner: None,
            hero: pos2(0.0, 0.0),
            facing: 1.0,
            pose: Pose::Idle(0.5),
            frame_clock: 0.0,
            station: "type",
            bubble: None,
            particles: vec![],
            emitters: vec![],
            lights: vec![],
            party: vec![],
            party_visible: true,
            done_last: 0.0,
            shuffle_at: 0.0,
            shuffle_period: 9.0,
            rng: Rng::new(seed),
            subagent_harnesses: vec![],
            annex: None,
            beside: false,
            band: None,
            training: false,
            crew: vec![],
        };
        room.update(agent, false);
        room
    }

    // ---- Geometry

    /// The room frame inside the cell.
    fn frame() -> Rect {
        Rect::from_min_size(pos2(2.0, 4.0 + HEADER), vec2(ROW_W - 4.0, ROW_H - 8.0 - HEADER - FOOTER))
    }

    /// The room frame at the size it is drawn.
    fn drawn_frame(&self) -> Rect {
        Rect::from_min_size(pos2(2.0, 4.0 + HEADER), vec2(ROW_W - 4.0, self.size.art_h() * 1.5))
    }

    /// The repo's band over the room, in cell points.
    fn header(&self) -> Rect {
        let f = self.drawn_frame();
        Rect::from_min_max(pos2(f.min.x, f.min.y - HEADER), pos2(f.max.x, f.min.y))
    }

    /// A large-room cell point where it is drawn at this room's size.
    fn place(&self, p: Pos2) -> Pos2 {
        if self.size == RoomSize::Large { return p; }
        let art = self.size.remap(self.style.status, Room::art_point(p));
        let f = self.drawn_frame();
        pos2(f.min.x + art.x * 1.5, f.min.y + art.y * 1.5)
    }

    /// Where the hero is drawn. A walk is placed at its ends and drawn
    /// straight between them: placing each point on the way would jump the
    /// hero back and forth wherever it crosses from one block of a medium or
    /// small room's furniture into another.
    fn place_hero(&self, hero: Pos2) -> Pos2 {
        match self.runner.as_ref().and_then(|r| r.current()) {
            Some((Step { kind: StepKind::Walk { from, to }, dur }, e)) if self.size != RoomSize::Large => {
                let k = e / dur.max(0.001);
                let base = lerp_pos(Room::rp(*from), Room::rp(*to), k);
                lerp_pos(self.place(Room::rp(*from)), self.place(Room::rp(*to)), k) + (hero - base)
            }
            _ => self.place(hero),
        }
    }

    /// The room's art at the size it is drawn.
    fn background(&self, style: RoomStyle) -> String {
        match self.size {
            RoomSize::Large => style.background.to_string(),
            size => format!("{}_{}", style.background, size.key()),
        }
    }

    /// Room-art pixel (origin top-left) to cell points, matching how the
    /// backdrop is aspect-filled and anchored right.
    fn rp(art: Pos2) -> Pos2 {
        let f = Room::frame();
        let scale = (f.width() / ART.x).max(f.height() / ART.y);
        pos2(f.max.x - (ART.x - art.x) * scale, f.center().y + (art.y - ART.y / 2.0) * scale)
    }

    /// Room-art pixel of a cell point (inverse of `rp`).
    fn art_point(p: Pos2) -> Pos2 {
        let f = Room::frame();
        let scale = (f.width() / ART.x).max(f.height() / ART.y);
        pos2(ART.x - (f.max.x - p.x) / scale, ART.y / 2.0 + (p.y - f.center().y) / scale)
    }

    // ---- Updates from the agent

    /// Apply the agent's latest data. If the status changed, swap the room.
    pub fn update(&mut self, agent: &Agent, animated: bool) {
        self.name = agent.name.clone();
        self.activity = agent.activity.clone();
        self.branch = agent.branch.clone().unwrap_or_default();
        self.shells = agent.shells.clone();
        self.harness = harness_for(&agent.name);
        self.limit_resets = agent.limit_resets;
        // The hero's own state picks its room; the band shows the whole's.
        let status = agent.hero_status.as_ref().unwrap_or(&agent.status);
        self.band = agent.hero_status.is_some().then(|| style_for(&agent.status));
        // The guild's room is always the workroom, its hero the director at
        // the desk: its state shows on it, never as another room.
        let guild = agent.guild_room || !agent.crew.is_empty();
        let action = if guild { Some("type".to_string()) } else { agent.action.clone() };
        if action != self.action {
            self.action = action;
            if self.style.status == "working" && (status == "working" || guild) && !self.in_transition {
                self.restart_working();
            }
        }
        let room_status = if guild { "working" } else { status.as_str() };
        if *status != self.status || guild != self.training {
            self.status = status.clone();
            if self.style.status != room_status || guild != self.training { self.apply_room(style_for(room_status), animated); }
        }
        if agent.subagents != self.subagents || agent.subagent_actions != self.subagent_actions || agent.subagent_harnesses != self.subagent_harnesses {
            self.subagents = agent.subagents;
            self.subagent_actions = agent.subagent_actions.clone();
            self.subagent_harnesses = agent.subagent_harnesses.clone();
            self.set_party();
        }
        self.training = guild;
        if self.training {
            self.band = Some(style_for(&agent.status));
            self.set_crew(&agent.crew, animated);
        }
        match (&agent.annex, &mut self.annex) {
            (Some(other), Some(annex)) => annex.update(other, animated),
            (Some(other), None) => self.annex = Some(Box::new(Room::new(other, self.rng.0.wrapping_add(17)))),
            (None, _) => self.annex = None,
        }
    }

    /// A status change: the hero gets up if asleep, walks out through the
    /// door, the room changes behind it, and it walks back in to its new
    /// spot (around the desk, to the bed) before starting that room's routine.
    fn apply_room(&mut self, next: RoomStyle, animated: bool) {
        if !animated || self.first_room {
            self.in_transition = false;
            self.swap_room(next, animated, true);
            return;
        }
        self.in_transition = true;
        self.particles.clear();
        self.emitters.clear();
        self.lights.clear();
        self.bubble = None;
        self.party_visible = false;
        let mut walker = Room::art_point(self.hero);
        if self.style.lying {
            walker = pos2(52.0, 70.0);
            self.hero = Room::rp(walker);
        }
        let mut steps = vec![];
        let old = self.style;
        for p in old.route.iter().rev() {
            if !old.lying || p.y > 71.0 { steps.push(walk(&mut walker, *p, TRANSITION_SPEED)); }
        }
        steps.push(walk(&mut walker, DOOR, TRANSITION_SPEED));
        steps.push(walk(&mut walker, OUTSIDE, TRANSITION_SPEED));
        steps.push(Step { kind: StepKind::Swap, dur: 0.0 });
        walker = OUTSIDE;
        steps.push(walk(&mut walker, DOOR, TRANSITION_SPEED));
        for p in next.route { steps.push(walk(&mut walker, *p, TRANSITION_SPEED)); }
        if !next.lying { steps.push(walk(&mut walker, next.spot, TRANSITION_SPEED)); }
        steps.push(Step { kind: StepKind::Place, dur: 0.0 });
        steps.push(Step { kind: StepKind::Wait, dur: 1.0 });
        self.pending_style = Some(next);
        self.runner = Some(Runner::new(steps, None));
    }

    fn swap_room(&mut self, next: RoomStyle, animated: bool, routine: bool) {
        self.style = next;
        self.first_room = false;
        if animated { self.swap_at = Some(self.clock); }
        if routine {
            self.place_actors();
        }
        self.set_party();
        if !routine { self.party_visible = false; }
    }

    /// Put the hero on its spot and start that room's motion: wander, hop,
    /// work at the desk, or sleep in bed.
    fn place_actors(&mut self) {
        self.hero = Room::rp(self.style.spot);
        self.facing = 1.0;
        self.particles.clear();
        self.emitters.clear();
        self.lights.clear();
        self.runner = None;
        self.routine_clock = 0.0;
        self.frame_clock = 0.0;
        self.bubble = self.style.bubble.filter(|_| !self.style.lying).map(str::to_string);
        self.in_transition = false;
        self.party_visible = true;
        self.pose = Pose::Idle(0.5);
        self.station = "type";
        match self.style.status {
            "working" => { let from = self.style.spot; self.start_working(from); }
            "blocked" => self.start_blocked(),
            "limited" => self.start_recovering(),
            "done" => self.start_done(),
            "idle" => self.start_reading(),
            _ => self.start_wandering(),
        }
    }

    /// The working agent's action changed: walk from wherever the hero is
    /// to the new station.
    fn restart_working(&mut self) {
        self.particles.clear();
        self.emitters.clear();
        self.lights.clear();
        let from = Room::art_point(self.hero);
        self.start_working(from);
    }

    fn working_activity(name: &str) -> Step {
        match name {
            "read" => Step { kind: StepKind::Read { station: "read", scroll: false, seconds: 3.6 }, dur: 4.6 },
            "plan" => Step { kind: StepKind::Read { station: "plan", scroll: true, seconds: 3.6 }, dur: 4.6 },
            "git" => Step { kind: StepKind::Git { station: "git" }, dur: 7.2 },
            "forge" => Step { kind: StepKind::Act { station: "forge", face: -1.0, pose: Pose::Work(0.16) }, dur: 3.0 },
            "brew" => Step { kind: StepKind::Act { station: "brew", face: 1.0, pose: Pose::Work(0.2) }, dur: 3.4 },
            "gems" => Step { kind: StepKind::Act { station: "gems", face: 1.0, pose: Pose::Work(0.22) }, dur: 2.0 },
            "summon" => Step { kind: StepKind::Act { station: "summon", face: 1.0, pose: Pose::Work(0.09) }, dur: 3.0 },
            _ => Step { kind: StepKind::Act { station: "type", face: 1.0, pose: Pose::Work(0.12) }, dur: 4.0 },
        }
    }

    /// Workshop. With a known action the hero goes to its station and keeps
    /// at it: reading a book (Read, Grep, web search), forging at the anvil
    /// (edits), brewing at the alchemy table (shell), summoning in a magic
    /// circle (subagents), studying a scroll (plans and todos) or typing at
    /// the laptop (thinking, writing). Without one it makes the rounds.
    fn start_working(&mut self, from: Pos2) {
        let spot = self.style.spot;
        let station_of = |name: &str| -> Pos2 {
            match name { "type" | "plan" | "git" => spot, other => station(other).map(|s| s.0).unwrap_or(spot) }
        };
        let mut walker = from;
        let mut steps = vec![];
        let runner = if let Some(action) = self.action.clone() {
            steps.extend(go(&mut walker, station_of(&action)));
            let act = Room::working_activity(&action);
            let loop_from = steps.len();
            steps.push(act);
            Runner::new(steps, Some(loop_from))
        } else {
            steps.extend(go(&mut walker, spot));
            steps.push(Room::working_activity("type"));
            for name in ["read", "forge", "brew", "gems"] { steps.extend(go(&mut walker, station_of(name))); steps.push(Room::working_activity(name)); }
            steps.extend(go(&mut walker, spot));
            // The first lap starts from wherever the hero stands; later laps loop from the desk.
            let loop_from = steps.len();
            steps.push(Room::working_activity("type"));
            for name in ["read", "forge", "brew", "gems"] { steps.extend(go(&mut walker, station_of(name))); steps.push(Room::working_activity(name)); }
            steps.extend(go(&mut walker, spot));
            Runner::new(steps, Some(loop_from))
        };
        self.runner = Some(runner);
        self.light(pos2(26.0, 18.0), 26.0, ORANGE, 0.08, 0.2, None);
        self.light(pos2(72.0, 47.0), 16.0, CYAN, 0.06, 0.16, None);
        self.light(pos2(124.0, 40.0), 14.0, CYAN, 0.05, 0.18, Some(1.1));
        self.light(pos2(93.0, 72.0), 12.0, GREEN, 0.06, 0.2, Some(0.8));
        self.emit(EmitterKind::Station, 0.14, 0.0);
        self.emit(EmitterKind::Cauldron, 0.3, 0.2);
        self.emit(EmitterKind::Glints(&WORKING_GLINTS, Color32::WHITE, 9.0), 0.45, 0.3);
        self.emit(EmitterKind::Furnace, 0.2, 0.15);
    }

    /// Needs attention: the hero keeps jumping on the rug under a huge
    /// pulsing red "!", the room throbs with red light and the border blinks.
    fn start_blocked(&mut self) {
        self.pose = Pose::Still;
        self.light(pos2(33.0, 33.0), 12.0, ORANGE, 0.1, 0.28, None);
        self.light(pos2(109.0, 33.0), 12.0, ORANGE, 0.1, 0.28, None);
        self.emit(EmitterKind::Brazier, 0.25, 0.15);
    }

    /// The usage limit: the hero rests on a cot in the infirmary until it
    /// resets, sleeping it off (the blanket breathes, z's drift up), under
    /// the braziers' warm light, with healing sparkles over the cot and the
    /// time left to recover on a tag at its foot.
    fn start_recovering(&mut self) {
        self.pose = Pose::Still;
        self.shuffle_period = self.rng.range(9.0, 13.0);
        self.shuffle_at = self.shuffle_period;
        self.light(pos2(33.0, 33.0), 12.0, ORANGE, 0.12, 0.3, None);
        self.light(pos2(109.0, 33.0), 12.0, ORANGE, 0.12, 0.3, None);
        self.light(pos2(72.0, 58.0), 26.0, Color32::WHITE, 0.04, 0.12, Some(3.6));
        self.emit(EmitterKind::Brazier, 0.3, 0.15);
        self.emit(EmitterKind::Zs, 1.4, 0.0);
        self.emit(EmitterKind::Glints(&COT_GLINTS, MINT, 8.0), 0.8, 0.3);
    }

    /// Done: the hero jumps for joy in the middle of the treasure room,
    /// arms up, under a speech bubble with a green tick, while confetti
    /// flies and the gold glints.
    fn start_done(&mut self) {
        self.pose = Pose::Work(0.1);
        self.done_last = 0.0;
        self.emit(EmitterKind::Coins, 0.12, 0.08);
        for x in [28.0, 72.0, 116.0] {
            let pulse = self.rng.range(1.2, 1.8);
            self.light(pos2(x, 40.0), 22.0, GOLD, 0.04, 0.14, Some(pulse));
        }
    }

    /// Unknown: wander the ruins looking left and right; the portal swirls,
    /// the orb pulses and motes drift up from it.
    fn start_wandering(&mut self) {
        let c = self.style.spot;
        let r = self.style.roam;
        let mut walker = c;
        let steps = vec![
            walk(&mut walker, pos2(c.x + r, c.y + 4.0), 18.0),
            Step { kind: StepKind::Look, dur: 1.4 },
            walk(&mut walker, pos2(c.x - r, c.y - 6.0), 18.0),
            Step { kind: StepKind::Say, dur: 1.0 },
            walk(&mut walker, pos2(c.x - 4.0, c.y + 12.0), 18.0),
            Step { kind: StepKind::Look, dur: 1.4 },
            walk(&mut walker, c, 18.0),
            Step { kind: StepKind::Wait, dur: 0.8 },
        ];
        self.runner = Some(Runner::new(steps, Some(0)));
        self.light(pos2(72.0, 20.0), 26.0, VIOLET, 0.08, 0.24, Some(1.6));
        self.light(pos2(72.0, 30.0), 12.0, CYAN, 0.1, 0.3, Some(0.9));
        self.emit(EmitterKind::Portal, 0.09, 0.0);
        self.emit(EmitterKind::Motes, 0.35, 0.2);
        self.emit(EmitterKind::Glints(&RUIN_GLINTS, CYAN, 9.0), 0.5, 0.3);
    }

    /// Waiting in its chamber: the hero reads a book by the fireplace,
    /// turning a page now and then, while the fire throws a flickering warm
    /// light with embers.
    fn start_reading(&mut self) {
        self.pose = Pose::Idle(0.9);
        self.emit(EmitterKind::Embers, 0.25, 0.2);
    }

    fn light(&mut self, art: Pos2, radius: f32, color: Color32, low: f32, high: f32, pulse: Option<f32>) {
        let phase = self.rng.range(0.0, 6.28);
        self.lights.push(Light { art, radius, color, low, high, pulse, phase, from: low, to: low, since: 0.0, until: 0.0 });
    }

    fn emit(&mut self, kind: EmitterKind, every: f32, range: f32) {
        self.emitters.push(Emitter { kind, every, range, next: self.routine_clock, angle: 0.0 });
    }

    /// Active subagents are small heroes of the same harness, each at the
    /// station of its own last tool (read at the shelf, brew at the alchemy
    /// table…), a step darker so the lead hero stays the clearest shape.
    /// Several at one station stand side by side; one with no known tool
    /// waits on the free floor in front of the desk. When a subagent changes
    /// tool it walks to the new station.
    fn set_party(&mut self) {
        let count = self.subagents.min(4);
        self.party.truncate(count);
        if count == 0 { return; }
        // The lead hero already stands at its own station: start beside it.
        let mut taken: HashMap<String, usize> = HashMap::new();
        if let Some(action) = &self.action { taken.insert(action.clone(), 1); }
        for i in 0..count {
            let action = self.subagent_actions.get(i).cloned().unwrap_or_else(|| "idle".into());
            let at_desk = action == "type" || action == "plan";
            let found = station(&action);
            // Typing and planning share the desk with the lead hero: stand beside it.
            let base = found.map(|s| s.0).unwrap_or(if at_desk { self.style.spot } else { pos2(72.0, 88.0) });
            let k = *taken.get(&action).unwrap_or(&0);
            taken.insert(action.clone(), k + 1);
            let nudge: [f32; 4] = if at_desk { [-18.0, 18.0, -30.0, 30.0] } else { [0.0, 11.0, -11.0, 22.0] };
            let n = nudge[k % 4];
            let art = pos2(base.x + n, base.y + if k > 0 { 2.0 } else { 0.0 });
            let home = Room::rp(art);
            let facing = found.map(|s| s.1).unwrap_or(1.0) * if at_desk { if n < 0.0 { 1.0 } else { -1.0 } } else { 1.0 };
            let front = art.y > self.style.spot.y;
            let harness = self.subagent_harnesses.get(i).map(|h| harness_for(h)).unwrap_or(self.harness);
            if let Some(mini) = self.party.get_mut(i) {
                if mini.target != home { mini.from = mini.pos; }
                mini.harness = harness;
                mini.target = home;
                mini.action = action;
                mini.facing = facing;
                mini.front = front;
            } else {
                self.party.push(Mini { harness, pos: home, from: home, target: home, action, facing, front });
            }
        }
    }

    // ---- Per-frame animation

    pub fn advance(&mut self, dt: f32) {
        if dt <= 0.0 { return; }
        if let Some(annex) = &mut self.annex { annex.size = self.size; annex.advance(dt); }
        for t in &mut self.crew {
            let d = t.target - t.pos;
            let dist = d.length();
            if dist > 0.3 { let step = (34.0 * dt).min(dist); t.pos += d / dist * step; if d.x.abs() > 0.5 { t.facing = d.x.signum(); } } else { t.pos = t.target; }
        }
        self.clock += dt;
        self.frame_clock += dt;
        if !self.in_transition { self.routine_clock += dt; }
        // Minis walk to their stations.
        for mini in &mut self.party {
            let d = mini.target - mini.pos;
            let dist = d.length();
            if dist > 0.5 {
                let step = (50.0 * dt).min(dist);
                mini.pos += d / dist * step;
            } else {
                mini.pos = mini.target;
            }
        }
        let fired = self.runner.as_mut().map(|r| r.advance(dt)).unwrap_or_default();
        for kind in fired {
            match kind {
                StepKind::Swap => {
                    if let Some(next) = self.pending_style.take() {
                        self.swap_room(next, true, false);
                        self.hero = Room::rp(OUTSIDE);
                    }
                }
                StepKind::Place => {
                    self.in_transition = false;
                    self.place_actors();
                    self.party_visible = true;
                    return;
                }
                _ => {}
            }
        }
        self.pose_from_runner();
        self.update_fx(dt);
    }

    /// Hero position, facing and pose from the current step.
    fn pose_from_runner(&mut self) {
        let Some(runner) = &self.runner else { return };
        let Some((step, e)) = runner.current() else { return };
        let dur = step.dur.max(0.001);
        match &step.kind {
            StepKind::Walk { from, to } => {
                let base = lerp_pos(Room::rp(*from), Room::rp(*to), e / dur);
                // A little bob with each step.
                let bob = ((e / 0.12) as i32 % 2) as f32;
                let phase = (e % 0.12) / 0.12;
                let y = if bob == 0.0 { -2.0 * phase } else { -2.0 * (1.0 - phase) };
                self.hero = pos2(base.x, base.y + y);
                let dx = to.x - from.x;
                if dx.abs() > 1.0 { self.facing = if dx < 0.0 { -1.0 } else { 1.0 }; }
                self.set_pose(Pose::Idle(0.12));
                self.station = "walk";
            }
            StepKind::Act { station, face, pose } => {
                self.station = station;
                self.facing = *face;
                self.set_pose(*pose);
            }
            StepKind::Read { station, .. } => {
                self.station = station;
                self.facing = 1.0;
                self.set_pose(Pose::Idle(0.6));
            }
            StepKind::Git { station } => {
                // Facing the table. The body stays calm (idle breathing) —
                // the writing is carried by the drawn arm and quill over the
                // ledger, not the harness's generic work frames.
                self.station = station;
                self.facing = 1.0;
                self.set_pose(Pose::Idle(0.5));
            }
            StepKind::Look => {
                self.facing = if e < 0.5 { -1.0 } else if e < 1.0 { 1.0 } else { -1.0 };
                self.set_pose(Pose::Idle(0.5));
            }
            StepKind::Say => {
                self.set_pose(Pose::Idle(0.5));
            }
            StepKind::Wait => {}
            StepKind::Swap | StepKind::Place => {}
        }
    }

    fn set_pose(&mut self, pose: Pose) {
        if self.pose != pose {
            self.pose = pose;
            self.frame_clock = 0.0;
        }
    }

    /// Where the hero is drawn this frame, with the routine's own motion
    /// (hops, reading bobs) added, plus its sprite and the prop it holds.
    fn hero_view(&self) -> (Pos2, Option<&'static str>, Option<(&'static str, Vec2, Pos2)>) {
        let rt = self.routine_clock;
        let mut pos = self.hero;
        let mut book: Option<(&'static str, Vec2, Pos2)> = None;
        if !self.in_transition {
            match self.style.status {
                "blocked" => {
                    let p = rt % 0.36;
                    pos.y += if p < 0.16 { -12.0 * (p / 0.16) } else if p < 0.30 { -12.0 * (1.0 - (p - 0.16) / 0.14) } else { 0.0 };
                }
                "done" => {
                    let p = rt % 2.21;
                    let hop = |q: f32| if q < 0.18 { -14.0 * (q / 0.18) } else if q < 0.34 { -14.0 * (1.0 - (q - 0.18) / 0.16) } else { 0.0 };
                    let small = |q: f32| if q < 0.1 { -6.0 * (q / 0.1) } else if q < 0.2 { -6.0 * (1.0 - (q - 0.1) / 0.1) } else { 0.0 };
                    pos.y += if p < 0.42 { hop(p) } else if p < 0.84 { hop(p - 0.42) } else if p < 1.14 { small(p - 0.84) } else if p < 1.44 { small(p - 1.14) } else if p < 1.86 { hop(p - 1.44) } else { 0.0 };
                }
                "idle" if self.runner.is_none() => {
                    // The hero sits still; every few seconds the book alone
                    // lifts a little and a page turns. Held out before it,
                    // toward the window.
                    let p = rt % 4.2;
                    let turning = p > 3.8;
                    book = Some((if turning { "book_open_flipped" } else { "book_open" }, vec2(14.0, 9.0), pos2(-11.0, if turning { 0.0 } else { 2.0 })));
                }
                _ => {}
            }
        }
        if let Some(runner) = &self.runner {
            if let Some((step, e)) = runner.current() {
                if let StepKind::Read { scroll, seconds, .. } = step.kind {
                    let end = seconds + 1.0;
                    let bob = |q: f32| -3.0 * (std::f32::consts::PI * q / 0.3).sin();
                    if e < 0.3 { pos.y += bob(e); }
                    else if e < 0.7 { book = Some((if scroll { "scroll_closed" } else { "book_closed" }, vec2(10.0, 10.0), pos2(0.0, 5.0))); }
                    else if e < end - 0.3 {
                        let flipped = (((e - 0.7) / 0.9) as i32) % 2 == 1;
                        let name = match (scroll, flipped) { (true, false) => "scroll_open", (true, true) => "scroll_open_flipped", (false, false) => "book_open", (false, true) => "book_open_flipped" };
                        book = Some((name, vec2(18.0, 12.0), pos2(0.0, 5.0)));
                    } else { pos.y += bob(e - (end - 0.3)); }
                }
            }
        }
        (pos, None, book)
    }

    /// The huge git ledger while the hero keeps it, as (frame of the
    /// ledger strip, how far it is raised off the desk in points). It lies
    /// flat on the desktop in front of the hero: set down shut, opened, then
    /// written in stroke by stroke, a page turned whenever two lines are
    /// done, and at last shut and lifted away. The strip's arm frames carry
    /// the quill, so the arm moves with the frame.
    fn git_tome(&self) -> Option<(usize, f32)> {
        if self.in_transition { return None; }
        let runner = self.runner.as_ref()?;
        let (step, e) = runner.current()?;
        let StepKind::Git { .. } = step.kind else { return None };
        let dur = step.dur.max(0.001);
        let lift = 10.0; // how far it is raised while carried on or off
        // Timeline over `dur` (default 7.2 s).
        let open_at = 1.2; // tome placed and swung open
        let close_at = dur - 1.8; // writing done, book shut
        let away_at = dur - 1.0; // lifted off the table again
        if e < open_at {
            // Lower the shut tome onto the desk from a little above it.
            let q = (e / open_at).clamp(0.0, 1.0);
            Some((0, -lift * (1.0 - q)))
        } else if e < close_at {
            // Open: a breath over the fresh spread, six strokes of the quill
            // (two lines on the right-hand page), then the written leaf is
            // lifted, stood up over the spine and laid down on the left.
            const BEATS: [(usize, f32); 11] = [(1, 0.3), (2, 0.22), (3, 0.2), (4, 0.26), (5, 0.22), (6, 0.2), (7, 0.3), (8, 0.11), (9, 0.11), (10, 0.11), (11, 0.14)];
            let cycle: f32 = BEATS.iter().map(|b| b.1).sum();
            let mut p = (e - open_at) % cycle;
            for (frame, beat) in BEATS {
                if p < beat { return Some((frame, 0.0)); }
                p -= beat;
            }
            Some((1, 0.0))
        } else if e < away_at {
            // Shut it, resting a moment on the desk.
            Some((0, 0.0))
        } else {
            // Lift it back up off the desk.
            let q = ((e - away_at) / (dur - away_at)).clamp(0.0, 1.0);
            Some((0, -lift * q))
        }
    }

    fn update_fx(&mut self, dt: f32) {
        let t = self.routine_clock;
        // Particles age and die.
        for p in &mut self.particles { p.age += dt; }
        self.particles.retain(|p| p.age < p.life);
        if self.in_transition { return; }
        // Lights flicker or pulse.
        for light in &mut self.lights {
            if light.pulse.is_none() && t >= light.until {
                light.from = light.to;
                light.to = light.low + (light.high - light.low) * ((self.rng.next_u32() % 1000) as f32 / 1000.0);
                light.since = t;
                light.until = t + 0.1 + ((self.rng.next_u32() % 1000) as f32 / 1000.0) * 0.35;
            }
        }
        // Emitters spawn.
        let mut spawned: Vec<Particle> = vec![];
        let hero = self.hero_view().0;
        let frame = Room::frame();
        let mut emitters = std::mem::take(&mut self.emitters);
        for emitter in &mut emitters {
            while t >= emitter.next {
                emitter.next += emitter.every + self.rng.range(-emitter.range, emitter.range).max(-emitter.every * 0.9);
                match emitter.kind {
                    EmitterKind::Station if self.training && self.status != "working" => {}
                    EmitterKind::Station => match self.station {
                        "type" => {
                            let laptop = Room::rp(pos2(72.0, 46.0));
                            let dx = self.rng.range(-8.0, 8.0);
                            spawned.push(bit(&mut self.rng, pos2(laptop.x + dx, laptop.y - 4.0), &[CYAN, Color32::WHITE, GREEN], 6.0, 14.0, 24.0, 2.0, 0.9));
                        }
                        "read" | "plan" => {
                            if self.rng.below(3) == 0 { spawned.push(bit(&mut self.rng, hero, &[Color32::WHITE, GOLD], 8.0, 8.0, 16.0, 2.0, 0.8)); }
                        }
                        "forge" => {
                            let anvil = Room::rp(pos2(20.0, 41.0));
                            for _ in 0..3 { spawned.push(bit(&mut self.rng, anvil, &[GOLD, ORANGE, Color32::WHITE], 16.0, 4.0, 16.0, 2.0, 0.45)); }
                        }
                        "brew" => {
                            let potions = [RED, GREEN, CYAN, VIOLET];
                            let x = self.rng.pick(&[105.0, 110.0, 115.0, 121.0, 125.0]);
                            let flask = Room::rp(pos2(x, 68.0));
                            let color = self.rng.pick(&potions);
                            spawned.push(bit(&mut self.rng, flask, &[color], 3.0, 10.0, 20.0, 3.0, 0.9));
                            if self.rng.below(4) == 0 {
                                let color = self.rng.pick(&potions);
                                let at = pos2(self.rng.range(103.0, 126.0), 66.0);
                                spawned.push(sparkle(&mut self.rng, Room::rp(at), color, 10.0, 3.0));
                            }
                        }
                        "gems" => {
                            let at = pos2(self.rng.range(116.0, 130.0), self.rng.range(34.0, 44.0));
                            spawned.push(sparkle(&mut self.rng, Room::rp(at), CYAN, 12.0, 3.0));
                        }
                        "summon" => {
                            // A ring of violet light turning around the hero's feet, motes rising out of it.
                            for k in 0..3 {
                                let a = emitter.angle + k as f32 * 2.1;
                                spawned.push(sparkle(&mut self.rng, Room::rp(pos2(72.0 + a.cos() * 16.0, 86.0 + a.sin() * 5.0)), VIOLET, 9.0, 0.0));
                            }
                            emitter.angle += 0.5;
                            let at = Room::rp(pos2(self.rng.range(58.0, 86.0), 86.0));
                            spawned.push(bit(&mut self.rng, at, &[VIOLET, Color32::WHITE], 2.0, 14.0, 26.0, 2.0, 0.9));
                        }
                        _ => {}
                    },
                    EmitterKind::Cauldron => {
                        let c = Room::rp(pos2(93.0, 71.0));
                        let dx = self.rng.range(-5.0, 5.0);
                        spawned.push(bit(&mut self.rng, pos2(c.x + dx, c.y), &[GREEN, CYAN], 2.0, 6.0, 12.0, 2.0, 0.7));
                    }
                    EmitterKind::Glints(points, color, size) => {
                        let at = self.rng.pick(points);
                        spawned.push(sparkle(&mut self.rng, Room::rp(at), color, size, 3.0));
                    }
                    EmitterKind::Furnace => {
                        let f = Room::rp(pos2(26.0, 16.0));
                        let dx = self.rng.range(-10.0, 10.0);
                        spawned.push(bit(&mut self.rng, pos2(f.x + dx, f.y), &[GOLD, ORANGE], 4.0, 8.0, 14.0, 2.0, 0.9));
                    }
                    EmitterKind::Brazier => {
                        let at = if self.rng.below(2) == 0 { pos2(33.0, 31.0) } else { pos2(109.0, 31.0) };
                        spawned.push(bit(&mut self.rng, Room::rp(at), &[GOLD, ORANGE], 3.0, 6.0, 12.0, 2.0, 0.8));
                    }
                    EmitterKind::Coins => {
                        let at = self.rng.pick(&COINS);
                        spawned.push(sparkle(&mut self.rng, Room::rp(at), Color32::WHITE, 10.0, 3.0));
                    }
                    EmitterKind::Portal => {
                        emitter.angle += 0.7;
                        let portal = Room::rp(pos2(72.0, 20.0));
                        let at = pos2(portal.x + emitter.angle.cos() * 18.0, portal.y + emitter.angle.sin() * 12.0);
                        spawned.push(bit(&mut self.rng, at, &[VIOLET, PINK, Color32::WHITE], 2.0, -2.0, 2.0, 2.0, 0.6));
                    }
                    EmitterKind::Motes => {
                        let at = Room::rp(pos2(self.rng.range(66.0, 78.0), 28.0));
                        spawned.push(bit(&mut self.rng, at, &[CYAN, Color32::WHITE], 6.0, 10.0, 20.0, 2.0, 1.2));
                    }
                    EmitterKind::Zs => {
                        spawned.push(Particle { kind: ParticleKind::Z, origin: Room::rp(self.style.spot), delta: Vec2::ZERO, life: 2.8, age: 0.0 });
                    }
                    EmitterKind::Embers => {
                        let hearth = Room::rp(HEARTH);
                        let color = self.rng.pick(&[GOLD, ORANGE]);
                        let dx = self.rng.range(-4.0, 4.0);
                        let dy = -self.rng.range(8.0, 14.0);
                        spawned.push(Particle { kind: ParticleKind::Bit { color, size: 2.0 }, origin: pos2(hearth.x + self.rng.range(-7.0, 7.0), hearth.y), delta: vec2(dx, dy), life: 0.9, age: 0.0 });
                    }
                }
            }
        }
        self.emitters = emitters;
        // Confetti bursts on the done room's beat.
        if self.style.status == "done" {
            let period = 2.21;
            let (last, now) = (self.done_last % period, t % period);
            for beat in [0.0, 0.42, 1.44] {
                let crossed = if now >= last { last <= beat && beat < now } else { beat >= last || beat < now };
                if crossed {
                    for _ in 0..3 {
                        let color = self.rng.pick(&[GOLD, Color32::WHITE, RED, CYAN]);
                        let at = pos2(hero.x + self.rng.range(-14.0, 14.0), hero.y - HERO / 2.0 - self.rng.range(2.0, 10.0));
                        let drift = self.rng.range(-18.0, 18.0);
                        spawned.push(Particle { kind: ParticleKind::Confetti { color, drift }, origin: at, delta: vec2(drift, 34.0), life: 1.1, age: 0.0 });
                    }
                }
            }
            self.done_last = t;
        }
        // The guild's members at work: each station throws its own sparks.
        if self.training {
            for c in self.crew.iter_mut() {
                if c.status != "working" || (c.target - c.pos).length() > 0.3 || t < c.next_spark { continue; }
                c.next_spark = t + 0.16 + self.rng.range(0.0, 0.12);
                let at = Room::rp(c.pos);
                match c.action.as_str() {
                    "forge" => { let anvil = Room::rp(pos2(20.0, 41.0)); for _ in 0..2 { spawned.push(bit(&mut self.rng, anvil, &[GOLD, ORANGE, Color32::WHITE], 16.0, 4.0, 16.0, 2.0, 0.45)); } }
                    "brew" => {
                        let colors = [RED, GREEN, CYAN, VIOLET];
                        let color = self.rng.pick(&colors);
                        let dx = self.rng.range(-8.0, 8.0);
                        spawned.push(bit(&mut self.rng, pos2(at.x + dx, at.y - 4.0), &[color], 3.0, 10.0, 20.0, 3.0, 0.9));
                        if self.rng.below(3) == 0 { let color = self.rng.pick(&colors); spawned.push(sparkle(&mut self.rng, pos2(at.x, at.y - 6.0), color, 10.0, 6.0)); }
                    }
                    "gems" => spawned.push(sparkle(&mut self.rng, pos2(at.x + 10.0, at.y - 4.0), CYAN, 12.0, 6.0)),
                    "summon" => { let dx = self.rng.range(-10.0, 10.0); spawned.push(sparkle(&mut self.rng, pos2(at.x + dx, at.y + 8.0), VIOLET, 9.0, 2.0)) }
                    "read" | "plan" => if self.rng.below(2) == 0 { spawned.push(bit(&mut self.rng, at, &[Color32::WHITE, GOLD], 8.0, 8.0, 16.0, 2.0, 0.8)); },
                    _ => spawned.push(bit(&mut self.rng, pos2(at.x, at.y - 4.0), &[CYAN, Color32::WHITE, GREEN], 6.0, 10.0, 20.0, 2.0, 0.8)),
                }
            }
        }
        self.particles.extend(spawned);
        let _ = frame;
    }

    // ---- Drawing

    fn clip(text: &str, chars: usize) -> String {
        let count = text.chars().count();
        if count > chars { text.chars().take(chars.saturating_sub(1).max(1)).collect::<String>() + "…" } else { text.to_string() }
    }

    fn frame_index(&self) -> (&'static str, usize) {
        match self.pose {
            Pose::Idle(pace) => ("idle", (self.frame_clock / pace.max(0.01)) as usize % 2),
            Pose::Work(pace) => ("work", (self.frame_clock / pace.max(0.01)) as usize % 3),
            Pose::Still => ("idle", 0),
        }
    }

    fn light_alpha(light: &Light, t: f32) -> f32 {
        match light.pulse {
            Some(period) => {
                let k = 0.5 - 0.5 * (std::f32::consts::PI * (t + light.phase) / period).cos();
                light.low + (light.high - light.low) * k
            }
            None => {
                let k = ((t - light.since) / (light.until - light.since).max(0.01)).clamp(0.0, 1.0);
                light.from + (light.to - light.from) * k
            }
        }
    }

    /// Draw the room at `origin` (the cell's top-left corner).
    /// Where a double room's other half is drawn, from this half's origin:
    /// beside it, or under it in a column too narrow for two.
    pub fn annex_offset(&self) -> Vec2 {
        if self.beside { vec2(ROW_W - 4.0 - 2.0 * SIDE_WALL, 0.0) } else { vec2(0.0, self.size.art_h() * 1.5 - self.size.bottom_wall()) }
    }

    /// How much of each half's frame the joint hides, (left, right,
    /// bottom): the walls between them, so the halves read as one hall.
    fn trims(&self) -> ((f32, f32, f32), (f32, f32, f32)) {
        if self.beside { ((0.0, SIDE_WALL, 0.0), (SIDE_WALL, 0.0, 0.0)) } else { ((0.0, 0.0, self.size.bottom_wall()), (0.0, 0.0, 0.0)) }
    }

    /// The whole room: its art, and the band around it (the repo's header,
    /// the footer). A double room draws both halves' art in one frame.
    pub fn draw(&self, painter: &Painter, origin: Pos2, assets: &Assets, selected: bool) {
        let trim = |f: Rect, (l, r, b): (f32, f32, f32)| Rect::from_min_max(pos2(f.min.x + l, f.min.y), pos2(f.max.x - r, f.max.y - b));
        let (mine, theirs) = if self.annex.is_some() { self.trims() } else { ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0)) };
        let mut art = trim(self.drawn_frame().translate(origin.to_vec2()), mine);
        let other = origin + self.annex_offset();
        let annex_art = self.annex.as_ref().map(|a| trim(a.drawn_frame().translate(other.to_vec2()), theirs));
        if let Some(a) = annex_art { art = art.union(a); }
        let cell = Rect::from_min_max(pos2(art.min.x, art.min.y - HEADER), pos2(art.max.x, art.max.y + FOOTER));
        // Footer band under the room.
        painter.rect_filled(cell, CornerRadius::same(6), Color32::from_rgb(18, 15, 23));
        self.draw_art(painter, origin, assets, trim(self.drawn_frame().translate(origin.to_vec2()), mine));
        if let (Some(annex), Some(clip)) = (&self.annex, annex_art) { annex.draw_art(painter, other, assets, clip); }
        self.draw_band(painter, origin, art, selected);
    }

    /// Where a member of the guild stands in the workroom, in its art
    /// pixels (`place` fits them to the room's size): at the station of
    /// what it does while it works (the bookshelf to read, the anvil to
    /// write, the potions to run commands, the crystals, beside the
    /// director's desk to plan), before the director's desk while it asks
    /// for something, on the floor while free. `k` is how many are already
    /// there.
    fn crew_spot(status: &str, action: &str, k: usize) -> (Pos2, f32) {
        let spots: &[(f32, f32, f32)] = match (status, action) {
            ("blocked", _) => &[(60.0, 72.0, 1.0), (84.0, 72.0, -1.0), (72.0, 78.0, 1.0)],
            ("unknown", _) => &[(72.0, 88.0, 1.0), (80.0, 88.0, -1.0)],
            ("working", "read") => &[(53.0, 33.0, 1.0), (62.0, 34.0, -1.0), (45.0, 35.0, 1.0)],
            ("working", "forge") => &[(32.0, 40.0, -1.0), (38.0, 46.0, -1.0), (28.0, 48.0, -1.0)],
            ("working", "brew") => &[(114.0, 63.0, 1.0), (104.0, 64.0, 1.0), (124.0, 64.0, -1.0)],
            ("working", "gems") => &[(108.0, 38.0, 1.0), (102.0, 42.0, 1.0)],
            ("working", "summon") => &[(72.0, 76.0, 1.0), (62.0, 78.0, 1.0)],
            ("working", _) => &[(40.0, 52.0, 1.0), (104.0, 52.0, -1.0), (34.0, 60.0, 1.0), (110.0, 58.0, -1.0)],
            _ => &[(34.0, 86.0, 1.0), (46.0, 86.0, 1.0), (58.0, 88.0, 1.0), (86.0, 88.0, -1.0), (100.0, 88.0, -1.0), (112.0, 88.0, -1.0), (124.0, 88.0, -1.0)],
        };
        let (x, y, facing) = spots[k % spots.len()];
        let nudge = (k / spots.len()) as f32 * 5.0;
        (pos2(x + nudge, y), facing)
    }

    /// Each member to its spot; one joining comes in by the door. Without
    /// animation, each is at its spot at once.
    fn set_crew(&mut self, crew: &[crate::herdr::CrewMember], animated: bool) {
        let mut next: Vec<Trainee> = vec![];
        let mut taken: HashMap<(String, String), usize> = HashMap::new();
        for m in crew {
            let action = m.action.clone().unwrap_or_default();
            let n = taken.entry((m.status.clone(), if m.status == "working" { action.clone() } else { String::new() })).or_insert(0);
            let (target, facing) = Room::crew_spot(&m.status, &action, *n);
            *n += 1;
            let mut t = match self.crew.iter().position(|t| t.name == m.name) {
                Some(i) => self.crew.remove(i),
                None => Trainee { name: m.name.clone(), harness: harness_for(&m.harness), status: String::new(), action: String::new(), pos: DOOR, target, facing, next_spark: 0.0 },
            };
            t.harness = harness_for(&m.harness);
            t.status = m.status.clone();
            t.action = action;
            t.target = target;
            if !animated { t.pos = target; }
            if (t.pos - t.target).length() < 0.5 { t.facing = facing; }
            next.push(t);
        }
        self.crew = next;
    }

    /// The guild's members over the workroom: each in its own harness, at
    /// work at its station (a book in hand to read or plan, its station
    /// lit), with the alert while it asks, the tick when it is done.
    fn draw_crew(&self, painter: &Painter, origin: Pos2, assets: &Assets) {
        let t = self.clock;
        let glow = assets.prop("glow");
        let mut order: Vec<&Trainee> = self.crew.iter().collect();
        order.sort_by(|a, b| a.pos.y.total_cmp(&b.pos.y));
        for (i, c) in order.into_iter().enumerate() {
            let q = self.place(Room::rp(c.pos));
            let p = pos2(origin.x + q.x, origin.y + q.y);
            let walking = (c.target - c.pos).length() > 0.3;
            let working = !walking && c.status == "working";
            if working {
                let color = match c.action.as_str() { "forge" => ORANGE, "brew" => GREEN, "gems" | "type" | "plan" => CYAN, "summon" => VIOLET, _ => GOLD };
                let pulse = 0.18 + 0.12 * (t * 3.0 + i as f32 * 1.7).sin();
                draw_sprite(painter, glow, p, vec2(40.0, 40.0), false, 0.0, with_alpha(color, pulse));
            }
            let frame = if working { assets.hero_frame(c.harness, "work", (t / 0.16) as usize) } else { assets.hero_frame(c.harness, "idle", (t / if walking { 0.14 } else { 0.5 }) as usize) };
            let s = 24.0;
            draw_sprite(painter, frame, p, vec2(s, s), c.facing < 0.0, 0.0, Color32::WHITE);
            if working && (c.action == "read" || c.action == "plan") {
                let prop = assets.prop(if c.action == "plan" { "scroll_open" } else { "book_open" });
                draw_sprite(painter, prop, pos2(p.x, p.y + 4.0), vec2(12.0, 8.0), false, 0.0, Color32::WHITE);
            }
            if walking { continue; }
            match c.status.as_str() {
                "blocked" => { let bob = (t * 6.0).sin() * 1.5; draw_sprite(painter, assets.prop("alert"), pos2(p.x, p.y - s / 2.0 - 8.0 + bob), vec2(6.0, 11.0) * 1.2, false, 0.0, Color32::WHITE); }
                "done" => draw_sprite(painter, assets.prop("tick"), pos2(p.x, p.y - s / 2.0 - 7.0), vec2(11.0, 12.0), false, 0.0, Color32::WHITE),
                _ => {}
            }
        }
    }

    /// The room's art: backdrop, lights, heroes, props, particles.
    fn draw_art(&self, painter: &Painter, origin: Pos2, assets: &Assets, clip: Rect) {
        let at = |p: Pos2| { let q = self.place(p); pos2(origin.x + q.x, origin.y + q.y) };
        let f = self.drawn_frame().translate(origin.to_vec2());
        let t = self.routine_clock;
        let style = self.style;
        let clipped = painter.with_clip_rect(clip);
        // Backdrop, fading in after a room change.
        let fade = self.swap_at.map(|s| ((self.clock - s) / (0.35 / 1.5)).clamp(0.0, 1.0)).unwrap_or(1.0);
        let tint = with_alpha(Color32::WHITE, fade);
        let background = self.background(style);
        if let Some(room) = assets.room(&background) {
            clipped.image(room.tex, f, room.uv, tint);
        }
        // Lights (soft glows) under the actors.
        let glow = assets.prop("glow");
        for light in &self.lights {
            let alpha = Room::light_alpha(light, t);
            let size = light.radius * 2.6;
            draw_sprite(&clipped, glow, at(Room::rp(light.art)), vec2(size, size), false, 0.0, with_alpha(light.color, alpha));
        }
        if style.status == "blocked" && !self.in_transition {
            let p = t % 0.6;
            let seal = if p < 0.3 { lerp(0.2, 0.5, p / 0.3) } else { lerp(0.5, 0.15, (p - 0.3) / 0.3) };
            draw_sprite(&clipped, glow, at(Room::rp(pos2(72.0, 15.0))), vec2(48.0, 48.0), false, 0.0, with_alpha(RED, seal));
            let q = t % 0.8;
            let wash = if q < 0.35 { lerp(0.02, 0.16, q / 0.35) } else { lerp(0.16, 0.02, (q - 0.35) / 0.45) };
            clipped.rect_filled(f, CornerRadius::ZERO, with_alpha(RED, wash));
        }
        // Subagents behind the desk.
        let (hero_pos, _, book) = self.hero_view();
        let mini_tint = Color32::from_rgb(178, 178, 178);
        let (kind, index) = self.frame_index();
        if self.party_visible {
            for mini in self.party.iter().filter(|m| !m.front) { self.draw_mini(&clipped, mini, origin, assets, mini_tint); }
        }
        // The hero.
        let hero_at = origin + self.place_hero(hero_pos).to_vec2();
        if style.lying && !self.in_transition {
            let shuffle = t - (self.shuffle_at - self.shuffle_period);
            let wobble = if shuffle < 0.15 { lerp(0.0, 0.12, shuffle / 0.15) } else if shuffle < 0.35 { lerp(0.12, -0.08, (shuffle - 0.15) / 0.2) } else if shuffle < 0.6 { lerp(-0.08, 0.0, (shuffle - 0.35) / 0.25) } else { 0.0 };
            let sleeper = assets.sleeper(self.harness);
            draw_sprite(&clipped, sleeper, hero_at, sleeper.px * 2.0, false, -(std::f32::consts::FRAC_PI_2 + wobble), Color32::WHITE);
        } else {
            // In the inn's room the hero reads in an armchair seen from its
            // side, in profile, facing the window: the chair's back stands
            // behind the hero's back, its seat, arm and legs go over the
            // hero's legs once it has sat down.
            let seat = at(Room::rp(style.spot));
            let seated = style.status == "idle" && !self.in_transition && self.runner.is_none();
            if style.status == "idle" {
                let back = assets.prop("chair_side_back");
                draw_sprite(&clipped, back, pos2(seat.x + 9.0, seat.y - 1.0), back.px * 2.0, false, 0.0, Color32::WHITE);
            }
            // Seated, it holds one frame: no breathing bob in the chair.
            // The guild's director types only while it works; otherwise it waits at the desk.
            let resting = self.training && self.status != "working";
            let sprite = if seated { assets.hero_frame(self.harness, "idle", 0) } else if resting { assets.hero_frame(self.harness, "idle", (self.clock / 0.6) as usize % 2) } else { assets.hero_frame(self.harness, kind, index) };
            draw_sprite(&clipped, sprite, hero_at, vec2(HERO, HERO), if seated { true } else { self.facing < 0.0 }, 0.0, Color32::WHITE);
            if seated {
                let front = assets.prop("chair_side_front");
                draw_sprite(&clipped, front, pos2(seat.x + 1.0, seat.y + 10.0), front.px * 2.0, false, 0.0, Color32::WHITE);
            }
            if let Some((name, size, offset)) = book {
                let prop = assets.prop(name);
                draw_sprite(&clipped, prop, pos2(hero_at.x + offset.x, hero_at.y + offset.y), size, false, 0.0, Color32::WHITE);
            }
            if !self.in_transition {
                match style.status {
                    "blocked" => {
                        let q = t % 0.55;
                        let scale = if q < 0.18 { lerp(1.0, 1.25, q / 0.18) } else if q < 0.4 { lerp(1.25, 1.0, (q - 0.18) / 0.22) } else { 1.0 };
                        let r = t % 0.48;
                        let angle = if r < 0.12 { lerp(0.0, 0.12, r / 0.12) } else if r < 0.36 { lerp(0.12, -0.12, (r - 0.12) / 0.24) } else { lerp(-0.12, 0.0, (r - 0.36) / 0.12) };
                        draw_sprite(&clipped, assets.prop("alert"), pos2(hero_at.x, hero_at.y - HERO / 2.0 - 16.0), vec2(13.5 * scale, 25.5 * scale), false, -angle, Color32::WHITE);
                    }
                    "done" => {
                        let arm = assets.arm(self.harness);
                        let r = t % 0.36;
                        for side in [-1.0f32, 1.0] {
                            let angle = if r < 0.18 { lerp(-side * 0.75, -side * 0.2, r / 0.18) } else { lerp(-side * 0.2, -side * 0.75, (r - 0.18) / 0.18) };
                            draw_anchored(&clipped, arm, pos2(hero_at.x + side * 11.0, hero_at.y + 2.0), vec2(6.0, 14.0), vec2(0.5, 0.92), false, -angle, Color32::WHITE);
                        }
                        let s = 1.0 + 0.12 * (0.5 - 0.5 * (std::f32::consts::PI * 2.0 * t / 0.5).cos());
                        draw_sprite(&clipped, assets.prop("tick"), pos2(hero_at.x, hero_at.y - HERO / 2.0 - 15.0), vec2(22.0 * s, 24.0 * s), false, 0.0, Color32::WHITE);
                    }
                    _ => {}
                }
            }
        }
        // The bed's own blanket over the sleeper, breathing.
        if style.lying && !self.in_transition {
            let corner = self.size.remap(style.status, pos2(70.0, 53.0));
            if let Some(blanket) = assets.room_part(&background, corner.x, corner.y, 20.0, 9.0) {
                let a = Room::rp(pos2(70.0, 53.0));
                let b = Room::rp(pos2(90.0, 62.0));
                let e = t % 3.6;
                let scale_y = if e < 1.6 { 1.0 + 0.12 * (std::f32::consts::FRAC_PI_2 * e / 1.6).sin() } else { 1.12 - 0.12 * (std::f32::consts::FRAC_PI_2 * (e - 1.6) / 2.0).sin() };
                let size = vec2(b.x - a.x, (b.y - a.y) * scale_y);
                draw_sprite(&clipped, blanket, at(pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)), size, false, 0.0, Color32::WHITE);
            }
        }
        if style.status == "limited" && !self.in_transition { self.draw_recovery(&clipped, painter, origin, assets); }
        // The room's front layer (the desk), over the hero.
        if let Some(front) = assets.room(&format!("{background}_front")) {
            clipped.image(front.tex, f, front.uv, tint);
        }
        // The git ledger, on the desktop (so over the desk's front layer),
        // drawn at the hero's scale and laid out around where it stands; the
        // hero's arm and quill over it, frame for frame.
        if let Some((frame, raised)) = self.git_tome() {
            let corner = pos2(hero_at.x - GIT_TOME_HERO.x * 2.0, hero_at.y - GIT_TOME_HERO.y * 2.0 + raised);
            let size = GIT_TOME_FRAME * 2.0;
            draw_anchored(&clipped, assets.git_tome(frame), corner, size, Vec2::ZERO, false, 0.0, Color32::WHITE);
            draw_anchored(&clipped, assets.git_tome_arm(self.harness, frame), corner, size, Vec2::ZERO, false, 0.0, Color32::WHITE);
        }
        if self.party_visible {
            for mini in self.party.iter().filter(|m| m.front) { self.draw_mini(&clipped, mini, origin, assets, mini_tint); }
        }
        if self.training {
            // The director's own state over it, then the guild at work around it.
            match self.status.as_str() {
                "blocked" => { let bob = (t * 6.0).sin() * 2.0; draw_sprite(&clipped, assets.prop("alert"), pos2(hero_at.x, hero_at.y - HERO / 2.0 - 5.0 + bob), vec2(9.0, 17.0), false, 0.0, Color32::WHITE); }
                "done" => draw_sprite(&clipped, assets.prop("tick"), pos2(hero_at.x, hero_at.y - HERO / 2.0 - 4.0), vec2(16.5, 18.0), false, 0.0, Color32::WHITE),
                _ => {}
            }
            self.draw_crew(&clipped, origin, assets);
        }
        // Night in the inn's room: the moon through the window lights the
        // reader; the hearth in the corner glows and flickers.
        if style.status == "idle" && !self.in_transition {
            let window = at(Room::rp(WINDOW));
            let reach = f.width() * 2.0;
            let e = t % 4.0;
            let s = if e < 1.8 { lerp(0.97, 1.05, e / 1.8) } else { lerp(1.05, 0.97, (e - 1.8) / 2.2) };
            draw_sprite(&clipped, assets.prop("darkness"), window, vec2(reach * s, reach * s), false, 0.0, Color32::WHITE);
            let hearth = at(Room::rp(HEARTH));
            let fire = 0.2 + 0.06 * (t * 2.3 + self.rng.0 as f32 % 7.0).sin() * (t * 0.7).cos();
            draw_sprite(&clipped, glow, pos2(hearth.x, hearth.y + 4.0), vec2(64.0, 48.0), false, 0.0, with_alpha(Color32::from_rgb(255, 140, 51), fire));
        }
        // Speech bubble, bobbing over the hero's head.
        let bubble_text = if let Some(runner) = &self.runner {
            match runner.current() { Some((Step { kind: StepKind::Say, .. }, _)) => Some("?".to_string()), _ => self.bubble.clone() }
        } else { self.bubble.clone() };
        if let (Some(text), false) = (bubble_text, self.in_transition) {
            let center = pos2(hero_at.x, hero_at.y - HERO / 2.0 - 10.0 - 2.0 * (std::f32::consts::TAU * t).sin());
            let rect = Rect::from_center_size(center, vec2(18.0, 16.0));
            clipped.rect_filled(rect, CornerRadius::same(5), Color32::WHITE);
            clipped.rect_stroke(rect, CornerRadius::same(5), Stroke::new(1.0_f32, Color32::from_gray(26)), egui::StrokeKind::Outside);
            clipped.text(center, Align2::CENTER_CENTER, text, FontId::monospace(13.0), Color32::BLACK);
        }
        // Particles.
        for p in &self.particles { self.draw_particle(&clipped, p, origin, assets); }
        self.draw_shells(painter, f, assets);
    }

    /// The band around the art `f` (one half's frame, or both halves'):
    /// the repo's header, the frame's stroke and the footer.
    fn draw_band(&self, painter: &Painter, origin: Pos2, f: Rect, selected: bool) {
        let header = Rect::from_min_max(pos2(f.min.x, f.min.y - HEADER), pos2(f.max.x, f.min.y));
        let cell = Rect::from_min_max(header.min, pos2(f.max.x, f.max.y + FOOTER));
        let style = self.band.unwrap_or(self.style);
        // Rounded corners: cover what the square clip let through.
        cover_corners(painter, f, 6.0, BACKGROUND);
        cover_corners(painter, cell, 6.0, BACKGROUND);
        // The repo's band: its colour, emblem and name, whatever the state.
        let identity = &self.identity;
        let primary = identity.primary_color();
        painter.rect_filled(header, CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, blend(Color32::from_rgb(18, 15, 23), primary, 0.16));
        painter.rect_filled(Rect::from_min_max(pos2(header.min.x, header.max.y - 2.0), header.max), CornerRadius::ZERO, primary);
        draw_emblem(painter, identity, self.emblem_rect(origin).min, 2.0);
        let name_room = header.width() - 40.0;
        let name_color = blend(primary, Color32::WHITE, 0.5);
        let name = fit_text(painter, &identity.name, FontId::monospace(13.0), name_room);
        let name_at = pos2(header.min.x + 34.0, header.center().y - 1.0);
        painter.text(name_at, Align2::LEFT_CENTER, &name, FontId::monospace(13.0), name_color);
        painter.text(pos2(name_at.x + 0.7, name_at.y), Align2::LEFT_CENTER, &name, FontId::monospace(13.0), name_color);
        // Frame: quiet, so the repo's colour leads; it only lights up for
        // an agent that needs you or is out of credits.
        let stroke = if style.status == "blocked" && !self.in_transition {
            let bright = Color32::from_rgb(255, 51, 56);
            let dim = Color32::from_rgb(89, 13, 20);
            Stroke::new(3.0_f32, if (self.clock % 0.7) < 0.35 { bright } else { dim })
        } else if style.status == "limited" && !self.in_transition {
            let q = (std::f32::consts::TAU * self.clock / 2.4).sin() * 0.5 + 0.5;
            Stroke::new(2.0_f32, blend(Color32::from_rgb(74, 30, 99), LIMITED, q))
        } else {
            Stroke::new(1.0_f32, Color32::from_rgb(58, 52, 68))
        };
        painter.rect_stroke(cell, CornerRadius::same(6), stroke, egui::StrokeKind::Inside);
        // The footer: what the agent is doing, its branch (bottom-left) and
        // the room's state with its light (bottom-right).
        let usable_chars = ((f.width() - 28.0) / 6.65) as usize;
        painter.text(pos2(f.min.x + 10.0, f.max.y + 11.0), Align2::LEFT_CENTER, Room::clip(&self.activity, usable_chars), FontId::monospace(11.0), Color32::WHITE);
        // The new state shows at once, while the hero is still walking
        // over to its new room.
        let shown = self.band.unwrap_or(self.pending_style.unwrap_or(style));
        let title = tr(shown.title) + &if self.subagents > 0 && self.subagents != usize::MAX { format!(" +{}", self.subagents) } else { String::new() };
        let title_color = blend(shown.color, Color32::WHITE, 0.45);
        let at = pos2(f.max.x - 10.0, f.max.y + FOOTER - 10.0);
        painter.text(at, Align2::RIGHT_CENTER, &title, FontId::monospace(11.0), title_color);
        let title_rect = painter.text(pos2(at.x + 0.6, at.y), Align2::RIGHT_CENTER, &title, FontId::monospace(11.0), title_color);
        painter.circle_filled(pos2(title_rect.min.x - 8.0, at.y), 3.5, shown.color);
        painter.circle_stroke(pos2(title_rect.min.x - 8.0, at.y), 3.5, Stroke::new(1.0_f32, Color32::from_black_alpha(140)));
        let branch_chars = (((title_rect.min.x - f.min.x - 36.0) / 6.65) as usize).max(4);
        painter.text(pos2(f.min.x + 10.0, at.y), Align2::LEFT_CENTER, Room::clip(&self.branch, branch_chars), FontId::monospace(11.0), Color32::from_gray(209));
        if selected {
            painter.rect_stroke(cell.expand(2.0), CornerRadius::same(7), Stroke::new(2.0_f32, Color32::from_rgb(217, 242, 166)), egui::StrokeKind::Outside);
        }
    }

    /// Where the repo's emblem is drawn on its band.
    fn emblem_rect(&self, origin: Pos2) -> Rect {
        let header = self.header().translate(origin.to_vec2());
        Rect::from_min_size(pos2(header.min.x + 7.0, header.center().y - 13.0), vec2(20.0, 24.0))
    }

    /// The repo's full name and key, while the pointer is on its band.
    pub fn header_tip(&self, origin: Pos2, pointer: Pos2) -> Option<String> {
        if !self.header().translate(origin.to_vec2()).contains(pointer) { return None; }
        let identity = &self.identity;
        let mut tip = identity.name.clone();
        if identity.key != identity.name.to_lowercase() { tip += &format!("\n{}", identity.key); }
        if !identity.kind.label().is_empty() { tip += &format!(" · {}", identity.kind.label()); }
        Some(tip)
    }

    /// Where the cauldrons of the shells it left running stand: the room's
    /// bottom-left corner, one each (three at most, then a count).
    fn shells_rect(&self, f: Rect) -> Option<Rect> {
        if self.shells.is_empty() { return None; }
        let shown = self.shells.len().min(3) as f32;
        Some(Rect::from_min_size(pos2(f.min.x + 10.0, f.max.y - 52.0), vec2(shown * 34.0 + if self.shells.len() > 3 { 24.0 } else { 0.0 }, 38.0)))
    }

    /// The cauldrons, bubbling over a flickering fire.
    fn draw_shells(&self, painter: &Painter, f: Rect, assets: &Assets) {
        let Some(area) = self.shells_rect(f) else { return };
        for i in 0..self.shells.len().min(3) {
            let at = pos2(area.min.x + 17.0 + i as f32 * 34.0, area.max.y - 16.0);
            let phase = self.clock * 6.0 + i as f32 * 1.7;
            // The fire under it.
            for (dx, h) in [(-6.0, 4.0), (0.0, 6.0), (6.0, 4.0)] {
                let flick = h + ((phase + dx).sin() * 1.5).round();
                painter.rect_filled(Rect::from_min_size(pos2(at.x + dx - 2.0, at.y + 15.0 - flick), vec2(4.0, flick)), CornerRadius::ZERO, Color32::from_rgb(255, 140, 40));
                painter.rect_filled(Rect::from_min_size(pos2(at.x + dx - 1.0, at.y + 15.0 - flick * 0.5), vec2(2.0, flick * 0.5)), CornerRadius::ZERO, Color32::from_rgb(255, 220, 90));
            }
            painter.circle_filled(pos2(at.x, at.y + 2.0), 18.0, with_alpha(Color32::from_rgb(120, 230, 90), 0.12));
            draw_sprite(painter, assets.prop("cauldron"), at, vec2(32.0, 24.0), false, 0.0, Color32::WHITE);
            // Bubbles rising off the brew.
            for b in 0..3 {
                let t = (self.clock * 0.9 + b as f32 / 3.0 + i as f32 * 0.37) % 1.0;
                let x = at.x - 7.0 + b as f32 * 7.0 + (t * 9.0 + b as f32).sin() * 2.0;
                let y = at.y - 12.0 - t * 16.0;
                painter.rect_filled(Rect::from_center_size(pos2(x, y), vec2(3.0, 3.0)), CornerRadius::ZERO, with_alpha(Color32::from_rgb(150, 240, 110), 1.0 - t));
            }
        }
        if self.shells.len() > 3 {
            label(painter, pos2(area.max.x - 2.0, area.center().y), Align2::RIGHT_CENTER, &format!("×{}", self.shells.len()), 11.0, Color32::from_rgb(150, 240, 110), true);
        }
    }

    /// What the cauldrons stand for, when the pointer is on them: where
    /// they stand, and per shell its description · running time and last line.
    pub fn shells_tip(&self, origin: Pos2, pointer: Pos2) -> Option<(Rect, Vec<(String, Option<String>)>)> {
        let f = self.drawn_frame().translate(origin.to_vec2());
        let area = self.shells_rect(f)?;
        if !area.contains(pointer) { return None; }
        let now = crate::herdr::unix_now();
        Some((area, self.shells.iter().map(|s| (format!("{} · {}", s.label, countdown(now - s.started)), s.last_line.clone())).collect()))
    }

    /// The tag at the cot's foot: how long until the hero has recovered
    /// (the usage limit resets).
    fn draw_recovery(&self, clipped: &Painter, painter: &Painter, origin: Pos2, assets: &Assets) {
        let text = self.limit_resets.map(|r| countdown(r - crate::herdr::unix_now())).unwrap_or_else(|| "--:--".into());
        let ink = blend(LIMITED, Color32::WHITE, 0.55);
        let galley = painter.layout_no_wrap(text, FontId::monospace(12.0), ink);
        let width = galley.size().x + 18.0;
        let foot = self.place(Room::rp(pos2(72.0, 73.0)));
        let pill = Rect::from_center_size(pos2(origin.x + foot.x, origin.y + foot.y), vec2(width + 8.0, galley.size().y + 4.0));
        clipped.rect_filled(pill, CornerRadius::same(3), Color32::from_black_alpha(200));
        clipped.rect_stroke(pill, CornerRadius::same(3), Stroke::new(1.0_f32, with_alpha(LIMITED, 0.8)), egui::StrokeKind::Inside);
        draw_sprite(clipped, assets.prop("hourglass"), pos2(pill.min.x + 10.0, pill.center().y), vec2(10.0, 10.0), false, 0.0, Color32::WHITE);
        clipped.galley(pos2(pill.min.x + 20.0, pill.center().y - galley.size().y / 2.0), galley, ink);
    }

    fn draw_mini(&self, painter: &Painter, mini: &Mini, origin: Pos2, assets: &Assets, tint: Color32) {
        let frame = (self.clock / 0.16) as usize % 3;
        let sprite = assets.hero_frame(mini.harness, "work", frame);
        // Placed at the ends of its walk, like the hero's.
        let span = (mini.target - mini.from).length();
        let k = if span > 0.5 { 1.0 - (mini.target - mini.pos).length() / span } else { 1.0 };
        let placed = lerp_pos(self.place(mini.from), self.place(mini.target), k.clamp(0.0, 1.0));
        let at = pos2(origin.x + placed.x, origin.y + placed.y);
        draw_sprite(painter, sprite, at, vec2(24.0, 24.0), mini.facing < 0.0, 0.0, tint);
        // Readers and planners hold an open book or scroll.
        if mini.action == "read" || mini.action == "plan" {
            let prop = assets.prop(if mini.action == "plan" { "scroll_open" } else { "book_open" });
            draw_sprite(painter, prop, pos2(at.x, at.y + 4.0), vec2(12.0, 8.0), false, 0.0, Color32::WHITE);
        }
    }

    fn draw_particle(&self, painter: &Painter, p: &Particle, origin: Pos2, assets: &Assets) {
        let k = (p.age / p.life).clamp(0.0, 1.0);
        let at = |q: Pos2| { let q = self.place(q); pos2(origin.x + q.x, origin.y + q.y) };
        match p.kind {
            ParticleKind::Bit { color, size } => {
                let pos = at(pos2(p.origin.x + p.delta.x * k, p.origin.y + p.delta.y * k));
                let alpha = if k < 0.5 { 1.0 } else { 1.0 - (k - 0.5) / 0.5 };
                painter.rect_filled(Rect::from_center_size(pos, vec2(size, size)), CornerRadius::ZERO, with_alpha(color, alpha));
            }
            ParticleKind::Sparkle { color, size } => {
                let a = p.age;
                let scale = if a < 0.18 { a / 0.18 } else if a < 0.3 { 1.0 } else { (1.0 - (a - 0.3) / 0.3).max(0.0) };
                let tint = if color == Color32::WHITE { Color32::WHITE } else { blend(Color32::WHITE, color, 0.6) };
                draw_sprite(painter, assets.prop("sparkle"), at(p.origin), vec2(size * scale, size * scale), false, std::f32::consts::FRAC_PI_4 * (a / 0.6), tint);
            }
            ParticleKind::Z => {
                let head = p.origin;
                let pos = at(pos2(head.x + 4.0 + 16.0 * k + 3.0 * (k * std::f32::consts::PI * 3.0).sin(), head.y - 10.0 - 30.0 * k));
                let scale = 0.5 + 0.8 * k;
                let alpha = if p.age < 0.4 { p.age / 0.4 } else if p.age < 2.0 { 1.0 } else { (1.0 - (p.age - 2.0) / 0.8).max(0.0) };
                draw_sprite(painter, assets.prop("z"), pos, vec2(14.0 * scale, 14.0 * scale), false, 0.0, with_alpha(Color32::WHITE, alpha));
            }
            ParticleKind::Confetti { color, drift } => {
                let pos = at(pos2(p.origin.x + drift * k, p.origin.y + 34.0 * k));
                let angle = std::f32::consts::TAU * k;
                let mut mesh = Mesh::default();
                mesh.add_colored_rect(Rect::from_center_size(pos, vec2(3.0, 3.0)), with_alpha(color, 1.0 - k));
                mesh.rotate(Rot2::from_angle(angle), pos);
                painter.add(Shape::mesh(mesh));
            }
        }
    }
}

fn bit(rng: &mut Rng, at: Pos2, colors: &[Color32], spread: f32, rise_min: f32, rise_max: f32, size: f32, life: f32) -> Particle {
    let color = rng.pick(colors);
    let dx = rng.range(-spread, spread);
    let dy = -rng.range(rise_min.min(rise_max), rise_max.max(rise_min));
    Particle { kind: ParticleKind::Bit { color, size }, origin: at, delta: vec2(dx, dy), life, age: 0.0 }
}

fn sparkle(rng: &mut Rng, at: Pos2, color: Color32, size: f32, jitter: f32) -> Particle {
    let origin = pos2(at.x + rng.range(-jitter, jitter), at.y + rng.range(-jitter, jitter));
    Particle { kind: ParticleKind::Sparkle { color, size }, origin, delta: Vec2::ZERO, life: 0.6, age: 0.0 }
}

/// A walk from where the routine left the hero to a room-art pixel.
/// How fast the hero walks out and back in when its room changes, in art
/// pixels per second.
const TRANSITION_SPEED: f32 = 66.0;

fn walk(walker: &mut Pos2, to: Pos2, speed: f32) -> Step {
    let from = *walker;
    *walker = to;
    let a = Room::rp(from);
    let b = Room::rp(to);
    let time = ((a.x - b.x).hypot(a.y - b.y) / speed).max(0.05);
    Step { kind: StepKind::Walk { from, to }, dur: time }
}

/// Walk to a station, around the desk: going between the top half of the
/// room and the bottom half uses the free lane on the left or right.
fn go(walker: &mut Pos2, to: Pos2) -> Vec<Step> {
    let from = *walker;
    if (from.y < 50.0) == (to.y < 50.0) { return vec![walk(walker, to, 34.0)]; }
    let lane = if (from.x + to.x) / 2.0 < 72.0 { 36.0 } else { 106.0 };
    vec![walk(walker, pos2(lane, from.y), 34.0), walk(walker, pos2(lane, to.y), 34.0), walk(walker, to, 34.0)]
}

/// Text with a dark pill behind it; `bold` draws it twice, a hair apart.
fn label(painter: &Painter, at: Pos2, align: Align2, text: &str, size: f32, color: Color32, bold: bool) -> Rect {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::monospace(size), color);
    let rect = align.anchor_size(at, galley.size());
    painter.rect_filled(rect.expand2(vec2(4.0, 2.0)), CornerRadius::same(3), Color32::from_black_alpha(184));
    painter.galley(rect.min, galley.clone(), color);
    if bold { painter.galley(pos2(rect.min.x + 0.7, rect.min.y), galley, color); }
    rect
}

/// A repo's emblem, 10×12 pixels of `px` points from `top_left`: a dark
/// edge, the field in its colour with the partition in the second one, and
/// the charge inked in the middle.
pub fn draw_emblem(painter: &Painter, identity: &Identity, top_left: Pos2, px: f32) {
    let ppp = painter.ctx().pixels_per_point();
    let top_left = pos2((top_left.x * ppp).round() / ppp, (top_left.y * ppp).round() / ppp);
    let mask = identity.shape.mask();
    let inside = |x: i32, y: i32| (0..10).contains(&x) && (0..12).contains(&y) && mask[y as usize].as_bytes()[x as usize] == b'#';
    let charge = identity.symbol.pixels();
    let inked = |x: i32, y: i32| (2..8).contains(&x) && (2..8).contains(&y) && charge[(y - 2) as usize].as_bytes()[(x - 2) as usize] == b'#';
    let (primary, secondary, ink) = (identity.primary_color(), identity.secondary_color(), identity.ink());
    let edge = Color32::from_rgb(24, 20, 37);
    let rim = |x: i32, y: i32| !(inside(x - 1, y) && inside(x + 1, y) && inside(x, y - 1) && inside(x, y + 1));
    let mut mesh = Mesh::default();
    for y in 0..12 {
        for x in 0..10 {
            if !inside(x, y) { continue; }
            let near_rim = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| inside(x + dx, y + dy) && rim(x + dx, y + dy));
            let color = if rim(x, y) { edge } else if inked(x, y) { ink } else if identity.pattern.second(x as usize, y as usize, near_rim) { secondary } else { primary };
            mesh.add_colored_rect(Rect::from_min_size(pos2(top_left.x + x as f32 * px, top_left.y + y as f32 * px), vec2(px, px)), color);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// `text` cut with "…" to fit `width` points in `font`.
fn fit_text(painter: &Painter, text: &str, font: FontId, width: f32) -> String {
    let fits = |t: &str| painter.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x <= width;
    if fits(text) { return text.to_string(); }
    let mut cut: String = text.to_string();
    while cut.pop().is_some() {
        let shown = format!("{}…", cut.trim_end());
        if fits(&shown) { return shown; }
    }
    "…".into()
}

/// Paint the window background over the corners of a square-clipped rect,
/// so it reads as rounded.
fn cover_corners(painter: &Painter, rect: Rect, radius: f32, color: Color32) {
    let corners = [(rect.min, vec2(1.0, 1.0)), (pos2(rect.max.x, rect.min.y), vec2(-1.0, 1.0)), (rect.max, vec2(-1.0, -1.0)), (pos2(rect.min.x, rect.max.y), vec2(1.0, -1.0))];
    for (corner, dir) in corners {
        let center = pos2(corner.x + dir.x * radius, corner.y + dir.y * radius);
        let mut points = vec![corner];
        let steps = 6;
        for i in 0..=steps {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / steps as f32;
            // From the vertical edge to the horizontal edge of this corner.
            points.push(pos2(center.x - dir.x * radius * a.cos(), center.y - dir.y * radius * a.sin()));
        }
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
    }
}

pub struct SceneResponse {
    pub clicked: Option<String>,
    pub finish: Option<String>,
    /// A terminal in this agent's folder was asked for from its room's menu.
    pub terminal: Option<String>,
    /// The spare "+" room was clicked.
    pub summon: bool,
    /// While summoning in the spare room: its rect, for the form drawn in
    /// it, and the part of the scene in view.
    pub summon_rect: Option<(Rect, Rect)>,
    /// A room's emblem was right-clicked: edit it.
    pub edit_emblem: Option<String>,
    /// A room was dragged to a new place: every room's id, in the new order.
    pub reorder: Option<Vec<String>>,
    /// Files from outside are being dragged over this room: open its chat.
    pub file_hover: Option<String>,
    /// Files from outside were dropped on this room: into its chat's message.
    pub file_drop: Option<String>,
}

/// How long a room's band is held before it comes loose to be dragged.
const HOLD: f64 = 0.45;

/// The room being dragged and where the pointer grabbed it.
#[derive(Clone)]
struct Drag {
    id: String,
    grab: Vec2,
}

enum ScrollRequest {
    Top,
    Down,
    Reveal(String),
}

pub struct Scene {
    rooms: HashMap<String, Room>,
    order: Vec<String>,
    asking_before: HashSet<String>,
    scroll_request: Option<ScrollRequest>,
    pub selected: Option<String>,
    pub reduced_motion: bool,
    pub empty_text: String,
    /// A spare room after the last one, with a "+" to summon an agent.
    pub summon_tile: bool,
    /// Full screen: rooms keep their places, so nobody asking scrolls the
    /// dungeon to the top (they are called over it instead).
    pub keep_order: bool,
    /// Full screen's column: every room at `room_size`, the chosen one
    /// too (it is only outlined), instead of all large.
    pub compact: bool,
    pub room_size: RoomSize,
    /// The user's choices for some repos' emblems (settings' `projects`).
    pub overrides: BTreeMap<String, ProjectOverride>,
    /// Every repo's emblem as first worked out (settings' `emblems`), and
    /// whether a new one was added, for the app to save.
    pub emblems: BTreeMap<String, crate::identity::SavedEmblem>,
    pub emblems_added: bool,
    /// The summon panel is open: the spare room holds its form.
    pub summoning: bool,
    summon_shown: bool,
    /// Full screen's column: a room held by its band, being dragged.
    dragging: Option<Drag>,
    seeds: u32,
    clock: f32,
    last_time: Option<f64>,
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            rooms: HashMap::new(),
            order: vec![],
            asking_before: HashSet::new(),
            scroll_request: None,
            selected: None,
            reduced_motion: false,
            empty_text: tr("Sin agentes en la sesión"),
            summon_tile: false,
            keep_order: false,
            compact: false,
            room_size: RoomSize::Medium,
            overrides: BTreeMap::new(),
            emblems: BTreeMap::new(),
            emblems_added: false,
            summoning: false,
            summon_shown: false,
            dragging: None,
            seeds: 7,
            clock: 0.0,
            last_time: None,
        }
    }

    /// What the emblem editor needs of an agent's room: its emblem, the
    /// repo's own name, and whether it is the demo's.
    pub fn emblem_of(&self, id: &str) -> Option<(Identity, String, bool)> {
        self.rooms.get(id).map(|room| (room.identity.clone(), room.repo_name.clone(), room.demo))
    }

    /// Work every room's emblem out again (after the settings changed).
    pub fn reload_emblems(&mut self) {
        for room in self.rooms.values_mut() { room.cwd.clear(); }
    }

    /// An agent's repo emblem, once its room is up.
    pub fn identity(&self, id: &str) -> Option<&Identity> {
        self.rooms.get(id).map(|room| &room.identity)
    }

    pub fn set_selected(&mut self, id: Option<String>) {
        if self.selected != id {
            self.selected = id.clone();
            if let Some(id) = id { self.scroll_request = Some(ScrollRequest::Reveal(id)); }
        }
    }

    /// Show these agents (already sorted: attention first, or in place in
    /// full screen). Someone new
    /// needing attention takes the first room: scroll up to it.
    pub fn sync(&mut self, agents: &[Agent]) {
        let ids: HashSet<&str> = agents.iter().map(|a| a.id.as_str()).collect();
        self.rooms.retain(|id, _| ids.contains(id.as_str()));
        let asking: HashSet<String> = agents.iter().filter(|a| a.status == "blocked").map(|a| a.id.clone()).collect();
        let newly_asking = asking.difference(&self.asking_before).next().is_some();
        self.asking_before = asking;
        let fresh: Vec<String> = agents.iter().map(|a| a.id.clone()).collect();
        if self.dragging.as_ref().is_some_and(|d| !fresh.contains(&d.id)) { self.dragging = None; }
        self.order = if self.dragging.is_some() {
            // While a room is dragged the order is the user's: rooms keep
            // their places, gone ones leave and new ones come last.
            let mut kept: Vec<String> = self.order.iter().filter(|id| fresh.contains(id)).cloned().collect();
            for id in &fresh { if !kept.contains(id) { kept.push(id.clone()); } }
            kept
        } else {
            fresh
        };
        if newly_asking && self.selected.is_none() && !self.keep_order { self.scroll_request = Some(ScrollRequest::Top); }
        let mut shown: Vec<usize> = vec![];
        for agent in agents {
            let room = match self.rooms.get_mut(&agent.id) {
                Some(room) => { room.update(agent, !self.reduced_motion); room }
                None => {
                    self.seeds = self.seeds.wrapping_mul(1664525).wrapping_add(1013904223);
                    self.rooms.entry(agent.id.clone()).or_insert(Room::new(agent, self.seeds))
                }
            };
            // The emblem, read from the folder once (a few small files).
            if room.cwd != agent.cwd || room.identity.key.is_empty() {
                room.cwd = agent.cwd.clone();
                let demo = crate::herdr::demo_meta(agent);
                let meta = demo.clone().unwrap_or_else(|| crate::identity::read_meta(&agent.cwd));
                // The emblem kept for this repo, else worked out and kept
                // (the demo's are never kept).
                let base = match self.emblems.get(&meta.key).and_then(|kept| kept.restore(&meta)) {
                    Some(kept) => kept,
                    None => {
                        // A colour no other repo has (the demo's: no other room).
                        let mut fresh = crate::identity::identity(&meta);
                        let taken = if demo.is_none() { crate::identity::SavedEmblem::colours_of(self.emblems.values()) } else { shown.clone() };
                        fresh.primary = crate::identity::free_colour(&meta.key, fresh.primary, &taken);
                        fresh.secondary = crate::identity::metal_apart(fresh.primary, fresh.secondary);
                        if demo.is_none() {
                            self.emblems.insert(meta.key.clone(), crate::identity::SavedEmblem::of(&fresh));
                            self.emblems_added = true;
                        }
                        fresh
                    }
                };
                room.identity = crate::identity::apply_override(&meta, base, self.overrides.get(&meta.key));
                room.repo_name = meta.name.clone();
                room.demo = demo.is_some();
            }
            shown.push(room.identity.primary);
        }
    }

    fn slots(&self) -> usize {
        self.order.len() + usize::from(self.summon_tile)
    }

    #[cfg(test)]
    fn content_height(&self, width: f32) -> f32 { self.layout(width).1 }

    /// A double room takes two places.
    fn is_double(&self, index: usize) -> bool {
        self.order.get(index).and_then(|id| self.rooms.get(id)).is_some_and(|r| r.annex.is_some())
    }

    /// How tall the rooms are drawn: the menu's size in full screen's
    /// column, large in the widget.
    pub fn size(&self) -> RoomSize {
        if self.compact { self.room_size } else { RoomSize::Large }
    }

    /// Each slot's top-left corner and size (the rooms in order, then the
    /// spare room), and the content's height: a grid of whole rooms, or
    /// full screen's column of rooms at the menu's size.
    fn layout(&self, width: f32) -> (Vec<Rect>, f32) {
        let mut slots = vec![];
        if self.compact {
            let x = ((width - ROW_W) / 2.0).max(GAP);
            let mut y = TOP_PAD;
            for index in 0..self.order.len() {
                let h = if self.is_double(index) { HALL_H } else { self.room_size.cell_h() };
                slots.push(Rect::from_min_size(pos2(x, y), vec2(ROW_W, h)));
                y += h + COLUMN_GAP;
            }
            if self.summon_tile {
                let h = if self.summoning { ROW_H } else { LINE_H };
                slots.push(Rect::from_min_size(pos2(x, y), vec2(ROW_W, h)));
                y += h + COLUMN_GAP;
            }
            return (slots, y - COLUMN_GAP + BOTTOM_PAD);
        }
        // Each room goes to the shortest column (the leftmost on a tie): a
        // grid of rows while every room is as tall, and no hole beside the
        // guild hall, which is a room's width and twice its height.
        let columns = columns_fitting(width);
        let gap = GAP.max((width - columns as f32 * ROW_W) / (columns + 1) as f32);
        let mut tops = vec![TOP_PAD; columns];
        for index in 0..self.slots() {
            let h = if self.is_double(index) { HALL_H } else { ROW_H };
            let column = (0..columns).min_by(|&a, &b| tops[a].total_cmp(&tops[b]).then(a.cmp(&b))).unwrap_or(0);
            slots.push(Rect::from_min_size(pos2(gap + column as f32 * (ROW_W + gap), tops[column]), vec2(ROW_W, h)));
            tops[column] += h + GAP;
        }
        let bottom = if slots.is_empty() { TOP_PAD } else { tops.iter().copied().fold(0.0, f32::max) - GAP };
        (slots, bottom + BOTTOM_PAD)
    }

    /// Lay the rooms out in the given width and draw them inside a scroll
    /// area; returns what the user clicked.
    pub fn ui(&mut self, ui: &mut egui::Ui, assets: &Assets, now: f64) -> SceneResponse {
        let dt = match self.last_time { Some(last) if !self.reduced_motion => ((now - last) as f32).clamp(0.0, 0.1), _ => 0.0 };
        self.last_time = Some(now);
        self.clock += dt;
        let mut response = SceneResponse { clicked: None, finish: None, terminal: None, summon: false, summon_rect: None, edit_emblem: None, reorder: None, file_hover: None, file_drop: None };
        // Files dragged from outside: where they are over the dungeon, read
        // from the system, since the pointer stands still for egui meanwhile.
        let (files_hovered, files_dropped) = ui.ctx().input(|i| (!i.raw.hovered_files.is_empty(), !i.raw.dropped_files.is_empty()));
        let drag_at = if files_hovered || files_dropped { crate::ui::drag_pointer(ui.ctx()) } else { None };
        let width = ui.available_width();
        let (slots, height) = self.layout(width);
        let scroll_request = self.scroll_request.take();
        let mut room_rects: Vec<Rect> = vec![];
        let output = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                let (content, _) = ui.allocate_exact_size(vec2(width, height.max(ui.available_height())), egui::Sense::hover());
                if self.order.is_empty() && !self.summon_tile {
                    ui.painter().text(content.center(), Align2::CENTER_CENTER, &self.empty_text, FontId::monospace(16.0), Color32::from_gray(128));
                }
                if matches!(scroll_request, Some(ScrollRequest::Top)) {
                    ui.scroll_to_rect(Rect::from_min_size(content.min, vec2(1.0, 1.0)), Some(egui::Align::Min));
                }
                if matches!(scroll_request, Some(ScrollRequest::Down)) {
                    // One screen further down.
                    let page = ui.clip_rect().height().max(ROW_H);
                    ui.scroll_with_delta(vec2(0.0, -(page - GAP)));
                }
                let order = self.order.clone();
                let size = self.size();
                for (index, id) in order.iter().enumerate() {
                    let slot = slots[index];
                    let Some(room) = self.rooms.get_mut(id) else { continue };
                    let target = slot.min;
                    if room.pos != target {
                        room.move_from = room.shown.unwrap_or(target);
                        room.move_at = self.clock;
                        room.pos = target;
                    }
                    let k = if self.reduced_motion { 1.0 } else { ((self.clock - room.move_at) / 0.25).clamp(0.0, 1.0) };
                    let shown = if room.shown.is_none() { target } else { lerp_pos(room.move_from, target, k) };
                    room.shown = Some(shown);
                    room.advance(dt);
                    let rect = Rect::from_min_size(pos2(content.min.x + shown.x, content.min.y + shown.y), slot.size());
                    if let Some(ScrollRequest::Reveal(wanted)) = &scroll_request {
                        if wanted == id { ui.scroll_to_rect(rect.expand(4.0), None); }
                    }
                    room_rects.push(rect);
                    // Full screen's rooms can be held and dragged, so they take drags too.
                    let hit = ui.interact(rect, ui.id().with(id), if self.compact { egui::Sense::click_and_drag() } else { egui::Sense::click() });
                    let dragged = self.dragging.as_ref().is_some_and(|d| &d.id == id);
                    // Holding a room's band still comes loose to drag it.
                    if self.compact && self.dragging.is_none() && hit.is_pointer_button_down_on() {
                        let (origin, start, at, time) = ui.input(|i| (i.pointer.press_origin(), i.pointer.press_start_time(), i.pointer.interact_pos(), i.time));
                        if let (Some(origin), Some(start), Some(at)) = (origin, start, at) {
                            if room.header().translate(rect.min.to_vec2()).contains(origin) && at.distance(origin) < 6.0 {
                                if time - start >= HOLD { self.dragging = Some(Drag { id: id.clone(), grab: origin - rect.min }); }
                                ui.ctx().request_repaint();
                            }
                        }
                    }
                    // Only what shows is drawn; the dragged room is drawn last, over the others.
                    room.size = size;
                    room.beside = slot.width() > ROW_W + 1.0;
                    if ui.is_rect_visible(rect) && !dragged {
                        room.draw(ui.painter(), rect.min, assets, self.selected.as_deref() == Some(id));
                    }
                    if hit.clicked() && !dragged { response.clicked = Some(id.clone()); }
                    if drag_at.is_some_and(|p| rect.contains(p)) {
                        if files_dropped { response.file_drop = Some(id.clone()); }
                        else {
                            response.file_hover = Some(id.clone());
                            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("room-drop")));
                            painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(6), Stroke::new(3.0_f32, Color32::from_rgb(140, 217, 115)), egui::StrokeKind::Inside);
                            painter.text(rect.center_bottom() - vec2(0.0, 10.0), Align2::CENTER_BOTTOM, tr("Suelta para dejar la ruta en su chat"), FontId::monospace(crate::ui::fs(11.0)), Color32::WHITE);
                        }
                    }
                    if let Some((area, rows)) = hit.hover_pos().and_then(|p| room.shells_tip(rect.min, p)) { crate::ui::shells_note(ui.ctx(), area, &rows); }
                    let hit = match hit.hover_pos().and_then(|p| room.header_tip(rect.min, p)) { Some(tip) => hit.on_hover_text(tip), None => hit };
                    // A right click on the emblem edits it; elsewhere it opens the room's menu.
                    let on_emblem = hit.hover_pos().is_some_and(|p| room.emblem_rect(rect.min).expand(2.0).contains(p));
                    if on_emblem && hit.secondary_clicked() { response.edit_emblem = Some(id.clone()); }
                    let hit = if on_emblem { hit.on_hover_text(tr("Clic derecho: editar el escudo")) } else { hit };
                    if !on_emblem { hit.context_menu(|ui| {
                        if ui.button(tr("Abrir una terminal en esta carpeta")).clicked() {
                            response.terminal = Some(id.clone());
                            ui.close_menu();
                        }
                        let guild_member = id.starts_with(crate::guild_view::ID_PREFIX);
                        // The hall is the guild itself: it closes as a whole.
                        let label = if id == crate::guild_view::HALL_ID { "Cerrar la guild" } else if guild_member { "Quitar de la guild" } else { "Finalizar agente (/exit)" };
                        if ui.button(tr(label)).clicked() {
                            response.finish = Some(id.clone());
                            ui.close_menu();
                        }
                        if ui.button(tr("Editar el escudo…")).clicked() {
                            response.edit_emblem = Some(id.clone());
                            ui.close_menu();
                        }
                    }); }
                }
                // The dragged room follows the pointer up or down, shivering,
                // and the others make way; letting go leaves it in its new place.
                if let Some(drag) = self.dragging.clone() {
                    let (down, at) = ui.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
                    let from = self.order.iter().position(|i| *i == drag.id);
                    match (down, at, from) {
                        (true, Some(at), Some(from)) => {
                            let top = at.y - drag.grab.y - content.min.y;
                            let centre = top + slots[from].height() / 2.0;
                            let to = (0..order.len()).min_by(|&a, &b| (slots[a].center().y - centre).abs().total_cmp(&(slots[b].center().y - centre).abs())).unwrap_or(from);
                            if to != from {
                                let moved = self.order.remove(from);
                                self.order.insert(to, moved);
                            }
                            let shiver = if self.reduced_motion { 0.0 } else { (self.clock * 55.0).sin() * 1.5 };
                            let place = pos2(slots[to].min.x + shiver, top);
                            let selected = self.selected.as_deref() == Some(drag.id.as_str());
                            if let Some(room) = self.rooms.get_mut(&drag.id) {
                                let rect = Rect::from_min_size(content.min + place.to_vec2(), slots[to].size());
                                ui.painter().rect_filled(rect.translate(vec2(4.0, 6.0)), CornerRadius::same(6), Color32::from_rgba_unmultiplied(0, 0, 0, 140));
                                room.draw(ui.painter(), rect.min, assets, selected);
                                room.shown = Some(place);
                            }
                            // Near the edges the column scrolls along.
                            let view = ui.clip_rect();
                            if at.y < view.min.y + 40.0 { ui.scroll_with_delta(vec2(0.0, 10.0)); }
                            if at.y > view.max.y - 40.0 { ui.scroll_with_delta(vec2(0.0, -10.0)); }
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            ui.ctx().request_repaint();
                        }
                        _ => {
                            // Dropped: it slides from where it was let go into its place.
                            if let Some(room) = self.rooms.get_mut(&drag.id) {
                                if let Some(shown) = room.shown { room.move_from = shown; room.move_at = self.clock; }
                            }
                            self.dragging = None;
                            response.reorder = Some(self.order.clone());
                        }
                    }
                }
                if self.summon_tile {
                    let rect = slots[order.len()].translate(content.min.to_vec2());
                    if self.summoning {
                        // The room holds the summon form, drawn over it by the app.
                        draw_summon_frame(ui.painter(), rect);
                        response.summon_rect = Some((rect, ui.clip_rect()));
                        if !self.summon_shown { ui.scroll_to_rect(rect.expand(4.0), None); }
                    } else {
                        let hit = ui.interact(rect, ui.id().with("summon-room"), egui::Sense::click());
                        if self.compact { draw_summon_line(ui.painter(), rect, hit.hovered()); } else { draw_summon_room(ui.painter(), rect, false, hit.hovered()); }
                        if hit.clicked() { response.summon = true; }
                    }
                    self.summon_shown = self.summoning;
                }
            });
        // Rooms hidden under the fold: a small badge, which scrolls to them.
        let view = output.inner_rect;
        let below = room_rects.iter().filter(|r| r.max.y > view.max.y + 8.0).count();
        let above = room_rects.iter().filter(|r| r.min.y < view.min.y - 8.0).count();
        for (count, down) in [(below, true), (above, false)] {
            if count == 0 { continue; }
            let text = if down { format!("{count} ↓") } else { format!("{count} ↑") };
            let galley = ui.painter().layout_no_wrap(text, FontId::monospace(11.0), Color32::WHITE);
            let size = galley.size() + vec2(12.0, 6.0);
            let corner = if down { pos2(view.max.x - 10.0 - size.x, view.max.y - 8.0 - size.y) } else { pos2(view.max.x - 10.0 - size.x, view.min.y + TOP_PAD + 2.0) };
            let rect = Rect::from_min_size(corner, size);
            let hit = ui.interact(rect, ui.id().with(if down { "fold-below" } else { "fold-above" }), egui::Sense::click());
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(4), Color32::from_rgba_unmultiplied(0, 0, 0, 200));
            painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 60)), egui::StrokeKind::Inside);
            painter.galley(rect.min + vec2(6.0, 3.0), galley, Color32::WHITE);
            if hit.clicked() {
                self.scroll_request = Some(if down { ScrollRequest::Down } else { ScrollRequest::Top });
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, status: &str) -> Agent {
        Agent::new(id, "claude", status, id, "Doing a long task that would never fit on one line of a window", &format!("/nonexistent/{id}"))
    }

    #[test]
    fn a_hero_walking_between_rooms_never_jumps_at_any_size() {
        for size in RoomSize::ALL {
            for (from, to) in [("idle", "working"), ("working", "idle"), ("blocked", "done"), ("done", "limited"), ("limited", "working")] {
                let mut room = Room::new(&agent("a", from), 1);
                room.size = size;
                room.update(&agent("a", from), false);
                room.update(&agent("a", to), true);
                let mut last = room.place_hero(room.hero_view().0);
                for _ in 0..600 {
                    room.advance(1.0 / 60.0);
                    if !room.in_transition { break; }
                    let now = room.place_hero(room.hero_view().0);
                    // Leaving the room is the one teleport: outside, out of sight.
                    let swapped = matches!(room.runner.as_ref().and_then(|r| r.current()), Some((Step { kind: StepKind::Walk { from, .. }, .. }, e)) if *from == OUTSIDE && e < 0.02);
                    assert!(swapped || (now - last).length() < 6.0, "{size:?} {from}→{to}: jumped from {last:?} to {now:?}");
                    last = now;
                }
            }
        }
    }

    #[test]
    fn a_dragged_order_holds_until_dropped() {
        let mut scene = Scene::new();
        scene.sync(&[agent("a", "idle"), agent("b", "idle"), agent("c", "idle")]);
        scene.order = vec!["c".into(), "a".into(), "b".into()];
        scene.dragging = Some(Drag { id: "c".into(), grab: vec2(0.0, 0.0) });
        // Herdr's order comes back each frame; the dragged one is kept, a new room comes last.
        scene.sync(&[agent("a", "idle"), agent("b", "idle"), agent("c", "idle"), agent("d", "idle")]);
        assert_eq!(scene.order, ["c", "a", "b", "d"]);
        // The dragged room closing lets go of the drag.
        scene.sync(&[agent("a", "idle"), agent("b", "idle")]);
        assert!(scene.dragging.is_none());
        assert_eq!(scene.order, ["a", "b"]);
    }

    #[test]
    fn the_guilds_room_is_the_workroom_with_the_director_at_the_desk_and_the_crew_at_the_stations() {
        let mut scene = Scene::new();
        let mut lead = agent("w1:p1", "blocked");
        lead.guild_room = true;
        scene.sync(&[agent("a", "idle"), lead.clone(), agent("b", "idle")]);
        let room = scene.rooms.get("w1:p1").unwrap();
        assert!(room.training, "the guild's room even before anyone is hired");
        assert_eq!((room.style.status, room.status.as_str()), ("working", "blocked"), "the workroom, whatever the director's state");
        assert_eq!(room.action.as_deref(), Some("type"), "the director stays at the desk");
        let member = |name: &str, status: &str, action: Option<&str>| crate::herdr::CrewMember { name: name.into(), harness: "kiro".into(), status: status.into(), action: action.map(str::to_string) };
        lead.status = "done".into();
        lead.crew = vec![member("Orin", "blocked", None), member("Aria", "working", Some("read")), member("Kael", "idle", None), member("Bo", "working", Some("brew")), member("Lira", "working", Some("forge"))];
        scene.sync(&[agent("a", "idle"), lead.clone(), agent("b", "idle")]);
        assert!(!scene.is_double(1));
        for columns in [1, 2, 3] {
            let (slots, _) = scene.layout(width_for(columns));
            assert_eq!(slots[1].size(), slots[0].size(), "{columns} columns: a room like the others");
        }
        let room = scene.rooms.get("w1:p1").unwrap();
        assert_eq!((room.style.status, room.status.as_str()), ("working", "done"), "a new state, the same room");
        let at = |name: &str| room.crew.iter().find(|c| c.name == name).unwrap().target;
        assert_eq!(at("Aria"), station("read").unwrap().0, "reading: at the bookshelf");
        assert_eq!(at("Bo"), station("brew").unwrap().0, "running commands: at the potions");
        assert_eq!(at("Lira"), pos2(32.0, 40.0), "writing: at the anvil");
        assert_eq!(at("Orin"), pos2(60.0, 72.0), "asking: before the director's desk");
        assert!(at("Kael").y > 80.0, "free: on the floor");
        // They walked in by the door; given time, they reach their spots, and the busy ones throw sparks.
        let room = scene.rooms.get_mut("w1:p1").unwrap();
        for _ in 0..200 { room.advance(0.05); }
        assert!(room.crew.iter().all(|c| c.pos == c.target));
        assert!(!room.particles.is_empty());
        // Asking for a permission: up and before the director; answered, back to its station.
        lead.crew[3].status = "blocked".into();
        scene.sync(&[agent("a", "idle"), lead.clone(), agent("b", "idle")]);
        assert_eq!(scene.rooms.get("w1:p1").unwrap().crew.iter().find(|c| c.name == "Bo").unwrap().target, pos2(84.0, 72.0));
        lead.crew[3].status = "working".into();
        scene.sync(&[agent("a", "idle"), lead.clone(), agent("b", "idle")]);
        assert_eq!(scene.rooms.get("w1:p1").unwrap().crew.iter().find(|c| c.name == "Bo").unwrap().target, station("brew").unwrap().0);
        // At every size, every spot lands inside the room.
        let room = scene.rooms.get_mut("w1:p1").unwrap();
        for size in RoomSize::ALL {
            room.size = size;
            let frame = room.drawn_frame();
            for status in ["blocked", "unknown", "idle", "done"] { for k in 0..4 { let p = room.place(Room::rp(Room::crew_spot(status, "", k).0)); assert!(frame.contains(p), "{status} {k} outside the {} room: {p:?}", size.key()); } }
            for action in ["read", "forge", "brew", "gems", "summon", "type", "plan"] { for k in 0..3 { let p = room.place(Room::rp(Room::crew_spot("working", action, k).0)); assert!(frame.contains(p), "{action} {k} outside the {} room: {p:?}", size.key()); } }
        }
        room.size = RoomSize::Large;
        // Someone hired comes in by the door; leaving guild mode gives the room back its own state.
        lead.crew.push(member("Nim", "idle", None));
        scene.sync(&[agent("a", "idle"), lead.clone(), agent("b", "idle")]);
        assert_eq!(scene.rooms.get("w1:p1").unwrap().crew.iter().find(|c| c.name == "Nim").unwrap().pos, DOOR);
        let mut plain = lead;
        plain.guild_room = false;
        plain.crew.clear();
        scene.sync(&[agent("a", "idle"), plain, agent("b", "idle")]);
        let room = scene.rooms.get("w1:p1").unwrap();
        assert!(!room.training);
        assert!(room.style.status == "done" || room.pending_style.map(|s| s.status) == Some("done"), "back to its own room");
    }

    #[test]
    fn every_room_keeps_the_menus_size() {
        let mut scene = Scene::new();
        scene.compact = true;
        scene.room_size = RoomSize::Small;
        let agents: Vec<Agent> = (0..12).map(|i| agent(&format!("w{i}"), ["working", "done", "idle", "limited", "blocked"][i % 5])).collect();
        scene.sync(&agents);
        // Choosing a room only outlines it: the column keeps its height.
        let before = scene.content_height(ROW_W);
        scene.set_selected(Some("w0".into()));
        scene.sync(&agents);
        assert_eq!(scene.content_height(ROW_W), before);
        assert_eq!(scene.size(), RoomSize::Small);
        let expected = TOP_PAD + 12.0 * RoomSize::Small.cell_h() + 11.0 * COLUMN_GAP + BOTTOM_PAD;
        assert!((before - expected).abs() < 0.5, "{before} vs {expected}");
        // A state change does not move the choice either.
        let mut done = agents.clone();
        done[0].status = "done".into();
        scene.sync(&done);
        assert_eq!(scene.selected.as_deref(), Some("w0"));
        // The widget's rooms are large.
        scene.compact = false;
        assert_eq!(scene.size(), RoomSize::Large);
    }

    #[test]
    fn shorter_rooms_keep_their_places() {
        assert_eq!(RoomSize::Large.cell_h(), ROW_H);
        assert!(RoomSize::Small.cell_h() < RoomSize::Medium.cell_h() && RoomSize::Medium.cell_h() < ROW_H);
        assert_eq!((RoomSize::from_key("small"), RoomSize::from_key("nonsense")), (RoomSize::Small, RoomSize::Medium));
        for size in [RoomSize::Medium, RoomSize::Small] {
            for style in STYLES {
                // Where the hero stands, walks in and works lands inside each room's shorter art.
                let mut places: Vec<(&str, Pos2)> = vec![("spot", style.spot), ("door", DOOR)];
                places.extend(style.route.iter().map(|p| ("route", *p)));
                if style.status == "working" { places.extend(["read", "forge", "brew", "gems", "summon"].iter().map(|n| (*n, station(n).unwrap().0))); }
                for (name, p) in places {
                    let q = size.remap(style.status, p);
                    assert!((0.0..=144.0).contains(&q.x) && (0.0..=size.art_h()).contains(&q.y), "{size:?} {} {name}: {q:?}", style.status);
                }
                assert_eq!(size.remap(style.status, pos2(72.0, 2.0)), pos2(72.0, 2.0), "the top wall stays");
            }
            // The hero stands behind the desk, and sleeps under its blanket, as in the large rooms.
            let working = style_for("working").spot;
            assert_eq!(size.remap("working", pos2(72.0, 44.0)).y - size.remap("working", working).y, 10.0, "{size:?}");
            let cot = style_for("limited").spot;
            assert_eq!(size.remap("limited", pos2(70.0, 53.0)).y - size.remap("limited", cot).y, 53.0 - cot.y, "{size:?}");
        }
    }

    #[test]
    fn kept_emblems_are_used_and_new_ones_kept() {
        use crate::identity::{SavedEmblem, Symbol};
        let mut scene = Scene::new();
        let kept = SavedEmblem { color: "pink".into(), metal: "ink".into(), charge: None, symbol: Symbol::Moon, pattern: crate::identity::Pattern::Chief, shape: crate::identity::Shape::Kite, kind: crate::identity::Kind::Unknown };
        scene.emblems.insert("kept".into(), kept.clone());
        scene.sync(&[agent("kept", "idle"), agent("fresh", "idle")]);
        let id = scene.identity("kept").unwrap();
        assert_eq!((id.symbol, crate::identity::COLOURS[id.primary].0), (Symbol::Moon, "pink"));
        assert!(scene.emblems_added && scene.emblems.contains_key("fresh"));
        // The demo's rooms are never kept.
        let mut demo = Scene::new();
        demo.sync(&crate::herdr::demo_agents(0));
        assert!(!demo.emblems_added && demo.emblems.is_empty());
    }

    #[test]
    fn first_room_on_entering_full_screen() {
        assert_eq!(first_room(&[agent("a", "done"), agent("b", "working"), agent("c", "working")]).as_deref(), Some("b"));
        assert_eq!(first_room(&[agent("a", "idle"), agent("b", "blocked")]).as_deref(), Some("b"));
        assert_eq!(first_room(&[]), None);
    }
}
