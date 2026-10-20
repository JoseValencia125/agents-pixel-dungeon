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
/// The guild's one room: a double hall with every member in it.
pub const HALL_ID: &str = "guild:hall";

enum Msg {
    Connected(bool),
    Snapshot(Snapshot),
    Event(Event),
    Transcript(String, Vec<Event>),
    Note(String, &'static str),
    Artifact(String, String),
    Result(String, Value),
}

/// The board's form for a new quest, kept between frames.
#[derive(Default)]
pub struct QuestForm {
    pub open: bool,
    /// A repo to register when the guild has no project for it yet.
    pub repo_path: String,
    pub project_id: String,
    pub title: String,
    /// One task per line: `Title :: instructions`; a leading `>` makes it
    /// depend on the task above.
    pub tasks: String,
    pub members_open: bool,
    pub member_name: String,
    pub member_harness: String,
    pub member_model: String,
    pub member_role: String,
}

impl QuestForm {
    /// The task lines as (title, instructions, depends on the previous).
    pub fn parse_tasks(text: &str) -> Vec<(String, String, bool)> {
        text.lines().filter(|l| !l.trim().is_empty()).map(|line| {
            let trimmed = line.trim_start();
            let (dep, rest) = match trimmed.strip_prefix('>') { Some(r) => (true, r.trim()), None => (false, trimmed.trim()) };
            let (title, instructions) = match rest.split_once("::") { Some((t, i)) => (t.trim(), i.trim()), None => (rest, "") };
            (title.to_string(), instructions.to_string(), dep)
        }).collect()
    }
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
    /// The last candidate diff fetched: (artifact id, text).
    pub diff: Option<(String, String)>,
    pub diff_open: bool,
    pub form: QuestForm,
    pub studio: crate::guild_studio::Studio,
    /// Answers to `request`, by tag, until taken or replaced.
    pub results: std::collections::HashMap<String, Value>,
    /// The member whose work the hall's panel shows, if one is open.
    pub hall_member: Option<String>,
    /// What the user is writing to the orchestrator.
    pub draft: String,
    /// The demo's conversation, as the user types to it.
    demo_chat: Vec<guild::orchestrator::ChatLine>,
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
            diff: None,
            diff_open: false,
            form: QuestForm::default(),
            studio: Default::default(),
            results: Default::default(),
            hall_member: None,
            draft: String::new(),
            demo_chat: vec![],
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
                Msg::Artifact(id, text) => self.diff = Some((id, text)),
                Msg::Result(tag, value) => { self.results.insert(tag, value); changed = true; }
            }
        }
        if self.demo && self.snapshot.as_ref().map(|s| s.service_version != "demo").unwrap_or(true) {
            self.snapshot = Some(self.demo_snapshot());
            changed = true;
        }
        changed
    }

    pub fn set_demo(&mut self, demo: bool) {
        self.demo = demo;
        self.dirty = true;
        self.snapshot = if demo { Some(self.demo_snapshot()) } else { None };
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
    fn demo_snapshot(&self) -> Snapshot {
        let mut snap = demo_snapshot(self.demo_tick);
        snap.conversation.extend(self.demo_chat.iter().cloned());
        snap
    }

    pub fn command(&mut self, method: &str, params: Value) {
        if self.demo {
            if method == "orchestrator.say" {
                let line = |author: &str, text: String| guild::orchestrator::ChatLine { author: author.into(), text, quest_id: None, at: now() };
                self.demo_chat.push(line("user", params["text"].as_str().unwrap_or("").into()));
                self.demo_chat.push(line("orchestrator", tr("Entendido. Lo reparto: Orin en la lógica, Lira en las pruebas. Te aviso cuando haya un candidato.")));
            }
            self.demo_tick += 1;
            self.snapshot = Some(self.demo_snapshot());
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

    /// Send a command whose answer the UI needs; it lands in `results`
    /// under `tag`. Failures go to the activity log, as for `command`.
    pub fn request(&mut self, method: &str, params: Value, tag: &str) {
        if self.demo {
            if let Some(v) = demo_result(method) { self.results.insert(tag.into(), v); }
            return;
        }
        let client = self.client.clone();
        let outbox = self.outbox.clone();
        let wake = self.wake.clone();
        let (method, tag) = (method.to_string(), tag.to_string());
        std::thread::spawn(move || {
            match client.call(&method, params) {
                Ok(v) => { let _ = outbox.send(Msg::Result(tag, v)); }
                Err(e) => { let _ = outbox.send(Msg::Note(trf("Guild: {}", &[&e.message]), "blocked")); }
            }
            wake();
        });
    }

    pub fn take_result(&mut self, tag: &str) -> Option<Value> { self.results.remove(tag) }

    /// Fetch an artifact's text (a candidate diff) on a thread.
    pub fn fetch_artifact(&mut self, id: &str) {
        if self.diff.as_ref().map(|d| d.0 == id).unwrap_or(false) { return; }
        if self.demo {
            self.diff = Some((id.into(), "diff --git a/src/status.rs b/src/status.rs\n+pub enum Status { Poison, Shield }\n".into()));
            return;
        }
        self.diff = Some((id.into(), String::new()));
        let client = self.client.clone();
        let outbox = self.outbox.clone();
        let wake = self.wake.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            match client.call("artifact.read", json!({"artifactId": id})) {
                Ok(r) => { let _ = outbox.send(Msg::Artifact(id, r["text"].as_str().unwrap_or("").to_string())); }
                Err(e) => { let _ = outbox.send(Msg::Note(trf("Guild: {}", &[&e.message]), "blocked")); }
            }
            wake();
        });
    }

    /// Register the project if a path was given, create the quest and its
    /// tasks in order, and start it when asked. One thread, in sequence,
    /// so each id feeds the next command; the first error stops it.
    pub fn create_quest(&mut self, start: bool) {
        let form = std::mem::take(&mut self.form);
        if self.demo {
            self.demo_tick += 1;
            self.snapshot = Some(self.demo_snapshot());
            return;
        }
        let client = self.client.clone();
        let outbox = self.outbox.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                let project = if !form.repo_path.trim().is_empty() {
                    let r = client.call("project.add", json!({"repoPath": form.repo_path.trim()})).map_err(|e| e.message)?;
                    r["project"]["id"].as_str().unwrap_or("").to_string()
                } else { form.project_id.clone() };
                if project.is_empty() { return Err(tr("Elige un proyecto")); }
                let title = form.title.trim();
                if title.is_empty() { return Err(tr("La misión necesita un título")); }
                let quest = client.call("quest.create", json!({"projectId": project, "title": title})).map_err(|e| e.message)?;
                let quest_id = quest["id"].as_str().unwrap_or("").to_string();
                let mut previous: Option<String> = None;
                for (t, instructions, dep) in QuestForm::parse_tasks(&form.tasks) {
                    let depends: Vec<String> = if dep { previous.iter().cloned().collect() } else { vec![] };
                    let r = client.call("task.create", json!({"questId": quest_id, "title": t, "instructions": instructions, "dependsOn": depends})).map_err(|e| e.message)?;
                    previous = r["id"].as_str().map(str::to_string);
                }
                if start { client.call("quest.start", json!({"questId": quest_id})).map_err(|e| e.message)?; }
                Ok(title.to_string())
            })();
            let _ = outbox.send(match result {
                Ok(title) => Msg::Note(trf("Misión creada: {}", &[&title]), "info"),
                Err(e) => Msg::Note(trf("Guild: {}", &[&e]), "blocked"),
            });
            wake();
        });
    }

    /// Start the service next to this binary, in the background.
    pub fn start_service(&mut self) {
        let outbox = self.outbox.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(format!("agents-pixel-dungeon-guild{}", std::env::consts::EXE_SUFFIX))));
            let binary = exe.filter(|p| p.exists()).or_else(|| guild::platform::which("agents-pixel-dungeon-guild"));
            let Some(binary) = binary else {
                let _ = outbox.send(Msg::Note(tr("No encuentro agents-pixel-dungeon-guild junto a la app"), "blocked"));
                wake();
                return;
            };
            let output = guild::platform::command(binary).args(["serve", "--daemon"]).output();
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

    /// The guild as one double room: the orchestrator at the desk, the
    /// members around it at the stations of what they do. The user only
    /// talks to the orchestrator; the members work below.
    pub fn agents(&self) -> Vec<Agent> {
        let Some(snap) = &self.snapshot else { return vec![] };
        hall_agent(snap).into_iter().collect()
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

/// The most pressing of some rooms' states: asking, then working, then a
/// candidate ready, then free.
fn most_pressing<'a>(states: impl Iterator<Item = &'a str>) -> &'static str {
    let states: Vec<&str> = states.collect();
    ["blocked", "working", "done", "unknown"].into_iter().find(|s| states.contains(s)).unwrap_or("idle")
}

