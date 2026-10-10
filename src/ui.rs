//! The widget's chrome around the dungeon: the HUD with state chips and
//! search, the chat panel, the "summon an agent" panel, the activity log on
//! its parchment, the connection banner, the hover controls, the drag
//! handle and the resize grip.

use crate::ansi::Span;
use crate::herdr::{Agent, MenuOptions, StatusFilter};
use crate::l10n::{tr, trf};
use crate::monitor::{GuildEvent, Monitor};
use crate::prefs::Prefs;
use crate::scene::agent_color;
use egui::{pos2, vec2, Align2, Color32, Context, CornerRadius, FontId, Frame, Key, Margin, RichText, Stroke, Ui, Vec2};
use std::sync::mpsc::Receiver;

pub const HUD_HEIGHT: f32 = 28.0;
pub const PANEL_HEIGHT: f32 = 232.0;
pub const PANEL_FILL: Color32 = Color32::from_rgb(23, 20, 28);
pub const HUD_FILL: Color32 = Color32::from_rgb(19, 17, 23);
const DIM: Color32 = Color32::from_rgb(150, 150, 155);

pub fn tint(filter: StatusFilter) -> Color32 {
    match filter {
        StatusFilter::Blocked => Color32::from_rgb(255, 69, 58),
        StatusFilter::Limited => crate::scene::LIMITED,
        StatusFilter::Working => Color32::from_rgb(255, 159, 10),
        StatusFilter::Idle => Color32::from_rgb(152, 152, 157),
        StatusFilter::Done => Color32::from_rgb(48, 209, 88),
        StatusFilter::All => Color32::WHITE,
    }
}

pub fn filter_name(filter: StatusFilter) -> String {
    tr(match filter {
        StatusFilter::All => "Todos",
        StatusFilter::Blocked => "Necesita atención",
        StatusFilter::Limited => "Límite de sesión",
        StatusFilter::Working => "Trabajando",
        StatusFilter::Idle => "En espera",
        StatusFilter::Done => "Listo",
    })
}

fn alpha(color: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (a * 255.0) as u8)
}

fn mono(text: &str, size: f32) -> RichText {
    RichText::new(text).font(FontId::monospace(fs(size)))
}

/// Files dropped on the window this frame, as paths ready for a message:
/// quoted when they hold a space, as the terminal would paste them.
fn dropped_paths(ui: &Ui) -> Vec<String> {
    ui.ctx().input(|i| {
        i.raw.dropped_files.iter().filter_map(|f| f.path.as_ref()).map(|p| {
            let path = p.to_string_lossy().to_string();
            if path.contains(' ') { format!("'{}'", path.replace('\'', "'\\''")) } else { path }
        }).collect()
    })
}

/// Add dropped files' paths to a draft, space-separated.
fn add_paths(draft: &mut String, paths: &[String]) {
    for path in paths {
        if !draft.is_empty() && !draft.ends_with(' ') { draft.push(' '); }
        draft.push_str(path);
        draft.push(' ');
    }
}

/// While files are dragged over the window: a frame and a word on the
/// panel, so it is clear the drop goes to the message.
fn drop_hint(ui: &Ui, rect: egui::Rect) {
    if !ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) { return; }
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop-hint")));
    painter.rect_filled(rect, CornerRadius::same(5), alpha(Color32::BLACK, 0.55));
    painter.rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0_f32, Color32::from_rgb(140, 217, 115)), egui::StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, tr("Suelta para añadir la ruta al mensaje"), FontId::monospace(fs(13.0)), Color32::WHITE);
}

/// A console line that opens what ⌘ (Ctrl elsewhere) + click lands on: a
/// URL in the browser, a file or folder (relative to the agent's folder,
/// `:line` dropped) with its app. Holding ⌘ over one shows the hand.
fn console_line(ui: &mut Ui, spans: &[Span], size: f32, cwd: &str) {
    // Without ⌘ it is a plain label, so its text can still be selected.
    if !ui.input(|i| i.modifiers.command) { ui.add(screen_line(spans, size)); return; }
    let (pos, galley, response) = screen_line(spans, size).sense(egui::Sense::click()).layout_in_ui(ui);
    let target = response.hover_pos().and_then(|pointer| {
        let index = galley.cursor_from_pos(pointer - pos).ccursor.index;
        crate::herdr::link_at(galley.text(), index).and_then(|link| crate::herdr::resolve_link(&link, cwd))
    });
    ui.painter().galley(pos, galley, Color32::from_gray(222));
    if let Some(target) = target {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        if response.clicked() { crate::herdr::reveal_target(&target); }
        let web = target.starts_with("http://") || target.starts_with("https://");
        response.on_hover_text(if web { target.clone() } else { trf("Mostrar en Finder: {}", &[&crate::herdr::abbreviate_home(&target)]) });
    }
}

/// One terminal line as a label: each run in its colour, bold as a
/// brighter stroke, dim faded, inverse swapped (done in the parser).
fn screen_line(spans: &[Span], size: f32) -> egui::Label {
    use egui::text::{LayoutJob, TextFormat};
    let mut job = LayoutJob::default();
    if spans.is_empty() {
        job.append(" ", 0.0, TextFormat { font_id: FontId::monospace(size), ..Default::default() });
    }
    for span in spans {
        let mut color = span.fg.unwrap_or(Color32::from_gray(222));
        if span.bold { color = Color32::from_rgb(color.r().saturating_add(25), color.g().saturating_add(25), color.b().saturating_add(25)); }
        if span.dim { color = alpha(color, 0.55); }
        let format = TextFormat {
            font_id: FontId::monospace(size),
            color,
            background: span.bg.unwrap_or(Color32::TRANSPARENT),
            italics: span.italic,
            underline: if span.underline { Stroke::new(1.0_f32, color) } else { Stroke::NONE },
            ..Default::default()
        };
        job.append(&span.text, 0.0, format);
    }
    egui::Label::new(job).wrap()
}

/// Text size in the panels, as a factor the menu and ⌘+/⌘- change.
static TEXT_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f800000); // 1.0

pub fn set_text_scale(scale: f32) {
    TEXT_SCALE.store(scale.clamp(0.7, 1.6).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

pub fn text_scale() -> f32 {
    f32::from_bits(TEXT_SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

/// A font size scaled by the text size preference.
pub fn fs(size: f32) -> f32 {
    (size * text_scale()).round()
}

/// The console's text size, a factor of its own (the terminal is read at a
/// different size than the panels around it).
static CONSOLE_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f800000); // 1.0

pub fn set_console_scale(scale: f32) {
    CONSOLE_SCALE.store(scale.clamp(0.6, 2.5).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

pub fn console_scale() -> f32 {
    f32::from_bits(CONSOLE_SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

fn flat_button(ui: &mut Ui, text: RichText) -> egui::Response {
    ui.add(egui::Button::new(text).frame(false))
}

// ---- HUD

#[derive(Default)]
pub struct HudState {
    pub searching: bool,
    focus_search: bool,
}

pub struct HudAction {
    pub toggle_log: bool,
    pub compose: bool,
    /// Open a terminal to type a command in.
    pub terminal: bool,
}

/// The HUD grows a second row while the search box is open.
pub fn hud_height(state: &HudState, monitor: &Monitor) -> f32 {
    if state.searching || !monitor.query.is_empty() { HUD_HEIGHT + SEARCH_ROW } else { HUD_HEIGHT }
}

pub const SEARCH_ROW: f32 = 26.0;

/// Bar under the rooms: one chip per state (with its count) to filter the
/// rooms, and a search box (on its own row) over project, harness, branch,
/// folder and activity.
pub fn hud(ui: &mut Ui, monitor: &mut Monitor, state: &mut HudState) -> HudAction {
    let mut action = HudAction { toggle_log: false, compose: false, terminal: false };
    // One column of rooms leaves no room for words: the "All" chip shows only its count.
    let narrow = ui.available_width() < crate::scene::width_for(2) - 20.0;
    let open = state.searching || !monitor.query.is_empty();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), HUD_HEIGHT), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = vec2(if narrow { 3.0 } else { 4.0 }, 0.0);
            for filter in StatusFilter::ALL {
                chip(ui, monitor, filter, narrow);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = vec2(12.0, 0.0);
                let glyph = if open { "x" } else { "🔍" };
                let button = flat_button(ui, mono(glyph, 13.0).color(alpha(Color32::WHITE, 0.75)))
                    .on_hover_text(if open { tr("Cerrar búsqueda (esc)") } else { tr("Buscar por proyecto, agente, rama, carpeta o actividad") });
                if button.clicked() {
                    if open { monitor.set_query(String::new()); state.searching = false; } else { state.searching = true; state.focus_search = true; }
                }
                if flat_button(ui, mono("📜", 13.0).color(alpha(Color32::from_rgb(237, 214, 158), 0.85))).on_hover_text(tr("Registro de actividad")).clicked() {
                    action.toggle_log = true;
                }
                if flat_button(ui, mono(">_", 11.0).strong().color(alpha(Color32::WHITE, 0.75))).on_hover_text(tr("Abrir una terminal")).clicked() {
                    action.terminal = true;
                }
                if flat_button(ui, mono("+", 16.0).strong().color(Color32::from_rgb(140, 217, 115))).on_hover_text(tr("Invocar un agente nuevo")).clicked() {
                    action.compose = true;
                }
            });
        });
        if open {
            ui.allocate_ui_with_layout(vec2(ui.available_width(), SEARCH_ROW), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let mut query = monitor.query.clone();
                let edit = egui::TextEdit::singleline(&mut query)
                    .hint_text(tr("Buscar…"))
                    .font(FontId::monospace(fs(11.0)))
                    .desired_width(ui.available_width())
                    .margin(Margin::symmetric(6, 3));
                let response = ui.add(edit);
                if state.focus_search { response.request_focus(); state.focus_search = false; }
                if response.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) { query.clear(); state.searching = false; }
                monitor.set_query(query);
            });
        }
    });
    action
}

