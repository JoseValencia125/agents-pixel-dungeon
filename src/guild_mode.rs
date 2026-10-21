//! Guild mode: an agent already running in Herdr becomes the guild's
//! orchestrator. One message asks it to run `guild orchestrate`, whose
//! brief tells it how to lead, which roles it can hire and the commands
//! that change the guild. Its room keeps the chat and gains tabs that read
//! the guild as the agent shapes it: the crew, the quests and the game
//! design document. The tabs only show; what is the user's to decide (a
//! permission, a candidate) is answered here as in the hall.

use crate::guild_view::{self, GuildView, PanelAction};
use crate::herdr::Agent;
use crate::l10n::{tr, trf};
use egui::{vec2, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use guild::design::{LivingGameDesign, Maturity};
use guild::domain::*;
use serde_json::json;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab { #[default] Chat, Guild, Quests, Design }

const INK: Color32 = Color32::from_rgb(222, 214, 232);
const DIM: Color32 = Color32::from_rgb(150, 143, 165);
const GOLD: Color32 = Color32::from_rgb(254, 174, 52);
const RED: Color32 = Color32::from_rgb(228, 59, 68);
const GREEN: Color32 = Color32::from_rgb(99, 199, 77);
const YELLOW: Color32 = Color32::from_rgb(254, 231, 97);

fn mono(text: &str, size: f32) -> RichText { RichText::new(text).font(egui::FontId::monospace(crate::ui::fs(size))) }

/// The guild's command line: next to the app, else on PATH.
pub fn guild_binary() -> String {
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(format!("agents-pixel-dungeon-guild{}", std::env::consts::EXE_SUFFIX))));
    beside.filter(|p| p.exists()).or_else(|| guild::platform::which("agents-pixel-dungeon-guild")).map(|p| p.display().to_string()).unwrap_or_else(|| "agents-pixel-dungeon-guild".into())
}

/// The one message that makes the agent the orchestrator, on one line
/// (each newline would press Enter in its terminal).
pub fn bootstrap(binary: &str, cwd: &str) -> String {
    let quote = |s: &str| if s.contains(' ') || s.contains('\'') { format!("'{}'", s.replace('\'', "'\\''")) } else { s.to_string() };
    trf("Modo guild: desde ahora eres el orquestador de la guild de este repositorio. Ejecuta `{} orchestrate {}`, lee todo lo que imprime y síguelo; luego dime en una línea que tomaste el mando.", &[&quote(binary), &quote(cwd)])
}

/// The guild's project for the agent's folder: the one whose repository
/// holds it (the deepest, if several), else the first.
pub fn project_for<'a>(snap: &'a Snapshot, cwd: &str) -> Option<&'a Project> {
    let inside = |p: &&Project| { let root = p.repo_path.trim_end_matches('/'); cwd == root || cwd.starts_with(&format!("{root}/")) };
    snap.projects.iter().filter(inside).max_by_key(|p| p.repo_path.len()).or(snap.projects.first())
}

/// Members waiting for the user: a permission, or a candidate to decide.
fn asking(snap: &Snapshot) -> Vec<(Member, Agent)> {
    // What the guild's orchestrator is deciding is not the user's yet.
    snap.members.iter().filter(|m| m.enabled && !guild_view::deciding(snap, m)).map(|m| (m.clone(), guild_view::member_agent(snap, m))).filter(|(_, a)| a.status == "blocked" || a.status == "done").collect()
}