fn hall_agent(snap: &Snapshot) -> Option<Agent> {
    let members: Vec<&Member> = snap.members.iter().filter(|m| m.enabled).collect();
    let lead = snap.orchestrator.as_ref().and_then(|id| members.iter().find(|m| &m.id == id)).or_else(|| members.first()).copied()?;
    let rooms: Vec<(&Member, Agent)> = members.iter().map(|m| (*m, member_agent(snap, m))).collect();
    // Whoever needs the user most leads the other half: its hero carries the alert.
    let urgency = |s: &str| ["blocked", "done", "working", "unknown"].iter().position(|x| *x == s).unwrap_or(4);
    let mut crew: Vec<&(&Member, Agent)> = rooms.iter().filter(|(m, _)| m.id != lead.id).collect();
    crew.sort_by_key(|(_, a)| urgency(&a.status));
    let project = snap.projects.first();
    let cwd = project.map(|p| p.repo_path.clone()).unwrap_or_default();
    let name = project.map(|p| p.name.clone()).unwrap_or_else(|| "Guild".into());
    let mine = rooms.iter().find(|(m, _)| m.id == lead.id).map(|(_, a)| a.clone());
    let working = rooms.iter().filter(|(_, a)| a.status == "working").count();
    let activity = if snap.orchestrator_busy {
        trf("{} piensa…", &[&lead.name])
    } else if let Some(asking) = rooms.iter().find(|(_, a)| a.status == "blocked") {
        asking.1.activity.clone()
    } else if working > 0 {
        trf("Habla con {} · {} trabajando", &[&lead.name, &working.to_string()])
    } else {
        trf("Habla con {}", &[&lead.name])
    };
    let status = if snap.orchestrator_busy { "working" } else { most_pressing(rooms.iter().map(|(_, a)| a.status.as_str())) };
    // A training hall: Claude leads from the dais (it orchestrates), and
    // every member walks to the station of what it does.
    let mut hall = Agent::new(HALL_ID, "claude", status, &name, &activity, &cwd);
    let _ = (&mine, &crew);
    hall.crew = rooms.iter().map(|(m, a)| crate::herdr::CrewMember { name: m.name.clone(), harness: m.harness.as_str().to_string(), status: a.status.clone(), action: a.action.clone() }).collect();
    Some(hall)
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
        "guild.founded" => (trf("Guild fundada: {} miembros", &[&p["members"]]), "info"),
        "design.updated" => (trf("Diseño guardado (v{})", &[&p["version"]]), "info"),
        "design.exported" => (trf("Diseño exportado a {}", &[&p["path"].as_str().unwrap_or("")]), "done"),
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
/// How long the attempt has run (or ran), for the panel.
fn elapsed_label(a: &Attempt) -> Option<String> {
    let start = chrono::DateTime::parse_from_rfc3339(a.started_at.as_deref()?).ok()?;
    let end = a.finished_at.as_deref().and_then(|f| chrono::DateTime::parse_from_rfc3339(f).ok()).map(|f| f.with_timezone(&chrono::Utc)).unwrap_or_else(chrono::Utc::now);
    let secs = (end - start.with_timezone(&chrono::Utc)).num_seconds().max(0);
    Some(if secs < 60 { format!("{secs} s") } else if secs < 3600 { format!("{} min", secs / 60) } else { format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60) })
}

