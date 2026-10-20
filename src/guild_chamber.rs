//! The design chamber: the living game design as the human edits it,
//! broadest first (the concept, the player experience, the loops), then
//! the systems with their maturity, the objects, and, apart from the
//! current design, the questions it has not answered and the decisions
//! that answered the others. Drafts live here until **Save** sends the
//! whole design to the service; questions and decisions go at once.

use crate::guild_studio::{area, cycle, field, mono, primary, small, FAINT, INK, PAPER_DARK, RUST};
use crate::l10n::{tr, trf};
use egui::{vec2, Color32, CornerRadius, Frame, Margin, Stroke, Ui};
use guild::design::{Concept, DesignQuestion, Experience, GameLoop, GameSystem, LivingGameDesign, Maturity, Part, Pillar, Style};
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
    pub formulas: String,
    pub tuning: String,
    pub edge_cases: String,
    pub acceptance: String,
    pub depends_on: String,
}

#[derive(Clone, Default)]
pub struct Draft {
    pub title: String,
    pub status: String,
    pub statement: String,
    pub genre: String,
    pub audience: String,
    pub usps: String,
    /// One per line: `name | statement | design test`.
    pub pillars: String,
    pub anti_pillars: String,
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
            pillars: d.pillars.iter().map(|p| [p.name.trim(), p.statement.trim(), p.design_test.trim()].join(" | ").trim_end_matches([' ', '|']).to_string()).collect::<Vec<_>>().join("\n"),
            anti_pillars: d.anti_pillars.join("\n"),
            world_fiction: d.concept.world_fiction.clone(), monetization: d.concept.monetization.clone(), platform_scope: d.concept.platform_scope.clone(), competition: d.concept.competition.clone(),
            fantasy: d.experience.fantasy.clone(), pov: d.experience.pov.clone(), feel: d.experience.player_should_feel.join("\n"), behaviors: d.experience.desired_behaviors.join("\n"),
            avoid: d.experience.avoid.join("\n"), key_moments: d.experience.key_moments.join("\n"), engagement: d.experience.engagement.clone(),
            visual: d.style.visual.clone(), audio: d.style.audio.clone(), references: d.style.references.join("\n"),
            objectives: d.objectives.clone(), interactivity: d.interactivity.clone(),
            loops: d.loops.iter().map(|l| LoopDraft { id: l.id.clone(), name: l.name.clone(), steps: l.steps.join("\n"), inputs: l.inputs.join("\n"), outputs: l.outputs.join("\n"), feedback: l.feedback.clone(), failure: l.failure.clone() }).collect(),
            systems: d.systems.iter().map(|s| SystemDraft { id: s.id.clone(), name: s.name.clone(), maturity: s.maturity, intent: s.intent.clone(), known: s.known.join("\n"), unknown: s.unknown.join("\n"), current: s.current.clone(), player_facing: s.player_facing, implementation: s.implementation.join("\n"), loops: s.loops.join(", "), parts: s.parts.join(", "), questions: s.questions.clone(), decisions: s.decisions.clone(), experiments: s.experiments.clone(), assets: s.assets.clone(), formulas: s.formulas.join("\n"), tuning: s.tuning.join("\n"), edge_cases: s.edge_cases.join("\n"), acceptance: s.acceptance.join("\n"), depends_on: s.depends_on.join(", ") }).collect(),
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
            pillars: lines(&self.pillars).into_iter().map(|l| {
                let mut f = l.splitn(3, '|').map(|x| x.trim().to_string());
                Pillar { name: f.next().unwrap_or_default(), statement: f.next().unwrap_or_default(), design_test: f.next().unwrap_or_default() }
            }).collect(),
            anti_pillars: lines(&self.anti_pillars),
            experience: Experience { fantasy: self.fantasy.trim().into(), pov: self.pov.trim().into(), player_should_feel: lines(&self.feel), desired_behaviors: lines(&self.behaviors), avoid: lines(&self.avoid), key_moments: lines(&self.key_moments), engagement: self.engagement.trim().into() },
            style: Style { visual: self.visual.trim().into(), audio: self.audio.trim().into(), references: lines(&self.references) },
            loops: self.loops.iter().filter(|l| !l.name.trim().is_empty() || !l.steps.trim().is_empty()).map(|l| GameLoop { id: l.id.clone(), name: l.name.trim().into(), steps: lines(&l.steps), inputs: lines(&l.inputs), outputs: lines(&l.outputs), feedback: l.feedback.trim().into(), failure: l.failure.trim().into(), notes: String::new() }).collect(),
            objectives: self.objectives.trim().into(),
            systems: self.systems.iter().filter(|s| !s.name.trim().is_empty()).map(|s| GameSystem { id: if s.id.trim().is_empty() { slug(&s.name) } else { s.id.clone() }, name: s.name.trim().into(), maturity: s.maturity, intent: s.intent.trim().into(), known: lines(&s.known), unknown: lines(&s.unknown), current: s.current.trim().into(), player_facing: s.player_facing, loops: csv(&s.loops), parts: csv(&s.parts), implementation: lines(&s.implementation), assets: s.assets.clone(), questions: s.questions.clone(), decisions: s.decisions.clone(), experiments: s.experiments.clone(), formulas: lines(&s.formulas), tuning: lines(&s.tuning), edge_cases: lines(&s.edge_cases), acceptance: lines(&s.acceptance), depends_on: csv(&s.depends_on) }).collect(),
            interactivity: self.interactivity.trim().into(),
            parts: self.parts.iter().filter(|p| !p.name.trim().is_empty()).map(|p| Part { id: p.id.clone(), name: p.name.trim().into(), attributes: lines(&p.attributes), rules: lines(&p.rules), notes: String::new() }).collect(),
            sections: self.sections.clone(),
            concept_maturity: self.concept_maturity, experience_maturity: self.experience_maturity, loops_maturity: self.loops_maturity,
            version: 0,
            updated_at: String::new(),
        }
    }
}

