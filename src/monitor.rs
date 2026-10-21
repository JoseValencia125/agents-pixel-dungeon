//! Keeps the agents current: a snapshot to bootstrap and reconcile, Herdr's
//! event stream for changes the moment they happen, local enrichment every
//! second, and the actions the chat panel and the summon sheet run on panes.
//! Background work reports back through a channel the UI drains each frame.

use crate::alerts::{alerts_for, AgentAlert};
use crate::events::{HerdrStream, StreamMessage};
use crate::herdr::*;
use crate::l10n::{tr, trf};
use serde_json::Value;
use std::collections::HashMap;
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

enum Inbox {
    Refresh { generation: u64, result: Result<(Fresh, Vec<Agent>), MonitorError>, herdr: HerdrState },
    Socket { generation: u64, path: Option<String>, panes: Vec<String>, was_up: bool },
    Stream(u64, StreamMessage),
    Sessions(Vec<HerdrSession>),
    Log(GuildEvent),
    Acknowledged(u64),
    Created { failure: Option<String>, waiting: Option<(String, String, String, String)>, log: Option<GuildEvent> },
}

struct PendingPrompt {
    pane: String,
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
    /// HUD state chip and search box.
    pub filter: StatusFilter,
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
    /// Background shells gone from the processes, waiting for the agent's
    /// transcript to say how they ended (or a few seconds).
    finishing: Vec<(String, crate::shells::Shell, Instant)>,
    /// Something the window chrome reacts to changed (agents, panels, filter).
    pub changed: bool,
    /// Herdr's availability; while not Up the rooms come from the standalone scan.
    pub herdr: HerdrState,
    /// Rooms that are not Herdr panes: the guild's members.
    extra: Vec<Agent>,
    /// The agent in guild mode and the guild's members it shows in its
    /// room, which then stands for the guild's own.
    lead: Option<(String, Vec<CrewMember>)>,
    next_herdr_try: Instant,

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
/// The wait before an Enter that follows text or arrows, as Herdr's own
/// `agent prompt` waits (300 ms).
const SUBMIT_PAUSE: Duration = Duration::from_millis(300);

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
            finishing: vec![],
            changed: false,
            herdr: HerdrState::Unknown,
            extra: vec![],
            lead: None,
            next_herdr_try: Instant::now(),
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
        filter_agents(&self.agents, self.filter, &self.query)
    }

    pub fn is_filtering(&self) -> bool {
        self.filter != StatusFilter::All || !self.query.trim().is_empty()
    }

    /// Without Herdr the rooms are read-only: nothing can be typed into an agent.
    pub fn standalone(&self) -> bool {
        !self.demo && self.herdr != HerdrState::Up
    }

    /// The HUD shows once there are enough rooms to sift, or while a filter hides some.
    pub fn shows_hud(&self) -> bool {
        self.agents.len() >= 3 || self.is_filtering()
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

    /// The guild's members, drawn as rooms beside Herdr's agents and kept
    /// through every refresh.
    pub fn set_extra(&mut self, extra: Vec<Agent>) {
        if self.extra == extra { return; }
        self.extra = extra;
        let base: Vec<Agent> = self.agents.iter().filter(|a| !a.id.starts_with("guild:")).cloned().collect();
        self.apply(base);
    }

    /// The agent in guild mode, with the members to draw in its room; the
    /// guild's own room goes while that agent is there.
    pub fn set_lead(&mut self, lead: Option<(String, Vec<CrewMember>)>) {
        if self.lead == lead { return; }
        self.lead = lead;
        // The last lead's decoration goes with it; `apply` puts it on the new one.
        let base: Vec<Agent> = self.agents.iter().filter(|a| !a.id.starts_with("guild:")).cloned().map(|mut a| { a.crew.clear(); a.guild_room = false; a }).collect();
        self.apply(base);
    }

    pub fn apply(&mut self, next: Vec<Agent>) {
        let mut next = next;
        next.extend(self.extra.iter().cloned());
        if let Some((id, crew)) = &self.lead {
            if next.iter().any(|a| &a.id == id) {
                next.retain(|a| a.id != crate::guild_view::HALL_ID);
                for a in next.iter_mut().filter(|a| &a.id == id) { a.crew = crew.clone(); a.guild_room = true; }
            }
        }
        // Sort last: background jobs and subagents can change a status after
        // Herdr's, and whoever needs attention must lead the grid.
        let next = sort_agents(next);
        let previous: HashMap<String, Agent> = self.agents.iter().map(|a| (a.id.clone(), a.clone())).collect();
        let ids: Vec<String> = next.iter().map(|a| a.id.clone()).collect();
        if self.updated.is_some() {
            let alerts = alerts_for(&previous, &next);
            self.alerts.extend(alerts);
            self.shells_ended(&previous, &next);
        }
        if self.updated.is_none() {
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
                } else {
                    self.log(trf("{} entró a la guild", &[&agent.project]), "joined");
                }
            }
            let gone: Vec<String> = self.agents.iter().filter(|a| !ids.contains(&a.id)).map(|a| a.project.clone()).collect();
            for project in gone { self.log(trf("{} salió de la guild", &[&project]), "left"); }
        }
        if self.agents != next { self.changed = true; }
        self.agents = next;
        self.updated = Some(Instant::now());
        self.error = None;
        if let Some(selected) = &self.selected {
            if !ids.contains(selected) { self.selected = None; self.changed = true; }
        }
    }

    /// Shells that left the processes since the last refresh wait here for
    /// the transcript's word on how they ended, up to 20 s, then ring and
    /// go to the log.
    fn shells_ended(&mut self, previous: &HashMap<String, Agent>, next: &[Agent]) {
        for old in previous.values() {
            let now: Vec<u32> = next.iter().find(|a| a.id == old.id).map(|a| a.shells.iter().map(|s| s.pid).collect()).unwrap_or_default();
            for shell in old.shells.iter().filter(|s| !now.contains(&s.pid)) {
                self.finishing.push((old.id.clone(), shell.clone(), Instant::now()));
            }
        }
        let mut waiting = vec![];
        for (id, shell, gone) in std::mem::take(&mut self.finishing) {
            let agent = next.iter().find(|a| a.id == id).or_else(|| previous.get(&id));
            let Some(agent) = agent.cloned() else { continue };
            let result = shell.task.as_ref().and_then(|t| agent.shell_results.iter().find(|r| &r.task == t)).cloned();
            if result.is_none() && shell.task.is_some() && gone.elapsed() < Duration::from_secs(20) { waiting.push((id, shell, gone)); continue; }
            if crate::shells::was_stopped(shell.pid) {
                self.log(trf("{} · shell {}: detenido", &[&agent.project, &shell.label]), "left");
                continue;
            }
            let ok = result.as_ref().map(|r| r.ok).unwrap_or(true);
            let how = result.map(|r| r.summary).unwrap_or_else(|| tr("Terminó."));
            self.log(trf("{} · shell {}: {}", &[&agent.project, &shell.label, &how]), if ok { "done" } else { "blocked" });
            // The orchestrator waiting on the guild: its next turn says what it found.
            if shell.is_guild() { continue; }
            let kind = if ok { crate::alerts::AlertKind::Finished } else { crate::alerts::AlertKind::NeedsHelp };
            self.alerts.push(AgentAlert { kind, agent, shell: Some((shell.label.clone(), how)) });
        }
        self.finishing = waiting;
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
                Inbox::Log(event) => { self.events.insert(0, event); self.events.truncate(40); }
                Inbox::Acknowledged(generation) => {
                    if generation == self.generation {
                        self.needs_snapshot = true;
                        self.refresh();
                    }
                }
                Inbox::Created { failure, waiting, log } => {
                    if let Some(log) = log { self.events.insert(0, log); self.events.truncate(40); }
                    if let Some((pane, name, project, text)) = waiting {
                        self.log(trf("{} espera tu respuesta; su prompt se enviará cuando esté listo.", &[&project]), "blocked");
                        if text.is_empty() {
                            self.select(Some(pane));
                        } else {
                            self.pending_prompts.push(PendingPrompt { pane, name, project, text, tries: 180, seen: false, next: Instant::now() });
                        }
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
        // Consume this request now so a newer request survives an in-flight result.
        if snapshot { self.needs_snapshot = false; }
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
            // The shells agents left running, from the processes.
            let result = result.map(|(mut base, mut shown)| {
                if !demo {
                    if let Some((fresh, _)) = base.as_mut() { crate::shells::drop_exited(fresh, &session); }
                    crate::shells::drop_exited(&mut shown, &session);
                    crate::shells::add_shells(&mut shown, &session);
                }
                (base, shown)
            });
            let _ = sender.send(Inbox::Refresh { generation, result, herdr });
            wake();
        });
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

    /// Run herdr commands off the UI thread; the receiver gets None or an
    /// error message.
    fn act(&self, commands: Vec<Vec<String>>) -> Receiver<Option<String>> {
        self.act_paced(commands, Duration::ZERO)
    }

    /// `act` with a pause between commands, so the agent's terminal has
    /// redrawn before the next keys arrive.
    fn act_paced(&self, commands: Vec<Vec<String>>, pause: Duration) -> Receiver<Option<String>> {
        let (tx, rx) = channel();
        let session = self.session.clone();
        let demo = self.demo;
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let mut failure = None;
            if !demo {
                for (i, args) in commands.iter().enumerate() {
                    if i > 0 && !pause.is_zero() { std::thread::sleep(pause); }
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    if run_herdr(&refs, &session, Duration::from_secs(6)).is_err() {
                        failure = Some(tr("Herdr rechazó la acción. Revisa el panel del agente."));
                        break;
                    }
                }
            }
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
    /// gets the raw keystrokes instead (e.g. an answer to its question): on
    /// one line, as each newline would press Enter, and Enter a moment after
    /// the text, as `agent prompt` does.
    /// `erase` backspaces in the agent's input go first, from its end: what
    /// is left there (an interrupted message put back, an unknown command)
    /// would otherwise be sent along with the message.
    pub fn send_erasing(&self, erase: usize, text: &str, agent: &Agent) -> Receiver<Option<String>> {
        let message = text.trim().to_string();
        let (tx, rx) = channel();
        if message.is_empty() { let _ = tx.send(None); return rx; }
        let inner = if agent.status == "blocked" {
            let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
            self.act_paced(vec![Monitor::args(&["pane", "send-text", &agent.id, &line]), Monitor::args(&["pane", "send-keys", &agent.id, "enter"])], SUBMIT_PAUSE)
        } else if erase > 0 {
            // ctrl+e first: the cursor may not be at the end of what is there.
            // Kiro drops backspaces that come together, so its line goes with ctrl+u.
            let mut keys = Monitor::args(&["agent", "send-keys", &agent.id, "ctrl+e"]);
            if harness_for(&agent.name) == "kiro" { keys.push("ctrl+u".into()); } else { keys.extend(std::iter::repeat_n("backspace".to_string(), erase)); }
            let erased = self.act(vec![keys]);
            let (inner_tx, inner_rx) = channel();
            let (session, demo, target, text) = (self.session.clone(), self.demo, agent.id.clone(), message.clone());
            std::thread::spawn(move || {
                let failure = erased.recv().unwrap_or(None);
                let _ = inner_tx.send(if failure.is_some() || demo { failure } else { prompt_failure(&session, &target, &text) });
            });
            inner_rx
        } else {
            self.prompt(&agent.id, &message)
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

    /// Answer a menu in words: `keys` turn its request down first (typed
    /// into the menu, letters would pick its shortcuts), then the text goes
    /// as a prompt as soon as the agent takes one again.
    pub fn decline_and_send(&self, keys: &[String], text: &str, agent: &Agent) -> Receiver<Option<String>> {
        let declined = self.press(keys, agent);
        let (tx, rx) = channel();
        let (session, demo, target, message, project) = (self.session.clone(), self.demo, agent.id.clone(), text.to_string(), agent.project.clone());
        let (log, wake) = (self.outbox.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let mut failure = declined.recv().unwrap_or(None);
            if failure.is_none() && !demo {
                // Herdr says agent_blocked until the menu is gone.
                for _ in 0..20 {
                    match run_herdr(&["agent", "prompt", &target, &message], &session, Duration::from_secs(10)) {
                        Err(e) if e.code() == Some("agent_blocked") => { std::thread::sleep(Duration::from_millis(250)); failure = Some(tr("El agente está esperando una respuesta; contéstale primero.")); }
                        Err(e) if !matches!(e.code(), Some("agent_prompt_stalled" | "agent_prompt_unverifiable")) => { failure = Some(tr("Herdr rechazó la acción. Revisa el panel del agente.")); break; }
                        _ => { failure = None; break; }
                    }
                }
            }
            if failure.is_none() { let _ = log.send(Inbox::Log(GuildEvent::new(trf("Tú → {}: {}", &[&project, &message]), "message"))); }
            let _ = tx.send(failure);
            wake();
        });
        rx
    }

    /// Press keys in the agent's pane: "enter" accepts a permission prompt's
    /// highlighted option, "esc" rejects it or interrupts the agent. Herdr
    /// writes the keys of one call at once, so an Enter after arrows goes a
    /// moment later: the menu has moved its highlight by then.
    pub fn press(&self, keys: &[String], agent: &Agent) -> Receiver<Option<String>> {
        let send = |keys: &[String]| { let mut args = Monitor::args(&["agent", "send-keys", &agent.id]); args.extend(keys.iter().cloned()); args };
        match keys.split_last() {
            Some((last, moves)) if last == "enter" && !moves.is_empty() => self.act_paced(vec![send(moves), send(&keys[keys.len() - 1..])], SUBMIT_PAUSE),
            _ => self.act(vec![send(keys)]),
        }
    }

    /// End the agent's session by typing /exit in its chat. A working or
    /// asking agent first gets esc, and a pause so the terminal does not read
    /// esc + "/" as Alt+/.
    pub fn finish(&self, agent: &Agent) -> Receiver<Option<String>> {
        let exit = vec![Monitor::args(&["pane", "send-text", &agent.id, "/exit"]), Monitor::args(&["pane", "send-keys", &agent.id, "enter"])];
        if agent.status != "working" && agent.status != "blocked" { return self.act(exit); }
        let (tx, rx) = channel();
        let first = self.act(vec![Monitor::args(&["agent", "send-keys", &agent.id, "esc"])]);
        let session = self.session.clone();
        let demo = self.demo;
        std::thread::spawn(move || {
            if let Some(failure) = first.recv().unwrap_or(None) { let _ = tx.send(Some(failure)); return; }
            std::thread::sleep(Duration::from_millis(600));
            let mut failure = None;
            if !demo {
                for args in &exit {
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    if run_herdr(&refs, &session, Duration::from_secs(6)).is_err() { failure = Some(tr("Herdr rechazó la acción. Revisa el panel del agente.")); break; }
                }
            }
            let _ = tx.send(failure);
        });
        rx
    }

    /// The agent's terminal as it shows, colours included (ANSI): the
    /// visible screen, or with `history` the last thousand lines (Herdr's most) of its
    /// scrollback, to scroll through. Demo mode paints a plausible screen.
    pub fn screen(&self, agent: &Agent, history: bool) -> Receiver<String> {
        let (tx, rx) = channel();
        let session = self.session.clone();
        let demo = self.demo;
        let agent = agent.clone();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let text = if demo || agent.id.starts_with("pid:") {
                format!("\x1b[38;2;215;119;87m●\x1b[0m \x1b[1m{}\x1b[0m\n  \x1b[2m⎿ {}\x1b[0m\n\n\x1b[38;2;136;136;136m────────────────────────────\x1b[0m\n❯ \x1b[2mTry \"fix the failing test\"\x1b[0m", agent.activity, tr("Leyendo archivos del proyecto…"))
            } else {
                let (source, lines) = if history { ("recent", "1000") } else { ("visible", "80") };
                run_herdr(&["agent", "read", &agent.id, "--source", source, "--lines", lines, "--format", "ansi"], &session, Duration::from_secs(4))
                    .map(|d| String::from_utf8_lossy(&d).to_string())
                    .unwrap_or_default()
            };
            let _ = tx.send(text);
            wake();
        });
        rx
    }

    /// Bring the agent's pane to the front inside Herdr.
    pub fn focus(&self, agent: &Agent) -> Receiver<Option<String>> {
        self.act(vec![Monitor::args(&["agent", "focus", &agent.id])])
    }

    /// Herdr marks a completion seen when explicitly focused. Reconcile
    /// immediately, even if the stream does not emit a status event.
    pub fn acknowledge(&self, agent: &Agent) -> Receiver<Option<String>> {
        let focused = self.focus(agent);
        let (tx, rx) = channel();
        let sender = self.outbox.clone();
        let generation = self.generation;
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let failure = focused.recv().unwrap_or_else(|_| Some(tr("Herdr rechazó la acción. Revisa el panel del agente.")));
            if failure.is_none() { let _ = sender.send(Inbox::Acknowledged(generation)); }
            let _ = tx.send(failure);
            wake();
        });
        rx
    }

    /// Submit a prompt with `herdr agent prompt`. Herdr only reports
    /// "stalled" or "unverifiable" when it cannot see the agent react within
    /// five seconds; that is not proof the text was lost, so it counts as sent.
    fn prompt(&self, target: &str, text: &str) -> Receiver<Option<String>> {
        let (tx, rx) = channel();
        let session = self.session.clone();
        let demo = self.demo;
        let (target, text) = (target.to_string(), text.to_string());
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let _ = tx.send(if demo { None } else { prompt_failure(&session, &target, &text) });
            wake();
        });
        rx
    }

    /// Start a new agent: a Herdr workspace in `folder` (its root pane is a
    /// fresh shell), `herdr agent start` of `kind` in it, then the first
    /// prompt if there is one. It shows up in the dungeon with the next
    /// snapshot. The receiver gets None or an error message.
    pub fn create(&self, kind: &str, folder: &str, prompt: &str, trust_all: bool) -> Receiver<Option<String>> {
        let (tx, rx) = channel();
        let session = self.session.clone();
        let demo = self.demo;
        let path = expand_home(folder);
        let label = std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
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
                    if !std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                        return Err(MonitorError::Message(tr("La carpeta no existe.")));
                    }
                    let created = run_herdr(&["workspace", "create", "--cwd", &path, "--label", &label, "--no-focus"], &session, Duration::from_secs(8))?;
                    let pane = root_pane(&created).ok_or_else(|| MonitorError::Message(tr("Herdr no devolvió el panel nuevo.")))?;
                    match start_agent(&session, &name, &kind, &pane, trust_all) {
                        Ok(_) => {}
                        Err(e) if e.code() == Some("agent_not_ready") => waiting_pane = Some(pane.clone()),
                        Err(e) => {
                            // What the shell shows says why, if anything does: read it before closing.
                            let screen = pane_screen(&session, &pane);
                            // Nothing runs in the new workspace: close it instead of leaving an empty shell behind.
                            if let Some(workspace) = created_workspace(&created) {
                                let _ = run_herdr(&["workspace", "close", &workspace], &session, Duration::from_secs(4));
                            }
                            if let Some(blocker) = shell_blocker(&screen) { return Err(MonitorError::Message(blocker)); }
                            let why = e.text();
                            let base = trf("{} no arrancó. Revisa que esté instalado.", &[&kind]);
                            let last = screen.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(60).collect::<String>());
                            return Err(MonitorError::Message(match (e.code(), last) {
                                (Some("timeout"), Some(last)) => trf("{} no arrancó. La terminal nueva quedó en: «{}»", &[&kind, &last]),
                                _ if why.is_empty() => base,
                                _ => format!("{base} ({why})"),
                            }));
                        }
                    }
                    if waiting_pane.is_none() && !text.is_empty() {
                        match run_herdr(&["agent", "prompt", &name, &text], &session, Duration::from_secs(10)) {
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
            let log = failure.is_none().then(|| GuildEvent::new(trf("Invocaste a {} en {}", &[&kind, &label]), "joined"));
            let waiting = waiting_pane.map(|pane| (pane, name.clone(), label.clone(), text.clone()));
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
        let mut sends: Vec<(String, String, String)> = vec![];
        for mut pending in std::mem::take(&mut self.pending_prompts) {
            if now < pending.next { keep.push(pending); continue; }
            pending.next = now + Duration::from_secs(1);
            if pending.tries == 0 {
                logs.push(GuildEvent::new(trf("{} no quedó listo; su prompt no se envió.", &[&pending.project]), "left"));
                continue;
            }
            pending.tries -= 1;
            let status = self.agents.iter().find(|a| a.id == pending.pane).map(|a| a.status.clone());
            if status.is_some() && !pending.seen { pending.seen = true; select = Some(pending.pane.clone()); }
            match status.as_deref() {
                Some("idle") | Some("done") => { sends.push((pending.name.clone(), pending.text.clone(), pending.project.clone())); continue; }
                None if pending.seen => continue, // the pane went away
                _ => keep.push(pending),
            }
        }
        self.pending_prompts = keep;
        for log in logs { self.events.insert(0, log); }
        self.events.truncate(40);
        if let Some(pane) = select { self.select(Some(pane)); }
        for (name, text, project) in sends {
            let rx = self.prompt(&name, &text);
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
        let session = self.session.clone();
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
                run_herdr(&["agent", "read", &agent.id, "--source", "recent", "--lines", "60", "--format", "text"], &session, Duration::from_secs(4))
                    .map(|d| String::from_utf8_lossy(&d).to_string())
                    .unwrap_or_default()
            };
            let rows = meaningful_lines(&text);
            let keep = if agent.status == "blocked" { 12 } else { lines };
            let start = rows.len().saturating_sub(keep);
            let start = if agent.status == "blocked" { menu_head(&rows, start) } else { start };
            let _ = tx.send(rows[start..].to_vec());
            wake();
        });
        rx
    }
}