/// The last two components of a path, for the transcript.
fn short_path(p: &str) -> String {
    let parts: Vec<&str> = p.rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

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

/// The hall's panel: the chat with the orchestrator, what needs the user,
/// and the crew below. A member's own work opens in place, with a way back.
pub fn hall_panel(ui: &mut Ui, view: &mut GuildView, now: f64, wide: bool) -> PanelAction {
    let mut action = PanelAction::default();
    let Some(snap) = view.snapshot.clone() else {
        ui.label(mono(&tr("Servicio de la guild apagado"), 12.0).color(DIM));
        return action;
    };
    if let Some(id) = view.hall_member.clone() {
        if let Some(member) = snap.members.iter().find(|m| m.id == id) {
            if ui.add(egui::Button::new(mono(&tr("← Volver a la sala"), 11.0).color(INK)).frame(false)).clicked() { view.hall_member = None; return action; }
            let inner = panel(ui, view, &member_agent(&snap, member), now, wide);
            if inner.close { view.hall_member = None; }
            action.terminal = inner.terminal;
            return action;
        }
        view.hall_member = None;
    }
    view.watch(None);
    let lead = snap.orchestrator.as_ref().and_then(|id| snap.members.iter().find(|m| &m.id == id));
    let lead_name = lead.map(|m| m.name.clone()).unwrap_or_else(|| tr("nadie"));
    ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
    ui.horizontal(|ui| {
        ui.label(mono(&snap.projects.first().map(|p| p.name.clone()).unwrap_or_else(|| "Guild".into()), 13.0).strong().color(INK));
        ui.label(mono(&trf("hablas con {}", &[&lead_name]), 11.0).color(DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(mono("x", 11.0).strong().color(INK)).frame(false)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
        });
    });
    // What only the user can decide: a permission, a candidate to accept.
    let members: Vec<&Member> = snap.members.iter().filter(|m| m.enabled).collect();
    let crew: Vec<(&Member, Agent)> = members.iter().map(|m| (*m, member_agent(&snap, m))).collect();
    let asking: Vec<&(&Member, Agent)> = crew.iter().filter(|(_, a)| a.status == "blocked" || a.status == "done").collect();
    if !asking.is_empty() {
        ui.label(mono(&tr("Necesita tu atención"), 10.5).strong().color(Color32::from_rgb(254, 174, 52)));
        for (m, a) in asking {
            let row = ui.add(egui::Label::new(mono(&format!("▸ {}", a.activity), 10.5).color(INK)).truncate().sense(egui::Sense::click()));
            if row.on_hover_text(tr("Ver su trabajo")).clicked() { view.hall_member = Some(m.id.clone()); }
        }
    }
    // The crew, below: one chip each, its state's colour; a click shows its work.
    ui.horizontal_wrapped(|ui| {
        ui.label(mono(&tr("Bajo el capó:"), 10.5).color(DIM));
        for (m, a) in &crew {
            let color = match a.status.as_str() { "working" => Color32::from_rgb(254, 231, 97), "blocked" => Color32::from_rgb(228, 59, 68), "done" => Color32::from_rgb(99, 199, 77), _ => DIM };
            let chip = ui.add(egui::Label::new(mono(&format!("● {}", m.name), 10.5).color(color)).sense(egui::Sense::click()));
            if chip.on_hover_text(&a.activity).clicked() { view.hall_member = Some(m.id.clone()); }
        }
    });
    ui.separator();
    // The conversation, newest at the bottom; the composer under it.
    let composer_h = 64.0;
    egui::ScrollArea::vertical().id_salt("guild-hall-chat").stick_to_bottom(true).auto_shrink([false, false]).max_height((ui.available_height() - composer_h).max(60.0)).show(ui, |ui| {
        if snap.conversation.is_empty() {
            ui.label(mono(&trf("Dile a {} qué quieres: lo reparte entre la guild.", &[&lead_name]), 11.0).color(DIM));
        }
        for line in &snap.conversation {
            let mine = line.author == "user";
            let who = if mine { tr("Tú") } else { lead_name.clone() };
            ui.label(mono(&who, 10.5).strong().color(if mine { Color32::from_rgb(44, 232, 245) } else { Color32::from_rgb(254, 174, 52) }));
            ui.label(mono(&line.text, 11.5).color(INK));
            if let Some(q) = line.quest_id.as_ref().and_then(|id| snap.quests.iter().find(|q| &q.id == id)) {
                let tasks: Vec<&Task> = snap.tasks.iter().filter(|t| t.quest_id == q.id).collect();
                let done = tasks.iter().filter(|t| t.state == TaskState::Completed).count();
                ui.label(mono(&trf("misión: {} · {}/{} tareas", &[&q.title, &done.to_string(), &tasks.len().to_string()]), 10.0).color(DIM));
            }
            ui.add_space(4.0);
        }
        if snap.orchestrator_busy { ui.label(mono(&trf("{} piensa…", &[&lead_name]), 11.0).italics().color(DIM)); }
    });
    let busy = snap.orchestrator_busy || lead.is_none();
    let mut send = false;
    ui.horizontal(|ui| {
        let edit = egui::TextEdit::multiline(&mut view.draft).desired_rows(2).desired_width(ui.available_width() - 60.0).hint_text(trf("Escribe a {}…", &[&lead_name])).font(egui::FontId::monospace(crate::ui::fs(11.5)));
        let response = ui.add_enabled(!busy, edit);
        if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) { send = true; }
        if ui.add_enabled(!busy && !view.draft.trim().is_empty(), egui::Button::new(mono(&tr("Enviar"), 11.0))).clicked() { send = true; }
    });
    if send && !busy {
        let text = view.draft.trim().to_string();
        if !text.is_empty() {
            view.command("orchestrator.say", json!({"text": text, "projectId": snap.projects.first().map(|p| p.id.clone())}));
            view.draft.clear();
        }
    }
    action
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
        if let Some(elapsed) = elapsed_label(&a) { ui.label(mono(&elapsed, 11.0).color(DIM)); }
        if a.resumed > 0 { ui.label(mono(&trf("reanudado ×{}", &[&a.resumed]), 10.5).color(DIM)); }
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
                if matches!(s, AttemptState::Running | AttemptState::WaitingInput) {
                    if button(ui, &tr("Interrumpir"), Color32::from_rgb(255, 159, 10)) { view.command("attempt.interrupt", json!({"attemptId": a.id})); }
                }
                if button(ui, &tr("Cancelar intento"), Color32::from_rgb(200, 200, 205)) { view.command("attempt.cancel", json!({"attemptId": a.id})); }
            }
            (_, Some(TaskState::Failed)) | (_, Some(TaskState::Blocked)) => {
                if let Some(t) = &task { if button(ui, &tr("Reintentar"), Color32::from_rgb(255, 159, 10)) { view.command("task.retry", json!({"taskId": t.id})); } }
            }
            _ => {}
        }
    });
    // The candidate's diff, or the transcript.
    let diff_id = a.candidate.as_ref().and_then(|c| c.diff_artifact_id.clone());
    if let Some(id) = &diff_id {
        ui.horizontal(|ui| {
            let on = view.diff_open;
            if ui.add(egui::Button::new(mono("± Diff", 11.0).strong().color(if on { Color32::from_rgb(20, 18, 26) } else { INK })).fill(if on { Color32::from_rgb(140, 217, 115) } else { Color32::TRANSPARENT }).corner_radius(CornerRadius::same(3)).stroke(Stroke::new(1.0_f32, DIM))).clicked() {
                view.diff_open = !view.diff_open;
            }
        });
        if view.diff_open { view.fetch_artifact(id); }
    } else {
        view.diff_open = false;
    }
    ui.add_space(2.0);
    let height = ui.available_height().max(60.0);
    if view.diff_open {
        let text = view.diff.as_ref().filter(|d| Some(&d.0) == diff_id.as_ref()).map(|d| d.1.clone()).unwrap_or_default();
        egui::ScrollArea::both().id_salt(("guild-diff", &a.id)).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
            if text.is_empty() { ui.label(mono("…", 11.0).color(DIM)); }
            let size = if wide { 11.0 } else { 10.0 };
            for line in text.lines() {
                let color = match line.chars().next() {
                    Some('+') if !line.starts_with("+++") => Color32::from_rgb(140, 217, 115),
                    Some('-') if !line.starts_with("---") => Color32::from_rgb(255, 120, 110),
                    Some('@') => Color32::from_rgb(120, 170, 255),
                    _ if line.starts_with("diff ") => INK,
                    _ => DIM,
                };
                ui.label(mono(line, size).color(color));
            }
        });
        return action;
    }
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
                "file.changed" => { for f in p["paths"].as_array().into_iter().flatten().filter_map(Value::as_str) { ui.label(mono(&format!("± {}", short_path(f)), size).color(Color32::from_rgb(140, 217, 115))); } }
                "session.resumed" => { ui.label(mono(&tr("Sesión reanudada tras un reinicio"), size).color(DIM)); }
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
    /// Open the guild window.
    pub studio: bool,
}