/// The tabs over the room's panel, and a way out of guild mode. Returns
/// true when the user leaves it.
pub fn tab_strip(ui: &mut Ui, tab: &mut Tab, view: &GuildView, cwd: &str) -> bool {
    let mut leave = false;
    let snap = view.snapshot.as_ref();
    let project = snap.and_then(|s| project_for(s, cwd));
    let members = snap.map(|s| s.members.iter().filter(|m| m.enabled).count()).unwrap_or(0);
    let waiting = snap.map(|s| asking(s).len()).unwrap_or(0);
    let quests = snap.zip(project).map(|(s, p)| s.quests.iter().filter(|q| q.project_id == p.id && matches!(q.state, QuestState::Running | QuestState::Paused | QuestState::Blocked)).count()).unwrap_or(0);
    let design = snap.zip(project).and_then(|(s, p)| s.designs.iter().find(|d| d.project_id == p.id));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
        ui.spacing_mut().button_padding = vec2(8.0, 3.0);
        let mut chip = |ui: &mut Ui, which: Tab, label: String, hot: bool| {
            let on = *tab == which;
            let ink = if on { Color32::from_rgb(20, 18, 26) } else if hot { RED } else { INK };
            let fill = if on { GOLD } else { Color32::from_rgba_unmultiplied(254, 174, 52, 14) };
            let button = egui::Button::new(mono(&label, 11.0).strong().color(ink)).fill(fill).stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(254, 174, 52, 90))).corner_radius(CornerRadius::same(3));
            if ui.add(button).clicked() { *tab = which; }
        };
        chip(ui, Tab::Chat, tr("Chat"), false);
        chip(ui, Tab::Guild, if waiting > 0 { format!("{} · {members} · {waiting}!", tr("Gremio")) } else { format!("{} · {members}", tr("Gremio")) }, waiting > 0);
        chip(ui, Tab::Quests, format!("{} · {quests}", tr("Misiones")), false);
        chip(ui, Tab::Design, match design { Some(d) => format!("{} · {}%", tr("Diseño"), guild::design::maturity_percent(d)), None => tr("Diseño") }, false);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Narrow, the way out is a × with its words on hover.
            let label = if ui.available_width() > 190.0 { tr("Salir del modo guild") } else { "×".into() };
            let tip = format!("{} — {}", tr("Salir del modo guild"), tr("La sala vuelve a ser solo el chat; la guild y su trabajo siguen"));
            if ui.add(egui::Button::new(mono(&label, 10.0).color(DIM)).frame(false)).on_hover_text(tip).clicked() { leave = true; }
            if !view.connected && !view.demo && ui.available_width() > 120.0 { ui.label(mono(&tr("guild apagada"), 10.0).color(RED)); }
        });
    });
    ui.add_space(4.0);
    leave
}

#[derive(Default)]
pub struct BodyAction {
    pub terminal: Option<String>,
    pub studio: bool,
}

/// One of the guild's tabs (not the chat, which the room draws itself).
pub fn body(ui: &mut Ui, tab: Tab, view: &mut GuildView, cwd: &str, now: f64, wide: bool) -> BodyAction {
    let mut action = BodyAction::default();
    let Some(snap) = view.snapshot.clone() else {
        ui.label(mono(&tr("Arrancando la guild…"), 11.5).color(DIM));
        return action;
    };
    // A member's own work opens in place, from the crew or a quest's task.
    if tab != Tab::Design {
        if let Some(id) = view.hall_member.clone() {
            if let Some(member) = snap.members.iter().find(|m| m.id == id) {
                if ui.add(egui::Button::new(mono(&tr("← Volver"), 11.0).color(INK)).frame(false)).clicked() { view.hall_member = None; return action; }
                let inner: PanelAction = guild_view::panel(ui, view, &guild_view::member_agent(&snap, member), now, wide);
                if inner.close { view.hall_member = None; }
                action.terminal = inner.terminal;
                return action;
            }
            view.hall_member = None;
        }
    }
    view.watch(None);
    let project = project_for(&snap, cwd).cloned();
    egui::ScrollArea::vertical().id_salt(("guild-mode", tab as u8)).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        match tab {
            Tab::Guild => crew(ui, view, &snap),
            Tab::Quests => quests(ui, view, &snap, project.as_ref()),
            Tab::Design => action.studio = design(ui, &snap, project.as_ref(), wide),
            Tab::Chat => {}
        }
    });
    action
}

fn state_color(status: &str) -> Color32 {
    match status { "working" => YELLOW, "blocked" => RED, "done" => GREEN, _ => DIM }
}

