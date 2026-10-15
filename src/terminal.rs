//! Local PTY shells, independent from the agents managed by Herdr.
use crate::l10n::tr;
use egui_term::{BackendSettings, PtyEvent, TerminalBackend, TerminalView};
use std::{path::PathBuf, sync::mpsc};

struct Session {
    id: u64,
    cwd: PathBuf,
    /// A program to run instead of the login shell (a game to play), with
    /// its environment, and the tag reported once it exits.
    command: Option<(Vec<String>, Vec<(String, String)>)>,
    tag: Option<String>,
    reported: bool,
    backend: Option<TerminalBackend>,
    error: Option<String>,
    exited: bool,
    visible: bool,
}

pub struct Terminals {
    sessions: Vec<Session>,
    sender: mpsc::Sender<(u64, PtyEvent)>,
    receiver: mpsc::Receiver<(u64, PtyEvent)>,
    finished: Vec<String>,
}

impl Default for Terminals {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self { sessions: Vec::new(), sender, receiver, finished: Vec::new() }
    }
}

fn working_directory(cwd: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    if cwd.trim().is_empty() || cwd == "~" { home }
    else if let Some(rest) = cwd.strip_prefix("~/") { home.join(rest) }
    else { PathBuf::from(cwd) }
}

/// What the pty runs: the login shell, or a program with variables
/// added (through env, so the program finds them on start).
fn settings_for(cwd: &PathBuf, command: Option<&(Vec<String>, Vec<(String, String)>)>) -> BackendSettings {
    match command {
        Some((argv, env)) => BackendSettings {
            shell: "/usr/bin/env".into(),
            args: env.iter().map(|(k, v)| format!("{k}={v}")).chain(argv.iter().cloned()).collect(),
            working_directory: Some(cwd.clone()),
        },
        None => BackendSettings {
            shell: std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/bash".into()),
            args: vec!["-l".into()],
            working_directory: Some(cwd.clone()),
        },
    }
}

impl Terminals {
    pub fn open(&mut self, ctx: &egui::Context, cwd: &str) {
        let cwd = working_directory(cwd);
        if let Some(session) = self.sessions.iter_mut().find(|s| s.cwd == cwd) {
            session.visible = true;
            ctx.send_viewport_cmd_to(egui::ViewportId::from_hash_of(("shell", session.id)), egui::ViewportCommand::Focus);
            return;
        }
        let id = self.sessions.len() as u64 + 1;
        self.sessions.push(Session { id, cwd, command: None, tag: None, reported: false, backend: None, error: None, exited: false, visible: true });
    }

    /// Run `argv` in a new terminal window in `cwd`, with `env` added;
    /// `tag` comes back from `take_finished` once the program exits.
    pub fn run(&mut self, cwd: &str, argv: Vec<String>, env: Vec<(String, String)>, tag: &str) {
        if argv.is_empty() { return; }
        let id = self.sessions.len() as u64 + 1;
        self.sessions.push(Session { id, cwd: working_directory(cwd), command: Some((argv, env)), tag: Some(tag.into()), reported: false, backend: None, error: None, exited: false, visible: true });
    }

    /// Tags of programs started with `run` that have exited since the last call.
    pub fn take_finished(&mut self) -> Vec<String> {
        std::mem::take(&mut self.finished)
    }