pub fn board(ctx: &Context, view: &mut GuildView) -> BoardAction {
    let ink = Color32::from_rgb(61, 38, 20);
    let faint = Color32::from_rgb(120, 90, 60);
    let mut action = BoardAction::default();
    let mut commands: Vec<(&'static str, Value)> = vec![];
    let mut start_service = false;
    let mut create: Option<bool> = None;
    egui::Area::new(egui::Id::new("quest-board")).anchor(Align2::LEFT_TOP, vec2(8.0, 38.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        Frame::new()
            .fill(Color32::from_rgb(237, 217, 168))
            .stroke(Stroke::new(2.0_f32, Color32::from_rgb(115, 74, 36)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(10))
            .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_rgba_unmultiplied(0, 0, 0, 128) })
            .show(ui, |ui| {
                crate::guild_studio::parchment(ui);
                ui.set_width((ctx.screen_rect().width() - 40.0).clamp(160.0, 340.0));
                ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                ui.horizontal(|ui| {
                    ui.label(mono(&tr("Tablón de misiones"), 12.0).strong().color(ink));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(mono("x", 11.0).strong().color(ink)).frame(false)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
                    });
                });
                let Some(snap) = view.snapshot.clone() else {
                    ui.label(mono(&tr("Servicio de la guild apagado"), 11.0).color(faint));
                    if button(ui, &tr("Arrancar el servicio"), Color32::from_rgb(255, 159, 10)) { start_service = true; }
                    return;
                };
                // Members, and the form to add one.
                ui.horizontal_wrapped(|ui| {
                    ui.label(mono(&tr("Miembros:"), 10.5).strong().color(ink));
                    if snap.members.is_empty() { ui.label(mono(&tr("ninguno"), 10.5).color(faint)); }
                    for (i, m) in snap.members.iter().enumerate() {
                        if i > 0 { ui.label(mono("·", 10.5).color(faint)); }
                        let text = format!("{} ({} · {})", m.name, m.harness, m.model_id.clone().unwrap_or_else(|| tr("modelo por defecto")));
                        if ui.add(egui::Label::new(mono(&text, 10.5).color(ink)).sense(egui::Sense::click())).on_hover_text(m.roles.join(", ")).clicked() {
                            action.open = Some(GuildView::member_agent_id(&m.id));
                        }
                    }
                    if small(ui, if view.form.members_open { "−" } else { "+" }, ink) { view.form.members_open = !view.form.members_open; }
                });
                if view.form.members_open {
                    let harnesses: Vec<String> = snap.harnesses.iter().filter(|h| h.available).map(|h| h.kind.to_string()).collect();
                    if view.form.member_harness.is_empty() { view.form.member_harness = harnesses.first().cloned().unwrap_or_else(|| "kiro".into()); }
                    if view.form.member_role.is_empty() { view.form.member_role = "implementer".into(); }
                    ui.horizontal_wrapped(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut view.form.member_name).hint_text(tr("nombre")).desired_width(70.0).font(egui::FontId::monospace(crate::ui::fs(10.5))));
                        egui::ComboBox::from_id_salt("member-harness").selected_text(mono(&view.form.member_harness, 10.5)).width(70.0).show_ui(ui, |ui| {
                            for h in &harnesses { ui.selectable_value(&mut view.form.member_harness, h.clone(), mono(h, 10.5)); }
                        });
                        ui.add(egui::TextEdit::singleline(&mut view.form.member_model).hint_text(tr("modelo")).desired_width(110.0).font(egui::FontId::monospace(crate::ui::fs(10.5))));
                        egui::ComboBox::from_id_salt("member-role").selected_text(mono(&view.form.member_role, 10.5)).width(90.0).show_ui(ui, |ui| {
                            for r in ["implementer", "reviewer", "planner"] { ui.selectable_value(&mut view.form.member_role, r.to_string(), mono(r, 10.5)); }
                        });
                        let name = view.form.member_name.trim().to_string();
                        if !name.is_empty() && small(ui, &tr("Añadir miembro"), ink) {
                            let id: String = name.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
                            let model = view.form.member_model.trim();
                            commands.push(("member.add", json!({"id": id, "name": name, "harness": view.form.member_harness, "roles": [view.form.member_role], "model": if model.is_empty() { Value::Null } else { json!(model) }})));
                            view.form.member_name.clear();
                        }
                    });
                }
                // The form for a new quest.
                ui.horizontal(|ui| {
                    if small(ui, &tr("Nueva misión"), ink) { view.form.open = !view.form.open; }
                    if small(ui, &tr("Gremio…"), ink) { action.studio = true; }
                    if snap.quests.is_empty() { ui.label(mono(&tr("Sin misiones todavía"), 11.0).color(faint)); }
                });
                if view.form.open {
                    Frame::new().fill(Color32::from_rgb(229, 206, 152)).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(6)).show(ui, |ui| {
                        ui.spacing_mut().item_spacing = vec2(5.0, 4.0);
                        ui.horizontal(|ui| {
                            ui.label(mono(&tr("Proyecto"), 10.5).color(ink));
                            if snap.projects.is_empty() {
                                ui.add(egui::TextEdit::singleline(&mut view.form.repo_path).hint_text(tr("ruta del repositorio")).desired_width(220.0).font(egui::FontId::monospace(crate::ui::fs(10.5))));
                            } else {
                                if view.form.project_id.is_empty() { view.form.project_id = snap.projects[0].id.clone(); }
                                let shown = snap.projects.iter().find(|p| p.id == view.form.project_id).map(|p| p.name.clone()).unwrap_or_default();
                                egui::ComboBox::from_id_salt("quest-project").selected_text(mono(&shown, 10.5)).width(150.0).show_ui(ui, |ui| {
                                    for p in &snap.projects { ui.selectable_value(&mut view.form.project_id, p.id.clone(), mono(&p.name, 10.5)); }
                                });
                                ui.add(egui::TextEdit::singleline(&mut view.form.repo_path).hint_text(tr("u otra ruta")).desired_width(90.0).font(egui::FontId::monospace(crate::ui::fs(10.5))));
                            }
                        });
                        ui.add(egui::TextEdit::singleline(&mut view.form.title).hint_text(tr("Título de la misión")).desired_width(f32::INFINITY).font(egui::FontId::monospace(crate::ui::fs(11.0))));
                        ui.add(egui::TextEdit::multiline(&mut view.form.tasks).hint_text(tr("Una tarea por línea: Título :: instrucciones\n> depende de la tarea anterior")).desired_rows(4).desired_width(f32::INFINITY).font(egui::FontId::monospace(crate::ui::fs(10.5))));
                        let tasks = QuestForm::parse_tasks(&view.form.tasks);
                        ui.horizontal(|ui| {
                            let ready = !view.form.title.trim().is_empty() && !tasks.is_empty() && (!view.form.project_id.is_empty() || !view.form.repo_path.trim().is_empty());
                            ui.label(mono(&trf("{} tareas", &[&tasks.len()]), 10.0).color(faint));
                            if ready {
                                if small(ui, &tr("Crear e iniciar"), ink) { create = Some(true); }
                                if small(ui, &tr("Crear"), ink) { create = Some(false); }
                            }
                            if small(ui, &tr("Cancelar"), ink) { view.form.open = false; }
                        });
                    });
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
                                let mut who = member.map(|m| format!(" · {}", m.name)).unwrap_or_default();
                                if t.candidates > 1 {
                                    let submitted = snap.attempts.iter().filter(|a| a.task_id == t.id && matches!(a.state, AttemptState::Submitted | AttemptState::Passed)).count();
                                    let winner = t.winner_attempt_id.as_ref().and_then(|w| snap.attempts.iter().find(|a| &a.id == w)).and_then(|a| snap.members.iter().find(|m| m.id == a.member_id)).map(|m| m.name.clone());
                                    who = match winner {
                                        Some(w) => trf(" · carrera: ganó {}", &[&w]),
                                        None => trf(" · carrera {}/{}", &[&submitted, &t.candidates]),
                                    };
                                }
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
    if let Some(start) = create { view.form.open = false; view.create_quest(start); }
    if start_service { view.start_service(); }
    action
}

