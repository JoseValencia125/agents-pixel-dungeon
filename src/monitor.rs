//! Keeps the agents current: a snapshot to bootstrap and reconcile, Herdr's
//! event stream for changes the moment they happen, local enrichment every
//! second, and the actions the chat panel and the summon sheet run on panes.
//! Background work reports back through a channel the UI drains each frame.

use crate::alerts::{alerts_for, AgentAlert};
use crate::events::{HerdrStream, Requester, StreamMessage};
use crate::herdr::*;
use crate::l10n::{tr, trf};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One line of the activity log. `tone` picks its marker: an agent state
/// (working, blocked, idle, done), "joined", "left", "subagents", "message"
/// or "info".
#[derive(Clone, Debug)]
pub struct GuildEvent {
    pub at: chrono::DateTime<chrono::Local>,
    pub text: String,
    pub tone: &'static str,
}

impl GuildEvent {
    pub fn new(text: String, tone: &'static str) -> GuildEvent {
        GuildEvent { at: chrono::Local::now(), text, tone }
    }
}

type Fresh = Option<(Vec<Agent>, HashMap<String, String>)>;

/// Whether Herdr can be used, as of the last refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HerdrState {
    Unknown,
    /// No `herdr` binary: the standalone viewer shows what runs in terminals.
    Missing,
    /// Installed, but its server does not answer: standalone meanwhile.
    Down,
    Up,
}

/// The link to a saved SSH machine, as of its last snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Up,
    /// The last attempt failed, with Herdr's reason.
    Down(String),
}

/// A saved SSH machine shown in the dungeon: its agents from its last good
/// snapshot, and how its link is doing. Each machine is read with `herdr
/// --machine <id> api snapshot`, backing off while it is unreachable. Herdr
/// serves no event stream over `--machine`, so one is held open over SSH
/// (see `events`): each event reads the machine again at once, and while
/// it is open the snapshots are only a reconciliation.
pub struct MachineView {
    pub machine: Machine,
    pub link: Link,
    pub agents: Vec<Agent>,
    pub last_ok: Option<Instant>,
    next_poll: Instant,
    busy: bool,
    /// The snapshot in flight: only its own answer counts, so one from
    /// before a reset or a hide is dropped and never frees a newer one.
    request: u64,
    failures: u32,
    /// Something changed while a snapshot was in flight: read again after it.
    dirty: bool,
    stream: Option<HerdrStream>,
    stream_panes: Vec<String>,
    /// The event stream is open (its subscription confirmed).
    live: bool,
    next_stream_try: Instant,
    stream_failures: u32,
}

impl MachineView {
    fn new(machine: Machine) -> MachineView {
        let now = Instant::now();
        MachineView {
            machine, link: Link::Connecting, agents: vec![], last_ok: None, next_poll: now, busy: false, request: 0, failures: 0,
            dirty: false, stream: None, stream_panes: vec![], live: false, next_stream_try: now, stream_failures: 0,
        }
    }

    /// Its event stream is open: changes are read the moment they happen.
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// Read the machine as soon as it is free.
    fn poke(&mut self) {
        if self.busy { self.dirty = true; } else { self.next_poll = Instant::now(); }
    }
}

/// The base of the back-off while a machine is unreachable, and its
/// longest wait between attempts (5, 10, 20, 40, then 60 s).
const REMOTE_EVERY: Duration = Duration::from_secs(5);
const REMOTE_BACKOFF_MAX: Duration = Duration::from_secs(60);
/// How often a reachable machine is read: often without its event stream,
/// only to reconcile with it.
const REMOTE_POLL: Duration = Duration::from_secs(2);
const REMOTE_RECONCILE: Duration = Duration::from_secs(20);
/// The longest wait before an event stream that dropped is tried again (a
/// machine without python3 never opens one and is polled instead).
const REMOTE_STREAM_BACKOFF_MAX: Duration = Duration::from_secs(120);
/// Herdr's saved machines are listed from its local file this often.
const MACHINES_EVERY: Duration = Duration::from_secs(30);

pub fn remote_backoff(failures: u32) -> Duration {
    (REMOTE_EVERY * 2u32.saturating_pow(failures.saturating_sub(1).min(6))).min(REMOTE_BACKOFF_MAX)
}

enum Inbox {
    Refresh { generation: u64, result: Result<(Fresh, Vec<Agent>), MonitorError>, herdr: HerdrState },
    Socket { generation: u64, path: Option<String>, panes: Vec<String>, was_up: bool },
    Stream(u64, StreamMessage),
    Sessions(Vec<HerdrSession>),
    Machines(Option<Vec<SavedMachine>>),
    Remote { request: u64, machine: String, result: Result<Vec<Agent>, MonitorError> },
    RemoteStream { machine: String, generation: u64, message: StreamMessage },
    /// An action reached this machine: read it again now.
    Poke(String),
    Log(GuildEvent),
    Created { failure: Option<String>, waiting: Option<PendingPrompt>, log: Option<GuildEvent> },
}

struct PendingPrompt {
    /// The new agent's id in the dungeon, and the server its pane is on.
    id: String,
    target: HerdrTarget,
    name: String,
    project: String,
    text: String,
    tries: u32,
    seen: bool,
    next: Instant,
}

pub struct Monitor {
    pub agents: Vec<Agent>,
    pub error: Option<String>,
    pub updated: Option<Instant>,
    pub events: Vec<GuildEvent>,
    pub demo: bool,
    /// HUD state chip, machine and project scope, and search box.
    pub filter: StatusFilter,
    pub scope: Scope,
    pub query: String,
    /// The room the user clicked; its chat panel is open while set.
    pub selected: Option<String>,
    /// The "new agent" panel is open.
    pub composing: bool,
    /// The Herdr session watched.
    pub session: String,
    /// Herdr's sessions, for the menu (refreshed with each snapshot).
    pub sessions: Vec<HerdrSession>,
    /// Alerts since the last drain: sounds and notifications are the app's.
    pub alerts: Vec<AgentAlert>,
    /// Something the window chrome reacts to changed (agents, panels, filter).
    pub changed: bool,
    /// Herdr's availability; while not Up the rooms come from the standalone scan.
    pub herdr: HerdrState,
    next_herdr_try: Instant,
    /// The machines saved in Herdr (for the menu), and those shown in the
    /// dungeon: enabled in Herdr and not hidden here.
    pub saved_machines: Vec<SavedMachine>,
    pub machines: Vec<MachineView>,
    hidden_machines: Vec<String>,
    next_machine_list: Instant,
    listing_machines: bool,
    requests: u64,
    // The local agents last shown, and whether the first local refresh is in:
    // machines are read only after it, so the guild starts with its own rooms.
    local: Vec<Agent>,
    local_done: bool,
    published: bool,