fn chip(ui: &mut Ui, monitor: &mut Monitor, filter: StatusFilter, narrow: bool) {
    let count = if filter == StatusFilter::All { monitor.agents.len() } else { monitor.agents.iter().filter(|a| filter.admits(a)).count() };
    let on = monitor.filter == filter;
    // The usage limit is rare: its chip shows up only while someone is trapped.
    if filter == StatusFilter::Limited && count == 0 && !on { return; }
    // Agents asking for help keep the chip lit red whatever the filter.
    let alarm = filter == StatusFilter::Blocked && count > 0;
    let color = tint(filter);
    let fill = if alarm { alpha(Color32::RED, if on { 0.75 } else { 0.45 }) } else if on { alpha(color, if filter == StatusFilter::All { 0.2 } else { 0.4 }) } else { alpha(Color32::BLACK, 0.35) };
    let edge = if on { alpha(Color32::WHITE, 0.7) } else if alarm { Color32::RED } else { alpha(Color32::WHITE, 0.15) };
    let ink = if alarm || on { Color32::WHITE } else { alpha(Color32::WHITE, 0.6) };
    let text = match filter {
        StatusFilter::All => if narrow { count.to_string() } else { format!("{} {}", tr("Todos"), count) },
        StatusFilter::Blocked => format!("! {count}"),
        _ => format!("■ {count}"),
    };
    let galley = ui.painter().layout_no_wrap(text.clone(), FontId::monospace(fs(10.0)), ink);
    let size = vec2(galley.size().x + if narrow { 6.0 } else { 10.0 }, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(3), fill);
    ui.painter().rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0_f32, edge), egui::StrokeKind::Inside);
    if matches!(filter, StatusFilter::Limited | StatusFilter::Working | StatusFilter::Idle | StatusFilter::Done) {
        // The square marker in the state's colour, then the count.
        let pad = if narrow { 3.0 } else { 5.0 };
        ui.painter().rect_filled(egui::Rect::from_center_size(pos2(rect.min.x + pad + 3.0, rect.center().y), vec2(6.0, 6.0)), CornerRadius::ZERO, color);
        ui.painter().text(pos2(rect.min.x + pad + 9.0, rect.center().y), Align2::LEFT_CENTER, count.to_string(), FontId::monospace(fs(10.0)), ink);
    } else {
        ui.painter().galley(pos2(rect.min.x + if narrow { 3.0 } else { 5.0 }, rect.center().y - galley.size().y / 2.0), galley, ink);
    }
    let response = response.on_hover_text(filter_name(filter));
    if response.clicked() {
        monitor.set_filter(if on && filter != StatusFilter::All { StatusFilter::All } else { filter });
    }
}

// ---- Chat panel

pub struct ChatState {
    pub agent_id: String,
    draft: String,
    lines: Vec<String>,
    note: Option<String>,
    sending: Option<Receiver<Option<String>>>,
    /// Option picked with ↑/↓ or a click, into `menu.options`.
    choice: Option<usize>,
    tail_rx: Option<Receiver<Vec<String>>>,
    next_tail: f64,
    focus: bool,
    status_seen: String,
    /// The terminal with its colours (full screen only), and its refresh.
    screen: Vec<Vec<Span>>,
    screen_rx: Option<Receiver<String>>,
    next_screen: f64,
    /// The harness's slash commands (read once), and the one highlighted
    /// in the "/" menu.
    slash: Option<Vec<crate::commands::SlashCommand>>,
    slash_choice: usize,
    /// The terminal's own width in columns (the widest line read), which sets the console's font.
    cols: usize,
}

impl ChatState {
    pub fn new(agent_id: &str) -> ChatState {
        ChatState { agent_id: agent_id.to_string(), draft: String::new(), lines: vec![], note: None, sending: None, choice: None, tail_rx: None, next_tail: 0.0, focus: true, status_seen: String::new(), screen: vec![], screen_rx: None, next_screen: 0.0, slash: None, slash_choice: 0, cols: 0 }
    }

    pub fn with_draft(mut self, draft: String) -> ChatState {
        self.draft = draft;
        self
    }

    fn menu(&self, agent: &Agent) -> MenuOptions {
        if agent.status == "blocked" { MenuOptions::new(&self.lines) } else { MenuOptions::default() }
    }

    /// Highlight an option and put its text in the box, ready to send with ⏎.
    fn pick(&mut self, menu: &MenuOptions, index: usize) {
        self.choice = Some(index);
        self.draft = menu.options[index].text.clone();
        self.focus = true;
    }

    fn run(&mut self, rx: Receiver<Option<String>>) {
        self.sending = Some(rx);
        self.note = None;
    }
}

pub struct ChatAction {
    pub close: bool,
    /// The console's text size: +1 bigger, -1 smaller.
    pub console_step: i8,
}