/// The Spanish source text of a production gap, for `tr`.
fn gap_es(gap: &str) -> &'static str {
    match gap { "the current design" => "el diseño actual", "edge cases" => "casos borde", "acceptance criteria" => "criterios de aceptación", _ => "ninguna incógnita abierta" }
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
    /// What the player is noting during a session.
    pub obs_text: String,
    /// The laboratory's forms.
    pub hyp_open: bool,
    pub hyp_problem: String,
    pub hyp_evidence: String,
    pub hyp_text: String,
    pub hyp_level: String,
    pub hyp_expected: String,
    pub hyp_change: String,
    pub hyp_measures: String,
    pub hyp_question: String,
    pub hyp_systems: String,
    pub exp_open: bool,
    pub exp_hypothesis: String,
    pub exp_title: String,
    pub exp_variants: String,
    pub exp_profiles: String,
    pub exp_runs: String,
    /// Per experiment: the variant chosen to accept, and the note.
    pub decide_variant: std::collections::HashMap<String, String>,
    pub decide_note: String,
    /// Who plays as an AI player, and as which persona.
    pub ai_member: String,
    pub ai_persona: String,
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
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&tr("Madurez del diseño"), 10.5).color(FAINT));
        // The bar takes what is left of the row, so a narrow dungeon never widens.
        let bar = (ui.available_width() - 40.0).clamp(40.0, 140.0);
        let (rect, _) = ui.allocate_exact_size(vec2(bar, 10.0), egui::Sense::hover());
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
    *dirty |= area(ui, &tr("Pilares, del más importante al menos (uno por línea: nombre | afirmación | prueba de diseño)"), &mut draft.pillars, 3);
    *dirty |= area(ui, &tr("Anti-pilares: lo que el juego no es (uno por línea)"), &mut draft.anti_pillars, 2);
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
            if ui.available_width() > 420.0 {
                ui.columns(2, |cols| {
                    *dirty |= area(&mut cols[0], &tr("Fórmulas, con variables y rangos (una por línea)"), &mut s.formulas, 2);
                    *dirty |= area(&mut cols[1], &tr("Perillas: nombre: valor (rango seguro) — qué cambia"), &mut s.tuning, 2);
                });
                ui.columns(2, |cols| {
                    *dirty |= area(&mut cols[0], &tr("Casos borde: el cero, el tope, dos eventos a la vez"), &mut s.edge_cases, 2);
                    *dirty |= area(&mut cols[1], &tr("Criterios de aceptación verificables (nunca «se siente bien»)"), &mut s.acceptance, 2);
                });
            } else {
                *dirty |= area(ui, &tr("Fórmulas, con variables y rangos (una por línea)"), &mut s.formulas, 2);
                *dirty |= area(ui, &tr("Perillas: nombre: valor (rango seguro) — qué cambia"), &mut s.tuning, 2);
                *dirty |= area(ui, &tr("Casos borde: el cero, el tope, dos eventos a la vez"), &mut s.edge_cases, 2);
                *dirty |= area(ui, &tr("Criterios de aceptación verificables (nunca «se siente bien»)"), &mut s.acceptance, 2);
            }
            let gaps = guild::design::production_gaps(&GameSystem { current: s.current.clone(), unknown: lines(&s.unknown), edge_cases: lines(&s.edge_cases), acceptance: lines(&s.acceptance), ..Default::default() });
            if !gaps.is_empty() { ui.label(mono(&trf("Para producción le falta: {}", &[&gaps.iter().map(|g| tr(gap_es(g))).collect::<Vec<_>>().join(", ")]), 10.0).color(FAINT)); }
            ui.horizontal_wrapped(|ui| {
                *dirty |= field(ui, &tr("Depende de"), &mut s.depends_on, 140.0);
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

// ---- Play: the game, the session, what the player noticed

pub fn observation_label(kind: &str) -> String {
    tr(match kind { "confusing" => "Confuso", "frustrating" => "Frustrante", "interesting" => "Interesante", "great_moment" => "Gran momento", "bug" => "Bug", _ => "Comentario" })
}

fn observation_color(kind: &str) -> Color32 {
    match kind {
        "confusing" => Color32::from_rgb(190, 120, 30),
        "frustrating" => Color32::from_rgb(190, 40, 30),
        "interesting" => Color32::from_rgb(40, 110, 170),
        "great_moment" => Color32::from_rgb(30, 140, 60),
        "bug" => Color32::from_rgb(150, 40, 120),
        _ => FAINT,
    }
}

fn short(id: &str) -> &str { if id.len() > 8 { &id[..8] } else { id } }

/// Play the current build, note what you notice while playing, and see
/// what the telemetry shows next to the experience the design wants.
pub fn play_tab(ui: &mut Ui, state: &mut ChamberState, snap: &Snapshot, project_id: &str, results: &std::collections::HashMap<String, Value>, commands: &mut Vec<(&'static str, Value)>, requests: &mut Vec<(&'static str, Value, &'static str)>) {
    ui.label(mono(&tr("Juega la build y anota lo que notas mientras juegas: cada nota queda ligada a la build, la versión del diseño, la run y la ronda."), 10.5).color(FAINT));
    let project = snap.projects.iter().find(|p| p.id == project_id);
    match project.and_then(|p| p.play.as_ref()) {
        Some(_) => {
            if primary(ui, &tr("▶ Jugar la build actual")) { requests.push(("play.start", json!({"projectId": project_id}), "play")); }
        }
        None => {
            ui.label(mono(&tr("El proyecto no dice cómo se juega. Añade a .guild/guild.toml:"), 10.0).color(Color32::from_rgb(190, 40, 30)));
            ui.label(mono("[play]\nargv = [\"python3\", \"game.py\", \"seed={seed}\"]", 9.5).color(INK));
        }
    }
    // Sessions running now.
    let live: Vec<&guild::experiment::Playtest> = snap.playtests.iter().filter(|p| p.project_id == project_id && p.kind == guild::experiment::PlaytestKind::Human && p.status == "running").collect();
    for pt in live {
        Frame::new().fill(PAPER_DARK).stroke(Stroke::new(1.5_f32, Color32::from_rgb(30, 140, 60))).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&tr("En juego"), 10.5).strong().color(Color32::from_rgb(30, 140, 60)));
                if let Some(sha) = &pt.build_sha { ui.label(mono(&trf("build {}", &[&short(sha)]), 10.0).color(FAINT)); }
                if let Some(v) = &pt.variant_id { ui.label(mono(&trf("variante {}", &[v]), 10.0).color(FAINT)); }
                if let Some(v) = pt.design_version { ui.label(mono(&format!("v{v}"), 10.0).color(FAINT)); }
            });
            area(ui, &tr("Qué notaste (opcional)"), &mut state.obs_text, 2);
            ui.horizontal_wrapped(|ui| {
                for kind in guild::experiment::OBSERVATION_KINDS {
                    let label = observation_label(kind);
                    if ui.add(egui::Button::new(mono(&label, 10.0).strong().color(Color32::WHITE)).fill(observation_color(kind)).corner_radius(CornerRadius::same(3))).clicked() {
                        commands.push(("observation.create", json!({"projectId": project_id, "playtestId": pt.id, "kind": kind, "text": state.obs_text.trim()})));
                        state.obs_text.clear();
                    }
                }
            });
            if small(ui, &tr("Terminar sesión")) { commands.push(("play.finish", json!({"playtestId": pt.id}))); }
        });
    }
    ai_players(ui, state, snap, project_id, commands);
    findings_list(ui, snap, project_id, commands);
    // What players noticed.
    let notes: Vec<&guild::experiment::Observation> = snap.observations.iter().filter(|o| o.project_id == project_id).collect();
    ui.label(mono(&trf("Observaciones ({})", &[&notes.len()]), 10.5).strong());
    for o in notes.iter().rev().take(30) {
        ui.horizontal_wrapped(|ui| {
            ui.label(mono(&o.id, 10.0).color(FAINT));
            ui.label(mono(&observation_label(&o.kind), 10.0).strong().color(observation_color(&o.kind)));
            ui.label(mono(if o.text.is_empty() { "—" } else { &o.text }, 10.0));
            if let Some(r) = o.round { ui.label(mono(&trf("ronda {}", &[&r]), 9.5).color(FAINT)); }
            if let Some(v) = &o.variant_id { ui.label(mono(&trf("variante {}", &[v]), 9.5).color(FAINT)); }
            match &o.question_id {
                Some(q) => { ui.label(mono(&format!("→ {q}"), 9.5).color(RUST)); }
                None => { if small(ui, &tr("→ Pregunta")) { commands.push(("observation.promote", json!({"projectId": project_id, "observationId": o.id}))); } }
            }
        });
    }
    // Observed against desired.
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&tr("Observado vs deseado"), 10.5).strong());
        if small(ui, &tr("Analizar playtests")) { requests.push(("signals", json!({"projectId": project_id}), "signals")); }
    });
    let Some(found) = results.get("signals") else {
        ui.label(mono(&tr("Calcula, sin IA, lo que muestran los eventos de las partidas jugadas: opciones dominantes, atributos que siempre ganan, opciones que nadie elige."), 10.0).color(FAINT));
        return;
    };
    ui.label(mono(&trf("{} playtests ({} humanos, {} sintéticos)", &[&found["playtests"], &found["human"], &found["synthetic"]]), 10.0).color(FAINT));
    let list = |ui: &mut Ui, title: String, items: &Value| {
        ui.label(mono(&title, 10.0).strong());
        for i in items.as_array().into_iter().flatten() { ui.label(mono(&format!("· {}", i.as_str().unwrap_or("")), 10.0)); }
    };
    Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
        list(ui, tr("El diseño quiere"), &found["desired"]);
        list(ui, tr("El diseño evita"), &found["avoid"]);
    });
    let signals = found["signals"].as_array().cloned().unwrap_or_default();
    if signals.is_empty() { ui.label(mono(&tr("Nada destacable en los eventos todavía."), 10.0).color(FAINT)); }
    for s in signals {
        let key = s["key"].as_str().unwrap_or("").to_string();
        let source = format!("signal:{key}");
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.label(mono(s["statement"].as_str().unwrap_or(""), 10.5));
            ui.horizontal_wrapped(|ui| {
                let asked = snap.questions.iter().find(|q| q.project_id == project_id && q.source.as_deref() == Some(source.as_str()));
                match asked {
                    Some(q) => { ui.label(mono(&format!("→ {} ({})", q.id, q.status), 10.0).color(RUST)); }
                    None => {
                        ui.label(mono(&format!("→ {}", s["suggested_question"].as_str().unwrap_or("")), 10.0).color(FAINT));
                        if small(ui, &tr("Abrir pregunta")) {
                            commands.push(("question.create", json!({"projectId": project_id, "question": s["suggested_question"], "context": format!("{} ({} playtests)", s["statement"].as_str().unwrap_or(""), s["playtests"]), "raisedBy": "guild", "source": source})));
                        }
                    }
                }
            });
        });
    }
}