/// The visible text of a pane, or nothing if Herdr does not answer.
fn pane_screen(session: &str, pane: &str) -> String {
    run_herdr(&["pane", "read", pane, "--source", "visible", "--format", "text"], session, Duration::from_secs(3)).map(|out| String::from_utf8_lossy(&out).into_owned()).unwrap_or_default()
}

/// `herdr agent start` in a pane that was created a moment ago. The pane's
/// shell takes a few hundred milliseconds to reach its prompt and Herdr
/// answers `agent_pane_busy` ("not an available shell") until then, so that
/// code is retried for up to ten seconds; any other answer is final. A shell
/// that asks something first (see `shell_blocker`) is not typed into: the
/// command would only answer its question.
///
/// `herdr agent start … -- <args>` forwards <args> to the harness's own
/// command, so full-permission flags (kiro: --trust-all-tools) ride along.
fn start_agent(session: &str, name: &str, kind: &str, pane: &str, trust_all: bool) -> Result<Vec<u8>, MonitorError> {
    let mut args: Vec<&str> = vec!["agent", "start", name, "--kind", kind, "--pane", pane, "--timeout", "60000"];
    let extra = if trust_all { crate::herdr::trust_all_args(kind) } else { &[] };
    if !extra.is_empty() {
        args.push("--");
        args.extend_from_slice(extra);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(blocker) = shell_blocker(&pane_screen(session, pane)) { return Err(MonitorError::Message(blocker)); }
        match run_herdr(&args, session, Duration::from_secs(65)) {
            Err(e) if e.code() == Some("agent_pane_busy") && Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            other => return other,
        }
    }
}

