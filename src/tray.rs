//! The status item: the app's crystal orb in the menu bar (macOS) or the
//! system tray (Linux). A left click toggles the widget; the menu has the
//! sounds, notifications, sessions, window options and quit.

use crate::herdr::HerdrSession;
use crate::l10n::{tr, trf};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
#[cfg(target_os = "linux")]
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Toggle,
    Reanchor,
    OnTop,
    Fullscreen,
    Login,
    Sounds,
    SoundHelp,
    SoundDone,
    Preview,
    Notify,
    NotifyHelp,
    NotifyDone,
    Session(String),
    Demo,
    /// The guild's quest board.
    QuestBoard,
    /// The guild window: found it, members and roles, the game design.
    Guild,
    OpenHerdr,
    OpenAtStart,
    TextBigger,
    TextSmaller,
    TextNormal,
    ConsoleBigger,
    ConsoleSmaller,
    ConsoleNormal,
    /// How tall full screen draws its rooms: "large", "medium", "small".
    RoomSize(&'static str),
    About,
    Quit,
}

/// What the menu shows: the preferences and the sessions.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuState {
    pub sounds: bool,
    pub sound_help: bool,
    pub sound_done: bool,
    pub notify: bool,
    pub notify_help: bool,
    pub notify_done: bool,
    pub on_top: bool,
    pub fullscreen: bool,
    pub login: bool,
    pub sessions: Vec<HerdrSession>,
    pub session: String,
    pub demo: bool,
    pub attention: usize,
    pub open_at_start: bool,
    pub text_scale: f32,
    pub console_scale: f32,
    pub room_size: String,
}

/// Menu bar icon: the app's crystal orb with its >_ prompt cut out, on its
/// stand, `size` pixels square (macOS shows it 18 pt tall, so 36 is sharp
/// on a Retina bar), smoothed by sampling each pixel 4×4 times. One colour
/// on clear, so macOS tints it as a template for light and dark bars.
pub fn orb_rgba(size: usize, color: [u8; 3]) -> (Vec<u8>, u32, u32) {
    // The shape in a 36-unit square.
    let segment = |px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)| {
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
    };
    let inside = |x: f32, y: f32| {
        let orb = (x - 18.0).powi(2) + (y - 14.5).powi(2) <= 13.0f32.powi(2);
        let chevron = segment(x, y, (10.0, 8.5), (16.0, 14.5)) <= 1.7 || segment(x, y, (16.0, 14.5), (10.0, 20.5)) <= 1.7;
        let underscore = (18.5..=27.0).contains(&x) && (18.8..=22.2).contains(&y);
        let neck = (12.0..=24.0).contains(&x) && (27.0..=32.0).contains(&y);
        let base = (6.0..=30.0).contains(&x) && (31.5..=34.5).contains(&y);
        (orb && !chevron && !underscore) || neck || base
    };
    let unit = 36.0 / size as f32;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let hits = (0..16).filter(|i| inside((x as f32 + ((i % 4) as f32 + 0.5) / 4.0) * unit, (y as f32 + ((i / 4) as f32 + 0.5) / 4.0) * unit)).count();
            if hits == 0 { continue; }
            let i = (y * size + x) * 4;
            rgba[i..i + 3].copy_from_slice(&color);
            rgba[i + 3] = (hits * 255 / 16) as u8;
        }
    }
    (rgba, size as u32, size as u32)
}