// ---- AI players and what the exploit hunter broke

pub fn persona_label(persona: &str) -> String {
    match persona {
        "player" => tr("jugador"),
        "novice" => tr("novato"),
        "optimizer" => tr("optimizador"),
        "explorer" => tr("explorador"),
        "exploit_hunter" => tr("cazador de exploits"),
        other => other.to_string(),
    }
}

fn severity_color(severity: &str) -> Color32 {
    match severity {
        "critical" => Color32::from_rgb(150, 20, 20),
        "high" => Color32::from_rgb(190, 40, 30),
        "medium" => Color32::from_rgb(190, 120, 30),
        _ => FAINT,
    }
}

pub fn finding_status_label(status: &str) -> String {
    tr(match status { "reproduced" => "reproducido", "not_reproduced" => "no se reproduce", "fixed" => "arreglado", "wont_fix" => "no se arreglará", _ => "sin verificar" })
}

/// A member plays the build as a persona, one move per turn; what it did
/// and what it said afterwards stay with the playtest.
fn ai_players(ui: &mut Ui, state: &mut ChamberState, snap: &Snapshot, project_id: &str, commands: &mut Vec<(&'static str, Value)>) {
    use guild::experiment::PlaytestKind;
    ui.add_space(4.0);
    ui.label(mono(&tr("Jugadores IA"), 10.5).strong());
    let project = snap.projects.iter().find(|p| p.id == project_id);
    if project.and_then(|p| p.ai_play.as_ref()).is_none() {
        ui.label(mono(&tr("Para que un miembro juegue, el juego debe hablar el protocolo de agentes. Añade a .guild/guild.toml:"), 10.0).color(FAINT));
        ui.label(mono("[ai_play]\nargv = [\"python3\", \"game.py\", \"profile=agent\", \"seed={seed}\"]", 9.5).color(INK));
    } else {
        let members: Vec<&guild::Member> = snap.members.iter().filter(|m| m.enabled).collect();
        if state.ai_member.is_empty() || !members.iter().any(|m| m.id == state.ai_member) {
            // Whoever has a playing role, else the first member.
            state.ai_member = members.iter().find(|m| m.roles.iter().any(|r| r == "ai-player" || r == "exploit-hunter")).or(members.first()).map(|m| m.id.clone()).unwrap_or_default();
        }
        if state.ai_persona.is_empty() { state.ai_persona = "player".into(); }
        ui.horizontal_wrapped(|ui| {
            let shown = members.iter().find(|m| m.id == state.ai_member).map(|m| format!("{} · {}", m.name, m.harness)).unwrap_or_else(|| "—".into());
            egui::ComboBox::from_id_salt("ai-member").selected_text(mono(&shown, 10.0)).width(150.0).show_ui(ui, |ui| {
                for m in &members { ui.selectable_value(&mut state.ai_member, m.id.clone(), mono(&format!("{} · {}", m.name, m.harness), 10.0)); }
            });
            egui::ComboBox::from_id_salt("ai-persona").selected_text(mono(&persona_label(&state.ai_persona), 10.0)).width(130.0).show_ui(ui, |ui| {
                for p in guild::aiplay::PERSONAS { ui.selectable_value(&mut state.ai_persona, p.to_string(), mono(&persona_label(p), 10.0)); }
            });
            if primary(ui, &tr("▶ Que juegue")) && !state.ai_member.is_empty() {
                commands.push(("ai.play", json!({"projectId": project_id, "memberId": state.ai_member, "persona": state.ai_persona})));
            }
        });
    }
    let mut sessions: Vec<&guild::experiment::Playtest> = snap.playtests.iter().filter(|p| p.project_id == project_id && p.kind == PlaytestKind::AiPlayer).collect();
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    for pt in sessions.into_iter().take(6) {
        let persona = pt.profile.rsplit(':').next().unwrap_or("");
        let running = pt.status == "running";
        Frame::new().fill(PAPER_DARK).stroke(Stroke::new(if running { 1.5_f32 } else { 0.0 }, Color32::from_rgb(40, 110, 170))).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(pt.player.as_deref().unwrap_or("?"), 10.5).strong());
                ui.label(mono(&persona_label(persona), 10.0).color(Color32::from_rgb(40, 110, 170)));
                if running {
                    ui.label(mono(&tr("jugando…"), 10.0).color(Color32::from_rgb(30, 140, 60)));
                    if small(ui, &tr("Detener")) { commands.push(("ai.stop", json!({"playtestId": pt.id}))); }
                } else {
                    let turns = pt.metrics["ai"]["turns"].as_u64().unwrap_or(0);
                    ui.label(mono(&trf("{} jugadas", &[&turns]), 10.0).color(FAINT));
                    let won = pt.metrics["runs"]["completed"].as_u64().unwrap_or(0) > 0;
                    if pt.status == "finished" { ui.label(mono(&tr(if won { "ganó" } else { "perdió" }), 10.0).strong().color(if won { Color32::from_rgb(30, 140, 60) } else { RUST })); } else { ui.label(mono(&pt.status, 10.0).color(RUST)); }
                }
                if let Some(sha) = &pt.build_sha { ui.label(mono(&trf("build {}", &[&short(sha)]), 9.5).color(FAINT)); }
                if let Some(v) = &pt.variant_id { ui.label(mono(&trf("variante {}", &[v]), 9.5).color(FAINT)); }
            });
            if !pt.observations.trim().is_empty() { ui.label(mono(&format!("“{}”", pt.observations.trim()), 10.0).italics().color(INK)); }
            if let Some(f) = &pt.failure { ui.label(mono(f, 9.5).color(RUST)); }
        });
    }
}

