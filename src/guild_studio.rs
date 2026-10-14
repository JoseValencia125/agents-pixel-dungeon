//! The guild window: found a guild for a repository (its members with
//! their roles, from the catalogue, and a design template), then shape
//! the game design in Sellers' three levels, the experience, the loops
//! and the parts, which every attempt's prompt ends with. Drafts live
//! here until **Save** sends the whole design to the service.

use crate::guild_view::GuildView;
use crate::l10n::{tr, trf};
use egui::{vec2, Color32, Context, CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use guild::config::Role;
use guild::design::{Experience, GameDesign, GameLoop, Part};
use guild::domain::*;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab { #[default] Guild, Experience, Loops, Parts }

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

#[derive(Clone, Default)]
pub struct LoopDraft { pub id: String, pub name: String, pub steps: String, pub inputs: String, pub outputs: String, pub feedback: String, pub failure: String }

#[derive(Clone, Default)]
pub struct PartDraft { pub id: String, pub name: String, pub attributes: String, pub rules: String }

#[derive(Clone, Default)]
pub struct Draft {
    pub title: String,
    pub fantasy: String,
    pub feel: String,
    pub behaviors: String,
    pub avoid: String,
    pub loops: Vec<LoopDraft>,
    pub parts: Vec<PartDraft>,
}

fn lines(s: &str) -> Vec<String> { s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect() }

impl Draft {
    pub fn from_design(d: &GameDesign) -> Draft {
        Draft {
            title: d.title.clone(),
            fantasy: d.experience.fantasy.clone(),
            feel: d.experience.player_should_feel.join("\n"),
            behaviors: d.experience.desired_behaviors.join("\n"),
            avoid: d.experience.avoid.join("\n"),
            loops: d.loops.iter().map(|l| LoopDraft { id: l.id.clone(), name: l.name.clone(), steps: l.steps.join("\n"), inputs: l.inputs.join("\n"), outputs: l.outputs.join("\n"), feedback: l.feedback.clone(), failure: l.failure.clone() }).collect(),
            parts: d.parts.iter().map(|p| PartDraft { id: p.id.clone(), name: p.name.clone(), attributes: p.attributes.join("\n"), rules: p.rules.join("\n") }).collect(),
        }
    }

    pub fn to_design(&self, project_id: &str) -> GameDesign {
        GameDesign {
            project_id: project_id.into(),
            title: self.title.trim().into(),
            experience: Experience { fantasy: self.fantasy.trim().into(), player_should_feel: lines(&self.feel), desired_behaviors: lines(&self.behaviors), avoid: lines(&self.avoid) },
            loops: self.loops.iter().filter(|l| !l.name.trim().is_empty() || !l.steps.trim().is_empty()).map(|l| GameLoop { id: l.id.clone(), name: l.name.trim().into(), steps: lines(&l.steps), inputs: lines(&l.inputs), outputs: lines(&l.outputs), feedback: l.feedback.trim().into(), failure: l.failure.trim().into(), notes: String::new() }).collect(),
            parts: self.parts.iter().filter(|p| !p.name.trim().is_empty()).map(|p| Part { id: p.id.clone(), name: p.name.trim().into(), attributes: lines(&p.attributes), rules: lines(&p.rules), notes: String::new() }).collect(),
            version: 0,
            updated_at: String::new(),
        }
    }
}

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
}

#[derive(Default)]
pub struct StudioAction { pub close: bool }

const PAPER: Color32 = Color32::from_rgb(237, 217, 168);
const PAPER_DARK: Color32 = Color32::from_rgb(229, 206, 152);
const INK: Color32 = Color32::from_rgb(61, 38, 20);
const FAINT: Color32 = Color32::from_rgb(120, 90, 60);
const RUST: Color32 = Color32::from_rgb(160, 120, 70);

fn mono(text: &str, size: f32) -> RichText { RichText::new(text).font(egui::FontId::monospace(crate::ui::fs(size))).color(INK) }

fn small(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(mono(text, 10.0).strong()).fill(Color32::from_rgb(222, 196, 140)).stroke(Stroke::new(1.0_f32, RUST)).corner_radius(CornerRadius::same(3))).clicked()
}

fn primary(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(mono(text, 10.5).strong().color(Color32::from_rgb(250, 240, 220))).fill(Color32::from_rgb(140, 80, 30)).stroke(Stroke::NONE).corner_radius(CornerRadius::same(3))).clicked()
}

