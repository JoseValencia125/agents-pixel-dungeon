//! Agents Pixel Dungeon: a native pixel-art guild for your live Herdr agents,
//! for macOS and Linux. Each agent is an animated hero in a dungeon room
//! that changes with its real Herdr status, in a small floating widget with
//! a status item in the menu bar or tray.

mod codex_settings;
mod alerts;
mod ansi;
mod changes;
mod assets;
mod commands;
mod events;
mod guild_chamber;
mod guild_studio;
mod guild_view;
mod herdr;
mod identity;
mod l10n;
mod monitor;
mod prefs;
mod room_moves;
mod scene;
mod selftest;
mod shells;
mod standalone;
mod terminal;
mod tray;
mod ui;
mod usage;

use alerts::{wants_sound, Notifier, Sounds};
use assets::Assets;
use egui::{vec2, Color32, CornerRadius, Frame, Rect, Vec2, ViewportCommand};
use herdr::{connection_note, StatusFilter};
use l10n::{tr, trf};
use monitor::{HerdrState, Monitor};
use prefs::Prefs;
use scene::{Scene, BACKGROUND, GAP, ROW_H};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray::{MenuState, Tray, TrayAction};
use ui::{ChatState, ComposeState, HudState, MIN_PANEL};

/// Room above the first row (drag handle) and below the last one.
const TOP_INSET: f32 = 14.0;
const BOTTOM_INSET: f32 = 6.0;
/// Margin from the screen edges.
const MARGIN: f32 = 12.0;
/// Space the menu bar takes at the top of the screen.
#[cfg(target_os = "macos")]
const SCREEN_TOP: f32 = 25.0;
#[cfg(not(target_os = "macos"))]
const SCREEN_TOP: f32 = 0.0;

/// macOS full screen (a space of its own, with the swipe between spaces)
/// for the widget's own borderless window. winit's route would give it a
/// title bar that stays as a black band on top, so the window is toggled
/// directly instead: macOS only takes a regular app (one with a Dock icon)
/// there, so the app becomes one on the way in and a menu-bar app again
/// after leaving (see `set_regular_app`); the window must also be at the
/// normal level and declared a full-screen candidate. Returns false where
/// there is no such thing (not macOS, no window).
#[cfg(target_os = "macos")]
fn native_fullscreen(on: bool) -> bool {
    use objc2_app_kit::{NSApplication, NSNormalWindowLevel, NSWindowCollectionBehavior, NSWindowStyleMask};
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    let Some(window) = windows.iter().find(|w| w.title().to_string() == "Agents Pixel Dungeon" && w.styleMask().contains(NSWindowStyleMask::Resizable)) else { return false };
    let is_full = window.styleMask().contains(NSWindowStyleMask::FullScreen);
    if on {
        unsafe { window.setCollectionBehavior(window.collectionBehavior() | NSWindowCollectionBehavior::FullScreenPrimary) };
        window.setLevel(NSNormalWindowLevel);
    }
    if is_full != on { window.toggleFullScreen(None); }
    true
}

/// Before macOS full screen: a regular app, activated. The policy only
/// takes hold once the app is active, so the toggle waits a moment
/// (`set_fullscreen`). (The black strip on top of the full-screen space on
/// a notched display is macOS's own; every app's space has it.)
#[cfg(target_os = "macos")]
fn prepare_fullscreen() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

#[cfg(not(target_os = "macos"))]
fn prepare_fullscreen() {}


#[cfg(not(target_os = "macos"))]
fn native_fullscreen(_on: bool) -> bool { false }

/// Back to a menu-bar app (no Dock icon) after full screen on macOS.
#[cfg(target_os = "macos")]
fn set_regular_app(regular: bool) {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else { return };
    let policy = if regular { NSApplicationActivationPolicy::Regular } else { NSApplicationActivationPolicy::Accessory };
    NSApplication::sharedApplication(mtm).setActivationPolicy(policy);
}

#[cfg(not(target_os = "macos"))]
fn set_regular_app(_regular: bool) {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--self-test") {
        selftest::run();
        return;
    }
    let prefs = Prefs::load(Prefs::default_path());
    let session = std::env::var("HERDR_SESSION").ok().or_else(|| prefs.session.clone()).unwrap_or_else(|| "default".into());
    if args.iter().any(|a| a == "--diagnose") {
        if herdr::herdr_binary().is_err() {
            let agents = herdr::enrich(standalone::agents());
            println!("Sin Herdr · modo autónomo: {} agentes", agents.len());
            for a in agents { println!("{} | {} | {} | {} | {} | {}", a.id, a.name, a.status, a.project, a.action.clone().unwrap_or_default(), a.activity); }
            return;
        }
        match herdr::fetch_snapshot(&session) {
            Ok(mut agents) => {
                shells::add_shells(&mut agents, &session);
                println!("OK: {} agentes", agents.len());
                for a in agents {
                    let left = a.limit_left(herdr::unix_now()).map(|s| format!(" | resets in {}", scene::countdown(s))).unwrap_or_default();
                    println!("{} | {} | {} | {} | {}{left}", a.id, a.name, a.status, a.project, a.activity);
                    for s in &a.shells { println!("    ⚗ {} | {} | {}s | {} | {}", s.pid, s.label, herdr::unix_now() - s.started, s.task.clone().unwrap_or_default(), s.last_line.clone().unwrap_or_default()); }
                    for r in &a.shell_results { println!("    ✓ {} | {} | {}", r.task, r.ok, r.summary); }
                }
            }
            Err(error) => { eprintln!("{error}"); std::process::exit(1); }
        }
        return;
    }
    let demo = args.iter().any(|a| a == "--demo");
    let fullscreen = args.iter().any(|a| a == "--fullscreen");
    let on_top = prefs.on_top;
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Agents Pixel Dungeon")
        .with_app_id("agents-pixel-dungeon")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(true)
        .with_inner_size([scene::width_for(2), 400.0])
        .with_min_inner_size([scene::width_for(1), 200.0]);
    if let Some(icon) = assets::decode_png(include_bytes!("../Resources/Icon/icon_256.png")) {
        let [w, h] = icon.size;
        viewport = viewport.with_icon(egui::IconData { rgba: icon.as_raw().to_vec(), width: w as u32, height: h as u32 });
    }
    if on_top { viewport = viewport.with_always_on_top(); }
    let options = eframe::NativeOptions { viewport, centered: false, ..Default::default() };
    let result = eframe::run_native("Agents Pixel Dungeon", options, Box::new(move |cc| Ok(Box::new(App::new(cc, prefs, session, demo, fullscreen)))));
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

struct App {
    monitor: Monitor,
    prefs: Prefs,
    sounds: Sounds,
    notifier: Notifier,
    scene: Scene,
    assets: Assets,
    tray: Tray,
    chat: Option<ChatState>,
    compose: ComposeState,
    hud: HudState,
    show_log: bool,
    /// The guild service's view: members as rooms, attempts, the quest board.
    guild: guild_view::GuildView,
    show_board: bool,
    show_studio: bool,
    shot_asked: bool,
    visible: bool,
    first_frame: bool,
    /// --fullscreen: go full screen on the first frame.
    start_fullscreen: bool,
    /// The size we asked for; anything else is the user resizing.
    expected: Option<Vec2>,
    /// The place we asked for, until the window reports it: its rect lags a
    /// frame or more behind.
    commanded_origin: Option<egui::Pos2>,
    commanded_at: Option<Instant>,
    last_size: Option<Vec2>,
    size_changed_at: Option<f64>,
    resize_start: Option<Rect>,
    grip: bool,
    last_outer: Option<Rect>,
    /// The chat (agent id) or "compose" panel that last took the keyboard.
    key_panel: Option<String>,
    login: Option<auto_launch::AutoLaunch>,
    login_enabled: bool,
    /// A panel opened while the widget was one room wide: it widens to two
    /// rooms meanwhile, so the chat has room, and narrows back on close.
    widened: bool,
    /// The width / height the user resized the widget to: kept as it is
    /// from then on, whatever the agents or the chat panel need.
    user_width: Option<f32>,
    user_height: Option<f32>,
    /// The chat panel's height when its edge was grabbed, while it is dragged.
    split_from: Option<f32>,
    /// The standalone banner: hidden for this run, an install in progress, its outcome.
    banner_dismissed: bool,
    installing: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    herdr_note: Option<String>,
    opened_herdr: bool,
    /// Full screen: the rooms fill the left side, the console the right.
    fullscreen: bool,
    /// When full screen was last toggled: the window is animating, so its
    /// size is neither tracked nor fitted until it settles.
    fullscreen_at: Option<Instant>,
    /// Where the widget was before going full screen, to come back to.
    widget_rect: Option<Rect>,
    /// Leaving full screen: once the window has settled, the app is a
    /// menu-bar app again, on top again, back where it was.
    leaving_fullscreen: bool,
    /// Entering full screen: the toggle itself waits for this moment.
    enter_fullscreen_at: Option<Instant>,
    /// Full screen's HP viewer: each harness's plan left, re-read now and then.
    usages: Vec<usage::HarnessUsage>,
    usage_rx: Option<std::sync::mpsc::Receiver<Vec<usage::HarnessUsage>>>,
    next_usage: f64,
    terminals: terminal::Terminals,
    /// Full screen's calls to action already answered: the state each agent
    /// was in when its chat was open. It is called again once that changes.
    calls_seen: std::collections::HashMap<String, String>,
    /// Full screen opened a room for this visit already (only on entering,
    /// so esc can still close it).
    room_opened: bool,
    /// A repo's emblem being edited, and whether it is the demo's (edited
    /// on screen only, never kept).
    emblem_editor: Option<(ui::EmblemEditor, bool)>,
    codex_settings: Option<codex_settings::Editor>,
}