/// What the exploit hunter broke: each with its replay (build, seed,
/// moves), whether the replay still breaks it, and the question it raised.
fn findings_list(ui: &mut Ui, snap: &Snapshot, project_id: &str, commands: &mut Vec<(&'static str, Value)>) {
    let findings: Vec<&guild::aiplay::Finding> = snap.findings.iter().filter(|f| f.project_id == project_id).collect();
    if findings.is_empty() { return; }
    ui.add_space(4.0);
    ui.label(mono(&trf("Hallazgos ({})", &[&findings.len()]), 10.5).strong());
    for f in findings.iter().rev().take(20) {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&f.id, 10.0).color(FAINT));
                ui.label(mono(&f.severity, 9.5).strong().color(severity_color(&f.severity)));
                ui.label(mono(&f.title, 10.5));
            });
            if !f.impact.is_empty() { ui.label(mono(&f.impact, 10.0).color(FAINT)); }
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&finding_status_label(&f.status), 10.0).strong().color(if f.status == "reproduced" { RUST } else { FAINT }));
                ui.label(mono(&trf("semilla {} · {} jugadas", &[&f.seed.unwrap_or(0), &f.actions.len()]), 9.5).color(FAINT));
                if let Some(r) = f.round { ui.label(mono(&trf("ronda {}", &[&r]), 9.5).color(FAINT)); }
                if small(ui, &tr("↻ Reproducir")) { commands.push(("finding.verify", json!({"projectId": project_id, "findingId": f.id}))); }
                match &f.question_id {
                    Some(q) => { ui.label(mono(&format!("→ {q}"), 9.5).color(RUST)); }
                    None => { if small(ui, &tr("→ Pregunta")) { commands.push(("finding.promote", json!({"projectId": project_id, "findingId": f.id}))); } }
                }
            });
            if let Some(v) = &f.verification { ui.label(mono(v, 9.5).color(FAINT)); }
        });
    }
}

