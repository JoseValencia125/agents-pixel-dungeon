//! The widget's view of the guild service: its snapshot kept current by
//! the event stream, the guild's members drawn as rooms beside Herdr's
//! agents, a panel for a member's attempt (candidate, verifiers,
//! permissions, transcript) and the quest board. Every change goes to
//! the service as a command; nothing is decided here.

use crate::herdr::Agent;
use crate::l10n::{tr, trf};
use egui::{vec2, Align2, Color32, Context, CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use guild::api::{Client, StreamMessage};
use guild::domain::*;
use serde_json::{json, Value};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ID_PREFIX: &str = "guild:";

enum Msg {
    Connected(bool),
    Snapshot(Snapshot),
    Event(Event),
    Transcript(String, Vec<Event>),
    Note(String, &'static str),
}

pub struct GuildView {
    client: Client,
    pub snapshot: Option<Snapshot>,
    pub connected: bool,
    pub demo: bool,
    /// The transcript kept fresh for this attempt while its panel is open.
    watch: Arc<Mutex<Option<String>>>,
    pub transcript: Vec<Event>,
    pub transcript_of: Option<String>,
    inbox: Receiver<Msg>,
    outbox: Sender<Msg>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Lines for the activity log, drained by the app.
    pub notes: Vec<(String, &'static str)>,
    started: bool,
    /// Which member/attempt a demo permission was answered on, so the
    /// demo moves along.
    demo_tick: usize,
    /// The rooms must be rebuilt (demo toggled) even without a message.
    dirty: bool,
}

impl GuildView {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> GuildView {
        let (outbox, inbox) = channel();
        GuildView {
            client: Client::default_socket(),
            snapshot: None,
            connected: false,
            demo: false,
            watch: Arc::new(Mutex::new(None)),
            transcript: vec![],
            transcript_of: None,
            inbox,
            outbox,
            wake,
            notes: vec![],
            started: false,
            demo_tick: 0,
            dirty: true,
        }
    }

    /// Follow the service from a background thread; reconnects on its own.
    pub fn start(&mut self) {
        if self.started { return; }
        self.started = true;
        let client = self.client.clone();
        let outbox = self.outbox.clone();
        let wake = self.wake.clone();
        let watch = self.watch.clone();
        std::thread::Builder::new().name("guild-view".into()).spawn(move || follow(client, outbox, wake, watch)).ok();
    }

    /// Drain what the thread sent. True when the rooms may have changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = std::mem::take(&mut self.dirty);
        while let Ok(msg) = self.inbox.try_recv() {
            match msg {
                Msg::Connected(up) => {
                    if self.connected != up {
                        self.connected = up;
                        changed = true;
                        if !self.demo {
                            self.notes.push((if up { tr("Servicio de la guild conectado") } else { tr("Servicio de la guild apagado") }, "info"));
                        }
                        if !up { self.snapshot = None; }
                    }
                }
                Msg::Snapshot(s) => {
                    if !self.demo {
                        changed = true;
                        self.snapshot = Some(s);
                    }
                }
                Msg::Event(ev) => {
                    if !self.demo {
                        if let Some(line) = log_line(&ev, self.snapshot.as_ref()) { self.notes.push(line); }
                    }
                }
                Msg::Transcript(id, events) => {
                    self.transcript_of = Some(id);
                    self.transcript = events;
                }
                Msg::Note(text, tone) => self.notes.push((text, tone)),
            }
        }
        if self.demo && self.snapshot.as_ref().map(|s| s.service_version != "demo").unwrap_or(true) {
            self.snapshot = Some(demo_snapshot(self.demo_tick));
            changed = true;
        }
        changed
    }

    pub fn set_demo(&mut self, demo: bool) {
        self.demo = demo;
        self.dirty = true;
        self.snapshot = if demo { Some(demo_snapshot(self.demo_tick)) } else { None };
        self.transcript.clear();
        self.transcript_of = None;
    }

    /// Keep this attempt's transcript fresh (None stops).
    pub fn watch(&mut self, attempt: Option<&str>) {
        let mut w = self.watch.lock().unwrap_or_else(|e| e.into_inner());
        if w.as_deref() != attempt {
            *w = attempt.map(str::to_string);
            if let (true, Some(id)) = (self.demo, attempt) {
                self.transcript_of = Some(id.into());
                self.transcript = demo_transcript(id);
            }
        }
    }

    /// Send a command on a thread; failures land in the activity log.
    pub fn command(&mut self, method: &str, params: Value) {
        if self.demo {
            self.demo_tick += 1;
            self.snapshot = Some(demo_snapshot(self.demo_tick));
            return;
        }
        let client = self.client.clone();
        let outbox = self.outbox.clone();
        let wake = self.wake.clone();
        let method = method.to_string();
        std::thread::spawn(move || {
            if let Err(e) = client.call(&method, params) {
                let _ = outbox.send(Msg::Note(trf("Guild: {}", &[&e.message]), "blocked"));
                wake();
            }
        });
    }

    /// Start the service next to this binary, in the background.
    pub fn start_service(&mut self) {
        let outbox = self.outbox.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("agents-pixel-dungeon-guild")));
            let binary = exe.filter(|p| p.exists()).or_else(|| which("agents-pixel-dungeon-guild"));
            let Some(binary) = binary else {
                let _ = outbox.send(Msg::Note(tr("No encuentro agents-pixel-dungeon-guild junto a la app"), "blocked"));
                wake();
                return;
            };
            let output = std::process::Command::new(binary).args(["serve", "--daemon"]).output();
            let note = match output {
                Ok(o) if o.status.success() => (tr("Servicio de la guild arrancado"), "info"),
                Ok(o) => (format!("Guild: {}", String::from_utf8_lossy(&o.stderr).trim()), "blocked"),
                Err(e) => (format!("Guild: {e}"), "blocked"),
            };
            let _ = outbox.send(Msg::Note(note.0, note.1));
            wake();
        });
    }

    pub fn member_agent_id(member_id: &str) -> String { format!("{ID_PREFIX}{member_id}") }

    /// The guild's members as agents, so the scene draws them as rooms.
    pub fn agents(&self) -> Vec<Agent> {
        let Some(snap) = &self.snapshot else { return vec![] };
        snap.members.iter().filter(|m| m.enabled).map(|m| member_agent(snap, m)).collect()
    }

    /// The member, its active (or latest) attempt, that attempt's task and quest.
    pub fn context(&self, agent_id: &str) -> Option<(Member, Option<Attempt>, Option<Task>, Option<Quest>, Option<Project>)> {
        let snap = self.snapshot.as_ref()?;
        let member_id = agent_id.strip_prefix(ID_PREFIX)?;
        let member = snap.members.iter().find(|m| m.id == member_id)?.clone();
        let attempt = current_attempt(snap, &member).cloned();
        let task = attempt.as_ref().and_then(|a| snap.tasks.iter().find(|t| t.id == a.task_id)).cloned();
        let quest = task.as_ref().and_then(|t| snap.quests.iter().find(|q| q.id == t.quest_id)).cloned();
        let project = quest.as_ref().and_then(|q| snap.projects.iter().find(|p| p.id == q.project_id)).cloned();
        Some((member, attempt, task, quest, project))
    }
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(name)).find(|p| p.is_file()))
}