/// The console's narrowest width in full screen; the rooms get whole
/// columns in the rest.
/// Room above the console's header in full screen, for the hover controls.
const CONSOLE_TOP: f32 = 44.0;

impl App {
    fn new(cc: &eframe::CreationContext<'_>, prefs: Prefs, session: String, demo: bool, fullscreen: bool) -> App {
        let ctx = cc.egui_ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let mut monitor = Monitor::new(session, StatusFilter::from_raw(&prefs.filter));
        monitor.set_wake(wake.clone());
        if demo { monitor.demo = true; }
        let mut guild = guild_view::GuildView::new(wake.clone());
        guild.set_demo(demo);
        guild.start();
        let mut scene = Scene::new();
        scene.reduced_motion = reduce_motion();
        scene.overrides = prefs.projects.clone();
        let mut prefs = prefs;
        // Repos that came to share a colour each get their own, once.
        if crate::identity::spread_colours(&mut prefs.emblems) { prefs.save(); }
        scene.emblems = prefs.emblems.clone();
        let login = login_item();
        let login_enabled = login.as_ref().map(|l| l.is_enabled().unwrap_or(false)).unwrap_or(false);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::TRANSPARENT;
        // Menus, popups and tooltips sit on an opaque dark card.
        visuals.window_fill = Color32::from_rgb(30, 27, 36);
        visuals.window_stroke = egui::Stroke::new(1.0_f32, Color32::from_rgb(90, 84, 104));
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(30, 27, 36);
        visuals.extreme_bg_color = Color32::from_rgb(8, 9, 11);
        cc.egui_ctx.set_visuals(visuals);
        let mut app = App {
            monitor,
            terminals: terminal::Terminals::default(),
            calls_seen: Default::default(),
            room_opened: false,
            emblem_editor: None,
            codex_settings: None,
            prefs,
            sounds: Sounds::new(),
            notifier: Notifier::new(),
            scene,
            assets: Assets::new(&cc.egui_ctx),
            tray: Tray::new(&MenuState { sounds: true, sound_help: true, sound_done: true, notify: true, notify_help: true, notify_done: false, on_top: true, fullscreen: false, login: false, sessions: vec![], session: String::new(), demo: false, attention: 0, open_at_start: false, text_scale: 1.0, console_scale: 1.0, room_size: String::new() }, wake),
            chat: None,
            compose: ComposeState::default(),
            hud: HudState::default(),
            show_log: false,
            guild,
            show_board: false,
            show_studio: false,
            shot_asked: false,
            visible: true,
            first_frame: true,
            start_fullscreen: fullscreen,
            expected: None,
            commanded_origin: None,
            commanded_at: None,
            last_size: None,
            size_changed_at: None,
            resize_start: None,
            grip: false,
            last_outer: None,
            key_panel: None,
            login,
            login_enabled,
            widened: false,
            user_width: None,
            user_height: None,
            split_from: None,
            banner_dismissed: false,
            installing: None,
            herdr_note: None,
            opened_herdr: false,
            fullscreen: false,
            fullscreen_at: None,
            widget_rect: None,
            leaving_fullscreen: false,
            enter_fullscreen_at: None,
            usages: vec![],
            usage_rx: None,
            next_usage: 0.0,
        };
        ui::set_text_scale(app.prefs.text_scale);
        ui::set_console_scale(app.prefs.console_scale);
        app.monitor.refresh();
        // HPD_TEST_COMPOSE: the summon form already open (for captures).
        if std::env::var_os("HPD_TEST_COMPOSE").is_some() { app.monitor.set_composing(true); }
        app
    }

    fn menu_state(&self) -> MenuState {
        MenuState {
            sounds: self.prefs.sound_enabled,
            sound_help: self.prefs.sound_needs_help,
            sound_done: self.prefs.sound_finished,
            notify: self.prefs.notify_enabled,
            notify_help: self.prefs.notify_needs_help,
            notify_done: self.prefs.notify_finished,
            on_top: self.prefs.on_top,
            fullscreen: self.fullscreen,
            login: self.login_enabled,
            sessions: self.monitor.sessions.clone(),
            session: self.monitor.session.clone(),
            demo: self.monitor.demo,
            attention: self.monitor.agents.iter().filter(|a| a.status == "blocked").count(),
            open_at_start: self.prefs.open_herdr_at_start,
            text_scale: self.prefs.text_scale,
            console_scale: self.prefs.console_scale,
            room_size: self.prefs.room_size.clone(),
        }
    }

    // ---- Window geometry

    /// Columns shown: the user's, but never more than there are agents
    /// until the user resizes the widget; then as many as fit its width.
    fn shown_columns(&self) -> usize {
        scene::columns_fitting(self.target_width())
    }

    fn panel_open(&self) -> bool {
        self.monitor.composing || self.chat.is_some()
    }

    /// Height of everything that is not rooms: insets, HUD and chat panel.
    fn chrome(&self) -> f32 {
        self.chrome_with(self.prefs.panel_height)
    }

    /// The same with the chat panel `panel` tall while one is open.
    fn chrome_with(&self, panel: f32) -> f32 {
        TOP_INSET + BOTTOM_INSET + if self.panel_open() { panel } else { 0.0 } + if self.monitor.shows_hud() { ui::hud_height(&self.hud, &self.monitor) } else { 0.0 }
    }

    /// Height that shows the agents' rows, up to the user's row count (at
    /// least one), capped to the screen. Once the user resized the widget,
    /// its height instead, the chat panel taking room from the rooms.
    fn target_height(&self, screen: Vec2) -> f32 {
        let columns = self.shown_columns();
        let needed = (self.monitor.visible_agents().len() + columns - 1) / columns;
        layout_height(needed, self.prefs.rows, self.chrome(), self.user_height, screen.y - SCREEN_TOP - 2.0 * MARGIN)
    }

    fn target_width(&self) -> f32 {
        layout_width(self.prefs.columns, self.monitor.agents.len(), self.widened && self.panel_open(), self.user_width)
    }

    /// The window's outer rect, and whether its place is known: Wayland
    /// tells a window its size but not where it is.
    fn window_rect(ctx: &egui::Context) -> (Rect, bool) {
        ctx.input(|i| i.viewport().outer_rect.map(|r| (r, true)).unwrap_or_else(|| (Rect::from_min_size(egui::Pos2::ZERO, i.screen_rect().size()), false)))
    }