// ---- Laboratory: hypotheses, experiments, the decision

fn hypothesis_label(h: guild::experiment::HypothesisStatus) -> String {
    use guild::experiment::HypothesisStatus::*;
    tr(match h { Proposed => "propuesta", Experimenting => "en experimento", Supported => "respaldada", Rejected => "rechazada", Inconclusive => "no concluyente", NotAssessed => "sin evaluar" })
}

fn experiment_label(e: guild::experiment::ExperimentStatus) -> String {
    use guild::experiment::ExperimentStatus::*;
    tr(match e { Draft => "borrador", Building => "construyendo variantes", Playtesting => "jugando", Analysing => "analizando", Decided => "decisión requerida", Accepted => "aceptado", Rejected => "rechazado", Failed => "falló" })
}

fn verdict_label(v: &str) -> String {
    tr(match v { "supports" => "respalda la hipótesis", "rejects" => "rechaza la hipótesis", _ => "no concluyente" })
}

/// `id: key=value, key=value` per line, as variants.
pub fn parse_variants(text: &str) -> Vec<Value> {
    lines(text).iter().filter_map(|line| {
        let (id, rest) = line.split_once(':').map(|(a, b)| (a.trim(), b.trim())).unwrap_or((line.trim(), ""));
        if id.is_empty() { return None; }
        let params: serde_json::Map<String, Value> = rest.split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| {
            let v = v.trim();
            (k.trim().to_string(), v.parse::<f64>().map(Value::from).unwrap_or_else(|_| Value::from(v)))
        }).collect();
        Some(json!({"id": id, "name": id, "params": params}))
    }).collect()
}