fn build_menu(state: &MenuState) -> (Menu, HashMap<MenuId, TrayAction>) {
    let mut actions = HashMap::new();
    let menu = Menu::new();
    let mut add = |item: &dyn tray_icon::menu::IsMenuItem, action: TrayAction| {
        actions.insert(item.id().clone(), action);
    };
    let item = |text: &str| MenuItem::new(text, true, None);
    let check = |text: &str, enabled: bool, checked: bool| CheckMenuItem::new(text, enabled, checked, None);

    let toggle = item(&tr("Mostrar / ocultar"));
    let reanchor = item(&tr("Reposicionar en la esquina"));
    let on_top = check(&tr("Siempre visible"), true, state.on_top);
    let fullscreen = check(&tr("Pantalla completa"), true, state.fullscreen);
    let login = check(&tr("Abrir al iniciar sesión"), true, state.login);
    let _ = menu.append_items(&[&toggle, &reanchor, &on_top, &fullscreen, &login, &PredefinedMenuItem::separator()]);
    add(&toggle, TrayAction::Toggle);
    add(&reanchor, TrayAction::Reanchor);
    add(&on_top, TrayAction::OnTop);
    add(&fullscreen, TrayAction::Fullscreen);
    add(&login, TrayAction::Login);

    let sounds = Submenu::new(tr("Sonidos"), true);
    let s_on = check(&tr("Sonidos activados"), true, state.sounds);
    let s_help = check(&format!("    {}", tr("Al necesitar ayuda")), state.sounds, state.sound_help);
    let s_done = check(&format!("    {}", tr("Al terminar")), state.sounds, state.sound_done);
    let preview = item(&tr("Probar sonidos"));
    let _ = sounds.append_items(&[&s_on, &s_help, &s_done, &PredefinedMenuItem::separator(), &preview]);
    add(&s_on, TrayAction::Sounds);
    add(&s_help, TrayAction::SoundHelp);
    add(&s_done, TrayAction::SoundDone);
    add(&preview, TrayAction::Preview);

    let notify = Submenu::new(tr("Notificaciones"), true);
    let n_on = check(&tr("Notificaciones activadas"), true, state.notify);
    let n_help = check(&format!("    {}", tr("Al necesitar ayuda")), state.notify, state.notify_help);
    let n_done = check(&format!("    {}", tr("Al terminar")), state.notify, state.notify_done);
    let _ = notify.append_items(&[&n_on, &n_help, &n_done]);
    add(&n_on, TrayAction::Notify);
    add(&n_help, TrayAction::NotifyHelp);
    add(&n_done, TrayAction::NotifyDone);

    // Sessions: the running ones (and the watched one, even if stopped).
    let sessions = Submenu::new(tr("Sesión de Herdr"), true);
    let mut list = state.sessions.clone();
    if !list.iter().any(|s| s.name == state.session) { list.insert(0, HerdrSession { name: state.session.clone(), running: false }); }
    for session in &list {
        let title = if session.running { session.name.clone() } else { trf("{} (detenida)", &[&session.name]) };
        let entry = check(&title, true, session.name == state.session && !state.demo);
        let _ = sessions.append(&entry);
        add(&entry, TrayAction::Session(session.name.clone()));
    }
    let demo = check(&tr("Demo (agentes ficticios)"), true, state.demo);
    let _ = sessions.append_items(&[&PredefinedMenuItem::separator(), &demo]);
    add(&demo, TrayAction::Demo);

    let board = item(&tr("Tablón de misiones"));
    add(&board, TrayAction::QuestBoard);
    let studio = item(&tr("Gremio"));
    add(&studio, TrayAction::Guild);
    let open_herdr = item(&tr("Abrir Herdr"));
    let open_at_start = check(&tr("Abrir Herdr al iniciar"), true, state.open_at_start);
    add(&open_herdr, TrayAction::OpenHerdr);
    add(&open_at_start, TrayAction::OpenAtStart);

    let text = Submenu::new(tr("Tamaño del texto"), true);
    let bigger = item(&tr("Más grande"));
    let smaller = item(&tr("Más pequeño"));
    let normal = check(&tr("Normal"), true, (state.text_scale - 1.0).abs() < 0.01);
    let _ = text.append_items(&[&bigger, &smaller, &normal]);
    add(&bigger, TrayAction::TextBigger);
    add(&smaller, TrayAction::TextSmaller);
    add(&normal, TrayAction::TextNormal);

    let console = Submenu::new(tr("Tamaño de la consola"), true);
    let console_bigger = item(&tr("Más grande"));
    let console_smaller = item(&tr("Más pequeño"));
    let console_normal = check(&tr("Normal"), true, (state.console_scale - 1.0).abs() < 0.01);
    let _ = console.append_items(&[&console_bigger, &console_smaller, &console_normal]);
    add(&console_bigger, TrayAction::ConsoleBigger);
    add(&console_smaller, TrayAction::ConsoleSmaller);
    add(&console_normal, TrayAction::ConsoleNormal);

    // Full screen's rooms, at one of three sizes.
    let rooms = Submenu::new(tr("Tamaño de las salas"), true);
    for size in crate::scene::RoomSize::ALL {
        let entry = check(&tr(size.label()), true, state.room_size == size.key());
        let _ = rooms.append(&entry);
        add(&entry, TrayAction::RoomSize(size.key()));
    }

    let about = item(&tr("Acerca de Agents Pixel Dungeon"));
    let quit = item(&tr("Salir"));
    let _ = menu.append_items(&[&sounds, &notify, &text, &console, &rooms, &PredefinedMenuItem::separator(), &board, &studio, &sessions, &open_herdr, &open_at_start, &PredefinedMenuItem::separator(), &about, &quit]);
    add(&about, TrayAction::About);
    add(&quit, TrayAction::Quit);
    (menu, actions)
}