/// The attempt a member's room shows: the active one, else the latest
/// one still waiting for a decision, else the latest of the day.
fn current_attempt<'a>(snap: &'a Snapshot, member: &Member) -> Option<&'a Attempt> {
    let mine: Vec<&Attempt> = snap.attempts.iter().filter(|a| a.member_id == member.id).collect();
    mine.iter().copied().filter(|a| a.state.is_active()).max_by(|a, b| a.created_at.cmp(&b.created_at))
        .or_else(|| mine.iter().copied().filter(|a| matches!(a.state, AttemptState::Failed | AttemptState::TimedOut | AttemptState::Lost) && snap.tasks.iter().any(|t| t.id == a.task_id && matches!(t.state, TaskState::Failed | TaskState::Blocked))).max_by(|a, b| a.created_at.cmp(&b.created_at)))
        .or_else(|| mine.iter().copied().filter(|a| a.state == AttemptState::Passed && snap.tasks.iter().any(|t| t.id == a.task_id && matches!(t.state, TaskState::Accepted | TaskState::Integrating))).max_by(|a, b| a.created_at.cmp(&b.created_at)))
}

fn member_agent(snap: &Snapshot, m: &Member) -> Agent {
    let attempt = current_attempt(snap, m);
    let task = attempt.and_then(|a| snap.tasks.iter().find(|t| t.id == a.task_id));
    let quest = task.and_then(|t| snap.quests.iter().find(|q| q.id == t.quest_id));
    let project = quest.and_then(|q| snap.projects.iter().find(|p| p.id == q.project_id)).or_else(|| snap.projects.first());
    let workspace = attempt.and_then(|a| a.workspace_id.as_ref()).and_then(|id| snap.workspaces.iter().find(|w| &w.id == id));
    let room = room_state(task, attempt);
    let status = match room {
        "verifying" | "integrating" => "working",
        "failed" => "blocked",
        other => other,
    };
    let title = task.map(|t| t.title.clone()).unwrap_or_default();
    let activity = match (room, attempt) {
        ("working", Some(a)) if a.state == AttemptState::Running => {
            if a.summary.is_empty() { format!("{} · {title}", m.name) } else { format!("{} · {title} · {}", m.name, a.summary) }
        }
        ("unknown", _) => trf("{} · preparando el taller", &[&m.name]),
        ("blocked", Some(a)) if a.pending_permission.is_some() => trf("{} · pide permiso: {}", &[&m.name, &a.pending_permission.as_ref().unwrap().title]),
        ("failed", Some(a)) => trf("{} · falló: {}", &[&m.name, &a.failure.clone().unwrap_or_default()]),
        ("verifying", _) => trf("{} · verificando · {}", &[&m.name, &title]),
        ("integrating", _) => trf("{} · integrando · {}", &[&m.name, &title]),
        ("done", _) => trf("{} · candidato listo · {}", &[&m.name, &title]),
        ("blocked", _) => trf("{} · bloqueado · {}", &[&m.name, &task.and_then(|t| t.block_reason.clone()).unwrap_or(title.clone())]),
        _ => trf("{} · libre · {}", &[&m.name, &m.roles.join(", ")]),
    };
    // The room carries the project's emblem and name; the worktree is where the terminal opens.
    let cwd = project.map(|p| p.repo_path.clone()).or_else(|| workspace.map(|w| w.path.clone())).unwrap_or_default();
    let mut agent = Agent::new(&Self_id(&m.id), m.harness.as_str(), status, &project.map(|p| p.name.clone()).unwrap_or_else(|| "Guild".into()), &activity, &cwd);
    agent.branch = workspace.map(|w| w.branch.clone());
    agent.action = attempt.and_then(|a| a.action.clone()).or_else(|| match room { "verifying" => Some("brew".into()), "integrating" => Some("forge".into()), _ => None });
    agent
}

