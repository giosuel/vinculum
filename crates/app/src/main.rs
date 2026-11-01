use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

use clap::Parser;
use futures_util::StreamExt;
use slint::{ComponentHandle, ModelRc, VecModel};

mod audio;
mod bluetooth;
mod config;
mod displays;
mod editing;
mod editor;
mod files;
mod hotspot;
mod instance;
mod nav;
mod notify;
mod openvpn;
mod portal;
mod profiles;
mod qr;
mod quick;
mod secrets;
mod sound;
mod theme;
mod tray;
mod view;
mod virt;
mod wireguard;

slint::include_modules!();

/// The app ID, also the Wayland app_id that Hyprland window rules match on.
const APP_ID: &str = "ch.giosuel.Lituus";

/// How often the colour sources are checked for changes.
const THEME_POLL: Duration = Duration::from_secs(1);

#[derive(Parser)]
#[command(name = "lituus", version = env!("APP_VERSION"), about = "Manage Wi-Fi, wired and VPN connections")]
struct Cli {
    /// Start without a window and keep running when it's closed, so NetworkManager can ask for
    /// passwords kept in the keyring (for example `exec-once = lituus --background` in
    /// Hyprland). Starting `lituus` again shows the window.
    #[arg(long)]
    background: bool,
    /// Show the quick popup, or hide it if it's open (for a bar's on-click or a keybind).
    #[arg(long, conflicts_with_all = ["background", "toggle_wifi"])]
    quick: bool,
    /// Turn Wi-Fi on or off.
    #[arg(long, conflicts_with = "background")]
    toggle_wifi: bool,
}

/// The security choices of the hidden-network dialog, by index; the others use them too.
const SECURITIES: [&str; 4] = ["None (open)", "WPA/WPA2 Personal", "WPA3 Personal", "WEP"];
const PASSWORD_HINTS: [&str; 4] = ["", "8 to 63 characters", "", "5 or 13 characters, or a passphrase"];

fn security_index(key_mgmt: lituus_nm::KeyMgmt) -> i32 {
    match key_mgmt {
        lituus_nm::KeyMgmt::WpaPsk => 1,
        lituus_nm::KeyMgmt::Sae => 2,
        lituus_nm::KeyMgmt::Wep => 3,
    }
}

fn key_mgmt_at(index: i32) -> Option<lituus_nm::KeyMgmt> {
    match index {
        1 => Some(lituus_nm::KeyMgmt::WpaPsk),
        2 => Some(lituus_nm::KeyMgmt::Sae),
        3 => Some(lituus_nm::KeyMgmt::Wep),
        _ => None,
    }
}

/// The password dialog's three forms.
enum Dialog {
    /// Joining a secured network from the list.
    Join(view::JoinTarget),
    /// Typing a hidden network's name.
    Hidden { device: String },
    /// NetworkManager asking through the secret agent.
    Prompt(secrets::Prompt),
}

/// The UI thread's state: the latest snapshot, and what the user picked.
#[derive(Default)]
struct App {
    snapshot: Option<lituus_nm::Snapshot>,
    error: Option<String>,
    ui: view::UiState,
    row_ids: Vec<String>,
    /// The palette from the colour sources, while it's followed; `None` means the built-in theme.
    palette: Option<theme::Palette>,
    /// The first usable colour source's palette, followed or not.
    wallpaper: Option<theme::Palette>,
    /// config.toml's `[theme]`, but for the sources.
    builtin: config::Builtin,
    mode: config::Mode,
    follow_wallpaper: bool,
    gradient: bool,
    /// Commands for the NetworkManager thread.
    commands: Option<tokio::sync::mpsc::UnboundedSender<view::Command>>,
    /// The message above the list, and its tone ("danger", "warn", "info").
    message: Option<(String, &'static str)>,
    /// The command waiting for the confirmation dialog, and the dialog's title, text and button.
    confirm: Option<view::Command>,
    confirm_text: Option<(String, String, String)>,
    /// The password dialog, and NetworkManager's requests waiting behind it.
    dialog: Option<Dialog>,
    prompts: VecDeque<secrets::Prompt>,
    /// Set when a dialog opens, to clear its fields once.
    reset_dialog: bool,
    /// What the keyring is called ("keyring", "KWallet"); `None` while there's none.
    keyring: Option<&'static str>,
    /// The row and name of the last connection attempt, for its failure.
    attempt: Option<(String, String)>,
    /// What the message's "Try again" runs.
    retry: Option<view::Command>,
    /// The message's bold first line.
    message_title: String,
    /// The connection editor, when it's open.
    editor: Option<editing::EditorState>,
    add_open: bool,
    /// The hotspot page's fields go to the window on the next render (after it's opened, or its
    /// settings were read), and the text its QR code shows.
    hotspot_fields: Option<(String, String, usize)>,
    hotspot_qr: String,
    /// The share dialog: its title, password and the QR code's text.
    share: Option<(String, String, String)>,
    hostname_open: bool,
    hostname_error: String,
    /// config.toml's `[tray]` and `[notifications]`.
    settings: Settings,
    /// The tray icon, while a tray host shows it, and what it and the popup show.
    tray: Option<tray::Handle>,
    quick: quick::Quick,
    /// The popup was just made, and needs everything once.
    quick_new: bool,
    /// The Bluetooth devices the popup shows: (id, name, meta, connected).
    quick_bt: Vec<(String, String, String, bool)>,
    notifier: Option<notify::Notifier>,
    /// The Bluetooth page.
    bt: bluetooth::State,
    /// Files over Bluetooth.
    files: files::State,
    /// The Displays page.
    displays: displays::State,
    /// The Sound page.
    sound: sound::State,
    /// The sidebar's search.
    nav_query: String,
    /// The Network tab last open, for coming back to it from the sidebar.
    net_page: view::Page,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Settings {
    tray: bool,
    keep_running: bool,
    notifications: bool,
}

impl Settings {
    fn from(config: &config::Config) -> Self {
        Self {
            tray: config.tray.enabled,
            keep_running: config.tray.keep_running,
            notifications: config.notifications.enabled,
        }
    }
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
    /// The quick popup, made the first time it's opened.
    static QUICK: RefCell<Option<QuickWindow>> = const { RefCell::new(None) };
}

fn main() -> Result<(), slint::PlatformError> {
    let cli = Cli::parse();
    let window = MainWindow::new()?;
    // Needs the platform, which creating the first window sets up; takes effect when it's shown.
    slint::set_xdg_app_id(APP_ID)?;
    let weak = window.as_weak();
    let request = match (cli.background, cli.quick, cli.toggle_wifi) {
        (true, ..) => None,
        (_, true, _) => Some(instance::Request::Quick),
        (_, _, true) => Some(instance::Request::ToggleWifi),
        _ => Some(instance::Request::Show),
    };
    let claim = instance::claim(APP_ID, request, move |request| {
        let _ = weak.upgrade_in_event_loop(move |window| handle_request(&window, request));
    });
    if matches!(claim, instance::Claim::Passed) {
        return Ok(());
    }
    if cli.toggle_wifi {
        toggle_wifi_once();
        return Ok(());
    }
    let settings = Settings::from(&config::load(&config::path()));
    APP.with(|app| {
        let mut app = app.borrow_mut();
        app.settings = settings;
        app.notifier = Some(notify::Notifier::spawn());
    });
    apply_settings(&window, settings);
    // Started from the bar for the popup: the window stays hidden, as in the background.
    let background = cli.background || cli.quick;
    window.window().on_close_requested(move || {
        // Closing the window answers its questions with "cancel": nobody sees them any more.
        APP.with(|app| {
            let mut app = app.borrow_mut();
            if let Some(Dialog::Prompt(prompt)) = app.dialog.take() {
                let _ = prompt.reply.send(None);
            }
            app.dialog = None;
            for prompt in app.prompts.drain(..) {
                let _ = prompt.reply.send(None);
            }
            // Dropping the pairing requests rejects them; nobody looks for devices any more.
            app.bt.prompt = None;
            app.bt.prompts.clear();
            bluetooth::set_open(&mut app.bt, false);
        });
        // In the background or the tray the agent keeps running; otherwise closing the window quits.
        let in_tray = APP.with(|app| {
            let app = app.borrow();
            app.tray.is_some() && app.settings.keep_running
        });
        if !background && !in_tray {
            let _ = slint::quit_event_loop();
        }
        slint::CloseRequestResponse::HideWindow
    });
    window.set_version(env!("APP_VERSION").into());
    connect_callbacks(&window);
    apply_theme(&window);
    render(&window);
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    APP.with(|app| app.borrow_mut().commands = Some(commands));
    spawn_network_manager(window.as_weak(), receiver);
    let (bt_commands, bt_receiver) = tokio::sync::mpsc::unbounded_channel();
    APP.with(|app| app.borrow_mut().bt.commands = Some(bt_commands));
    bluetooth::spawn(window.as_weak(), bt_receiver);
    sound::spawn(window.as_weak());
    files::spawn(window.as_weak(), config::load(&config::path()).bluetooth.accept_files_from);
    displays::refresh(&window.as_weak());
    spawn_hyprland_watcher(window.as_weak());
    // The Displays page's countdown after Apply.
    let weak = window.as_weak();
    let countdown = slint::Timer::default();
    countdown.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let keeping = APP.with(|app| app.borrow().displays.keep_pending());
        if keeping {
            let w = weak.clone();
            update(&weak, |app| displays::tick(&w, &mut app.displays));
        }
    });
    std::mem::forget(countdown);
    // The Bluetooth page's "Visible · 2:41" counts down.
    let countdown = slint::Timer::default();
    let weak = window.as_weak();
    countdown.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
        if APP.with(|app| bluetooth::counting_down(&app.borrow()))
            && let Some(window) = weak.upgrade()
        {
            render(&window);
        }
    });
    spawn_theme_watcher(window.as_weak());
    if cli.quick {
        toggle_quick(&window);
    } else if !background {
        window.show()?;
    }
    slint::run_event_loop_until_quit()
}

/// What another start asked for (see instance.rs).
fn handle_request(window: &MainWindow, request: instance::Request) {
    match request {
        instance::Request::Show => {
            let _ = window.show();
        }
        instance::Request::Quick => toggle_quick(window),
        instance::Request::ToggleWifi => update(&window.as_weak(), |app| {
            if let Some(command) = app.snapshot.as_ref().map(view::wifi_switch) {
                send(app, command);
            }
        }),
    }
}

