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
use egui::{pos2, vec2, Align2, Color32, Context, CornerRadius, FontId, Frame, Key, Margin, Rect, RichText, Stroke, Ui, Vec2};
use std::sync::mpsc::Receiver;

pub const HUD_HEIGHT: f32 = 28.0;
/// The chat panel's height under the rooms until its edge is dragged, and
/// the least it can be dragged to (the console and the box still fit).
pub const PANEL_HEIGHT: f32 = 232.0;
pub const MIN_PANEL: f32 = 160.0;
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
    // Painted as a selectable label too: a label left out of the selection
    // for a frame drops it, and ⌘ is held to copy what is selected.
    egui::text_selection::LabelSelectionState::label_text_selection(ui, &response, pos, galley, Color32::from_gray(222), Stroke::NONE);
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

// ---- Calls to action

/// Full screen keeps the rooms in their places, so the agents that wait
/// for you (asking, or done) are called over them instead.
const CALLS_SHOWN: usize = 4;

pub fn calls_height(count: usize) -> f32 {
    if count == 0 { return 0.0; }
    let more = if count > CALLS_SHOWN { fs(16.0) } else { 0.0 };
    fs(14.0) + 16.0 + count.min(CALLS_SHOWN) as f32 * (fs(12.0) + 12.0) + more
}

/// The agent picked from the list, to open.
pub fn calls(ui: &mut Ui, agents: &[Agent], scene: &crate::scene::Scene) -> Option<String> {
    let mut picked = None;
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::ZERO, HUD_FILL);
    ui.painter().hline(rect.x_range(), rect.max.y - 0.5, egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 20)));
    let inner = rect.shrink2(vec2(10.0, 6.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        ui.label(mono(&tr("Te esperan"), 10.0).strong().color(alpha(Color32::WHITE, 0.55)));
        for agent in agents.iter().take(CALLS_SHOWN) {
            let color = crate::scene::agent_color(&agent.status);
            let row = vec2(ui.available_width(), fs(12.0) + 10.0);
            let (rect, response) = ui.allocate_exact_size(row, egui::Sense::click());
            let fill = if response.hovered() { alpha(color, 0.22) } else { alpha(color, 0.12) };
            ui.painter().rect(rect, CornerRadius::same(4), fill, egui::Stroke::new(1.0_f32, alpha(color, 0.55)), egui::StrokeKind::Inside);
            // The repo's emblem (its guild), or the state's dot until its room is up.
            match scene.identity(&agent.id) {
                Some(identity) => crate::scene::draw_emblem(ui.painter(), identity, egui::pos2(rect.min.x + 5.0, rect.center().y - 9.0), 1.5),
                None => { ui.painter().circle_filled(egui::pos2(rect.min.x + 10.0, rect.center().y), 3.5, color); }
            }
            let font = FontId::monospace(fs(11.0));
            // The card's own word for the state, short enough to leave the folder whole.
            let state = ui.painter().text(egui::pos2(rect.max.x - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, tr(if agent.status == "blocked" { "ATENCIÓN" } else { "LISTO" }), font.clone(), color);
            let room = (state.min.x - rect.min.x - 34.0).max(0.0);
            // The folder, cut with "…" where the state starts.
            let ink = Color32::from_gray(225);
            let mut text = agent.folder();
            let mut name = ui.painter().layout_no_wrap(text.clone(), font.clone(), ink);
            while name.size().x > room && text.pop().is_some() {
                name = ui.painter().layout_no_wrap(format!("{}…", text.trim_end()), font.clone(), ink);
            }
            ui.painter().galley(egui::pos2(rect.min.x + 24.0, rect.center().y - name.size().y / 2.0), name, ink);
            let response = response.on_hover_text(if agent.activity.is_empty() { tr("Abrir su chat") } else { agent.activity.clone() }).on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() { picked = Some(agent.id.clone()); }
        }
        if agents.len() > CALLS_SHOWN {
            ui.label(mono(&trf("y {} más", &[&(agents.len() - CALLS_SHOWN).to_string()]), 10.0).color(alpha(Color32::WHITE, 0.5)));
        }
    });
    picked
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
    let mut action = HudAction { toggle_log: false, compose: false };
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
    /// The last message sent, put back in the box when esc interrupts the
    /// agent so it can be edited and sent again; `restored` while it is.
    last_sent: Option<String>,
    restored: bool,
    /// The background shell whose output is open, its lines (re-read each
    /// second), and the one asked to stop (waiting for a yes).
    shell_open: Option<(u32, f64, Vec<String>)>,
    shell_stop: Option<u32>,
    /// Full screen's column of changed files (re-read every 3 s while open),
    /// the file whose diff shows, and the branch's pull request (every minute).
    changes: Option<Result<(Vec<crate::changes::FileChange>, Option<String>), String>>,
    changes_rx: Option<Receiver<Result<(Vec<crate::changes::FileChange>, Option<String>), String>>>,
    next_changes: f64,
    diff_path: Option<String>,
    diff: Vec<String>,
    diff_rx: Option<Receiver<Vec<String>>>,
    pull: Option<crate::changes::PullRequest>,
    pull_rx: Option<Receiver<Option<crate::changes::PullRequest>>>,
    next_pull: f64,
}