#[allow(non_snake_case)]
fn Self_id(member_id: &str) -> String { GuildView::member_agent_id(member_id) }

/// The thread: connect, snapshot, follow events, refresh the watched
/// transcript, and start over when the service goes away.
fn follow(client: Client, outbox: Sender<Msg>, wake: Arc<dyn Fn() + Send + Sync>, watch: Arc<Mutex<Option<String>>>) {
    loop {
        if !client.is_up() {
            let _ = outbox.send(Msg::Connected(false));
            wake();
            std::thread::sleep(Duration::from_secs(3));
            continue;
        }
        let Ok(snap) = client.call("guild.snapshot", json!({})) else { std::thread::sleep(Duration::from_secs(2)); continue };
        let Ok(snap) = serde_json::from_value::<Snapshot>(snap) else { std::thread::sleep(Duration::from_secs(2)); continue };
        let last_seq = snap.last_seq;
        let _ = outbox.send(Msg::Connected(true));
        let _ = outbox.send(Msg::Snapshot(snap));
        wake();
        let (tx, rx) = channel();
        let Ok(handle) = client.subscribe(last_seq, 1, tx) else { std::thread::sleep(Duration::from_secs(2)); continue };
        let mut dirty_at: Option<Instant> = None;
        let mut next_transcript = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok((_, StreamMessage::Event(ev))) => {
                    let _ = outbox.send(Msg::Event(ev));
                    if dirty_at.is_none() { dirty_at = Some(Instant::now()); }
                }
                Ok((_, StreamMessage::Open { .. })) => {}
                Ok((_, StreamMessage::Close)) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            // Coalesce bursts of events into one snapshot.
            if dirty_at.map(|t| t.elapsed() > Duration::from_millis(150)).unwrap_or(false) {
                dirty_at = None;
                match client.call("guild.snapshot", json!({})).ok().and_then(|v| serde_json::from_value::<Snapshot>(v).ok()) {
                    Some(snap) => { let _ = outbox.send(Msg::Snapshot(snap)); }
                    None => break,
                }
                next_transcript = Instant::now();
                wake();
            }
            if Instant::now() >= next_transcript {
                next_transcript = Instant::now() + Duration::from_secs(2);
                let watched = watch.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(id) = watched {
                    if let Ok(events) = client.call("events.attempt", json!({"attemptId": id, "limit": 400})) {
                        if let Ok(events) = serde_json::from_value::<Vec<Event>>(events) {
                            let _ = outbox.send(Msg::Transcript(id, events));
                            wake();
                        }
                    }
                }
            }
        }
        handle.close();
        let _ = outbox.send(Msg::Connected(false));
        wake();
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// What an event says in the activity log, if it deserves a line.
fn log_line(ev: &Event, snap: Option<&Snapshot>) -> Option<(String, &'static str)> {
    let task_title = || ev.task_id.as_ref().and_then(|id| snap?.tasks.iter().find(|t| &t.id == id)).map(|t| t.title.clone()).unwrap_or_default();
    let member = || ev.attempt_id.as_ref().and_then(|id| snap?.attempts.iter().find(|a| &a.id == id)).and_then(|a| snap?.members.iter().find(|m| m.id == a.member_id)).map(|m| m.name.clone()).unwrap_or_else(|| "Guild".into());
    let p = &ev.payload;
    Some(match ev.kind.as_str() {
        "quest.started" => (trf("Misión iniciada: {}", &[&p["title"].as_str().unwrap_or("")]), "info"),
        "quest.completed" => (trf("Misión completada: {}", &[&p["title"].as_str().unwrap_or("")]), "done"),
        "attempt.started" => (trf("{} toma {}", &[&p["member"].as_str().unwrap_or(""), &p["task"].as_str().unwrap_or("")]), "working"),
        "permission.requested" => (trf("{} pide permiso: {}", &[&member(), &p["title"].as_str().unwrap_or("")]), "blocked"),
        "attempt.submitted" => (trf("{} entregó un candidato para {}", &[&member(), &task_title()]), "done"),
        "attempt.failed" | "attempt.timed_out" | "attempt.lost" => (trf("{} falló en {}: {}", &[&member(), &task_title(), &p["failure"].as_str().unwrap_or("")]), "blocked"),
        "verifier.finished" => (trf("Verificador {}: {}", &[&p["verifierId"].as_str().unwrap_or(""), &p["status"].as_str().unwrap_or("")]), if p["status"] == "passed" { "done" } else { "blocked" }),
        "merge.completed" => (trf("Integrado: {}", &[&task_title()]), "done"),
        "merge.blocked" => (trf("Integración bloqueada: {}", &[&p["reason"].as_str().unwrap_or("")]), "blocked"),
        "task.blocked" => (trf("Tarea bloqueada: {} · {}", &[&p["title"].as_str().unwrap_or(""), &p["reason"].as_str().unwrap_or("")]), "blocked"),
        _ => return None,
    })
}

// ---- The member's panel

#[derive(Default)]
pub struct PanelAction {
    pub close: bool,
    /// Open the integrated terminal in this folder (the attempt's worktree).
    pub terminal: Option<String>,
}

/// An attempt's state in the user's words.
pub fn state_label(state: AttemptState) -> String {
    tr(match state {
        AttemptState::Queued => "en cola",
        AttemptState::Provisioning => "preparando el taller",
        AttemptState::Running => "trabajando",
        AttemptState::WaitingInput => "esperando permiso",
        AttemptState::Submitted => "candidato entregado",
        AttemptState::Verifying => "verificando",
        AttemptState::Passed => "aceptado",
        AttemptState::Failed => "fallido",
        AttemptState::Cancelled => "cancelado",
        AttemptState::Lost => "perdido",
        AttemptState::TimedOut => "tiempo agotado",
    })
}

const INK: Color32 = Color32::from_rgb(222, 214, 232);
const DIM: Color32 = Color32::from_rgb(150, 143, 165);

fn mono(text: &str, size: f32) -> RichText { RichText::new(text).font(egui::FontId::monospace(crate::ui::fs(size))) }

fn button(ui: &mut Ui, text: &str, color: Color32) -> bool {
    let label = mono(text, 11.0).strong().color(Color32::from_rgb(20, 18, 26));
    ui.add(egui::Button::new(label).fill(color).corner_radius(CornerRadius::same(3)).stroke(Stroke::NONE)).clicked()
}

pub fn panel(ui: &mut Ui, view: &mut GuildView, agent: &Agent, _now: f64, wide: bool) -> PanelAction {
    let mut action = PanelAction::default();
    let Some((member, attempt, task, quest, _project)) = view.context(&agent.id) else {
        ui.label(mono(&tr("Servicio de la guild apagado"), 12.0).color(DIM));
        return action;
    };
    view.watch(attempt.as_ref().map(|a| a.id.as_str()));
    ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
    ui.horizontal(|ui| {
        ui.label(mono(&member.name, 13.0).strong().color(INK));
        ui.label(mono(&format!("{} · {}", member.harness, attempt.as_ref().and_then(|a| a.model_effective.clone()).or(member.model_id.clone()).unwrap_or_else(|| tr("modelo por defecto"))), 11.0).color(DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(mono("x", 11.0).strong().color(INK)).frame(false)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
            let worktree = attempt.as_ref().and_then(|a| a.workspace_id.as_ref()).and_then(|id| view.snapshot.as_ref()?.workspaces.iter().find(|w| &w.id == id)).map(|w| w.path.clone());
            if let Some(path) = worktree {
                if ui.add(egui::Button::new(mono(">_", 11.0).strong().color(INK)).frame(false)).on_hover_text(tr("Terminal")).clicked() { action.terminal = Some(path); }
            }
        });
    });
    match (&task, &quest) {
        (Some(t), Some(q)) => {
            ui.label(mono(&format!("{} — {}", q.title, t.title), 12.0).strong().color(INK));
            if !t.instructions.trim().is_empty() { ui.label(mono(&t.instructions, 11.0).color(DIM)); }
            for ac in &t.acceptance { ui.label(mono(&format!("· {}: {}", ac.id, ac.text), 10.5).color(DIM)); }
        }
        _ => { ui.label(mono(&trf("Libre · {}", &[&member.roles.join(", ")]), 12.0).color(DIM)); }
    }
    let Some(a) = attempt else { return action };
    let state = state_label(a.state);
    let color = crate::scene::agent_color(&agent.status);
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&state, 11.0).strong().color(color));
        if let Some(c) = &a.candidate {
            ui.label(mono(&trf("{} archivos · +{} −{}", &[&c.files_changed, &c.insertions, &c.deletions]), 11.0).color(INK));
        }
        for v in &a.verifier_results {
            let ok = v.status == "passed";
            ui.label(mono(&format!("{} {}", if ok { "✓" } else { "✗" }, v.id), 11.0).color(if ok { Color32::from_rgb(48, 209, 88) } else { Color32::from_rgb(255, 69, 58) })).on_hover_text(v.tail.clone());
        }
        if let Some(f) = &a.failure { ui.label(mono(f, 11.0).color(Color32::from_rgb(255, 120, 110))); }
    });
    // Decisions.
    if let Some(p) = &a.pending_permission {
        ui.label(mono(&trf("Pide permiso: {}", &[&p.title]), 11.5).strong().color(Color32::from_rgb(255, 159, 10)));
        if !p.detail.is_empty() { ui.label(mono(&p.detail.chars().take(400).collect::<String>(), 10.5).color(DIM)); }
        ui.horizontal_wrapped(|ui| {
            for o in &p.options {
                let allow = o.kind.starts_with("allow") || o.id.contains("accept");
                let fill = if allow { Color32::from_rgb(48, 209, 88) } else { Color32::from_rgb(255, 69, 58) };
                if button(ui, &o.name, fill) {
                    view.command("permission.respond", json!({"attemptId": a.id, "requestId": p.request_id, "optionId": o.id}));
                }
            }
        });
    }
    let task_state = task.as_ref().map(|t| t.state);
    ui.horizontal_wrapped(|ui| {
        match (a.state, task_state) {
            (AttemptState::Submitted, _) | (AttemptState::Verifying, _) => {
                if let Some(t) = &task {
                    if button(ui, &tr("Aceptar e integrar"), Color32::from_rgb(48, 209, 88)) { view.command("task.accept", json!({"taskId": t.id, "integrate": true})); }
                    if button(ui, &tr("Aceptar"), Color32::from_rgb(140, 217, 115)) { view.command("task.accept", json!({"taskId": t.id})); }
                    if button(ui, &tr("Rechazar"), Color32::from_rgb(255, 69, 58)) { view.command("task.reject", json!({"taskId": t.id, "reason": "rejected from the dungeon"})); }
                }
            }
            (AttemptState::Passed, Some(TaskState::Accepted)) => {
                if let Some(t) = &task { if button(ui, &tr("Integrar"), Color32::from_rgb(48, 209, 88)) { view.command("task.integrate", json!({"taskId": t.id})); } }
            }
            (s, _) if s.is_active() => {
                if button(ui, &tr("Cancelar intento"), Color32::from_rgb(200, 200, 205)) { view.command("attempt.cancel", json!({"attemptId": a.id})); }
            }
            (_, Some(TaskState::Failed)) | (_, Some(TaskState::Blocked)) => {
                if let Some(t) = &task { if button(ui, &tr("Reintentar"), Color32::from_rgb(255, 159, 10)) { view.command("task.retry", json!({"taskId": t.id})); } }
            }
            _ => {}
        }
    });
    // Transcript.
    ui.add_space(2.0);
    let height = ui.available_height().max(60.0);
    egui::ScrollArea::vertical().id_salt(("guild-transcript", &a.id)).max_height(height).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
        if view.transcript_of.as_deref() != Some(a.id.as_str()) {
            ui.label(mono("…", 11.0).color(DIM));
            return;
        }
        let size = if wide { 11.5 } else { 10.5 };
        for ev in &view.transcript {
            let p = &ev.payload;
            match ev.kind.as_str() {
                "turn.text" => {
                    for line in p["text"].as_str().unwrap_or("").lines() {
                        ui.label(mono(line, size).color(INK));
                    }
                }
                "tool.started" => { ui.label(mono(&format!("⚒ {}", p["title"].as_str().unwrap_or("")), size).color(Color32::from_rgb(255, 159, 10))); }
                "permission.requested" => { ui.label(mono(&format!("? {}", p["title"].as_str().unwrap_or("")), size).color(Color32::from_rgb(255, 69, 58))); }
                "permission.resolved" => { ui.label(mono(&format!("→ {}", p["optionId"].as_str().unwrap_or("")), size).color(DIM)); }
                "candidate.committed" => { ui.label(mono(&trf("Candidato: {} archivos", &[&p["filesChanged"]]), size).color(Color32::from_rgb(48, 209, 88))); }
                "verifier.finished" => { ui.label(mono(&format!("{} {} {}", if p["status"] == "passed" { "✓" } else { "✗" }, p["verifierId"].as_str().unwrap_or(""), p["status"].as_str().unwrap_or("")), size).color(DIM)); }
                "attempt.failed" | "attempt.timed_out" | "attempt.lost" | "merge.blocked" => { ui.label(mono(&format!("✗ {}", p["failure"].as_str().or(p["reason"].as_str()).unwrap_or(ev.kind.as_str())), size).color(Color32::from_rgb(255, 69, 58))); }
                "merge.completed" => { ui.label(mono(&tr("Integrado en la rama de la misión"), size).color(Color32::from_rgb(48, 209, 88))); }
                _ => {}
            }
        }
    });
    action
}