/// `lituus --toggle-wifi` with no Lituus running: switch it and leave.
fn toggle_wifi_once() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    let result = runtime.block_on(async {
        let client = lituus_nm::Client::system().await?;
        let on = client.snapshot().await?.wireless_enabled;
        client.set_wireless_enabled(!on).await
    });
    if let Err(error) = result {
        eprintln!("lituus: couldn't switch Wi-Fi: {error}");
    }
}

/// Starts or stops the tray as config.toml says.
fn apply_settings(window: &MainWindow, settings: Settings) {
    let weak = window.as_weak();
    APP.with(|app| {
        let mut app = app.borrow_mut();
        app.settings = settings;
        if settings.tray && app.tray.is_none() {
            let quick = app.quick.clone();
            app.tray = tray::spawn(quick, move |event| {
                let _ = weak.upgrade_in_event_loop(move |window| on_tray(&window, event));
            });
        } else if !settings.tray {
            app.tray = None;
        }
    });
}

/// The tray menu's and the quick popup's actions.
fn on_tray(window: &MainWindow, event: tray::Event) {
    let weak = window.as_weak();
    match event {
        tray::Event::Activate => toggle_quick(window),
        tray::Event::Connect(id) | tray::Event::Toggle(id) => {
            update(&weak, |app| {
                let Some(snap) = &app.snapshot else { return };
                let q = quick::build(Some(snap));
                let page = if id.starts_with("wifi:") { view::Page::Wifi } else { view::Page::Vpn };
                let action = if id.starts_with("wifi:") {
                    if q.networks.iter().any(|n| n.id == id && n.active) {
                        return;
                    }
                    "connect"
                } else if q.vpns.iter().any(|v| v.id == id && v.on) {
                    "disconnect"
                } else {
                    "connect"
                };
                app.ui = view::UiState { page, selected: Some(id), ..view::UiState::default() };
                act(app, action);
            });
            // A password to type, or something to confirm: that's the window's job.
            if APP.with(|app| {
                let app = app.borrow();
                app.dialog.is_some() || app.confirm.is_some() || app.message.is_some()
            }) {
                hide_quick();
                let _ = window.show();
            }
        }
        tray::Event::SetWifi(on) => update(&weak, |app| send(app, view::Command::SetWifi(on))),
        tray::Event::SetAirplane(on) => update(&weak, |app| send(app, view::Command::SetAirplane(on))),
        tray::Event::Open => {
            hide_quick();
            let _ = window.show();
        }
        tray::Event::Settings => {
            hide_quick();
            update(&weak, |app| app.ui = view::UiState { page: view::Page::Settings, ..view::UiState::default() });
            let _ = window.show();
        }
        tray::Event::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

fn hide_quick() {
    QUICK.with(|q| {
        if let Some(q) = q.borrow().as_ref() {
            let _ = q.hide();
        }
    });
}

/// Opens the quick popup, or closes it if it's open.
fn toggle_quick(window: &MainWindow) {
    let made = QUICK.with(|q| q.borrow().is_some());
    if !made {
        let Ok(popup) = QuickWindow::new() else { return };
        let main = window.as_weak();
        let on = move |event: tray::Event| {
            let main = main.clone();
            move || {
                if let Some(window) = main.upgrade() {
                    on_tray(&window, event.clone());
                }
            }
        };
        popup.on_wifi_switched({
            let main = window.as_weak();
            move || {
                if let Some(window) = main.upgrade() {
                    let on = APP.with(|app| app.borrow().quick.wifi_on);
                    on_tray(&window, tray::Event::SetWifi(!on));
                }
            }
        });
        popup.on_rescan({
            let main = window.as_weak();
            move || {
                update(&main, |app| {
                    if let Some(command) = app.snapshot.as_ref().and_then(view::rescan) {
                        send(app, command);
                    }
                })
            }
        });
        popup.on_connect({
            let main = window.as_weak();
            move |id| {
                if let Some(window) = main.upgrade() {
                    on_tray(&window, tray::Event::Connect(id.into()));
                }
            }
        });
        popup.on_toggled({
            let main = window.as_weak();
            move |id| {
                if let Some(window) = main.upgrade() {
                    if let Some(device) = id.strip_prefix("bt:") {
                        // A Bluetooth device: connect or disconnect it.
                        let device = device.to_owned();
                        update(&main, |app| {
                            app.bt.selected = Some(device);
                            bluetooth::activate(&mut app.bt);
                        });
                        return;
                    }
                    let event = if id == "airplane" {
                        let on = APP.with(|app| app.borrow().quick.airplane);
                        tray::Event::SetAirplane(!on)
                    } else {
                        tray::Event::Toggle(id.into())
                    };
                    on_tray(&window, event);
                }
            }
        });
        popup.on_open_main(on(tray::Event::Open));
        popup.on_open_settings(on(tray::Event::Settings));
        popup.on_dismissed(hide_quick);
        QUICK.with(|q| *q.borrow_mut() = Some(popup));
        APP.with(|app| app.borrow_mut().quick_new = true);
        apply_theme(window);
        render(window);
    }
    QUICK.with(|q| {
        let q = q.borrow();
        let Some(popup) = q.as_ref() else { return };
        if popup.window().is_visible() {
            let _ = popup.hide();
        } else {
            let _ = popup.show();
            // A fresh list to pick from.
            update(&window.as_weak(), |app| {
                if let Some(command) = app.snapshot.as_ref().and_then(view::rescan) {
                    send(app, command);
                }
            });
        }
    });
}

/// The tray and the popup show what changed.
fn render_quick(app: &mut App) {
    let q = quick::build(app.snapshot.as_ref());
    let bt = bluetooth::quick(&app.bt);
    if q == app.quick && bt == app.quick_bt && !std::mem::take(&mut app.quick_new) {
        return;
    }
    if let Some(tray) = &app.tray {
        tray.show(q.clone());
    }
    QUICK.with(|popup| {
        let Some(popup) = popup.borrow().as_ref().map(|p| p.as_weak()) else { return };
        let Some(popup) = popup.upgrade() else { return };
        popup.set_wifi_available(q.wifi_available);
        popup.set_wifi_on(q.wifi_on);
        popup.set_wifi_status(q.status.as_str().into());
        popup.set_networks(model(
            q.networks
                .iter()
                .take(quick::NETWORKS)
                .map(|n| QuickRow {
                    id: n.id.as_str().into(),
                    name: n.name.as_str().into(),
                    meta: n.meta.as_str().into(),
                    bars: n.bars.into(),
                    active: n.active,
                    connecting: n.connecting,
                })
                .collect(),
        ));
        let mut toggles: Vec<QuickToggle> = q
            .vpns
            .iter()
            .map(|v| QuickToggle {
                id: v.id.as_str().into(),
                name: v.name.as_str().into(),
                meta: v.meta.as_str().into(),
                on: v.on,
            })
            .collect();
        toggles.extend(bt.iter().map(|(id, name, meta, on)| QuickToggle {
            id: id.as_str().into(),
            name: name.as_str().into(),
            meta: meta.as_str().into(),
            on: *on,
        }));
        toggles.push(QuickToggle {
            id: "airplane".into(),
            name: "Airplane mode".into(),
            meta: "Turns off Wi-Fi and mobile broadband".into(),
            on: q.airplane,
        });
        popup.set_toggles(model(toggles));
    });
    app.quick = q;
    app.quick_bt = bt;
}

fn connect_callbacks(window: &MainWindow) {
    let weak = window.as_weak();
    window.on_navigate(move |id| {
        let id = if id == "network" { APP.with(|app| app.borrow().net_page.id().into()) } else { id };
        if let Some(page) = view::Page::from_id(&id) {
            update(&weak, |app| {
                if page.is_network() {
                    app.net_page = page;
                }
                app.ui = view::UiState { page, ..view::UiState::default() };
                if page == view::Page::Hotspot {
                    open_hotspot(app);
                }
            });
        }
    });
    let weak = window.as_weak();
    window.on_nav_search(move |text| update(&weak, |app| app.nav_query = text.to_string()));
    let weak = window.as_weak();
    window.on_nav_search_accepted(move || {
        let Some(window) = weak.upgrade() else { return };
        let target = APP.with(|app| {
            let app = app.borrow();
            nav::first_match(&nav_entries(&app), &app.nav_query).map(str::to_owned)
        });
        if let Some(id) = target {
            window.invoke_navigate(id.into());
        }
    });
    let weak = window.as_weak();
    window.on_hero_action(move |verb| {
        update(&weak, |app| {
            let hero = view::build(app.snapshot.as_ref(), app.error.as_deref(), &app.ui).net_hero;
            let Some(row) = hero.row else { return };
            if app.ui.page != hero.page {
                app.ui = view::UiState { page: hero.page, ..view::UiState::default() };
                app.net_page = hero.page;
            }
            app.ui.selected = Some(row);
            act(app, &verb);
        })
    });
    let weak = window.as_weak();
    window.on_select(move |id| update(&weak, |app| app.ui.selected = Some(id.to_string())));
    let weak = window.as_weak();
    window.on_search(move |text| {
        update(&weak, |app| {
            app.ui.search = text.to_string();
            app.ui.selected = None;
        });
    });
    let weak = window.as_weak();
    window.on_move_selection(move |delta| {
        update(&weak, |app| {
            if app.ui.page == view::Page::Bluetooth {
                return bluetooth::move_selection(&mut app.bt, delta);
            }
            let ids = &app.row_ids;
            if ids.is_empty() {
                return;
            }
            let current = app.ui.selected.as_ref().and_then(|s| ids.iter().position(|id| id == s)).unwrap_or(0);
            let next = (current as i64 + i64::from(delta)).clamp(0, ids.len() as i64 - 1) as usize;
            app.ui.selected = Some(ids[next].clone());
        });
    });
    let weak = window.as_weak();
    window.on_action(move |id| update(&weak, |app| act(app, &id)));
    let weak = window.as_weak();
    window.on_activate_selected(move || {
        update(&weak, |app| match app.ui.page {
            view::Page::Bluetooth => bluetooth::activate(&mut app.bt),
            _ => act(app, "connect"),
        })
    });
    let weak = window.as_weak();
    window.on_bt_act(move |verb, argument| {
        if let Some(window) = weak.upgrade() {
            bluetooth::act(&window, &verb, &argument);
        }
    });
    let weak = window.as_weak();
    window.on_toggle(move |id| {
        update(&weak, |app| {
            let command = match (&app.snapshot, &app.ui.selected) {
                (Some(snap), Some(selected)) => view::toggle_command(snap, selected, &id),
                _ => None,
            };
            if let Some(command) = command {
                send(app, command);
            }
        })
    });
    editing::connect(window);
    let weak = window.as_weak();
    window.on_option_toggled(move |id| {
        update(&weak, |app| {
            let s = &mut app.settings;
            let (table, key, on) = match id.as_str() {
                "tray" => ("tray", "enabled", &mut s.tray),
                "keep_running" => ("tray", "keep_running", &mut s.keep_running),
                _ => ("notifications", "enabled", &mut s.notifications),
            };
            *on = !*on;
            let on = *on;
            // The file watcher starts or stops the tray once it sees the change.
            if let Err(error) = config::set(&config::path(), table, key, on) {
                app.message_title.clear();
                app.message = Some((format!("Couldn't save the setting: {error}."), "danger"));
            }
        })
    });
    let weak = window.as_weak();
    window.on_files_act(move |verb| {
        let always = weak.upgrade().is_some_and(|w| w.get_files_always());
        update(&weak, |app| match verb.as_str() {
            "cancel" => app.files.send(files::Command::Cancel),
            "hide" => {
                if app.files.sending.as_ref().is_some_and(|s| s.transfers.iter().all(lituus_bt::Transfer::done)) {
                    app.files.sending = None;
                } else {
                    app.files.hidden = true;
                }
            }
            "accept" | "decline" => app.files.send(files::Command::Answer { accept: verb == "accept", always }),
            _ => {}
        })
    });
    let weak = window.as_weak();
    window.on_displays_act(move |verb, arg, value| {
        let w = weak.clone();
        update(&weak, |app| displays::act(&w, &mut app.displays, &verb, &arg, value))
    });
    let weak = window.as_weak();
    window.on_displays_drag(move |i, x, y, done| {
        update(&weak, |app| displays::drag(&mut app.displays, i.max(0) as usize, x, y, done))
    });
    let weak = window.as_weak();
    window.on_sound_act(move |verb, id, value| {
        update(&weak, |app| {
            if let Some((table, key, on)) = sound::act(&mut app.sound, &verb, &id, value)
                && let Err(error) = config::set(&config::path(), table, key, on)
            {
                app.message_title.clear();
                app.message = Some((format!("Couldn't save the setting: {error}."), "danger"));
            }
        })
    });
    let weak = window.as_weak();
    window.on_ap_act(move |what, i| {
        update(&weak, |app| {
            let i = i.max(0) as usize;
            let saved = match what.as_str() {
                "follow" => {
                    app.follow_wallpaper = !app.follow_wallpaper;
                    app.palette = app.wallpaper.filter(|_| app.follow_wallpaper);
                    config::set(&config::path(), "theme", "follow_wallpaper", app.follow_wallpaper)
                }
                "theme" => {
                    app.builtin = config::Builtin::ALL[i.min(config::Builtin::ALL.len() - 1)];
                    config::set(&config::path(), "theme", "builtin", app.builtin.key())
                }
                "mode" => {
                    app.mode = config::Mode::ALL[i.min(2)];
                    config::set(&config::path(), "theme", "mode", app.mode.key())
                }
                _ => {
                    app.gradient = !app.gradient;
                    config::set(&config::path(), "theme", "gradient", app.gradient)
                }
            };
            if let Err(error) = saved {
                app.message_title.clear();
                app.message = Some((format!("Couldn't save the setting: {error}."), "danger"));
            }
        });
        if let Some(window) = weak.upgrade() {
            apply_theme(&window);
        }
    });
    let weak = window.as_weak();
    window.on_add_clicked(move || update(&weak, |app| app.add_open = true));
    let weak = window.as_weak();
    window.on_share_closed(move || update(&weak, |app| app.share = None));
    let weak = window.as_weak();
    window.on_hotspot_edited(move || update(&weak, |_| {}));
    let weak = window.as_weak();
    window.on_hotspot_new_password(move || {
        let Some(window) = weak.upgrade() else { return };
        window.set_hotspot_password(hotspot::new_password().into());
        update(&weak, |_| {});
    });
    let weak = window.as_weak();
    window.on_hotspot_toggled(move || {
        let Some(window) = weak.upgrade() else { return };
        let (ssid, password, band) = (
            window.get_hotspot_ssid().trim().to_owned(),
            window.get_hotspot_password().to_string(),
            window.get_hotspot_band(),
        );
        update(&weak, |app| {
            let Some(h) = app.snapshot.as_ref().map(view::hotspot_view) else { return };
            if let Some(active) = h.active {
                return send(app, view::Command::StopHotspot { active });
            }
            let Some(device) = h.device else { return };
            if ssid.is_empty() || ssid.len() > 32 || !(8..=63).contains(&password.chars().count()) {
                app.message_title.clear();
                app.message = Some((
                    "The hotspot needs a name (1 to 32 bytes) and a password of 8 to 63 characters.".into(),
                    "danger",
                ));
                return;
            }
            send(
                app,
                view::Command::StartHotspot {
                    connection: h.connection,
                    device,
                    ssid,
                    password,
                    band: band.max(0) as usize,
                },
            );
        })
    });
    let weak = window.as_weak();
    window.on_empty_action_clicked(move || {
        update(&weak, |app| match app.ui.page {
            view::Page::Vpn => send(app, view::Command::ImportFile),
            _ => {
                if let Some(command) = app.snapshot.as_ref().map(view::wifi_switch) {
                    send(app, command);
                }
            }
        })
    });
    let weak = window.as_weak();
    window.on_add_cancelled(move || update(&weak, |app| app.add_open = false));
    let weak = window.as_weak();
    window.on_add_chosen(move |kind| {
        update(&weak, |app| {
            app.add_open = false;
            let kind = match kind.as_str() {
                "wifi" => editor::Kind::Wifi,
                "wireguard" => editor::Kind::WireGuard,
                "openvpn" => editor::Kind::OpenVpn,
                "vlan" => editor::Kind::Vlan,
                "bridge" => editor::Kind::Bridge,
                "bond" => editor::Kind::Bond,
                "import" => return send(app, view::Command::ImportFile),
                "hotspot" => {
                    app.ui = view::UiState { page: view::Page::Hotspot, ..view::UiState::default() };
                    return open_hotspot(app);
                }
                _ => editor::Kind::Ethernet,
            };
            send(app, view::Command::OpenEditor { connection: None, new: Some(kind) });
        })
    });
    let weak = window.as_weak();
    window.on_hostname_clicked(move || {
        let Some(window) = weak.upgrade() else { return };
        let hostname = APP.with(|app| app.borrow().snapshot.as_ref().map(|s| s.hostname.clone()).unwrap_or_default());
        window.set_hostname_input(hostname.into());
        update(&weak, |app| {
            app.hostname_open = true;
            app.hostname_error.clear();
        });
    });
    let weak = window.as_weak();
    window.on_hostname_cancelled(move || update(&weak, |app| app.hostname_open = false));
    let weak = window.as_weak();
    window.on_hostname_save(move || {
        let Some(window) = weak.upgrade() else { return };
        let name = window.get_hostname_input().trim().to_owned();
        update(&weak, |app| match hostname_problem(&name) {
            Some(problem) => app.hostname_error = problem.into(),
            None => {
                app.hostname_open = false;
                send(app, view::Command::SaveHostname(name));
            }
        });
    });
    let weak = window.as_weak();
    window.on_wifi_switched(move || {
        update(&weak, |app| {
            if let Some(command) = app.snapshot.as_ref().map(view::wifi_switch) {
                send(app, command);
            }
        })
    });
    let weak = window.as_weak();
    window.on_rescan(move || {
        update(&weak, |app| {
            if let Some(command) = app.snapshot.as_ref().and_then(view::rescan) {
                send(app, command);
            }
        })
    });
    let weak = window.as_weak();
    window.on_dismiss_message(move || update(&weak, |app| app.message = None));
    let weak = window.as_weak();
    window.on_confirm_accepted(move || {
        update(&weak, |app| {
            if let Some(command) = app.confirm.take() {
                send(app, command);
            }
        })
    });
    let weak = window.as_weak();
    window.on_confirm_cancelled(move || update(&weak, |app| app.confirm = None));
    let weak = window.as_weak();
    window.on_message_action_clicked(move || {
        update(&weak, |app| {
            if let Some(command) = app.retry.take() {
                app.ui.failed = None;
                send(app, command);
            }
        })
    });
    let weak = window.as_weak();
    window.on_bottom_clicked(move || {
        update(&weak, |app| {
            if app.ui.page == view::Page::Wifi
                && let Some(device) = app.snapshot.as_ref().and_then(view::hidden_network_device)
            {
                open_dialog(app, Dialog::Hidden { device });
            } else if app.ui.page != view::Page::Wifi {
                send(app, view::Command::ImportFile);
            }
        })
    });
    let weak = window.as_weak();
    window.on_airplane_toggled(move || {
        update(&weak, |app| {
            if let Some(command) = app.snapshot.as_ref().map(view::airplane_switch) {
                send(app, command);
            }
        })
    });
    window.on_join_check(|mode, name, security, password| {
        let password_ok = |index| key_mgmt_at(index).is_some_and(|k| k.accepts(&password));
        if mode == "hidden" {
            !name.is_empty() && name.len() <= 32 && (security == 0 || password_ok(security))
        } else {
            password_ok(security)
        }
    });
    let weak = window.as_weak();
    window.on_join_accepted(move || {
        let Some(window) = weak.upgrade() else { return };
        let fields = (
            window.get_join_name().to_string(),
            window.get_join_security(),
            window.get_join_password().to_string(),
            window.get_join_storage(),
            window.get_join_autoconnect(),
        );
        update(&weak, |app| accept_dialog(app, fields));
    });
    let weak = window.as_weak();
    window.on_join_cancelled(move || {
        update(&weak, |app| {
            if let Some(Dialog::Prompt(prompt)) = app.dialog.take() {
                let _ = prompt.reply.send(None);
            }
            next_prompt(app);
        })
    });
}

/// What's wrong with a hostname: letters, digits and hyphens, in dot-separated labels.
fn hostname_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.len() > 64 {
        return Some("From 1 to 64 characters.");
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if name.split('.').all(label_ok) { None } else { Some("Letters, digits and hyphens only, such as thinkpad-x1.") }
}

/// The hotspot page opens with its saved settings, or with the hostname and a new password.
fn open_hotspot(app: &mut App) {
    let Some(snap) = &app.snapshot else { return };
    match snap.connections.iter().find(|c| c.hotspot) {
        Some(c) => send(app, view::Command::LoadHotspot { connection: c.path.clone() }),
        None => app.hotspot_fields = Some((snap.hostname.clone(), hotspot::new_password(), 0)),
    }
}

fn render_hotspot(window: &MainWindow, app: &mut App, view: Option<&view::HotspotView>) {
    let Some(h) = view else { return };
    window.set_hotspot_available(h.device.is_some());
    window.set_hotspot_on(h.active.is_some());
    window.set_hotspot_status(h.status.as_str().into());
    window.set_hotspot_warning(h.warning.as_str().into());
    if let Some((ssid, password, band)) = app.hotspot_fields.take() {
        window.set_hotspot_ssid(ssid.into());
        window.set_hotspot_password(password.into());
        window.set_hotspot_band(band as i32);
    }
    let text = qr::wifi_text(&window.get_hotspot_ssid(), Some(&window.get_hotspot_password()), false, false);
    if text != app.hotspot_qr
        && let Some(image) = qr::image(&text, 8)
    {
        window.set_hotspot_qr(image);
        app.hotspot_qr = text;
    }
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let clients = if h.active.is_some() { hotspot::clients(&h.interface) } else { Vec::new() };
    window.set_hotspot_clients(model(
        clients
            .into_iter()
            .map(|c| HostRow {
                name: c.name.into(),
                address: c.address.into(),
                // dnsmasq hands out leases for an hour; what's left of it says how long ago.
                since: match (3600 + now).saturating_sub(c.expires) / 60 {
                    0 => "just now".into(),
                    m => format!("{m} min").into(),
                },
            })
            .collect(),
    ));
}

fn open_dialog(app: &mut App, dialog: Dialog) {
    if let Some(Dialog::Prompt(prompt)) = app.dialog.take() {
        // A request from NetworkManager isn't dropped for a dialog the user opened.
        app.prompts.push_front(prompt);
    }
    app.dialog = Some(dialog);
    app.reset_dialog = true;
}

/// Shows the next waiting request from NetworkManager, if any.
fn next_prompt(app: &mut App) {
    if app.dialog.is_none()
        && let Some(prompt) = app.prompts.pop_front()
    {
        app.dialog = Some(Dialog::Prompt(prompt));
        app.reset_dialog = true;
    }
}

/// Connect in the password dialog, with its (name, security, password, storage, autoconnect).
fn accept_dialog(app: &mut App, (name, security, password, storage, autoconnect): (String, i32, String, i32, bool)) {
    let storage = match storage {
        1 => lituus_nm::Storage::System,
        2 => lituus_nm::Storage::Ask,
        _ => lituus_nm::Storage::Agent,
    };
    // "All users" connections are everyone's; the others belong to this user.
    let owner = if storage == lituus_nm::Storage::System { None } else { secrets::username() };
    match app.dialog.take() {
        Some(Dialog::Join(target)) => {
            let join = lituus_nm::WifiJoin {
                ssid: target.ssid.clone(),
                uuid: secrets::new_uuid(),
                security: Some((target.key_mgmt, password)),
                storage,
                owner,
                autoconnect,
                hidden: false,
            };
            app.attempt = app.ui.selected.clone().map(|row| (row, target.name.clone()));
            send(app, view::Command::Join { device: target.device, access_point: Some(target.access_point), join });
        }
        Some(Dialog::Hidden { device }) => {
            let join = lituus_nm::WifiJoin {
                ssid: name.clone().into_bytes(),
                uuid: secrets::new_uuid(),
                security: key_mgmt_at(security).map(|k| (k, password)),
                storage,
                owner,
                autoconnect,
                hidden: true,
            };
            app.attempt = Some((String::new(), name));
            send(app, view::Command::Join { device, access_point: None, join });
        }
        Some(Dialog::Prompt(prompt)) => {
            let _ = prompt.reply.send(Some(password));
        }
        None => {}
    }
    next_prompt(app);
}

/// A details button (or Enter, for "connect") on the selected row.
fn act(app: &mut App, action: &str) {
    let (Some(snapshot), Some(selected)) = (app.snapshot.as_ref(), app.ui.selected.as_deref()) else { return };
    let name = view::row_name(snapshot, selected);
    match view::intent(snapshot, selected, action) {
        Some(view::Intent::Run(command)) => {
            if matches!(command, view::Command::Activate { .. } | view::Command::ConnectOpen { .. }) {
                app.attempt = name.map(|n| (selected.to_owned(), n));
                app.ui.failed = None;
            }
            send(app, command)
        }
        Some(view::Intent::Join(target)) => open_dialog(app, Dialog::Join(target)),
        Some(view::Intent::Confirm { title, text, button, command }) => {
            app.confirm = Some(command);
            app.confirm_text = Some((title, text, button));
        }
        None => {}
    }
}

fn send(app: &mut App, command: view::Command) {
    app.message = None;
    app.message_title.clear();
    app.retry = None;
    if let Some(commands) = &app.commands {
        let _ = commands.send(command);
    }
}

/// Changes the state, then shows it.
fn update(weak: &slint::Weak<MainWindow>, change: impl FnOnce(&mut App)) {
    APP.with(|app| change(&mut app.borrow_mut()));
    if let Some(window) = weak.upgrade() {
        render(&window);
    }
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// Reads the monitors again whenever Hyprland says one came or went; when Hyprland isn't there,
/// looks again now and then.
fn spawn_hyprland_watcher(weak: slint::Weak<MainWindow>) {
    std::thread::spawn(move || {
        loop {
            if let Ok(h) = lituus_hypr::Hyprland::from_env() {
                let _ = h.events(|name, _| {
                    match name {
                        "monitoraddedv2" | "monitorremovedv2" => displays::hotplugged(&weak),
                        "configreloaded" => displays::refresh(&weak),
                        _ => {}
                    }
                    true
                });
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

/// The Appearance page.
fn render_appearance(window: &MainWindow, app: &App) {
    let in_use = app.palette.is_some();
    let dark = app.mode.dark(window.get_prefers_dark());
    window.set_ap_follow(app.follow_wallpaper);
    window.set_ap_follow_available(app.wallpaper.is_some());
    window.set_ap_wallpaper_in_use(in_use);
    let mode = ["auto", "dark", "light"][config::Mode::ALL.iter().position(|m| *m == app.mode).unwrap_or(0)];
    let (sub, name, text) = if in_use {
        (
            "Following your wallpaper".to_owned(),
            "Wallpaper colours".to_owned(),
            "Made by matugen from your wallpaper; the window recolours when it changes.".to_owned(),
        )
    } else {
        let why = if app.wallpaper.is_none() { "No wallpaper colours found · " } else { "" };
        (
            format!("{why}{} · {}", app.builtin.label(), if dark { "dark" } else { "light" }),
            app.builtin.label().to_owned(),
            format!("Built-in theme · {mode} mode"),
        )
    };
    window.set_ap_sub(sub.into());
    window.set_ap_name(name.into());
    window.set_ap_name_text(text.into());
    let chips = app.wallpaper.map(|p| {
        [("Primary", p.accent), ("Container", p.accent_soft_border), ("Secondary", p.accent_soft_bg), ("Surface", p.bg)]
            .into_iter()
            .map(|(label, c)| ColourChip {
                label: label.into(),
                value: color(c),
                hex: format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2).into(),
            })
            .collect()
    });
    window.set_ap_chips(model(chips.unwrap_or_default()));
    window.set_ap_swatches(model(
        config::Builtin::ALL
            .iter()
            .map(|&b| {
                let p = theme::Palette::builtin(b, dark);
                ThemeSwatch {
                    name: b.label().into(),
                    bg: color(p.bg),
                    side: color(p.panel),
                    accent: color(p.accent),
                    selected: b == app.builtin,
                }
            })
            .collect(),
    ));
    let statuses: Vec<theme::SourceStatus> = app.wallpaper.map(theme::SourceStatus::Used).into_iter().collect();
    window.set_ap_note(theme::builtin_note(app.follow_wallpaper, &statuses).into());
    window.set_ap_mode(config::Mode::ALL.iter().position(|m| *m == app.mode).unwrap_or(0) as i32);
    window.set_ap_gradient(app.gradient);
}

/// The phones NetworkManager can use the internet of, by Bluetooth address, for the Bluetooth
/// page.
fn tether_offers(snapshot: Option<&lituus_nm::Snapshot>) -> std::collections::HashMap<String, bluetooth::Tether> {
    let Some(snap) = snapshot else { return Default::default() };
    snap.devices
        .iter()
        .filter(|d| d.kind == lituus_nm::DeviceKind::Bluetooth)
        .map(|d| {
            let address = d.hw_address.to_ascii_uppercase();
            let connection = snap
                .connections
                .iter()
                .find(|c| matches!(&c.kind, lituus_nm::ConnectionKind::Bluetooth { address: a } if a.eq_ignore_ascii_case(&address)));
            let active = d.active_connection.clone().filter(|a| {
                snap.active_connection(a).is_some_and(|a| a.state != lituus_nm::ActiveState::Deactivated)
            });
            let ip = d.ip4.as_ref().and_then(|ip| ip.addresses.first()).map(|(a, _)| a.clone());
            let connecting = active.as_deref().and_then(|a| snap.active_connection(a)).is_some_and(|a| a.state != lituus_nm::ActiveState::Activated);
            let metered = connection.and_then(|c| c.metered).unwrap_or(true);
            let mut status = vec![if connecting { "Connecting…".to_owned() } else { "Connected".to_owned() }];
            status.extend(ip);
            if metered {
                status.push("metered".to_owned());
            }
            let tether = bluetooth::Tether {
                device: d.path.clone(),
                connection: connection.map(|c| c.path.clone()),
                active,
                status: status.join(" · "),
            };
            (address, tether)
        })
        .collect()
}

/// The sidebar's pages; the Network item's status is filled in from the view.
fn nav_entries(app: &App) -> Vec<nav::Entry> {
    vec![
        nav::Entry {
            id: "network",
            label: "Network",
            icon: "wifi",
            group: "CONNECTIONS",
            keywords: "wi-fi wifi wired ethernet vpn wireguard openvpn hotspot internet saved connections hostname",
            status: String::new(),
        },
        nav::Entry {
            id: "bluetooth",
            label: "Bluetooth",
            icon: "bluetooth",
            group: "CONNECTIONS",
            keywords: "pair pairing headphones headset keyboard mouse speaker devices",
            status: bluetooth::nav(&app.bt, false).sub.to_string(),
        },
        nav::Entry {
            id: "displays",
            label: "Displays",
            icon: "monitor",
            group: "DEVICES",
            keywords: "monitors screens resolution refresh rate scale hdr vrr rotation mirror hyprland layout",
            status: displays::status(&app.displays),
        },
        nav::Entry {
            id: "sound",
            label: "Sound",
            icon: "volume",
            group: "DEVICES",
            keywords: "audio volume speakers headphones microphone input output pipewire mute apps",
            status: sound::status(&app.sound),
        },
        nav::Entry {
            id: "appearance",
            label: "Appearance",
            icon: "palette",
            group: "PERSONAL",
            keywords: "theme colours colors dark light mode matugen wallpaper accent gradient",
            status: if app.palette.is_some() { "Wallpaper".to_owned() } else { app.builtin.label().to_owned() },
        },
    ]
}

fn render(window: &MainWindow) {
    APP.with(|app| {
        let mut app = app.borrow_mut();
        let v = view::build(app.snapshot.as_ref(), app.error.as_deref(), &app.ui);
        let current = if app.ui.page.is_network() { "network" } else { app.ui.page.id() };
        let mut entries = nav_entries(&app);
        entries[0].status = v.net_status.clone();
        window.set_nav_groups(model(nav::groups(&entries, &app.nav_query, current)));
        window.set_net_sub(v.net_sub.as_str().into());
        window.set_net_hero(HeroInfo {
            icon: v.net_hero.icon.into(),
            name: v.net_hero.name.as_str().into(),
            meta: v.net_hero.meta.as_str().into(),
            tone: v.net_hero.tone.into(),
            detail: v.net_hero.detail.as_str().into(),
        });
        window.set_net_hero_actions(model(
            v.net_hero
                .actions
                .iter()
                .map(|&(id, label, kind)| Action {
                    id: id.into(),
                    label: label.into(),
                    kind: kind.into(),
                    enabled: true,
                })
                .collect(),
        ));
        window.set_wifi_available(v.wifi_available);
        let on_bluetooth = app.ui.page == view::Page::Bluetooth && window.window().is_visible();
        bluetooth::set_open(&mut app.bt, on_bluetooth);
        // What files.rs has to tell goes in the Bluetooth page's message.
        if let Some(note) = app.files.note.take() {
            app.bt.message = Some(bluetooth::Message { title: String::new(), text: note, tone: "info", retry: None });
        }
        app.bt.tether = tether_offers(app.snapshot.as_ref());
        bluetooth::render(window, &mut app);
        window.set_page(app.ui.page.id().into());
        window.set_page_title(v.page_title.into());
        window.set_show_wifi_switch(v.show_wifi_switch);
        window.set_wifi_on(v.wifi_on);
        window.set_show_rescan(v.show_rescan);
        window.set_show_add(v.show_add);
        window.set_show_search(v.show_search);
        window.set_search_placeholder(v.search_placeholder.into());
        window.set_bottom_label(v.bottom_label.into());
        window.set_sections(model(v.sections));
        window.set_has_details(v.details.is_some());
        if let Some(details) = v.details {
            window.set_details(details);
        }
        let empty = v.empty.unwrap_or_default();
        window.set_empty_icon(empty.icon.into());
        window.set_empty_title(empty.title.into());
        window.set_empty_text(empty.text.into());
        window.set_empty_action(empty.action.into());
        window.set_banner(v.banner.into());
        window.set_placeholder(v.placeholder.into());
        window.set_hostname(v.hostname.into());
        render_hotspot(window, &mut app, v.hotspot.as_ref());
        window.set_share_open(app.share.is_some());
        if let Some((title, password, text)) = &app.share {
            window.set_share_title(title.into());
            window.set_share_password(password.into());
            if let Some(image) = qr::image(text, 8) {
                window.set_share_qr(image);
            }
        }
        let (message, tone) = app.message.clone().unwrap_or_default();
        window.set_message(message.into());
        window.set_message_tone(if tone.is_empty() { "info".into() } else { tone.into() });
        window.set_confirm_open(app.confirm.is_some());
        if let Some((title, text, button)) = &app.confirm_text {
            window.set_confirm_title(title.into());
            window.set_confirm_text(text.into());
            window.set_confirm_button(button.into());
        }
        window.set_message_title(if app.message.is_some() { app.message_title.as_str().into() } else { "".into() });
        window.set_message_action(if app.retry.is_some() { "Try again".into() } else { "".into() });
        window.set_airplane(app.snapshot.as_ref().is_some_and(|s| s.airplane()));
        let keyring = app.keyring;
        window.set_keyring_label(
            match keyring {
                Some("KWallet") => "Kept in KWallet",
                Some(_) => "Kept in your keyring (Secret Service)",
                None => "No keyring found, so NetworkManager keeps them, readable by administrators",
            }
            .into(),
        );
        render_dialog(window, &mut app);
        editing::render(window, &mut app);
        window.set_add_open(app.add_open);
        window.set_hostname_open(app.hostname_open);
        window.set_hostname_error(app.hostname_error.as_str().into());
        app.row_ids = v.row_ids;
        // Keep a selection that went away (a network out of range) from sticking.
        app.ui.selected = v.selected;
        let tray_sub = match (app.settings.tray, app.tray.is_some()) {
            (true, false) => "Show Lituus in the system tray · no tray found: start a bar with one, such as Waybar",
            _ => "Show Lituus in the system tray",
        };
        window.set_options(model(vec![
            view::toggle("tray", "Tray icon", tray_sub, app.settings.tray),
            view::toggle(
                "keep_running",
                "Close to tray",
                "Closing the window keeps the tray icon running",
                app.settings.keep_running,
            ),
            view::toggle(
                "notifications",
                "Notifications",
                "Connected, disconnected and failed",
                app.settings.notifications,
            ),
        ]));
        render_appearance(window, &app);
        sound::render(window, &app.sound);
        displays::render(window, &app.displays);
        files::render(window, &app.files);
        render_quick(&mut app);
    });
}

fn render_dialog(window: &MainWindow, app: &mut App) {
    window.set_join_open(app.dialog.is_some());
    let Some(dialog) = &app.dialog else { return };
    let (mode, title, subtitle, error, security) = match dialog {
        Dialog::Join(t) => {
            let subtitle = format!("{} · signal {} %", view::security_long(t.security), t.strength);
            ("new", format!("Connect to {}", t.name), subtitle, String::new(), security_index(t.key_mgmt))
        }
        Dialog::Hidden { .. } => (
            "hidden",
            "Connect to a hidden network".to_owned(),
            "The network doesn't broadcast its name, so type it exactly.".to_owned(),
            String::new(),
            1,
        ),
        Dialog::Prompt(p) => {
            let error =
                if p.retry { "The password was rejected. Check it and try again.".to_owned() } else { String::new() };
            ("prompt", p.title.clone(), p.subtitle.clone(), error, security_index(p.key_mgmt))
        }
    };
    window.set_join_mode(mode.into());
    window.set_join_title(title.into());
    window.set_join_subtitle(subtitle.into());
    window.set_join_error(error.into());
    window.set_join_securities(model(SECURITIES.iter().map(|&s| s.into()).collect()));
    window.set_join_password_hints(model(PASSWORD_HINTS.iter().map(|&s| s.into()).collect()));
    let keyring_hint = match app.keyring {
        Some(name) => format!("Kept in your {name}. No admin password needed."),
        None => "No keyring is running, so NetworkManager keeps it, readable by administrators. Only you can use the connection.".into(),
    };
    window.set_join_storage_hints(model(vec![
        keyring_hint.into(),
        "NetworkManager keeps it for everyone on this computer. Needs an admin password.".into(),
        "Not saved: you'll be asked each time you connect.".into(),
    ]));
    if std::mem::take(&mut app.reset_dialog) {
        window.set_join_name("".into());
        window.set_join_security(security);
        window.set_join_password("".into());
        window.set_join_storage(0);
        window.set_join_autoconnect(true);
    }
}

/// Watches NetworkManager on a tokio thread, hands every snapshot to the UI thread, runs the
/// UI's commands (reporting their failures back), and answers the secret agent's requests.
fn spawn_network_manager(
    weak: slint::Weak<MainWindow>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<view::Command>,
) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        runtime.block_on(async move {
            let keyrings = secrets::Keyrings::default();
            let ui = AgentUi(weak.clone());
            loop {
                tell_keyring(&weak, &keyrings).await;
                let result = async {
                    let client = lituus_nm::Client::system().await?;
                    let mut snapshots = Box::pin(client.watch().await?);
                    // Without the agent (NetworkManager refuses agents outside a login session),
                    // everything but the password dialog still works.
                    let mut agent = match client.register_agent(APP_ID).await {
                        Ok(agent) => Some(agent),
                        Err(error) => {
                            eprintln!("lituus: couldn't register the secret agent: {error}");
                            None
                        }
                    };
                    // Devices whose traffic is counted already.
                    let mut counting = std::collections::HashSet::new();
                    loop {
                        tokio::select! {
                            snapshot = snapshots.next() => match snapshot {
                                Some(snapshot) => {
                                    // Count traffic on the devices in use, for "Received / sent".
                                    if let Ok(snap) = &snapshot {
                                        for d in snap.devices.iter().filter(|d| d.active_connection.is_some() && d.traffic.is_none()) {
                                            if counting.insert(d.path.clone()) {
                                                let _ = client.count_traffic(&d.path, 5000).await;
                                            }
                                        }
                                    }
                                    deliver(&weak, snapshot)
                                }
                                None => break,
                            },
                            Some(command) = commands.recv() => {
                                let (client, weak, keyrings) = (client.clone(), weak.clone(), keyrings.clone());
                                tokio::spawn(async move {
                                    match run(&client, &keyrings, &weak, &command).await {
                                        Ok(Some((reason, retry))) => failed(&weak, reason, retry),
                                        Ok(None) => {}
                                        Err(error) => report(&weak, &command, &error),
                                    }
                                });
                            }
                            Some(event) = async { agent.as_mut()?.recv().await } => {
                                let (keyrings, ui) = (keyrings.clone(), ui.clone());
                                tokio::spawn(async move { secrets::handle(event, &keyrings, &ui).await });
                            }
                        }
                    }
                    Ok::<(), lituus_nm::Error>(())
                }
                .await;
                if let Err(error) = result {
                    deliver(&weak, Err(error));
                }
                // The bus connection broke, or there's no system bus: try again shortly.
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });
    });
}

async fn tell_keyring(weak: &slint::Weak<MainWindow>, keyrings: &secrets::Keyrings) {
    let name = keyrings.get().await.map(|k| k.name());
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| app.borrow_mut().keyring = name);
        render(&window);
    });
}

/// The secret agent's way to the password dialog.
#[derive(Clone)]
struct AgentUi(slint::Weak<MainWindow>);

impl secrets::Ui for AgentUi {
    fn ask(&self, prompt: secrets::Prompt) {
        let _ = self.0.upgrade_in_event_loop(move |window| {
            APP.with(|app| {
                let mut app = app.borrow_mut();
                app.prompts.push_back(prompt);
                next_prompt(&mut app);
            });
            // Started in the background, or closed: the question needs the window.
            if !window.window().is_visible() {
                let _ = window.show();
            }
            render(&window);
        });
    }

    fn cancel(&self, connection_path: String) {
        let _ = self.0.upgrade_in_event_loop(move |window| {
            APP.with(|app| {
                let mut app = app.borrow_mut();
                app.prompts.retain(|p| p.connection_path != connection_path);
                if matches!(&app.dialog, Some(Dialog::Prompt(p)) if p.connection_path == connection_path) {
                    app.dialog = None;
                }
                next_prompt(&mut app);
            });
            render(&window);
        });
    }
}

/// Runs a command. For a connection attempt it waits for the outcome, and gives the reason it
/// failed and what "Try again" should run.
async fn run(
    client: &lituus_nm::Client,
    keyrings: &secrets::Keyrings,
    weak: &slint::Weak<MainWindow>,
    command: &view::Command,
) -> Result<Option<(String, Option<view::Command>)>, lituus_nm::Error> {
    use view::Command::*;
    let (active, device, retry) = match command {
        OpenEditor { connection, new } => {
            if *new == Some(editor::Kind::OpenVpn) && !openvpn::plugin_installed() {
                notice(weak, OPENVPN_MISSING);
                return Ok(None);
            }
            let settings = match connection {
                Some(path) => client.settings(path).await?,
                None => Default::default(),
            };
            let mut extras = editing::Extras::default();
            if let Some(path) = connection
                && settings.get("connection").and_then(|c| c.get("type")).and_then(|t| <&str>::try_from(&**t).ok())
                    == Some("wireguard")
            {
                // For the public key; NetworkManager may ask polkit, and then there's none.
                extras.wg_key =
                    client.secrets(path, "wireguard").await.ok().and_then(|s| s.get("private-key").cloned());
            }
            let zones = client.firewall_zones().await;
            let (path, new) = (connection.clone(), *new);
            let _ = weak.upgrade_in_event_loop(move |window| {
                APP.with(|app| editing::open(&mut app.borrow_mut(), path, settings, zones, new, extras));
                render(&window);
            });
            return Ok(None);
        }
        NewEnterprise { ssid, suite_b } => {
            let settings = editor::enterprise_settings(ssid, *suite_b, &secrets::new_uuid());
            let zones = client.firewall_zones().await;
            let _ = weak.upgrade_in_event_loop(move |window| {
                // Like an imported file: new, but filled in already.
                APP.with(|app| {
                    editing::open(&mut app.borrow_mut(), None, settings, zones, None, editing::Extras::default())
                });
                render(&window);
            });
            return Ok(None);
        }
        ChooseFile { field } => {
            let (title, filters): (&str, &[portal::Filter]) = if field.ends_with("key") {
                ("Choose the key", &[("Keys", &["*.pem", "*.key", "*.p12", "*.pfx"]), ("All files", &["*"])])
            } else {
                (
                    "Choose the certificate",
                    &[("Certificates", &["*.pem", "*.crt", "*.cer", "*.der", "*.p12"]), ("All files", &["*"])],
                )
            };
            let path = portal::open_file(title, filters)
                .await
                .map(|p| p.map(|p| p.to_string_lossy().into_owned()))
                .map_err(|e| e.to_string());
            let field = field.clone();
            let _ = weak.upgrade_in_event_loop(move |window| {
                APP.with(|app| editing::file_chosen(&mut app.borrow_mut(), &field, path));
                render(&window);
            });
            return Ok(None);
        }
        ImportFile => {
            import_file(client, weak).await;
            return Ok(None);
        }
        ExportWireGuard { connection } => {
            let settings = client.settings(connection).await?;
            let secrets = client.secrets_raw(connection, "wireguard").await?;
            let conf = wireguard::export(&wireguard::from_settings(&settings, &secrets));
            let name = settings
                .get("connection")
                .and_then(|c| c.get("id"))
                .and_then(|v| <&str>::try_from(&**v).ok())
                .unwrap_or("wireguard")
                .to_owned();
            match portal::save_file("Export the WireGuard configuration", &format!("{name}.conf")).await {
                Ok(Some(path)) => match write_private(&path, &conf) {
                    Ok(()) => {
                        notice(weak, &format!("Saved {}. It holds the private key: keep it safe.", path.display()))
                    }
                    Err(e) => notice(weak, &format!("Couldn't write {}: {e}.", path.display())),
                },
                Ok(None) => {}
                Err(e) => notice(weak, &format!("{e}.")),
            }
            return Ok(None);
        }
        SaveConnection { path, settings, secret, reapply, ports } => {
            let result =
                save_connection(client, keyrings, path.as_deref(), &settings.0, secret.as_ref(), reapply.as_ref())
                    .await;
            let result = match (result, ports) {
                (Ok((path, saved)), Some(plan)) => {
                    save_ports(client, &saved, plan).await.map(|added| (path, saved, added))
                }
                (Ok((path, saved)), None) => Ok((path, saved, Vec::new())),
                (Err(e), _) => Err(e),
            };
            let zones = client.firewall_zones().await;
            let _ = weak.upgrade_in_event_loop(move |window| {
                APP.with(|app| editing::saved(&mut app.borrow_mut(), result, zones));
                render(&window);
            });
            return Ok(None);
        }
        SetSetting { connection, setting, key, value } => {
            let value = match value {
                view::SettingValue::Bool(b) => zbus::zvariant::Value::from(*b),
                view::SettingValue::I32(i) => zbus::zvariant::Value::from(*i),
            };
            return client.set_setting(connection, setting, key, value).await.map(|()| None);
        }
        SaveHostname(name) => return client.save_hostname(name).await.map(|()| None),
        ShareWifi { connection, name } => {
            let settings = client.settings(connection).await?;
            let secrets = client.secrets(connection, secrets::WIFI_SECURITY).await.unwrap_or_default();
            let get = |setting: &str, key: &str| {
                settings
                    .get(setting)
                    .and_then(|g| g.get(key))
                    .and_then(|v| <&str>::try_from(&**v).ok())
                    .map(str::to_owned)
            };
            let ssid = settings
                .get("802-11-wireless")
                .and_then(|w| w.get("ssid"))
                .and_then(|v| <Vec<u8>>::try_from(v.try_clone().ok()?).ok())
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_else(|| name.clone());
            let hidden =
                settings.get("802-11-wireless").and_then(|w| w.get("hidden")).and_then(|v| bool::try_from(&**v).ok())
                    == Some(true);
            let wep = get(secrets::WIFI_SECURITY, "key-mgmt").as_deref() == Some("none");
            let open = !settings.contains_key(secrets::WIFI_SECURITY);
            let password = secrets.get("psk").or_else(|| secrets.get("wep-key0")).cloned();
            if !open && password.is_none() {
                notice(
                    weak,
                    &format!(
                        "Couldn't read the password of {name}: it may be asked for each time, or not readable without an admin password."
                    ),
                );
                return Ok(None);
            }
            let text = qr::wifi_text(&ssid, password.as_deref(), wep, hidden);
            let share = (format!("Share {name}"), password.unwrap_or_default(), text);
            let _ = weak.upgrade_in_event_loop(move |window| {
                APP.with(|app| app.borrow_mut().share = Some(share));
                render(&window);
            });
            return Ok(None);
        }
        LoadHotspot { connection } => {
            let settings = client.settings(connection).await?;
            let password =
                client.secrets(connection, secrets::WIFI_SECURITY).await.ok().and_then(|s| s.get("psk").cloned());
            let ssid = settings
                .get("802-11-wireless")
                .and_then(|w| w.get("ssid"))
                .and_then(|v| <Vec<u8>>::try_from(v.try_clone().ok()?).ok())
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let band = settings
                .get("802-11-wireless")
                .and_then(|w| w.get("band"))
                .and_then(|v| <&str>::try_from(&**v).ok())
                .map_or(0, |b| hotspot::BANDS.iter().position(|x| *x == b).unwrap_or(0));
            let fields = (ssid, password.unwrap_or_else(hotspot::new_password), band);
            let _ = weak.upgrade_in_event_loop(move |window| {
                APP.with(|app| app.borrow_mut().hotspot_fields = Some(fields));
                render(&window);
            });
            return Ok(None);
        }
        StopHotspot { active } => return client.deactivate(active).await.map(|()| None),
        StartHotspot { connection, device, ssid, password, band } => {
            let path = match connection {
                Some(path) => {
                    let mut settings = client.settings(path).await?;
                    hotspot::apply(&mut settings, ssid, password, *band);
                    client.update(path, &settings).await?;
                    path.clone()
                }
                None => {
                    let owner = secrets::username();
                    client
                        .add(&hotspot::settings(ssid, password, *band, &secrets::new_uuid(), owner.as_deref()))
                        .await?
                }
            };
            let active = client.activate(&path, Some(device), None).await?;
            let retry = Activate { connection: path, device: Some(device.clone()), access_point: None };
            (active, Some(device.clone()), retry)
        }
        SetWifi(on) => return client.set_wireless_enabled(*on).await.map(|()| None),
        SetAirplane(on) => return client.set_airplane(*on).await.map(|()| None),
        Rescan { device } => return client.request_scan(device).await.map(|()| None),
        Deactivate { active } => return client.deactivate(active).await.map(|()| None),
        Forget { connection } => return client.forget(connection).await.map(|()| None),
        Tether { device, address, name } => return client.tether(device, address, name).await.map(|_| None),
        Activate { connection, device, access_point } => {
            let active = client.activate(connection, device.as_deref(), access_point.as_deref()).await?;
            (active, device.clone(), command.clone())
        }
        ConnectOpen { device, access_point, ssid } => {
            let (connection, active) = client.connect_open(device, access_point, ssid).await?;
            let retry = Activate { connection, device: Some(device.clone()), access_point: Some(access_point.clone()) };
            (active, Some(device.clone()), retry)
        }
        Join { device, access_point, join } => {
            let mut join = join.clone();
            if let (lituus_nm::Storage::Agent, Some((key_mgmt, password))) = (join.storage, &join.security) {
                match keyrings.get().await {
                    Some(keyring) => {
                        let name = String::from_utf8_lossy(&join.ssid).into_owned();
                        let key = secrets::join_key(*key_mgmt);
                        let entry = lituus_keyring::Entry { uuid: &join.uuid, setting: secrets::WIFI_SECURITY, key };
                        keyring.store(entry, &name, password).await.map_err(|e| {
                            lituus_nm::Error::Refused(format!("the keyring didn't take the password ({e})"))
                        })?;
                    }
                    // No keyring after all: NetworkManager keeps it, and the connection stays the user's.
                    None => join.storage = lituus_nm::Storage::System,
                }
            }
            let joined = client.join_wifi(device, access_point.as_deref(), &join).await;
            if joined.is_err()
                && join.storage == lituus_nm::Storage::Agent
                && let Some(keyring) = keyrings.get().await
            {
                // No connection came of it, so its password goes too.
                let _ = keyring.delete(&join.uuid).await;
            }
            let (connection, active) = joined?;
            let retry = Activate { connection, device: Some(device.clone()), access_point: access_point.clone() };
            (active, Some(device.clone()), retry)
        }
    };
    Ok(match client.outcome(&active, device.as_deref()).await? {
        lituus_nm::Outcome::Failed(reason) => Some((reason, Some(retry))),
        lituus_nm::Outcome::Connected | lituus_nm::Outcome::Stopped => None,
    })
}

const OPENVPN_MISSING: &str = "OpenVPN needs NetworkManager's OpenVPN plugin: networkmanager-openvpn on Arch, network-manager-openvpn on Debian and Ubuntu, NetworkManager-openvpn on Fedora. Install it, then try again.";

/// A note above the list.
fn notice(weak: &slint::Weak<MainWindow>, text: &str) {
    let text = text.to_owned();
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            app.message_title.clear();
            app.retry = None;
            app.message = Some((text, "info"));
        });
        render(&window);
    });
}

