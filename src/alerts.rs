//! Moments worth a sound or a notification: an agent starts needing help,
//! or finishes its work.

use crate::herdr::Agent;
use crate::l10n::trf;
use crate::prefs::Prefs;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    NeedsHelp,
    Finished,
}

/// One agent whose change deserves an alert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAlert {
    pub kind: AlertKind,
    pub agent: Agent,
    /// For a background shell that ended: what it was and how it ended. It
    /// rings as finishing, or as needing help when it failed.
    pub shell: Option<(String, String)>,
}

/// The agents whose change deserves an alert, those needing help first.
pub fn alerts_for(previous: &HashMap<String, Agent>, next: &[Agent]) -> Vec<AgentAlert> {
    let mut alerts = vec![];
    for agent in next {
        let Some(old) = previous.get(&agent.id) else { continue };
        if old.status == agent.status { continue; }
        if agent.status == "blocked" {
            alerts.push(AgentAlert { kind: AlertKind::NeedsHelp, agent: agent.clone(), shell: None });
        } else if agent.status == "done" || (old.status == "working" && agent.status == "idle") {
            alerts.push(AgentAlert { kind: AlertKind::Finished, agent: agent.clone(), shell: None });
        }
    }
    let (help, done): (Vec<_>, Vec<_>) = alerts.into_iter().partition(|a| a.kind == AlertKind::NeedsHelp);
    help.into_iter().chain(done).collect()
}

/// Which jingle a snapshot change deserves. Needing help wins over finishing
/// so a busy refresh never plays two jingles on top of each other.
pub fn alert_for(previous: &HashMap<String, Agent>, next: &[Agent]) -> Option<AlertKind> {
    alerts_for(previous, next).first().map(|a| a.kind)
}

pub fn wants_sound(prefs: &Prefs, kind: AlertKind) -> bool {
    prefs.sound_enabled && if kind == AlertKind::NeedsHelp { prefs.sound_needs_help } else { prefs.sound_finished }
}

pub fn wants_notification(prefs: &Prefs, kind: AlertKind) -> bool {
    prefs.notify_enabled && if kind == AlertKind::NeedsHelp { prefs.notify_needs_help } else { prefs.notify_finished }
}

/// Bell-like notes (frequency Hz, seconds until the next one; 0 Hz is a
/// rest) rendered to 16-bit mono samples. Each note is a sine with soft
/// overtones, a gentle attack and a natural exponential decay that keeps
/// ringing under the next note, so it chimes instead of beeping.
pub fn jingle(notes: &[(f64, f64)], rate: u32, volume: f64) -> Vec<i16> {
    let ring = 0.45; // how long a note keeps sounding after the next one starts
    let rate_f = rate as f64;
    let length: f64 = notes.iter().map(|n| n.1).sum::<f64>() + ring;
    let mut mix = vec![0.0f64; (length * rate_f) as usize];
    let mut start = 0.0;
    for &(frequency, step) in notes {
        if frequency > 0.0 {
            let first = (start * rate_f) as usize;
            let count = (((step + ring) * rate_f) as usize).min(mix.len().saturating_sub(first));
            for i in 0..count {
                let t = i as f64 / rate_f;
                let attack = (t / 0.012).min(1.0);
                let decay = (-t * 5.5).exp();
                let tone = (2.0 * std::f64::consts::PI * frequency * t).sin()
                    + 0.22 * (4.0 * std::f64::consts::PI * frequency * t).sin()
                    + 0.06 * (6.0 * std::f64::consts::PI * frequency * t).sin();
                mix[first + i] += tone * attack * decay;
            }
        }
        start += step;
    }
    // Fade the very end so the last sample is silent, then scale.
    let tail = (0.03 * rate_f) as usize;
    let n = mix.len();
    for i in 0..tail.min(n) { mix[n - 1 - i] *= i as f64 / tail as f64; }
    mix.into_iter().map(|s| ((s * volume).clamp(-1.0, 1.0) * i16::MAX as f64) as i16).collect()
}

pub const SAMPLE_RATE: u32 = 44100;

pub fn chime(kind: AlertKind) -> Vec<i16> {
    match kind {
        // A soft two-note chime, falling a third: "ding-dong", someone is at the door.
        AlertKind::NeedsHelp => jingle(&[(784.0, 0.22), (659.0, 0.5)], SAMPLE_RATE, 0.12),
        // A gentle rising arpeggio that rings out: quest complete.
        AlertKind::Finished => jingle(&[(523.0, 0.12), (659.0, 0.12), (784.0, 0.12), (1047.0, 0.6)], SAMPLE_RATE, 0.1),
    }
}

/// Plays the chimes on the default output. The audio thread owns the device;
/// requests are queued so a missing device never blocks the UI.
pub struct Sounds {
    queue: Sender<AlertKind>,
}