impl ChatState {
    pub fn new(agent_id: &str) -> ChatState {
        ChatState { agent_id: agent_id.to_string(), draft: String::new(), lines: vec![], note: None, sending: None, choice: None, tail_rx: None, next_tail: 0.0, focus: true, status_seen: String::new(), screen: vec![], screen_rx: None, next_screen: 0.0, slash: None, slash_choice: 0, cols: 0, last_sent: None, restored: false, shell_open: None, shell_stop: None, changes: None, changes_rx: None, next_changes: 0.0, diff_path: None, diff: vec![], diff_rx: None, pull: None, pull_rx: None, next_pull: 0.0 }
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

    /// esc to a working agent interrupts it: the message it was working on
    /// comes back to the box, unless something else is typed there.
    fn interrupt(&mut self, monitor: &Monitor, agent: &Agent) {
        self.run(monitor.press(&["esc".into()], agent));
        if agent.status != "working" || !self.draft.trim().is_empty() { return; }
        if let Some(text) = self.last_sent.clone() {
            self.draft = text;
            self.restored = true;
            self.focus = true;
        }
    }
}

pub struct ChatAction {
    pub close: bool,
    pub terminal: bool,
    /// The console's text size: +1 bigger, -1 smaller.
    pub console_step: i8,
}

/// Opens under the rooms when one is clicked: the tail of the agent's
/// terminal, quick answers for a permission prompt, and a box to type to it.
/// The terminal is shown as it is, colours and spinners included, refreshed
/// twice a second; `wide` (full screen) sizes the font so its lines stay
/// whole. A question keeps the plain lines, whose options are clicked.
/// What the changed files are compared with (kept while the app runs, for
/// every agent).
fn diff_base(ctx: &Context) -> crate::changes::DiffBase {
    let index = ctx.data(|d| d.get_temp::<usize>(egui::Id::new("diff-base"))).unwrap_or(0);
    crate::changes::DiffBase::ALL[index.min(2)]
}

/// Whether full screen shows the column of changed files (kept while the
/// app runs, for every agent).
fn changes_open(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::new("changes-open"))).unwrap_or(false)
}

pub fn chat_panel(ui: &mut Ui, monitor: &Monitor, agent: &Agent, state: &mut ChatState, now: f64, wide: bool) -> ChatAction {
    poll_changes(ui.ctx(), agent, state, now, wide && changes_open(ui.ctx()));
    if !(wide && changes_open(ui.ctx())) { return chat_body(ui, monitor, agent, state, now, wide); }
    // The changed files take the console's right side.
    let full = ui.max_rect();
    let side = (full.width() * 0.36).clamp(260.0, 560.0);
    let left = Rect::from_min_max(full.min, pos2(full.max.x - side - 12.0, full.max.y));
    let right = Rect::from_min_max(pos2(full.max.x - side, full.min.y), full.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| changes_column(ui, agent, state));
    ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| chat_body(ui, monitor, agent, state, now, wide)).inner
}

/// Keep the pull request (every minute, while the chat is open) and, with
/// the column open, the changed files and the open diff (every 3 s) fresh.
fn poll_changes(ctx: &Context, agent: &Agent, state: &mut ChatState, now: f64, column: bool) {
    let in_repo = agent.branch.is_some();
    if in_repo && now >= state.next_pull && state.pull_rx.is_none() {
        state.pull_rx = Some(crate::changes::fetch_pull_request(agent.cwd.clone()));
        state.next_pull = now + 60.0;
    }
    if let Some(rx) = &state.pull_rx {
        if let Ok(pull) = rx.try_recv() { state.pull = pull; state.pull_rx = None; ctx.request_repaint(); }
    }
    if column && now >= state.next_changes && state.changes_rx.is_none() {
        state.changes_rx = Some(crate::changes::fetch_changes(agent.cwd.clone(), diff_base(ctx)));
        state.next_changes = now + 3.0;
        let picked = state.diff_path.as_ref().and_then(|p| state.changes.as_ref()?.as_ref().ok()?.0.iter().find(|f| &f.path == p)).cloned();
        if let (Some(change), None) = (picked, &state.diff_rx) { state.diff_rx = Some(crate::changes::fetch_diff(agent.cwd.clone(), change, diff_base(ctx))); }
    }
    if let Some(rx) = &state.changes_rx {
        if let Ok(changes) = rx.try_recv() { state.changes = Some(changes); state.changes_rx = None; ctx.request_repaint(); }
    }
    if let Some(rx) = &state.diff_rx {
        if let Ok(diff) = rx.try_recv() { state.diff = diff; state.diff_rx = None; ctx.request_repaint(); }
    }
}

/// The list's height in the column: its rows, up to 42% of the column (60
/// pt at least, when there are that many).
pub(crate) fn changes_list_height(column: f32, files: usize) -> f32 {
    (21.0 * files as f32 + 4.0).min((column * 0.42).max(60.0))
}