/// Writes a file only the user can read (keys and certificates).
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(contents.as_bytes())
}

/// Asks for a WireGuard or OpenVPN file and opens it in the editor as a new connection.
async fn import_file(client: &lituus_nm::Client, weak: &slint::Weak<MainWindow>) {
    let filters: &[portal::Filter] = &[("WireGuard and OpenVPN", &["*.conf", "*.ovpn"]), ("All files", &["*"])];
    let path = match portal::open_file("Import a VPN connection", filters).await {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(e) => return notice(weak, &format!("{e}.")),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => return notice(weak, &format!("Couldn't read {}: {e}.", path.display())),
    };
    let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let owner = secrets::username();
    let uuid = secrets::new_uuid();
    let looks_wireguard = text.lines().any(|l| l.trim().eq_ignore_ascii_case("[interface]"));
    let (settings, banner) = if looks_wireguard {
        match wireguard::parse(&text) {
            Ok(conf) => {
                let name = file.trim_end_matches(".conf");
                let peers =
                    if conf.peers.len() == 1 { "1 peer".to_owned() } else { format!("{} peers", conf.peers.len()) };
                (wireguard::settings(&conf, name, &uuid, owner.as_deref()), format!("Imported from {file} · {peers}"))
            }
            Err(e) => return notice(weak, &format!("{file} isn't a WireGuard file Lituus can read: {e}")),
        }
    } else {
        if !openvpn::plugin_installed() {
            return notice(weak, OPENVPN_MISSING);
        }
        let name = file.trim_end_matches(".ovpn").trim_end_matches(".conf").to_owned();
        let home = std::env::var("HOME").unwrap_or_default();
        let dir = std::path::PathBuf::from(home).join(".local/share/lituus/certificates").join(&name);
        let full = path.to_string_lossy().into_owned();
        let imported = match openvpn::import(&text, &full, &dir) {
            Ok(imported) => imported,
            Err(e) => return notice(weak, &format!("{file} isn't an OpenVPN file Lituus can read: {e}")),
        };
        for (path, contents) in &imported.files {
            if let Err(e) = write_private(path, contents) {
                return notice(weak, &format!("Couldn't save {}: {e}.", path.display()));
            }
        }
        let mut settings = editor::new_settings(editor::Kind::OpenVpn, &uuid);
        let put = |s: &mut lituus_nm::Settings, k: &str, v: zbus::zvariant::Value| {
            s.entry("connection".into())
                .or_default()
                .insert(k.into(), zbus::zvariant::OwnedValue::try_from(v).expect("no fds"));
        };
        put(&mut settings, "id", imported.name.as_str().into());
        put(&mut settings, "autoconnect", false.into());
        if let Some(owner) = &owner {
            put(&mut settings, "permissions", vec![format!("user:{owner}:")].into());
        }
        openvpn::write(&mut settings, None, &imported.form);
        let mut banner = format!("Imported from {file}");
        if !imported.files.is_empty() {
            let n = imported.files.len();
            let files = if n == 1 { "1 certificate".to_owned() } else { format!("{n} certificates") };
            banner += &format!(
                " · {files} saved to {}",
                dir.display().to_string().replacen(&std::env::var("HOME").unwrap_or_default(), "~", 1)
            );
        }
        if !imported.ignored.is_empty() {
            banner += &format!(" · not used: {}", imported.ignored.join(", "));
        }
        (settings, banner)
    };
    let zones = client.firewall_zones().await;
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let extras = editing::Extras { wg_key: None, banner };
            editing::open(&mut app.borrow_mut(), None, settings, zones, None, extras)
        });
        render(&window);
    });
}