impl Sounds {
    pub fn new() -> Sounds {
        let (queue, requests) = channel::<AlertKind>();
        std::thread::Builder::new()
            .name("chimes".into())
            .spawn(move || play_loop(requests))
            .ok();
        Sounds { queue }
    }

    pub fn play(&self, kind: AlertKind) {
        let _ = self.queue.send(kind);
    }

    /// Play both chimes, whatever the preferences: the menu's sound check.
    pub fn preview(&self) {
        let queue = self.queue.clone();
        std::thread::spawn(move || {
            let _ = queue.send(AlertKind::NeedsHelp);
            std::thread::sleep(std::time::Duration::from_millis(1300));
            let _ = queue.send(AlertKind::Finished);
        });
    }
}

fn play_loop(requests: Receiver<AlertKind>) {
    let help = chime(AlertKind::NeedsHelp);
    let done = chime(AlertKind::Finished);
    let mut output: Option<(rodio::OutputStream, rodio::OutputStreamHandle)> = None;
    while let Ok(kind) = requests.recv() {
        if output.is_none() { output = rodio::OutputStream::try_default().ok(); }
        let Some((_, handle)) = &output else { continue };
        let samples = if kind == AlertKind::NeedsHelp { help.clone() } else { done.clone() };
        let source = rodio::buffer::SamplesBuffer::new(1, SAMPLE_RATE, samples);
        if let Ok(sink) = rodio::Sink::try_new(handle) {
            sink.append(source);
            sink.detach();
        }
    }
}

/// Desktop notifications for the same moments as the chimes: on by default
/// when an agent needs help, opt-in when one finishes. On Linux, clicking a
/// banner shows the dungeon with that agent's chat open.
pub struct Notifier {
    opened: Receiver<String>,
    open_tx: Sender<String>,
}

impl Notifier {
    pub fn new() -> Notifier {
        let (open_tx, opened) = channel();
        prepare();
        Notifier { opened, open_tx }
    }

    /// The pane ids whose banners were clicked since the last call.
    pub fn clicked(&self) -> Vec<String> {
        let mut ids = vec![];
        while let Ok(id) = self.opened.try_recv() { ids.push(id); }
        ids
    }

    pub fn post(&self, prefs: &Prefs, alerts: &[AgentAlert]) {
        for alert in alerts.iter().filter(|a| wants_notification(prefs, a.kind)) {
            let agent = alert.agent.clone();
            let (title, body) = match &alert.shell {
                Some((label, how)) => (
                    if alert.kind == AlertKind::NeedsHelp { trf("{}: un shell en segundo plano falló", &[&agent.project]) } else { trf("{}: un shell en segundo plano terminó", &[&agent.project]) },
                    format!("{label}\n{how}"),
                ),
                None => (
                    if alert.kind == AlertKind::NeedsHelp { trf("{} necesita atención", &[&agent.name]) } else { trf("{} terminó", &[&agent.name]) },
                    format!("{}\n{}", agent.project, agent.question.clone().unwrap_or_else(|| agent.activity.clone())),
                ),
            };
            let open_tx = self.open_tx.clone();
            std::thread::spawn(move || post_one(&title, &body, &agent.id, open_tx));
        }
    }
}

/// On macOS a notification is posted on behalf of an application. Set it
/// once up front: the bundle when running inside one, else Terminal.
/// Without this the library looks an app up by name and macOS answers with
/// a "Where is use_default?" dialog for every notification.
#[cfg(target_os = "macos")]
fn prepare() {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let bundle = if exe.contains("/Contents/MacOS/") { "io.github.josevalencia125.agents-pixel-dungeon" } else { "com.apple.Terminal" };
    if mac_notification_sys::set_application(bundle).is_err() {
        // Not registered with Launch Services (e.g. a bundle never opened): no notifications rather than dialogs.
        NOTIFICATIONS_OFF.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(not(target_os = "macos"))]
fn prepare() {}

static NOTIFICATIONS_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "linux")]
fn post_one(title: &str, body: &str, pane: &str, open_tx: Sender<String>) {
    let shown = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .appname("Agents Pixel Dungeon")
        .action("default", "Open")
        .show();
    if let Ok(handle) = shown {
        let pane = pane.to_string();
        handle.wait_for_action(|action| {
            if action == "default" { let _ = open_tx.send(pane.clone()); }
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn post_one(title: &str, body: &str, _pane: &str, _open_tx: Sender<String>) {
    if NOTIFICATIONS_OFF.load(std::sync::atomic::Ordering::Relaxed) { return; }
    let _ = notify_rust::Notification::new().summary(title).body(body).appname("Agents Pixel Dungeon").show();
}
