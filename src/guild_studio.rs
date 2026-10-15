//! The guild window: found a guild for a repository (its members with
//! their roles, from the catalogue, and a design template), then shape
//! the game design in Sellers' three levels, the experience, the loops
//! and the parts, which every attempt's prompt ends with. Drafts live
//! here until **Save** sends the whole design to the service.

use crate::guild_view::GuildView;
use crate::l10n::{tr, trf};
use egui::{vec2, Color32, Context, CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use guild::config::Role;
use guild::domain::*;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab { #[default] Guild, Concept, Experience, Loops, Systems, Parts, Play, Laboratory, Questions, Decisions }

#[derive(Clone, Default)]
pub struct MemberRow {
    pub name: String,
    pub role: String,
    pub harness: String,
    pub model: String,
}

#[derive(Default)]
pub struct FoundForm {
    pub repo_path: String,
    pub name: String,
    /// videogame, blank or none
    pub template: String,
    pub members: Vec<MemberRow>,
}

pub use crate::guild_chamber::Draft;

#[derive(Default)]
pub struct Studio {
    pub tab: Tab,
    pub project_id: String,
    pub found: FoundForm,
    pub founding: bool,
    pub draft: Draft,
    pub dirty: bool,
    /// The (project, version) the draft was loaded from.
    loaded: Option<(String, u32)>,
    /// Per-member edits on the guild tab, until applied.
    member_edits: HashMap<String, MemberRow>,
    pub chamber: crate::guild_chamber::ChamberState,
    /// The tallest the window grows (600 points unless set).
    pub max_height: Option<f32>,
}

#[derive(Default)]
pub struct StudioAction { pub close: bool }

pub(crate) const PAPER: Color32 = Color32::from_rgb(237, 217, 168);
pub(crate) const PAPER_DARK: Color32 = Color32::from_rgb(229, 206, 152);
pub(crate) const INK: Color32 = Color32::from_rgb(61, 38, 20);
pub(crate) const FAINT: Color32 = Color32::from_rgb(120, 90, 60);
pub(crate) const RUST: Color32 = Color32::from_rgb(160, 120, 70);

pub(crate) fn mono(text: &str, size: f32) -> RichText { RichText::new(text).font(egui::FontId::monospace(crate::ui::fs(size))).color(INK) }

pub(crate) fn small(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(mono(text, 10.0).strong()).fill(Color32::from_rgb(222, 196, 140)).stroke(Stroke::new(1.0_f32, RUST)).corner_radius(CornerRadius::same(3))).clicked()
}

pub(crate) fn primary(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(mono(text, 10.5).strong().color(Color32::from_rgb(250, 240, 220))).fill(Color32::from_rgb(140, 80, 30)).stroke(Stroke::NONE).corner_radius(CornerRadius::same(3))).clicked()
}

pub(crate) fn field(ui: &mut Ui, label: &str, text: &mut String, width: f32) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(label, 10.5).color(FAINT));
        let width = width.min((ui.available_width() - 4.0).max(80.0));
        ui.add(egui::TextEdit::singleline(text).desired_width(width).font(egui::FontId::monospace(crate::ui::fs(10.5)))).changed()
    }).inner
}

pub(crate) fn area(ui: &mut Ui, label: &str, text: &mut String, rows: usize) -> bool {
    ui.label(mono(label, 10.5).color(FAINT));
    ui.add(egui::TextEdit::multiline(text).desired_rows(rows).desired_width(f32::INFINITY).font(egui::FontId::monospace(crate::ui::fs(10.5)))).changed()
}

/// The role's name as shown: the catalogue's, translated.
pub fn role_name(roles: &[Role], id: &str) -> String {
    roles.iter().find(|r| r.id == id).map(|r| tr(&r.name)).unwrap_or_else(|| id.to_string())
}