pub struct Tray {
    actions: Arc<Mutex<HashMap<MenuId, TrayAction>>>,
    events: Receiver<TrayAction>,
    #[cfg(target_os = "linux")]
    updates: Sender<MenuState>,
    #[cfg(not(target_os = "linux"))]
    icon: Option<tray_icon::TrayIcon>,
    shown: Option<MenuState>,
}

impl Tray {
    /// Create the status item. `wake` is called whenever an event arrives,
    /// so the UI can process it right away.
    pub fn new(state: &MenuState, wake: Arc<dyn Fn() + Send + Sync>) -> Tray {
        let actions: Arc<Mutex<HashMap<MenuId, TrayAction>>> = Arc::new(Mutex::new(HashMap::new()));
        let (tx, events) = channel::<TrayAction>();
        // Menu picks and icon clicks arrive on tray-icon's own channels; route them to ours.
        {
            let tx = tx.clone();
            let actions = actions.clone();
            let wake = wake.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if let Some(action) = actions.lock().ok().and_then(|a| a.get(&event.id).cloned()) {
                    let _ = tx.send(action);
                    wake();
                }
            }));
        }
        {
            let tx = tx.clone();
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click { button: tray_icon::MouseButton::Left, button_state: tray_icon::MouseButtonState::Up, .. } = event {
                    let _ = tx.send(TrayAction::Toggle);
                    wake();
                }
            }));
        }
        #[cfg(target_os = "linux")]
        let updates = linux_host(actions.clone());
        let mut tray = Tray {
            actions,
            events,
            #[cfg(target_os = "linux")]
            updates,
            #[cfg(not(target_os = "linux"))]
            icon: None,
            shown: None,
        };
        tray.update(state);
        tray
    }

    /// Rebuild the menu when what it shows changed.
    pub fn update(&mut self, state: &MenuState) {
        if self.shown.as_ref() == Some(state) { return; }
        self.shown = Some(state.clone());
        #[cfg(target_os = "linux")]
        {
            let _ = self.updates.send(state.clone());
        }
        #[cfg(not(target_os = "linux"))]
        {
            let (menu, actions) = build_menu(state);
            *self.actions.lock().unwrap() = actions;
            let tooltip = trf("Agents Pixel Dungeon · {} necesitan atención", &[&state.attention]);
            match &self.icon {
                Some(icon) => {
                    icon.set_menu(Some(Box::new(menu)));
                    let _ = icon.set_tooltip(Some(tooltip));
                }
                None => {
                    let (rgba, w, h) = if cfg!(windows) { orb_rgba(32, [230, 230, 230]) } else { orb_rgba(36, [0, 0, 0]) };
                    let built = TrayIconBuilder::new()
                        .with_menu(Box::new(menu))
                        .with_menu_on_left_click(false)
                        .with_icon_as_template(cfg!(target_os = "macos"))
                        .with_tooltip(tooltip)
                        .with_icon(Icon::from_rgba(rgba, w, h).expect("orb icon"))
                        .build();
                    self.icon = built.ok();
                }
            }
        }
    }

    /// The actions picked since the last call.
    pub fn poll(&self) -> Vec<TrayAction> {
        let mut actions = vec![];
        while let Ok(action) = self.events.try_recv() { actions.push(action); }
        actions
    }
}

/// On Linux the tray lives on a GTK thread of its own; the menu is rebuilt
/// there from the states sent over.
#[cfg(target_os = "linux")]
fn linux_host(shared: Arc<Mutex<HashMap<MenuId, TrayAction>>>) -> Sender<MenuState> {
    let (tx, rx) = channel::<MenuState>();
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || {
            if gtk::init().is_err() { return; }
            let mut icon: Option<tray_icon::TrayIcon> = None;
            loop {
                while gtk::events_pending() { gtk::main_iteration_do(false); }
                match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(state) => {
                        let (menu, actions) = build_menu(&state);
                        *shared.lock().unwrap() = actions;
                        let tooltip = trf("Agents Pixel Dungeon · {} necesitan atención", &[&state.attention]);
                        match &icon {
                            Some(existing) => { existing.set_menu(Some(Box::new(menu))); let _ = existing.set_tooltip(Some(tooltip)); }
                            None => {
                                let (rgba, w, h) = orb_rgba(32, [230, 230, 230]);
                                icon = TrayIconBuilder::new()
                                    .with_menu(Box::new(menu))
                                    .with_tooltip(tooltip)
                                    .with_icon(Icon::from_rgba(rgba, w, h).expect("orb icon"))
                                    .build()
                                    .ok();
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
            }
        })
        .ok();
    tx
}