/// Saves the editor's settings and gives the saved connection's path and settings back, or why
/// it didn't work.
/// Adds a bridge's or bond's new ports and deletes the ones let go; the new ports' (interface,
/// connection) come back.
async fn save_ports(
    client: &lituus_nm::Client,
    controller: &lituus_nm::Settings,
    plan: &virt::PortPlan,
) -> Result<Vec<(String, String)>, String> {
    let uuid = controller
        .get("connection")
        .and_then(|c| c.get("uuid"))
        .and_then(|u| <&str>::try_from(&**u).ok())
        .unwrap_or_default()
        .to_owned();
    for path in &plan.remove {
        client.forget(path).await.map_err(|e| format!("Saved, but a port couldn't be let go: {e}."))?;
    }
    let mut added = Vec::new();
    for interface in &plan.add {
        let settings = virt::port_settings(plan, &uuid, interface, &secrets::new_uuid());
        let path = client
            .add(&settings)
            .await
            .map_err(|e| format!("Saved, but {interface} couldn't be added as a port: {e}."))?;
        added.push((interface.clone(), path));
    }
    Ok(added)
}

async fn save_connection(
    client: &lituus_nm::Client,
    keyrings: &secrets::Keyrings,
    path: Option<&str>,
    settings: &lituus_nm::Settings,
    secret: Option<&editing::KeyringSecret>,
    reapply: Option<&(String, Option<String>)>,
) -> Result<(String, lituus_nm::Settings), String> {
    let explain = |e: lituus_nm::Error| {
        match e {
        lituus_nm::Error::Denied(_) => {
            "Not allowed: NetworkManager asks polkit first, and no polkit agent answered. Start one (for example hyprpolkitagent)."
                .to_owned()
        }
        other => format!("Couldn't save: {other}."),
    }
    };
    if let Some((uuid, key, name, password)) = secret {
        match keyrings.get().await {
            Some(keyring) => {
                let entry = lituus_keyring::Entry { uuid, setting: secrets::WIFI_SECURITY, key };
                keyring
                    .store(entry, name, password)
                    .await
                    .map_err(|e| format!("The keyring didn't take the password ({e})."))?;
            }
            None => return Err("No keyring is running for \"for me only\". Pick \"All users\" or start one.".into()),
        }
    }
    let path = match path {
        Some(path) => {
            let mut settings = settings.clone();
            // Edited WireGuard peers come without their pre-shared keys; put the saved ones back.
            if settings.get("wireguard").is_some_and(|w| w.contains_key("peers"))
                && let Ok(secrets) = client.secrets_raw(path, "wireguard").await
            {
                wireguard::keep_preshared_keys(&mut settings, &secrets);
            }
            client.update(path, &settings).await.map_err(explain)?;
            path.to_owned()
        }
        None => client.add(settings).await.map_err(explain)?,
    };
    // Up already: apply it there too, or reconnect when that isn't possible.
    if let Some((_, Some(device))) = reapply
        && client.reapply(device).await.is_err()
    {
        client.activate(&path, Some(device), None).await.map_err(explain)?;
    }
    let saved = client.settings(&path).await.map_err(explain)?;
    Ok((path, saved))
}