    fn command_size(&mut self, ctx: &egui::Context, (outer, placed): (Rect, bool), size: Vec2, anchor_left: bool) {
        // A place just asked for counts before the window reports it, or the
        // stale rect would move the window back.
        let outer = match (self.commanded_origin, self.expected) {
            (Some(origin), Some(expected)) => Rect::from_min_size(origin, expected),
            _ => outer,
        };
        let x = if anchor_left { outer.min.x } else { outer.max.x - size.x };
        if placed {
            let origin = egui::pos2(x, outer.min.y);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(origin));
            self.commanded_origin = Some(origin);
        }
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        self.expected = Some(size);
        self.commanded_at = Some(Instant::now());
    }

    /// Resize the window to whole rooms. Changes in the agent count keep the
    /// top-right corner where the user put it; a resize by the user keeps
    /// the corner opposite the one dragged (the top-left for the grip).
    fn fit_size(&mut self, ctx: &egui::Context, anchor_left: bool) {
        // Full screen is the screen's size, whatever the rooms need.
        if self.fullscreen || self.fullscreen_at.is_some() { return; }
        let window = Self::window_rect(ctx);
        let outer = window.0;
        let screen = ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1440.0, 900.0));
        // The chat panel gives way down to its least before the last row of rooms.
        ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(vec2(scene::width_for(1), self.chrome_with(MIN_PANEL) + ROW_H)));
        if self.resize_start.is_some() { return; }
        let size = vec2(self.target_width(), self.target_height(screen));
        if (outer.width() - size.x).abs() > 0.5 || (outer.height() - size.y).abs() > 0.5 {
            self.command_size(ctx, window, size, anchor_left);
        }
    }

    /// Put the widget back where it was, if that spot is still on the
    /// screen; otherwise in the top-right corner.
    fn restore_position(&mut self, ctx: &egui::Context) {
        let screen = ctx.input(|i| i.viewport().monitor_size).unwrap_or(vec2(1440.0, 900.0));
        let size = vec2(self.target_width(), self.target_height(screen));
        // Where the screens are is only known on macOS; elsewhere the window's
        // monitor is taken to start at the origin.
        let mut screens = screen_areas();
        if screens.is_empty() { screens.push(Rect::from_min_max(egui::pos2(0.0, SCREEN_TOP), screen.to_pos2())); }
        let origin = widget_origin(self.prefs.top_right, size, &screens);
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(origin));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        self.commanded_origin = Some(origin);
        self.expected = Some(size);
        self.commanded_at = Some(Instant::now());
    }

    fn anchor_to_corner(&mut self, ctx: &egui::Context) {
        self.prefs.top_right = None;
        self.prefs.save();
        self.restore_position(ctx);
    }

    /// Watch the window: a size we did not ask for is the user resizing, and
    /// once it settles it snaps to whole rooms; a move is remembered.
    fn track_window(&mut self, ctx: &egui::Context, now: f64) {
        // Going full screen (and back) changes the size and place without
        // the user resizing: nothing to remember.
        if self.fullscreen || self.fullscreen_at.is_some() { return; }
        let (outer, placed) = Self::window_rect(ctx);
        let size = outer.size();
        let settled = self.commanded_at.map(|t| t.elapsed() > Duration::from_millis(600)).unwrap_or(true);
        if self.commanded_origin.is_some_and(|origin| settled || (outer.min - origin).length() < 1.0) { self.commanded_origin = None; }
        if let (Some(expected), Some(last)) = (self.expected, self.last_size) {
            let differs = (size.x - expected.x).abs() > 1.0 || (size.y - expected.y).abs() > 1.0;
            let moved = (size.x - last.x).abs() > 0.5 || (size.y - last.y).abs() > 0.5;
            if differs && moved && settled {
                if self.resize_start.is_none() { self.resize_start = self.last_outer; }
                self.size_changed_at = Some(now);
            }
        }
        if let (Some(start), Some(changed)) = (self.resize_start, self.size_changed_at) {
            // After the grip hands the drag to the system, the button's
            // release may never reach the app: only the pause counts then.
            let dragging = !self.grip && ctx.input(|i| i.pointer.any_down());
            if now - changed > 0.35 && !dragging {
                // The new size is kept as it is from then on: clicks, the chat
                // panel and the agents coming and going no longer change it.
                // The nearest columns and rows are remembered for the next
                // start. A side left alone keeps its setting.
                if (size.x - start.width()).abs() > 1.0 {
                    self.prefs.columns = snap_columns(size.x);
                    self.user_width = Some(size.x);
                    self.widened = false;
                }
                if (size.y - start.height()).abs() > 1.0 {
                    // Shorter than the chat and a row of rooms: the chat gives way.
                    if self.panel_open() { self.prefs.panel_height = fit_panel(self.prefs.panel_height, size.y, self.chrome_with(0.0)); }
                    self.prefs.rows = snap_rows(size.y, self.chrome());
                    self.user_height = Some(size.y);
                }
                self.prefs.save();
                let anchor_left = self.grip || (outer.min.x - start.min.x).abs() < 1.0;
                self.resize_start = None;
                self.size_changed_at = None;
                self.grip = false;
                self.fit_size(ctx, anchor_left);
            }
        } else if let (Some(last), true) = (self.last_outer, placed) {
            let moved = (outer.min.x - last.min.x).abs() > 0.5 || (outer.min.y - last.min.y).abs() > 0.5;
            if moved && settled {
                self.prefs.top_right = Some((outer.max.x, outer.min.y));
                self.prefs.save();
            }
        }
        self.last_size = Some(size);
        self.last_outer = Some(outer);
    }

    // ---- Full screen

    /// Fill the screen with the rooms on the left and the console on the
    /// right, or go back to the widget where it was.
    fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        if self.fullscreen == on { return; }
        self.fullscreen = on;
        self.fullscreen_at = Some(Instant::now());
        self.resize_start = None;
        self.size_changed_at = None;
        self.grip = false;
        // The platform's own full screen (on macOS a space of its own, with
        // the swipe between spaces). Getting there drops the window's
        // always-on-top level and makes the app a regular one; both come
        // back once the window has settled after leaving.
        if on {
            self.widget_rect = ctx.input(|i| i.viewport().outer_rect);
            self.show(ctx);
            prepare_fullscreen();
            self.enter_fullscreen_at = Some(Instant::now() + Duration::from_millis(250));
            ctx.request_repaint_after(Duration::from_millis(260));
        } else {
            self.enter_fullscreen_at = None;
            self.leaving_fullscreen = true;
            if !native_fullscreen(false) { ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false)); }
            ctx.request_repaint_after(Duration::from_millis(1250));
        }
    }

    /// Full screen left or entered by other means than the app's own (the
    /// system's gestures, Mission Control, the title bar's button): follow
    /// the window, so the layout, the app's kind and its place come back
    /// as they would after Esc. Nothing is done mid-transition.
    fn follow_native_fullscreen(&mut self, ctx: &egui::Context) {
        if self.enter_fullscreen_at.is_some() || self.fullscreen_at.is_some() { return; }
        let Some(native) = ctx.input(|i| i.viewport().fullscreen) else { return };
        if self.fullscreen && !native {
            self.fullscreen = false;
            self.leaving_fullscreen = true;
            self.fullscreen_at = Some(Instant::now());
            self.resize_start = None;
            self.size_changed_at = None;
            ctx.request_repaint_after(Duration::from_millis(1250));
        } else if !self.fullscreen && native {
            self.fullscreen = true;
            self.fullscreen_at = Some(Instant::now());
            self.resize_start = None;
            self.size_changed_at = None;
            self.grip = false;
            ctx.request_repaint_after(Duration::from_millis(1250));
        }
    }

    /// The toggle into full screen, once the app is ready for it.
    fn enter_fullscreen_when_due(&mut self, ctx: &egui::Context) {
        let Some(at) = self.enter_fullscreen_at else { return };
        if Instant::now() < at { ctx.request_repaint_after(at - Instant::now()); return; }
        self.enter_fullscreen_at = None;
        self.fullscreen_at = Some(Instant::now());
        if !native_fullscreen(true) { ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true)); }
    }

    fn place(&mut self, ctx: &egui::Context, rect: Rect) {
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(rect.min));
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(rect.size()));
        self.commanded_origin = Some(rect.min);
        self.expected = Some(rect.size());
        self.commanded_at = Some(Instant::now());
    }

    /// Once the window has settled after a toggle, the widget's size is
    /// fitted to its rooms again (full screen needs nothing).
    fn settle_fullscreen(&mut self, ctx: &egui::Context) -> bool {
        let Some(at) = self.fullscreen_at else { return false };
        if at.elapsed() < Duration::from_millis(1200) { return false; }
        self.fullscreen_at = None;
        self.expected = None;
        self.last_size = None;
        self.last_outer = None;
        if self.leaving_fullscreen {
            self.leaving_fullscreen = false;
            set_regular_app(false);
            if self.prefs.on_top { ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop)); }
            match self.widget_rect.take() {
                Some(rect) => self.place(ctx, rect),
                None => self.restore_position(ctx),
            }
            self.show(ctx);
        }
        !self.fullscreen
    }

    // ---- Showing and hiding

    fn show(&mut self, ctx: &egui::Context) {
        self.visible = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    /// Show the dungeon with an agent's chat open, clearing a filter that hides it.
    fn open_agent(&mut self, ctx: &egui::Context, id: &str) {
        if !self.monitor.agents.iter().any(|a| a.id == id) { return self.show(ctx); }
        if !self.monitor.visible_agents().iter().any(|a| a.id == id) {
            self.monitor.set_filter(StatusFilter::All);
            self.monitor.set_query(String::new());
            self.prefs.filter = "all".into();
            self.prefs.save();
        }
        self.monitor.select(Some(id.to_string()));
        self.show(ctx);
    }

    fn handle_tray(&mut self, ctx: &egui::Context, action: TrayAction) {
        match action {
            TrayAction::Toggle => { if self.visible { self.hide(ctx) } else { self.show(ctx) } }
            TrayAction::Reanchor => { self.set_fullscreen(ctx, false); self.anchor_to_corner(ctx); self.show(ctx); }
            TrayAction::Fullscreen => { let on = !self.fullscreen; self.set_fullscreen(ctx, on); }
            TrayAction::OnTop => {
                self.prefs.on_top = !self.prefs.on_top;
                self.prefs.save();
                ctx.send_viewport_cmd(ViewportCommand::WindowLevel(if self.prefs.on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal }));
            }
            TrayAction::Login => {
                if let Some(login) = &self.login {
                    let result = if self.login_enabled { login.disable() } else { login.enable() };
                    match result {
                        Ok(()) => self.login_enabled = !self.login_enabled,
                        Err(error) => {
                            rfd::MessageDialog::new().set_title("Agents Pixel Dungeon").set_description(format!("{}\n{}", tr("El sistema no permitió abrir la app al iniciar sesión."), error)).show();
                        }
                    }
                }
            }
            TrayAction::Sounds => { self.prefs.sound_enabled = !self.prefs.sound_enabled; self.prefs.save(); }
            TrayAction::SoundHelp => { self.prefs.sound_needs_help = !self.prefs.sound_needs_help; self.prefs.save(); self.sounds.play(alerts::AlertKind::NeedsHelp); }
            TrayAction::SoundDone => { self.prefs.sound_finished = !self.prefs.sound_finished; self.prefs.save(); self.sounds.play(alerts::AlertKind::Finished); }
            TrayAction::Preview => self.sounds.preview(),
            TrayAction::Notify => { self.prefs.notify_enabled = !self.prefs.notify_enabled; self.prefs.save(); }
            TrayAction::NotifyHelp => { self.prefs.notify_needs_help = !self.prefs.notify_needs_help; self.prefs.save(); }
            TrayAction::NotifyDone => { self.prefs.notify_finished = !self.prefs.notify_finished; self.prefs.save(); }
            TrayAction::Session(name) => {
                if self.monitor.demo { self.monitor.set_demo(false); }
                self.monitor.set_session(&name);
                self.prefs.session = Some(name);
                self.prefs.save();
                self.chat = None;
            }
            TrayAction::Demo => { let demo = !self.monitor.demo; self.monitor.set_demo(demo); self.guild.set_demo(demo); self.monitor.set_extra(self.guild.agents()); self.chat = None; }
            TrayAction::QuestBoard => self.show_board = !self.show_board,
            TrayAction::Guild => self.show_studio = !self.show_studio,
            TrayAction::OpenHerdr => self.open_herdr(),
            TrayAction::OpenAtStart => { self.prefs.open_herdr_at_start = !self.prefs.open_herdr_at_start; self.prefs.save(); }
            TrayAction::TextBigger => self.set_text_scale(self.prefs.text_scale + 0.1),
            TrayAction::TextSmaller => self.set_text_scale(self.prefs.text_scale - 0.1),
            TrayAction::TextNormal => self.set_text_scale(1.0),
            TrayAction::ConsoleBigger => self.step_console_scale(1),
            TrayAction::ConsoleSmaller => self.step_console_scale(-1),
            TrayAction::ConsoleNormal => self.set_console_scale(1.0),
            TrayAction::RoomSize(size) => { self.prefs.room_size = size.to_string(); self.prefs.save(); }
            TrayAction::About => {
                rfd::MessageDialog::new().set_title(tr("Acerca de Agents Pixel Dungeon")).set_description(format!("Agents Pixel Dungeon {}\n{}", env!("CARGO_PKG_VERSION"), tr("Creado por Diabolo Studio (diabolostudio.com) · Nacho Valencia.\nCódigo y pixel art originales · MIT.\n\nProyecto independiente, sin afiliación con Herdr, Inc."))).show();
            }
            TrayAction::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
    }

    fn set_text_scale(&mut self, scale: f32) {
        self.prefs.text_scale = scale.clamp(0.7, 1.6);
        self.prefs.save();
        ui::set_text_scale(self.prefs.text_scale);
    }

    /// The console's own text size, apart from the panels'.
    fn set_console_scale(&mut self, scale: f32) {
        self.prefs.console_scale = scale.clamp(0.6, 2.5);
        self.prefs.save();
        ui::set_console_scale(self.prefs.console_scale);
    }

    fn step_console_scale(&mut self, step: i8) {
        if step != 0 { self.set_console_scale(self.prefs.console_scale * if step > 0 { 1.15 } else { 1.0 / 1.15 }); }
    }

    /// Open Herdr in a terminal window (its server stays up afterwards).
    fn open_herdr(&mut self) {
        self.herdr_note = standalone::open_herdr().err().map(|_| tr("No se pudo abrir una terminal con Herdr."));
    }

    /// Run Herdr's installer in the background.
    fn install_herdr(&mut self) {
        if self.installing.is_some() { return; }
        let (tx, rx) = std::sync::mpsc::channel();
        self.herdr_note = None;
        std::thread::spawn(move || { let _ = tx.send(standalone::install_herdr()); });
        self.installing = Some(rx);
    }

    /// A terminal to type a command in: in that agent's folder, or in the
    /// open chat's, else where agents were last summoned, else home.
    fn open_terminal_for(&mut self, ctx: &egui::Context, id: Option<&str>) {
        let id = id.map(str::to_string).or_else(|| self.chat.as_ref().map(|c| c.agent_id.clone()));
        let cwd = id.and_then(|id| self.monitor.agents.iter().find(|a| a.id == id).map(|a| a.cwd.clone())).unwrap_or_else(|| self.prefs.create_folder.clone());
        self.terminals.open(ctx, &cwd);
    }

    /// Ask before typing /exit into an agent's chat.
    fn confirm_finish(&mut self, id: &str) {
        let Some(agent) = self.monitor.agents.iter().find(|a| a.id == id).cloned() else { return };
        let answer = rfd::MessageDialog::new()
            .set_title(trf("¿Finalizar {}?", &[&agent.project]))
            .set_description(tr("Se escribirá /exit en su chat de Herdr."))
            .set_level(rfd::MessageLevel::Warning)
            .set_buttons(rfd::MessageButtons::OkCancelCustom(tr("Finalizar"), tr("Cancelar")))
            .show();
        if matches!(answer, rfd::MessageDialogResult::Ok | rfd::MessageDialogResult::Custom(_)) {
            if let rfd::MessageDialogResult::Custom(label) = &answer { if *label != tr("Finalizar") { return; } }
            let _ = self.monitor.finish(&agent);
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        // HPD_TEST_HIDE=1: hide after 2 s and show again after 6 s, to check
        // that a hidden window still wakes up (what the tray toggle relies on).
        if std::env::var("HPD_TEST_HIDE").is_ok() {
            if now > 2.0 && now < 6.0 && self.visible { self.hide(ctx); }
            if now > 6.0 && !self.visible { self.show(ctx); }
        }
        // HPD_TEST_SELECT=1: open the first room's chat after 1 s (for captures).
        // A value other than 1 names the room to open. HPD_TEST_BOARD=1 opens the quest board.
        if let Ok(which) = std::env::var("HPD_TEST_SELECT") {
            if now > 1.0 && now < 1.2 && self.monitor.selected.is_none() {
                let id = if which == "1" { self.monitor.visible_agents().first().map(|a| a.id.clone()) } else { Some(which) };
                if let Some(id) = id { self.monitor.select(Some(id)); }
            }
        }
        if let Ok(v) = std::env::var("HPD_TEST_BOARD") {
            if now > 1.0 && now < 1.2 { self.show_board = true; self.guild.form.open = v == "form"; }
        }
        // HPD_TEST_STUDIO=guild|experience|loops|parts opens the guild window on that tab.
        if let Ok(v) = std::env::var("HPD_TEST_STUDIO") {
            if now > 1.0 && now < 1.2 {
                self.show_studio = true;
                self.guild.studio.tab = match v.as_str() { "concept" => guild_studio::Tab::Concept, "experience" => guild_studio::Tab::Experience, "loops" => guild_studio::Tab::Loops, "systems" => guild_studio::Tab::Systems, "parts" => guild_studio::Tab::Parts, "play" => guild_studio::Tab::Play, "laboratory" => guild_studio::Tab::Laboratory, "questions" => guild_studio::Tab::Questions, "decisions" => guild_studio::Tab::Decisions, _ => guild_studio::Tab::Guild };
            }
        }
        if std::env::var("HPD_TEST_DIFF").is_ok() && now > 1.0 && now < 1.2 { self.guild.diff_open = true; }
        // HPD_TEST_SHOT=<file.png>: save what the window shows after 3 s (or
        // HPD_TEST_SHOT_AT seconds), for captures when the screen cannot be
        // grabbed (another space is full screen).
        if let Ok(path) = std::env::var("HPD_TEST_SHOT") {
            let at = std::env::var("HPD_TEST_SHOT_AT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(3.0);
            if now > at && !self.shot_asked {
                self.shot_asked = true;
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
            }
            let shots: Vec<Arc<egui::ColorImage>> = ctx.input(|i| i.events.iter().filter_map(|e| match e { egui::Event::Screenshot { image, .. } => Some(image.clone()), _ => None }).collect());
            for image in shots {
                let [w, h] = image.size;
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(png) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) { let _ = png.save(&path); }
            }
        }
        if self.first_frame {
            self.first_frame = false;
            self.restore_position(ctx);
            if self.start_fullscreen { self.set_fullscreen(ctx, true); }
        }
        let mut needs_fit = self.monitor.poll();
        // The guild's members are rooms too; what the service says goes to the log.
        if self.guild.poll() {
            self.monitor.set_extra(self.guild.agents());
            needs_fit = true;
        }
        for (text, tone) in std::mem::take(&mut self.guild.notes) { self.monitor.log(text, tone); }
        if ctx.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::G)) { self.show_studio = !self.show_studio; }
        else if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::G)) { self.show_board = !self.show_board; }
        // Sounds and notifications for what just changed.
        let alerts: Vec<alerts::AgentAlert> = std::mem::take(&mut self.monitor.alerts);
        if let Some(first) = alerts.first() {
            if wants_sound(&self.prefs, first.kind) { self.sounds.play(first.kind); }
            self.notifier.post(&self.prefs, &alerts);
        }
        for id in self.notifier.clicked() { self.open_agent(ctx, &id); needs_fit = true; }
        for action in self.tray.poll() { self.handle_tray(ctx, action); needs_fit = true; }
        // ⌘+ / ⌘- / ⌘0 change the text size; with ⇧ the console's. The
        // physical key counts too: with ⇧ the "+" key types another character
        // on many layouts.
        let steps: Vec<(i8, bool)> = ctx.input(|i| {
            i.events.iter().filter_map(|e| match e {
                egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } if modifiers.command => {
                    let is = |k: egui::Key| *key == k || *physical_key == Some(k);
                    let step = if is(egui::Key::Plus) || is(egui::Key::Equals) { 1 } else if is(egui::Key::Minus) { -1 } else if is(egui::Key::Num0) { 0 } else { return None };
                    Some((step, modifiers.shift))
                }
                _ => None,
            }).collect()
        });
        for (step, console) in steps {
            match (console, step) {
                (true, 0) => self.set_console_scale(1.0),
                (true, step) => self.step_console_scale(step),
                (false, 0) => self.set_text_scale(1.0),
                (false, step) => self.set_text_scale(self.prefs.text_scale + 0.1 * step as f32),
            }
        }
        // F11 (and ⌃⌘F on macOS) toggle full screen; esc leaves it once
        // nothing else is open to close.
        let (toggle, escape) = ctx.input(|i| {
            let mac = cfg!(target_os = "macos") && i.modifiers.mac_cmd && i.modifiers.ctrl && i.key_pressed(egui::Key::F);
            (i.key_pressed(egui::Key::F11) || mac, i.key_pressed(egui::Key::Escape))
        });
        if toggle { let on = !self.fullscreen; self.set_fullscreen(ctx, on); }
        if escape && self.show_studio { self.show_studio = false; }
        else if escape && self.show_board { self.show_board = false; }
        else if escape && self.fullscreen && !self.panel_open() && !self.hud.searching && !self.show_log { self.set_fullscreen(ctx, false); }
        self.enter_fullscreen_when_due(ctx);
        self.follow_native_fullscreen(ctx);
        if self.settle_fullscreen(ctx) { needs_fit = true; }
        // The installer's outcome.
        if let Some(rx) = &self.installing {
            if let Ok(result) = rx.try_recv() {
                self.installing = None;
                self.herdr_note = Some(match result { Ok(_) => tr("Herdr instalado. Ábrelo y lanza tus agentes dentro."), Err(e) => trf("La instalación falló: {}", &[&e]) });
            }
        }
        // Open Herdr once at startup when asked and its server is down.
        if self.prefs.open_herdr_at_start && !self.opened_herdr && self.monitor.herdr == HerdrState::Down {
            self.opened_herdr = true;
            self.open_herdr();
        }
        self.tray.update(&self.menu_state());
        self.track_window(ctx, now);

        // Full screen has one whole room: entering it with none chosen opens
        // the first one at work (once, so esc can still close it).
        if !self.fullscreen { self.room_opened = false; }
        if self.fullscreen && !self.room_opened && !self.monitor.agents.is_empty() {
            self.room_opened = true;
            if self.monitor.selected.is_none() {
                if let Some(id) = scene::first_room(&self.monitor.visible_agents()) { self.monitor.select(Some(id)); }
            }
        }
        // A panel opening at one column widens the widget while it is open.
        let was_open = self.chat.is_some() || self.key_panel.is_some();
        let opening = (self.monitor.selected.is_some() || self.monitor.composing) && !was_open;
        if opening && self.shown_columns() == 1 { self.widened = true; }
        if !self.monitor.composing && self.monitor.selected.is_none() && self.widened { self.widened = false; needs_fit = true; }
        // The chat panel follows the selection.
        match (&self.monitor.selected, &self.chat) {
            (Some(id), Some(chat)) if &chat.agent_id == id => {}
            (Some(id), _) => {
                // HPD_TEST_DRAFT: text already in the box (for captures).
                self.chat = Some(ChatState::new(id).with_draft(std::env::var("HPD_TEST_DRAFT").unwrap_or_default()));
                needs_fit = true;
            }
            (None, Some(_)) => { self.chat = None; needs_fit = true; }
            (None, None) => {}
        }
        // Typing in the chat or new-agent panel needs the keyboard, but only
        // take it when one opens: later updates must not pull typing away.
        let panel = self.monitor.selected.clone().or_else(|| self.monitor.composing.then(|| "compose".to_string()));
        if panel.is_some() && panel != self.key_panel { ctx.send_viewport_cmd(ViewportCommand::Focus); }
        self.key_panel = panel;
        self.scene.set_selected(self.monitor.selected.clone());
        self.scene.empty_text = if !self.monitor.agents.is_empty() { tr("Ningún agente coincide") } else if self.monitor.error.is_some() { tr("Esperando a Herdr…") } else { tr("Sin agentes en la sesión") };
        let mut visible_agents = self.monitor.visible_agents();
        // Full screen keeps each room in its place; who waits for you is
        // called on top instead.
        // The order the user dragged the rooms to comes first.
        if self.fullscreen {
            let placed = |a: &herdr::Agent| self.prefs.room_order.iter().position(|folder| *folder == a.cwd).unwrap_or(usize::MAX);
            visible_agents.sort_by(|a, b| placed(a).cmp(&placed(b)).then_with(|| herdr::natural_cmp(&a.id, &b.id)));
        }
        self.scene.keep_order = self.fullscreen;
        self.scene.compact = self.fullscreen;
        self.scene.room_size = scene::RoomSize::from_key(&self.prefs.room_size);
        self.scene.sync(&visible_agents);
        if self.scene.emblems_added {
            self.scene.emblems_added = false;
            self.prefs.emblems = self.scene.emblems.clone();
            self.prefs.save();
        }
        if let Some(agent) = self.monitor.selected.as_ref().and_then(|id| visible_agents.iter().find(|a| &a.id == id)) {
            self.calls_seen.insert(agent.id.clone(), agent.status.clone());
        }
        self.calls_seen.retain(|id, status| visible_agents.iter().any(|a| &a.id == id && &a.status == status));
        let calls: Vec<herdr::Agent> = if self.fullscreen {
            let mut waiting: Vec<herdr::Agent> = visible_agents.iter().filter(|a| matches!(a.status.as_str(), "blocked" | "done") && !self.calls_seen.contains_key(&a.id)).cloned().collect();
            waiting.sort_by_key(|a| a.rank());
            waiting
        } else { Vec::new() };
        let calls_h = ui::calls_height(calls.len());

        let hovering = ctx.input(|i| i.pointer.hover_pos()).is_some();
        let note = connection_note(self.monitor.error.as_deref(), self.monitor.updated, Instant::now(), 5.0);
        let panel_open = self.panel_open();
        let shows_hud = self.monitor.shows_hud();

        let fullscreen = self.fullscreen;
        // The HP viewer's data, every 30 s while in full screen.
        if fullscreen && now >= self.next_usage && self.usage_rx.is_none() {
            // The demo shows made-up meters, never the user's own.
            if self.monitor.demo { self.usages = usage::demo(chrono::Local::now().timestamp()); } else { self.usage_rx = Some(usage::fetch(herdr::home_dir(), self.prefs.kiro_plan, self.prefs.claude_usage_live)); }
            self.next_usage = now + 30.0;
        }
        if let Some(rx) = &self.usage_rx {
            if let Ok(found) = rx.try_recv() { self.usages = found; self.usage_rx = None; }
        }
        let usage_h = if fullscreen { ui::usage_height(ctx, &self.usages, self.prefs.guild_hp_open) } else { 0.0 };
        egui::CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
            let full = ui.max_rect();
            // The widget has rounded corners; full screen has the screen's.
            let round = if fullscreen { 0 } else { 14 };
            ui.painter().rect_filled(full, CornerRadius::same(round), BACKGROUND);
            let hud_h = if shows_hud { ui::hud_height(&self.hud, &self.monitor) } else { 0.0 };
            // The rooms, the HUD under them and the chat or summon panel:
            // stacked in the widget; side by side in full screen, where the
            // rooms take one column on the left and the console all the rest.
            let (scene_rect, hud_rect, panel_rect) = if fullscreen {
                let split = full.min.x + scene::width_for(1);
                let scene_rect = Rect::from_min_max(egui::pos2(full.min.x, full.min.y + calls_h), egui::pos2(split, full.max.y - hud_h - usage_h));
                let hud_rect = Rect::from_min_max(egui::pos2(full.min.x, full.max.y - hud_h), egui::pos2(split, full.max.y));
                (scene_rect, hud_rect, Rect::from_min_max(egui::pos2(split, full.min.y), full.max))
            } else {
                let panel_h = if panel_open { self.prefs.panel_height.min((full.height() - hud_h).max(0.0)) } else { 0.0 };
                let scene_rect = Rect::from_min_max(full.min, egui::pos2(full.max.x, full.max.y - panel_h - hud_h));
                let hud_rect = Rect::from_min_max(egui::pos2(full.min.x, scene_rect.max.y), egui::pos2(full.max.x, scene_rect.max.y + hud_h));
                (scene_rect, hud_rect, Rect::from_min_max(egui::pos2(full.min.x, hud_rect.max.y), full.max))
            };
            // Full screen: who waits for you, over the rooms.
            if calls_h > 0.0 {
                let rect = Rect::from_min_max(full.min, egui::pos2(scene_rect.max.x, scene_rect.min.y));
                if let Some(id) = ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| { ui.set_clip_rect(rect); ui::calls(ui, &calls, &self.scene) }).inner {
                    self.monitor.select(Some(id));
                }
            }
            // The dungeon, with a spare room to summon an agent in full screen.
            self.scene.summon_tile = fullscreen;
            self.scene.summoning = self.monitor.composing;
            let response = ui.scope_builder(egui::UiBuilder::new().max_rect(scene_rect), |ui| {
                ui.set_clip_rect(scene_rect);
                self.scene.ui(ui, &self.assets, now)
            }).inner;
            if note.is_some() {
                // The last known rooms stay, faded, while the data is not live.
                ui.painter().rect_filled(scene_rect, CornerRadius { nw: round, ne: round, sw: 0, se: 0 }, Color32::from_rgba_unmultiplied(13, 15, 17, 90));
            }
            // A room dragged to a new place: keep the order by folder, with
            // folders not showing now where they were, after these.
            if let Some(order) = response.reorder {
                let mut folders: Vec<String> = vec![];
                for id in &order {
                    if let Some(agent) = visible_agents.iter().find(|a| &a.id == id) {
                        if !folders.contains(&agent.cwd) { folders.push(agent.cwd.clone()); }
                    }
                }
                for kept in &self.prefs.room_order {
                    if !folders.contains(kept) { folders.push(kept.clone()); }
                }
                self.prefs.room_order = folders;
                self.prefs.save();
            }
            if let Some(id) = response.clicked {
                let next = if self.monitor.selected.as_deref() == Some(&id) { None } else { Some(id) };
                self.monitor.select(next);
            }
            // Files dragged over a room open its chat; dropped there, they go
            // into its message, ready to send.
            if let Some(id) = response.file_hover {
                if self.monitor.selected.as_deref() != Some(&id) { self.monitor.select(Some(id)); ctx.request_repaint(); }
            }
            if let Some(id) = response.file_drop {
                if self.monitor.selected.as_deref() != Some(&id) { self.monitor.select(Some(id.clone())); }
                if self.chat.as_ref().map(|c| c.agent_id != id).unwrap_or(true) { self.chat = Some(ChatState::new(&id)); }
                if let Some(chat) = &mut self.chat { chat.add_dropped(ctx); }
                ui::mark_drop_taken(ctx);
                needs_fit = true;
            }
            if let Some(id) = response.finish { self.confirm_finish(&id); }
            if let Some(id) = response.terminal { self.open_terminal_for(ctx, Some(&id)); }
            if let Some((emblem, repo_name, demo)) = response.edit_emblem.and_then(|id| self.scene.emblem_of(&id)) {
                let taken = identity::SavedEmblem::colours_of(self.scene.emblems.iter().filter(|(k, _)| **k != emblem.key).map(|(_, e)| e));
                let name = if emblem.name == repo_name { String::new() } else { emblem.name.clone() };
                self.emblem_editor = Some((ui::EmblemEditor { key: emblem.key.clone(), repo_name, name, emblem, taken }, demo));
            }
            if response.summon { let on = !self.monitor.composing; self.monitor.set_composing(on); }
            // The edge between the rooms and the open panel, the HUD with it:
            // dragged up the chat grows and the rooms give it their room,
            // down to a last row; the widget keeps its height. Added before
            // the HUD, whose chips and buttons stay on top for clicks.
            let mut split_hot = false;
            if panel_open && !fullscreen {
                let edge = Rect::from_x_y_ranges(full.x_range(), (scene_rect.max.y - 3.0)..=(panel_rect.min.y + 4.0));
                let split = ui.interact(edge, ui.id().with("panel-split"), egui::Sense::click_and_drag());
                if split.drag_started() {
                    self.split_from = Some(self.prefs.panel_height);
                    if self.user_height.is_none() { self.user_height = Some(full.height()); }
                }
                if let (true, Some(from), Some(origin), Some(pointer)) = (split.dragged(), self.split_from, ctx.input(|i| i.pointer.press_origin()), split.interact_pointer_pos()) {
                    self.prefs.panel_height = fit_panel(from + origin.y - pointer.y, full.height(), TOP_INSET + BOTTOM_INSET + hud_h);
                }
                if split.drag_stopped() {
                    self.split_from = None;
                    self.prefs.save();
                }
                // Over the HUD's own chips and buttons the pointer is theirs.
                split_hot = split.dragged() || (split.hovered() && ctx.viewport(|v| v.hits.click.map(|w| w.id)) == Some(split.id));
                if split_hot { ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical); }
            } else {
                self.split_from = None;
            }
            // Full screen summons in the spare room itself.
            if let (Some((rect, view)), true) = (response.summon_rect, fullscreen && self.monitor.composing) {
                let inner = rect.shrink(12.0);
                let action = ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                    ui.set_clip_rect(view.intersect(scene_rect));
                    ui::summon_form(ui, &self.monitor, &self.assets, &mut self.prefs, &mut self.compose, now)
                }).inner;
                if action.close { self.monitor.set_composing(false); }
            }
            // Full screen: the HP viewer between the rooms and the HUD.
            if usage_h > 0.0 {
                let rect = Rect::from_min_max(egui::pos2(scene_rect.min.x, scene_rect.max.y), egui::pos2(scene_rect.max.x, scene_rect.max.y + usage_h));
                ui.painter().rect_filled(rect, CornerRadius::ZERO, ui::HUD_FILL);
                ui.painter().hline(rect.x_range(), rect.min.y, egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 20)));
                let inner = Rect::from_min_max(egui::pos2(rect.min.x + 10.0, rect.min.y), egui::pos2(rect.max.x - 10.0, rect.max.y));
                let drawn = ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| { ui.set_clip_rect(rect); ui::usage_panel(ui, &self.assets, &self.usages, now, self.prefs.claude_usage_live, self.prefs.guild_hp_open) });
                // The open panel gets the height it really took, from the next frame on.
                let needed = drawn.response.rect.height() + 6.0;
                if self.prefs.guild_hp_open && (needed - usage_h).abs() > 0.5 {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new(ui::USAGE_DRAWN), needed));
                    ctx.request_repaint();
                }
                let picked = drawn.inner;
                if picked.codex_settings { self.codex_settings = Some(codex_settings::Editor::load()); }
                if picked.toggle_open { self.prefs.guild_hp_open = !self.prefs.guild_hp_open; self.prefs.save(); }
                if let Some(plan) = picked.kiro_plan { self.prefs.kiro_plan = plan; self.prefs.save(); self.next_usage = 0.0; }
                if let Some(live) = picked.claude_live { self.prefs.claude_usage_live = live; self.prefs.save(); self.next_usage = 0.0; }
            }
            // The HUD.
            if shows_hud {
                let bottom = if panel_open || fullscreen { 0 } else { 14 };
                ui.painter().rect_filled(hud_rect, CornerRadius { nw: 0, ne: 0, sw: bottom, se: bottom }, ui::HUD_FILL);
                ui.painter().hline(hud_rect.x_range(), hud_rect.min.y, egui::Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 20)));
                let inner = Rect::from_min_max(egui::pos2(hud_rect.min.x + 8.0, hud_rect.min.y), egui::pos2(hud_rect.max.x - 8.0, hud_rect.max.y));
                let action = ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| ui::hud(ui, &mut self.monitor, &mut self.hud)).inner;
                if action.toggle_log { self.show_log = !self.show_log; }
                if action.compose { let on = !self.monitor.composing; self.monitor.set_composing(on); }
            }
            // The chat or summon panel: under the rooms while one is open;
            // the console side in full screen, always.
            if panel_open || fullscreen {
                let line = if self.monitor.composing { Color32::from_rgb(140, 217, 115) } else {
                    self.chat.as_ref().and_then(|c| self.monitor.agents.iter().find(|a| a.id == c.agent_id)).map(|a| scene::agent_color(&a.status)).unwrap_or(Color32::GRAY)
                };
                let edge = Color32::from_rgba_unmultiplied(line.r(), line.g(), line.b(), if panel_open { 180 } else { 60 });
                let inner = if fullscreen {
                    ui.painter().rect_filled(panel_rect, CornerRadius::ZERO, ui::PANEL_FILL);
                    ui.painter().vline(panel_rect.min.x, panel_rect.y_range(), egui::Stroke::new(1.0_f32, edge));
                    Rect::from_min_max(egui::pos2(panel_rect.min.x + 14.0, panel_rect.min.y + CONSOLE_TOP), egui::pos2(panel_rect.max.x - 14.0, panel_rect.max.y - 12.0))
                } else {
                    ui.painter().rect_filled(panel_rect, CornerRadius { nw: 0, ne: 0, sw: 14, se: 14 }, ui::PANEL_FILL);
                    ui.painter().hline(panel_rect.x_range(), panel_rect.min.y, egui::Stroke::new(1.0_f32, edge));
                    Rect::from_min_max(egui::pos2(panel_rect.min.x + 10.0, panel_rect.min.y + 8.0), egui::pos2(panel_rect.max.x - 10.0, panel_rect.max.y - 8.0))
                };
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                    ui.set_clip_rect(panel_rect);
                    if self.monitor.composing && !fullscreen {
                        let action = ui::compose_panel(ui, &self.monitor, &mut self.prefs, &mut self.compose);
                        if action.close { self.monitor.set_composing(false); }
                    } else if let Some(chat) = &mut self.chat {
                        if let Some(agent) = self.monitor.agents.iter().find(|a| a.id == chat.agent_id).cloned() {
                            if agent.id.starts_with(guild_view::ID_PREFIX) {
                                let action = guild_view::panel(ui, &mut self.guild, &agent, now, fullscreen);
                                if let Some(path) = action.terminal { self.terminals.open(ctx, &path); }
                                if action.close { self.monitor.select(None); }
                            } else {
                                let action = ui::chat_panel(ui, &self.monitor, &agent, chat, now, fullscreen);
                                if action.terminal { self.terminals.open(ctx, &agent.cwd); }
                                if action.close { self.monitor.select(None); }
                                if action.console_step != 0 { self.step_console_scale(action.console_step); }
                            }
                        }
                    } else {
                        ui::console_placeholder(ui);
                    }
                });
                // The grabbed edge shows a bar to pull, as the drag handle does.
                if split_hot {
                    let bar = Rect::from_center_size(egui::pos2(panel_rect.center().x, panel_rect.min.y), vec2(56.0, 4.0));
                    ui.painter().rect_filled(bar, CornerRadius::same(2), Color32::from_rgba_unmultiplied(255, 255, 255, 190));
                }
            }
        });

        self.terminals.show(ctx);
        if let Some(editor) = &mut self.codex_settings {
            if !editor.show(ctx) { self.codex_settings = None; }
        }
        self.edit_emblem(ctx);

        // Overlays.
        let standalone = self.monitor.standalone() && matches!(self.monitor.herdr, HerdrState::Missing | HerdrState::Down);
        if standalone && !self.banner_dismissed {
            let action = ui::herdr_banner(ctx, self.monitor.herdr == HerdrState::Missing, self.installing.is_some(), self.herdr_note.as_deref());
            if action.install { self.install_herdr(); }
            if action.open { self.open_herdr(); }
            if action.dismiss { self.banner_dismissed = true; }
        } else if let Some((text, lost)) = &note { ui::connection_banner(ctx, text, *lost); }
        // Full screen has no strip of buttons: esc and the menu leave it, and
        // the bar under the rooms summons and opens the log.
        if (hovering || self.show_log) && !self.fullscreen {
            let action = ui::controls(ctx, self.prefs.sound_enabled, self.show_log, self.fullscreen);
            if action.compose { let on = !self.monitor.composing; self.monitor.set_composing(on); }
            if action.log { self.show_log = !self.show_log; }
            if action.sound { self.prefs.sound_enabled = !self.prefs.sound_enabled; self.prefs.save(); }
            if action.fullscreen { let on = !self.fullscreen; self.set_fullscreen(ctx, on); }
            if action.minimize { self.hide(ctx); }
            if action.close { ctx.send_viewport_cmd(ViewportCommand::Close); }
        }
        if self.show_log && ui::activity_log(ctx, &self.monitor.events) { self.show_log = false; }
        if self.show_studio {
            let action = guild_studio::studio(ctx, &mut self.guild);
            if action.close { self.show_studio = false; }
        }
        // A play session the guild opened: the game runs in a terminal; when
        // it exits, the session is closed and its telemetry read.
        if let Some(play) = self.guild.take_result("play") {
            let argv: Vec<String> = play["argv"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
            let env: Vec<(String, String)> = play["env"].as_object().into_iter().flatten().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect();
            if let (Some(cwd), Some(id)) = (play["cwd"].as_str(), play["playtest"]["id"].as_str()) { self.terminals.run(cwd, argv, env, id); }
        }
        for playtest in self.terminals.take_finished() {
            self.guild.command("play.finish", serde_json::json!({"playtestId": playtest}));
        }
        if self.show_board {
            let action = guild_view::board(ctx, &mut self.guild);
            if action.close { self.show_board = false; }
            if action.studio { self.show_studio = true; }
            if let Some(id) = action.open { self.monitor.select(Some(id)); needs_fit = true; }
        }
        if self.chat.is_none() { self.guild.watch(None); }
        // Full screen has nothing to drag or resize.
        if !self.fullscreen {
            ui::drag_handle(ctx);
            if ui::resize_grip(ctx, hovering) { self.grip = true; }
        }

        if self.monitor.changed { self.monitor.changed = false; needs_fit = true; }
        if self.monitor.filter.raw() != self.prefs.filter { self.prefs.filter = self.monitor.filter.raw().into(); self.prefs.save(); }
        if needs_fit { self.fit_size(ctx, false); }
        ctx.request_repaint_after(Duration::from_millis(if self.visible { 33 } else { 500 }));
    }
}