fn prompt_failure(session: &str, target: &str, text: &str) -> Option<String> {
    match run_herdr(&["agent", "prompt", target, text], session, Duration::from_secs(10)) {
        Ok(_) => None,
        Err(e) if matches!(e.code(), Some("agent_prompt_stalled" | "agent_prompt_unverifiable")) => None,
        Err(e) if e.code() == Some("agent_blocked") => Some(tr("El agente está esperando una respuesta; contéstale primero.")),
        Err(_) => Some(tr("Herdr rechazó la acción. Revisa el panel del agente.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_in_guild_mode_takes_the_crew_and_the_guilds_own_room_goes() {
        let mut monitor = Monitor::new("default".into(), StatusFilter::All);
        let hall = Agent::new(crate::guild_view::HALL_ID, "claude", "working", "Game", "", "/r");
        monitor.set_extra(vec![hall]);
        monitor.apply(vec![Agent::new("w1:p1", "claude", "idle", "Game", "", "/r"), Agent::new("w2:p1", "codex", "idle", "Other", "", "/o")]);
        let crew = vec![CrewMember { name: "Orin".into(), harness: "kiro".into(), status: "working".into(), action: Some("read".into()) }];
        let ids = |m: &Monitor| { let mut v: Vec<String> = m.agents.iter().map(|a| a.id.clone()).collect(); v.sort(); v };
        assert_eq!(ids(&monitor), vec!["guild:hall", "w1:p1", "w2:p1"]);
        monitor.set_lead(Some(("w1:p1".into(), crew.clone())));
        assert_eq!(ids(&monitor), vec!["w1:p1", "w2:p1"], "the lead's room stands for the guild's");
        let lead = monitor.agents.iter().find(|a| a.id == "w1:p1").unwrap();
        assert!(lead.guild_room && lead.crew == crew);
        assert!(monitor.agents.iter().find(|a| a.id == "w2:p1").unwrap().crew.is_empty(), "only the lead");
        // Herdr's next refresh keeps it so.
        monitor.apply(vec![Agent::new("w1:p1", "claude", "working", "Game", "", "/r"), Agent::new("w2:p1", "codex", "idle", "Other", "", "/o")]);
        assert!(monitor.agents.iter().any(|a| a.id == "w1:p1" && a.guild_room) && !monitor.agents.iter().any(|a| a.id == crate::guild_view::HALL_ID));
        // A lead that is not there (its pane closed) leaves the guild its own room.
        monitor.set_lead(Some(("w9:p1".into(), crew)));
        assert!(monitor.agents.iter().any(|a| a.id == crate::guild_view::HALL_ID));
        // Leaving guild mode: the hall is back, the agent's room is its own.
        monitor.set_lead(None);
        assert!(monitor.agents.iter().any(|a| a.id == crate::guild_view::HALL_ID));
        assert!(!monitor.agents.iter().find(|a| a.id == "w1:p1").unwrap().guild_room);
    }
}
