//! The design chamber: the living game design as the human edits it,
//! broadest first (the concept, the player experience, the loops), then
//! the systems with their maturity, the objects, and, apart from the
//! current design, the questions it has not answered and the decisions
//! that answered the others. Drafts live here until **Save** sends the
//! whole design to the service; questions and decisions go at once.

use crate::guild_studio::{area, cycle, field, mono, primary, small, FAINT, INK, PAPER_DARK, RUST};
use crate::l10n::{tr, trf};
use egui::{vec2, Color32, CornerRadius, Frame, Margin, Stroke, Ui};
use guild::design::{Concept, DesignQuestion, Experience, GameLoop, GameSystem, LivingGameDesign, Maturity, Part, Style};
use guild::domain::Snapshot;
use serde_json::{json, Value};

#[derive(Clone, Default)]
pub struct LoopDraft { pub id: String, pub name: String, pub steps: String, pub inputs: String, pub outputs: String, pub feedback: String, pub failure: String }

#[derive(Clone, Default)]
pub struct PartDraft { pub id: String, pub name: String, pub attributes: String, pub rules: String }

#[derive(Clone, Default)]
pub struct SystemDraft {
    pub id: String,
    pub name: String,
    pub maturity: Maturity,
    pub intent: String,
    pub known: String,
    pub unknown: String,
    pub current: String,
    pub player_facing: bool,
    pub implementation: String,
    pub loops: String,
    pub parts: String,
    pub questions: Vec<String>,
    pub decisions: Vec<String>,
    pub experiments: Vec<String>,
    pub assets: Vec<String>,
}

#[derive(Clone, Default)]
pub struct Draft {
    pub title: String,
    pub status: String,
    pub statement: String,
    pub genre: String,
    pub audience: String,
    pub usps: String,
    pub world_fiction: String,
    pub monetization: String,
    pub platform_scope: String,
    pub competition: String,
    pub fantasy: String,
    pub pov: String,
    pub feel: String,
    pub behaviors: String,
    pub avoid: String,
    pub key_moments: String,
    pub engagement: String,
    pub visual: String,
    pub audio: String,
    pub references: String,
    pub objectives: String,
    pub interactivity: String,
    pub loops: Vec<LoopDraft>,
    pub systems: Vec<SystemDraft>,
    pub parts: Vec<PartDraft>,
    pub concept_maturity: Maturity,
    pub experience_maturity: Maturity,
    pub loops_maturity: Maturity,
    pub sections: Vec<guild::design::DesignSection>,
}

pub fn lines(s: &str) -> Vec<String> { s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect() }

impl Draft {
    pub fn from_design(d: &LivingGameDesign) -> Draft {
        Draft {
            title: d.title.clone(), status: d.status.clone(),
            statement: d.concept.statement.clone(), genre: d.concept.genre.clone(), audience: d.concept.audience.clone(), usps: d.concept.usps.join("\n"),
            world_fiction: d.concept.world_fiction.clone(), monetization: d.concept.monetization.clone(), platform_scope: d.concept.platform_scope.clone(), competition: d.concept.competition.clone(),
            fantasy: d.experience.fantasy.clone(), pov: d.experience.pov.clone(), feel: d.experience.player_should_feel.join("\n"), behaviors: d.experience.desired_behaviors.join("\n"),
            avoid: d.experience.avoid.join("\n"), key_moments: d.experience.key_moments.join("\n"), engagement: d.experience.engagement.clone(),
            visual: d.style.visual.clone(), audio: d.style.audio.clone(), references: d.style.references.join("\n"),
            objectives: d.objectives.clone(), interactivity: d.interactivity.clone(),
            loops: d.loops.iter().map(|l| LoopDraft { id: l.id.clone(), name: l.name.clone(), steps: l.steps.join("\n"), inputs: l.inputs.join("\n"), outputs: l.outputs.join("\n"), feedback: l.feedback.clone(), failure: l.failure.clone() }).collect(),
            systems: d.systems.iter().map(|s| SystemDraft { id: s.id.clone(), name: s.name.clone(), maturity: s.maturity, intent: s.intent.clone(), known: s.known.join("\n"), unknown: s.unknown.join("\n"), current: s.current.clone(), player_facing: s.player_facing, implementation: s.implementation.join("\n"), loops: s.loops.join(", "), parts: s.parts.join(", "), questions: s.questions.clone(), decisions: s.decisions.clone(), experiments: s.experiments.clone(), assets: s.assets.clone() }).collect(),
            parts: d.parts.iter().map(|p| PartDraft { id: p.id.clone(), name: p.name.clone(), attributes: p.attributes.join("\n"), rules: p.rules.join("\n") }).collect(),
            concept_maturity: d.concept_maturity, experience_maturity: d.experience_maturity, loops_maturity: d.loops_maturity,
            sections: d.sections.clone(),
        }
    }