/// A connection attempt failed: mark its row, and offer "Try again".
fn failed(weak: &slint::Weak<MainWindow>, reason: String, retry: Option<view::Command>) {
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            let (row, name) = app.attempt.clone().unwrap_or_default();
            app.message_title =
                if name.is_empty() { "Couldn't connect".to_owned() } else { format!("Couldn't connect to {name}") };
            app.message = Some((reason.clone(), "danger"));
            app.retry = retry;
            if app.settings.notifications
                && !window.window().is_visible()
                && let Some(notifier) = &app.notifier
            {
                notifier.send(notify::failure(&name, &reason));
            }
            app.ui.failed = (!row.is_empty()).then_some((row, reason));
        });
        render(&window);
    });
}

/// Shows why a command failed, above the list.
fn report(weak: &slint::Weak<MainWindow>, command: &view::Command, error: &lituus_nm::Error) {
    use view::Command::*;
    let what = match command {
        SetWifi(true) => "Couldn't turn Wi-Fi on",
        SetWifi(false) => "Couldn't turn Wi-Fi off",
        SetAirplane(true) => "Couldn't turn airplane mode on",
        SetAirplane(false) => "Couldn't turn airplane mode off",
        Rescan { .. } => "Couldn't scan",
        Activate { .. } | ConnectOpen { .. } | Join { .. } => "Couldn't connect",
        OpenEditor { .. } | NewEnterprise { .. } => "Couldn't read the connection",
        SaveConnection { .. } => "Couldn't save the connection",
        SetSetting { .. } => "Couldn't change the connection",
        SaveHostname(_) => "Couldn't change the hostname",
        ChooseFile { .. } => "Couldn't open the file dialog",
        ImportFile => "Couldn't import the file",
        ExportWireGuard { .. } => "Couldn't export the configuration",
        ShareWifi { .. } => "Couldn't share the network",
        LoadHotspot { .. } => "Couldn't read the hotspot's settings",
        StartHotspot { .. } => "Couldn't start the hotspot",
        StopHotspot { .. } => "Couldn't stop the hotspot",
        Deactivate { .. } => "Couldn't disconnect",
        Forget { .. } => "Couldn't forget the connection",
        Tether { .. } => "Couldn't use the phone's internet",
    };
    let (text, tone) = match error {
        lituus_nm::Error::Denied(_) => (
            format!(
                "{what}: not allowed. NetworkManager asks polkit first, and no polkit agent answered. Start one (for example hyprpolkitagent) and try again."
            ),
            "warn",
        ),
        other => (format!("{what}: {other}."), "danger"),
    };
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            app.message_title.clear();
            app.retry = None;
            app.message = Some((text, tone));
        });
        render(&window);
    });
}