impl App {
    /// The emblem editor, while open: Save keeps the emblem (and the name,
    /// when it is not the repo's own) in the settings; Automatic forgets them,
    /// so the repo's emblem is worked out again with a free colour.
    fn edit_emblem(&mut self, ctx: &egui::Context) {
        let Some((editor, demo)) = &mut self.emblem_editor else { return };
        let demo = *demo;
        match ui::emblem_editor(ctx, editor) {
            ui::EditorOutcome::Open => return,
            ui::EditorOutcome::Cancel => {}
            ui::EditorOutcome::Save => {
                let key = editor.key.clone();
                let name = editor.name.trim().to_string();
                let saved = identity::SavedEmblem::of(&editor.emblem);
                let mut choice = self.prefs.projects.get(&key).cloned().unwrap_or_default();
                // What the editor shows now wins over a colour or charge set by hand.
                choice.color = None;
                choice.symbol = None;
                choice.kind = None;
                choice.name = (!name.is_empty() && name != editor.repo_name).then_some(name);
                if demo {
                    self.scene.emblems.insert(key.clone(), saved);
                    self.scene.overrides.insert(key, choice);
                } else {
                    self.prefs.emblems.insert(key.clone(), saved);
                    if choice == identity::ProjectOverride::default() { self.prefs.projects.remove(&key); } else { self.prefs.projects.insert(key, choice); }
                    self.prefs.save();
                    self.scene.emblems = self.prefs.emblems.clone();
                    self.scene.overrides = self.prefs.projects.clone();
                }
                self.scene.reload_emblems();
            }
            ui::EditorOutcome::Auto => {
                let key = editor.key.clone();
                self.scene.emblems.remove(&key);
                self.scene.overrides.remove(&key);
                if !demo {
                    self.prefs.emblems.remove(&key);
                    self.prefs.projects.remove(&key);
                    self.prefs.save();
                }
                self.scene.reload_emblems();
            }
        }
        self.emblem_editor = None;
    }
}