fn small(ui: &mut Ui, text: &str, ink: Color32) -> bool {
    ui.add(egui::Button::new(mono(text, 10.0).strong().color(ink)).fill(Color32::from_rgb(222, 196, 140)).stroke(Stroke::new(1.0_f32, Color32::from_rgb(160, 120, 70))).corner_radius(CornerRadius::same(3))).clicked()
}

// ---- Demo

fn demo_hypotheses(t: &str) -> Vec<guild::experiment::Hypothesis> {
    vec![guild::experiment::Hypothesis { id: "h1".into(), project_id: "verbrawl".into(), question_id: Some("Q-001".into()), problem: tr("Los jugadores siempre eligen la palabra más larga"), evidence: "OBS-002 · max_attribute:length 81%".into(), level: guild::experiment::DesignLevel::Loop,
        hypothesis: tr("Subir la sinergia semántica hace competitivas las palabras cortas temáticas"), expected_effect: tr("más diversidad de palabras"), proposed_change: "semantic_multiplier 1.2 → 1.6".into(), measurements: vec!["choices.top_share".into(), "max_attribute:length".into()], related_systems: vec!["combat".into()], status: guild::experiment::HypothesisStatus::Experimenting, created_at: t.into(), updated_at: t.into() }]
}

fn demo_experiments(t: &str) -> Vec<guild::experiment::Experiment> {
    use guild::experiment::*;
    let variant = |id: &str, name: &str, l: f64, s: f64| Variant { id: id.into(), name: name.into(), params: [("length_multiplier".to_string(), json!(l)), ("semantic_multiplier".to_string(), json!(s))].into_iter().collect(), instructions: String::new(), task_id: Some(format!("t-{id}")), attempt_id: Some(format!("a-{id}")) };
    let metrics = json!({
        "control": {"name": "control", "params": {}, "profiles": {"greedy": {"playtests": 2, "runs": {"completion_rate": 1.0}, "choices": {"top": "whirlwind", "top_share": 0.42}}, "explorer": {"playtests": 2, "runs": {"completion_rate": 0.5}, "choices": {"top": "ice", "top_share": 0.3}}, "human": {"playtests": 1, "runs": {"completion_rate": 1.0}, "choices": {"top": "inferno", "top_share": 0.5}}}},
        "a": {"name": "semantic 1.6", "params": {}, "profiles": {"greedy": {"playtests": 2, "runs": {"completion_rate": 1.0}, "choices": {"top": "whirlwind", "top_share": 0.38}}, "explorer": {"playtests": 2, "runs": {"completion_rate": 1.0}, "choices": {"top": "chill", "top_share": 0.25}}}},
        "b": {"name": "flat length", "params": {}, "profiles": {"greedy": {"playtests": 2, "runs": {"completion_rate": 0.5}, "choices": {"top": "whirlwind", "top_share": 0.4}}, "explorer": {"playtests": 2, "runs": {"completion_rate": 1.0}, "choices": {"top": "spark", "top_share": 0.2}}, "human": {"playtests": 1, "runs": {"completion_rate": 1.0}, "choices": {"top": "chill", "top_share": 0.25}}}}
    });
    vec![Experiment { id: "e1".into(), project_id: "verbrawl".into(), hypothesis_id: "h1".into(), title: tr("Bono semántico"), status: ExperimentStatus::Decided, quest_id: None,
        variants: vec![variant("control", "control", 1.35, 1.2), variant("a", "semantic 1.6", 1.35, 1.6), variant("b", "flat length", 1.0, 1.6)],
        plan: PlaytestPlan { profiles: vec!["greedy".into(), "explorer".into()], runs_per_profile: 2, seeds: vec![] }, analysis_task_id: None,
        result: Some(ExperimentResult { verdict: "supports".into(), inputs: vec![], evidence: vec![tr("explorer completa más runs en a y b"), tr("en b la palabra más larga deja de ganar siempre")], unexpected: vec![tr("greedy pierde la mitad de sus runs en b")], recommended_variant: Some("a".into()), recommended_next: tr("probar semantic 1.8 con length 1.2"), confidence: Some(0.6), analyst_attempt_id: None, metrics, created_at: t.into() }),
        decision: None, failure: None, created_at: t.into(), updated_at: t.into() }]
}