    pub fn to_design(&self, project_id: &str) -> LivingGameDesign {
        let csv = |s: &str| s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect::<Vec<_>>();
        LivingGameDesign {
            project_id: project_id.into(),
            title: self.title.trim().into(),
            status: self.status.trim().into(),
            concept: Concept { statement: self.statement.trim().into(), genre: self.genre.trim().into(), audience: self.audience.trim().into(), usps: lines(&self.usps), world_fiction: self.world_fiction.trim().into(), monetization: self.monetization.trim().into(), platform_scope: self.platform_scope.trim().into(), competition: self.competition.trim().into() },
            experience: Experience { fantasy: self.fantasy.trim().into(), pov: self.pov.trim().into(), player_should_feel: lines(&self.feel), desired_behaviors: lines(&self.behaviors), avoid: lines(&self.avoid), key_moments: lines(&self.key_moments), engagement: self.engagement.trim().into() },
            style: Style { visual: self.visual.trim().into(), audio: self.audio.trim().into(), references: lines(&self.references) },
            loops: self.loops.iter().filter(|l| !l.name.trim().is_empty() || !l.steps.trim().is_empty()).map(|l| GameLoop { id: l.id.clone(), name: l.name.trim().into(), steps: lines(&l.steps), inputs: lines(&l.inputs), outputs: lines(&l.outputs), feedback: l.feedback.trim().into(), failure: l.failure.trim().into(), notes: String::new() }).collect(),
            objectives: self.objectives.trim().into(),
            systems: self.systems.iter().filter(|s| !s.name.trim().is_empty()).map(|s| GameSystem { id: if s.id.trim().is_empty() { slug(&s.name) } else { s.id.clone() }, name: s.name.trim().into(), maturity: s.maturity, intent: s.intent.trim().into(), known: lines(&s.known), unknown: lines(&s.unknown), current: s.current.trim().into(), player_facing: s.player_facing, loops: csv(&s.loops), parts: csv(&s.parts), implementation: lines(&s.implementation), assets: s.assets.clone(), questions: s.questions.clone(), decisions: s.decisions.clone(), experiments: s.experiments.clone() }).collect(),
            interactivity: self.interactivity.trim().into(),
            parts: self.parts.iter().filter(|p| !p.name.trim().is_empty()).map(|p| Part { id: p.id.clone(), name: p.name.trim().into(), attributes: lines(&p.attributes), rules: lines(&p.rules), notes: String::new() }).collect(),
            sections: self.sections.clone(),
            concept_maturity: self.concept_maturity, experience_maturity: self.experience_maturity, loops_maturity: self.loops_maturity,
            version: 0,
            updated_at: String::new(),
        }
    }
}

fn slug(s: &str) -> String { s.trim().to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect() }

/// The chamber's own state: the forms for a question and its resolution.
#[derive(Default)]
pub struct ChamberState {
    pub question: String,
    pub question_context: String,
    pub question_system: String,
    /// The question being resolved and the decision's draft.
    pub resolving: Option<String>,
    pub decision: String,
    pub why: String,
    pub answer: String,
    pub evidence: String,
    pub rejected: String,
    /// Which systems are unfolded.
    pub open_systems: Vec<String>,
    pub show_resolved: bool,
}

pub fn maturity_label(m: Maturity) -> String {
    tr(match m { Maturity::Undefined => "sin definir", Maturity::Concept => "concepto", Maturity::Hypothesis => "hipótesis", Maturity::Prototyping => "prototipando", Maturity::Validated => "validado", Maturity::Production => "producción", Maturity::Revised => "en revisión" })
}