/// Opens under the rooms when one is clicked: the tail of the agent's
/// terminal, quick answers for a permission prompt, and a box to type to it.
/// The terminal is shown as it is, colours and spinners included, refreshed
/// twice a second; `wide` (full screen) sizes the font so its lines stay
/// whole. A question keeps the plain lines, whose options are clicked.
pub fn chat_panel(ui: &mut Ui, monitor: &Monitor, agent: &Agent, state: &mut ChatState, now: f64, wide: bool) -> ChatAction {
    let mut action = ChatAction { close: false, console_step: 0 };
    // Files dropped on the window go to the message as paths.
    let dropped = dropped_paths(ui);
    if !dropped.is_empty() { add_paths(&mut state.draft, &dropped); state.focus = true; }
    drop_hint(ui, ui.max_rect());
    // Keep the terminal tail fresh while the panel is open.
    if state.status_seen != agent.status { state.status_seen = agent.status.clone(); state.next_tail = 0.0; state.next_screen = 0.0; }
    if now >= state.next_tail && state.tail_rx.is_none() {
        state.tail_rx = Some(monitor.tail(agent, 30));
        state.next_tail = now + 2.0;
    }
    if now >= state.next_screen && state.screen_rx.is_none() {
        state.screen_rx = Some(monitor.screen(agent, wide));
        state.next_screen = now + 0.5;
    }
    if let Some(rx) = &state.screen_rx {
        if let Ok(text) = rx.try_recv() {
            let parsed = crate::ansi::parse(&text);
            if !parsed.iter().all(|l| l.is_empty()) || state.screen.is_empty() {
                // The lines a narrow pane wrapped are joined again, so the
                // console wraps them to its own width instead of showing a
                // thin column of text.
                state.cols = parsed.iter().map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>().trim_end().chars().count()).max().unwrap_or(0);
                state.screen = crate::ansi::reflow(parsed);
            }
            state.screen_rx = None;
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
    if let Some(rx) = &state.tail_rx {
        if let Ok(lines) = rx.try_recv() {
            state.lines = lines;
            state.tail_rx = None;
            if let Some(c) = state.choice { if c >= state.menu(agent).options.len() { state.choice = None; } }
        }
    }
    if let Some(rx) = &state.sending {
        if let Ok(failure) = rx.try_recv() {
            state.sending = None;
            state.note = failure;
            state.focus = true;
            state.next_tail = now + 0.25;
            state.tail_rx = None;
            state.next_screen = now + 0.3;
        }
    }
    let sending = state.sending.is_some();
    let menu = state.menu(agent);
    let color = agent_color(&agent.status);
    let standalone = monitor.standalone();

    // While the agent is asking something, ↑/↓ move its highlighted option
    // and ⏎ (with nothing typed) picks it, as in its own terminal.
    // The "/" menu under the box keeps ↑/↓ for itself.
    let slash_open = !slash_matches(state, agent).is_empty();
    let (up, down, escape) = ui.input_mut(|i| {
        let plain = i.modifiers.is_none() && !slash_open;
        (plain && i.consume_key(egui::Modifiers::NONE, Key::ArrowUp), plain && i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.key_pressed(Key::Escape))
    });
    // esc goes to the agent's terminal, as if typed there: it closes the
    // agent's own dialogs (/usage, /model…) and interrupts. The panel
    // closes with its x. A picker the agent opened itself takes ↑/↓ too.
    let picker = agent.status != "blocked" && !standalone && picker_open(&state.screen);
    if escape {
        if standalone { action.close = true; } else if !sending { state.run(monitor.press(&["esc".into()], agent)); }
    }
    if picker && (up || down) && !sending {
        state.run(monitor.press(&[if up { "up".to_string() } else { "down".to_string() }], agent));
    }
    if agent.status == "blocked" && (up || down) && !standalone {
        let delta: i64 = if up { -1 } else { 1 };
        if menu.is_menu() {
            // A menu driven by arrows: they go to the terminal, so its own ❯
            // moves; the highlight here follows it on the next read.
            if !sending {
                state.choice = None;
                state.run(monitor.press(&[if up { "up".to_string() } else { "down".to_string() }], agent));
            }
        } else if !menu.options.is_empty() {
            let start = state.choice.map(|c| c as i64).or(menu.highlighted.map(|h| h as i64)).unwrap_or(if delta > 0 { -1 } else { menu.options.len() as i64 });
            let index = (start + delta).clamp(0, menu.options.len() as i64 - 1) as usize;
            state.pick(&menu, index);
        } else if !sending {
            state.run(monitor.press(&[if up { "up".to_string() } else { "down".to_string() }], agent));
        }
    }

    // The panel fills what it is given: PANEL_HEIGHT under the rooms, the
    // whole side in full screen.
    let total = ui.available_height();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 7.0);
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, color);
            ui.label(mono(&agent.name, 12.0).strong().color(Color32::WHITE));
            let mut status = agent.label();
            if let Some(left) = agent.limit_left(crate::herdr::unix_now()).filter(|_| agent.status == "limited") {
                status += &format!(" · {}", trf("se reinicia en {}", &[&crate::scene::countdown(left)]));
            }
            ui.label(mono(&format!("{} · {}", agent.project, status), 11.0).color(DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if flat_button(ui, mono("x", 13.0).color(Color32::WHITE)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
                if !standalone && flat_button(ui, mono("↗", 13.0).color(Color32::WHITE)).on_hover_text(tr("Enfocar este agente en Herdr")).clicked() && !sending {
                    state.run(monitor.focus(agent));
                }
                if flat_button(ui, mono(">_", 11.0).strong().color(Color32::from_gray(200))).on_hover_text(tr("Abrir una terminal en esta carpeta")).clicked() {
                    crate::herdr::open_terminal(&agent.cwd);
                }
                // The console's own text size.
                if flat_button(ui, mono("A+", 11.0).color(Color32::from_gray(200))).on_hover_text(format!("{} (⌘⇧+)", tr("Consola más grande"))).clicked() { action.console_step = 1; }
                if flat_button(ui, mono("A-", 11.0).color(Color32::from_gray(200))).on_hover_text(format!("{} (⌘⇧-)", tr("Consola más pequeña"))).clicked() { action.console_step = -1; }
            });
        });
        let asking = agent.status == "blocked" && !state.lines.is_empty();
        // Full screen: the box to type in sits on the terminal's own prompt
        // line (❯, ›), where typing happens in the terminal; without one in
        // sight it stays under the console.
        let prompt_line = if wide && !asking && !standalone { prompt_line(&state.screen) } else { None };
        let inline = prompt_line.is_some();
        // The box under the console grows to three rows with what is typed.
        let box_height = input_height(draft_rows(&state.draft, (ui.available_width() / (fs(13.0) * 0.62)) as usize), fs(13.0), true) + 10.0;
        let body_height = total - 7.0 * 2.0 - 22.0 - if standalone { 14.0 } else if inline { 0.0 } else { box_height } - if state.note.is_some() { 16.0 } else { 0.0 };
        Frame::new().fill(alpha(Color32::BLACK, 0.45)).corner_radius(CornerRadius::same(5)).inner_margin(Margin::same(6)).show(ui, |ui| {
            ui.set_min_height(body_height.max(40.0));
            ui.set_max_height(body_height.max(40.0));
            ui.set_width(ui.available_width());
            // The screen as the terminal paints it, colours and all.
            if !asking && !state.screen.is_empty() {
                // Full screen: the terminal's width sets the font, so its
                // lines (rules, options) stay whole while the re-flowed
                // paragraphs wrap to the console. Then the console's own
                // size factor, apart from the panels' text size.
                let widest = state.cols.max(1) as f32;
                let base = if wide { (ui.available_width() / (widest * 0.62)).clamp(7.0, 12.0) } else { 11.0 };
                let size = (base * console_scale()).round().max(6.0);
                let screen = state.screen.clone();
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                    for (index, line) in screen.iter().enumerate() {
                        if prompt_line == Some(index) {
                            // The prompt's glyph, then the box in the terminal's own font.
                            // Top-aligned, the glyph lowered by the box's margin,
                            // so it sits on the typed text's line.
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                                if let Some(glyph) = line.iter().find(|s| !s.text.trim().is_empty()).map(|s| Span { text: s.text.trim_start().chars().take(1).collect(), ..s.clone() }) {
                                    ui.vertical(|ui| { ui.add_space(3.0); ui.add(screen_line(&[glyph], size)); });
                                }
                                input_row(ui, monitor, agent, state, &menu, sending, size, false);
                            });
                            continue;
                        }
                        // A rule the terminal's width: drawn to this width instead of wrapping.
                        let text: String = line.iter().map(|s| s.text.as_str()).collect();
                        let rule = text.trim();
                        if rule.chars().count() >= 8 && rule.chars().all(|c| matches!(c, '─' | '━' | '═' | '-' | '╌' | '┄')) {
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), size + 2.0), egui::Sense::hover());
                            let color = line.iter().find_map(|s| s.fg).unwrap_or(Color32::from_gray(100));
                            ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0_f32, color));
                        } else {
                            console_line(ui, line, size, &agent.cwd);
                        }
                    }
                });
                return;
            }
            // A question keeps its lines (the options are picked by line); a
            // plain tail is re-flowed into paragraphs that wrap to the panel.
            let shown: Vec<String> = if asking { state.lines.clone() } else { crate::herdr::paragraphs(&state.lines) };
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(!asking).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, if asking { 2.0 } else { 4.0 });
                if state.lines.is_empty() {
                    ui.label(mono(&tr("Leyendo la terminal…"), 11.0).color(DIM));
                }
                let mut picked: Option<usize> = None;
                for (i, line) in shown.iter().enumerate() {
                    let option = menu.options.iter().position(|o| o.line == i);
                    let highlighted = option.is_some() && option == state.choice.or(menu.highlighted);
                    let lead = asking && i == 0 && agent.question.is_some();
                    let text = mono(line, 11.0).color(if highlighted || lead { Color32::WHITE } else { Color32::from_gray(222) });
                    let text = if highlighted || lead { text.strong() } else { text };
                    let label = egui::Label::new(text).wrap();
                    let response = if option.is_some() {
                        let frame = Frame::new().fill(if highlighted { alpha(color, 0.35) } else { Color32::TRANSPARENT }).corner_radius(CornerRadius::same(3));
                        frame.show(ui, |ui| { ui.set_width(ui.available_width()); ui.add(label.sense(egui::Sense::click())) }).inner
                    } else {
                        ui.add(label)
                    };
                    if response.clicked() { picked = option; }
                }
                if let Some(index) = picked { state.pick(&menu, index); }
            });
        });
        if standalone {
            ui.label(mono(&tr("Instala Herdr para responder desde aquí."), 10.0).color(DIM));
            return;
        }
        if !inline { input_row(ui, monitor, agent, state, &menu, sending, fs(13.0), true); }
        if let Some(note) = &state.note {
            ui.label(RichText::new(note).size(fs(10.0)).color(Color32::from_rgb(255, 159, 10)));
        }
    });
    action
}