/// The crew: how many work at once, who waits for the user, and every
/// member with its roles, harness and what it does now.
fn crew(ui: &mut Ui, view: &mut GuildView, snap: &Snapshot) {
    ui.horizontal(|ui| {
        ui.label(mono(&tr("Trabajan a la vez:"), 11.0).color(DIM));
        let n = snap.concurrency.max(1);
        if ui.add_enabled(n > 1, egui::Button::new(mono("−", 11.0))).clicked() { view.command("guild.concurrency", json!({"concurrency": n - 1})); }
        ui.label(mono(&n.to_string(), 12.0).strong().color(INK));
        if ui.add_enabled(n < 16, egui::Button::new(mono("+", 11.0))).clicked() { view.command("guild.concurrency", json!({"concurrency": n + 1})); }
    });
    let waiting = asking(snap);
    if !waiting.is_empty() {
        ui.label(mono(&tr("Necesita tu atención"), 10.5).strong().color(GOLD));
        for (m, a) in &waiting {
            let row = ui.add(egui::Label::new(mono(&format!("▸ {}", a.activity), 10.5).color(INK)).truncate().sense(egui::Sense::click()));
            if row.on_hover_text(tr("Ver su trabajo")).clicked() { view.hall_member = Some(m.id.clone()); }
        }
        ui.add_space(4.0);
    }
    if snap.members.is_empty() {
        ui.label(mono(&tr("Sin miembros todavía. Pídele al orquestador, en el Chat, que arme el equipo: elige los roles y cuántos según el trabajo."), 11.0).color(DIM));
        return;
    }
    let mut members: Vec<&Member> = snap.members.iter().collect();
    members.sort_by_key(|m| !m.enabled);
    for m in members {
        let agent = guild_view::member_agent(snap, m);
        let color = if m.enabled { state_color(&agent.status) } else { Color32::from_gray(90) };
        let roles: Vec<String> = m.roles.iter().map(|r| crate::guild_studio::role_name(&snap.roles, r)).collect();
        let frame = Frame::new().fill(Color32::from_rgba_unmultiplied(255, 255, 255, 6)).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(8, 5));
        let response = frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(mono("●", 11.0).color(color));
                ui.label(mono(&m.name, 12.0).strong().color(if m.enabled { INK } else { DIM }));
                ui.label(mono(&roles.join(", "), 10.5).color(GOLD));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(mono(&format!("{} · {}", m.harness, m.model_id.clone().unwrap_or_else(|| tr("modelo por defecto"))), 10.0).color(DIM));
                });
            });
            let doing = if m.enabled { agent.activity.clone() } else { tr("Desactivado") };
            ui.add(egui::Label::new(mono(&doing, 10.5).color(DIM)).truncate());
        }).response.interact(egui::Sense::click());
        if response.on_hover_text(tr("Ver su trabajo")).clicked() { view.hall_member = Some(m.id.clone()); }
    }
}

fn quest_state(state: QuestState) -> String {
    tr(match state {
        QuestState::Draft => "borrador",
        QuestState::Ready => "lista",
        QuestState::Running => "en curso",
        QuestState::Paused => "pausada",
        QuestState::Blocked => "bloqueada",
        QuestState::Completed => "completada",
        QuestState::Failed => "fallida",
        QuestState::Cancelled => "cancelada",
    })
}

fn task_glyph(state: TaskState) -> (&'static str, Color32) {
    match state {
        TaskState::Completed => ("●", GREEN),
        TaskState::Failed | TaskState::Blocked => ("!", RED),
        TaskState::Cancelled => ("–", DIM),
        TaskState::Running | TaskState::Integrating => ("▸", YELLOW),
        TaskState::Verifying | TaskState::Accepted => ("?", GOLD),
        _ => ("·", DIM),
    }
}