fn maturity_color(m: Maturity) -> Color32 {
    match m {
        Maturity::Undefined => Color32::from_rgb(150, 130, 110),
        Maturity::Concept => Color32::from_rgb(120, 90, 60),
        Maturity::Hypothesis => Color32::from_rgb(190, 120, 30),
        Maturity::Prototyping => Color32::from_rgb(200, 110, 0),
        Maturity::Validated => Color32::from_rgb(30, 140, 60),
        Maturity::Production => Color32::from_rgb(20, 100, 50),
        Maturity::Revised => Color32::from_rgb(120, 60, 160),
    }
}

/// A maturity picker; true when changed.
fn maturity_combo(ui: &mut Ui, id: &str, m: &mut Maturity) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(("maturity", id)).selected_text(mono(&maturity_label(*m), 10.0).color(maturity_color(*m))).width(120.0).show_ui(ui, |ui| {
        for option in Maturity::ALL { if ui.selectable_value(m, option, mono(&maturity_label(option), 10.0).color(maturity_color(option))).changed() { changed = true; } }
    });
    changed
}

/// The chamber's head: the maturity bar and what the design still owes.
pub fn header(ui: &mut Ui, snap: &Snapshot, design: Option<&LivingGameDesign>, project_id: &str) {
    let Some(d) = design else { return };
    let percent = guild::design::maturity_percent(d);
    ui.horizontal(|ui| {
        ui.label(mono(&tr("Madurez del diseño"), 10.5).color(FAINT));
        let (rect, _) = ui.allocate_exact_size(vec2(140.0, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(222, 196, 140));
        let mut fill = rect;
        fill.set_width(rect.width() * percent as f32 / 100.0);
        ui.painter().rect_filled(fill, 2.0, Color32::from_rgb(140, 80, 30));
        ui.label(mono(&format!("{percent}%"), 10.5));
    });
    let open = snap.questions.iter().filter(|q| q.project_id == project_id && q.status == "open").count();
    let hyps = snap.hypotheses.iter().filter(|h| h.project_id == project_id && matches!(h.status, guild::experiment::HypothesisStatus::Proposed | guild::experiment::HypothesisStatus::Experimenting)).count();
    let running = snap.experiments.iter().filter(|e| e.project_id == project_id && matches!(e.status, guild::experiment::ExperimentStatus::Building | guild::experiment::ExperimentStatus::Playtesting | guild::experiment::ExperimentStatus::Analysing)).count();
    let needs = snap.experiments.iter().filter(|e| e.project_id == project_id && e.status == guild::experiment::ExperimentStatus::Decided).count();
    let decisions = snap.design_decisions.iter().filter(|d| d.project_id == project_id).count();
    let playtests = snap.playtests.iter().filter(|p| p.project_id == project_id && p.status == "finished").count();
    ui.horizontal_wrapped(|ui| {
        let chip = |ui: &mut Ui, label: String, n: usize, hot: bool| {
            let color = if hot && n > 0 { Color32::from_rgb(190, 40, 30) } else { INK };
            ui.label(mono(&format!("{n} {label}"), 10.0).color(color));
            ui.label(mono("·", 10.0).color(FAINT));
        };
        chip(ui, tr("preguntas abiertas"), open, false);
        chip(ui, tr("hipótesis activas"), hyps, false);
        chip(ui, tr("experimentos en curso"), running, false);
        chip(ui, tr("esperan tu decisión"), needs, true);
        chip(ui, tr("playtests"), playtests, false);
        ui.label(mono(&format!("{decisions} {}", tr("decisiones")), 10.0));
    });
}

pub fn concept_tab(ui: &mut Ui, draft: &mut Draft, dirty: &mut bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&tr("El concepto: el juego en un tuit y por qué merece hacerse."), 10.5).color(FAINT));
        *dirty |= maturity_combo(ui, "concept", &mut draft.concept_maturity);
    });
    *dirty |= field(ui, &tr("Título de trabajo"), &mut draft.title, 300.0);
    *dirty |= area(ui, &tr("Estado actual (qué está hecho, en curso, bloqueado)"), &mut draft.status, 2);
    *dirty |= area(ui, &tr("Concepto en una o dos frases: qué es el juego y por qué es divertido"), &mut draft.statement, 2);
    *dirty |= field(ui, &tr("Género"), &mut draft.genre, f32::INFINITY);
    *dirty |= area(ui, &tr("Audiencia: motivaciones, edad, clasificación"), &mut draft.audience, 2);
    *dirty |= area(ui, &tr("Puntos de venta únicos (uno por línea): por qué destaca entre todos los demás"), &mut draft.usps, 3);
    *dirty |= area(ui, &tr("Ficción del mundo, como la ve el jugador"), &mut draft.world_fiction, 2);
    *dirty |= area(ui, &tr("Monetización"), &mut draft.monetization, 1);
    *dirty |= area(ui, &tr("Plataforma, tecnología y alcance: equipo, tiempo, riesgos"), &mut draft.platform_scope, 2);
    *dirty |= area(ui, &tr("Competencia: juegos parecidos y qué distingue a este"), &mut draft.competition, 2);
    *dirty |= area(ui, &tr("Objetivos y progresión: cómo avanza el jugador, metas a corto y largo plazo"), &mut draft.objectives, 3);
    *dirty |= area(ui, &tr("Interactividad: qué hace el jugador momento a momento"), &mut draft.interactivity, 3);
}