/// The box to type to the agent, with its send button and the buttons a
/// question or a run gets. `font` is its text size; `roomy` pads it as the
/// widget's own row (a line and a half tall).
fn input_row(ui: &mut Ui, monitor: &Monitor, agent: &Agent, state: &mut ChatState, menu: &MenuOptions, sending: bool, font: f32, roomy: bool) {
    // On the prompt line the box keeps to the top, level with the glyph,
    // instead of centring in a row taller than itself.
    ui.with_layout(egui::Layout::left_to_right(if roomy { egui::Align::Center } else { egui::Align::Min }), |ui| {
        let picker = agent.status != "blocked" && !monitor.standalone() && picker_open(&state.screen);
        let hint = if picker { tr("↑↓ y Enter eligen · esc cancela") } else if agent.status == "blocked" { trf("↑↓ y Enter eligen · o responde a {}…", &[&agent.name]) } else { trf("Escribir a {}…", &[&agent.name]) };
        let buttons_width = match agent.status.as_str() { "blocked" => 150.0, "working" => 80.0, _ => 26.0 };
        let mut draft = state.draft.clone();
        // The "/" menu: the harness's commands matching what is typed so
        // far. ↑↓ move, tab or ⏎ complete (⏎ then sends), a click too.
        let matches = slash_matches(state, agent);
        let menu_open = !matches.is_empty();
        let mut complete: Option<String> = None;
        if menu_open {
            state.slash_choice = state.slash_choice.min(matches.len() - 1);
            let (up, down, tab) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, Key::Tab)));
            if up { state.slash_choice = state.slash_choice.saturating_sub(1); }
            if down { state.slash_choice = (state.slash_choice + 1).min(matches.len() - 1); }
            if tab { complete = Some(matches[state.slash_choice].0.clone()); }
        } else {
            state.slash_choice = 0;
        }
        // Roomy: the box is a line and a half tall, so typing does not feel cramped.
        let margin = if roomy { Margin::symmetric(8, 8) } else { Margin::symmetric(6, 3) };
        // A message longer than a line grows the box to three rows, then
        // scrolls; ⏎ sends and ⇧⏎ breaks the line.
        let width = ui.available_width() - buttons_width;
        let rows = draft_rows(&draft, (width / (font * 0.62)) as usize);
        // A fixed id per agent: the box moves with the terminal's prompt line
        // as new lines arrive, and an id taken from its position would change
        // and drop the focus.
        let edit = egui::TextEdit::multiline(&mut draft)
            .id(egui::Id::new(("chat-input", agent.id.as_str())))
            .hint_text(hint)
            .font(FontId::monospace(font))
            .margin(margin)
            .desired_width(width)
            .desired_rows(1)
            .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, Key::Enter))
            .interactive(!sending);
        let response = ui.scope(|ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            egui::ScrollArea::vertical().id_salt(("draft", agent.id.as_str())).max_height(input_height(rows, font, roomy) - 4.0).auto_shrink([false, true]).show(ui, |ui| ui.add(edit)).inner
        }).inner;
        if state.focus { response.request_focus(); state.focus = false; }
        let submitted = response.has_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        if menu_open {
            let chosen = &matches[state.slash_choice].0;
            if submitted && draft.trim() != format!("/{chosen}") { complete = Some(chosen.clone()); }
            let width = response.rect.width().min(560.0);
            egui::Area::new(ui.id().with("slash-menu")).order(egui::Order::Foreground).pivot(Align2::LEFT_BOTTOM).fixed_pos(response.rect.left_top() - vec2(0.0, 4.0)).show(ui.ctx(), |ui| {
                Frame::new().fill(Color32::from_rgb(28, 30, 36)).stroke(Stroke::new(1.0_f32, Color32::from_gray(80))).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(6, 6)).show(ui, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                    for (i, (name, about)) in matches.iter().take(12).enumerate() {
                        let picked = i == state.slash_choice;
                        let fill = if picked { alpha(Color32::from_rgb(140, 217, 115), 0.25) } else { Color32::TRANSPARENT };
                        let row = Frame::new().fill(fill).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(6, 3)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                                ui.label(mono(&format!("/{name}"), 12.0).color(Color32::WHITE).strong());
                                ui.add(egui::Label::new(RichText::new(about).size(fs(11.0)).color(DIM)).truncate());
                            });
                        }).response.interact(egui::Sense::click());
                        if row.clicked() { complete = Some(name.clone()); }
                    }
                    if matches.len() > 12 { ui.label(mono(&format!("… +{}", matches.len() - 12), 10.0).color(DIM)); }
                });
            });
        }
        if let Some(name) = complete {
            draft = format!("/{name} ");
            state.focus = true;
            ui.ctx().request_repaint();
        }
        let completing = menu_open && draft.ends_with(' ') && submitted;
        let empty = draft.trim().is_empty();
        state.draft = draft;
        if submitted && !sending && !completing {
            if (agent.status == "blocked" || picker) && empty {
                // Nothing typed: ⏎ picks the option its terminal already highlights.
                state.run(monitor.press(&["enter".into()], agent));
            } else {
                submit(state, monitor, agent, menu);
            }
            state.focus = true;
        }
        if ui.add_enabled(!sending && !empty, egui::Button::new(mono("→", 13.0)).frame(false)).on_hover_text(tr("Enviar (⏎)")).clicked() {
            submit(state, monitor, agent, menu);
        }
        if agent.status == "blocked" {
            if ui.add_enabled(!sending, egui::Button::new(mono(&tr("Aceptar"), 11.0))).on_hover_text(tr("Elegir la opción marcada del agente (⏎ en su terminal)")).clicked() {
                match state.choice { Some(choice) => answer(state, monitor, agent, &menu, choice), None => state.run(monitor.press(&["enter".into()], agent)) }
            }
            if ui.add_enabled(!sending, egui::Button::new(mono(&tr("Rechazar"), 11.0))).on_hover_text(tr("Cancelar la petición del agente (esc en su terminal)")).clicked() {
                state.run(monitor.press(&["esc".into()], agent));
            }
        } else if agent.status == "working" {
            if ui.add_enabled(!sending, egui::Button::new(mono(&tr("Detener"), 11.0))).on_hover_text(tr("Interrumpir al agente (esc en su terminal)")).clicked() {
                state.run(monitor.press(&["esc".into()], agent));
            }
        }
    });
}

/// The harness's "/" commands matching what is typed, with what each does.
fn slash_matches(state: &mut ChatState, agent: &Agent) -> Vec<(String, String)> {
    if agent.status == "blocked" || !state.draft.starts_with('/') { return vec![]; }
    let commands = state.slash.get_or_insert_with(|| crate::commands::slash_commands(crate::herdr::harness_for(&agent.name), &agent.cwd, &crate::herdr::home_dir()));
    crate::commands::matching(commands, &state.draft).into_iter().map(|c| (c.name.clone(), c.about.clone())).collect()
}

/// How many rows the box needs for what is typed, up to three: its lines,
/// each wrapped to `cols` columns.
pub(crate) fn draft_rows(draft: &str, cols: usize) -> usize {
    let cols = cols.max(8);
    draft.split('\n').map(|l| l.chars().count().max(1).div_ceil(cols)).sum::<usize>().clamp(1, 3)
}