    inbox: Receiver<Inbox>,
    outbox: Sender<Inbox>,
    wake: Arc<dyn Fn() + Send + Sync>,
    busy: bool,
    again: bool,
    next_refresh: Instant,
    tick: usize,
    generation: u64,
    // Herdr's view of the agents before local enrichment: from a snapshot,
    // then kept current by the event stream.
    base: Vec<Agent>,
    names: HashMap<String, String>,
    last_snapshot: Option<Instant>,
    needs_snapshot: bool,
    stream: Option<HerdrStream>,
    stream_panes: Vec<String>,
    stream_up: bool,
    next_stream_try: Instant,
    socket_path: Option<String>,
    connecting: bool,
    streams_made: u64,
    pending_prompts: Vec<PendingPrompt>,
}

/// Snapshots double as a reconciliation while the stream is live.
const RECONCILE_EVERY: Duration = Duration::from_secs(60);

impl Monitor {
    pub fn new(session: String, filter: StatusFilter) -> Monitor {
        let (outbox, inbox) = channel();
        Monitor {
            agents: vec![],
            error: None,
            updated: None,
            events: vec![],
            demo: false,
            filter,
            query: String::new(),
            selected: None,
            composing: false,
            session,
            sessions: vec![],
            alerts: vec![],
            changed: false,
            herdr: HerdrState::Unknown,
            next_herdr_try: Instant::now(),
            scope: Scope::default(),
            saved_machines: vec![],
            machines: vec![],
            hidden_machines: vec![],
            next_machine_list: Instant::now(),
            listing_machines: false,
            requests: 0,
            local: vec![],
            local_done: false,
            published: false,
            inbox,
            outbox,
            wake: Arc::new(|| {}),
            busy: false,
            again: false,
            next_refresh: Instant::now(),
            tick: 0,
            generation: 0,
            base: vec![],
            names: HashMap::new(),
            last_snapshot: None,
            needs_snapshot: true,
            stream: None,
            stream_panes: vec![],
            stream_up: false,
            next_stream_try: Instant::now(),
            socket_path: None,
            connecting: false,
            streams_made: 0,
            pending_prompts: vec![],
        }
    }