pub fn experience_tab(ui: &mut Ui, draft: &mut Draft, dirty: &mut bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&tr("El todo: la experiencia que el jugador debe tener. Cada decisión de diseño se argumenta desde aquí."), 10.5).color(FAINT));
        *dirty |= maturity_combo(ui, "experience", &mut draft.experience_maturity);
    });
    *dirty |= area(ui, &tr("Fantasía: quién es el jugador, en una frase"), &mut draft.fantasy, 2);
    *dirty |= area(ui, &tr("Punto de vista y escenario; cuánto dura una partida"), &mut draft.pov, 2);
    *dirty |= area(ui, &tr("El jugador debe sentir (una por línea)"), &mut draft.feel, 3);
    *dirty |= area(ui, &tr("Conductas deseadas (una por línea)"), &mut draft.behaviors, 3);
    *dirty |= area(ui, &tr("Evitar (una por línea)"), &mut draft.avoid, 3);
    *dirty |= area(ui, &tr("Momentos clave (uno por línea): los primeros minutos, las luchas, las victorias"), &mut draft.key_moments, 3);
    *dirty |= area(ui, &tr("Qué mantiene al jugador durante toda la partida"), &mut draft.engagement, 2);
    ui.add_space(4.0);
    ui.label(mono(&tr("Estilo visual y sonoro"), 10.5).strong());
    *dirty |= area(ui, &tr("Visual: el look and feel y cómo sostiene la experiencia"), &mut draft.visual, 2);
    *dirty |= area(ui, &tr("Audio"), &mut draft.audio, 1);
    *dirty |= area(ui, &tr("Referencias (una por línea); nunca se usan en el juego"), &mut draft.references, 2);
}

pub fn loops_tab(ui: &mut Ui, draft: &mut Draft, dirty: &mut bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&tr("Los sistemas: ciclos que producen la experiencia. El último paso alimenta al primero."), 10.5).color(FAINT));
        *dirty |= maturity_combo(ui, "loops", &mut draft.loops_maturity);
    });
    let mut remove: Option<usize> = None;
    for (i, l) in draft.loops.iter_mut().enumerate() {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                *dirty |= field(ui, &tr("Loop"), &mut l.name, 180.0);
                if small(ui, &tr("Quitar")) { remove = Some(i); }
            });
            cycle(ui, &lines(&l.steps));
            if ui.available_width() > 420.0 {
                ui.columns(2, |cols| {
                    *dirty |= area(&mut cols[0], &tr("Pasos del ciclo (uno por línea)"), &mut l.steps, 4);
                    *dirty |= area(&mut cols[1], &tr("Entradas (una por línea)"), &mut l.inputs, 2);
                    *dirty |= area(&mut cols[1], &tr("Salidas (una por línea)"), &mut l.outputs, 2);
                });
            } else {
                *dirty |= area(ui, &tr("Pasos del ciclo (uno por línea)"), &mut l.steps, 4);
                *dirty |= area(ui, &tr("Entradas (una por línea)"), &mut l.inputs, 2);
                *dirty |= area(ui, &tr("Salidas (una por línea)"), &mut l.outputs, 2);
            }
            *dirty |= field(ui, &tr("Retroalimentación"), &mut l.feedback, f32::INFINITY);
            *dirty |= field(ui, &tr("Falla cuando"), &mut l.failure, f32::INFINITY);
        });
    }
    if let Some(i) = remove { draft.loops.remove(i); *dirty = true; }
    if small(ui, &tr("+ loop")) { draft.loops.push(LoopDraft { id: format!("loop-{}", draft.loops.len() + 1), ..Default::default() }); *dirty = true; }
}