/// The column of changed files: each with its status, path and lines added
/// and removed; a click shows its diff under the list.
fn changes_column(ui: &mut Ui, agent: &Agent, state: &mut ChatState) {
    let rect = ui.max_rect();
    ui.painter().rect_filled(rect, CornerRadius::same(5), alpha(Color32::BLACK, 0.45));
    let inner = rect.shrink(8.0);
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        let files = match &state.changes { Some(Ok((files, _))) => Some(files.clone()), _ => None };
        // Its title: how many files and lines, and × to close it.
        ui.horizontal(|ui| {
            let title = match (&state.changes, &files) {
                (_, Some(files)) => trf("{} archivos cambiados", &[&files.len().to_string()]),
                (Some(Err(e)), _) if e == "no upstream" => tr("Esta rama no está en el remoto"),
                (Some(Err(e)), _) if e == "no main" => tr("No hay rama main ni master"),
                (Some(Err(_)), _) => tr("No es un repositorio git"),
                _ => tr("Leyendo cambios…"),
            };
            ui.label(mono(&title, 11.0).strong().color(if files.is_some() { Color32::WHITE } else { DIM }));
            if let (Some(branch), Some(_)) = (&agent.branch, &files) { ui.label(mono(branch, 10.0).color(DIM)); }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if flat_button(ui, mono("×", 14.0).color(Color32::WHITE)).on_hover_text(tr("Cerrar los archivos cambiados")).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("changes-open"), false));
                }
                if let Some(files) = &files {
                    let (added, removed) = files.iter().fold((0, 0), |(a, r), f| (a + f.added.unwrap_or(0), r + f.removed.unwrap_or(0)));
                    ui.add_space(4.0);
                    ui.label(mono(&format!("−{removed}"), 10.0).color(Color32::from_rgb(255, 105, 97)));
                    ui.label(mono(&format!("+{added}"), 10.0).color(Color32::from_rgb(99, 199, 77)));
                }
            });
        });
        // What they are compared with: the last commit, the remote, main.
        let base = diff_base(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
            ui.spacing_mut().button_padding = vec2(6.0, 2.0);
            for (i, option) in crate::changes::DiffBase::ALL.into_iter().enumerate() {
                let (label, tip) = match option {
                    crate::changes::DiffBase::Uncommitted => (tr("Sin commit"), tr("Lo que cambió desde el último commit")),
                    crate::changes::DiffBase::Unpushed => (tr("Sin subir"), tr("Todo lo que aún no está en el remoto: commits sin subir y cambios sin commit")),
                    crate::changes::DiffBase::Main => (tr("Contra main"), tr("Todo lo de la rama desde que salió de main, como en un PR")),
                };
                let on = option == base;
                let cyan = Color32::from_rgb(44, 232, 245);
                let chip = egui::Button::new(mono(&label, 10.0).color(if on { Color32::from_rgb(10, 30, 34) } else { cyan })).fill(if on { cyan } else { alpha(cyan, 0.06) }).stroke(Stroke::new(1.0_f32, alpha(cyan, 0.45))).corner_radius(CornerRadius::same(3));
                if ui.add(chip).on_hover_text(tip).clicked() && !on {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("diff-base"), i));
                    state.changes = None;
                    state.changes_rx = None;
                    state.next_changes = 0.0;
                    state.diff.clear();
                    state.diff_rx = None;
                }
            }
            if let Some(Ok((_, Some(name)))) = &state.changes { ui.label(mono(&trf("contra {}", &[name]), 10.0).color(DIM)); }
        });
        let Some(files) = files else { return };
        if files.is_empty() { ui.label(mono(&tr("Sin cambios"), 11.0).color(DIM)); return; }
        let list_h = changes_list_height(ui.available_height(), files.len());
        egui::ScrollArea::vertical().id_salt("changed-files").max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
            for file in &files {
                let picked = state.diff_path.as_deref() == Some(file.path.as_str());
                let (row, response) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), egui::Sense::click());
                if picked || response.hovered() { ui.painter().rect_filled(row, CornerRadius::same(3), alpha(Color32::WHITE, if picked { 0.1 } else { 0.05 })); }
                let color = match file.status { 'A' => Color32::from_rgb(99, 199, 77), 'D' => Color32::from_rgb(255, 105, 97), '?' => Color32::from_rgb(44, 232, 245), 'R' => Color32::from_rgb(181, 80, 136), 'U' => Color32::from_rgb(255, 69, 58), _ => Color32::from_rgb(254, 174, 52) };
                let font = FontId::monospace(fs(11.0));
                let painter = ui.painter();
                // As editors show them: U untracked, ! in conflict.
                let letter = match file.status { '?' => 'U', 'U' => '!', other => other };
                painter.text(pos2(row.min.x + 6.0, row.center().y), Align2::LEFT_CENTER, letter.to_string(), font.clone(), color);
                let counts = match (file.added, file.removed) { (Some(a), Some(r)) => format!("+{a} −{r}"), _ => "bin".to_string() };
                let counts_rect = painter.text(pos2(row.max.x - 6.0, row.center().y), Align2::RIGHT_CENTER, &counts, FontId::monospace(fs(10.0)), DIM);
                // The file's name in white, its folder dimmed before it, cut from the left.
                let room = counts_rect.min.x - row.min.x - 30.0;
                let (dir, name) = file.path.rsplit_once('/').map(|(d, n)| (format!("{d}/"), n.to_string())).unwrap_or((String::new(), file.path.clone()));
                let name_w = painter.layout_no_wrap(name.clone(), font.clone(), Color32::WHITE).size().x;
                let mut dir_shown = dir.clone();
                while !dir_shown.is_empty() && painter.layout_no_wrap(format!("…{dir_shown}"), font.clone(), DIM).size().x + name_w > room { dir_shown.remove(0); }
                let dir_text = if dir_shown.len() < dir.len() { format!("…{dir_shown}") } else { dir_shown };
                let at = painter.text(pos2(row.min.x + 22.0, row.center().y), Align2::LEFT_CENTER, &dir_text, font.clone(), DIM);
                painter.text(pos2(at.max.x, row.center().y), Align2::LEFT_CENTER, &name, font, Color32::WHITE);
                if response.on_hover_text(&file.path).clicked() {
                    state.diff_path = if picked { None } else { Some(file.path.clone()) };
                    state.diff.clear();
                    if !picked { state.diff_rx = Some(crate::changes::fetch_diff(agent.cwd.clone(), file.clone(), base)); }
                }
            }
        });
        ui.separator();
        if state.diff_path.is_none() { ui.label(mono(&tr("Elige un archivo para ver su diff"), 11.0).color(DIM)); return; }
        egui::ScrollArea::both().id_salt(("diff", state.diff_path.clone())).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for line in &state.diff {
                let color = if line.starts_with("@@") { Color32::from_rgb(44, 232, 245) } else if line.starts_with('+') { Color32::from_rgb(120, 220, 110) } else if line.starts_with('-') { Color32::from_rgb(255, 120, 110) } else { Color32::from_gray(190) };
                ui.add(egui::Label::new(mono(line, 11.0).color(color)).extend());
            }
        });
    });
}