pub fn laboratory_tab(ui: &mut Ui, state: &mut ChamberState, snap: &Snapshot, project_id: &str, commands: &mut Vec<(&'static str, Value)>, requests: &mut Vec<(&'static str, Value, &'static str)>) {
    ui.label(mono(&tr("Una duda se vuelve hipótesis; una hipótesis, un experimento con variantes que se construyen, se juegan y se analizan. Tú decides."), 10.5).color(FAINT));
    let hypotheses: Vec<&guild::experiment::Hypothesis> = snap.hypotheses.iter().filter(|h| h.project_id == project_id).collect();
    // Hypotheses.
    ui.horizontal(|ui| {
        ui.label(mono(&trf("Hipótesis ({})", &[&hypotheses.len()]), 10.5).strong());
        let label = if state.hyp_open { "−".to_string() } else { tr("+ hipótesis") };
        if small(ui, &label) { state.hyp_open = !state.hyp_open; }
    });
    if state.hyp_open {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            if state.hyp_level.is_empty() { state.hyp_level = "loop".into(); }
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&tr("Pregunta"), 10.5).color(FAINT));
                let open: Vec<&DesignQuestion> = snap.questions.iter().filter(|q| q.project_id == project_id && q.status == "open").collect();
                egui::ComboBox::from_id_salt("hyp-question").selected_text(mono(if state.hyp_question.is_empty() { "—" } else { &state.hyp_question }, 10.0)).width(90.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.hyp_question, String::new(), mono("—", 10.0));
                    for q in open { ui.selectable_value(&mut state.hyp_question, q.id.clone(), mono(&format!("{} {}", q.id, q.question.chars().take(40).collect::<String>()), 10.0)); }
                });
                ui.label(mono(&tr("Nivel"), 10.5).color(FAINT));
                for (id, label) in [("whole", tr("todo")), ("loop", tr("loop")), ("part", tr("parte"))] { ui.selectable_value(&mut state.hyp_level, id.to_string(), mono(&label, 10.0)); }
            });
            area(ui, &tr("Problema observado"), &mut state.hyp_problem, 2);
            area(ui, &tr("Evidencia"), &mut state.hyp_evidence, 1);
            area(ui, &tr("Hipótesis: qué creemos que lo resuelve y por qué"), &mut state.hyp_text, 2);
            area(ui, &tr("Efecto esperado"), &mut state.hyp_expected, 1);
            area(ui, &tr("Cambio propuesto"), &mut state.hyp_change, 1);
            area(ui, &tr("Qué medir (una por línea)"), &mut state.hyp_measures, 2);
            field(ui, &tr("Sistemas"), &mut state.hyp_systems, 200.0);
            if !state.hyp_problem.trim().is_empty() && !state.hyp_text.trim().is_empty() && primary(ui, &tr("Crear hipótesis")) {
                commands.push(("hypothesis.create", json!({"projectId": project_id, "problem": state.hyp_problem.trim(), "evidence": state.hyp_evidence.trim(), "hypothesis": state.hyp_text.trim(), "level": state.hyp_level,
                    "expectedEffect": state.hyp_expected.trim(), "proposedChange": state.hyp_change.trim(), "measurements": lines(&state.hyp_measures),
                    "relatedSystems": state.hyp_systems.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>(), "questionId": if state.hyp_question.is_empty() { Value::Null } else { json!(state.hyp_question) }})));
                for f in [&mut state.hyp_problem, &mut state.hyp_evidence, &mut state.hyp_text, &mut state.hyp_expected, &mut state.hyp_change, &mut state.hyp_measures, &mut state.hyp_systems, &mut state.hyp_question] { f.clear(); }
                state.hyp_open = false;
            }
        });
    }
    for h in &hypotheses {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&hypothesis_label(h.status), 10.0).strong().color(RUST));
                ui.label(mono(&format!("{:?}", h.level).to_lowercase(), 9.5).color(FAINT));
                if let Some(q) = &h.question_id { ui.label(mono(&format!("← {q}"), 9.5).color(FAINT)); }
            });
            ui.label(mono(&h.hypothesis, 10.5));
            ui.label(mono(&h.problem, 10.0).color(FAINT));
            if !h.proposed_change.is_empty() { ui.label(mono(&format!("→ {}", h.proposed_change), 10.0).color(FAINT)); }
        });
    }
    // Experiments.
    let experiments: Vec<&guild::experiment::Experiment> = snap.experiments.iter().filter(|e| e.project_id == project_id).collect();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(mono(&trf("Experimentos ({})", &[&experiments.len()]), 10.5).strong());
        let label = if state.exp_open { "−".to_string() } else { tr("+ experimento") };
        if !hypotheses.is_empty() && small(ui, &label) { state.exp_open = !state.exp_open; }
    });
    if state.exp_open {
        Frame::new().fill(PAPER_DARK).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            if state.exp_hypothesis.is_empty() { state.exp_hypothesis = hypotheses.last().map(|h| h.id.clone()).unwrap_or_default(); }
            if state.exp_profiles.is_empty() { state.exp_profiles = "greedy, explorer".into(); }
            if state.exp_runs.is_empty() { state.exp_runs = "3".into(); }
            if state.exp_variants.is_empty() { state.exp_variants = "control:\na: key=value".into(); }
            let shown = hypotheses.iter().find(|h| h.id == state.exp_hypothesis).map(|h| h.hypothesis.chars().take(40).collect::<String>()).unwrap_or_default();
            egui::ComboBox::from_id_salt("exp-hypothesis").selected_text(mono(&shown, 10.0)).width(220.0).show_ui(ui, |ui| {
                for h in &hypotheses { ui.selectable_value(&mut state.exp_hypothesis, h.id.clone(), mono(&h.hypothesis.chars().take(60).collect::<String>(), 10.0)); }
            });
            field(ui, &tr("Título"), &mut state.exp_title, 220.0);
            area(ui, &tr("Variantes, una por línea: id: parámetro=valor, parámetro=valor (control sin parámetros es el juego tal cual)"), &mut state.exp_variants, 4);
            field(ui, &tr("Perfiles de jugador"), &mut state.exp_profiles, 160.0);
            field(ui, &tr("Partidas por perfil"), &mut state.exp_runs, 40.0);
            let variants = parse_variants(&state.exp_variants);
            ui.label(mono(&trf("{} variantes", &[&variants.len()]), 9.5).color(FAINT));
            if !variants.is_empty() && !state.exp_hypothesis.is_empty() && primary(ui, &tr("Crear experimento")) {
                let mut params = json!({"projectId": project_id, "hypothesisId": state.exp_hypothesis, "variants": variants,
                    "plan": {"profiles": state.exp_profiles.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>(), "runs_per_profile": state.exp_runs.trim().parse::<u64>().unwrap_or(3)}});
                if !state.exp_title.trim().is_empty() { params["title"] = json!(state.exp_title.trim()); }
                commands.push(("experiment.create", params));
                state.exp_title.clear();
                state.exp_variants.clear();
                state.exp_open = false;
            }
        });
    }
    for e in experiments.iter().rev() {
        let decided = e.status == guild::experiment::ExperimentStatus::Decided;
        let stroke = if decided { Stroke::new(2.0_f32, Color32::from_rgb(190, 40, 30)) } else { Stroke::NONE };
        Frame::new().fill(PAPER_DARK).stroke(stroke).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&e.title, 10.5).strong());
                let color = if decided { Color32::from_rgb(190, 40, 30) } else { RUST };
                ui.label(mono(&experiment_label(e.status), 10.0).strong().color(color));
            });
            let playtests: Vec<&guild::experiment::Playtest> = snap.playtests.iter().filter(|p| p.experiment_id.as_deref() == Some(e.id.as_str()) && p.status == "finished").collect();
            let human = playtests.iter().filter(|p| p.kind == guild::experiment::PlaytestKind::Human).count();
            use guild::experiment::ExperimentStatus::*;
            match e.status {
                Draft => {
                    for v in &e.variants { ui.label(mono(&format!("· {}: {}", v.id, serde_json::to_string(&v.params).unwrap_or_default()), 10.0)); }
                    if primary(ui, &tr("Iniciar: construir y jugar las variantes")) { commands.push(("experiment.start", json!({"experimentId": e.id}))); }
                }
                Building | Playtesting | Analysing => {
                    let built = e.variants.iter().filter(|v| v.attempt_id.is_some()).count();
                    let planned = e.variants.len() * e.plan.profiles.len() * e.plan.runs_per_profile.max(1) as usize;
                    ui.label(mono(&trf("{}/{} variantes construidas · {}/{} playtests", &[&built, &e.variants.len(), &(playtests.len() - human), &planned]), 10.0).color(FAINT));
                }
                Failed => {
                    ui.label(mono(e.failure.as_deref().unwrap_or(""), 10.0).color(Color32::from_rgb(190, 40, 30)));
                    ui.horizontal(|ui| {
                        if small(ui, &tr("Iterar")) { commands.push(("experiment.iterate", json!({"experimentId": e.id}))); }
                        if small(ui, &tr("Rechazar")) { commands.push(("experiment.reject", json!({"experimentId": e.id}))); }
                    });
                }
                Decided => decision_card(ui, state, snap, project_id, e, &playtests, human, commands, requests),
                Accepted | Rejected => {
                    if let Some(d) = &e.decision { ui.label(mono(&format!("{} {} · {}", d.action, d.variant_id.clone().unwrap_or_default(), d.note), 10.0)); }
                    let dec = snap.design_decisions.iter().find(|d| d.experiment_ids.contains(&e.id));
                    if let Some(d) = dec { ui.label(mono(&format!("→ {}: {}", d.id, d.decision), 10.0).color(Color32::from_rgb(30, 140, 60))); }
                }
            }
        });
    }
}