// ---- The quest board

#[derive(Default)]
pub struct BoardAction {
    pub close: bool,
    pub open: Option<String>,
}

pub fn board(ctx: &Context, view: &mut GuildView) -> BoardAction {
    let ink = Color32::from_rgb(61, 38, 20);
    let faint = Color32::from_rgb(120, 90, 60);
    let mut action = BoardAction::default();
    let mut commands: Vec<(&'static str, Value)> = vec![];
    let mut start_service = false;
    egui::Area::new(egui::Id::new("quest-board")).anchor(Align2::LEFT_TOP, vec2(8.0, 38.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        Frame::new()
            .fill(Color32::from_rgb(237, 217, 168))
            .stroke(Stroke::new(2.0_f32, Color32::from_rgb(115, 74, 36)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(10))
            .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 128) })
            .show(ui, |ui| {
                ui.set_width(340.0);
                ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                ui.horizontal(|ui| {
                    ui.label(mono(&tr("Tablón de misiones"), 12.0).strong().color(ink));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(mono("x", 11.0).strong().color(ink)).frame(false)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
                    });
                });
                let Some(snap) = view.snapshot.as_ref() else {
                    ui.label(mono(&tr("Servicio de la guild apagado"), 11.0).color(faint));
                    if button(ui, &tr("Arrancar el servicio"), Color32::from_rgb(255, 159, 10)) { start_service = true; }
                    ui.label(mono("agents-pixel-dungeon-guild project add <repo>", 10.0).color(faint));
                    return;
                };
                if snap.quests.is_empty() {
                    ui.label(mono(&tr("Sin misiones todavía"), 11.0).color(faint));
                    ui.label(mono("agents-pixel-dungeon-guild quest new <project> \"…\"", 10.0).color(faint));
                }
                egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
                    for q in &snap.quests {
                        let tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == q.id).collect();
                        let done = tasks.iter().filter(|t| t.state == TaskState::Completed).count();
                        ui.horizontal(|ui| {
                            ui.label(mono(&format!("{} · {}/{}", q.title, done, tasks.len()), 11.5).strong().color(ink));
                            ui.label(mono(&format!("{:?}", q.state).to_lowercase(), 10.0).color(faint));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                match q.state {
                                    QuestState::Draft | QuestState::Ready => { if small(ui, &tr("Iniciar"), ink) { commands.push(("quest.start", json!({"questId": q.id}))); } }
                                    QuestState::Running => { if small(ui, &tr("Pausar"), ink) { commands.push(("quest.pause", json!({"questId": q.id}))); } }
                                    QuestState::Paused => { if small(ui, &tr("Reanudar"), ink) { commands.push(("quest.resume", json!({"questId": q.id}))); } }
                                    _ => {}
                                }
                            });
                        });
                        for t in tasks {
                            let attempt = snap.attempts.iter().filter(|a| a.task_id == t.id).max_by(|a, b| a.created_at.cmp(&b.created_at));
                            let member = attempt.and_then(|a| snap.members.iter().find(|m| m.id == a.member_id));
                            let state = format!("{:?}", t.state).to_lowercase();
                            let color = match t.state {
                                TaskState::Running => Color32::from_rgb(200, 110, 0),
                                TaskState::Verifying | TaskState::Accepted => Color32::from_rgb(30, 140, 60),
                                TaskState::Completed => Color32::from_rgb(30, 140, 60),
                                TaskState::Blocked | TaskState::Failed => Color32::from_rgb(190, 40, 30),
                                _ => faint,
                            };
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                let who = member.map(|m| format!(" · {}", m.name)).unwrap_or_default();
                                let row = ui.add(egui::Label::new(mono(&format!("{:<10} {}{}", state, t.title, who), 10.5).color(color)).sense(egui::Sense::click()));
                                if row.clicked() {
                                    if let Some(m) = member { action.open = Some(GuildView::member_agent_id(&m.id)); }
                                }
                            });
                            let has_actions = matches!(t.state, TaskState::Verifying | TaskState::Accepted | TaskState::Failed | TaskState::Blocked);
                            if has_actions {
                                ui.horizontal(|ui| {
                                    ui.add_space(86.0);
                                    match t.state {
                                        TaskState::Verifying => {
                                            if small(ui, &tr("Aceptar e integrar"), ink) { commands.push(("task.accept", json!({"taskId": t.id, "integrate": true}))); }
                                            if small(ui, &tr("Aceptar"), ink) { commands.push(("task.accept", json!({"taskId": t.id}))); }
                                            if small(ui, &tr("Rechazar"), ink) { commands.push(("task.reject", json!({"taskId": t.id, "reason": "rejected from the board"}))); }
                                        }
                                        TaskState::Accepted => { if small(ui, &tr("Integrar"), ink) { commands.push(("task.integrate", json!({"taskId": t.id}))); } }
                                        TaskState::Failed | TaskState::Blocked => { if small(ui, &tr("Reintentar"), ink) { commands.push(("task.retry", json!({"taskId": t.id}))); } }
                                        _ => {}
                                    }
                                });
                            }
                            if let Some(r) = &t.block_reason { ui.label(mono(&format!("           {r}"), 9.5).color(faint)); }
                            if !t.depends_on.is_empty() && matches!(t.state, TaskState::Pending | TaskState::Draft) {
                                ui.label(mono(&trf("           espera a {}", &[&t.depends_on.join(", ")]), 9.5).color(faint));
                            }
                        }
                        ui.add_space(4.0);
                    }
                });
            });
    });
    for (method, params) in commands { view.command(method, params); }
    if start_service { view.start_service(); }
    action
}