/// The height of the box with `rows` rows, margins included.
fn input_height(rows: usize, font: f32, roomy: bool) -> f32 {
    let margin = if roomy { 16.0 } else { 6.0 };
    font * 1.3 * rows as f32 + margin + 4.0
}

/// The last line starting with ❯ (Claude Code), › (Kiro, Codex) or >, and
/// whether it is a numbered option ("❯ 2. Opus 5.5"): the highlight of a
/// picker such as /model rather than a prompt.
/// A line starting with the prompt's marker: whether it is a numbered
/// option, and whether nothing follows the marker (an empty prompt).
fn marked(line: &[Span]) -> Option<(bool, bool)> {
    let text: String = line.iter().map(|s| s.text.as_str()).collect();
    let text = text.trim();
    let mut chars = text.chars();
    let marked = matches!(chars.next(), Some('❯' | '›' | '>')) && matches!(chars.next(), None | Some(' ' | '\u{a0}'));
    marked.then(|| (crate::herdr::is_option(text), chars.as_str().trim().is_empty()))
}

/// A narrow pane keeps the empty prompt in sight under a picker's options:
/// the picker still takes the keys, so an empty prompt counts as the picker
/// when one is open just above it (a highlighted option, or its footer).
fn last_marked_line(screen: &[Vec<Span>]) -> Option<(usize, bool)> {
    let (index, option, bare) = screen.iter().enumerate().rev().find_map(|(i, l)| marked(l).map(|(o, b)| (i, o, b)))?;
    if option || !bare { return Some((index, option)); }
    // The nearest line with text above the prompt's box (past its top rule
    // and blank lines) is the picker's footer or one of its options.
    let nearest = screen[..index].iter().rev().map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>()).find(|t| {
        let t = t.trim();
        !t.is_empty() && !t.chars().all(|c| matches!(c, '─' | '━' | '═' | '╌' | '┄' | '-'))
    });
    let picker = nearest.map(|t| crate::herdr::is_menu_footer(t.trim()) || (t.trim_start().starts_with('❯') && crate::herdr::is_option(t.trim()))).unwrap_or(false);
    Some((index, picker))
}

/// The terminal's prompt line, where the agent takes typing. None while a
/// picker is open: its highlighted option is not a prompt.
pub(crate) fn prompt_line(screen: &[Vec<Span>]) -> Option<usize> {
    last_marked_line(screen).filter(|(_, option)| !option).map(|(index, _)| index)
}

/// Whether the terminal shows a picker driven by arrow keys (/model,
/// /resume…) though the agent is not asking anything.
pub(crate) fn picker_open(screen: &[Vec<Span>]) -> bool {
    last_marked_line(screen).map(|(_, option)| option).unwrap_or(false)
}

fn submit(state: &mut ChatState, monitor: &Monitor, agent: &Agent, menu: &MenuOptions) {
    if let Some(choice) = state.choice {
        if menu.options.get(choice).map(|o| o.text == state.draft).unwrap_or(false) {
            return answer(state, monitor, agent, menu, choice);
        }
    }
    state.choice = None;
    submit_text(state, monitor, agent);
}

fn submit_text(state: &mut ChatState, monitor: &Monitor, agent: &Agent) {
    let text = state.draft.clone();
    if state.sending.is_some() || text.trim().is_empty() { return; }
    state.run(monitor.send(&text, agent));
    state.draft.clear();
}

/// Answer with an option: a terminal menu gets the arrows that reach it
/// and ⏎; a question asked in plain text gets the option's text.
fn answer(state: &mut ChatState, monitor: &Monitor, agent: &Agent, menu: &MenuOptions, index: usize) {
    if index >= menu.options.len() { return; }
    state.choice = None;
    if menu.is_menu() {
        state.run(monitor.press(&menu.keys_choosing(index), agent));
        state.draft.clear();
    } else {
        state.draft = menu.options[index].text.clone();
        submit_text(state, monitor, agent);
    }
}

// ---- New agent panel

#[derive(Default)]
pub struct ComposeState {
    pub prompt: String,
    busy: Option<Receiver<Option<String>>>,
    note: Option<String>,
    /// The harnesses installed here, looked up once.
    installed: Option<Vec<&'static str>>,
}

pub struct ComposeAction {
    pub close: bool,
}

/// Opens under the rooms from the + button: pick a harness, a folder (one
/// of the agents' or any other) and an optional first prompt, and Herdr
/// starts the agent in a new workspace there.
pub fn compose_panel(ui: &mut Ui, monitor: &Monitor, prefs: &mut Prefs, state: &mut ComposeState) -> ComposeAction {
    let dropped = dropped_paths(ui);
    if !dropped.is_empty() { add_paths(&mut state.prompt, &dropped); }
    drop_hint(ui, ui.max_rect());
    let mut action = ComposeAction { close: false };
    let (busy, known) = compose_prelude(monitor, prefs, state, &mut action);
    if ui.input(|i| i.key_pressed(Key::Escape)) { action.close = true; }
    let green = Color32::from_rgb(140, 217, 115);
    let total = ui.available_height();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 7.0);
        ui.horizontal(|ui| {
            ui.label(mono("+", 14.0).color(green));
            ui.label(mono(&tr("Invocar un agente"), 12.0).strong().color(Color32::WHITE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if flat_button(ui, mono("x", 13.0).color(Color32::WHITE)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
            });
        });
        ui.horizontal(|ui| {
            let mut kind = prefs.create_kind.clone();
            let kinds = state.installed.clone().unwrap_or_default();
            egui::ComboBox::from_id_salt("kind").selected_text(mono(&kind, 11.0)).width(96.0).show_ui(ui, |ui| {
                for option in kinds { ui.selectable_value(&mut kind, option.to_string(), option); }
            });
            if kind != prefs.create_kind { prefs.create_kind = kind; prefs.save(); }
            folder_picker(ui, prefs, &known, 11.0);
        });
        Frame::new().fill(alpha(Color32::BLACK, 0.45)).corner_radius(CornerRadius::same(5)).inner_margin(Margin::same(3)).show(ui, |ui| {
            let height = total - 7.0 * 3.0 - 22.0 - 24.0 - 24.0;
            ui.set_width(ui.available_width());
            let edit = egui::TextEdit::multiline(&mut state.prompt).hint_text(tr("Primer prompt (opcional)")).font(FontId::monospace(fs(11.0))).desired_rows(4).frame(false).desired_width(f32::INFINITY).interactive(!busy);
            ui.add_sized(vec2(ui.available_width(), height.max(40.0)), edit);
        });
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
                ui.label(RichText::new(tr("Invocando…")).size(fs(10.0)).color(DIM));
            } else if let Some(note) = &state.note {
                ui.label(RichText::new(note).size(fs(10.0)).color(Color32::from_rgb(255, 159, 10)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
                let can = !busy && !prefs.create_folder.is_empty();
                if (ui.add_enabled(can, egui::Button::new(mono(&tr("Invocar"), 11.0))).on_hover_text(tr("Crear el agente (⌘⏎)")).clicked() || (shortcut && can)) && state.busy.is_none() {
                    state.note = None;
                    state.busy = Some(monitor.create(&prefs.create_kind, &prefs.create_folder, &state.prompt));
                }
            });
        });
    });
    action
}

/// What both summon forms do first: take a finished summon's outcome, look
/// up the installed harnesses (keeping the chosen one among them) and list
/// the folders the agents work in, defaulting the folder to a favourite or
/// the first of them.
fn compose_prelude(monitor: &Monitor, prefs: &mut Prefs, state: &mut ComposeState, action: &mut ComposeAction) -> (bool, Vec<String>) {
    if let Some(rx) = &state.busy {
        if let Ok(failure) = rx.try_recv() {
            state.busy = None;
            match failure {
                Some(failure) => state.note = Some(failure),
                None => { state.prompt.clear(); action.close = true; }
            }
        }
    }
    let kinds = state.installed.get_or_insert_with(crate::herdr::installed_kinds);
    if !kinds.contains(&prefs.create_kind.as_str()) { prefs.create_kind = kinds[0].to_string(); prefs.save(); }
    // Folders the current agents work in, most common first.
    let mut known: Vec<String> = vec![];
    for agent in &monitor.agents {
        let path = crate::herdr::expand_home(&agent.cwd);
        if !known.contains(&path) { known.push(path); }
    }
    if prefs.create_folder.is_empty() {
        if let Some(first) = prefs.favorite_folders.first().or(known.first()).cloned() { prefs.create_folder = first; prefs.save(); }
    }
    (state.busy.is_some(), known)
}