/// The widget's width: the width the user resized it to (at least a room),
/// or the user's columns but never more than there are agents; at least two
/// rooms while a panel widens a one-room widget.
fn layout_width(pref_columns: usize, agents: usize, widened: bool, user_width: Option<f32>) -> f32 {
    let width = match user_width {
        Some(width) => width.max(scene::width_for(1)),
        None => scene::width_for(pref_columns.min(agents.max(1))),
    };
    if widened { width.max(scene::width_for(2)) } else { width }
}

/// The widget's height: the agents' rows up to the user's row count, capped
/// to `limit`; or the height the user resized it to, which may span screens,
/// never under one row of rooms.
fn layout_height(needed_rows: usize, pref_rows: usize, chrome: f32, user_height: Option<f32>, limit: f32) -> f32 {
    if let Some(height) = user_height { return height.max(chrome + ROW_H); }
    let shown = needed_rows.max(1).min(pref_rows) as f32;
    let wanted = chrome + shown * ROW_H + (shown - 1.0) * GAP;
    if limit > 100.0 { wanted.min(limit) } else { wanted }
}

/// Where the widget's top-left corner goes: back where it was (its top-right
/// corner, `saved`) when that spot is on one of the `screens`, kept inside
/// that screen; otherwise the first screen's top-right corner. `screens` are
/// the usable areas in the window's coordinates, the menu-bar screen first.
fn widget_origin(saved: Option<(f32, f32)>, size: Vec2, screens: &[Rect]) -> egui::Pos2 {
    let Some(&first) = screens.first() else { return egui::pos2(MARGIN, SCREEN_TOP + MARGIN) };
    let spot = saved.and_then(|(right, top)| screens.iter().find(|s| s.contains(egui::pos2(right - 20.0, top + 20.0))).map(|s| (*s, right, top)));
    let (screen, right, top) = spot.unwrap_or((first, first.max.x - MARGIN, first.min.y + MARGIN));
    let x = (right - size.x).clamp(screen.min.x, (screen.max.x - size.x).max(screen.min.x));
    egui::pos2(x, top.max(screen.min.y))
}