/// The human's decision: the problem, the evidence, the analyst's
/// recommendation, each variant's numbers with a button to play it, and
/// accept, iterate or reject.
#[allow(clippy::too_many_arguments)]
fn decision_card(ui: &mut Ui, state: &mut ChamberState, snap: &Snapshot, project_id: &str, e: &guild::experiment::Experiment, playtests: &[&guild::experiment::Playtest], human: usize, commands: &mut Vec<(&'static str, Value)>, requests: &mut Vec<(&'static str, Value, &'static str)>) {
    let h = snap.hypotheses.iter().find(|h| h.id == e.hypothesis_id);
    if let Some(h) = h {
        ui.label(mono(&trf("Problema: {}", &[&h.problem]), 10.5));
        ui.label(mono(&trf("Hipótesis: {}", &[&h.hypothesis]), 10.0).color(FAINT));
    }
    let notes = snap.observations.iter().filter(|o| o.experiment_id.as_deref() == Some(e.id.as_str())).count();
    ui.label(mono(&trf("Evidencia: {} playtests sintéticos · {} humanos · {} observaciones", &[&(playtests.len() - human), &human, &notes]), 10.0).color(FAINT));
    let result = e.result.clone().unwrap_or_default();
    ui.label(mono(&trf("Analista: {}{}", &[&verdict_label(&result.verdict), &result.confidence.map(|c| format!(" · {:.0}%", c * 100.0)).unwrap_or_default()]), 10.5).strong());
    for x in &result.evidence { ui.label(mono(&format!("· {x}"), 10.0)); }
    for x in &result.unexpected { ui.label(mono(&format!("! {x}"), 10.0).color(Color32::from_rgb(190, 120, 30))); }
    if !result.recommended_next.is_empty() { ui.label(mono(&trf("Siguiente: {}", &[&result.recommended_next]), 10.0).color(FAINT)); }
    // Each variant, its numbers, and a way to play it: one block per
    // variant, so a narrow dungeon reads it as well as a wide one.
    for v in &e.variants {
        let recommended = result.recommended_variant.as_deref() == Some(v.id.as_str());
        let profiles = &result.metrics[&v.id]["profiles"];
        Frame::new().stroke(Stroke::new(1.0_f32, if recommended { Color32::from_rgb(30, 140, 60) } else { RUST })).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(4)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(mono(&format!("{}{}", if recommended { "★ " } else { "" }, v.name), 10.0).strong());
                ui.label(mono(&serde_json::to_string(&v.params).unwrap_or_default(), 9.0).color(FAINT));
                if v.attempt_id.is_some() && small(ui, &tr("▶ Jugar")) {
                    requests.push(("play.start", json!({"projectId": project_id, "experimentId": e.id, "variantId": v.id}), "play"));
                }
            });
            for (profile, m) in profiles.as_object().into_iter().flatten() {
                let completion = m["runs"]["completion_rate"].as_f64().map(|r| trf("{}% completadas", &[&format!("{:.0}", r * 100.0)])).unwrap_or_default();
                let top = match (m["choices"]["top"].as_str(), m["choices"]["top_share"].as_f64()) { (Some(c), Some(s)) => trf("más tomada: {} {}%", &[&c, &format!("{:.0}", s * 100.0)]), _ => String::new() };
                ui.label(mono(&format!("{profile}: {completion} · {top}"), 9.5));
            }
        });
    }
    let choice = state.decide_variant.entry(e.id.clone()).or_insert_with(|| result.recommended_variant.clone().unwrap_or_else(|| e.variants.first().map(|v| v.id.clone()).unwrap_or_default()));
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&tr("Adoptar"), 10.5).color(FAINT));
        egui::ComboBox::from_id_salt(("decide", &e.id)).selected_text(mono(choice, 10.0)).width(90.0).show_ui(ui, |ui| {
            for v in &e.variants { ui.selectable_value(choice, v.id.clone(), mono(&v.name, 10.0)); }
        });
    });
    area(ui, &tr("Por qué (queda en la decisión)"), &mut state.decide_note, 1);
    let variant = choice.clone();
    ui.horizontal_wrapped(|ui| {
        if primary(ui, &tr("Aceptar")) { commands.push(("experiment.accept", json!({"experimentId": e.id, "variantId": variant, "note": state.decide_note.trim()}))); state.decide_note.clear(); }
        if small(ui, &tr("Iterar")) { commands.push(("experiment.iterate", json!({"experimentId": e.id, "note": state.decide_note.trim()}))); state.decide_note.clear(); }
        if small(ui, &tr("Rechazar")) { commands.push(("experiment.reject", json!({"experimentId": e.id, "note": state.decide_note.trim()}))); state.decide_note.clear(); }
    });
}