fn field(ui: &mut Ui, label: &str, text: &mut String, width: f32) -> bool {
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(label, 10.5).color(FAINT));
        let width = width.min((ui.available_width() - 4.0).max(80.0));
        ui.add(egui::TextEdit::singleline(text).desired_width(width).font(egui::FontId::monospace(crate::ui::fs(10.5)))).changed()
    }).inner
}

fn area(ui: &mut Ui, label: &str, text: &mut String, rows: usize) -> bool {
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
    let Some(snap) = view.snapshot.clone() else {
        egui::Window::new(mono(&tr("Gremio"), 12.0).strong()).id(egui::Id::new("guild-studio")).collapsible(false).resizable(false).frame(paper()).show(ctx, |ui| {
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
    let height = (screen.height() - 64.0).clamp(200.0, 600.0);
    egui::Window::new(mono(&tr("Gremio"), 12.0).strong()).id(egui::Id::new("guild-studio")).collapsible(false).resizable(false).fixed_size([width, height]).constrain_to(screen).frame(paper()).show(ctx, |ui| {
        ui.set_width(width - 20.0);
        ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
        // Header: project, tabs, close.
        ui.horizontal_wrapped(|ui| {
            if !snap.projects.is_empty() {
                let shown = snap.projects.iter().find(|p| p.id == studio.project_id).map(|p| p.name.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("studio-project").selected_text(mono(&shown, 10.5)).width(140.0).show_ui(ui, |ui| {
                    for p in &snap.projects {
                        if ui.selectable_value(&mut studio.project_id, p.id.clone(), mono(&p.name, 10.5)).changed() { studio.dirty = false; studio.loaded = None; }
                    }
                });
            }
            for (tab, label) in [(Tab::Guild, tr("Gremio")), (Tab::Experience, tr("Experiencia")), (Tab::Loops, tr("Loops")), (Tab::Parts, tr("Partes"))] {
                let on = studio.tab == tab;
                let fill = if on { Color32::from_rgb(140, 80, 30) } else { Color32::from_rgb(222, 196, 140) };
                let color = if on { Color32::from_rgb(250, 240, 220) } else { INK };
                if ui.add(egui::Button::new(mono(&label, 10.5).strong().color(color)).fill(fill).stroke(Stroke::new(1.0_f32, RUST)).corner_radius(CornerRadius::same(3))).clicked() { studio.tab = tab; }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(mono("x", 11.0).strong()).frame(false)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
                if studio.dirty { ui.label(mono(&tr("sin guardar"), 10.0).color(Color32::from_rgb(190, 40, 30))); }
                else if let Some(d) = &design { ui.label(mono(&format!("v{}", d.version), 10.0).color(FAINT)); }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match studio.tab {
                Tab::Guild => guild_tab(ui, studio, &snap, founding, &harnesses, &mut commands),
                Tab::Experience => {
                    ui.label(mono(&tr("El todo: la experiencia que el jugador debe tener. Cada decisión de diseño se argumenta desde aquí."), 10.5).color(FAINT));
                    studio.dirty |= field(ui, &tr("Título"), &mut studio.draft.title, 300.0);
                    studio.dirty |= area(ui, &tr("Fantasía: quién es el jugador, en una frase"), &mut studio.draft.fantasy, 2);
                    studio.dirty |= area(ui, &tr("El jugador debe sentir (una por línea)"), &mut studio.draft.feel, 3);
                    studio.dirty |= area(ui, &tr("Conductas deseadas (una por línea)"), &mut studio.draft.behaviors, 3);
                    studio.dirty |= area(ui, &tr("Evitar (una por línea)"), &mut studio.draft.avoid, 3);
                }
                Tab::Loops => {
                    ui.label(mono(&tr("Los sistemas: ciclos que producen la experiencia. El último paso alimenta al primero."), 10.5).color(FAINT));
                    let mut remove: Option<usize> = None;
                    for (i, l) in studio.draft.loops.iter_mut().enumerate() {
                        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                studio.dirty |= field(ui, &tr("Loop"), &mut l.name, 180.0);
                                if small(ui, &tr("Quitar")) { remove = Some(i); }
                            });
                            cycle(ui, &lines(&l.steps));
                            if ui.available_width() > 420.0 {
                                ui.columns(2, |cols| {
                                    studio.dirty |= area(&mut cols[0], &tr("Pasos del ciclo (uno por línea)"), &mut l.steps, 4);
                                    studio.dirty |= area(&mut cols[1], &tr("Entradas (una por línea)"), &mut l.inputs, 2);
                                    studio.dirty |= area(&mut cols[1], &tr("Salidas (una por línea)"), &mut l.outputs, 2);
                                });
                            } else {
                                studio.dirty |= area(ui, &tr("Pasos del ciclo (uno por línea)"), &mut l.steps, 4);
                                studio.dirty |= area(ui, &tr("Entradas (una por línea)"), &mut l.inputs, 2);
                                studio.dirty |= area(ui, &tr("Salidas (una por línea)"), &mut l.outputs, 2);
                            }
                            studio.dirty |= field(ui, &tr("Retroalimentación"), &mut l.feedback, f32::INFINITY);
                            studio.dirty |= field(ui, &tr("Falla cuando"), &mut l.failure, f32::INFINITY);
                        });
                    }
                    if let Some(i) = remove { studio.draft.loops.remove(i); studio.dirty = true; }
                    if small(ui, &tr("+ loop")) { studio.draft.loops.push(LoopDraft { id: format!("loop-{}", studio.draft.loops.len() + 1), ..Default::default() }); studio.dirty = true; }
                }
                Tab::Parts => {
                    ui.label(mono(&tr("Las partes: entidades y reglas concretas. Un atributo por línea, como nombre: tipo o rango."), 10.5).color(FAINT));
                    let mut remove: Option<usize> = None;
                    for (i, p) in studio.draft.parts.iter_mut().enumerate() {
                        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                studio.dirty |= field(ui, &tr("Parte"), &mut p.name, 180.0);
                                if small(ui, &tr("Quitar")) { remove = Some(i); }
                            });
                            if ui.available_width() > 420.0 {
                                ui.columns(2, |cols| {
                                    studio.dirty |= area(&mut cols[0], &tr("Atributos (uno por línea)"), &mut p.attributes, 4);
                                    studio.dirty |= area(&mut cols[1], &tr("Reglas (una por línea)"), &mut p.rules, 4);
                                });
                            } else {
                                studio.dirty |= area(ui, &tr("Atributos (uno por línea)"), &mut p.attributes, 4);
                                studio.dirty |= area(ui, &tr("Reglas (una por línea)"), &mut p.rules, 4);
                            }
                        });
                    }
                    if let Some(i) = remove { studio.draft.parts.remove(i); studio.dirty = true; }
                    if small(ui, &tr("+ parte")) { studio.draft.parts.push(PartDraft { id: format!("part-{}", studio.draft.parts.len() + 1), ..Default::default() }); studio.dirty = true; }
                }
            }
        });
        // Footer for the design tabs.
        if studio.tab != Tab::Guild && !studio.project_id.is_empty() {
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
                ui.label(mono(&tr("game-design/ en el repositorio, en TOML; los agentes lo reciben al final de cada prompt"), 9.5).color(FAINT));
            });
        }
    });
    for (method, params) in commands { view.command(method, params); }
    action
}

fn paper() -> Frame {
    Frame::new().fill(PAPER).stroke(Stroke::new(2.0_f32, Color32::from_rgb(115, 74, 36))).corner_radius(CornerRadius::same(4)).inner_margin(Margin::same(10))
        .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 128) })
}

/// The loop drawn as a cycle of steps, wrapped by hand: a framed label
/// does not wrap on its own, and the row width is what we have here.
fn cycle(ui: &mut Ui, steps: &[String]) {
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

    #[test]
    fn the_draft_round_trips_and_drops_empty_rows() {
        let d = guild::design::template("p", "Verbrawl", "videogame");
        let mut draft = Draft::from_design(&d);
        draft.loops.push(LoopDraft::default());
        draft.parts.push(PartDraft { id: "x".into(), ..Default::default() });
        let back = draft.to_design("p");
        assert_eq!(back.experience, d.experience);
        assert_eq!(back.loops, d.loops);
        assert_eq!(back.parts, d.parts);
        let members = videogame_members(&["kiro".into(), "claude".into()]);
        assert_eq!(members.len(), 6);
        assert!(members.iter().all(|m| m.harness == "kiro" || m.harness == "claude"), "codex not available falls back to the first harness");
    }
}