/// Systems, each with its maturity: folded to a line, unfolded to its
/// intent, what is known and unknown, the current design and its links.
pub fn systems_tab(ui: &mut Ui, draft: &mut Draft, state: &mut ChamberState, snap: &Snapshot, project_id: &str, dirty: &mut bool, commands: &mut Vec<(&'static str, Value)>) {
    ui.label(mono(&tr("Cada sistema con su madurez: la intención, lo sabido, lo que no (preguntas, no suposiciones) y el diseño actual en presente, con números donde los hay."), 10.5).color(FAINT));
    let mut remove: Option<usize> = None;
    let mut ask: Option<(String, String)> = None;
    for (i, s) in draft.systems.iter_mut().enumerate() {
        let id = if s.id.is_empty() { format!("new-{i}") } else { s.id.clone() };
        let open = state.open_systems.contains(&id);
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add(egui::Button::new(mono(if open { "▾" } else { "▸" }, 11.0)).frame(false)).clicked() {
                    if open { state.open_systems.retain(|x| x != &id); } else { state.open_systems.push(id.clone()); }
                }
                *dirty |= field(ui, "", &mut s.name, 150.0);
                *dirty |= maturity_combo(ui, &id, &mut s.maturity);
                let open_q = snap.questions.iter().filter(|q| q.project_id == project_id && q.status == "open" && q.system_id.as_deref() == Some(s.id.as_str())).count();
                if open_q > 0 { ui.label(mono(&trf("{} preguntas abiertas", &[&open_q]), 10.0).color(Color32::from_rgb(190, 40, 30))); }
                if !s.decisions.is_empty() { ui.label(mono(&trf("{} decisiones", &[&s.decisions.len()]), 10.0).color(FAINT)); }
            });
            if !open { return; }
            *dirty |= area(ui, &tr("Intención: el porqué del sistema, en términos de la experiencia"), &mut s.intent, 2);
            *dirty |= area(ui, &tr("Diseño actual (en presente, con números donde los haya)"), &mut s.current, 3);
            if ui.available_width() > 420.0 {
                ui.columns(2, |cols| {
                    *dirty |= area(&mut cols[0], &tr("Sabido (uno por línea)"), &mut s.known, 3);
                    *dirty |= area(&mut cols[1], &tr("Desconocido (uno por línea): cada uno debería ser una pregunta"), &mut s.unknown, 3);
                });
            } else {
                *dirty |= area(ui, &tr("Sabido (uno por línea)"), &mut s.known, 3);
                *dirty |= area(ui, &tr("Desconocido (uno por línea): cada uno debería ser una pregunta"), &mut s.unknown, 3);
            }
            ui.horizontal_wrapped(|ui| {
                *dirty |= ui.checkbox(&mut s.player_facing, mono(&tr("el jugador lo toca"), 10.0)).changed();
                *dirty |= field(ui, &tr("Loops"), &mut s.loops, 120.0);
                *dirty |= field(ui, &tr("Objetos"), &mut s.parts, 120.0);
            });
            *dirty |= area(ui, &tr("Implementación (rutas, una por línea)"), &mut s.implementation, 2);
            ui.horizontal_wrapped(|ui| {
                for q in &s.questions { ui.label(mono(q, 10.0).color(RUST)); }
                for d in &s.decisions { ui.label(mono(d, 10.0).color(Color32::from_rgb(30, 140, 60))); }
                for e in &s.experiments { ui.label(mono(e, 10.0).color(FAINT)); }
                for u in lines(&s.unknown) {
                    if !s.id.is_empty() && small(ui, &trf("Preguntar: {}", &[&u.chars().take(28).collect::<String>()])) { ask = Some((s.id.clone(), u.clone())); }
                }
                if small(ui, &tr("Quitar sistema")) { remove = Some(i); }
            });
        });
    }
    if let Some((sid, text)) = ask {
        commands.push(("question.create", json!({"projectId": project_id, "question": text, "systemId": sid, "raisedBy": "human"})));
    }
    if let Some(i) = remove { draft.systems.remove(i); *dirty = true; }
    if small(ui, &tr("+ sistema")) { draft.systems.push(SystemDraft { maturity: Maturity::Concept, ..Default::default() }); *dirty = true; }
}

