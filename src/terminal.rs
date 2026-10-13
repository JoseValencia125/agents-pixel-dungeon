//! Local PTY shells, independent from the agents managed by Herdr.
use crate::l10n::tr;
use egui_term::{BackendSettings, PtyEvent, TerminalBackend, TerminalView};
use std::{path::PathBuf, sync::mpsc};

struct Session {
    id: u64,
    cwd: PathBuf,
    backend: Option<TerminalBackend>,
    error: Option<String>,
    exited: bool,
    visible: bool,
}

pub struct Terminals {
    sessions: Vec<Session>,
    sender: mpsc::Sender<(u64, PtyEvent)>,
    receiver: mpsc::Receiver<(u64, PtyEvent)>,
}

impl Default for Terminals {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self { sessions: Vec::new(), sender, receiver }
    }
}

fn working_directory(cwd: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    if cwd.trim().is_empty() || cwd == "~" { home }
    else if let Some(rest) = cwd.strip_prefix("~/") { home.join(rest) }
    else { PathBuf::from(cwd) }
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
        self.sessions.push(Session { id, cwd, backend: None, error: None, exited: false, visible: true });
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        for (id, event) in self.receiver.try_iter() {
            if matches!(event, PtyEvent::Exit) {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) { s.exited = true; }
            }
        }
        for session in &mut self.sessions {
            if !session.visible { continue; }
            let sender = self.sender.clone();
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("shell", session.id)),
                egui::ViewportBuilder::default()
                    .with_title(format!("Herdr >_ — {}", session.cwd.display()))
                    .with_inner_size([850.0, 520.0]).with_min_inner_size([400.0, 240.0]),
                |ctx, _| {
                    if ctx.input(|i| i.viewport().close_requested()) { session.visible = false; }
                    if session.backend.is_none() && session.error.is_none() {
                        let settings = BackendSettings {
                            shell: std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/bash".into()),
                            args: vec!["-l".into()],
                            working_directory: Some(session.cwd.clone()),
                        };
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
                            if session.exited || session.error.is_some() {
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