/// The screens' usable areas (past the menu bar and the Dock) in the
/// window's coordinates: from the menu-bar screen's top-left corner, y down.
/// AppKit counts from its bottom-left corner, y up.
#[cfg(target_os = "macos")]
fn screen_areas() -> Vec<Rect> {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else { return Vec::new() };
    let screens = NSScreen::screens(mtm);
    let Some(main_height) = screens.iter().next().map(|s| s.frame().size.height) else { return Vec::new() };
    screens.iter().map(|s| {
        let f = s.visibleFrame();
        let top = main_height - f.origin.y - f.size.height;
        Rect::from_min_size(egui::pos2(f.origin.x as f32, top as f32), vec2(f.size.width as f32, f.size.height as f32))
    }).collect()
}

#[cfg(not(target_os = "macos"))]
fn screen_areas() -> Vec<Rect> { Vec::new() }

/// The chat panel's height: the one wanted, but leaving a row of rooms
/// above it in a widget `height` tall (`others`: its insets and the HUD),
/// and never under `MIN_PANEL`.
fn fit_panel(wanted: f32, height: f32, others: f32) -> f32 {
    wanted.min(height - others - ROW_H).max(MIN_PANEL)
}

/// The column count nearest to a width.
fn snap_columns(width: f32) -> usize {
    (((width - GAP) / (scene::ROW_W + GAP)).round() as usize).clamp(1, 8)
}