fn small(ui: &mut Ui, text: &str, ink: Color32) -> bool {
    ui.add(egui::Button::new(mono(text, 10.0).strong().color(ink)).fill(Color32::from_rgb(222, 196, 140)).stroke(Stroke::new(1.0_f32, Color32::from_rgb(160, 120, 70))).corner_radius(CornerRadius::same(3))).clicked()
}

// ---- Demo

fn demo_snapshot(tick: usize) -> Snapshot {
    let t = now();
    let member = |id: &str, name: &str, harness: HarnessKind, model: &str, role: &str| Member { id: id.into(), name: name.into(), roles: vec![role.into()], harness, model_id: Some(model.into()), mode: None, role_prompt: String::new(), enabled: true };
    let task = |id: &str, title: &str, state: TaskState, deps: &[&str]| Task {
        id: id.into(), quest_id: "q1".into(), title: tr(title), instructions: String::new(), acceptance: vec![], depends_on: deps.iter().map(|s| s.to_string()).collect(), role: "implementer".into(),
        member_id: None, model_id: None, priority: 0, state, block_reason: None, attempts_made: 1, max_attempts: 2, version: 1, created_at: t.clone(), updated_at: t.clone(),
    };
    let attempt = |id: &str, task: &str, member: &str, harness: HarnessKind, model: &str, state: AttemptState| Attempt {
        id: id.into(), task_id: task.into(), task_version: 1, quest_id: "q1".into(), project_id: "verbrawl".into(), member_id: member.into(), harness, model_requested: Some(model.into()), model_effective: Some(model.into()),
        state, workspace_id: Some(format!("w-{id}")), session_id: None, generation: 1, candidate: None, summary: String::new(), action: None, pending_permission: None, failure: None, verifier_results: vec![], turns: 1,
        started_at: Some(t.clone()), finished_at: None, created_at: t.clone(), updated_at: t.clone(),
    };
    let workspace = |id: &str, branch: &str| Workspace { id: format!("w-{id}"), project_id: "verbrawl".into(), attempt_id: id.into(), path: format!("~/demo/verbrawl/worktrees/{id}"), branch: branch.into(), base_sha: "490acd6".into(), state: WorkspaceState::Active, created_at: t.clone(), updated_at: t.clone() };
    let mut a2 = attempt("a2", "t2", "aria", HarnessKind::Kiro, "claude-sonnet-5.5", AttemptState::Running);
    a2.action = Some(["forge", "read", "brew", "type"][(tick / 3) % 4].into());
    a2.summary = tr("Añadiendo el tick de veneno al combate");
    let mut a3 = attempt("a3", "t3", "orin", HarnessKind::Codex, "gpt-5.5", AttemptState::WaitingInput);
    a3.pending_permission = Some(PermissionRequest { request_id: "perm-1".into(), title: "Run Command".into(), detail: "pnpm test --run".into(), options: vec![
        PermissionOption { id: "accept".into(), name: "Accept".into(), kind: "allow_once".into() },
        PermissionOption { id: "reject".into(), name: "Reject".into(), kind: "reject_once".into() },
    ], asked_at: t.clone() });
    let mut a4 = attempt("a4", "t4", "lira", HarnessKind::Claude, "claude-opus-5.5", AttemptState::Submitted);
    a4.candidate = Some(Candidate { base_sha: "490acd6".into(), head_sha: "dcdbee6".into(), tree_sha: "e3b0c44".into(), files_changed: 3, insertions: 48, deletions: 6, diff_artifact_id: None });
    a4.verifier_results = vec![VerifierResult { id: "unit".into(), required: true, status: "passed".into(), exit_code: Some(0), duration_ms: 1840, log_artifact_id: None, tail: "12 passed".into() }];
    a4.summary = tr("Fixtures y pruebas del contrato listos");
    if tick % 2 == 1 {
        // The user answered the demo's permission: Orin works again.
        a3.state = AttemptState::Running;
        a3.pending_permission = None;
        a3.action = Some("brew".into());
    }
    Snapshot {
        schema_version: SCHEMA_VERSION,
        last_seq: 0,
        service_version: "demo".into(),
        projects: vec![Project { id: "verbrawl".into(), name: "Verbrawl".into(), repo_path: "~/demo/verbrawl".into(), target_ref: "refs/heads/main".into(), setup: vec![], verifiers: vec![], created_at: t.clone() }],
        quests: vec![Quest { id: "q1".into(), project_id: "verbrawl".into(), title: tr("Efectos de estado"), description: String::new(), state: QuestState::Running, integration_ref: "refs/heads/guild/quest/q1".into(), base_sha: Some("490acd6".into()), plan_version: 1, created_at: t.clone(), updated_at: t.clone() }],
        tasks: vec![
            task("t1", "Contrato de datos", TaskState::Completed, &[]),
            task("t2", "Lógica de combate", TaskState::Running, &["t1"]),
            task("t3", "Interfaz", TaskState::Running, &["t1"]),
            task("t4", "Pruebas de aceptación", TaskState::Verifying, &["t1"]),
            task("t5", "Integración y QA visual", TaskState::Pending, &["t2", "t3", "t4"]),
        ],
        members: vec![
            member("aria", "Aria", HarnessKind::Kiro, "claude-sonnet-5.5", "implementer"),
            member("orin", "Orin", HarnessKind::Codex, "gpt-5.5", "implementer"),
            member("lira", "Lira", HarnessKind::Claude, "claude-opus-5.5", "tester"),
        ],
        attempts: vec![a2, a3, a4],
        workspaces: vec![workspace("a2", "guild/q1/t2/a2"), workspace("a3", "guild/q1/t3/a3"), workspace("a4", "guild/q1/t4/a4")],
        harnesses: vec![],
    }
}

fn demo_transcript(attempt_id: &str) -> Vec<Event> {
    let line = |kind: &str, payload: Value| Event { payload, ..Event::new(kind, "demo") };
    match attempt_id {
        "a2" => vec![
            line("tool.started", json!({"title": "Read File"})),
            line("turn.text", json!({"text": tr("Leo el contrato de efectos y la entidad de combate.")})),
            line("tool.started", json!({"title": "Write File"})),
            line("turn.text", json!({"text": tr("Añado PoisonEffect con duración y daño por tick, sin duplicar ticks al reaplicar.")})),
        ],
        "a3" => vec![
            line("tool.started", json!({"title": "Write File"})),
            line("turn.text", json!({"text": tr("La barra de estado muestra pilas y duración.")})),
            line("permission.requested", json!({"title": "Run Command"})),
        ],
        _ => vec![
            line("turn.text", json!({"text": tr("Fixtures y pruebas del contrato listos. Las pruebas funcionales esperan a T2 y T3.")})),
            line("candidate.committed", json!({"filesChanged": 3})),
            line("verifier.finished", json!({"verifierId": "unit", "status": "passed"})),
        ],
    }
}
