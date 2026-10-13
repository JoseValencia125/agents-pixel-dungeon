//! The dungeon: one room per agent, drawn with egui's painter. Each room
//! shows the art of the agent's state and a hero that works at the right
//! station, jumps for attention, rests exhausted, reads, celebrates or
//! wanders; status changes walk the hero out through the door and into the new room.
//! Animation is driven by elapsed time, so Reduce Motion simply stops it.

use crate::assets::{Assets, Sprite};
use crate::herdr::{harness_for, Agent};
use crate::l10n::tr;
use egui::emath::Rot2;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::collections::{HashMap, HashSet};

pub const ART: Vec2 = vec2(144.0, 100.0);
pub const FOOTER: f32 = 36.0; // caption band under the room: chat title + folder · branch
pub const ROW_H: f32 = 158.0 + FOOTER; // room art 100 px at 1.5 pt/px + 8 pt frame, plus the footer
pub const ROW_W: f32 = 220.0; // room art 144 px at 1.5 pt/px + 4 pt frame
pub const HERO: f32 = 32.0; // 16 px sprite at 2 pt/px
pub const GAP: f32 = 6.0;
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
    RoomStyle { status: "idle", title: "EN ESPERA", background: "room_idle", color: Color32::from_rgb(152, 152, 157), bubble: None, spot: pos2(88.0, 40.0), roam: 0.0, lying: false,
                route: &[pos2(72.0, 80.0), pos2(98.0, 74.0), pos2(98.0, 42.0)] },
    RoomStyle { status: "done", title: "LISTO", background: "room_done", color: Color32::from_rgb(48, 209, 88), bubble: None, spot: pos2(72.0, 66.0), roam: 0.0, lying: false, route: &[] },
    RoomStyle { status: "unknown", title: "SIN ESTADO", background: "room_unknown", color: Color32::from_rgb(152, 152, 157), bubble: Some("…"), spot: pos2(72.0, 62.0), roam: 24.0, lying: false, route: &[] },
];

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
    pos: Pos2,
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
    color: Color32,
    activity: String,
    repo: String,
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
            color: agent_color(&agent.status),
            activity: String::new(),
            repo: String::new(),
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
        };
        room.update(agent, false);
        room
    }

    // ---- Geometry

    /// The room frame inside the cell.
    fn frame() -> Rect {
        Rect::from_min_size(pos2(2.0, 4.0), vec2(ROW_W - 4.0, ROW_H - 8.0 - FOOTER))
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
        self.color = agent_color(&agent.status);
        self.activity = agent.activity.clone();
        self.repo = agent.folder();
        self.branch = agent.branch.clone().unwrap_or_default();
        self.shells = agent.shells.clone();
        self.harness = harness_for(&agent.name);
        self.limit_resets = agent.limit_resets;
        if agent.action != self.action {
            self.action = agent.action.clone();
            if self.style.status == "working" && agent.status == "working" && !self.in_transition {
                self.restart_working();
            }
        }
        if agent.status != self.status {
            self.status = agent.status.clone();
            self.apply_room(style_for(&agent.status), animated);
        }
        if agent.subagents != self.subagents || agent.subagent_actions != self.subagent_actions {
            self.subagents = agent.subagents;
            self.subagent_actions = agent.subagent_actions.clone();
            self.set_party();
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
            match name { "type" | "plan" => spot, other => station(other).map(|s| s.0).unwrap_or(spot) }
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
            if let Some(mini) = self.party.get_mut(i) {
                mini.target = home;
                mini.action = action;
                mini.facing = facing;
                mini.front = front;
            } else {
                self.party.push(Mini { pos: home, target: home, action, facing, front });
            }
        }
    }

    // ---- Per-frame animation

    pub fn advance(&mut self, dt: f32) {
        if dt <= 0.0 { return; }
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
                    // A page turns every few seconds; the hero nods with it.
                    let p = rt % 4.2;
                    let turning = p > 3.9;
                    if turning { pos.y -= 1.0; }
                    book = Some((if turning { "book_open_flipped" } else { "book_open" }, vec2(18.0, 12.0), pos2(0.0, 5.0)));
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
                        let hearth = Room::rp(pos2(70.0, 22.0));
                        let color = self.rng.pick(&[GOLD, ORANGE]);
                        let dx = self.rng.range(-4.0, 4.0);
                        let dy = -self.rng.range(8.0, 14.0);
                        spawned.push(Particle { kind: ParticleKind::Bit { color, size: 2.0 }, origin: pos2(hearth.x + self.rng.range(-14.0, 14.0), hearth.y), delta: vec2(dx, dy), life: 0.9, age: 0.0 });
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
    pub fn draw(&self, painter: &Painter, origin: Pos2, assets: &Assets, selected: bool) {
        let at = |p: Pos2| pos2(origin.x + p.x, origin.y + p.y);
        let f = Room::frame().translate(origin.to_vec2());
        let cell = Rect::from_min_max(f.min, pos2(f.max.x, f.max.y + FOOTER));
        let t = self.routine_clock;
        let style = self.style;
        // Footer band under the room.
        painter.rect_filled(cell, CornerRadius::same(6), Color32::from_rgb(18, 15, 23));
        let clipped = painter.with_clip_rect(f);
        // Backdrop, fading in after a room change.
        let fade = self.swap_at.map(|s| ((self.clock - s) / (0.35 / 1.5)).clamp(0.0, 1.0)).unwrap_or(1.0);
        let tint = with_alpha(Color32::WHITE, fade);
        if let Some(room) = assets.room(style.background) {
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
        let hero_at = at(hero_pos);
        if style.lying && !self.in_transition {
            let shuffle = t - (self.shuffle_at - self.shuffle_period);
            let wobble = if shuffle < 0.15 { lerp(0.0, 0.12, shuffle / 0.15) } else if shuffle < 0.35 { lerp(0.12, -0.08, (shuffle - 0.15) / 0.2) } else if shuffle < 0.6 { lerp(-0.08, 0.0, (shuffle - 0.35) / 0.25) } else { 0.0 };
            let sleeper = assets.sleeper(self.harness);
            draw_sprite(&clipped, sleeper, hero_at, sleeper.px * 2.0, false, -(std::f32::consts::FRAC_PI_2 + wobble), Color32::WHITE);
        } else {
            // In the chamber the hero reads in an armchair by the fire: the
            // chair's back stands behind it, its arms and seat go over its
            // legs once it has sat down.
            let seat = at(Room::rp(style.spot));
            let seated = style.status == "idle" && !self.in_transition && self.runner.is_none();
            if style.status == "idle" {
                let back = assets.prop("chair_back");
                draw_sprite(&clipped, back, pos2(seat.x, seat.y - 10.0), back.px * 2.0, false, 0.0, Color32::WHITE);
            }
            let sprite = assets.hero_frame(self.harness, kind, index);
            draw_sprite(&clipped, sprite, hero_at, vec2(HERO, HERO), self.facing < 0.0, 0.0, Color32::WHITE);
            if seated {
                let front = assets.prop("chair_front");
                draw_sprite(&clipped, front, pos2(seat.x, seat.y + 8.0), front.px * 2.0, false, 0.0, Color32::WHITE);
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
            if let Some(blanket) = assets.room_part(style.background, 70.0, 53.0, 20.0, 9.0) {
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
        if let Some(front) = assets.room(&format!("{}_front", style.background)) {
            clipped.image(front.tex, f, front.uv, tint);
        }
        if self.party_visible {
            for mini in self.party.iter().filter(|m| m.front) { self.draw_mini(&clipped, mini, origin, assets, mini_tint); }
        }
        // Night in the chamber: only the fireplace lights the room.
        if style.status == "idle" && !self.in_transition {
            let hearth = at(Room::rp(pos2(70.0, 22.0)));
            let reach = f.width() * 2.0;
            let e = t % 4.0;
            let s = if e < 1.8 { lerp(0.97, 1.05, e / 1.8) } else { lerp(1.05, 0.97, (e - 1.8) / 2.2) };
            draw_sprite(&clipped, assets.prop("darkness"), hearth, vec2(reach * s, reach * s), false, 0.0, Color32::WHITE);
            let fire = 0.15 + 0.05 * (t * 2.3 + self.rng.0 as f32 % 7.0).sin() * (t * 0.7).cos();
            draw_sprite(&clipped, glow, pos2(hearth.x + 6.0, hearth.y + 8.0), vec2(120.0, 84.0), false, 0.0, with_alpha(Color32::from_rgb(255, 140, 51), fire));
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
        // Rounded corners: cover what the square clip let through.
        cover_corners(painter, f, 6.0, BACKGROUND);
        cover_corners(painter, cell, 6.0, BACKGROUND);
        // Frame.
        let stroke = if style.status == "blocked" && !self.in_transition {
            let bright = Color32::from_rgb(255, 51, 56);
            let dim = Color32::from_rgb(89, 13, 20);
            Stroke::new(3.0_f32, if (self.clock % 0.7) < 0.35 { bright } else { dim })
        } else if style.status == "limited" && !self.in_transition {
            let q = (std::f32::consts::TAU * self.clock / 2.4).sin() * 0.5 + 0.5;
            Stroke::new(2.0_f32, blend(Color32::from_rgb(74, 30, 99), LIMITED, q))
        } else {
            Stroke::new(1.0_f32, with_alpha(style.color, 0.85))
        };
        painter.rect_stroke(cell, CornerRadius::same(6), stroke, egui::StrokeKind::Inside);
        // Labels: the repo (top-left), then the footer: what the agent is
        // doing, its branch (bottom-left) and the room's state (bottom-right).
        let repo_chars = (((f.width() - 32.0) / 7.8) as usize).max(4);
        label(painter, pos2(f.min.x + 12.0, f.min.y + 14.0), Align2::LEFT_CENTER, &Room::clip(&self.repo, repo_chars), 13.0, self.color, true);
        let usable_chars = ((f.width() - 28.0) / 6.65) as usize;
        painter.text(pos2(f.min.x + 10.0, f.max.y + 11.0), Align2::LEFT_CENTER, Room::clip(&self.activity, usable_chars), FontId::monospace(11.0), Color32::WHITE);
        // The new state shows at once, while the hero is still walking
        // over to its new room.
        let shown = self.pending_style.unwrap_or(style);
        let title = tr(shown.title) + &if self.subagents > 0 && self.subagents != usize::MAX { format!(" +{}", self.subagents) } else { String::new() };
        let title_color = blend(shown.color, Color32::WHITE, 0.45);
        let at = pos2(f.max.x - 10.0, f.max.y + FOOTER - 10.0);
        painter.text(at, Align2::RIGHT_CENTER, &title, FontId::monospace(11.0), title_color);
        let title_rect = painter.text(pos2(at.x + 0.6, at.y), Align2::RIGHT_CENTER, &title, FontId::monospace(11.0), title_color);
        let branch_chars = (((title_rect.min.x - f.min.x - 22.0) / 6.65) as usize).max(4);
        painter.text(pos2(f.min.x + 10.0, at.y), Align2::LEFT_CENTER, Room::clip(&self.branch, branch_chars), FontId::monospace(11.0), Color32::from_gray(209));
        if selected {
            painter.rect_stroke(cell.expand(2.0), CornerRadius::same(7), Stroke::new(2.0_f32, Color32::from_rgb(217, 242, 166)), egui::StrokeKind::Outside);
        }
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
        let f = Room::frame().translate(origin.to_vec2());
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
        let foot = Room::rp(pos2(72.0, 73.0));
        let pill = Rect::from_center_size(pos2(origin.x + foot.x, origin.y + foot.y), vec2(width + 8.0, galley.size().y + 4.0));
        clipped.rect_filled(pill, CornerRadius::same(3), Color32::from_black_alpha(200));
        clipped.rect_stroke(pill, CornerRadius::same(3), Stroke::new(1.0_f32, with_alpha(LIMITED, 0.8)), egui::StrokeKind::Inside);
        draw_sprite(clipped, assets.prop("hourglass"), pos2(pill.min.x + 10.0, pill.center().y), vec2(10.0, 10.0), false, 0.0, Color32::WHITE);
        clipped.galley(pos2(pill.min.x + 20.0, pill.center().y - galley.size().y / 2.0), galley, ink);
    }

    fn draw_mini(&self, painter: &Painter, mini: &Mini, origin: Pos2, assets: &Assets, tint: Color32) {
        let frame = (self.clock / 0.16) as usize % 3;
        let sprite = assets.hero_frame(self.harness, "work", frame);
        let at = pos2(origin.x + mini.pos.x, origin.y + mini.pos.y);
        draw_sprite(painter, sprite, at, vec2(24.0, 24.0), mini.facing < 0.0, 0.0, tint);
        // Readers and planners hold an open book or scroll.
        if mini.action == "read" || mini.action == "plan" {
            let prop = assets.prop(if mini.action == "plan" { "scroll_open" } else { "book_open" });
            draw_sprite(painter, prop, pos2(at.x, at.y + 4.0), vec2(12.0, 8.0), false, 0.0, Color32::WHITE);
        }
    }

    fn draw_particle(&self, painter: &Painter, p: &Particle, origin: Pos2, assets: &Assets) {
        let k = (p.age / p.life).clamp(0.0, 1.0);
        let at = |q: Pos2| pos2(origin.x + q.x, origin.y + q.y);
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
    /// The summon panel is open: the spare room holds its form.
    pub summoning: bool,
    summon_shown: bool,
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
            summoning: false,
            summon_shown: false,
            seeds: 7,
            clock: 0.0,
            last_time: None,
        }
    }

    pub fn set_selected(&mut self, id: Option<String>) {
        if self.selected != id {
            self.selected = id.clone();
            if let Some(id) = id { self.scroll_request = Some(ScrollRequest::Reveal(id)); }
        }
    }

    /// Show these agents (already sorted, attention first). Someone new
    /// needing attention takes the first room: scroll up to it.
    pub fn sync(&mut self, agents: &[Agent]) {
        let ids: HashSet<&str> = agents.iter().map(|a| a.id.as_str()).collect();
        self.rooms.retain(|id, _| ids.contains(id.as_str()));
        let asking: HashSet<String> = agents.iter().filter(|a| a.status == "blocked").map(|a| a.id.clone()).collect();
        let newly_asking = asking.difference(&self.asking_before).next().is_some();
        self.asking_before = asking;
        self.order = agents.iter().map(|a| a.id.clone()).collect();
        if newly_asking && self.selected.is_none() { self.scroll_request = Some(ScrollRequest::Top); }
        for agent in agents {
            match self.rooms.get_mut(&agent.id) {
                Some(room) => room.update(agent, !self.reduced_motion),
                None => {
                    self.seeds = self.seeds.wrapping_mul(1664525).wrapping_add(1013904223);
                    self.rooms.insert(agent.id.clone(), Room::new(agent, self.seeds));
                }
            }
        }
    }

    fn slots(&self) -> usize {
        self.order.len() + usize::from(self.summon_tile)
    }

    pub fn content_height(&self, width: f32) -> f32 {
        let columns = columns_fitting(width);
        let lines = (self.slots() + columns - 1) / columns;
        TOP_PAD + lines as f32 * ROW_H + lines.saturating_sub(1) as f32 * GAP + BOTTOM_PAD
    }

    /// Lay the rooms out in the given width and draw them inside a scroll
    /// area; returns what the user clicked.
    pub fn ui(&mut self, ui: &mut egui::Ui, assets: &Assets, now: f64) -> SceneResponse {
        let dt = match self.last_time { Some(last) if !self.reduced_motion => ((now - last) as f32).clamp(0.0, 0.1), _ => 0.0 };
        self.last_time = Some(now);
        self.clock += dt;
        let mut response = SceneResponse { clicked: None, finish: None, terminal: None, summon: false, summon_rect: None };
        let width = ui.available_width();
        let columns = columns_fitting(width);
        let gap = GAP.max((width - columns as f32 * ROW_W) / (columns + 1) as f32);
        let height = self.content_height(width);
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
                for (index, id) in order.iter().enumerate() {
                    let Some(room) = self.rooms.get_mut(id) else { continue };
                    let col = index % columns;
                    let line = index / columns;
                    let target = pos2(gap + col as f32 * (ROW_W + gap), TOP_PAD + line as f32 * (ROW_H + GAP));
                    if room.pos != target {
                        room.move_from = room.shown.unwrap_or(target);
                        room.move_at = self.clock;
                        room.pos = target;
                    }
                    let k = if self.reduced_motion { 1.0 } else { ((self.clock - room.move_at) / 0.25).clamp(0.0, 1.0) };
                    let shown = if room.shown.is_none() { target } else { lerp_pos(room.move_from, target, k) };
                    room.shown = Some(shown);
                    room.advance(dt);
                    let rect = Rect::from_min_size(pos2(content.min.x + shown.x, content.min.y + shown.y), vec2(ROW_W, ROW_H));
                    if let Some(ScrollRequest::Reveal(wanted)) = &scroll_request {
                        if wanted == id { ui.scroll_to_rect(rect.expand(4.0), None); }
                    }
                    room_rects.push(rect);
                    let hit = ui.interact(rect, ui.id().with(id), egui::Sense::click());
                    room.draw(ui.painter(), rect.min, assets, self.selected.as_deref() == Some(id));
                    if hit.clicked() { response.clicked = Some(id.clone()); }
                    if let Some((area, rows)) = hit.hover_pos().and_then(|p| room.shells_tip(rect.min, p)) { crate::ui::shells_note(ui.ctx(), area, &rows); }
                    hit.context_menu(|ui| {
                        if ui.button(tr("Abrir una terminal en esta carpeta")).clicked() {
                            response.terminal = Some(id.clone());
                            ui.close_menu();
                        }
                        if ui.button(tr("Finalizar agente (/exit)")).clicked() {
                            response.finish = Some(id.clone());
                            ui.close_menu();
                        }
                    });
                }
                if self.summon_tile {
                    let index = order.len();
                    let at = pos2(gap + (index % columns) as f32 * (ROW_W + gap), TOP_PAD + (index / columns) as f32 * (ROW_H + GAP));
                    let rect = Rect::from_min_size(content.min + at.to_vec2(), vec2(ROW_W, ROW_H));
                    if self.summoning {
                        // The room holds the summon form, drawn over it by the app.
                        draw_summon_frame(ui.painter(), rect);
                        response.summon_rect = Some((rect, ui.clip_rect()));
                        if !self.summon_shown { ui.scroll_to_rect(rect.expand(4.0), None); }
                    } else {
                        let hit = ui.interact(rect, ui.id().with("summon-room"), egui::Sense::click());
                        draw_summon_room(ui.painter(), rect, false, hit.hovered());
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