/// A chip of the stroke colour, for the folded summary line of a system.
#[allow(dead_code)]
fn chip(ui: &mut Ui, text: &str) {
    Frame::new().stroke(Stroke::new(1.0_f32, RUST)).corner_radius(CornerRadius::same(3)).inner_margin(Margin::symmetric(4, 1)).show(ui, |ui| { ui.label(mono(text, 9.5)); });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_from_the_chamber_keeps_pillars_and_the_precise_fields_of_a_system() {
        let mut d = guild::design::template("p", "Juego", "videogame");
        d.pillars = vec![Pillar { name: "Words over reflexes".into(), statement: "you win by choosing".into(), design_test: "can a slow player win?".into() }, Pillar { name: "Short runs".into(), ..Default::default() }];
        d.anti_pillars = vec!["a typing game".into()];
        let s = &mut d.systems[0];
        s.formulas = vec!["power = sum(letter power) × length_multiplier^(len−3)".into()];
        s.tuning = vec!["length_multiplier: 1.2 (1.0–1.5) — how much length pays".into()];
        s.edge_cases = vec!["an empty rare pool".into()];
        s.acceptance = vec!["a matched 3-letter word beats an unmatched 6-letter word".into()];
        s.depends_on = vec!["economy".into(), "progression".into()];
        let back = Draft::from_design(&d).to_design("p");
        assert_eq!(back.pillars, d.pillars);
        assert_eq!(back.anti_pillars, d.anti_pillars);
        assert_eq!(back.systems[0], d.systems[0]);
    }
}