pub fn parts_tab(ui: &mut Ui, draft: &mut Draft, dirty: &mut bool) {
    ui.label(mono(&tr("Las partes: entidades y reglas concretas. Un atributo por línea, como nombre: tipo o rango."), 10.5).color(FAINT));
    let mut remove: Option<usize> = None;
    for (i, p) in draft.parts.iter_mut().enumerate() {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                *dirty |= field(ui, &tr("Parte"), &mut p.name, 180.0);
                if small(ui, &tr("Quitar")) { remove = Some(i); }
            });
            if ui.available_width() > 420.0 {
                ui.columns(2, |cols| {
                    *dirty |= area(&mut cols[0], &tr("Atributos (uno por línea)"), &mut p.attributes, 4);
                    *dirty |= area(&mut cols[1], &tr("Reglas (una por línea)"), &mut p.rules, 4);
                });
            } else {
                *dirty |= area(ui, &tr("Atributos (uno por línea)"), &mut p.attributes, 4);
                *dirty |= area(ui, &tr("Reglas (una por línea)"), &mut p.rules, 4);
            }
        });
    }
    if let Some(i) = remove { draft.parts.remove(i); *dirty = true; }
    if small(ui, &tr("+ parte")) { draft.parts.push(PartDraft { id: format!("part-{}", draft.parts.len() + 1), ..Default::default() }); *dirty = true; }
}

/// What the design does not know: open questions, each resolvable with
/// a decision; resolved ones stay, folded, pointing at theirs.
pub fn questions_tab(ui: &mut Ui, state: &mut ChamberState, snap: &Snapshot, project_id: &str, systems: &[SystemDraft], commands: &mut Vec<(&'static str, Value)>) {
    ui.label(mono(&tr("Lo que el diseño no sabe todavía. Una pregunta no desaparece: se resuelve con una decisión y queda apuntando a ella."), 10.5).color(FAINT));
    Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
        ui.label(mono(&tr("Nueva pregunta"), 10.5).strong());
        area(ui, &tr("Pregunta"), &mut state.question, 2);
        area(ui, &tr("Contexto: qué se observó"), &mut state.question_context, 2);
        ui.horizontal_wrapped(|ui| {
            ui.label(mono(&tr("Sistema"), 10.5).color(FAINT));
            egui::ComboBox::from_id_salt("question-system").selected_text(mono(if state.question_system.is_empty() { "—" } else { &state.question_system }, 10.0)).width(140.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut state.question_system, String::new(), mono("—", 10.0));
                for s in systems.iter().filter(|s| !s.id.is_empty()) { ui.selectable_value(&mut state.question_system, s.id.clone(), mono(&s.name, 10.0)); }
            });
            if !state.question.trim().is_empty() && primary(ui, &tr("Abrir pregunta")) {
                commands.push(("question.create", json!({"projectId": project_id, "question": state.question.trim(), "context": state.question_context.trim(), "systemId": if state.question_system.is_empty() { Value::Null } else { json!(state.question_system) }, "raisedBy": "human"})));
                state.question.clear();
                state.question_context.clear();
            }
        });
    });
    let questions: Vec<&DesignQuestion> = snap.questions.iter().filter(|q| q.project_id == project_id).collect();
    let open: Vec<&DesignQuestion> = questions.iter().copied().filter(|q| q.status == "open").collect();
    ui.label(mono(&trf("Abiertas ({})", &[&open.len()]), 10.5).strong());
    for q in open {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&q.id, 10.5).strong().color(RUST));
                ui.label(mono(&q.question, 10.5));
                if let Some(s) = &q.system_id { ui.label(mono(s, 10.0).color(FAINT)); }
                if state.resolving.as_deref() != Some(q.id.as_str()) && small(ui, &tr("Resolver")) { state.resolving = Some(q.id.clone()); state.decision.clear(); state.why.clear(); state.answer.clear(); state.evidence.clear(); state.rejected.clear(); }
            });
            if !q.context.trim().is_empty() { ui.label(mono(&q.context, 10.0).color(FAINT)); }
            ui.horizontal_wrapped(|ui| {
                for h in &q.hypothesis_ids { ui.label(mono(&trf("hipótesis {}", &[&h.chars().rev().take(8).collect::<String>().chars().rev().collect::<String>()]), 9.5).color(FAINT)); }
                for e in &q.experiment_ids { ui.label(mono(&trf("experimento {}", &[&e.chars().rev().take(8).collect::<String>().chars().rev().collect::<String>()]), 9.5).color(FAINT)); }
            });
            if state.resolving.as_deref() == Some(q.id.as_str()) {
                ui.label(mono(&tr("Decisión que la responde"), 10.5).strong());
                area(ui, &tr("Decisión (en presente: cómo funciona el juego ahora)"), &mut state.decision, 2);
                area(ui, &tr("Por qué"), &mut state.why, 2);
                area(ui, &tr("Respuesta breve a la pregunta"), &mut state.answer, 1);
                area(ui, &tr("Evidencia (una por línea)"), &mut state.evidence, 2);
                area(ui, &tr("Alternativas rechazadas (una por línea)"), &mut state.rejected, 2);
                ui.horizontal(|ui| {
                    if !state.decision.trim().is_empty() && primary(ui, &tr("Decidir")) {
                        commands.push(("question.resolve", json!({"projectId": project_id, "questionId": q.id, "decision": state.decision.trim(), "why": state.why.trim(), "answer": state.answer.trim(), "evidence": lines(&state.evidence), "rejectedAlternatives": lines(&state.rejected), "decidedBy": "human"})));
                        state.resolving = None;
                    }
                    if small(ui, &tr("Cancelar")) { state.resolving = None; }
                });
            }
        });
    }
    let resolved: Vec<&DesignQuestion> = questions.iter().copied().filter(|q| q.status != "open").collect();
    ui.horizontal(|ui| {
        ui.label(mono(&trf("Resueltas ({})", &[&resolved.len()]), 10.5).strong());
        if !resolved.is_empty() && small(ui, if state.show_resolved { "−" } else { "+" }) { state.show_resolved = !state.show_resolved; }
    });
    if state.show_resolved {
        for q in resolved {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&q.id, 10.0).color(FAINT));
                ui.label(mono(&q.question, 10.0).color(FAINT));
                ui.label(mono("→", 10.0).color(FAINT));
                ui.label(mono(&q.decision_id.clone().unwrap_or_default(), 10.0).color(Color32::from_rgb(30, 140, 60)));
                if !q.answer.trim().is_empty() { ui.label(mono(&q.answer, 10.0)); }
            });
        }
    }
}