fn demo_playtests(t: &str) -> Vec<guild::experiment::Playtest> {
    use guild::experiment::*;
    let mut out = vec![];
    for (i, (v, profile, kind)) in [("control", "greedy", PlaytestKind::Simulation), ("a", "greedy", PlaytestKind::Simulation), ("b", "explorer", PlaytestKind::Simulation), ("control", "human", PlaytestKind::Human), ("b", "human", PlaytestKind::Human)].iter().enumerate() {
        out.push(Playtest { id: format!("pt{i}"), project_id: "verbrawl".into(), experiment_id: Some("e1".into()), variant_id: Some(v.to_string()), kind: *kind, profile: profile.to_string(), status: "finished".into(), event_count: 40, created_at: t.into(), ..Default::default() });
    }
    out.push(Playtest { id: "pt-ai1".into(), project_id: "verbrawl".into(), kind: PlaytestKind::AiPlayer, profile: "ai:player".into(), seed: Some(12), status: "finished".into(), player: Some("Orin".into()), build_sha: Some("490acd6f00".into()), event_count: 46,
        metrics: json!({"runs": {"started": 1, "completed": 1}, "ai": {"turns": 6, "fallbacks": 0, "findings": 0}}), observations: tr("Las palabras largas ganaban siempre; solo dudé cuando me quedaban pocas letras raras."), created_at: t.into(), ..Default::default() });
    out.push(Playtest { id: "pt-ai2".into(), project_id: "verbrawl".into(), kind: PlaytestKind::AiPlayer, profile: "ai:exploit_hunter".into(), seed: Some(7), status: "running".into(), player: Some("Lira".into()), build_sha: Some("490acd6f00".into()), created_at: t.into(), ..Default::default() });
    out.push(Playtest { id: "pt-live".into(), project_id: "verbrawl".into(), kind: PlaytestKind::Human, profile: "human".into(), status: "running".into(), player: Some("nacho".into()), build_sha: Some("490acd6f00".into()), design_version: Some(1), created_at: t.into(), ..Default::default() });
    out
}