/// The folder to summon in: favourites first (★), then the agents' folders,
/// then any other; the star beside it adds or removes the chosen one.
fn folder_picker(ui: &mut Ui, prefs: &mut Prefs, known: &[String], size: f32) {
    let starred = prefs.favorite_folders.contains(&prefs.create_folder);
    let star = if starred { "★" } else { "☆" };
    let star_width = 18.0;
    let shown = if prefs.create_folder.is_empty() { tr("Carpeta…") } else { crate::herdr::abbreviate_home(&prefs.create_folder) };
    let mut folder = prefs.create_folder.clone();
    let mut choose = false;
    egui::ComboBox::from_id_salt("folder").selected_text(mono(&shown, size)).truncate().width(ui.available_width() - star_width - 14.0).show_ui(ui, |ui| {
        for path in &prefs.favorite_folders { ui.selectable_value(&mut folder, path.clone(), format!("★ {}", crate::herdr::abbreviate_home(path))); }
        if !prefs.favorite_folders.is_empty() { ui.separator(); }
        for path in known.iter().filter(|p| !prefs.favorite_folders.contains(p)) { ui.selectable_value(&mut folder, path.clone(), crate::herdr::abbreviate_home(path)); }
        if !known.is_empty() { ui.separator(); }
        if ui.button(tr("Elegir carpeta…")).clicked() { choose = true; }
    });
    let hint = if starred { tr("Quitar de favoritas") } else { tr("Añadir a favoritas") };
    let gold = Color32::from_rgb(255, 204, 0);
    if ui.add_enabled(!prefs.create_folder.is_empty(), egui::Button::new(mono(star, size + 2.0).color(if starred { gold } else { DIM })).frame(false)).on_hover_text(hint).clicked() {
        if starred { prefs.favorite_folders.retain(|f| f != &prefs.create_folder); } else { prefs.favorite_folders.push(prefs.create_folder.clone()); }
        prefs.save();
    }
    if choose {
        let mut dialog = rfd::FileDialog::new();
        if !folder.is_empty() { dialog = dialog.set_directory(&folder); }
        if let Some(picked) = dialog.pick_folder() { folder = picked.to_string_lossy().to_string(); }
    }
    if folder != prefs.create_folder { prefs.create_folder = folder; prefs.save(); }
}

/// Full screen: the summon form inside the spare "+" room itself. The
/// installed harnesses as heroes to pick, the folder (favourites first), an
/// optional first prompt and the button.
pub fn summon_form(ui: &mut Ui, monitor: &Monitor, assets: &crate::assets::Assets, prefs: &mut Prefs, state: &mut ComposeState, now: f64) -> ComposeAction {
    let mut action = ComposeAction { close: false };
    let (busy, known) = compose_prelude(monitor, prefs, state, &mut action);
    if ui.input(|i| i.key_pressed(Key::Escape)) { action.close = true; }
    let green = Color32::from_rgb(140, 217, 115);
    ui.spacing_mut().item_spacing = vec2(4.0, 5.0);
    ui.horizontal(|ui| {
        ui.label(mono("+", 12.0).color(green));
        ui.label(mono(&tr("Invocar un agente"), 11.0).strong().color(Color32::WHITE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if flat_button(ui, mono("x", 12.0).color(Color32::WHITE)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
        });
    });
    // The installed harnesses, as their heroes: the chosen one walks.
    let kinds = state.installed.clone().unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
        for kind in kinds {
            let chosen = prefs.create_kind == kind;
            let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::click());
            let fill = if chosen { alpha(green, 0.25) } else if response.hovered() { alpha(Color32::WHITE, 0.08) } else { Color32::TRANSPARENT };
            ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
            if chosen { ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, green), egui::StrokeKind::Inside); }
            let frame = if chosen { (now * 3.0) as usize } else { 0 };
            let hero = crate::herdr::HERO_ORDER.iter().find(|h| **h == kind).copied().unwrap_or("hero");
            crate::scene::draw_sprite(ui.painter(), assets.hero_frame(hero, if chosen { "work" } else { "idle" }, frame), rect.center(), vec2(20.0, 20.0), false, 0.0, if chosen || response.hovered() { Color32::WHITE } else { Color32::from_gray(150) });
            if response.on_hover_text(kind).clicked() { prefs.create_kind = kind.to_string(); prefs.save(); }
        }
    });
    ui.label(mono(&prefs.create_kind, 10.0).strong().color(green));
    ui.horizontal(|ui| folder_picker(ui, prefs, &known, 10.0));
    let edit = egui::TextEdit::multiline(&mut state.prompt).hint_text(tr("Primer prompt (opcional)")).font(FontId::monospace(fs(10.0))).desired_rows(2).desired_width(f32::INFINITY).interactive(!busy);
    ui.add(edit);
    ui.horizontal(|ui| {
        if busy {
            ui.spinner();
        } else if let Some(note) = &state.note {
            ui.add(egui::Label::new(RichText::new(note).size(fs(9.0)).color(Color32::from_rgb(255, 159, 10))).truncate());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
            let can = !busy && !prefs.create_folder.is_empty();
            if (ui.add_enabled(can, egui::Button::new(mono(&tr("Invocar"), 11.0).color(Color32::WHITE)).fill(alpha(green, 0.35))).on_hover_text(tr("Crear el agente (⌘⏎)")).clicked() || (shortcut && can)) && state.busy.is_none() {
                state.note = None;
                state.busy = Some(monitor.create(&prefs.create_kind, &prefs.create_folder, &state.prompt));
            }
        });
    });
    action
}

/// The console side in full screen while no room is chosen.
pub fn console_placeholder(ui: &mut Ui) {
    let rect = ui.available_rect_before_wrap();
    let center = rect.center();
    ui.painter().text(center - vec2(0.0, 10.0), Align2::CENTER_CENTER, tr("Elige una sala para ver su consola"), FontId::monospace(fs(12.0)), DIM);
    ui.painter().text(center + vec2(0.0, 10.0), Align2::CENTER_CENTER, tr("esc vuelve a la ventana"), FontId::monospace(fs(10.0)), alpha(DIM, 0.6));
}

// ---- Activity log

/// The guild's activity log on a parchment: who joined or left, state
/// changes, subagents and messages you sent, newest first, with the time.
/// Returns true when its close button was clicked.
pub fn activity_log(ctx: &Context, events: &[GuildEvent]) -> bool {
    let ink = Color32::from_rgb(61, 38, 20);
    let mut close = false;
    egui::Area::new(egui::Id::new("activity-log")).anchor(Align2::RIGHT_TOP, vec2(-8.0, 38.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        Frame::new()
            .fill(Color32::from_rgb(237, 217, 168))
            .stroke(Stroke::new(2.0_f32, Color32::from_rgb(115, 74, 36)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(10))
            .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: alpha(Color32::BLACK, 0.5) })
            .show(ui, |ui| {
                ui.set_width(260.0);
                ui.spacing_mut().item_spacing = vec2(5.0, 6.0);
                ui.horizontal(|ui| {
                    ui.label(mono(&tr("Registro de la guild"), 12.0).strong().color(ink));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if flat_button(ui, mono("x", 11.0).strong().color(ink)).on_hover_text(tr("Cerrar")).clicked() { close = true; }
                    });
                });
                let line = ui.available_rect_before_wrap();
                ui.painter().hline(line.x_range(), line.min.y, Stroke::new(1.0_f32, alpha(ink, 0.35)));
                ui.add_space(2.0);
                if events.is_empty() {
                    ui.label(mono(&tr("Aún no pasa nada."), 10.0).color(alpha(ink, 0.7)));
                } else {
                    egui::ScrollArea::vertical().max_height(200.0).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing = vec2(5.0, 4.0);
                        for event in events {
                            ui.horizontal_top(|ui| {
                                ui.label(mono(&event.at.format("%H:%M:%S").to_string(), 10.0).color(alpha(ink, 0.6)));
                                let (dot, _) = ui.allocate_exact_size(vec2(6.0, 12.0), egui::Sense::hover());
                                ui.painter().rect_filled(egui::Rect::from_center_size(pos2(dot.center().x, dot.center().y + 1.0), vec2(6.0, 6.0)), CornerRadius::ZERO, tone_color(event.tone));
                                ui.add(egui::Label::new(mono(&event.text, 10.0).color(ink)).wrap());
                            });
                        }
                    });
                }
            });
    });
    close
}