fn deliver(weak: &slint::Weak<MainWindow>, snapshot: Result<lituus_nm::Snapshot, lituus_nm::Error>) {
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            match snapshot {
                Ok(snapshot) => {
                    if app.settings.notifications
                        && !window.window().is_visible()
                        && let (Some(before), Some(notifier)) = (&app.snapshot, &app.notifier)
                    {
                        for notice in notify::changes(before, &snapshot) {
                            notifier.send(notice);
                        }
                    }
                    app.snapshot = Some(snapshot);
                    app.error = None;
                }
                Err(error) => {
                    app.snapshot = None;
                    app.error = Some(error.to_string());
                }
            }
        });
        render(&window);
    });
}

/// What the colour sources looked like last time, to notice changes.
type Signature = Vec<(std::path::PathBuf, Option<SystemTime>, u64)>;

fn signature(config_path: &std::path::Path, config: &config::Config) -> Signature {
    let stat = |path: std::path::PathBuf| {
        let meta = std::fs::metadata(&path).ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        let len = meta.map(|m| m.len()).unwrap_or(0);
        (path, modified, len)
    };
    std::iter::once(stat(config_path.to_owned()))
        .chain(config.theme.sources.iter().map(|s| stat(s.resolved())))
        .collect()
}

/// Re-reads config.toml and the colour sources whenever one of them changes, so the window
/// recolours with the wallpaper.
fn spawn_theme_watcher(weak: slint::Weak<MainWindow>) {
    std::thread::spawn(move || {
        let path = config::path();
        let mut last: Option<Signature> = None;
        loop {
            let config = config::load(&path);
            let now = signature(&path, &config);
            if last.as_ref() != Some(&now) {
                let follow = config.theme.follow_wallpaper;
                let (statuses, palette) = theme::resolve(&config.theme.sources, follow);
                let wallpaper = theme::resolve(&config.theme.sources, true).1;
                let rows = source_rows(&config, &statuses, follow);
                let (builtin, mode, gradient) = (config.theme.builtin, config.theme.mode, config.theme.gradient);
                let overamplify = config.sound.overamplify;
                let settings = Settings::from(&config);
                let config_path = path.display().to_string();
                let _ = weak.upgrade_in_event_loop(move |window| {
                    APP.with(|app| {
                        let mut app = app.borrow_mut();
                        app.palette = palette;
                        app.wallpaper = wallpaper;
                        app.builtin = builtin;
                        app.mode = mode;
                        app.follow_wallpaper = follow;
                        app.gradient = gradient;
                        app.sound.overamplify = overamplify;
                    });
                    apply_theme(&window);
                    apply_settings(&window, settings);
                    render(&window);
                    window.set_config_path(shorten_home(&config_path).into());
                    window.set_sources(model(rows.into_iter().map(SourceRow::from).collect()));
                });
                last = Some(now);
            }
            std::thread::sleep(THEME_POLL);
        }
    });
}