/// The members a video game guild starts with, on the harnesses at hand.
pub fn videogame_members(harnesses: &[String]) -> Vec<MemberRow> {
    let pick = |preferred: &str| if harnesses.iter().any(|h| h == preferred) { preferred.to_string() } else { harnesses.first().cloned().unwrap_or_else(|| "kiro".into()) };
    let model_for = |h: &str| match h { "kiro" => "claude-sonnet-5.5", "claude" => "sonnet", _ => "" }.to_string();
    [("Aria", "planner", "kiro"), ("Sellers", "systems-designer", "kiro"), ("Orin", "gameplay-engineer", "kiro"), ("Lira", "reviewer", "codex"), ("Mara", "art-director", "claude"), ("Kael", "exploit-hunter", "kiro")]
        .iter()
        .map(|(n, r, h)| { let h = pick(h); MemberRow { name: n.to_string(), role: r.to_string(), harness: h.clone(), model: model_for(&h) } })
        .collect()
}

pub fn studio(ctx: &Context, view: &mut GuildView) -> StudioAction {
    let mut action = StudioAction::default();
    let mut commands: Vec<(&'static str, Value)> = vec![];
    let mut requests: Vec<(&'static str, Value, &'static str)> = vec![];
    let results = view.results.clone();
    let Some(snap) = view.snapshot.clone() else {
        egui::Window::new(mono(&tr("Gremio"), 12.0).strong()).id(egui::Id::new("guild-studio")).collapsible(false).resizable(false).frame(paper()).show(ctx, |ui| {
            parchment(ui);
            ui.label(mono(&tr("Servicio de la guild apagado"), 11.0).color(FAINT));
            if small(ui, &tr("Arrancar el servicio")) { view.start_service(); }
            if small(ui, &tr("Cerrar")) { action.close = true; }
        });
        return action;
    };
    let studio = &mut view.studio;
    if studio.project_id.is_empty() || !snap.projects.iter().any(|p| p.id == studio.project_id) {
        studio.project_id = snap.projects.first().map(|p| p.id.clone()).unwrap_or_default();
    }
    let design = snap.designs.iter().find(|d| d.project_id == studio.project_id).cloned();
    // Load the draft from the service's design when it is new to us and ours is clean.
    let current = design.as_ref().map(|d| (d.project_id.clone(), d.version));
    if current != studio.loaded && !studio.dirty {
        studio.draft = design.as_ref().map(Draft::from_design).unwrap_or_default();
        studio.loaded = current.clone();
    }
    let founding = studio.founding || snap.projects.is_empty();
    let harnesses: Vec<String> = snap.harnesses.iter().filter(|h| h.available).map(|h| h.kind.to_string()).collect();

    // Fit the widget's window: the dungeon is often a narrow strip.
    let screen = ctx.screen_rect();
    let width = (screen.width() - 24.0).clamp(180.0, 720.0);
    let height = (screen.height() - 64.0).clamp(200.0, studio.max_height.unwrap_or(600.0));
    egui::Window::new(mono(&tr("Gremio"), 12.0).strong()).id(egui::Id::new("guild-studio")).collapsible(false).resizable(false).fixed_size([width, height]).constrain_to(screen).frame(paper()).show(ctx, |ui| {
        parchment(ui);
        ui.set_width(width - 20.0);
        ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
        // Header: project, tabs, close.
        ui.horizontal(|ui| {
            if !snap.projects.is_empty() {
                let shown = snap.projects.iter().find(|p| p.id == studio.project_id).map(|p| p.name.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("studio-project").selected_text(mono(&shown, 10.5)).width(140.0).show_ui(ui, |ui| {
                    for p in &snap.projects {
                        if ui.selectable_value(&mut studio.project_id, p.id.clone(), mono(&p.name, 10.5)).changed() { studio.dirty = false; studio.loaded = None; }
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(mono("x", 11.0).strong()).frame(false)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
                if studio.dirty { ui.label(mono(&tr("sin guardar"), 10.0).color(Color32::from_rgb(190, 40, 30))); }
                else if let Some(d) = &design { ui.label(mono(&format!("v{}", d.version), 10.0).color(FAINT)); }
            });
        });
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [(Tab::Guild, tr("Gremio")), (Tab::Concept, tr("Concepto")), (Tab::Experience, tr("Experiencia")), (Tab::Loops, tr("Loops")), (Tab::Systems, tr("Sistemas")), (Tab::Parts, tr("Partes")), (Tab::Play, tr("Jugar")), (Tab::Laboratory, tr("Laboratorio")), (Tab::Questions, tr("Preguntas")), (Tab::Decisions, tr("Decisiones"))] {
                let on = studio.tab == tab;
                let fill = if on { Color32::from_rgb(140, 80, 30) } else { Color32::from_rgb(222, 196, 140) };
                let color = if on { Color32::from_rgb(250, 240, 220) } else { INK };
                if ui.add(egui::Button::new(mono(&label, 10.5).strong().color(color)).fill(fill).stroke(Stroke::new(1.0_f32, RUST)).corner_radius(CornerRadius::same(3))).clicked() { studio.tab = tab; }
            }
        });
        ui.separator();
        if studio.tab != Tab::Guild { crate::guild_chamber::header(ui, &snap, design.as_ref(), &studio.project_id); }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match studio.tab {
                Tab::Guild => guild_tab(ui, studio, &snap, founding, &harnesses, &mut commands),
                Tab::Concept => crate::guild_chamber::concept_tab(ui, &mut studio.draft, &mut studio.dirty),
                Tab::Experience => crate::guild_chamber::experience_tab(ui, &mut studio.draft, &mut studio.dirty),
                Tab::Loops => crate::guild_chamber::loops_tab(ui, &mut studio.draft, &mut studio.dirty),
                Tab::Systems => crate::guild_chamber::systems_tab(ui, &mut studio.draft, &mut studio.chamber, &snap, &studio.project_id, &mut studio.dirty, &mut commands),
                Tab::Parts => crate::guild_chamber::parts_tab(ui, &mut studio.draft, &mut studio.dirty),
                Tab::Questions => crate::guild_chamber::questions_tab(ui, &mut studio.chamber, &snap, &studio.project_id, &studio.draft.systems, &mut commands),
                Tab::Decisions => crate::guild_chamber::decisions_tab(ui, &snap, &studio.project_id),
                Tab::Play => crate::guild_chamber::play_tab(ui, &mut studio.chamber, &snap, &studio.project_id, &results, &mut commands, &mut requests),
                Tab::Laboratory => crate::guild_chamber::laboratory_tab(ui, &mut studio.chamber, &snap, &studio.project_id, &mut commands, &mut requests),
            }
        });
        // Footer for the design tabs.
        if !matches!(studio.tab, Tab::Guild | Tab::Questions | Tab::Decisions | Tab::Play | Tab::Laboratory) && !studio.project_id.is_empty() {
            ui.separator();
            ui.horizontal(|ui| {
                if primary(ui, &tr("Guardar diseño")) {
                    commands.push(("design.set", json!({"projectId": studio.project_id, "design": studio.draft.to_design(&studio.project_id)})));
                    studio.dirty = false;
                    studio.loaded = None;
                }
                if small(ui, &tr("Plantilla videojuego")) {
                    commands.push(("design.template", json!({"projectId": studio.project_id, "template": "videogame"})));
                    studio.dirty = false;
                    studio.loaded = None;
                }
                if design.is_some() && small(ui, &tr("Exportar al repo")) { commands.push(("design.export", json!({"projectId": studio.project_id}))); }
                ui.label(mono(&tr("game-design/ en la rama guild/design del repositorio, en Markdown enlazado; los agentes reciben el diseño actual al final de cada prompt"), 9.5).color(FAINT));
            });
        }
    });
    for (method, params) in commands { view.command(method, params); }
    for (method, params, tag) in requests { view.request(method, params, tag); }
    action
}