    /// Called by background threads when they have something for `poll`.
    pub fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.wake = wake;
    }

    pub fn visible_agents(&self) -> Vec<Agent> {
        filter_agents(&self.agents, self.filter, &self.query).into_iter().filter(|a| self.scope.admits(a)).collect()
    }

    pub fn is_filtering(&self) -> bool {
        self.filter != StatusFilter::All || !self.scope.is_all() || !self.query.trim().is_empty()
    }

    /// Without Herdr the rooms are read-only: nothing can be typed into an agent.
    pub fn standalone(&self) -> bool {
        !self.demo && self.herdr != HerdrState::Up
    }

    /// Whether nothing can be sent to this agent: a local one without the
    /// local Herdr, a remote one while its machine is unreachable.
    pub fn read_only(&self, agent: &Agent) -> bool {
        match &agent.machine {
            Some(machine) => matches!(self.link_of(&machine.id), Some(Link::Down(_)) | None),
            None => self.standalone(),
        }
    }

    pub fn link_of(&self, machine: &str) -> Option<&Link> {
        self.machines.iter().find(|v| v.machine.id == machine).map(|v| &v.link)
    }

    /// Rooms whose machine is unreachable: shown as last seen, faded.
    pub fn stale_ids(&self) -> HashSet<String> {
        self.machines.iter().filter(|v| matches!(v.link, Link::Down(_))).flat_map(|v| v.agents.iter().map(|a| a.id.clone())).collect()
    }

    /// The HUD shows once there are enough rooms to sift, while a filter
    /// hides some, or while machines are shown (for their scope menu).
    pub fn shows_hud(&self) -> bool {
        self.agents.len() >= 3 || self.is_filtering() || !self.machines.is_empty()
    }

    /// What the banner says while shown machines are out of reach: the
    /// first one, how long since its last good snapshot, and how many more.
    pub fn machines_note(&self, now: Instant) -> Option<String> {
        let down: Vec<&MachineView> = self.machines.iter().filter(|v| matches!(v.link, Link::Down(_))).collect();
        let first = down.first()?;
        let mut text = trf("{} sin conexión", &[&first.machine.label]);
        if let Some(last) = first.last_ok {
            text += &format!(" · {}", trf("última actualización {}", &[&ago(now.saturating_duration_since(last).as_secs_f64())]));
        }
        if down.len() > 1 { text += &format!(" · +{}", down.len() - 1); }
        Some(text)
    }

    /// Read every shown machine on the next poll instead of waiting out its back-off.
    pub fn retry_machines(&mut self) {
        let now = Instant::now();
        for view in &mut self.machines { view.next_poll = now; }
    }

    pub fn set_scope(&mut self, scope: Scope) {
        if self.scope != scope { self.scope = scope; self.changed = true; }
    }

    /// Machines (profile ids) not to show; the others Herdr has enabled are.
    pub fn set_hidden_machines(&mut self, hidden: Vec<String>) {
        if self.hidden_machines == hidden { return; }
        self.hidden_machines = hidden;
        let saved = self.saved_machines.clone();
        self.apply_machines(saved);
    }

    /// Keep one view per machine that is enabled in Herdr and not hidden:
    /// new ones start connecting, dropped ones leave with their rooms, and a
    /// renamed one keeps its rooms under the new label.
    fn apply_machines(&mut self, saved: Vec<SavedMachine>) {
        let wanted: Vec<Machine> = saved.iter().filter(|m| m.enabled && !self.hidden_machines.contains(&m.machine.id)).map(|m| m.machine.clone()).collect();
        if self.saved_machines != saved { self.saved_machines = saved; self.changed = true; }
        let gone: Vec<Machine> = self.machines.iter().filter(|v| !wanted.iter().any(|m| m.id == v.machine.id)).map(|v| v.machine.clone()).collect();
        self.machines.retain(|v| wanted.iter().any(|m| m.id == v.machine.id));
        let mut touched = !gone.is_empty();
        // One line for a machine that leaves, not one per room.
        for machine in &gone { self.log(trf("{} salió de la guild", &[&machine.label]), "left"); }
        for machine in wanted {
            match self.machines.iter_mut().find(|v| v.machine.id == machine.id) {
                Some(view) if view.machine.label != machine.label => {
                    for agent in &mut view.agents { agent.machine = Some(machine.clone()); }
                    view.machine = machine;
                    touched = true;
                }
                Some(_) => {}
                None => { self.machines.push(MachineView::new(machine)); touched = true; }
            }
        }
        if let Some(scope) = &self.scope.machine {
            if scope != LOCAL && !self.machines.iter().any(|v| &v.machine.id == scope) { self.scope.machine = None; }
        }
        if touched { self.changed = true; self.publish(&gone.iter().map(|m| m.id.clone()).collect::<Vec<_>>()); }
    }

    pub fn select(&mut self, id: Option<String>) {
        if self.selected != id {
            if id.is_some() { self.composing = false; }
            self.selected = id;
            self.changed = true;
        }
    }

    pub fn set_composing(&mut self, on: bool) {
        if self.composing != on {
            if on { self.selected = None; }
            self.composing = on;
            self.changed = true;
        }
    }

    pub fn set_filter(&mut self, filter: StatusFilter) {
        if self.filter != filter { self.filter = filter; self.changed = true; }
    }

    pub fn set_query(&mut self, query: String) {
        if self.query != query { self.query = query; self.changed = true; }
    }

    pub fn set_demo(&mut self, enabled: bool) {
        self.generation += 1;
        self.demo = enabled;
        // Demo agents are all on this computer: a machine scope would hide them.
        if enabled { self.scope.machine = None; }
        self.reset();
    }

    /// Watch another Herdr session, starting over with its agents.
    pub fn set_session(&mut self, name: &str) {
        if name == self.session { return; }
        self.generation += 1;
        self.session = name.to_string();
        self.reset();
    }

    /// Drop the event stream and Herdr's cached view: the next refresh
    /// starts over with a snapshot.
    fn drop_stream(&mut self) {
        if let Some(stream) = self.stream.take() { stream.stop(); }
        self.stream_panes.clear();
        self.stream_up = false;
        self.connecting = false;
        self.base.clear();
        self.names.clear();
        self.last_snapshot = None;
        self.needs_snapshot = true;
        self.next_stream_try = Instant::now();
        self.socket_path = None;
    }

    fn reset(&mut self) {
        self.drop_stream();
        self.machines.clear();
        self.local.clear();
        self.local_done = false;
        self.published = false;
        self.next_machine_list = Instant::now();
        self.agents.clear();
        self.events.clear();
        self.updated = None;
        self.error = None;
        self.selected = None;
        self.pending_prompts.clear();
        self.changed = true;
        self.refresh();
    }

    pub fn log(&mut self, text: String, tone: &'static str) {
        self.events.insert(0, GuildEvent::new(text, tone));
        self.events.truncate(40);
    }

    /// Show the local agents (Herdr's, the standalone scan's or the demo's),
    /// with the remote machines' beside them.
    pub fn apply(&mut self, next: Vec<Agent>) {
        self.local = next;
        self.publish(&[]);
        self.updated = Some(Instant::now());
        self.error = None;
    }

    /// One remote machine's snapshot: its rooms are replaced as a whole, so
    /// a reconnection never leaves a duplicate or a ghost behind.
    pub fn apply_remote(&mut self, machine: &str, result: Result<Vec<Agent>, MonitorError>) {
        let Some(index) = self.machines.iter().position(|v| v.machine.id == machine) else { return };
        let now = Instant::now();
        let view = &mut self.machines[index];
        view.busy = false;
        let label = view.machine.label.clone();
        match result {
            Ok(agents) => {
                let first = view.last_ok.is_none();
                let was_down = matches!(view.link, Link::Down(_));
                view.link = Link::Up;
                view.failures = 0;
                view.last_ok = Some(now);
                view.next_poll = if view.dirty { now } else { now + if view.live { REMOTE_RECONCILE } else { REMOTE_POLL } };
                view.dirty = false;
                let count = agents.len();
                view.agents = agents;
                self.changed = true;
                if first {
                    self.log(trf("{} conectada · {} agentes", &[&label, &count]), "info");
                } else if was_down {
                    self.log(trf("{} reconectada", &[&label]), "info");
                }
                let quiet = if first { vec![machine.to_string()] } else { vec![] };
                self.publish(&quiet);
            }
            Err(error) => {
                view.failures += 1;
                view.next_poll = now + remote_backoff(view.failures);
                let text = error.text();
                let news = !matches!(view.link, Link::Down(_));
                if view.link != Link::Down(text.clone()) { view.link = Link::Down(text.clone()); self.changed = true; }
                if news { self.log(trf("{} sin conexión: {}", &[&label, &text]), "left"); }
            }
        }
    }

    /// Show the local and the remote agents together, logging and alerting
    /// what changed. A machine that just connected or left (`quiet`) brings
    /// or takes its rooms without one line each.
    fn publish(&mut self, quiet: &[String]) {
        let loud = |agent: &Agent| agent.machine.as_ref().map(|m| !quiet.contains(&m.id)).unwrap_or(true);
        let mut next = self.local.clone();
        if !self.demo {
            for view in &self.machines { next.extend(view.agents.iter().cloned()); }
        }
        // Sort last: background jobs and subagents can change a status after
        // Herdr's, and whoever needs attention must lead the grid.
        let next = sort_agents(next);
        let previous: HashMap<String, Agent> = self.agents.iter().map(|a| (a.id.clone(), a.clone())).collect();
        let ids: Vec<String> = next.iter().map(|a| a.id.clone()).collect();
        if self.published {
            let alerts = alerts_for(&previous, &next);
            self.alerts.extend(alerts);
        }
        if !self.published {
            self.published = true;
            self.log(trf("Guild conectada · {} agentes", &[&next.len()]), "info");
        } else {
            for agent in &next {
                if let Some(old) = previous.get(&agent.id) {
                    if old.status != agent.status {
                        let tone = match agent.status.as_str() { "working" => "working", "blocked" => "blocked", "limited" => "limited", "idle" => "idle", "done" => "done", _ => "info" };
                        self.log(format!("{} · {}", agent.project, agent.label()), tone);
                    }
                    if agent.subagents > old.subagents {
                        self.log(trf("{} · {} subagentes activos", &[&agent.project, &agent.subagents]), "subagents");
                    }
                } else if loud(agent) {
                    self.log(trf("{} entró a la guild", &[&agent.project]), "joined");
                }
            }
            let gone: Vec<String> = self.agents.iter().filter(|a| !ids.contains(&a.id) && loud(a)).map(|a| a.project.clone()).collect();
            for project in gone { self.log(trf("{} salió de la guild", &[&project]), "left"); }
        }
        if self.agents != next { self.changed = true; }
        self.agents = next;
        if let Some(selected) = &self.selected {
            if !ids.contains(selected) { self.selected = None; self.changed = true; }
        }
    }

    /// Drain what the background threads sent and run the timers. Returns
    /// true when something visible may have changed.
    pub fn poll(&mut self) -> bool {
        let mut touched = false;
        while let Ok(message) = self.inbox.try_recv() {
            touched = true;
            match message {
                Inbox::Refresh { generation, result, herdr } => {
                    self.busy = false;
                    if generation != self.generation { self.refresh(); continue; }
                    self.local_done = true;
                    if herdr != self.herdr { self.herdr = herdr; self.changed = true; }
                    if herdr == HerdrState::Down || herdr == HerdrState::Missing {
                        // Try Herdr again in a few seconds; the scan carries on meanwhile.
                        self.next_herdr_try = Instant::now() + Duration::from_secs(if herdr == HerdrState::Missing { 10 } else { 4 });
                    }
                    match result {
                        Ok((fresh, shown)) => {
                            if let Some((agents, names)) = fresh {
                                self.base = agents;
                                self.names = names;
                                self.last_snapshot = Some(Instant::now());
                                self.needs_snapshot = false;
                            }
                            let demo = self.demo;
                            self.apply(shown);
                            if !demo && herdr == HerdrState::Up { self.keep_stream(); }
                        }
                        Err(error) => {
                            let text = error.text();
                            if self.error.as_deref() != Some(&text) { self.changed = true; }
                            self.error = Some(text);
                            self.needs_snapshot = true;
                        }
                    }
                    if self.again { self.again = false; self.refresh(); }
                }
                Inbox::Socket { generation, path, panes, was_up } => {
                    self.connecting = false;
                    self.socket_path = path.clone();
                    if generation != self.generation || self.stream.is_some() { continue; }
                    let Some(path) = path else {
                        self.stream_up = false;
                        self.next_stream_try = Instant::now() + Duration::from_secs(5);
                        continue;
                    };
                    self.streams_made += 1;
                    let stream = HerdrStream::new(&path, &panes, self.streams_made);
                    let sender = self.outbox.clone();
                    let wake = self.wake.clone();
                    let (tx, rx) = channel::<(u64, StreamMessage)>();
                    // Relay the stream's messages into the inbox, waking the UI.
                    std::thread::spawn(move || {
                        while let Ok((generation, message)) = rx.recv() {
                            if sender.send(Inbox::Stream(generation, message)).is_err() { break; }
                            wake();
                        }
                    });
                    stream.start(tx);
                    self.stream = Some(stream);
                    self.stream_panes = panes;
                    if !was_up { self.needs_snapshot = true; }
                }
                Inbox::Stream(generation, message) => {
                    if self.stream.as_ref().map(|s| s.generation) != Some(generation) { continue; }
                    match message {
                        StreamMessage::Open => {
                            self.stream_up = true;
                            // Anything that changed while (re)subscribing is caught by a snapshot.
                            self.needs_snapshot = true;
                        }
                        StreamMessage::Event(event) => self.handle(&event),
                        StreamMessage::Close => {
                            self.stream = None;
                            self.stream_panes.clear();
                            self.stream_up = false;
                            self.needs_snapshot = true;
                            self.socket_path = None; // look it up again: the server may have moved
                            self.next_stream_try = Instant::now() + Duration::from_secs(3);
                        }
                    }
                }
                Inbox::Sessions(sessions) => {
                    if self.sessions != sessions { self.sessions = sessions; self.changed = true; }
                }
                Inbox::Machines(saved) => {
                    self.listing_machines = false;
                    self.next_machine_list = Instant::now() + MACHINES_EVERY;
                    // Herdr could not list them: keep the machines as they are.
                    if let (Some(saved), false) = (saved, self.demo) { self.apply_machines(saved); }
                }
                Inbox::Remote { request, machine, result } => {
                    if !self.machines.iter().any(|v| v.machine.id == machine && v.busy && v.request == request) { continue; }
                    self.apply_remote(&machine, result);
                }
                Inbox::Poke(machine) => {
                    if let Some(view) = self.machines.iter_mut().find(|v| v.machine.id == machine) { view.poke(); }
                }
                Inbox::RemoteStream { machine, generation, message } => {
                    let Some(view) = self.machines.iter_mut().find(|v| v.machine.id == machine) else { continue };
                    if view.stream.as_ref().map(|s| s.generation) != Some(generation) { continue; }
                    match message {
                        // Read it now: anything may have changed while subscribing.
                        StreamMessage::Open => { view.live = true; view.stream_failures = 0; view.poke(); }
                        StreamMessage::Event(_) => view.poke(),
                        StreamMessage::Close => {
                            view.stream = None;
                            view.stream_panes.clear();
                            view.live = false;
                            view.stream_failures += 1;
                            view.next_stream_try = Instant::now() + (REMOTE_EVERY * 2u32.saturating_pow(view.stream_failures.saturating_sub(1).min(6))).min(REMOTE_STREAM_BACKOFF_MAX);
                            view.poke();
                        }
                    }
                }
                Inbox::Log(event) => { self.events.insert(0, event); self.events.truncate(40); }
                Inbox::Created { failure, waiting, log } => {
                    if let Some(log) = log { self.events.insert(0, log); self.events.truncate(40); }
                    if let Some(pending) = waiting {
                        self.log(trf("{} espera tu respuesta; su prompt se enviará cuando esté listo.", &[&pending.project]), "blocked");
                        // Its chat opens once its room is there (a machine's
                        // shows up with its next snapshot); then a prompt waits.
                        self.pending_prompts.push(pending);
                    }
                    let _ = failure;
                    self.refresh();
                }
            }
        }
        if Instant::now() >= self.next_refresh {
            self.next_refresh = Instant::now() + Duration::from_secs(1);
            self.refresh();
            self.check_pending_prompts();
        }
        self.poll_machines();
        if self.changed { touched = true; }
        touched
    }

    /// Every second: enrich Herdr's view from local files and show it.
    /// Herdr itself is read with a snapshot only to bootstrap, after a
    /// structural event, once a minute to reconcile, or every time while the
    /// event stream is down (plain polling, as a fallback).
    pub fn refresh(&mut self) {
        if self.busy { self.again = true; return; }
        self.busy = true;
        let generation = self.generation;
        let demo = self.demo;
        let tick = self.tick;
        let session = self.session.clone();
        let reconcile = self.last_snapshot.map(|t| t.elapsed() > RECONCILE_EVERY).unwrap_or(true);
        let snapshot = !demo && (self.needs_snapshot || !self.stream_up || reconcile);
        let herdr_now = self.herdr;
        let try_herdr = herdr_now == HerdrState::Up || herdr_now == HerdrState::Unknown || Instant::now() >= self.next_herdr_try;
        let known = self.base.clone();
        self.tick += 1;
        let sender = self.outbox.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let mut herdr = herdr_now;
            let result = (|| {
                if demo { return Ok((None, demo_agents(tick))); }
                let installed = herdr_binary().is_ok();
                if !installed {
                    herdr = HerdrState::Missing;
                } else if herdr == HerdrState::Up && !snapshot {
                    return Ok((None, enrich(known)));
                } else if try_herdr {
                    match run_herdr(&["api", "snapshot"], &session, Duration::from_secs(4)) {
                        Ok(data) => {
                            let (agents, names) = decode_snapshot_parts(&data)?;
                            let shown = enrich(agents.clone());
                            // The sessions list rides along with each snapshot, for the menu.
                            let _ = sender.send(Inbox::Sessions(list_sessions()));
                            herdr = HerdrState::Up;
                            return Ok((Some((agents, names)), shown));
                        }
                        Err(error) => {
                            // Herdr answered but with something broken: say so. Unreachable: standalone.
                            if error.code().is_some() || herdr == HerdrState::Up { return Err(error); }
                            herdr = HerdrState::Down;
                        }
                    }
                }
                // Standalone: the agents running in any terminal, from their processes and files.
                Ok((None, enrich(crate::standalone::agents())))
            })();
            let _ = sender.send(Inbox::Refresh { generation, result, herdr });
            wake();
        });
    }

    /// List Herdr's saved machines now and then, and read each shown machine
    /// when its turn comes: one snapshot in flight per machine, never in
    /// demo mode, and only once the local agents are in.
    fn poll_machines(&mut self) {
        if self.demo || !self.local_done { return; }
        self.keep_remote_streams();
        let now = Instant::now();
        let due = (!self.listing_machines && now >= self.next_machine_list) || self.machines.iter().any(|v| !v.busy && now >= v.next_poll);
        if !due || herdr_binary().is_err() { return; }
        if !self.listing_machines && now >= self.next_machine_list {
            self.listing_machines = true;
            let sender = self.outbox.clone();
            let wake = self.wake.clone();
            std::thread::spawn(move || {
                let _ = sender.send(Inbox::Machines(list_machines()));
                wake();
            });
        }
        for view in self.machines.iter_mut().filter(|v| !v.busy && now >= v.next_poll) {
            self.requests += 1;
            let request = self.requests;
            view.busy = true;
            view.request = request;
            let machine = view.machine.clone();
            let sender = self.outbox.clone();
            let wake = self.wake.clone();
            std::thread::spawn(move || {
                let result = run_herdr_on(&["api", "snapshot"], &HerdrTarget::Machine(machine.id.clone()), Duration::from_secs(10))
                    .and_then(|data| decode_remote_snapshot(&data, &machine));
                let _ = sender.send(Inbox::Remote { request, machine: machine.id, result });
                wake();
            });
        }
    }

    /// Keep an event stream open on each reachable machine's agent panes,
    /// reopening it when they change; none while a machine is out of reach.
    fn keep_remote_streams(&mut self) {
        let now = Instant::now();
        for view in &mut self.machines {
            if view.link != Link::Up || view.machine.target.is_empty() {
                if view.stream.take().is_some() { view.stream_panes.clear(); view.live = false; }
                continue;
            }
            let mut panes: Vec<String> = view.agents.iter().map(|a| a.pane.clone()).collect();
            panes.sort();
            if (view.stream.is_some() && panes == view.stream_panes) || now < view.next_stream_try { continue; }
            self.streams_made += 1;
            let stream = HerdrStream::remote(&view.machine.target, &view.machine.session, &panes, self.streams_made);
            let (tx, rx) = channel::<(u64, StreamMessage)>();
            let sender = self.outbox.clone();
            let wake = self.wake.clone();
            let machine = view.machine.id.clone();
            std::thread::spawn(move || {
                while let Ok((generation, message)) = rx.recv() {
                    if sender.send(Inbox::RemoteStream { machine: machine.clone(), generation, message }).is_err() { break; }
                    wake();
                }
            });
            stream.start(tx);
            // The one it replaces stops as it drops.
            view.stream = Some(stream);
            view.stream_panes = panes;
            view.live = false;
        }
    }

    /// Keep one event stream open on the current agent panes, reopening it
    /// when they change and retrying a few seconds after it drops.
    fn keep_stream(&mut self) {
        let mut panes: Vec<String> = self.base.iter().map(|a| a.id.clone()).collect();
        panes.sort();
        if self.connecting || (self.stream.is_some() && panes == self.stream_panes) || Instant::now() < self.next_stream_try { return; }
        self.connecting = true;
        if let Some(stream) = self.stream.take() { stream.stop(); }
        self.stream_panes = panes.clone();
        let session = self.session.clone();
        let generation = self.generation;
        let was_up = self.stream_up;
        let cached = self.socket_path.clone();
        let sender = self.outbox.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let path = cached.or_else(|| herdr_socket(&session));
            let _ = sender.send(Inbox::Socket { generation, path, panes, was_up });
            wake();
        });
    }

    /// An event from Herdr: apply it and show it right away, or ask for a snapshot.
    fn handle(&mut self, event: &Value) {
        match apply_herdr_event(event, &mut self.base, &self.names) {
            EventEffect::None => {}
            EventEffect::Changed => self.refresh(),
            EventEffect::Resync => { self.needs_snapshot = true; self.refresh(); }
        }
    }

    // ---- Actions on an agent's pane

    /// Run herdr commands on the agent's own server (its machine, or the
    /// local session) off the UI thread; the receiver gets None or an error
    /// message. A remote machine is read again right after.
    fn act(&self, agent: &Agent, commands: Vec<Vec<String>>) -> Receiver<Option<String>> {
        let (tx, rx) = channel();
        let target = agent.target(&self.session);
        let demo = self.demo;
        let wake = self.wake.clone();
        let poke = self.outbox.clone();
        std::thread::spawn(move || {
            let failure = if demo { None } else { run_all(&target, &commands) };
            if let (HerdrTarget::Machine(id), None) = (&target, &failure) { let _ = poke.send(Inbox::Poke(id.clone())); }
            let _ = tx.send(failure);
            wake();
        });
        rx
    }

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// Type a message into the agent's chat and press Enter. Herdr refuses
    /// `agent prompt` while a permission prompt is open, so a blocked agent
    /// gets the raw keystrokes instead (e.g. an answer to its question).
    /// `erase` backspaces in the agent's input go first: a terminal that put
    /// an interrupted message back there would otherwise get the edited one
    /// after it.
    pub fn send_erasing(&self, erase: usize, text: &str, agent: &Agent) -> Receiver<Option<String>> {
        let message = text.trim().to_string();
        let (tx, rx) = channel();
        if message.is_empty() { let _ = tx.send(None); return rx; }
        let inner = if agent.status == "blocked" {
            self.act(agent, vec![Monitor::args(&["pane", "send-text", &agent.pane, &message]), Monitor::args(&["pane", "send-keys", &agent.pane, "enter"])])
        } else if erase > 0 {
            let mut keys = Monitor::args(&["agent", "send-keys", &agent.pane]);
            keys.extend(std::iter::repeat_n("backspace".to_string(), erase));
            let erased = self.act(agent, vec![keys]);
            let (inner_tx, inner_rx) = channel();
            let (server, demo, pane, text) = (agent.target(&self.session), self.demo, agent.pane.clone(), message.clone());
            std::thread::spawn(move || {
                let failure = erased.recv().unwrap_or(None);
                let _ = inner_tx.send(if failure.is_some() || demo { failure } else { prompt_failure(&server, &pane, &text) });
            });
            inner_rx
        } else {
            self.prompt(agent.target(&self.session), &agent.pane, &message)
        };
        let log = self.outbox.clone();
        let project = agent.project.clone();
        std::thread::spawn(move || {
            let failure = inner.recv().unwrap_or(None);
            if failure.is_none() {
                let _ = log.send(Inbox::Log(GuildEvent::new(trf("Tú → {}: {}", &[&project, &message]), "message")));
            }
            let _ = tx.send(failure);
        });
        rx
    }

    /// Press keys in the agent's pane: "enter" accepts a permission prompt's
    /// highlighted option, "esc" rejects it or interrupts the agent.
    pub fn press(&self, keys: &[String], agent: &Agent) -> Receiver<Option<String>> {
        let mut args = Monitor::args(&["agent", "send-keys", &agent.pane]);
        args.extend(keys.iter().cloned());
        self.act(agent, vec![args])
    }

    /// End the agent's session by typing /exit in its chat. A working or
    /// asking agent first gets esc, and a pause so the terminal does not read
    /// esc + "/" as Alt+/.
    pub fn finish(&self, agent: &Agent) -> Receiver<Option<String>> {
        let exit = vec![Monitor::args(&["pane", "send-text", &agent.pane, "/exit"]), Monitor::args(&["pane", "send-keys", &agent.pane, "enter"])];
        if agent.status != "working" && agent.status != "blocked" { return self.act(agent, exit); }
        let (tx, rx) = channel();
        let first = self.act(agent, vec![Monitor::args(&["agent", "send-keys", &agent.pane, "esc"])]);
        let target = agent.target(&self.session);
        let demo = self.demo;
        let poke = self.outbox.clone();
        std::thread::spawn(move || {
            if let Some(failure) = first.recv().unwrap_or(None) { let _ = tx.send(Some(failure)); return; }
            std::thread::sleep(Duration::from_millis(600));
            let failure = if demo { None } else { run_all(&target, &exit) };
            if let (HerdrTarget::Machine(id), None) = (&target, &failure) { let _ = poke.send(Inbox::Poke(id.clone())); }
            let _ = tx.send(failure);
        });
        rx
    }

    /// The agent's terminal as it shows, colours included (ANSI): the
    /// visible screen, or with `history` the last few hundred lines of its
    /// scrollback, to scroll through. Demo mode paints a plausible screen.
    pub fn screen(&self, agent: &Agent, history: bool) -> Receiver<String> {
        let (tx, rx) = channel();
        let target = agent.target(&self.session);
        let bridge = self.bridge(agent);
        let demo = self.demo;
        let agent = agent.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let text = if demo || agent.id.starts_with("pid:") {
                format!("\x1b[38;2;215;119;87m●\x1b[0m \x1b[1m{}\x1b[0m\n  \x1b[2m⎿ Leyendo archivos del proyecto…\x1b[0m\n\n\x1b[38;2;136;136;136m────────────────────────────\x1b[0m\n❯ \x1b[2mTry \"fix the failing test\"\x1b[0m", agent.activity)
            } else {
                let (source, lines) = if history { ("recent", "300") } else { ("visible", "80") };
                read_terminal(&agent, &target, bridge.as_ref(), source, lines, "ansi")
            };
            let _ = tx.send(text);
            wake();
        });
        rx
    }

    /// The open bridge to a remote agent's machine, for its terminal reads.
    fn bridge(&self, agent: &Agent) -> Option<Requester> {
        let machine = agent.machine.as_ref()?;
        let view = self.machines.iter().find(|v| v.machine.id == machine.id && v.live)?;
        view.stream.as_ref().map(|s| s.requester())
    }

    /// Whether a remote agent's terminal is read through its machine's open
    /// bridge (a round trip) rather than a `herdr --machine` each time.
    pub fn reads_fast(&self, agent: &Agent) -> bool {
        !agent.is_remote() || self.bridge(agent).is_some()
    }

    /// Bring the agent's pane to the front inside Herdr.
    pub fn focus(&self, agent: &Agent) -> Receiver<Option<String>> {
        self.act(agent, vec![Monitor::args(&["agent", "focus", &agent.pane])])
    }

    /// Submit a prompt with `herdr agent prompt`. Herdr only reports
    /// "stalled" or "unverifiable" when it cannot see the agent react within
    /// five seconds; that is not proof the text was lost, so it counts as sent.
    fn prompt(&self, server: HerdrTarget, target: &str, text: &str) -> Receiver<Option<String>> {
        let (tx, rx) = channel();
        let demo = self.demo;
        let (target, text) = (target.to_string(), text.to_string());
        let wake = self.wake.clone();
        let poke = self.outbox.clone();
        std::thread::spawn(move || {
            let failure = if demo { None } else { prompt_failure(&server, &target, &text) };
            if let (HerdrTarget::Machine(id), None) = (&server, &failure) { let _ = poke.send(Inbox::Poke(id.clone())); }
            let _ = tx.send(failure);
            wake();
        });
        rx
    }

    /// Start a new agent: a Herdr workspace in `folder` (its root pane is a
    /// fresh shell), `herdr agent start` of `kind` in it, then the first
    /// prompt if there is one. It shows up in the dungeon with the next
    /// snapshot. On a saved `machine` every step runs there and the folder is
    /// an absolute path on that machine (Herdr does not expand `~` there; it
    /// would fall back to the home folder). The receiver gets None or an
    /// error message.
    pub fn create(&self, kind: &str, folder: &str, prompt: &str, machine: Option<&Machine>) -> Receiver<Option<String>> {
        let (tx, rx) = channel();
        let server = match machine { Some(m) => HerdrTarget::Machine(m.id.clone()), None => HerdrTarget::Session(self.session.clone()) };
        let machine = machine.cloned();
        let demo = self.demo;
        let path = if machine.is_some() { folder.trim().trim_end_matches(['/', '\\']).to_string() } else { expand_home(folder) };
        // The folder's own name, whichever separator its machine uses.
        let label = path.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
        let name = agent_name(kind, &label);
        let text = prompt.trim().to_string();
        let kind = kind.to_string();
        let sender = self.outbox.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let mut waiting_pane: Option<String> = None; // started, but stopped at a startup question
            let failure: Option<String> = if demo {
                None
            } else {
                (|| -> Result<(), MonitorError> {
                    if machine.is_some() {
                        // Another computer's folder cannot be checked from here; Herdr says if it is wrong.
                        if !remote_folder_ok(&path) { return Err(MonitorError::Message(tr("Escribe una ruta absoluta de esa máquina."))); }
                    } else if !std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                        return Err(MonitorError::Message(tr("La carpeta no existe.")));
                    }
                    let created = run_herdr_on(&["workspace", "create", "--cwd", &path, "--label", &label, "--no-focus"], &server, Duration::from_secs(12))?;
                    let pane = root_pane(&created).ok_or_else(|| MonitorError::Message(tr("Herdr no devolvió el panel nuevo.")))?;
                    match start_agent(&server, &name, &kind, &pane) {
                        Ok(_) => {}
                        Err(e) if e.code() == Some("agent_not_ready") => waiting_pane = Some(pane.clone()),
                        Err(e) => {
                            // Nothing runs in the new workspace: close it instead of leaving an empty shell behind.
                            if let Some(workspace) = created_workspace(&created) {
                                let _ = run_herdr_on(&["workspace", "close", &workspace], &server, Duration::from_secs(8));
                            }
                            let why = e.text();
                            let base = trf("{} no arrancó. Revisa que esté instalado.", &[&kind]);
                            return Err(MonitorError::Message(if why.is_empty() { base } else { format!("{base} ({why})") }));
                        }
                    }
                    if waiting_pane.is_none() && !text.is_empty() {
                        match run_herdr_on(&["agent", "prompt", &name, &text], &server, Duration::from_secs(12)) {
                            Ok(_) => {}
                            Err(e) if matches!(e.code(), Some("agent_prompt_stalled" | "agent_prompt_unverifiable")) => {}
                            Err(e) if e.code() == Some("agent_blocked") => waiting_pane = Some(pane),
                            Err(_) => return Err(MonitorError::Message(tr("El agente arrancó, pero no recibió el prompt."))),
                        }
                    }
                    Ok(())
                })()
                .err()
                .map(|e| e.text())
            };
            let place = match &machine { Some(m) => format!("{label} · {}", m.label), None => label.clone() };
            let log = failure.is_none().then(|| GuildEvent::new(trf("Invocaste a {} en {}", &[&kind, &place]), "joined"));
            let waiting = waiting_pane.map(|pane| PendingPrompt {
                id: machine.as_ref().map(|m| remote_id(&m.id, &pane)).unwrap_or_else(|| pane.clone()),
                target: server.clone(),
                name: name.clone(),
                project: label.clone(),
                text: text.clone(),
                tries: 180,
                seen: false,
                next: Instant::now(),
            });
            if let Some(m) = &machine { let _ = sender.send(Inbox::Poke(m.id.clone())); }
            let _ = sender.send(Inbox::Created { failure: failure.clone(), waiting, log });
            let _ = tx.send(failure);
            wake();
        });
        rx
    }

    /// A new agent stopped at a startup question (trusting the folder, a
    /// login…): open its chat so it can be answered, and send the first
    /// prompt once it is idle. Gives up after three minutes.
    fn check_pending_prompts(&mut self) {
        let now = Instant::now();
        let mut keep = vec![];
        let mut logs: Vec<GuildEvent> = vec![];
        let mut select: Option<String> = None;
        let mut sends: Vec<(HerdrTarget, String, String, String)> = vec![];
        for mut pending in std::mem::take(&mut self.pending_prompts) {
            if now < pending.next { keep.push(pending); continue; }
            pending.next = now + Duration::from_secs(1);
            if pending.tries == 0 {
                if !pending.text.is_empty() { logs.push(GuildEvent::new(trf("{} no quedó listo; su prompt no se envió.", &[&pending.project]), "left")); }
                continue;
            }
            pending.tries -= 1;
            let status = self.agents.iter().find(|a| a.id == pending.id).map(|a| a.status.clone());
            if status.is_some() && !pending.seen { pending.seen = true; select = Some(pending.id.clone()); }
            if pending.text.is_empty() {
                if !pending.seen { keep.push(pending); }
                continue;
            }
            match status.as_deref() {
                Some("idle") | Some("done") => { sends.push((pending.target.clone(), pending.name.clone(), pending.text.clone(), pending.project.clone())); continue; }
                None if pending.seen => continue, // the pane went away
                _ => keep.push(pending),
            }
        }
        self.pending_prompts = keep;
        for log in logs { self.events.insert(0, log); }
        self.events.truncate(40);
        if let Some(pane) = select { self.select(Some(pane)); }
        for (server, name, text, project) in sends {
            let rx = self.prompt(server, &name, &text);
            let sender = self.outbox.clone();
            std::thread::spawn(move || {
                let failure = rx.recv().unwrap_or(None);
                let event = if failure.is_none() {
                    GuildEvent::new(trf("Tú → {}: {}", &[&project, &text]), "message")
                } else {
                    GuildEvent::new(trf("{} no recibió su prompt.", &[&project]), "left")
                };
                let _ = sender.send(Inbox::Log(event));
            });
        }
    }

    /// The last non-blank lines of the agent's terminal, so a question can be
    /// answered without switching windows.
    pub fn tail(&self, agent: &Agent, lines: usize) -> Receiver<Vec<String>> {
        let (tx, rx) = channel();
        let target = agent.target(&self.session);
        let bridge = self.bridge(agent);
        let demo = self.demo;
        let agent = agent.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            // Standalone (no Herdr): the agent's last reply, from its transcript.
            if agent.id.starts_with("pid:") {
                let text = agent.question_transcript.as_deref().and_then(|p| file_tail(p, 131_072)).and_then(|t| last_assistant_text(&t)).unwrap_or_default();
                let rows: Vec<String> = if agent.status == "blocked" { question_lines(&text, None) } else { text.lines().map(str::to_string).collect() };
                let start = rows.len().saturating_sub(lines);
                let _ = tx.send(rows[start..].to_vec());
                wake();
                return;
            }
            // A background job asking something: its whole question, options included.
            if !demo && (agent.status == "blocked" || agent.status == "limited") {
                if let Some(path) = &agent.question_transcript {
                    if let Some(message) = file_tail(path, 131_072).and_then(|t| last_assistant_text(&t)) {
                        let rows = question_lines(&message, agent.question.as_deref());
                        if !rows.is_empty() { let _ = tx.send(rows); wake(); return; }
                    }
                }
            }
            let text = if demo {
                if agent.status == "limited" {
                    "You've hit your session limit · resets 2:20pm".to_string()
                } else if agent.status == "blocked" {
                    "● Bash(rm -rf build && make)\n  Do you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again\n  3. No, and tell Claude what to do".to_string()
                } else {
                    format!("● {}\n  ⎿ {}", agent.activity, tr("Leyendo archivos del proyecto…"))
                }
            } else {
                read_terminal(&agent, &target, bridge.as_ref(), "recent", "60", "text")
            };
            let rows = meaningful_lines(&text);
            let keep = if agent.status == "blocked" { 12 } else { lines };
            let start = rows.len().saturating_sub(keep);
            let _ = tx.send(rows[start..].to_vec());
            wake();
        });
        rx
    }
}