/// Why the design is what it is: every decision with what it was based on.
pub fn decisions_tab(ui: &mut Ui, snap: &Snapshot, project_id: &str) {
    ui.label(mono(&tr("Por qué el diseño es como es: cada decisión, con sus razones, en qué se basó y qué descartó."), 10.5).color(FAINT));
    let decisions: Vec<_> = snap.design_decisions.iter().filter(|d| d.project_id == project_id).collect();
    if decisions.is_empty() { ui.label(mono(&tr("Aún no hay decisiones; nacen al resolver preguntas o al aceptar un experimento."), 10.0).color(FAINT)); }
    for d in decisions.iter().rev() {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&d.id, 10.5).strong().color(Color32::from_rgb(30, 140, 60)));
                ui.label(mono(&d.decision, 10.5));
            });
            if !d.why.trim().is_empty() { ui.label(mono(&d.why, 10.0).color(FAINT)); }
            ui.horizontal_wrapped(|ui| {
                for q in &d.question_ids { ui.label(mono(q, 9.5).color(RUST)); }
                for s in &d.system_ids { ui.label(mono(s, 9.5).color(FAINT)); }
                for e in &d.evidence { ui.label(mono(&format!("· {e}"), 9.5).color(FAINT)); }
                if !d.rejected_alternatives.is_empty() { ui.label(mono(&trf("descartadas: {}", &[&d.rejected_alternatives.join("; ")]), 9.5).color(FAINT)); }
                ui.label(mono(&trf("v{} · {}", &[&d.design_version, &d.decided_by]), 9.5).color(FAINT));
            });
        });
    }
}

/// A chip of the stroke colour, for the folded summary line of a system.
#[allow(dead_code)]
fn chip(ui: &mut Ui, text: &str) {
    Frame::new().stroke(Stroke::new(1.0_f32, RUST)).corner_radius(CornerRadius::same(3)).inner_margin(Margin::symmetric(4, 1)).show(ui, |ui| { ui.label(mono(text, 9.5)); });
}