/// Widgets readable on parchment: cream fields with dark ink, light
/// combo boxes, a brown selection. egui's dark defaults vanish here.
pub fn parchment(ui: &mut Ui) {
    let cream = Color32::from_rgb(250, 242, 224);
    let v = &mut ui.style_mut().visuals;
    v.override_text_color = None;
    v.extreme_bg_color = cream;
    v.faint_bg_color = PAPER_DARK;
    v.window_fill = PAPER;
    v.panel_fill = PAPER;
    v.selection.bg_fill = Color32::from_rgb(140, 80, 30);
    v.selection.stroke = Stroke::new(1.0_f32, cream);
    v.hyperlink_color = Color32::from_rgb(140, 80, 30);
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, PAPER, RUST),
        (&mut v.widgets.inactive, cream, RUST),
        (&mut v.widgets.hovered, Color32::from_rgb(255, 250, 238), Color32::from_rgb(140, 80, 30)),
        (&mut v.widgets.active, Color32::from_rgb(232, 208, 160), Color32::from_rgb(140, 80, 30)),
        (&mut v.widgets.open, cream, Color32::from_rgb(140, 80, 30)),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::new(1.0_f32, stroke);
        w.fg_stroke = Stroke::new(1.0_f32, INK);
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, FAINT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, RUST);
}