fn tone_color(tone: &str) -> Color32 {
    match tone {
        "blocked" => Color32::RED,
        "limited" => crate::scene::LIMITED,
        "working" => Color32::from_rgb(255, 159, 10),
        "done" | "joined" => Color32::from_rgb(38, 153, 64),
        "left" | "idle" => Color32::GRAY,
        "subagents" => Color32::from_rgb(160, 60, 180),
        "message" => Color32::from_rgb(40, 110, 230),
        _ => Color32::from_rgb(115, 74, 36),
    }
}

// ---- Connection banner

/// A pixel banner over the rooms while Herdr is unreachable or the data is
/// stale; reconnection keeps being retried in the background.
pub fn connection_banner(ctx: &Context, text: &str, lost: bool) {
    egui::Area::new(egui::Id::new("connection")).anchor(Align2::CENTER_TOP, vec2(0.0, 16.0)).order(egui::Order::Foreground).interactable(false).show(ctx, |ui| {
        let fill = if lost { Color32::from_rgb(153, 31, 36) } else { Color32::from_rgb(140, 92, 20) };
        Frame::new().fill(alpha(fill, 0.95)).stroke(Stroke::new(2.0_f32, alpha(Color32::BLACK, 0.6))).corner_radius(CornerRadius::same(3)).inner_margin(Margin::symmetric(8, 3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(mono(if lost { "⚡" } else { "⌛" }, 11.0).color(Color32::WHITE));
                ui.label(mono(text, 10.0).strong().color(Color32::WHITE));
            });
        })
        .response
        .on_hover_text(if lost { tr("Reintentando la conexión automáticamente.") } else { tr("Herdr no ha entregado datos nuevos.") });
    });
}

// ---- Standalone banner

pub struct HerdrBannerAction {
    pub install: bool,
    pub open: bool,
    pub dismiss: bool,
}

/// Over the rooms while Herdr is missing or down: what that means, and the
/// buttons to install it, open it, or carry on with the read-only viewer.
pub fn herdr_banner(ctx: &Context, missing: bool, installing: bool, note: Option<&str>) -> HerdrBannerAction {
    let mut action = HerdrBannerAction { install: false, open: false, dismiss: false };
    egui::Area::new(egui::Id::new("herdr-banner")).anchor(Align2::CENTER_TOP, vec2(0.0, 16.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        Frame::new().fill(alpha(Color32::from_rgb(40, 36, 52), 0.96)).stroke(Stroke::new(2.0_f32, alpha(Color32::BLACK, 0.6))).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
                let text = if missing { tr("Herdr no está instalado · modo autónomo, solo lectura") } else { tr("Herdr no está corriendo · modo autónomo, solo lectura") };
                ui.label(mono(&text, 10.0).strong().color(Color32::WHITE)).on_hover_text(tr("Los agentes se ven, pero no se les puede hablar: Herdr no está."));
                if let Some(note) = note { ui.label(mono(note, 10.0).color(Color32::from_rgb(255, 159, 10))); }
                ui.horizontal(|ui| {
                    if installing {
                        ui.spinner();
                        ui.label(mono(&tr("Instalando Herdr…"), 10.0).color(DIM));
                    } else if missing {
                        if ui.button(mono(&tr("Instalar Herdr"), 10.0)).clicked() { action.install = true; }
                    } else if ui.button(mono(&tr("Abrir Herdr"), 10.0)).clicked() { action.open = true; }
                    if ui.button(mono(&tr("Seguir sin Herdr"), 10.0)).clicked() { action.dismiss = true; }
                });
            });
        });
    });
    action
}

// ---- Controls, drag handle, grip

pub struct ControlAction {
    pub compose: bool,
    pub log: bool,
    pub sound: bool,
    pub fullscreen: bool,
    pub minimize: bool,
    pub close: bool,
}

/// The keys that toggle full screen, for the tooltips.
pub fn fullscreen_keys() -> &'static str {
    if cfg!(target_os = "macos") { "⌃⌘F" } else { "F11" }
}

/// The buttons that appear at the top-right on hover.
pub fn controls(ctx: &Context, sounds_on: bool, log_open: bool, fullscreen: bool) -> ControlAction {
    let mut action = ControlAction { compose: false, log: false, sound: false, fullscreen: false, minimize: false, close: false };
    egui::Area::new(egui::Id::new("controls")).anchor(Align2::RIGHT_TOP, vec2(-6.0, 6.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        Frame::new().fill(alpha(Color32::BLACK, 0.7)).corner_radius(CornerRadius::same(14)).inner_margin(Margin::symmetric(9, 4)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                if flat_button(ui, mono("+", 19.0).strong().color(Color32::from_rgb(140, 217, 115))).on_hover_text(tr("Invocar un agente nuevo")).clicked() { action.compose = true; }
                if flat_button(ui, mono("📜", 16.0).color(alpha(Color32::from_rgb(237, 214, 158), if log_open { 1.0 } else { 0.85 }))).on_hover_text(tr("Registro de actividad")).clicked() { action.log = true; }
                let speaker = if sounds_on { "🔊" } else { "🔇" };
                if flat_button(ui, mono(speaker, 16.0).color(alpha(Color32::WHITE, if sounds_on { 0.9 } else { 0.5 }))).on_hover_text(if sounds_on { tr("Silenciar sonidos") } else { tr("Activar sonidos") }).clicked() { action.sound = true; }
                let hint = format!("{} · {}", if fullscreen { tr("Salir de pantalla completa") } else { tr("Pantalla completa") }, fullscreen_keys());
                if flat_button(ui, mono(if fullscreen { "⤡" } else { "⤢" }, 17.0).strong().color(alpha(Color32::WHITE, 0.9))).on_hover_text(hint).clicked() { action.fullscreen = true; }
                if flat_button(ui, mono("–", 19.0).strong().color(Color32::from_rgb(250, 189, 46))).on_hover_text(tr("Minimizar (ocultar)")).clicked() { action.minimize = true; }
                if flat_button(ui, mono("x", 17.0).strong().color(Color32::from_rgb(242, 84, 77))).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
            });
        });
    });
    action
}

/// Grab bar centred at the top of the widget: dragging it moves the window.
pub fn drag_handle(ctx: &Context) {
    egui::Area::new(egui::Id::new("drag-handle")).anchor(Align2::CENTER_TOP, Vec2::ZERO).order(egui::Order::Foreground).show(ctx, |ui| {
        let (rect, response) = ui.allocate_exact_size(vec2(160.0, 14.0), egui::Sense::drag());
        let hovering = response.hovered() || response.dragged();
        let width = if hovering { 64.0 } else { 48.0 };
        let bar = egui::Rect::from_center_size(pos2(rect.center().x, rect.min.y + 7.0), vec2(width, 5.0));
        ui.painter().rect_filled(bar.translate(vec2(0.0, 1.0)), CornerRadius::same(3), alpha(Color32::BLACK, 0.6));
        ui.painter().rect_filled(bar, CornerRadius::same(3), alpha(Color32::WHITE, if hovering { 0.75 } else { 0.35 }));
        if response.drag_started() { ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); }
        response.on_hover_text(tr("Arrastrar para mover"));
    });
}

/// Pixel grip in the bottom-right corner: drag it to resize the widget.
pub fn resize_grip(ctx: &Context, visible: bool) -> bool {
    let mut started = false;
    egui::Area::new(egui::Id::new("resize-grip")).anchor(Align2::RIGHT_BOTTOM, vec2(-2.0, -2.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        let (rect, response) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::drag());
        if visible || response.hovered() {
            // Three steps of pixels down the diagonal.
            for (x, y) in [(10.0, 2.0), (6.0, 2.0), (10.0, 6.0), (2.0, 2.0), (6.0, 6.0), (10.0, 10.0)] {
                ui.painter().rect_filled(egui::Rect::from_min_size(pos2(rect.min.x + x, rect.max.y - y - 2.0), vec2(2.0, 2.0)), CornerRadius::ZERO, alpha(Color32::WHITE, 0.55));
            }
        }
        if response.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(egui::ResizeDirection::SouthEast));
            started = true;
        }
    });
    started
}