fn chat_body(ui: &mut Ui, monitor: &Monitor, agent: &Agent, state: &mut ChatState, now: f64, wide: bool) -> ChatAction {
    let mut action = ChatAction { close: false, terminal: false, console_step: 0 };
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
        if standalone { action.close = true; } else if !sending { state.interrupt(monitor, agent); }
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

    // The panel fills what it is given: the height its edge was dragged to
    // under the rooms, the whole side in full screen.
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
                if wide {
                    // Full screen has room for words: each button says what it does.
                    let button = |ui: &mut Ui, text: &str, color: Color32| {
                        ui.add(egui::Button::new(mono(text, 11.0).color(color)).fill(alpha(color, 0.08)).stroke(Stroke::new(1.0_f32, alpha(color, 0.35))).corner_radius(CornerRadius::same(4)))
                    };
                    // The terminal in amber, the changed files in cyan (lit while
                    // their column is open), the pull request in its state's colour.
                    ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
                    ui.spacing_mut().button_padding = vec2(8.0, 4.0);
                    // Right to left, so they read Terminal, PR, Diff.
                    let open = changes_open(ui.ctx());
                    let cyan = Color32::from_rgb(44, 232, 245);
                    let files = egui::Button::new(mono("± Diff", 11.0).color(if open { Color32::from_rgb(10, 30, 34) } else { cyan }))
                        .fill(if open { cyan } else { alpha(cyan, 0.08) }).stroke(Stroke::new(1.0_f32, alpha(cyan, 0.6))).corner_radius(CornerRadius::same(4));
                    if ui.add(files).on_hover_text(tr("Los archivos que cambiaron en su carpeta y su diff, a la derecha")).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("changes-open"), !open));
                        state.next_changes = 0.0;
                    }
                    if let Some(pull) = &state.pull { pull_chip(ui, pull); }
                    if button(ui, ">_ Terminal", Color32::from_rgb(254, 174, 52)).on_hover_text(tr("Abrir una terminal en esta carpeta")).clicked() {
                        action.terminal = true;
                    }
                    return;
                }
                if flat_button(ui, mono("x", 13.0).color(Color32::WHITE)).on_hover_text(tr("Cerrar")).clicked() { action.close = true; }
                if !standalone && flat_button(ui, mono("↗", 13.0).color(Color32::WHITE)).on_hover_text(tr("Enfocar este agente en Herdr")).clicked() && !sending {
                    state.run(monitor.focus(agent));
                }
                if flat_button(ui, mono(">_", 11.0).strong().color(Color32::from_gray(200))).on_hover_text(tr("Abrir una terminal en esta carpeta")).clicked() {
                    action.terminal = true;
                }
                // The console's own text size.
                if flat_button(ui, mono("A+", 11.0).color(Color32::from_gray(200))).on_hover_text(format!("{} (⌘⇧+)", tr("Consola más grande"))).clicked() { action.console_step = 1; }
                if flat_button(ui, mono("A-", 11.0).color(Color32::from_gray(200))).on_hover_text(format!("{} (⌘⇧-)", tr("Consola más pequeña"))).clicked() { action.console_step = -1; }
                if let Some(pull) = &state.pull { pull_chip(ui, pull); }
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
        let strip = shells_strip(ui, agent, state, now);
        let body_height = total - 7.0 * 2.0 - 22.0 - strip - if standalone { 14.0 } else if inline { 0.0 } else { box_height } - if state.note.is_some() { 16.0 } else { 0.0 };
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

/// The shells the agent left running, one row each over the console: what
/// it does, for how long, its last line; ▸ opens its output, ✕ stops it
/// (after a yes). Returns the height it took, with its spacing.
fn shells_strip(ui: &mut Ui, agent: &Agent, state: &mut ChatState, now: f64) -> f32 {
    if agent.shells.is_empty() { state.shell_open = None; return 0.0; }
    let unix = crate::herdr::unix_now();
    let top = ui.cursor().min.y;
    let brew = Color32::from_rgb(150, 240, 110);
    Frame::new().fill(alpha(brew, 0.07)).stroke(Stroke::new(1.0_f32, alpha(brew, 0.25))).corner_radius(CornerRadius::same(5)).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
        for shell in &agent.shells {
            let open = state.shell_open.as_ref().is_some_and(|o| o.0 == shell.pid);
            ui.horizontal(|ui| {
                let toggle = flat_button(ui, mono(if open { "▾" } else { "▸" }, 11.0).color(brew)).on_hover_text(tr("Ver su salida"));
                if toggle.clicked() { state.shell_open = if open { None } else { Some((shell.pid, 0.0, vec![])) }; }
                cauldron_icon(ui, brew);
                ui.label(mono(&shell.label, 11.0).strong().color(brew)).on_hover_text(&shell.command);
                ui.label(mono(&crate::scene::countdown(unix - shell.started), 10.0).color(DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if state.shell_stop == Some(shell.pid) {
                        if flat_button(ui, mono(&tr("no"), 10.0).color(DIM)).clicked() { state.shell_stop = None; }
                        if flat_button(ui, mono(&tr("sí, detener"), 10.0).strong().color(Color32::from_rgb(255, 105, 97))).clicked() {
                            crate::shells::stop(shell.pid);
                            state.shell_stop = None;
                        }
                    } else if flat_button(ui, mono("x", 12.0).color(DIM)).on_hover_text(tr("Detener este shell")).clicked() {
                        state.shell_stop = Some(shell.pid);
                    }
                    if let Some(last) = &shell.last_line {
                        ui.add(egui::Label::new(mono(last, 10.0).color(Color32::from_gray(170))).truncate());
                    }
                });
            });
            if open {
                // Its output, re-read once a second while open.
                if let (Some(view), Some(path)) = (state.shell_open.as_mut(), shell.output.as_deref()) {
                    if now >= view.1 { view.2 = crate::shells::output_tail(path, 8); view.1 = now + 1.0; }
                }
                let lines = state.shell_open.as_ref().map(|o| o.2.clone()).unwrap_or_default();
                Frame::new().fill(alpha(Color32::BLACK, 0.4)).corner_radius(CornerRadius::same(3)).inner_margin(Margin::same(4)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if lines.is_empty() { ui.label(mono(&tr("Sin salida todavía."), 10.0).color(DIM)); }
                    for line in &lines { ui.add(egui::Label::new(mono(line, 10.0).color(Color32::from_gray(200))).truncate()); }
                    if let Some(path) = &shell.output { ui.label(mono(&crate::herdr::abbreviate_home(path), 9.0).color(DIM)); }
                });
            }
        }
    });
    if state.shell_open.as_ref().is_some_and(|o| !agent.shells.iter().any(|s| s.pid == o.0)) { state.shell_open = None; }
    ui.cursor().min.y - top
}