/// A colour source row for Settings, built off the UI thread (no Slint types in it).
struct SourceRowData {
    n: i32,
    label: String,
    path: String,
    status: &'static str,
    tone: &'static str,
    swatches: Vec<theme::Rgb>,
}

impl From<SourceRowData> for SourceRow {
    fn from(d: SourceRowData) -> Self {
        SourceRow {
            n: d.n,
            label: d.label.into(),
            path: d.path.into(),
            status: d.status.into(),
            tone: d.tone.into(),
            swatches: model(d.swatches.into_iter().map(color).collect()),
        }
    }
}

fn source_rows(config: &config::Config, statuses: &[theme::SourceStatus], follow: bool) -> Vec<SourceRowData> {
    let mut used = false;
    let rows: Vec<SourceRowData> = config
        .theme
        .sources
        .iter()
        .zip(statuses)
        .enumerate()
        .map(|(i, (source, status))| {
            let label = if source.path.contains("ml4w") { "ML4W (matugen)".into() } else { "Colour file".into() };
            let (status, tone, swatches) = match status {
                theme::SourceStatus::Missing => ("Not found", "faint", Vec::new()),
                theme::SourceStatus::Unusable => ("Not usable", "danger", Vec::new()),
                theme::SourceStatus::Used(p) if !used => {
                    used = true;
                    let status = if follow { "In use" } else { "Found · not followed" };
                    (
                        status,
                        if follow { "accent" } else { "muted" },
                        vec![p.accent, p.accent_soft_bg, p.surface, p.danger],
                    )
                }
                theme::SourceStatus::Used(_) => ("Usable", "muted", Vec::new()),
            };
            SourceRowData { n: i as i32 + 1, label, path: source.path.clone(), status, tone, swatches }
        })
        .collect();
    rows
}