fn demo_observations(t: &str) -> Vec<guild::experiment::Observation> {
    use guild::experiment::Observation;
    vec![
        Observation { id: "OBS-001".into(), project_id: "verbrawl".into(), kind: "great_moment".into(), text: tr("Ganar con «ice» sobre un enemigo de hielo se sintió genial"), playtest_id: Some("pt3".into()), run_id: Some("human-12".into()), round: Some(3), created_at: t.into(), ..Default::default() },
        Observation { id: "OBS-002".into(), project_id: "verbrawl".into(), kind: "frustrating".into(), text: tr("Siempre termino eligiendo la palabra más larga"), playtest_id: Some("pt3".into()), run_id: Some("human-12".into()), round: Some(5), question_id: Some("Q-001".into()), created_at: t.into(), ..Default::default() },
        Observation { id: "OBS-003".into(), project_id: "verbrawl".into(), kind: "confusing".into(), text: tr("No entiendo por qué esta palabra hizo tanto daño"), playtest_id: Some("pt-live".into()), run_id: Some("human-31".into()), round: Some(2), created_at: t.into(), ..Default::default() },
    ]
}

fn demo_findings(t: &str) -> Vec<guild::aiplay::Finding> {
    use guild::aiplay::Finding;
    vec![
        Finding { id: "F-001".into(), project_id: "verbrawl".into(), title: tr("La palabra más larga siempre gana"), impact: tr("elegir palabra deja de importar"), severity: "high".into(), playtest_id: Some("pt-ai2".into()), member_id: Some("lira".into()), build_sha: Some("490acd6f00".into()), seed: Some(7), actions: vec!["3".into(), "1".into(), "5".into()], event_index: 14, round: Some(3), status: "reproduced".into(), verification: Some("Replayed 3 moves with seed 7 on 490acd6f: the first 14 events match.".into()), created_at: t.into(), ..Default::default() },
        Finding { id: "F-002".into(), project_id: "verbrawl".into(), title: tr("Las letras raras se recargan gratis cada ronda"), impact: tr("gastar letras raras no cuesta nada"), severity: "medium".into(), playtest_id: Some("pt-ai2".into()), member_id: Some("lira".into()), build_sha: Some("490acd6f00".into()), seed: Some(7), actions: vec!["3".into(), "1".into(), "5".into(), "2".into(), "2".into()], event_index: 22, round: Some(5), status: "unverified".into(), created_at: t.into(), ..Default::default() },
    ]
}

/// What the demo answers to a request, so the chamber's screens fill.
fn demo_result(method: &str) -> Option<Value> {
    match method {
        "signals" => Some(json!({
            "playtests": 14, "human": 2, "synthetic": 12,
            "desired": ["consider several words before committing", "take risks on rare letters"],
            "avoid": ["a dominant strategy", "always choosing the longest word"],
            "signals": [
                {"key": "max_attribute:length", "statement": "The chosen option had the highest length in 81% of 96 choices (about 24% if chosen at random)", "value": 0.81, "n": 96, "baseline": 0.24, "suggested_question": "Does the highest length always win?", "playtests": 14},
                {"key": "never_chosen", "statement": "Offered at least 5 times and never chosen: ash, ice, ore", "value": 3.0, "n": 96, "suggested_question": "Why does nobody choose ash, ice, ore?", "playtests": 14}
            ]
        })),
        _ => None,
    }
}