/// `herdr agent start` in a pane that was created a moment ago. The pane's
/// shell takes a few hundred milliseconds to reach its prompt and Herdr
/// answers `agent_pane_busy` ("not an available shell") until then, so that
/// code is retried for up to ten seconds; any other answer is final.
fn start_agent(server: &HerdrTarget, name: &str, kind: &str, pane: &str) -> Result<Vec<u8>, MonitorError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match run_herdr_on(&["agent", "start", name, "--kind", kind, "--pane", pane, "--timeout", "60000"], server, Duration::from_secs(65)) {
            Err(e) if e.code() == Some("agent_pane_busy") && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            other => return other,
        }
    }
}

/// Run commands in order on one server; the first refusal stops them.
fn run_all(server: &HerdrTarget, commands: &[Vec<String>]) -> Option<String> {
    let timeout = Duration::from_secs(if matches!(server, HerdrTarget::Machine(_)) { 10 } else { 6 });
    for args in commands {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        if run_herdr_on(&refs, server, timeout).is_err() { return Some(tr("Herdr rechazó la acción. Revisa el panel del agente.")); }
    }
    None
}

/// A folder on a remote machine: an absolute path (`/srv/x`, or `C:\x` and
/// `\\host\x` on a Windows machine). `herdr --machine … workspace create
/// --cwd ~/x` does not expand `~` on the server.
pub fn remote_folder_ok(path: &str) -> bool {
    let drive = path.len() >= 3 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' && matches!(path.as_bytes()[2], b'\\' | b'/');
    path.starts_with('/') || path.starts_with("\\\\") || drive
}