/// The project's quests, the ones under way first, each with its tasks:
/// state, role, who has it; a task with a member opens its work.
fn quests(ui: &mut Ui, view: &mut GuildView, snap: &Snapshot, project: Option<&Project>) {
    let Some(project) = project else {
        ui.label(mono(&tr("La guild aún no tiene este repositorio. El orquestador lo añade al pasar a modo guild."), 11.0).color(DIM));
        return;
    };
    let mut list: Vec<&Quest> = snap.quests.iter().filter(|q| q.project_id == project.id).collect();
    if list.is_empty() {
        ui.label(mono(&tr("Sin misiones. El orquestador las crea cuando le pides trabajo en el Chat."), 11.0).color(DIM));
        return;
    }
    let open = |q: &Quest| matches!(q.state, QuestState::Running | QuestState::Paused | QuestState::Blocked);
    list.sort_by(|a, b| open(b).cmp(&open(a)).then(b.created_at.cmp(&a.created_at)));
    for q in list {
        let tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == q.id).collect();
        let done = tasks.iter().filter(|t| t.state == TaskState::Completed).count();
        let frame = Frame::new().fill(Color32::from_rgba_unmultiplied(255, 255, 255, if open(q) { 8 } else { 3 })).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(8, 6));
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&q.title, 12.0).strong().color(if open(q) { INK } else { DIM }));
                ui.label(mono(&format!("{} · {done}/{}", quest_state(q.state), tasks.len()), 10.5).color(DIM));
                ui.label(mono(q.integration_ref.trim_start_matches("refs/heads/"), 10.0).color(Color32::from_rgb(44, 232, 245)));
            });
            for t in tasks {
                let (glyph, color) = task_glyph(t.state);
                let attempt = snap.attempts.iter().filter(|a| a.task_id == t.id).max_by(|a, b| a.created_at.cmp(&b.created_at));
                let member = attempt.and_then(|a| snap.members.iter().find(|m| m.id == a.member_id));
                let needs = attempt.is_some_and(|a| a.pending_permission.is_some() || matches!(a.state, AttemptState::Submitted | AttemptState::Passed)) && !matches!(t.state, TaskState::Completed | TaskState::Cancelled);
                let who = member.map(|m| format!(" · {}", m.name)).unwrap_or_default();
                let text = format!("{glyph} {} [{}]{who}{}", t.title, t.role, if needs { format!("  ⟵ {}", tr("te necesita")) } else { String::new() });
                let row = ui.add(egui::Label::new(mono(&text, 10.5).color(if needs { GOLD } else { color })).truncate().sense(egui::Sense::click()));
                let row = match &t.block_reason { Some(r) => row.on_hover_text(r), None => row };
                if let Some(m) = member { if row.clicked() { view.hall_member = Some(m.id.clone()); } }
            }
        });
    }
}

fn h1(ui: &mut Ui, text: &str) { ui.add_space(2.0); ui.label(mono(text, 16.0).strong().color(Color32::WHITE)); }
fn h2(ui: &mut Ui, text: &str, maturity: Option<Maturity>) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(mono(text, 13.0).strong().color(GOLD));
        if let Some(m) = maturity { ui.label(mono(&crate::guild_chamber::maturity_label(m), 10.0).color(DIM)); }
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(254, 174, 52, 50));
}
fn h3(ui: &mut Ui, text: &str) { ui.add_space(3.0); ui.label(mono(text, 11.5).strong().color(INK)); }
fn para(ui: &mut Ui, text: &str) { if !text.trim().is_empty() { ui.label(mono(text.trim(), 11.0).color(INK)); } }
fn field(ui: &mut Ui, label: &str, text: &str) {
    if text.trim().is_empty() { return; }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(mono(&format!("{label}:"), 11.0).strong().color(DIM));
        ui.label(mono(text.trim(), 11.0).color(INK));
    });
}
fn bullets(ui: &mut Ui, label: &str, items: &[String]) {
    if items.is_empty() { return; }
    if !label.is_empty() { ui.label(mono(label, 10.5).strong().color(DIM)); }
    for i in items { ui.label(mono(&format!("  • {}", i.trim()), 11.0).color(INK)); }
}
fn missing(ui: &mut Ui) { ui.label(mono(&tr("Aún sin escribir."), 10.5).italics().color(DIM)); }