fn paper() -> Frame {
    Frame::new().fill(PAPER).stroke(Stroke::new(2.0_f32, Color32::from_rgb(115, 74, 36))).corner_radius(CornerRadius::same(4)).inner_margin(Margin::same(10))
        .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 128) })
}

/// The loop drawn as a cycle of steps, wrapped by hand: a framed label
/// does not wrap on its own, and the row width is what we have here.
pub(crate) fn cycle(ui: &mut Ui, steps: &[String]) {
    if steps.is_empty() { return; }
    let row = ui.available_width();
    let font = egui::FontId::monospace(crate::ui::fs(10.0));
    let arrow = 18.0;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
        let mut x = 0.0;
        for (i, s) in steps.iter().enumerate() {
            let w = ui.fonts(|f| f.layout_no_wrap(s.clone(), font.clone(), INK).size().x) + 14.0;
            if x > 0.0 && x + w + arrow > row { ui.end_row(); x = 0.0; }
            Frame::new().fill(Color32::from_rgb(250, 240, 220)).stroke(Stroke::new(1.0_f32, RUST)).corner_radius(CornerRadius::same(3)).inner_margin(Margin::symmetric(5, 2)).show(ui, |ui| {
                ui.add(egui::Label::new(mono(s, 10.0)).wrap_mode(egui::TextWrapMode::Extend));
            });
            ui.label(mono(if i + 1 < steps.len() { "→" } else { "↺" }, 11.0).color(Color32::from_rgb(140, 80, 30)));
            x += w + arrow + 6.0;
        }
    });
}