/// A little cauldron of brew, in pixels (the font has no such glyph).
fn cauldron_icon(ui: &mut Ui, brew: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 12.0), egui::Sense::hover());
    let p = ui.painter();
    let px = |x: f32, y: f32, w: f32, h: f32, c: Color32| p.rect_filled(egui::Rect::from_min_size(rect.min + vec2(x, y), vec2(w, h)), CornerRadius::ZERO, c);
    px(2.0, 1.0, 10.0, 2.0, brew);
    px(0.0, 3.0, 14.0, 1.0, Color32::from_gray(20));
    px(1.0, 4.0, 12.0, 5.0, Color32::from_rgb(84, 84, 102));
    px(2.0, 4.0, 1.0, 4.0, Color32::from_rgb(140, 142, 163));
    px(3.0, 9.0, 8.0, 1.0, Color32::from_rgb(84, 84, 102));
    px(3.0, 10.0, 1.0, 2.0, Color32::from_gray(20));
    px(10.0, 10.0, 1.0, 2.0, Color32::from_gray(20));
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
            if ui.add_enabled(!sending, egui::Button::new(mono(&tr("Aceptar"), 11.0))).on_hover_text(tr("Aceptar lo que pide el agente: su opción marcada, o el «Sí» si la marcada es un «No»")).clicked() {
                match state.choice.or(menu.affirmative()) { Some(choice) => answer(state, monitor, agent, &menu, choice), None => state.run(monitor.press(&["enter".into()], agent)) }
            }
            if ui.add_enabled(!sending, egui::Button::new(mono(&tr("Rechazar"), 11.0))).on_hover_text(tr("Cancelar la petición del agente (esc en su terminal)")).clicked() {
                state.run(monitor.press(&["esc".into()], agent));
            }
        } else if agent.status == "working" {
            if ui.add_enabled(!sending, egui::Button::new(mono(&tr("Detener"), 11.0))).on_hover_text(tr("Interrumpir al agente (esc en su terminal)")).clicked() {
                state.interrupt(monitor, agent);
            }
        }
    });
}

/// The branch's pull request, in the chat's header: its number in its
/// state's colour (green open, purple merged, grey closed or draft); a
/// click opens it in the browser.
fn pull_chip(ui: &mut Ui, pull: &crate::changes::PullRequest) {
    let (color, word) = match (pull.state.as_str(), pull.draft) {
        ("MERGED", _) => (Color32::from_rgb(181, 120, 230), tr("mergeado")),
        ("CLOSED", _) => (Color32::from_gray(150), tr("cerrado")),
        (_, true) => (Color32::from_gray(170), tr("borrador")),
        _ => (Color32::from_rgb(99, 199, 77), tr("abierto")),
    };
    let chip = egui::Button::new(mono(&format!("PR #{}", pull.number), 10.5).strong().color(color)).fill(alpha(color, 0.12)).stroke(Stroke::new(1.0_f32, alpha(color, 0.6))).corner_radius(CornerRadius::same(4));
    if ui.add(chip).on_hover_text(format!("{} ({word})\n{}", pull.title, tr("Abrir el PR en el navegador"))).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        crate::herdr::open_target(&pull.url);
    }
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
    if option { return Some((index, true)); }
    // An option without a number ("❯ auto", Kiro's /model) has the
    // picker's footer below it; typed text on a prompt does not.
    if !bare {
        let footer = screen[index + 1..].iter().any(|l| crate::herdr::is_menu_footer(l.iter().map(|s| s.text.as_str()).collect::<String>().trim()));
        return Some((index, footer));
    }
    // The nearest line with text above the prompt's box (past its top rule
    // and blank lines) is the picker's footer or one of its options.
    let nearest = screen[..index].iter().rev().map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>()).find(|t| {
        let t = t.trim();
        !t.is_empty() && !t.chars().all(|c| matches!(c, '─' | '━' | '═' | '╌' | '┄' | '-'))
    });
    let picker = nearest.map(|t| crate::herdr::is_menu_footer(t.trim()) || (t.trim_start().starts_with(['❯', '›']) && crate::herdr::is_option(t.trim()))).unwrap_or(false);
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
    // Words typed at a menu would hit its one-key shortcuts (Codex approves
    // on the y of "try"): turn the request down with its "tell it instead"
    // option, or esc, and send them as a message once it takes one.
    if agent.status == "blocked" && menu.is_menu() && !monitor.standalone() {
        let text = state.draft.trim().to_string();
        if state.sending.is_some() || text.is_empty() { return; }
        let keys = menu.tell_instead().map(|i| menu.keys_choosing(i)).unwrap_or_else(|| vec!["esc".into()]);
        state.run(monitor.decline_and_send(&keys, &text, agent));
        state.draft.clear();
        return;
    }
    submit_text(state, monitor, agent);
}

fn submit_text(state: &mut ChatState, monitor: &Monitor, agent: &Agent) {
    let text = state.draft.clone();
    if state.sending.is_some() || text.trim().is_empty() { return; }
    // Whatever is left in the terminal's own input goes first, or the
    // message is sent with it (Codex keeps an unknown /command there, and
    // its cursor at the start: the message would land before it). After
    // esc that is the interrupted message, which may wrap past the line read.
    let erase = if state.restored { erase_count(&state.screen, state.last_sent.as_deref().unwrap_or("")) } else { composer_text(&state.screen).map_or(0, |t| t.chars().count()) };
    state.run(monitor.send_erasing(erase, &text, agent));
    if agent.status != "blocked" { state.last_sent = Some(text.trim().to_string()); }
    state.restored = false;
    state.draft.clear();
}