/// The game design document: the living design on one page, broadest
/// first, unknowns said as such, open questions and decisions last.
/// Returns true when the user asks to edit it in the design chamber.
fn design(ui: &mut Ui, snap: &Snapshot, project: Option<&Project>, wide: bool) -> bool {
    let mut edit = false;
    let Some(d) = project.and_then(|p| snap.designs.iter().find(|d| d.project_id == p.id)) else {
        ui.label(mono(&tr("Aún no hay documento de diseño. Dile al orquestador, en el Chat, qué juego quieres hacer: propondrá el concepto y lo escribirá aquí."), 11.0).color(DIM));
        if ui.button(mono(&tr("Abrir la cámara de diseño"), 11.0)).clicked() { edit = true; }
        return edit;
    };
    let pid = &d.project_id;
    let questions: Vec<_> = snap.questions.iter().filter(|q| &q.project_id == pid).collect();
    let open: Vec<_> = questions.iter().filter(|q| q.status == "open").collect();
    let decisions: Vec<_> = snap.design_decisions.iter().filter(|x| &x.project_id == pid).collect();
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&trf("versión {} · {}% maduro · {} preguntas abiertas · {} decisiones", &[&d.version, &guild::design::maturity_percent(d), &open.len(), &decisions.len()]), 10.5).color(DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(mono(&tr("Editar en la cámara"), 10.5).color(INK))).clicked() { edit = true; }
        });
    });
    // A readable column in full screen, the whole width under the rooms.
    let column = if wide { ui.available_width().min(760.0) } else { ui.available_width() };
    ui.allocate_ui(vec2(column, 0.0), |ui| {
        ui.set_max_width(column);
        document(ui, d);
        h2(ui, &trf("Preguntas abiertas ({})", &[&open.len()]), None);
        if open.is_empty() { missing(ui); }
        for q in &open { ui.label(mono(&format!("{} · {}", q.id, q.question), 11.0).color(INK)); }
        h2(ui, &trf("Decisiones ({})", &[&decisions.len()]), None);
        if decisions.is_empty() { missing(ui); }
        for x in decisions.iter().rev() {
            ui.label(mono(&format!("{} · {}", x.id, x.decision), 11.0).color(INK));
            if !x.why.trim().is_empty() { ui.label(mono(&format!("    {}", x.why.trim()), 10.5).color(DIM)); }
        }
    });
    edit
}