    /// Note which programs exited, and queue the tags of the ones `run` started.
    fn drain_events(&mut self) {
        for (id, event) in self.receiver.try_iter() {
            if matches!(event, PtyEvent::Exit) {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                    s.exited = true;
                    if let (Some(tag), false) = (&s.tag, s.reported) { self.finished.push(tag.clone()); s.reported = true; }
                }
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        self.drain_events();
        for session in &mut self.sessions {
            if !session.visible { continue; }
            let sender = self.sender.clone();
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("shell", session.id)),
                egui::ViewportBuilder::default()
                    .with_title(if session.command.is_some() { format!("{} — {}", tr("Jugar"), session.cwd.display()) } else { format!("Herdr >_ — {}", session.cwd.display()) })
                    .with_inner_size([850.0, 520.0]).with_min_inner_size([400.0, 240.0]),
                |ctx, _| {
                    if ctx.input(|i| i.viewport().close_requested()) { session.visible = false; }
                    if session.backend.is_none() && session.error.is_none() {
                        let settings = settings_for(&session.cwd, session.command.as_ref());
                        match TerminalBackend::new(session.id, ctx.clone(), sender.clone(), settings) {
                            Ok(backend) => { session.backend = Some(backend); session.exited = false; }
                            Err(error) => session.error = Some(error.to_string()),
                        }
                    }
                    egui::TopBottomPanel::top("shell_toolbar").show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(session.cwd.to_string_lossy());
                            if ui.button(tr("Ocultar terminal")).clicked() { session.visible = false; }
                            if ui.button(tr("Terminal externa")).clicked() {
                                crate::herdr::open_terminal(&session.cwd.to_string_lossy());
                            }
                            if (session.exited || session.error.is_some()) && session.command.is_none() {
                                if ui.button(tr("Nueva sesión")).clicked() {
                                    session.backend = None;
                                    session.error = None;
                                }
                            }
                        });
                        ui.small(tr("Cerrar la ventana conserva la sesión. Usa exit para terminarla."));
                        if session.exited { ui.label(tr("Sesión terminada")); }
                        if let Some(error) = &session.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
                    });
                    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(egui::Color32::BLACK)).show(ctx, |ui| {
                        if let Some(backend) = &mut session.backend {
                            let view = TerminalView::new(ui, backend).set_focus(!session.exited);
                            ui.add(view);
                        }
                    });
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_the_same_folder_restores_the_session() {
        let mut terminals = Terminals::default();
        let ctx = egui::Context::default();
        terminals.open(&ctx, "/tmp/a folder");
        terminals.sessions[0].visible = false;
        terminals.open(&ctx, "/tmp/a folder");
        assert_eq!(terminals.sessions.len(), 1);
        assert!(terminals.sessions[0].visible);
        assert_eq!(terminals.sessions[0].cwd, PathBuf::from("/tmp/a folder"));
        terminals.open(&ctx, "/tmp/another");
        assert_eq!(terminals.sessions.len(), 2);
    }

    #[test]
    fn a_game_runs_in_the_pty_with_its_telemetry_variable_and_reports_its_end() {
        use std::time::{Duration, Instant};
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let out = cwd.join(format!("hpd-play-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let command = (vec!["/bin/sh".to_string(), "-c".to_string(), "printf %s \"$GUILD_TELEMETRY\" > \"$OUT\"".to_string()], vec![("GUILD_TELEMETRY".to_string(), "/tmp/t.jsonl".to_string()), ("OUT".to_string(), out.display().to_string())]);
        let (tx, rx) = mpsc::channel();
        let _backend = TerminalBackend::new(7, egui::Context::default(), tx, settings_for(&cwd, Some(&command))).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let event = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("the program should exit");
            if matches!(event.1, PtyEvent::Exit) { break; }
        }
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "/tmp/t.jsonl");
        let _ = std::fs::remove_file(&out);
        // A run session reports its tag once, after its exit.
        let mut terminals = Terminals::default();
        terminals.run("/tmp", vec!["true".into()], vec![], "pt-1");
        assert!(terminals.take_finished().is_empty());
        terminals.sender.send((1, PtyEvent::Exit)).unwrap();
        terminals.sender.send((1, PtyEvent::Exit)).unwrap();
        terminals.drain_events();
        assert_eq!(terminals.take_finished(), vec!["pt-1".to_string()], "reported once");
        assert!(terminals.take_finished().is_empty());
    }

    #[test]
    fn pty_runs_commands_in_the_requested_directory_and_exits() {
        use egui_term::BackendCommand;
        use std::time::{Duration, Instant};
        let (tx, rx) = mpsc::channel();
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let mut backend = TerminalBackend::new(99, egui::Context::default(), tx, BackendSettings {
            shell: "/bin/sh".into(), args: vec![], working_directory: Some(cwd.clone()),
        }).unwrap();
        backend.process_command(BackendCommand::Write(b"printf 'PTY_%s_OK\\n' 42; pwd\r".to_vec()));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text: String = backend.sync().grid.display_iter().map(|cell| cell.c).collect();
            if text.contains("PTY_42_OK") && text.contains(cwd.to_str().unwrap()) { break; }
            assert!(Instant::now() < deadline, "PTY output missing: {text}");
            std::thread::sleep(Duration::from_millis(20));
        }
        backend.process_command(BackendCommand::Write(b"exit\r".to_vec()));
        loop {
            let event = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("shell should exit");
            if matches!(event.1, PtyEvent::Exit) { break; }
        }
    }
}