/// What is typed in the terminal's input (its prompt line past the marker,
/// without the dim placeholder), if anything.
pub(crate) fn composer_text(screen: &[Vec<Span>]) -> Option<String> {
    let line = &screen[prompt_line(screen)?];
    // Kiro paints its placeholder grey rather than dim, right after the
    // cursor when nothing is typed: the grey from there on is not typing.
    let grey = |s: &Span| s.fg.is_some_and(|c| c.r() == c.g() && c.g() == c.b() && (90..200).contains(&c.r()));
    let typed_before = |i: usize| line[..i].iter().any(|s| !s.dim && s.bg.is_none() && s.text.trim().chars().any(|c| !matches!(c, '❯' | '›' | '>')));
    let placeholder = line.iter().enumerate().position(|(i, s)| grey(s) && !s.text.trim().is_empty() && !typed_before(i)).unwrap_or(line.len());
    let text: String = line[..placeholder].iter().filter(|s| !s.dim).map(|s| s.text.as_str()).collect();
    let text = text.trim_start();
    let typed = text.chars().skip(1).collect::<String>().trim().to_string();
    (!typed.is_empty()).then_some(typed)
}

/// Backspaces that empty the terminal's input after esc: as many as it
/// shows, or the whole interrupted message when that is what it shows the
/// start of (a long one wraps past the line read).
pub(crate) fn erase_count(screen: &[Vec<Span>], interrupted: &str) -> usize {
    let Some(typed) = composer_text(screen) else { return 0 };
    let shown = typed.chars().count();
    let head: String = interrupted.trim().chars().take(shown.min(20)).collect();
    if !head.is_empty() && typed.starts_with(&head) { shown.max(interrupted.trim().chars().count()) } else { shown }
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
        // A failure says what to do, so it wraps above the button instead of being cut.
        if let (false, Some(note)) = (busy, &state.note) {
            ui.label(RichText::new(note).size(fs(10.0)).color(Color32::from_rgb(255, 159, 10)));
        }
        ui.horizontal(|ui| {
            if busy {
                ui.spinner();
                ui.label(RichText::new(tr("Invocando…")).size(fs(10.0)).color(DIM));
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
    // A failure says what to do: it wraps above the button, and the prompt
    // gives up a row so the room still holds it.
    let note = state.note.clone().filter(|_| !busy);
    let edit = egui::TextEdit::multiline(&mut state.prompt).hint_text(tr("Primer prompt (opcional)")).font(FontId::monospace(fs(10.0))).desired_rows(if note.is_some() { 1 } else { 2 }).desired_width(f32::INFINITY).interactive(!busy);
    ui.add(edit);
    if let Some(note) = note {
        ui.label(RichText::new(note).size(fs(9.0)).color(Color32::from_rgb(255, 159, 10)));
    }
    ui.horizontal(|ui| {
        if busy {
            ui.spinner();
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

/// What a room's cauldrons are brewing, on a parchment like the guild
/// log's, hung just under them while the pointer is there.
pub fn shells_note(ctx: &Context, cauldrons: egui::Rect, rows: &[(String, Option<String>)]) {
    let ink = Color32::from_rgb(61, 38, 20);
    // As wide as the guild log, or what the window leaves to its right.
    let screen = ctx.screen_rect();
    let width = (screen.width() - 12.0).min(260.0);
    let x = cauldrons.min.x.min(screen.max.x - width - 22.0).max(screen.min.x + 4.0);
    egui::Area::new(egui::Id::new("shells-note")).fixed_pos(pos2(x, cauldrons.max.y + 2.0)).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
        Frame::new()
            .fill(Color32::from_rgb(237, 217, 168))
            .stroke(Stroke::new(2.0_f32, Color32::from_rgb(115, 74, 36)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(8))
            .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: alpha(Color32::BLACK, 0.5) })
            .show(ui, |ui| {
                ui.set_width(width - 18.0);
                ui.spacing_mut().item_spacing = vec2(5.0, 4.0);
                ui.label(mono(&tr("Cocinando en background"), 12.0).strong().color(ink));
                let rule = ui.available_rect_before_wrap();
                ui.painter().hline(rule.x_range(), rule.min.y, Stroke::new(1.0_f32, alpha(ink, 0.35)));
                ui.add_space(2.0);
                for (line, last) in rows {
                    ui.horizontal_top(|ui| {
                        let (dot, _) = ui.allocate_exact_size(vec2(6.0, 12.0), egui::Sense::hover());
                        ui.painter().rect_filled(egui::Rect::from_center_size(pos2(dot.center().x, dot.center().y + 1.0), vec2(6.0, 6.0)), CornerRadius::ZERO, Color32::from_rgb(38, 153, 64));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.add(egui::Label::new(mono(line, 10.0).color(ink)).wrap());
                            if let Some(last) = last { ui.add(egui::Label::new(mono(last, 10.0).color(alpha(ink, 0.6))).truncate()); }
                        });
                    });
                }
            });
    });
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

/// The HP viewer's height for these harnesses: closed, one line; open, a
/// title and one row each, the one opened with a click taller by its options.
pub fn usage_height(ctx: &egui::Context, usages: &[crate::usage::HarnessUsage], open_panel: bool) -> f32 {
    if usages.is_empty() { return 0.0; }
    if !open_panel { return USAGE_LINE; }
    let open = usage_open(ctx);
    26.0 + usages.iter().map(|u| 54.0 + if open.as_deref() == Some(u.harness) && has_options(u.harness) { 22.0 } else { 0.0 }).sum::<f32>()
}

/// The hero whose options (Claude's source, Kiro's plan) are showing.
fn usage_open(ctx: &egui::Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(egui::Id::new("hp-open")))
}

fn has_options(harness: &str) -> bool {
    matches!(harness, "claude" | "kiro")
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
    /// The title was clicked: open or close the panel.
    pub toggle_open: bool,
}

/// The closed HP viewer's height: its line.
const USAGE_LINE: f32 = 30.0;

/// A harness's HP in a few characters, for the closed viewer: what is left
/// in percent, Kiro's credits left, or KO.
pub fn hp_short(usage: &crate::usage::HarnessUsage, unix: i64) -> (String, Option<f32>) {
    let health = usage.meters.iter().find(|m| m.window != "5h");
    let hp = health.map(|m| m.hp(unix));
    let text = match (health, &usage.credits) {
        (Some(_), Some((spent, Some(plan)))) => format!("{:.0}", (*plan as f64 - spent).max(0.0)),
        (Some(m), _) if m.hp(unix) <= 0.0 => "KO".to_string(),
        (Some(m), _) => format!("{:.0}%", m.hp(unix)),
        (None, Some((spent, None))) => format!("{spent:.0} cr"),
        _ => "—".to_string(),
    };
    (text, hp)
}

pub fn usage_panel(ui: &mut Ui, assets: &crate::assets::Assets, usages: &[crate::usage::HarnessUsage], now: f64, claude_live: bool, open_panel: bool) -> UsagePick {
    let mut picked = UsagePick::default();
    let unix = crate::herdr::unix_now();
    let mana_color = Color32::from_rgb(90, 150, 255);
    ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
    if !open_panel {
        // Closed: one line, each hero with what it has left; a click opens it.
        let (line, response) = ui.allocate_exact_size(vec2(ui.available_width(), USAGE_LINE), egui::Sense::click());
        let painter = ui.painter();
        let mut x = painter.text(egui::pos2(line.min.x, line.center().y), Align2::LEFT_CENTER, "▸ HP", FontId::monospace(fs(10.0)), if response.hovered() { Color32::WHITE } else { DIM }).max.x + 10.0;
        for usage in usages {
            let (text, hp) = hp_short(usage, unix);
            crate::scene::draw_sprite(painter, assets.hero_frame(usage.harness, "idle", (now * 2.0) as usize), egui::pos2(x + 8.0, line.center().y), vec2(16.0, 16.0), false, 0.0, Color32::WHITE);
            x = painter.text(egui::pos2(x + 18.0, line.center().y), Align2::LEFT_CENTER, &text, FontId::monospace(fs(10.0)), hp.map(hp_color).unwrap_or(DIM)).max.x + 10.0;
        }
        let tip = usages.iter().map(|u| format!("{} {}", u.harness, hp_short(u, unix).0)).collect::<Vec<_>>().join(" · ");
        if response.on_hover_text(format!("{}\n{tip}", tr("HP de la guild (uso restante)"))).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { picked.toggle_open = true; }
        return picked;
    }
    ui.add_space(5.0);
    let title = ui.add(egui::Label::new(mono(&format!("▾ {}", tr("HP de la guild (uso restante)")), 10.0).strong().color(DIM)).sense(egui::Sense::click()));
    if title.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { picked.toggle_open = true; }
    let open = usage_open(ui.ctx());
    let mut toggled: Option<&str> = None;
    for usage in usages {
        let expanded = open.as_deref() == Some(usage.harness);
        let name = match usage.harness { "claude" => "Claude", "codex" => "Codex", "kiro" => "Kiro", other => other };
        let health = usage.meters.iter().find(|m| m.window != "5h");
        let mana = usage.meters.iter().find(|m| m.window == "5h");
        let hp = health.map(|m| m.hp(unix));
        ui.horizontal(|ui| {
            // A click on the hero or its name shows its options.
            let (icon, hero) = ui.allocate_exact_size(vec2(28.0, 40.0), if has_options(usage.harness) { egui::Sense::click() } else { egui::Sense::hover() });
            if hero.clicked() { toggled = Some(usage.harness); }
            // Out of HP or mana, the hero stops and greys out.
            let spent = hp == Some(0.0) || mana.map(|m| m.hp(unix) <= 0.0).unwrap_or(false);
            let frame = if spent { 0 } else { (now * 2.0) as usize };
            let tint = if spent { Color32::from_gray(110) } else { Color32::WHITE };
            crate::scene::draw_sprite(ui.painter(), assets.hero_frame(usage.harness, "idle", frame), icon.center(), vec2(24.0, 24.0), false, 0.0, tint);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
                let row = ui.horizontal(|ui| {
                    let label = ui.add(egui::Label::new(mono(&format!("{name}{}", if !has_options(usage.harness) { "" } else if expanded { " ▾" } else { " ▸" }), 11.0).strong().color(Color32::WHITE)).sense(if has_options(usage.harness) { egui::Sense::click() } else { egui::Sense::hover() }));
                    if label.clicked() { toggled = Some(usage.harness); }
                    if has_options(usage.harness) && (label.hovered() || hero.hovered()) { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
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
                        if expanded {
                            egui::ComboBox::from_id_salt("kiro-plan").selected_text(mono(&shown, 9.0)).width(70.0).show_ui(ui, |ui| {
                                for (label, credits) in crate::usage::KIRO_PLANS {
                                    if ui.selectable_label(plan == Some(credits), format!("{label} · {credits}")).clicked() { picked.kiro_plan = Some(credits); }
                                }
                            });
                        }
                        // The month's credits come back on the 1st.
                        if let Some(at) = health.filter(|m| m.used > 0.0).and_then(|m| m.resets).and_then(|r| crate::usage::reset_label(r, chrono::Local::now(), false)) {
                            ui.label(mono(&format!("↻ {at}"), 9.0).color(DIM));
                        }
                    });
                }
                // Where Claude's numbers come from: the last /usage Claude
                // Code cached, or live with its token (opt-in).
                if usage.harness == "claude" && expanded {
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
    if let Some(harness) = toggled {
        let next = (open.as_deref() != Some(harness)).then(|| harness.to_string());
        ui.ctx().data_mut(|d| match next { Some(h) => d.insert_temp(egui::Id::new("hp-open"), h), None => d.remove::<String>(egui::Id::new("hp-open")) });
    }
    picked
}

// ---- Emblem editor

/// A repo's emblem being edited (a right click on it): its name, colours,
/// charge, partition and shield.
pub struct EmblemEditor {
    pub key: String,
    /// The repo's own name, the one the name field falls back to.
    pub repo_name: String,
    pub name: String,
    pub emblem: crate::identity::Identity,
    /// Colours other repos' emblems use, marked in the palette.
    pub taken: Vec<usize>,
}

pub enum EditorOutcome {
    Open,
    Cancel,
    Save,
    /// Back to the emblem worked out from the repo.
    Auto,
}

pub fn emblem_editor(ctx: &Context, editor: &mut EmblemEditor) -> EditorOutcome {
    use crate::identity::{metal_apart, Pattern, Shape, Symbol, COLOURS};
    let mut outcome = EditorOutcome::Open;
    let modal = egui::Modal::new(egui::Id::new("emblem-editor")).frame(Frame::popup(&ctx.style()).inner_margin(Margin::same(16))).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(60.0, 72.0), egui::Sense::hover());
            crate::scene::draw_emblem(ui.painter(), &editor.emblem, rect.min, 6.0);
            ui.vertical(|ui| {
                ui.label(mono(&tr("Escudo del repo"), 13.0).strong().color(Color32::WHITE));
                ui.label(mono(&editor.key, 10.0).color(DIM));
                ui.add(egui::TextEdit::singleline(&mut editor.name).hint_text(editor.repo_name.clone()).font(FontId::monospace(fs(12.0))).desired_width(300.0));
            });
        });
        // A row of choices, each drawn as the emblem with it.
        let section = |ui: &mut Ui, title: &str| { ui.add_space(2.0); ui.label(mono(title, 10.0).strong().color(DIM)); };
        section(ui, &tr("Color principal"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (i, (name, colour)) in COLOURS.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::click());
                ui.painter().rect_filled(rect.shrink(3.0), CornerRadius::same(4), *colour);
                if editor.emblem.primary == i { ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0_f32, Color32::WHITE), egui::StrokeKind::Inside); }
                let used = editor.taken.contains(&i) && editor.emblem.primary != i;
                if used { ui.painter().circle_filled(rect.right_top() + vec2(-5.0, 5.0), 3.0, Color32::from_rgb(24, 20, 37)); }
                let tip = if used { format!("{name} · {}", tr("lo usa otro repo")) } else { name.to_string() };
                if response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    editor.emblem.primary = i;
                    editor.emblem.secondary = metal_apart(i, editor.emblem.secondary);
                }
            }
        });
        section(ui, &tr("Color secundario"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (i, (name, colour)) in crate::identity::METALS.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(vec2(26.0, 26.0), egui::Sense::click());
                ui.painter().rect_filled(rect.shrink(3.0), CornerRadius::same(4), *colour);
                if editor.emblem.secondary == i { ui.painter().rect_stroke(rect, CornerRadius::same(5), Stroke::new(2.0_f32, Color32::WHITE), egui::StrokeKind::Inside); }
                if response.on_hover_text(*name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { editor.emblem.secondary = i; }
            }
        });
        let mut choice = |ui: &mut Ui, title: &str, count: usize, build: &dyn Fn(&mut crate::identity::Identity, usize), picked: &dyn Fn(&crate::identity::Identity) -> usize| {
            section(ui, title);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for i in 0..count {
                    let mut option = editor.emblem.clone();
                    build(&mut option, i);
                    let (rect, response) = ui.allocate_exact_size(vec2(30.0, 34.0), egui::Sense::click());
                    if picked(&editor.emblem) == i { ui.painter().rect_filled(rect, CornerRadius::same(4), alpha(Color32::WHITE, 0.15)); }
                    if response.hovered() { ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0_f32, alpha(Color32::WHITE, 0.4)), egui::StrokeKind::Inside); }
                    crate::scene::draw_emblem(ui.painter(), &option, rect.min + vec2(5.0, 5.0), 2.0);
                    if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() { editor.emblem = option; }
                }
            });
        };
        choice(ui, &tr("Símbolo"), Symbol::ALL.len(), &|e, i| e.symbol = Symbol::ALL[i], &|e| Symbol::ALL.iter().position(|s| *s == e.symbol).unwrap_or(0));
        choice(ui, &tr("Partición"), Pattern::ALL.len(), &|e, i| e.pattern = Pattern::ALL[i], &|e| Pattern::ALL.iter().position(|p| *p == e.pattern).unwrap_or(0));
        choice(ui, &tr("Forma"), Shape::ALL.len(), &|e, i| e.shape = Shape::ALL[i], &|e| Shape::ALL.iter().position(|p| *p == e.shape).unwrap_or(0));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(mono(&tr("Automático"), 11.0)).on_hover_text(tr("Volver al escudo que sale del repo, con un color libre")).clicked() { outcome = EditorOutcome::Auto; }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(mono(&tr("Guardar"), 11.0).strong()).clicked() { outcome = EditorOutcome::Save; }
                if ui.button(mono(&tr("Cancelar"), 11.0)).clicked() { outcome = EditorOutcome::Cancel; }
            });
        });
    });
    if modal.should_close() && matches!(outcome, EditorOutcome::Open) { outcome = EditorOutcome::Cancel; }
    outcome
}