fn document(ui: &mut Ui, d: &LivingGameDesign) {
    h1(ui, if d.title.trim().is_empty() { "—" } else { d.title.trim() });
    para(ui, &d.status);
    let c = &d.concept;
    h2(ui, &tr("Concepto"), Some(d.concept_maturity));
    if c.statement.trim().is_empty() && c.genre.trim().is_empty() && d.pillars.is_empty() { missing(ui); }
    para(ui, &c.statement);
    field(ui, &tr("Género"), &c.genre);
    field(ui, &tr("Audiencia"), &c.audience);
    bullets(ui, &tr("Puntos de venta únicos"), &c.usps);
    if !d.pillars.is_empty() {
        h3(ui, &tr("Pilares, del más importante al menos"));
        for (i, p) in d.pillars.iter().enumerate() {
            ui.label(mono(&format!("{}. {}{}", i + 1, p.name.trim(), if p.statement.trim().is_empty() { String::new() } else { format!(": {}", p.statement.trim()) }), 11.0).color(INK));
            if !p.design_test.trim().is_empty() { ui.label(mono(&format!("    {} {}", tr("Prueba:"), p.design_test.trim()), 10.5).color(DIM)); }
        }
    }
    bullets(ui, &tr("Anti-pilares"), &d.anti_pillars);
    field(ui, &tr("Mundo"), &c.world_fiction);
    field(ui, &tr("Plataforma y alcance"), &c.platform_scope);
    field(ui, &tr("Monetización"), &c.monetization);
    field(ui, &tr("Competencia"), &c.competition);
    let e = &d.experience;
    h2(ui, &tr("Experiencia del jugador"), Some(d.experience_maturity));
    if e.fantasy.trim().is_empty() && e.player_should_feel.is_empty() { missing(ui); }
    field(ui, &tr("Fantasía"), &e.fantasy);
    field(ui, &tr("Punto de vista"), &e.pov);
    bullets(ui, &tr("Debe sentir"), &e.player_should_feel);
    bullets(ui, &tr("Conductas deseadas"), &e.desired_behaviors);
    bullets(ui, &tr("Evitar"), &e.avoid);
    bullets(ui, &tr("Momentos clave"), &e.key_moments);
    field(ui, &tr("Qué lo mantiene"), &e.engagement);
    if !(d.style.visual.trim().is_empty() && d.style.audio.trim().is_empty()) {
        h3(ui, &tr("Estilo visual y sonoro"));
        field(ui, &tr("Visual"), &d.style.visual);
        field(ui, &tr("Audio"), &d.style.audio);
        bullets(ui, &tr("Referencias"), &d.style.references);
    }
    h2(ui, &tr("Loops"), Some(d.loops_maturity));
    if d.loops.is_empty() { missing(ui); }
    for l in &d.loops {
        h3(ui, &l.name);
        if !l.steps.is_empty() { ui.label(mono(&format!("{} ↺", l.steps.join(" → ")), 11.0).color(Color32::from_rgb(44, 232, 245))); }
        field(ui, &tr("Entradas"), &l.inputs.join(", "));
        field(ui, &tr("Salidas"), &l.outputs.join(", "));
        field(ui, &tr("Retroalimentación"), &l.feedback);
        field(ui, &tr("Falla cuando"), &l.failure);
    }
    if !d.objectives.trim().is_empty() { h2(ui, &tr("Objetivos y progresión"), None); para(ui, &d.objectives); }
    if !d.interactivity.trim().is_empty() { h2(ui, &tr("Interactividad"), None); para(ui, &d.interactivity); }
    h2(ui, &tr("Sistemas"), None);
    if d.systems.is_empty() { missing(ui); }
    for s in &d.systems {
        ui.add_space(3.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(mono(&s.name, 11.5).strong().color(INK));
            ui.label(mono(&crate::guild_chamber::maturity_label(s.maturity), 10.0).color(DIM));
        });
        field(ui, &tr("Intención"), &s.intent);
        para(ui, &s.current);
        bullets(ui, &tr("Sabido"), &s.known);
        bullets(ui, &tr("Por decidir"), &s.unknown);
        bullets(ui, &tr("Fórmulas"), &s.formulas);
        bullets(ui, &tr("Ajustes"), &s.tuning);
        bullets(ui, &tr("Casos límite"), &s.edge_cases);
        bullets(ui, &tr("Criterios de aceptación"), &s.acceptance);
        field(ui, &tr("Depende de"), &s.depends_on.join(", "));
    }
    if !d.parts.is_empty() {
        h2(ui, &tr("Objetos"), None);
        for p in &d.parts {
            h3(ui, &p.name);
            bullets(ui, "", &p.attributes);
            bullets(ui, "", &p.rules);
            para(ui, &p.notes);
        }
    }
    for s in d.sections.iter().filter(|s| !s.body.trim().is_empty()) {
        h2(ui, &s.title, Some(s.maturity));
        para(ui, &s.body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every piece of text one frame of a guild tab draws, on the demo guild.
    fn texts_on(tab: Tab) -> String {
        let ctx = egui::Context::default();
        let mut view = GuildView::new(std::sync::Arc::new(|| {}));
        view.set_demo(true);
        view.poll();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 6000.0))), ..Default::default() };
        let mut out = String::new();
        for _ in 0..2 {
            let output = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut t = tab;
                    tab_strip(ui, &mut t, &view, "~/demo/verbrawl/src");
                    body(ui, tab, &mut view, "~/demo/verbrawl/src", 0.0, true);
                });
            });
            out.clear();
            for clipped in output.shapes {
                if let egui::Shape::Text(t) = clipped.shape { out.push_str(&t.galley.job.text); out.push('\n'); }
            }
        }
        out
    }

    #[test]
    fn the_tabs_show_the_crew_the_quests_and_the_design_document() {
        let crew = texts_on(Tab::Guild);
        for needle in [tr("Chat"), tr("Salir del modo guild"), tr("Trabajan a la vez:"), "3".into()] {
            assert!(crew.contains(needle.as_str()), "crew lacks {needle:?}:\n{crew}");
        }
        let quests = texts_on(Tab::Quests);
        assert!(quests.contains("guild/quest/"), "quests show their branch:\n{quests}");
        let design = texts_on(Tab::Design);
        for needle in ["Verbrawl".to_string(), tr("Concepto"), tr("Experiencia del jugador"), tr("Sistemas"), trf("Preguntas abiertas ({})", &[&1]), tr("Editar en la cámara")] {
            assert!(design.contains(needle.as_str()), "design lacks {needle:?}:\n{design}");
        }
    }

    #[test]
    fn what_the_orchestrator_decides_is_not_the_users_and_what_it_escalates_is() {
        let mut view = GuildView::new(std::sync::Arc::new(|| {}));
        view.set_demo(true);
        view.poll();
        let mut snap = view.snapshot.clone().unwrap();
        let orin = snap.members.iter().find(|m| m.name == "Orin").unwrap().clone();
        assert!(asking(&snap).iter().any(|(m, _)| m.id == orin.id), "the demo's Orin asks for a permission");
        let a = snap.attempts.iter_mut().find(|a| a.member_id == orin.id && a.pending_permission.is_some()).unwrap();
        a.deciding = true;
        assert!(!asking(&snap).iter().any(|(m, _)| m.id == orin.id), "the orchestrator is on it");
        assert!(guild_view::member_agent(&snap, &orin).activity.contains(&trf("{} · el orquestador decide", &[&""]).trim_start_matches(" · ").to_string()));
        let a = snap.attempts.iter_mut().find(|a| a.member_id == orin.id && a.pending_permission.is_some()).unwrap();
        a.deciding = false;
        a.escalation = Some("it installs software".into());
        assert!(asking(&snap).iter().any(|(m, _)| m.id == orin.id), "escalated: the user's");
        assert!(guild_view::member_agent(&snap, &orin).activity.contains("it installs software"));
    }

    #[test]
    fn the_message_is_one_line_with_the_command_quoted() {
        let m = bootstrap("/Applications/Agents Pixel Dungeon.app/Contents/MacOS/agents-pixel-dungeon-guild", "/Users/x/my game");
        assert!(!m.contains('\n'));
        assert!(m.contains("'/Applications/Agents Pixel Dungeon.app/Contents/MacOS/agents-pixel-dungeon-guild' orchestrate '/Users/x/my game'"), "{m}");
    }

    #[test]
    fn the_project_is_the_repository_that_holds_the_folder() {
        let p = |id: &str, path: &str| Project { id: id.into(), name: id.into(), repo_path: path.into(), target_ref: "refs/heads/main".into(), setup: vec![], verifiers: vec![], playtest: None, play: None, ai_play: None, permitted_programs: vec![], created_at: String::new() };
        let snap = Snapshot { projects: vec![p("a", "/r/app"), p("b", "/r/app/game"), p("c", "/r/application")], ..Default::default() };
        assert_eq!(project_for(&snap, "/r/app/game/src").map(|p| p.id.as_str()), Some("b"));
        assert_eq!(project_for(&snap, "/r/app").map(|p| p.id.as_str()), Some("a"));
        assert_eq!(project_for(&snap, "/r/applications").map(|p| p.id.as_str()), Some("a"), "no prefix match: the first project");
    }
}