fn shorten_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

fn color(rgb: theme::Rgb) -> slint::Color {
    slint::Color::from_rgb_u8(rgb.0, rgb.1, rgb.2)
}

fn apply_theme(window: &MainWindow) {
    let (palette, builtin, mode, gradient) = APP.with(|app| {
        let app = app.borrow();
        (app.palette, app.builtin, app.mode, app.gradient)
    });
    let p = palette.unwrap_or_else(|| theme::Palette::builtin(builtin, mode.dark(window.get_prefers_dark())));
    set_theme(window.global::<Theme>(), &p);
    window.global::<Theme>().set_gradient(gradient);
    QUICK.with(|q| {
        if let Some(q) = q.borrow().as_ref() {
            set_theme(q.global::<Theme>(), &p);
        }
    });
    render(window);
}

fn set_theme(t: Theme<'_>, p: &theme::Palette) {
    t.set_bg(color(p.bg));
    t.set_panel(color(p.panel));
    t.set_surface(color(p.surface));
    t.set_border(color(p.border));
    t.set_border2(color(p.border2));
    t.set_ctrl_border(color(p.ctrl_border));
    t.set_ctrl_bg(color(p.ctrl_bg));
    t.set_input_bg(color(p.input_bg));
    t.set_text(color(p.text));
    t.set_muted(color(p.muted));
    t.set_faint(color(p.faint));
    t.set_icon_soft(color(p.icon_soft));
    t.set_accent(color(p.accent));
    t.set_on_accent(color(p.on_accent));
    t.set_accent_soft_bg(color(p.accent_soft_bg));
    t.set_accent_soft_border(color(p.accent_soft_border));
    t.set_off_track(color(p.off_track));
    t.set_knob_off(color(p.knob_off));
    t.set_bar_off(color(p.bar_off));
    t.set_sel_bg(color(p.sel_bg));
    t.set_danger(color(p.danger));
    t.set_warn_bg(color(p.warn_bg));
    t.set_warn_border(color(p.warn_border));
    t.set_warn_text(color(p.warn_text));
    t.set_glow(color(p.glow));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use i_slint_backend_testing::{ElementHandle, mock_elapsed_time};
    use slint::{ComponentHandle, LogicalSize};

    use super::MainWindow;

    /// Widening the window must not show the closed editor in the new space, not even while
    /// the editor's slide animation would be running.
    #[test]
    fn the_closed_editor_stays_out_of_sight_when_the_window_grows() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(1040.0, 700.0));
        window.show().unwrap();
        // Where the editor is, if it's drawn at all (a hidden element isn't found).
        let editor =
            || ElementHandle::find_by_element_id(&window, "MainWindow::editor").next().map(|e| e.absolute_position().x);
        assert_eq!(editor(), None);

        window.window().set_size(LogicalSize::new(1600.0, 700.0));
        for _ in 0..5 {
            assert_eq!(editor(), None);
            mock_elapsed_time(Duration::from_millis(50));
        }

        // Opening it still slides it in.
        window.set_editor_open(true);
        mock_elapsed_time(Duration::from_millis(50));
        let x = editor().expect("the editor is shown");
        assert!(0.0 < x && x < 1600.0, "{x}");
        mock_elapsed_time(Duration::from_millis(500));
        assert_eq!(editor(), Some(0.0));
    }
}