// ---- HP viewer (full screen)

/// The HP viewer's height for these harnesses: a title and one row each.
pub fn usage_height(usages: &[crate::usage::HarnessUsage]) -> f32 {
    if usages.is_empty() { 0.0 } else { 26.0 + usages.len() as f32 * 60.0 }
}

fn hp_color(hp: f32) -> Color32 {
    if hp > 50.0 { Color32::from_rgb(48, 209, 88) } else if hp > 20.0 { Color32::from_rgb(255, 159, 10) } else { Color32::from_rgb(255, 69, 58) }
}

/// A segmented bar, as in a game's HUD: `hp` (0–100) of it lit.
fn hp_bar(ui: &mut Ui, hp: f32, height: f32, segments: usize, fill: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(1), Color32::from_rgb(40, 34, 46));
    let lit = rect.width() * hp / 100.0;
    if lit > 0.0 { painter.rect_filled(egui::Rect::from_min_size(rect.min, vec2(lit, height)), CornerRadius::same(1), fill); }
    // The top highlight and the cuts between segments.
    if lit > 0.0 && height >= 6.0 { painter.hline(rect.min.x..=rect.min.x + lit, rect.min.y + 1.0, Stroke::new(1.0_f32, alpha(Color32::WHITE, 0.35))); }
    for i in 1..segments {
        let x = rect.min.x + rect.width() * i as f32 / segments as f32;
        painter.vline(x, rect.y_range(), Stroke::new(1.0_f32, Color32::from_rgb(19, 17, 23)));
    }
    painter.rect_stroke(rect, CornerRadius::same(1), Stroke::new(1.0_f32, Color32::from_rgb(70, 60, 80)), egui::StrokeKind::Outside);
}

/// Each harness's plan left, as its hero's stats: HP is the week (Kiro's
/// month of credits), mana the session (the 5-hour window), refilling at
/// its countdown. Returns a Kiro plan the user picked.
/// What was changed in the HP viewer: Kiro's plan, Claude's source.
#[derive(Default)]
pub struct UsagePick {
    pub kiro_plan: Option<u32>,
    pub claude_live: Option<bool>,
}

pub fn usage_panel(ui: &mut Ui, assets: &crate::assets::Assets, usages: &[crate::usage::HarnessUsage], now: f64, claude_live: bool) -> UsagePick {
    let mut picked = UsagePick::default();
    let unix = crate::herdr::unix_now();
    let mana_color = Color32::from_rgb(90, 150, 255);
    ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
    ui.add_space(5.0);
    ui.label(mono(&tr("HP de la guild (uso restante)"), 10.0).strong().color(DIM));
    for usage in usages {
        let name = match usage.harness { "claude" => "Claude", "codex" => "Codex", "kiro" => "Kiro", other => other };
        let health = usage.meters.iter().find(|m| m.window != "5h");
        let mana = usage.meters.iter().find(|m| m.window == "5h");
        let hp = health.map(|m| m.hp(unix));
        ui.horizontal(|ui| {
            let (icon, _) = ui.allocate_exact_size(vec2(28.0, 40.0), egui::Sense::hover());
            // Out of HP or mana, the hero stops and greys out.
            let spent = hp == Some(0.0) || mana.map(|m| m.hp(unix) <= 0.0).unwrap_or(false);
            let frame = if spent { 0 } else { (now * 2.0) as usize };
            let tint = if spent { Color32::from_gray(110) } else { Color32::WHITE };
            crate::scene::draw_sprite(ui.painter(), assets.hero_frame(usage.harness, "idle", frame), icon.center(), vec2(24.0, 24.0), false, 0.0, tint);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
                let row = ui.horizontal(|ui| {
                    ui.label(mono(name, 11.0).strong().color(Color32::WHITE));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let text = match (health, &usage.credits) {
                            (Some(_), Some((spent, Some(plan)))) => format!("HP {:.0}/{plan}", (*plan as f64 - spent).max(0.0)),
                            (Some(m), _) if m.hp(unix) <= 0.0 => "KO".to_string(),
                            (Some(m), _) => format!("HP {:.0}%", m.hp(unix)),
                            (None, Some((spent, None))) => trf("{} créditos", &[&format!("{spent:.0}")]),
                            _ => "—".to_string(),
                        };
                        ui.label(mono(&text, 10.0).strong().color(hp.map(hp_color).unwrap_or(DIM)));
                    });
                }).response;
                if let Some(hp) = hp { hp_bar(ui, hp, 7.0, 10, hp_color(hp)); }
                if let Some(mana) = mana {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(5.0, 0.0);
                        ui.label(mono("MP", 9.0).strong().color(mana_color));
                        let left = mana.hp(unix);
                        ui.label(mono(&format!("{left:.0}%"), 9.0).color(DIM));
                        ui.add_space(2.0);
                        hp_bar(ui, left, 4.0, 5, mana_color);
                    });
                } else if usage.harness == "kiro" {
                    ui.horizontal(|ui| {
                        let plan = usage.credits.and_then(|c| c.1);
                        let shown = plan.and_then(|p| crate::usage::KIRO_PLANS.iter().find(|(_, c)| *c == p)).map(|(n, _)| n.to_string()).unwrap_or_else(|| tr("Elegir plan…"));
                        egui::ComboBox::from_id_salt("kiro-plan").selected_text(mono(&shown, 9.0)).width(70.0).show_ui(ui, |ui| {
                            for (label, credits) in crate::usage::KIRO_PLANS {
                                if ui.selectable_label(plan == Some(credits), format!("{label} · {credits}")).clicked() { picked.kiro_plan = Some(credits); }
                            }
                        });
                        // The month's credits come back on the 1st.
                        if let Some(at) = health.filter(|m| m.used > 0.0).and_then(|m| m.resets).and_then(|r| crate::usage::reset_label(r, chrono::Local::now(), false)) {
                            ui.label(mono(&format!("↻ {at}"), 9.0).color(DIM));
                        }
                    });
                }
                // Where Claude's numbers come from: the last /usage Claude
                // Code cached, or live with its token (opt-in).
                if usage.harness == "claude" {
                    let source = |live: bool| if live { tr("En vivo (token)") } else { tr("Último /usage") };
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("claude-source").selected_text(mono(&source(claude_live), 9.0)).width(110.0).show_ui(ui, |ui| {
                            if ui.selectable_label(!claude_live, source(false)).on_hover_text(tr("Lo que Claude Code guardó la última vez que corriste /usage. No lee credenciales.")).clicked() { picked.claude_live = Some(false); }
                            if ui.selectable_label(claude_live, source(true)).on_hover_text(tr("Pregunta a Anthropic cada 2 min con el token de Claude Code (llavero de macOS o ~/.claude/.credentials.json).")).clicked() { picked.claude_live = Some(true); }
                        });
                    });
                }
                // When each refills: HP on the week's (or month's) reset
                // date, mana at its hour; the countdown on hover.
                let local = chrono::Local::now();
                let refill = |meter: Option<&crate::usage::Meter>, hour: bool| {
                    meter.filter(|m| m.used > 0.0).and_then(|m| Some((crate::usage::left(m)?, crate::usage::reset_label(m.resets?, local, hour)?)))
                };
                let refills: Vec<(&str, (i64, String))> = [("HP", refill(health, usage.harness != "kiro")), ("MP", refill(mana, true))].into_iter().filter_map(|(k, r)| Some((k, r?))).collect();
                if !refills.is_empty() && usage.harness != "kiro" {
                    let text = refills.iter().map(|(k, (_, at))| format!("{k} ↻ {at}")).collect::<Vec<_>>().join(" · ");
                    let tip = refills.iter().map(|(k, (wait, _))| format!("{k}: {}", trf("se reinicia en {}", &[&crate::scene::countdown(*wait)]))).collect::<Vec<_>>().join("\n");
                    ui.add(egui::Label::new(mono(&text, 9.0).color(DIM)).truncate()).on_hover_text(tip);
                }
                let tip = usage.meters.iter().map(|m| trf("{}: {}% usado", &[&tr(m.window), &format!("{:.0}", m.used)])).collect::<Vec<_>>().join(" · ");
                if !tip.is_empty() { row.on_hover_text(tip); }
            });
        });
    }
    picked
}