fn prompt_failure(server: &HerdrTarget, target: &str, text: &str) -> Option<String> {
    let timeout = Duration::from_secs(if matches!(server, HerdrTarget::Machine(_)) { 12 } else { 10 });
    match run_herdr_on(&["agent", "prompt", target, text], server, timeout) {
        Ok(_) => None,
        Err(e) if matches!(e.code(), Some("agent_prompt_stalled" | "agent_prompt_unverifiable")) => None,
        Err(e) if e.code() == Some("agent_blocked") => Some(tr("El agente está esperando una respuesta; contéstale primero.")),
        Err(_) => Some(tr("Herdr rechazó la acción. Revisa el panel del agente.")),
    }
}

/// An agent's terminal text (`agent read`): through its machine's open
/// bridge when there is one, else with `herdr [--machine]`; empty when it
/// could not be read.
fn read_terminal(agent: &Agent, target: &HerdrTarget, bridge: Option<&Requester>, source: &str, lines: &str, format: &str) -> String {
    let params = serde_json::json!({ "target": agent.pane, "source": source, "lines": lines.parse::<u32>().ok(), "format": format, "strip_ansi": format != "ansi" });
    // Herdr's refusal (a pane that went away) is as good as the CLI's.
    if let Some(reply) = bridge.and_then(|b| b.request("agent.read", params)).and_then(|rx| rx.recv_timeout(Duration::from_secs(8)).ok()) {
        return reply.get("result").and_then(|r| r.get("read")).and_then(|r| r.get("text")).and_then(Value::as_str).unwrap_or_default().to_string();
    }
    run_herdr_on(&["agent", "read", &agent.pane, "--source", source, "--lines", lines, "--format", format], target, Duration::from_secs(if agent.is_remote() { 8 } else { 4 }))
        .map(|d| String::from_utf8_lossy(&d).to_string())
        .unwrap_or_default()
}