fn guild_tab(ui: &mut Ui, studio: &mut Studio, snap: &Snapshot, founding: bool, harnesses: &[String], commands: &mut Vec<(&'static str, Value)>) {
    if founding {
        ui.label(mono(&tr("Fundar una guild: un repositorio, sus miembros con un rol cada uno, y un diseño de partida."), 10.5).color(FAINT));
        let f = &mut studio.found;
        field(ui, &tr("Repositorio"), &mut f.repo_path, 380.0);
        field(ui, &tr("Nombre"), &mut f.name, 200.0);
        ui.horizontal(|ui| {
            ui.label(mono(&tr("Plantilla"), 10.5).color(FAINT));
            if f.template.is_empty() { f.template = "videogame".into(); }
            for (id, label) in [("videogame", tr("videojuego")), ("blank", tr("en blanco")), ("none", tr("ninguna"))] {
                ui.selectable_value(&mut f.template, id.to_string(), mono(&label, 10.5));
            }
            if small(ui, &tr("Roles de videojuego")) { f.members = videogame_members(harnesses); }
        });
        ui.label(mono(&tr("Miembros"), 10.5).strong());
        member_rows(ui, &mut f.members, &snap.roles, harnesses);
        if small(ui, &tr("+ miembro")) { f.members.push(MemberRow { harness: harnesses.first().cloned().unwrap_or_else(|| "kiro".into()), role: "implementer".into(), ..Default::default() }); }
        let mut founded = false;
        let mut cancelled = false;
        ui.horizontal(|ui| {
            let ready = !f.repo_path.trim().is_empty();
            if ready && primary(ui, &tr("Fundar la guild")) {
                let members: Vec<Value> = f.members.iter().filter(|m| !m.name.trim().is_empty()).map(|m| json!({"name": m.name.trim(), "role": m.role, "harness": m.harness, "model": if m.model.trim().is_empty() { Value::Null } else { json!(m.model.trim()) }})).collect();
                let mut params = json!({"repoPath": f.repo_path.trim(), "template": f.template, "members": members});
                if !f.name.trim().is_empty() { params["name"] = json!(f.name.trim()); }
                commands.push(("guild.found", params));
                founded = true;
            }
            if !snap.projects.is_empty() && small(ui, &tr("Cancelar")) { cancelled = true; }
        });
        if founded {
            studio.found = FoundForm::default();
            studio.founding = false;
            studio.loaded = None;
            studio.dirty = false;
        }
        if cancelled { studio.founding = false; }
        return;
    }
    let project = snap.projects.iter().find(|p| p.id == studio.project_id);
    if let Some(p) = project { ui.label(mono(&p.repo_path, 10.0).color(FAINT)); }
    ui.label(mono(&tr("Miembros"), 10.5).strong());
    let mut remove: Option<String> = None;
    for m in &snap.members {
        let edit = studio.member_edits.entry(m.id.clone()).or_insert_with(|| MemberRow { name: m.name.clone(), role: m.roles.first().cloned().unwrap_or_default(), harness: m.harness.to_string(), model: m.model_id.clone().unwrap_or_default() });
        let changed = member_row(ui, edit, &snap.roles, harnesses, Some(&m.id));
        ui.horizontal_wrapped(|ui| {
            ui.add_space(12.0);
            let differs = edit.name != m.name || Some(&edit.role) != m.roles.first() || edit.harness != m.harness.to_string() || edit.model != m.model_id.clone().unwrap_or_default();
            if (changed || differs) && small(ui, &tr("Aplicar")) {
                commands.push(("member.update", json!({"memberId": m.id, "name": edit.name.trim(), "roles": [edit.role], "harness": edit.harness, "model": if edit.model.trim().is_empty() { Value::Null } else { json!(edit.model.trim()) }})));
            }
            if small(ui, &tr("Quitar")) { remove = Some(m.id.clone()); }
        });
        if let Some(a) = snap.attempts.iter().filter(|a| a.member_id == m.id && a.state.is_active()).next_back() {
            ui.add(egui::Label::new(mono(&trf("ocupado: {}", &[&snap.tasks.iter().find(|t| t.id == a.task_id).map(|t| t.title.clone()).unwrap_or_default()]), 9.5).color(FAINT)).truncate());
        }
    }
    if let Some(id) = remove { commands.push(("member.remove", json!({"memberId": id}))); studio.member_edits.remove(&id); }
    ui.horizontal(|ui| {
        if small(ui, &tr("+ miembro")) {
            studio.found.members = vec![MemberRow { harness: harnesses.first().cloned().unwrap_or_else(|| "kiro".into()), role: "implementer".into(), ..Default::default() }];
        }
        if small(ui, &tr("Roles de videojuego")) { studio.found.members = videogame_members(harnesses); }
        if small(ui, &tr("Fundar otra guild")) { studio.founding = true; }
    });
    if !studio.found.members.is_empty() {
        ui.label(mono(&tr("Nuevos miembros"), 10.5).color(FAINT));
        member_rows(ui, &mut studio.found.members, &snap.roles, harnesses);
        ui.horizontal(|ui| {
            if primary(ui, &tr("Añadir")) {
                for m in studio.found.members.drain(..).filter(|m| !m.name.trim().is_empty()) {
                    let id: String = m.name.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
                    commands.push(("member.add", json!({"id": id, "name": m.name.trim(), "harness": m.harness, "roles": [m.role], "model": if m.model.trim().is_empty() { Value::Null } else { json!(m.model.trim()) }})));
                }
            }
            if small(ui, &tr("Cancelar")) { studio.found.members.clear(); }
        });
    }
    ui.add_space(6.0);
    ui.label(mono(&tr("Roles del catálogo"), 10.5).strong());
    for guild_name in ["coordination", "design", "engineering", "quality", "art", "play"] {
        ui.horizontal_wrapped(|ui| {
            ui.label(mono(&tr(guild_name), 10.0).color(FAINT));
            for r in snap.roles.iter().filter(|r| r.guild == guild_name) {
                ui.label(mono(&tr(&r.name), 10.0)).on_hover_text(r.prompt.clone());
            }
        });
    }
}

fn member_rows(ui: &mut Ui, rows: &mut Vec<MemberRow>, roles: &[Role], harnesses: &[String]) {
    let mut remove: Option<usize> = None;
    for (i, m) in rows.iter_mut().enumerate() {
        ui.horizontal_wrapped(|ui| {
            member_row(ui, m, roles, harnesses, None);
            if small(ui, "−") { remove = Some(i); }
        });
    }
    if let Some(i) = remove { rows.remove(i); }
}

/// One member line: name, role, harness, model, wrapped by hand so a
/// narrow dungeon gets two or three short rows instead of a cut one.
fn member_row(ui: &mut Ui, m: &mut MemberRow, roles: &[Role], harnesses: &[String], id: Option<&str>) -> bool {
    let salt = id.map(str::to_string).unwrap_or_else(|| format!("{:p}", m));
    let mut changed = false;
    let row = ui.available_width();
    ui.horizontal_wrapped(|ui| {
        let mut x = 0.0_f32;
        let mut fit = |ui: &mut Ui, w: f32| { if x > 0.0 && x + w > row { ui.end_row(); x = 0.0; } x += w + 6.0; };
        fit(ui, 90.0);
        changed |= ui.add(egui::TextEdit::singleline(&mut m.name).hint_text(tr("nombre")).desired_width(90.0).font(egui::FontId::monospace(crate::ui::fs(10.5)))).changed();
        fit(ui, 150.0);
        egui::ComboBox::from_id_salt(("role", &salt)).selected_text(mono(&role_name(roles, &m.role), 10.0)).width(150.0).show_ui(ui, |ui| {
            for r in roles { if ui.selectable_value(&mut m.role, r.id.clone(), mono(&tr(&r.name), 10.0)).on_hover_text(r.prompt.clone()).changed() { changed = true; } }
        });
        fit(ui, 70.0);
        egui::ComboBox::from_id_salt(("harness", &salt)).selected_text(mono(&m.harness, 10.0)).width(70.0).show_ui(ui, |ui| {
            for h in harnesses { if ui.selectable_value(&mut m.harness, h.clone(), mono(h, 10.0)).changed() { changed = true; } }
        });
        fit(ui, 130.0);
        changed |= ui.add(egui::TextEdit::singleline(&mut m.model).hint_text(tr("modelo")).desired_width(130.0).font(egui::FontId::monospace(crate::ui::fs(10.5)))).changed();
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every piece of text a frame of the guild window draws, on a tab of the demo.
    fn texts_on(tab: Tab) -> String {
        let ctx = egui::Context::default();
        let mut view = GuildView::new(std::sync::Arc::new(|| {}));
        view.set_demo(true);
        view.poll();
        view.studio.tab = tab;
        view.studio.max_height = Some(3900.0);
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(760.0, 4000.0))), ..Default::default() };
        let mut out = String::new();
        for _ in 0..2 {
            let output = ctx.run(input.clone(), |ctx| { studio(ctx, &mut view); });
            out.clear();
            for clipped in output.shapes {
                if let egui::Shape::Text(t) = clipped.shape { out.push_str(&t.galley.job.text); out.push('\n'); }
            }
        }
        out
    }

    #[test]
    fn the_laboratory_shows_the_decision_to_take_and_play_shows_the_session() {
        let lab = texts_on(Tab::Laboratory);
        for needle in ["decisión requerida", "Problema:", "Analista:", "★ semantic 1.6", "flat length", "▶ Jugar", "Adoptar", "Aceptar", "Iterar", "Rechazar", "2 humanos"] {
            assert!(lab.contains(needle), "laboratory lacks {needle:?}:\n{lab}");
        }
        let play = texts_on(Tab::Play);
        for needle in ["▶ Jugar la build actual", "En juego", "Gran momento", "Terminar sesión", "Observaciones (3)", "→ Q-001", "→ Pregunta", "Observado vs deseado"] {
            assert!(play.contains(needle), "play lacks {needle:?}:\n{play}");
        }
    }

    #[test]
    fn the_draft_round_trips_and_drops_empty_rows() {
        let d = guild::design::template("p", "Verbrawl", "videogame");
        let mut draft = Draft::from_design(&d);
        draft.loops.push(crate::guild_chamber::LoopDraft::default());
        draft.parts.push(crate::guild_chamber::PartDraft { id: "x".into(), ..Default::default() });
        let back = draft.to_design("p");
        assert_eq!(back.experience, d.experience);
        assert_eq!(back.loops, d.loops);
        assert_eq!(back.parts, d.parts);
        let members = videogame_members(&["kiro".into(), "claude".into()]);
        assert_eq!(back.systems.len(), d.systems.len());
        assert_eq!(members.len(), 6);
        assert!(members.iter().all(|m| m.harness == "kiro" || m.harness == "claude"), "codex not available falls back to the first harness");
    }
}