/// The row count nearest to a height, past the chrome.
fn snap_rows(height: f32, chrome: f32) -> usize {
    (((height - chrome + GAP) / (ROW_H + GAP)).round() as usize).max(1)
}

/// Launch at login through a LaunchAgent (macOS) or an autostart entry (Linux).
fn login_item() -> Option<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.to_string_lossy().to_string();
    // Inside an .app bundle, register the bundle itself.
    let app = path.find("/Contents/MacOS/").map(|at| path[..at].to_string()).unwrap_or(path);
    let login = auto_launch::AutoLaunchBuilder::new().set_app_name("Agents Pixel Dungeon").set_app_path(&app).set_use_launch_agent(true).build().ok()?;
    // Launching at login under the app's old name moves to the new one.
    if let Ok(old) = auto_launch::AutoLaunchBuilder::new().set_app_name("Herdr Pixel Dungeon").set_app_path(&app).set_use_launch_agent(true).build() {
        if old.is_enabled().unwrap_or(false) && old.disable().is_ok() { let _ = login.enable(); }
    }
    Some(login)
}

/// The system's Reduce Motion setting (macOS); Linux has no common switch,
/// so HPD_REDUCE_MOTION=1 serves there.
fn reduce_motion() -> bool {
    if std::env::var("HPD_REDUCE_MOTION").map(|v| v == "1").unwrap_or(false) { return true; }
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("defaults").args(["read", "com.apple.universalaccess", "reduceMotion"]).output() {
            return String::from_utf8_lossy(&output.stdout).trim() == "1";
        }
    }
    false
}