fn demo_snapshot(tick: usize) -> Snapshot {
    let t = now();
    let member = |id: &str, name: &str, harness: HarnessKind, model: &str, role: &str| Member { id: id.into(), name: name.into(), roles: vec![role.into()], harness, model_id: Some(model.into()), mode: None, role_prompt: String::new(), enabled: true };
    let task = |id: &str, title: &str, state: TaskState, deps: &[&str]| Task {
        id: id.into(), quest_id: "q1".into(), title: tr(title), instructions: String::new(), acceptance: vec![], depends_on: deps.iter().map(|s| s.to_string()).collect(), role: "implementer".into(),
        member_id: None, model_id: None, priority: 0, state, block_reason: None, attempts_made: 1, max_attempts: 2, strategy: ExecutionStrategy::default(), candidates: 1, auto_integrate: false, winner_attempt_id: None, version: 1, created_at: t.clone(), updated_at: t.clone(),
    };
    let attempt = |id: &str, task: &str, member: &str, harness: HarnessKind, model: &str, state: AttemptState| Attempt {
        id: id.into(), task_id: task.into(), task_version: 1, quest_id: "q1".into(), project_id: "verbrawl".into(), member_id: member.into(), harness, model_requested: Some(model.into()), model_effective: Some(model.into()),
        kind: AttemptKind::Work, state, workspace_id: Some(format!("w-{id}")), session_id: None, generation: 1, candidate: None, summary: String::new(), action: None, pending_permission: None, failure: None, verifier_results: vec![], turns: 1, resumed: 0,
        started_at: Some(t.clone()), finished_at: None, created_at: t.clone(), updated_at: t.clone(),
    };
    let workspace = |id: &str, branch: &str| Workspace { id: format!("w-{id}"), project_id: "verbrawl".into(), attempt_id: id.into(), path: format!("~/demo/verbrawl/worktrees/{id}"), branch: branch.into(), base_sha: "490acd6".into(), state: WorkspaceState::Active, created_at: t.clone(), updated_at: t.clone() };
    let mut a2 = attempt("a2", "t2", "aria", HarnessId::named("kiro"), "claude-sonnet-5.5", AttemptState::Running);
    a2.action = Some(["forge", "read", "brew", "type"][(tick / 3) % 4].into());
    a2.summary = tr("Añadiendo el tick de veneno al combate");
    let mut a3 = attempt("a3", "t3", "orin", HarnessId::named("codex"), "gpt-5.5", AttemptState::WaitingInput);
    a3.pending_permission = Some(PermissionRequest { request_id: "perm-1".into(), title: "Run Command".into(), detail: "pnpm test --run".into(), options: vec![
        PermissionOption { id: "accept".into(), name: "Accept".into(), kind: "allow_once".into() },
        PermissionOption { id: "reject".into(), name: "Reject".into(), kind: "reject_once".into() },
    ], asked_at: t.clone() });
    let mut a4 = attempt("a4", "t4", "lira", HarnessId::named("claude"), "claude-opus-5.5", AttemptState::Submitted);
    a4.candidate = Some(Candidate { base_sha: "490acd6".into(), head_sha: "dcdbee6".into(), tree_sha: "e3b0c44".into(), files_changed: 3, insertions: 48, deletions: 6, diff_artifact_id: Some("art-demo".into()) });
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
        projects: vec![Project { id: "verbrawl".into(), name: "Verbrawl".into(), repo_path: "~/demo/verbrawl".into(), target_ref: "refs/heads/main".into(), setup: vec![], verifiers: vec![], playtest: None, play: Some(PlaytestRunner { argv: vec!["python3".into(), "wordfight.py".into(), "profile=human".into(), "seed={seed}".into()], timeout_seconds: 300 }), ai_play: Some(PlaytestRunner { argv: vec!["python3".into(), "wordfight.py".into(), "profile=agent".into(), "seed={seed}".into()], timeout_seconds: 300 }), created_at: t.clone() }],
        decisions: vec![],
        hypotheses: demo_hypotheses(&t),
        questions: vec![guild::design::DesignQuestion { id: "Q-001".into(), project_id: "verbrawl".into(), question: tr("¿Las palabras largas son demasiado dominantes?"), context: "OBS-002".into(), system_id: Some("combat".into()), status: "open".into(), raised_by: "playtest".into(), source: Some("OBS-002".into()), hypothesis_ids: vec!["h1".into()], experiment_ids: vec!["e1".into()], decision_id: None, answer: String::new(), created_at: t.clone(), resolved_at: None }],
        design_decisions: vec![],
        experiments: demo_experiments(&t),
        playtests: demo_playtests(&t),
        observations: demo_observations(&t),
        findings: demo_findings(&t),
        designs: vec![guild::design::template("verbrawl", "Verbrawl", "videogame")],
        roles: guild::config::role_catalog(),
        quests: vec![Quest { id: "q1".into(), project_id: "verbrawl".into(), title: tr("Efectos de estado"), description: String::new(), state: QuestState::Running, integration_ref: "refs/heads/guild/quest/q1".into(), base_sha: Some("490acd6".into()), plan_version: 1, created_at: t.clone(), updated_at: t.clone() }],
        tasks: vec![
            task("t1", "Contrato de datos", TaskState::Completed, &[]),
            task("t2", "Lógica de combate", TaskState::Running, &["t1"]),
            task("t3", "Interfaz", TaskState::Running, &["t1"]),
            task("t4", "Pruebas de aceptación", TaskState::Verifying, &["t1"]),
            task("t5", "Integración y QA visual", TaskState::Pending, &["t2", "t3", "t4"]),
        ],
        members: vec![
            member("aria", "Aria", HarnessId::named("kiro"), "claude-sonnet-5.5", "implementer"),
            member("orin", "Orin", HarnessId::named("codex"), "gpt-5.5", "implementer"),
            member("lira", "Lira", HarnessId::named("claude"), "claude-opus-5.5", "tester"),
            member("kael", "Kael", HarnessId::named("claude"), "claude-opus-5.5", "planner"),
        ],
        attempts: vec![a2, a3, a4],
        workspaces: vec![workspace("a2", "guild/q1/t2/a2"), workspace("a3", "guild/q1/t3/a3"), workspace("a4", "guild/q1/t4/a4")],
        harnesses: vec![],
        conversation: vec![
            guild::orchestrator::ChatLine { author: "user".into(), text: tr("Quiero efectos de estado en el combate: veneno y escudo."), quest_id: None, at: t.clone() },
            guild::orchestrator::ChatLine { author: "orchestrator".into(), text: tr("Lo armé en cinco tareas: el contrato de datos primero, luego combate, interfaz y pruebas en paralelo, y al final integración con QA visual."), quest_id: Some("q1".into()), at: t.clone() },
        ],
        orchestrator: Some("kael".into()),
        orchestrator_busy: false,
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
