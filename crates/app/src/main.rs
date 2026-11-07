use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use clap::Parser;
use futures_util::StreamExt;
use slint::{ComponentHandle, ModelRc, VecModel};

mod airplane;
mod audio;
mod bluetooth;
mod config;
mod displays;
mod editing;
mod editor;
mod files;
mod hotspot;
mod input;
mod instance;
mod nav;
mod notify;
mod openconnect;
mod openvpn;
mod portal;
mod profiles;
mod qr;
mod quick;
mod secrets;
mod shortcuts;
mod sound;
mod theme;
mod tray;
mod view;
mod virt;
mod wireguard;
mod xkb;

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

/// The confirmation dialog: its title, text and button, and what accepting it does. The button
/// is red when the action can't be undone (Forget, Delete), else the usual primary (Update).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Confirm {
    pub title: String,
    pub text: String,
    pub button: String,
    pub destructive: bool,
    pub then: Confirmed,
}

/// What a confirmation dialog runs once it's accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmed {
    Network(view::Command),
    Bluetooth(bluetooth::Command),
    /// A Displays profile change ("profile-update", "profile-delete") and the profile's name.
    Displays(&'static str, String),
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
    /// The confirmation dialog, while it's open.
    confirm: Option<Confirm>,
    /// The password dialog, and NetworkManager's requests waiting behind it.
    dialog: Option<Dialog>,
    prompts: VecDeque<secrets::Prompt>,
    /// Set when a dialog opens, to clear its fields once.
    reset_dialog: bool,
    /// What the keyring is called ("keyring", "KWallet"); `None` while there's none.
    keyring: Option<&'static str>,
    /// The row and name of the last connection attempt, for its failure.
    attempt: Option<(String, String)>,
    /// The connections (by UUID) whose sign-in window is open; kept here, since `ui` starts over
    /// on every page.
    signing_in: Vec<String>,
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
    /// The popup's Bluetooth (on or off, if there is any, and the devices) and volume (see
    /// `sound::quick`), as last shown.
    quick_bt: (Option<bool>, Vec<bluetooth::QuickDevice>),
    quick_sound: Option<(f32, bool, f32)>,
    notifier: Option<notify::Notifier>,
    /// The Bluetooth page.
    bt: bluetooth::State,
    /// Files over Bluetooth.
    files: files::State,
    /// The Displays page.
    displays: displays::State,
    /// The Sound page.
    sound: sound::State,
    /// The Input pages, and which of them was open last (for the Input item).
    input: input::State,
    input_page: Option<view::Page>,
    /// Input › Shortcuts.
    shortcuts: shortcuts::State,
    /// Airplane mode, from when it's turned on until it ends.
    airplane: Option<airplane::Airplane>,
    /// The sidebar's search.
    nav_query: String,
    /// The sidebar items whose sub-items are folded away, for this session only (config.toml
    /// holds settings, not the window's state).
    nav_folded: Vec<String>,
    /// The Network sub-page last open, for coming back to it from the sidebar.
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
    connect_focus(&window);
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
    input::refresh(&window.as_weak());
    shortcuts::refresh(&window.as_weak());
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
    // The Bluetooth Visible switch counts down ("2:41").
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
                let q = quick::build(Some(snap), app.quick.airplane, &app.signing_in);
                let page = if id.starts_with("wifi:") { "wifi" } else { "vpn" };
                let action = if id.starts_with("wifi:") {
                    if q.networks.iter().any(|n| n.id == id && n.active) {
                        return;
                    }
                    "connect"
                } else if id.strip_prefix("conn:").is_some_and(|uuid| app.signing_in.iter().any(|u| u == uuid)) {
                    "cancel-sign-in"
                } else if q.vpns.iter().any(|v| v.id == id && v.on) {
                    "disconnect"
                } else {
                    "connect"
                };
                go(app, page);
                app.ui.selected = Some(id);
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
        tray::Event::SetAirplane(on) => update(&weak, |app| set_airplane(app, on)),
        tray::Event::Open => {
            hide_quick();
            let _ = window.show();
        }
        tray::Event::Settings => {
            hide_quick();
            update(&weak, |app| go(app, "settings"));
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
        popup.on_row_button({
            let main = window.as_weak();
            move |id| {
                let Some(window) = main.upgrade() else { return };
                if let Some(device) = id.strip_prefix("bt:") {
                    // A Bluetooth device: connect or disconnect it.
                    let device = device.to_owned();
                    update(&main, |app| {
                        app.bt.selected = Some(device);
                        bluetooth::activate(&mut app.bt);
                    });
                } else if id.starts_with("wifi:")
                    && APP.with(|app| app.borrow().quick.networks.iter().any(|n| n.id == id.as_str() && n.active))
                {
                    // The connected network: Disconnect, as in its details.
                    update(&main, |app| {
                        go(app, "wifi");
                        app.ui.selected = Some(id.to_string());
                        act(app, "disconnect");
                    });
                } else if id.starts_with("wifi:") {
                    on_tray(&window, tray::Event::Connect(id.into()));
                } else {
                    // A VPN: connect, disconnect, or cancel its sign-in.
                    on_tray(&window, tray::Event::Toggle(id.into()));
                }
            }
        });
        popup.on_row_opened({
            let main = window.as_weak();
            move |id| {
                let Some(window) = main.upgrade() else { return };
                hide_quick();
                update(&main, |app| {
                    if let Some(device) = id.strip_prefix("bt:") {
                        go(app, "bluetooth");
                        app.bt.selected = Some(device.to_owned());
                    } else {
                        go(app, if id.starts_with("wifi:") { "wifi" } else { "vpn" });
                        app.ui.selected = Some(id.to_string());
                    }
                });
                let _ = window.show();
            }
        });
        popup.on_bt_switched({
            let main = window.as_weak();
            move || {
                if let Some(window) = main.upgrade() {
                    bluetooth::act(&window, "power", "");
                }
            }
        });
        popup.on_volume_moved({
            let main = window.as_weak();
            move |v| {
                update(&main, |app| {
                    sound::act(&mut app.sound, "out-volume", "", v);
                })
            }
        });
        popup.on_mute_clicked({
            let main = window.as_weak();
            move || {
                update(&main, |app| {
                    sound::act(&mut app.sound, "out-mute", "", 0.0);
                })
            }
        });
        popup.on_airplane_switched({
            let main = window.as_weak();
            move || {
                if let Some(window) = main.upgrade() {
                    let on = APP.with(|app| app.borrow().quick.airplane);
                    on_tray(&window, tray::Event::SetAirplane(!on));
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
fn render_quick(app: &mut App, airplane: bool) {
    let q = quick::build(app.snapshot.as_ref(), airplane, &app.signing_in);
    let bt = bluetooth::quick(&app.bt);
    let sound = sound::quick(&app.sound);
    if q == app.quick && bt == app.quick_bt && sound == app.quick_sound && !std::mem::take(&mut app.quick_new) {
        return;
    }
    if let Some(tray) = &app.tray {
        tray.show(q.clone());
    }
    QUICK.with(|popup| {
        let Some(popup) = popup.borrow().as_ref().map(|p| p.as_weak()) else { return };
        let Some(popup) = popup.upgrade() else { return };
        // The one quick action on a row: connect it, or disconnect it once it's on.
        let button = |active: bool| if active { "Disconnect" } else { "Connect" };
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
                    icon: "bars".into(),
                    bars: n.bars.into(),
                    active: n.active,
                    button: button(n.active).into(),
                })
                .collect(),
        ));
        popup.set_vpns(model(
            q.vpns
                .iter()
                .map(|v| {
                    let signing_in = v.id.strip_prefix("conn:").is_some_and(|u| app.signing_in.iter().any(|s| s == u));
                    QuickRow {
                        id: v.id.as_str().into(),
                        name: v.name.as_str().into(),
                        meta: v.meta.as_str().into(),
                        icon: "vpn".into(),
                        active: v.on,
                        button: if signing_in { "Cancel".into() } else { button(v.on).into() },
                        ..QuickRow::default()
                    }
                })
                .collect(),
        ));
        popup.set_bt_available(bt.0.is_some());
        popup.set_bt_on(bt.0 == Some(true));
        popup.set_devices(model(
            bt.1.iter()
                .map(|d| QuickRow {
                    id: d.id.as_str().into(),
                    name: d.name.as_str().into(),
                    meta: d.meta.as_str().into(),
                    icon: d.icon.into(),
                    active: d.connected,
                    button: button(d.connected).into(),
                    ..QuickRow::default()
                })
                .collect(),
        ));
        popup.set_sound_available(sound.is_some());
        if let Some((volume, muted, max)) = sound {
            popup.set_volume(volume);
            popup.set_muted(muted);
            popup.set_volume_max(max);
        }
        popup.set_airplane(q.airplane);
    });
    app.quick = q;
    app.quick_bt = bt;
    app.quick_sound = sound;
}

/// A dialog's FocusTrap asks for Tab (or Shift+Tab) to be pressed again once it has moved the
/// focus to its other end, which sends it round to the first (or last) control. Slint can't press
/// keys itself, and it's done after the key that got there, not inside its handling.
fn connect_focus(window: &MainWindow) {
    use slint::platform::{Key, WindowEvent};
    use std::{cell::Cell, rc::Rc};
    let weak = window.as_weak();
    let stepping = Rc::new(Cell::new(false));
    window.global::<Focus>().on_step(move |forward| {
        // A card with nothing to focus would send it round for ever.
        if stepping.get() {
            return;
        }
        let (weak, stepping) = (weak.clone(), stepping.clone());
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(window) = weak.upgrade() else { return };
            let key = slint::SharedString::from(if forward { Key::Tab } else { Key::Backtab });
            stepping.set(true);
            window.window().dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
            window.window().dispatch_event(WindowEvent::KeyReleased { text: key });
            stepping.set(false);
        });
    });
}

fn connect_callbacks(window: &MainWindow) {
    let weak = window.as_weak();
    window.on_navigate(move |id| {
        update(&weak, |app| go(app, &id));
        // Devices come and go without Hyprland saying so: read them again.
        if APP.with(|app| matches!(app.borrow().ui.page, view::Page::Keyboard | view::Page::Mouse)) {
            input::refresh(&weak);
        }
        // The config may have changed since.
        if APP.with(|app| app.borrow().ui.page == view::Page::Shortcuts) {
            shortcuts::refresh(&weak);
        }
    });
    let weak = window.as_weak();
    window.on_nav_search(move |text| update(&weak, |app| app.nav_query = text.to_string()));
    let weak = window.as_weak();
    window.on_nav_fold(move |id| {
        update(&weak, |app| match app.nav_folded.iter().position(|f| *f == id.as_str()) {
            Some(i) => _ = app.nav_folded.remove(i),
            None => app.nav_folded.push(id.into()),
        })
    });
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
            match app.ui.page {
                view::Page::Bluetooth => return bluetooth::move_selection(&mut app.bt, delta),
                view::Page::Keyboard => return input::move_layout_selection(&mut app.input, delta),
                view::Page::Mouse => return input::move_selection(&mut app.input, delta),
                view::Page::Shortcuts => return,
                _ => {}
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
            view::Page::Keyboard | view::Page::Mouse | view::Page::Shortcuts => {}
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
    let weak = window.as_weak();
    window.on_metered_chosen(move |choice| {
        update(&weak, |app| {
            let command = match (&app.snapshot, &app.ui.selected) {
                (Some(snap), Some(selected)) => view::metered_command(snap, selected, choice),
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
        update(&weak, |app| {
            if let Some(confirm) = displays::act(&w, &mut app.displays, &verb, &arg, value) {
                app.confirm = Some(confirm);
            }
        })
    });
    let weak = window.as_weak();
    window.on_displays_drop(move |i, x, y| {
        update(&weak, |app| displays::drop(&mut app.displays, i.max(0) as usize, x, y))
    });
    // Asked on every move of a drag: it only reads.
    window.on_displays_snap(|i, x, y, threshold, free| {
        APP.with(|app| match app.try_borrow() {
            Ok(app) => displays::snap(&app.displays, i.max(0) as usize, x, y, threshold, free),
            Err(_) => DragSnap { x, y, ..DragSnap::default() },
        })
    });
    let weak = window.as_weak();
    window.on_input_act(move |verb, arg, value| {
        let w = weak.clone();
        update(&weak, |app| input::act(&w, &mut app.input, &verb, &arg, value))
    });
    let weak = window.as_weak();
    window.on_shortcuts_act(move |verb, arg| {
        let w = weak.clone();
        update(&weak, |app| shortcuts::act(&w, &mut app.shortcuts, &verb, &arg))
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
            // Wi-Fi is off because of airplane mode: end it, which turns Wi-Fi back on.
            view::Page::Wifi if app.ui.airplane => set_airplane(app, false),
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
                "openconnect" => editor::Kind::OpenConnect,
                "vlan" => editor::Kind::Vlan,
                "bridge" => editor::Kind::Bridge,
                "bond" => editor::Kind::Bond,
                "import" => return send(app, view::Command::ImportFile),
                "hotspot" => return go(app, "hotspot"),
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
        update(&weak, |app| match app.confirm.take().map(|c| c.then) {
            Some(Confirmed::Network(command)) => send(app, command),
            Some(Confirmed::Bluetooth(command)) => app.bt.send(command),
            Some(Confirmed::Displays(verb, name)) => displays::confirmed(&mut app.displays, verb, &name),
            None => {}
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
        update(&weak, |app| match app.ui.page {
            view::Page::Wifi => {
                if let Some(device) = app.snapshot.as_ref().and_then(view::hidden_network_device) {
                    open_dialog(app, Dialog::Hidden { device });
                }
            }
            view::Page::Wired => {
                send(app, view::Command::OpenEditor { connection: None, new: Some(editor::Kind::Ethernet) })
            }
            _ => send(app, view::Command::ImportFile),
        })
    });
    let weak = window.as_weak();
    window.on_airplane_toggled(move || update(&weak, |app| set_airplane(app, !app.quick.airplane)));
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

/// Goes to a page (a sidebar item's id); Network and Input open their sub-page used last.
fn go(app: &mut App, id: &str) {
    let id = match (id, app.input_page) {
        ("input", Some(page)) => page.id(),
        _ => id,
    };
    match view::navigate(&mut app.ui, &mut app.net_page, id) {
        Some(view::Page::Hotspot) => open_hotspot(app),
        Some(page @ (view::Page::Keyboard | view::Page::Mouse | view::Page::Shortcuts)) => app.input_page = Some(page),
        _ => {}
    }
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
    // Its sign-in window is open: Enter, the popup and the tray don't open a second one.
    if action == "connect"
        && selected.strip_prefix("conn:").is_some_and(|uuid| app.signing_in.iter().any(|u| u == uuid))
    {
        return;
    }
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
            app.confirm = Some(Confirm { title, text, button, destructive: true, then: Confirmed::Network(command) });
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

/// Airplane mode on or off: NetworkManager switches Wi-Fi and mobile broadband, the Bluetooth
/// thread the adapters.
fn set_airplane(app: &mut App, on: bool) {
    let change = airplane::switch(&mut app.airplane, on, app.snapshot.as_ref(), app.bt.snapshot.as_ref());
    send(app, view::Command::SetAirplane { on, wifi: change.wifi, wwan: change.wwan });
    for (adapter, on) in change.bluetooth {
        app.bt.send(bluetooth::Command::Power { adapter, on });
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

impl Hint {
    /// A key in the bar at the bottom ("↵") and what it does there ("Connect").
    pub fn new(key: &str, label: &str) -> Self {
        Hint { key: key.into(), label: label.into() }
    }
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
                        "configreloaded" => {
                            displays::refresh(&weak);
                            input::refresh(&weak);
                            shortcuts::refresh(&weak);
                        }
                        // A keyboard switched layouts (Super+Space, say).
                        "activelayout" => input::refresh(&weak),
                        _ => {}
                    }
                    true
                });
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

/// Settings' Appearance section, and the look in use in its hero.
fn render_appearance(window: &MainWindow, app: &App) {
    let in_use = app.palette.is_some();
    let dark = app.mode.dark(window.get_prefers_dark());
    window.set_ap_follow(app.follow_wallpaper);
    window.set_ap_follow_available(app.wallpaper.is_some());
    window.set_ap_wallpaper_in_use(in_use);
    let (name, text) = if in_use {
        (
            "Wallpaper colours".to_owned(),
            "Made by matugen from your wallpaper; the window recolours when it changes.".to_owned(),
        )
    } else {
        let auto = if app.mode == config::Mode::Auto { ", as the desktop prefers" } else { "" };
        (app.builtin.label().to_owned(), format!("Built-in theme · {}{auto}", if dark { "dark" } else { "light" }))
    };
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

/// The sidebar's pages; Network's statuses are filled in from the view.
fn nav_entries(app: &App) -> Vec<nav::Entry> {
    let (keyboard, mouse) = input::status(&app.input);
    vec![
        nav::Entry {
            id: "network",
            label: "Network",
            icon: "wifi",
            group: "CONNECTIONS",
            keywords: "internet networkmanager saved connections hostname",
            status: String::new(),
            parent: None,
        },
        nav::Entry {
            id: "wifi",
            label: "Wi-Fi",
            icon: "wifi",
            group: "CONNECTIONS",
            keywords: "wi-fi wifi wireless wlan networks hidden enterprise eduroam saved",
            status: String::new(),
            parent: Some("network"),
        },
        nav::Entry {
            id: "wired",
            label: "Wired",
            icon: "wired",
            group: "CONNECTIONS",
            keywords: "ethernet cable lan bridge bond vlan virtual interfaces profiles",
            status: String::new(),
            parent: Some("network"),
        },
        nav::Entry {
            id: "vpn",
            label: "VPN",
            icon: "vpn",
            group: "CONNECTIONS",
            keywords: "wireguard openvpn openconnect tunnel import",
            status: String::new(),
            parent: Some("network"),
        },
        nav::Entry {
            id: "hotspot",
            label: "Hotspot",
            icon: "hotspot",
            group: "CONNECTIONS",
            keywords: "tethering share access point",
            status: String::new(),
            parent: Some("network"),
        },
        nav::Entry {
            id: "bluetooth",
            label: "Bluetooth",
            icon: "bluetooth",
            group: "CONNECTIONS",
            keywords: "pair pairing headphones headset keyboard mouse speaker devices visible discoverable pairable adapter",
            status: bluetooth::nav(&app.bt, false).sub.to_string(),
            parent: None,
        },
        nav::Entry {
            id: "displays",
            label: "Displays",
            icon: "monitor",
            group: "DEVICES",
            keywords: "monitors screens resolution refresh rate scale hdr vrr rotation mirror hyprland layout",
            status: displays::status(&app.displays),
            parent: None,
        },
        nav::Entry {
            id: "sound",
            label: "Sound",
            icon: "volume",
            group: "DEVICES",
            keywords: "audio volume speakers headphones microphone input output pipewire mute apps",
            status: sound::status(&app.sound),
            parent: None,
        },
        nav::Entry {
            id: "input",
            label: "Input",
            icon: "keyboard",
            group: "DEVICES",
            keywords: "keyboard mouse touchpad trackpad pointer devices",
            status: keyboard.clone(),
            parent: None,
        },
        nav::Entry {
            id: "keyboard",
            label: "Keyboard",
            icon: "keyboard",
            group: "DEVICES",
            keywords: "keyboard layout layouts language switch xkb repeat delay rate caps lock compose num lock numlock \
                       typing keys dvorak qwertz azerty",
            status: keyboard,
            parent: Some("input"),
        },
        nav::Entry {
            id: "mouse",
            label: "Mouse & touchpad",
            icon: "mouse",
            group: "DEVICES",
            keywords: "mouse mice touchpad trackpad trackpoint pointer speed sensitivity acceleration natural scrolling \
                       scroll speed left-handed tap to click disable while typing clickfinger",
            status: mouse,
            parent: Some("input"),
        },
        nav::Entry {
            id: "shortcuts",
            label: "Shortcuts",
            icon: "keyboard",
            group: "DEVICES",
            keywords: "shortcuts shortcut keybinds keybind binds bind hotkeys hotkey keys super workspace launch",
            status: shortcuts::status(&app.shortcuts),
            parent: Some("input"),
        },
        // Lituus's own look is in Settings; "Appearance" is kept for the desktop's (GTK) later.
        nav::Entry {
            id: "settings",
            label: "Settings",
            icon: "settings",
            group: nav::LITUUS,
            keywords: "appearance theme colours colors dark light mode matugen wallpaper accent gradient tray \
                       notifications passwords keyring config",
            status: String::new(),
            parent: None,
        },
    ]
}

fn render(window: &MainWindow) {
    APP.with(|app| {
        let mut app = app.borrow_mut();
        app.ui.signing_in = app.signing_in.clone();
        let state = &mut *app;
        let airplane = airplane::follow(&mut state.airplane, state.snapshot.as_ref(), state.bt.snapshot.as_ref());
        app.ui.airplane = airplane;
        let v = view::build(app.snapshot.as_ref(), app.error.as_deref(), &app.ui);
        let network_keys = view::keys(&v);
        let mut entries = nav_entries(&app);
        let statuses = [&v.net_status, &v.net_items.wifi, &v.net_items.wired, &v.net_items.vpn, &v.net_items.hotspot];
        for (entry, status) in entries.iter_mut().zip(statuses) {
            entry.status = status.clone();
        }
        window.set_nav_groups(model(nav::groups(&entries, &app.nav_query, app.ui.page.id(), &app.nav_folded)));
        window.set_net_sub(v.net_hero.sub.as_str().into());
        window.set_count(v.count.as_str().into());
        window.set_net_hero(HeroInfo {
            icon: v.net_hero.icon.into(),
            name: v.net_hero.name.as_str().into(),
            meta: v.net_hero.meta.as_str().into(),
            tone: v.net_hero.tone.into(),
            detail: v.net_hero.detail.as_str().into(),
        });
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
        window.set_empty_command(empty.command.into());
        window.set_empty_action(empty.action.into());
        window.set_empty_page(empty.page);
        window.set_banner(v.banner.into());
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
        if let Some(confirm) = &app.confirm {
            window.set_confirm_title(confirm.title.as_str().into());
            window.set_confirm_text(confirm.text.as_str().into());
            window.set_confirm_button(confirm.button.as_str().into());
            window.set_confirm_destructive(confirm.destructive);
        }
        window.set_message_title(if app.message.is_some() { app.message_title.as_str().into() } else { "".into() });
        window.set_message_action(if app.retry.is_some() { "Try again".into() } else { "".into() });
        window.set_airplane(airplane);
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
        let state = &mut *app;
        input::render(window, &mut state.input, state.bt.snapshot.as_ref());
        shortcuts::render(window, &app.shortcuts);
        files::render(window, &app.files);
        // The keys that work on this page now, after the pages above worked out their lists.
        let mut hints = vec![Hint::new("Ctrl K", "Find a setting"), Hint::new("F6", "Sidebar or page")];
        hints.extend(match app.ui.page {
            view::Page::Wifi | view::Page::Wired | view::Page::Vpn => network_keys,
            view::Page::Bluetooth => bluetooth::keys(&app.bt),
            view::Page::Keyboard => input::keyboard_keys(&app.input),
            view::Page::Mouse => input::mouse_keys(&app.input),
            view::Page::Shortcuts => shortcuts::keys(&app.shortcuts),
            _ => Vec::new(),
        });
        window.set_hints(model(hints));
        render_quick(&mut app, airplane);
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
            let sign_ins = openconnect::SignIns::default();
            // The OpenConnect VPNs' settings paths, from the last snapshot.
            let mut oc_paths = Arc::new(std::collections::HashSet::new());
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
                                        oc_paths = Arc::new(openconnect::paths(snap));
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
                                let (client, weak, keyrings, sign_ins) = (client.clone(), weak.clone(), keyrings.clone(), sign_ins.clone());
                                let vpns = Vpns { sign_ins, openconnect: oc_paths.clone() };
                                tokio::spawn(async move {
                                    match run(&client, &keyrings, &vpns, &weak, &command).await {
                                        Ok(Some((reason, retry, target))) => failed(&weak, reason, retry, target),
                                        Ok(None) => {}
                                        Err(error) => report(&weak, &command, &error),
                                    }
                                });
                            }
                            Some(event) = async { agent.as_mut()?.recv().await } => {
                                let (keyrings, sign_ins, ui) = (keyrings.clone(), sign_ins.clone(), ui.clone());
                                tokio::spawn(async move { secrets::handle(event, &keyrings, &sign_ins, &ui).await });
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

    fn signing_in(&self, uuid: String, on: bool) {
        signing_in(&self.0, uuid, on);
    }

    fn notice(&self, text: String) {
        notice(&self.0, &text);
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

    fn reconnect(&self, connection_path: String) {
        let _ = self.0.upgrade_in_event_loop(move |window| {
            APP.with(|app| {
                let command = view::Command::Activate { connection: connection_path, device: None, access_point: None };
                send(&mut app.borrow_mut(), command);
            });
            render(&window);
        });
    }
}

/// Why a connection attempt failed, what "Try again" runs, and the row and name it was for when
/// that's known (otherwise it's the last attempt's).
type Failure = (String, Option<view::Command>, Option<(String, String)>);

/// Runs a command. For a connection attempt it waits for the outcome, and gives the reason it
/// failed and what "Try again" should run.
async fn run(
    client: &lituus_nm::Client,
    keyrings: &secrets::Keyrings,
    vpns: &Vpns,
    weak: &slint::Weak<MainWindow>,
    command: &view::Command,
) -> Result<Option<Failure>, lituus_nm::Error> {
    use view::Command::*;
    // For an OpenConnect VPN: its row and name, and whether another agent signs in.
    let (mut target, mut delegated) = (None, false);
    let (active, device, retry) = match command {
        OpenEditor { connection, new } => {
            if *new == Some(editor::Kind::OpenVpn) && !openvpn::plugin_installed() {
                notice(weak, OPENVPN_MISSING);
                return Ok(None);
            }
            if *new == Some(editor::Kind::OpenConnect) && openconnect::missing() {
                notice(weak, secrets::openconnect_missing());
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
            let (title, filters): (&str, &[portal::Filter]) = if field == "oc.csd" {
                ("Choose the host scan script", &[("Scripts", &["*.sh"]), ("All files", &["*"])])
            } else if field.ends_with("key") {
                ("Choose the key", &[("Keys", &["*.pem", "*.key", "*.p12", "*.pfx"]), ("All files", &["*"])])
            } else {
                (
                    "Choose the certificate",
                    &[("Certificates", &["*.pem", "*.crt", "*.cer", "*.der", "*.p12"]), ("All files", &["*"])],
                )
            };
            let path = portal::open_file(title, filters, None)
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
        SetAirplane { wifi, wwan, .. } => return client.set_radios(*wifi, *wwan).await.map(|()| None),
        Rescan { device } => return client.request_scan(device).await.map(|()| None),
        Deactivate { active } => return client.deactivate(active).await.map(|()| None),
        Forget { connection } => return client.forget(connection).await.map(|()| None),
        Tether { device, address, name } => return client.tether(device, address, name).await.map(|_| None),
        CancelSignIn { uuid } => {
            vpns.sign_ins.cancel(uuid);
            return Ok(None);
        }
        Activate { connection, device, access_point } => {
            let mut uuid = None;
            if vpns.openconnect.contains(connection) {
                let (id, name) = match sign_in_first(client, &vpns.sign_ins, weak, connection).await? {
                    First::Go { uuid, name } => (uuid, name),
                    First::Delegate { uuid, name } => {
                        delegated = true;
                        (uuid, name)
                    }
                    First::Stop => return Ok(None),
                    First::Failed { reason, uuid, name } => {
                        return Ok(Some((reason, Some(command.clone()), Some((format!("conn:{uuid}"), name)))));
                    }
                };
                target = Some((format!("conn:{id}"), name));
                uuid = Some(id);
            }
            let activated = client.activate(connection, device.as_deref(), access_point.as_deref()).await;
            // NetworkManager won't ask for the session of an attempt that didn't start.
            if let (Err(_), Some(uuid)) = (&activated, &uuid) {
                vpns.sign_ins.take_ready(uuid);
            }
            (activated?, device.clone(), command.clone())
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
        lituus_nm::Outcome::Failed(reason) => Some((reason, Some(retry), target)),
        lituus_nm::Outcome::NoSecrets if delegated => {
            notice(weak, secrets::openconnect_missing());
            None
        }
        lituus_nm::Outcome::Connected | lituus_nm::Outcome::Stopped | lituus_nm::Outcome::NoSecrets => None,
    })
}

/// What connecting a VPN needs besides NetworkManager.
struct Vpns {
    sign_ins: openconnect::SignIns,
    /// The OpenConnect VPNs' settings paths.
    openconnect: Arc<std::collections::HashSet<String>>,
}

/// What happens before NetworkManager is asked to connect an OpenConnect VPN.
enum First {
    Go {
        uuid: String,
        name: String,
    },
    /// Connect, and let another agent sign in (in a Flatpak, which can't open the window).
    Delegate {
        uuid: String,
        name: String,
    },
    /// Nothing more to do: the sign-in was cancelled, or is running already, or the notice says
    /// why.
    Stop,
    Failed {
        reason: String,
        uuid: String,
        name: String,
    },
}

/// An OpenConnect VPN signs in first, in the plugin's window, and the session waits for
/// NetworkManager's request: that way a slow MFA approval isn't cut off by NetworkManager, which
/// gives up on an agent after 120 seconds.
async fn sign_in_first(
    client: &lituus_nm::Client,
    sign_ins: &openconnect::SignIns,
    weak: &slint::Weak<MainWindow>,
    connection: &str,
) -> Result<First, lituus_nm::Error> {
    let settings = client.settings(connection).await?;
    let string = |key: &str| {
        settings.get("connection").and_then(|c| c.get(key)).and_then(|v| <&str>::try_from(&**v).ok()).unwrap_or("")
    };
    let (uuid, name) = (string("uuid").to_owned(), string("id").to_owned());
    // Signed in already: NetworkManager gave up on the agent while the window was open.
    if sign_ins.has_ready(&uuid) {
        return Ok(First::Go { uuid, name });
    }
    let Some(dialog) = openconnect::auth_dialog() else {
        if openconnect::in_flatpak() {
            return Ok(First::Delegate { uuid, name });
        }
        notice(weak, secrets::openconnect_missing());
        return Ok(First::Stop);
    };
    let Some(started) = sign_ins.start(&uuid, connection) else { return Ok(First::Stop) };
    // What NetworkManager kept from earlier sign-ins; asking may need polkit, and then there's none.
    let kept = client.secrets_raw(connection, "vpn").await.map(|s| openconnect::secrets(&s)).unwrap_or_default();
    signing_in(weak, uuid.clone(), true);
    let data = openconnect::data(&settings);
    let result = openconnect::sign_in(
        &dialog,
        openconnect::SignIn { uuid: &uuid, name: &name, data: &data, secrets: &kept, again: false },
        started.cancel,
    )
    .await;
    sign_ins.finish(started.id);
    signing_in(weak, uuid.clone(), false);
    Ok(match result {
        Ok(session) => {
            sign_ins.set_ready(&uuid, session);
            First::Go { uuid, name }
        }
        Err(openconnect::SignInError::Cancelled) => First::Stop,
        Err(openconnect::SignInError::Closed) => First::Failed { reason: openconnect::CLOSED.into(), uuid, name },
        Err(openconnect::SignInError::Failed(reason)) => First::Failed { reason, uuid, name },
    })
}

/// A connection's sign-in window opened or closed.
fn signing_in(weak: &slint::Weak<MainWindow>, uuid: String, on: bool) {
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let signing = &mut app.borrow_mut().signing_in;
            signing.retain(|u| *u != uuid);
            if on {
                signing.push(uuid);
            }
        });
        render(&window);
    });
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

/// The base of an imported VPN connection: its name, started by hand, and the user's own.
fn imported_settings(kind: editor::Kind, name: &str, owner: Option<&str>) -> lituus_nm::Settings {
    let mut settings = editor::new_settings(kind, &secrets::new_uuid());
    let connection = settings.entry("connection".into()).or_default();
    let mut put = |k: &str, v: zbus::zvariant::Value| {
        connection.insert(k.into(), zbus::zvariant::OwnedValue::try_from(v).expect("no fds"));
    };
    put("id", name.into());
    put("autoconnect", false.into());
    if let Some(owner) = owner {
        put("permissions", vec![format!("user:{owner}:")].into());
    }
    settings
}

/// Asks for a WireGuard or OpenVPN file, or openconnect-saml's config, and opens it in the editor
/// as a new connection.
async fn import_file(client: &lituus_nm::Client, weak: &slint::Weak<MainWindow>) {
    let filters: &[portal::Filter] =
        &[("WireGuard, OpenVPN and openconnect-saml", &["*.conf", "*.ovpn", "*.toml"]), ("All files", &["*"])];
    // With openconnect-saml set up, its (hidden) folder is the likely one.
    let saml = openconnect::saml_config();
    let folder = saml.as_deref().and_then(std::path::Path::parent);
    let path = match portal::open_file("Import a VPN connection", filters, folder).await {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(e) => return notice(weak, &format!("{e}.")),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => return notice(weak, &format!("Couldn't read {}: {e}.", path.display())),
    };
    let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    if openconnect::is_saml_config(&file, &text) {
        return import_saml(client, weak, &file, &text).await;
    }
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
        let mut settings = imported_settings(editor::Kind::OpenVpn, &imported.name, owner.as_deref());
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

/// openconnect-saml's profiles: one opens in the editor; several are added at once.
async fn import_saml(client: &lituus_nm::Client, weak: &slint::Weak<MainWindow>, file: &str, text: &str) {
    if openconnect::missing() {
        return notice(weak, secrets::openconnect_missing());
    }
    let profiles = match openconnect::import_saml(text) {
        Ok(profiles) => profiles,
        Err(e) => return notice(weak, &format!("{file} isn't an openconnect-saml file Lituus can read: {e}")),
    };
    let owner = secrets::username();
    let settings_of = |name: &str, form: &openconnect::OcForm| {
        let mut settings = imported_settings(editor::Kind::OpenConnect, name, owner.as_deref());
        openconnect::write(&mut settings, None, form);
        settings
    };
    if let [(name, form)] = profiles.as_slice() {
        let settings = settings_of(name, form);
        let banner = format!("Imported from openconnect-saml ({file})");
        let zones = client.firewall_zones().await;
        let _ = weak.upgrade_in_event_loop(move |window| {
            APP.with(|app| {
                let extras = editing::Extras { wg_key: None, banner };
                editing::open(&mut app.borrow_mut(), None, settings, zones, None, extras)
            });
            render(&window);
        });
        return;
    }
    let (mut added, mut failed) = (Vec::new(), Vec::new());
    for (name, form) in &profiles {
        match client.add(&settings_of(name, form)).await {
            Ok(_) => added.push(name.clone()),
            Err(e) => failed.push(format!("{name} ({e})")),
        }
    }
    let mut text = if added.is_empty() {
        "Couldn't add the AnyConnect connections from openconnect-saml.".to_owned()
    } else {
        format!("Added {} AnyConnect connections from openconnect-saml: {}.", added.len(), added.join(", "))
    };
    if !failed.is_empty() {
        text += &format!(" Couldn't add {}.", failed.join(", "));
    }
    notice(weak, &text);
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
fn failed(
    weak: &slint::Weak<MainWindow>,
    reason: String,
    retry: Option<view::Command>,
    target: Option<(String, String)>,
) {
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            let name = mark_failed(&mut app, reason.clone(), retry, target);
            if app.settings.notifications
                && !window.window().is_visible()
                && let Some(notifier) = &app.notifier
            {
                notifier.send(notify::failure(&name, &reason));
            }
        });
        render(&window);
    });
}

/// Shows a failed attempt above the list and on its row: `target`'s, or else the last
/// attempt's (a sign-in can take minutes, and another attempt may have started meanwhile). The
/// connection's name.
fn mark_failed(
    app: &mut App,
    reason: String,
    retry: Option<view::Command>,
    target: Option<(String, String)>,
) -> String {
    let (row, name) = target.or_else(|| app.attempt.clone()).unwrap_or_default();
    let what = if reason == openconnect::CLOSED { "Couldn't sign in" } else { "Couldn't connect" };
    app.message_title = if name.is_empty() { what.to_owned() } else { format!("{what} to {name}") };
    app.message = Some((reason.clone(), "danger"));
    app.retry = retry;
    app.ui.failed = (!row.is_empty()).then_some((row, reason));
    name
}

/// Shows why a command failed, above the list.
fn report(weak: &slint::Weak<MainWindow>, command: &view::Command, error: &lituus_nm::Error) {
    use view::Command::*;
    let what = match command {
        SetWifi(true) => "Couldn't turn Wi-Fi on",
        SetWifi(false) => "Couldn't turn Wi-Fi off",
        SetAirplane { on: true, .. } => "Couldn't turn airplane mode on",
        SetAirplane { on: false, .. } => "Couldn't turn airplane mode off",
        Rescan { .. } => "Couldn't scan",
        Activate { .. } | ConnectOpen { .. } | Join { .. } => "Couldn't connect",
        CancelSignIn { .. } => "Couldn't close the sign-in window",
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
    let airplane_failed = matches!(command, SetAirplane { on: true, .. });
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            if airplane_failed {
                app.airplane = None;
            }
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
mod dialog_tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    use i_slint_backend_testing::mock_elapsed_time;
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};

    use super::{DisplaysView, FilesView, MainWindow};

    /// A window on the test backend whose dialog callbacks write down what they were called
    /// with, and close the dialog as the app does.
    fn dialogs() -> (MainWindow, Rc<RefCell<Vec<String>>>) {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(1200.0, 760.0));
        window.show().unwrap();
        let log = Rc::new(RefCell::new(Vec::new()));
        let (l, w) = (log.clone(), window.as_weak());
        let note = move |what: &str| {
            let (log, w) = (l.clone(), w.clone());
            let what = what.to_owned();
            move || {
                log.borrow_mut().push(what.clone());
                let w = w.unwrap();
                match what.as_str() {
                    "hostname-cancelled" | "hostname-save" => w.set_hostname_open(false),
                    "add-cancelled" => w.set_add_open(false),
                    "share-closed" => w.set_share_open(false),
                    "join-cancelled" | "join-accepted" => w.set_join_open(false),
                    "confirm-cancelled" => w.set_confirm_open(false),
                    _ => {}
                }
            }
        };
        window.on_hostname_cancelled(note("hostname-cancelled"));
        window.on_hostname_save(note("hostname-save"));
        window.on_add_cancelled(note("add-cancelled"));
        window.on_share_closed(note("share-closed"));
        window.on_join_cancelled(note("join-cancelled"));
        window.on_join_accepted(note("join-accepted"));
        window.on_confirm_cancelled(note("confirm-cancelled"));
        window.on_join_check(|_, _, _, password| !password.is_empty());
        let w = window.as_weak();
        let l = log.clone();
        window.on_bt_act(move |verb, _| {
            l.borrow_mut().push(format!("bt {verb}"));
            if verb == "rename-cancel" || verb == "rename-save" {
                w.unwrap().set_bt_rename_open(false);
            }
        });
        let w = window.as_weak();
        let l = log.clone();
        window.on_displays_act(move |verb, _, _| {
            l.borrow_mut().push(format!("displays {verb}"));
            let w = w.unwrap();
            w.set_displays(DisplaysView { keep_open: false, config_open: false, ..w.get_displays() });
        });
        let w = window.as_weak();
        let l = log.clone();
        window.on_files_act(move |verb| {
            l.borrow_mut().push(format!("files {verb}"));
            w.unwrap().set_files(FilesView::default());
        });
        let l = log.clone();
        window.on_move_selection(move |by| l.borrow_mut().push(format!("move {by}")));
        (window, log)
    }

    /// What the password dialog shows under its fields, for one kind of security.
    fn join_hints(window: &MainWindow) {
        window.set_join_password_hints([SharedString::new()].as_slice().into());
        window
            .set_join_storage_hints([SharedString::new(), SharedString::new(), SharedString::new()].as_slice().into());
    }

    fn press(window: &MainWindow, key: impl Into<SharedString>) {
        let key = key.into();
        window.window().dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
        window.window().dispatch_event(WindowEvent::KeyReleased { text: key });
    }

    /// A click in the window's top left corner, beside any dialog.
    fn click_beside(window: &MainWindow) {
        let position = LogicalPosition::new(8.0, 8.0);
        window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    }

    /// Opens each dialog, types into its field if it has one, and presses Esc.
    #[test]
    fn escape_closes_every_dialog_even_from_a_text_field() {
        let (window, log) = dialogs();
        let esc = |open: &dyn Fn(), expected: &str| {
            open();
            press(&window, Key::Escape);
            assert_eq!(log.borrow_mut().drain(..).collect::<Vec<_>>(), [expected]);
        };

        esc(
            &|| {
                window.set_hostname_input("thinkpad".into());
                window.set_hostname_open(true);
                // The field has the keyboard, with its text selected.
                press(&window, "x");
                assert_eq!(window.get_hostname_input(), "x");
            },
            "hostname-cancelled",
        );
        esc(
            &|| {
                window.set_join_mode("new".into());
                join_hints(&window);
                window.set_join_open(true);
                press(&window, "x");
                assert_eq!(window.get_join_password(), "x");
            },
            "join-cancelled",
        );
        esc(
            &|| {
                window.set_bt_rename_open(true);
                press(&window, "x");
                assert_eq!(window.get_bt_rename_name(), "x");
            },
            "bt rename-cancel",
        );
        esc(&|| window.set_add_open(true), "add-cancelled");
        esc(&|| window.set_confirm_open(true), "confirm-cancelled");
        esc(&|| window.set_share_open(true), "share-closed");
        esc(
            &|| {
                window.set_page("displays".into());
                window.set_displays(DisplaysView { config_open: true, ..Default::default() });
            },
            "displays close-config",
        );
        // After Apply, Esc goes back to the old settings.
        esc(&|| window.set_displays(DisplaysView { keep_open: true, ..Default::default() }), "displays revert-now");
        esc(&|| window.set_files(FilesView { incoming: true, ..Default::default() }), "files decline");
        esc(&|| window.set_files(FilesView { sending: true, ..Default::default() }), "files hide");
    }

    /// A click beside a dialog closes it only when nothing typed in it would be lost.
    #[test]
    fn a_click_beside_a_dialog_never_loses_what_was_typed() {
        let (window, log) = dialogs();
        let taken = || log.borrow_mut().drain(..).collect::<Vec<_>>();

        window.set_add_open(true);
        click_beside(&window);
        assert_eq!(taken(), ["add-cancelled"]);
        window.set_share_open(true);
        click_beside(&window);
        assert_eq!(taken(), ["share-closed"]);
        window.set_confirm_open(true);
        click_beside(&window);
        assert_eq!(taken(), ["confirm-cancelled"]);
        window.set_page("displays".into());
        window.set_displays(DisplaysView { config_open: true, ..Default::default() });
        click_beside(&window);
        assert_eq!(taken(), ["displays close-config"]);

        join_hints(&window);
        window.set_join_open(true);
        press(&window, "x");
        click_beside(&window);
        assert!(window.get_join_open());
        assert_eq!(window.get_join_password(), "x");
        window.set_join_open(false);

        window.set_hostname_open(true);
        click_beside(&window);
        assert!(window.get_hostname_open());
        window.set_hostname_open(false);

        window.set_bt_rename_open(true);
        click_beside(&window);
        assert!(window.get_bt_rename_open());
        window.set_bt_rename_open(false);
        assert_eq!(taken(), Vec::<String>::new());
    }

    /// Enter in the field of a one-field dialog accepts it.
    #[test]
    fn enter_accepts_a_dialog_with_one_field() {
        let (window, log) = dialogs();
        window.set_hostname_open(true);
        press(&window, "a");
        press(&window, Key::Return);
        assert_eq!(log.borrow_mut().drain(..).collect::<Vec<_>>(), ["hostname-save"]);

        join_hints(&window);
        window.set_join_open(true);
        // Not with an empty password.
        press(&window, Key::Return);
        assert!(window.get_join_open());
        press(&window, "x");
        press(&window, Key::Return);
        assert_eq!(log.borrow_mut().drain(..).collect::<Vec<_>>(), ["join-accepted"]);
    }

    /// A dialog takes the keyboard from the page while it's open, and gives it back when it
    /// closes, even when it was closed from Rust rather than by its own buttons.
    #[test]
    fn the_page_gets_the_keyboard_back_after_a_dialog() {
        let (window, log) = dialogs();
        press(&window, Key::DownArrow);
        assert_eq!(log.borrow_mut().drain(..).collect::<Vec<_>>(), ["move 1"]);

        window.set_page("displays".into());
        window.set_displays(DisplaysView { keep_open: true, ..Default::default() });
        press(&window, Key::DownArrow);
        assert!(log.borrow().is_empty());
        // The time runs out.
        window.set_displays(DisplaysView::default());
        mock_elapsed_time(Duration::from_millis(20));
        press(&window, Key::DownArrow);
        assert_eq!(log.borrow_mut().drain(..).collect::<Vec<_>>(), ["move 1"]);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use std::cell::RefCell;
    use std::rc::Rc;

    use i_slint_backend_testing::{AccessibleRole, ElementHandle, mock_elapsed_time};
    use slint::platform::{Key, PointerEventButton, WindowEvent};
    use slint::{ComponentHandle, LogicalPosition, LogicalSize, Model, SharedString};

    use super::{App, BtPage, MainWindow, act, mark_failed, openconnect, view};

    /// An app with a saved AnyConnect VPN, selected, whose commands go to the receiver.
    fn with_a_vpn() -> (App, tokio::sync::mpsc::UnboundedReceiver<view::Command>) {
        let mut snap = view::tests::home();
        let kind = lituus_nm::ConnectionKind::Vpn {
            service: openconnect::SERVICE.into(),
            gateway: "vpn.uzh.ch".into(),
            protocol: "anyconnect".into(),
        };
        snap.connections.push(view::tests::saved("u-uzh", "UZH VPN", kind, "", None));
        let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App { snapshot: Some(snap), commands: Some(commands), ..App::default() };
        app.ui = view::UiState { page: view::Page::Vpn, selected: Some("conn:u-uzh".into()), ..Default::default() };
        (app, receiver)
    }

    /// Enter on a VPN whose sign-in window is open (or its switch in the popup) doesn't open a
    /// second window.
    #[test]
    fn connecting_while_signing_in_does_nothing() {
        let (mut app, mut commands) = with_a_vpn();
        app.signing_in = vec!["u-uzh".into()];
        act(&mut app, "connect");
        assert!(commands.try_recv().is_err());
        act(&mut app, "cancel-sign-in");
        assert!(matches!(commands.try_recv(), Ok(view::Command::CancelSignIn { uuid }) if uuid == "u-uzh"));
        app.signing_in.clear();
        act(&mut app, "connect");
        assert!(matches!(commands.try_recv(), Ok(view::Command::Activate { .. })));
    }

    /// A sign-in that fails after minutes is reported against its VPN, not the network joined
    /// meanwhile.
    #[test]
    fn a_failed_sign_in_marks_its_own_row() {
        let (mut app, _commands) = with_a_vpn();
        app.attempt = Some(("wifi:Aster Café".into(), "Aster Café".into()));
        let target = Some(("conn:u-uzh".to_owned(), "UZH VPN".to_owned()));
        assert_eq!(mark_failed(&mut app, openconnect::CLOSED.into(), None, target), "UZH VPN");
        assert_eq!(app.message_title, "Couldn't sign in to UZH VPN");
        assert_eq!(app.ui.failed, Some(("conn:u-uzh".into(), openconnect::CLOSED.into())));
        // Without one, it's the last attempt's.
        mark_failed(&mut app, "The network rejected the password.".into(), None, None);
        assert_eq!(app.message_title, "Couldn't connect to Aster Café");
        assert_eq!(app.ui.failed.unwrap().0, "wifi:Aster Café");
    }

    fn press(window: &MainWindow, text: impl Into<SharedString>) {
        let text = text.into();
        window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window.window().dispatch_event(WindowEvent::KeyReleased { text });
        // The Tab a dialog presses again to send the focus round (connect_focus).
        mock_elapsed_time(Duration::ZERO);
    }

    /// The window with the sidebar filled in, as the app shows it.
    fn window() -> MainWindow {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        super::connect_focus(&window);
        let entries = super::nav_entries(&super::App::default());
        window.set_nav_groups(super::model(super::nav::groups(&entries, "", "wifi", &[])));
        window.set_hostname("thinkpad-x1".into());
        window.window().set_size(LogicalSize::new(1200.0, 760.0));
        window.show().unwrap();
        window
    }

    /// Whether the focus ring is drawn, and around an element with this accessible label (a
    /// text with the same words is one too).
    fn ring_on(window: &MainWindow, label: &str) -> bool {
        let Some(ring) = ElementHandle::find_by_element_id(window, "Focusable::ring").next() else { return false };
        let (r, rs) = (ring.absolute_position(), ring.size());
        ElementHandle::find_by_accessible_label(window, label).any(|e| {
            let (p, s) = (e.absolute_position(), e.size());
            let (cx, cy) = (p.x + s.width / 2.0, p.y + s.height / 2.0);
            r.x <= cx && cx <= r.x + rs.width && r.y <= cy && cy <= r.y + rs.height && rs.height <= s.height + 8.0
        })
    }

    /// Presses Tab until the ring is on `label`; how many presses that took.
    fn tab_to(window: &MainWindow, label: &str) -> usize {
        for n in 1..=120 {
            press(window, Key::Tab);
            if ring_on(window, label) {
                return n;
            }
        }
        panic!("Tab never reached {label}");
    }

    /// Presses ↓ (or ↑) until the ring is on `label`; how many presses that took.
    fn arrow_to(window: &MainWindow, key: Key, label: &str) -> usize {
        for n in 1..=30 {
            press(window, key);
            if ring_on(window, label) {
                return n;
            }
        }
        panic!("the arrow keys never reached {label}");
    }

    /// The sidebar's label for a page ("Bluetooth, 2 connected"), as its button reads it out.
    fn nav_label(window: &MainWindow, id: &str) -> String {
        let groups = window.get_nav_groups();
        let item = groups.iter().flat_map(|g| g.items.iter().collect::<Vec<_>>()).find(|i| i.id == id).unwrap();
        if item.sub.is_empty() { item.label.to_string() } else { format!("{}, {}", item.label, item.spoken) }
    }

    /// The sidebar's pages are one Tab stop, the current page; Settings, Airplane mode and the
    /// hostname come next, then the page. Shift+Tab comes back; ↑↓ move among the pages and Enter
    /// opens one; Space and Enter work the control with the ring, which is drawn only once the
    /// keyboard moved the focus.
    #[test]
    fn the_keyboard_reaches_the_sidebar_and_works_its_controls() {
        let window = window();
        let navigated = Rc::new(RefCell::new(Vec::new()));
        let n = navigated.clone();
        window.on_navigate(move |id| n.borrow_mut().push(id.to_string()));
        let airplane = Rc::new(RefCell::new(0));
        let a = airplane.clone();
        window.on_airplane_toggled(move || *a.borrow_mut() += 1);
        assert!(ElementHandle::find_by_element_id(&window, "Focusable::ring").next().is_none());

        // After the search field, the current page, and none of the others.
        assert_eq!(tab_to(&window, &nav_label(&window, "wifi")), 2);
        assert_eq!(tab_to(&window, "Settings"), 1);
        assert_eq!(tab_to(&window, "Airplane mode"), 1);
        assert_eq!(tab_to(&window, "Change the hostname"), 1);
        // After the sidebar, the page: the hero's button.
        assert_eq!(tab_to(&window, "Add a connection"), 1);

        press(&window, Key::Backtab);
        press(&window, Key::Backtab);
        assert!(ring_on(&window, "Airplane mode"));
        press(&window, " ");
        assert_eq!(*airplane.borrow(), 1);
        // The switch keeps the focus, so Tab carries on from it.
        assert!(ring_on(&window, "Airplane mode"));
        press(&window, Key::Backtab);
        press(&window, Key::Return);
        assert_eq!(*navigated.borrow(), ["settings"]);

        // Back to the pages, and down them with the arrow keys: Enter opens the one with the ring.
        press(&window, Key::Backtab);
        assert!(ring_on(&window, &nav_label(&window, "wifi")));
        assert_eq!(arrow_to(&window, Key::DownArrow, &nav_label(&window, "vpn")), 2);
        press(&window, Key::UpArrow);
        assert!(ring_on(&window, &nav_label(&window, "wired")));
        press(&window, Key::Return);
        assert_eq!(*navigated.borrow(), ["settings", "wired"]);
        // The ends: ↑ on the first page stays there.
        arrow_to(&window, Key::UpArrow, &nav_label(&window, "network"));
        press(&window, Key::UpArrow);
        assert!(ring_on(&window, &nav_label(&window, "network")));
    }

    /// F6 goes from the page to the sidebar's current page, and from anywhere in the sidebar to
    /// the page's first control.
    #[test]
    fn f6_moves_between_the_sidebar_and_the_page() {
        let window = window();
        press(&window, Key::F6);
        assert!(ring_on(&window, &nav_label(&window, "wifi")));
        press(&window, Key::F6);
        assert!(ring_on(&window, "Add a connection"));
        press(&window, Key::F6);
        assert!(ring_on(&window, &nav_label(&window, "wifi")));
        // From the search field and the footer too.
        tab_to(&window, "Airplane mode");
        press(&window, Key::F6);
        assert!(ring_on(&window, "Add a connection"));
        press(&window, Key::F6);
        // The search field, just before the current page.
        press(&window, Key::Backtab);
        assert!(ElementHandle::find_by_element_id(&window, "Focusable::ring").next().is_none());
        press(&window, Key::F6);
        assert!(ring_on(&window, "Add a connection"));
        // Not out of a dialog.
        window.set_confirm_title("Forget Aster Café?".into());
        window.set_confirm_open(true);
        tab_to(&window, "Cancel");
        press(&window, Key::F6);
        assert!(ring_on(&window, "Cancel"));
    }

    /// Rust sends the sidebar again after every change, and Slint rebuilds its buttons: the
    /// focus moves to the rebuilt one instead of getting lost.
    #[test]
    fn a_rebuilt_sidebar_keeps_the_focus() {
        let window = window();
        let label = nav_label(&window, "bluetooth");
        tab_to(&window, &nav_label(&window, "wifi"));
        arrow_to(&window, Key::DownArrow, &label);
        let entries = super::nav_entries(&super::App::default());
        window.set_nav_groups(super::model(super::nav::groups(&entries, "", "wifi", &[])));
        assert!(ring_on(&window, &label));
        // And keeps it: the window doesn't take it for a control that's gone.
        for _ in 0..5 {
            mock_elapsed_time(std::time::Duration::from_millis(100));
        }
        assert!(ring_on(&window, &label));
    }

    /// As in a tree: ← on Network folds it, → unfolds it or, unfolded, goes to its first page,
    /// and ← on one of its pages goes back to it. The chevron is for the pointer, no Tab stop;
    /// the folded sidebar, rebuilt, keeps the focus on Network.
    #[test]
    fn the_keyboard_folds_network() {
        let window = window();
        let folds = Rc::new(RefCell::new(0));
        let f = folds.clone();
        window.on_nav_fold(move |id| {
            assert_eq!(id, "network");
            *f.borrow_mut() += 1;
        });
        let network = nav_label(&window, "network");
        tab_to(&window, &nav_label(&window, "wifi"));
        press(&window, Key::LeftArrow);
        assert!(ring_on(&window, &network));
        assert_eq!(*folds.borrow(), 0);
        press(&window, Key::LeftArrow);
        assert_eq!(*folds.borrow(), 1);
        // Still unfolded here (the app would fold it): → goes to its first page.
        press(&window, Key::RightArrow);
        assert!(ring_on(&window, &nav_label(&window, "wifi")));
        press(&window, Key::Tab);
        assert!(!ring_on(&window, "Network pages, expanded"));
        press(&window, Key::Backtab);
        press(&window, Key::LeftArrow);
        let entries = super::nav_entries(&super::App::default());
        window.set_nav_groups(super::model(super::nav::groups(&entries, "", "wifi", &["network".to_owned()])));
        assert!(ring_on(&window, &nav_label(&window, "network")));
        // Folded: → unfolds.
        press(&window, Key::RightArrow);
        assert_eq!(*folds.borrow(), 2);
    }

    #[test]
    fn the_arrow_keys_move_a_segmented_control() {
        let window = window();
        window.set_page("hotspot".into());
        window.set_hotspot_available(true);
        tab_to(&window, "Band");
        press(&window, Key::RightArrow);
        assert_eq!(window.get_hotspot_band(), 1);
        press(&window, Key::RightArrow);
        press(&window, Key::RightArrow);
        assert_eq!(window.get_hotspot_band(), 2);
        press(&window, Key::LeftArrow);
        assert_eq!(window.get_hotspot_band(), 1);
    }

    #[test]
    fn a_select_opens_with_space_and_moves_with_the_arrows() {
        let window = window();
        window.set_join_mode("hidden".into());
        window.set_join_securities(super::model(vec!["None".into(), "WPA2 Personal".into(), "WPA3 Personal".into()]));
        window.set_join_password_hints(super::model(vec![SharedString::new(); 3]));
        window.set_join_storage_hints(super::model(vec![SharedString::new(); 3]));
        window.set_join_open(true);
        tab_to(&window, "Security");
        press(&window, " ");
        press(&window, Key::DownArrow);
        press(&window, Key::DownArrow);
        press(&window, Key::Return);
        assert_eq!(window.get_join_security(), 2);
        // Esc closes the list and keeps the choice.
        press(&window, Key::Return);
        press(&window, Key::UpArrow);
        press(&window, Key::Escape);
        assert_eq!(window.get_join_security(), 2);
        assert!(window.get_join_open());
    }

    /// The single-letter keys work wherever the focus is on the page, but not while typing.
    #[test]
    fn shortcuts_wait_while_a_text_field_has_the_focus() {
        let window = window();
        window.set_page("bluetooth".into());
        let keys = Rc::new(RefCell::new(Vec::new()));
        let k = keys.clone();
        window.on_bt_act(move |verb, arg| k.borrow_mut().push(format!("{verb} {arg}")));
        press(&window, "s");
        assert_eq!(*keys.borrow(), ["key s"]);
        // On a control.
        tab_to(&window, "Airplane mode");
        press(&window, "p");
        assert_eq!(*keys.borrow(), ["key s", "key p"]);
        // Into the sidebar's search field: typed, not a shortcut.
        for _ in 0..30 {
            press(&window, Key::Backtab);
            if ElementHandle::find_by_element_id(&window, "Focusable::ring").next().is_none() {
                break;
            }
        }
        press(&window, "s");
        press(&window, "p");
        assert_eq!(*keys.borrow(), ["key s", "key p"]);
        press(&window, Key::Escape);
        press(&window, "s");
        assert_eq!(*keys.borrow(), ["key s", "key p", "key s"]);
    }

    /// A dialog's button closes the dialog and goes with it: the window takes the focus back,
    /// so its keys work again.
    #[test]
    fn the_window_takes_the_focus_when_the_focused_control_goes() {
        let window = window();
        window.set_page("bluetooth".into());
        let keys = Rc::new(RefCell::new(Vec::new()));
        let k = keys.clone();
        window.on_bt_act(move |verb, arg| k.borrow_mut().push(format!("{verb} {arg}")));
        let weak = window.as_weak();
        window.on_confirm_cancelled(move || weak.unwrap().set_confirm_open(false));
        window.set_confirm_title("Forget Aster Café?".into());
        window.set_confirm_open(true);
        tab_to(&window, "Cancel");
        press(&window, " ");
        assert!(!window.get_confirm_open());
        // Drawing the window removes the dialog, as the next frame does.
        assert!(ElementHandle::find_by_accessible_label(&window, "Forget Aster Café?").next().is_none());
        for _ in 0..5 {
            mock_elapsed_time(std::time::Duration::from_millis(100));
        }
        press(&window, "s");
        assert_eq!(*keys.borrow(), ["key s"]);
    }

    /// The same in a `Modal` (#41's dialogs), which takes the focus when it opens: round its
    /// field and buttons, and back to the control that opened it after Esc.
    #[test]
    fn tab_stays_in_a_modal_and_the_focus_comes_back() {
        let window = window();
        let weak = window.as_weak();
        window.on_hostname_cancelled(move || weak.unwrap().set_hostname_open(false));
        tab_to(&window, "Change the hostname");
        window.set_hostname_input("thinkpad-x1".into());
        window.set_hostname_open(true);
        mock_elapsed_time(Duration::ZERO);
        // The field has the focus; Tab goes to the buttons and round.
        press(&window, Key::Tab);
        assert!(ring_on(&window, "Cancel"));
        press(&window, Key::Tab);
        assert!(ring_on(&window, "Save"));
        press(&window, Key::Tab);
        press(&window, Key::Tab);
        assert!(ring_on(&window, "Cancel"));
        // Back past the field, round to the last button.
        press(&window, Key::Backtab);
        press(&window, Key::Backtab);
        assert!(ring_on(&window, "Save"));
        press(&window, Key::Escape);
        assert!(!window.get_hostname_open());
        mock_elapsed_time(Duration::ZERO);
        assert!(ring_on(&window, "Change the hostname"));
    }

    /// Tab and Shift+Tab go round a dialog's controls instead of on to the page behind it, and
    /// when the dialog closes, the control that had the focus before gets it back.
    #[test]
    fn tab_stays_in_a_dialog_and_the_focus_comes_back() {
        let window = window();
        window.set_page("bluetooth".into());
        tab_to(&window, "Airplane mode");
        window.set_confirm_title("Forget Aster Café?".into());
        window.set_confirm_button("Forget".into());
        window.set_confirm_open(true);
        tab_to(&window, "Cancel");
        press(&window, Key::Tab);
        assert!(ring_on(&window, "Forget"));
        // Past the last button: round to the first, not to the sidebar.
        press(&window, Key::Tab);
        assert!(ring_on(&window, "Cancel"));
        press(&window, Key::Backtab);
        assert!(ring_on(&window, "Forget"));
        press(&window, Key::Backtab);
        assert!(ring_on(&window, "Cancel"));
        for _ in 0..6 {
            press(&window, Key::Tab);
            assert!(ring_on(&window, "Cancel") || ring_on(&window, "Forget"));
        }
        window.set_confirm_open(false);
        // The next turn of the event loop.
        mock_elapsed_time(Duration::ZERO);
        assert!(ring_on(&window, "Airplane mode"));
        // And it stays there: the window doesn't take it for the button that went.
        for _ in 0..5 {
            mock_elapsed_time(Duration::from_millis(100));
        }
        assert!(ring_on(&window, "Airplane mode"));
    }

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
            || ElementHandle::find_by_element_id(&window, "Editor::sheet").next().map(|e| e.absolute_position().x);
        assert_eq!(editor(), None);

        window.window().set_size(LogicalSize::new(1600.0, 700.0));
        for _ in 0..5 {
            assert_eq!(editor(), None);
            mock_elapsed_time(Duration::from_millis(50));
        }

        // Opening it still slides it in, to the page under the hero.
        window.set_editor_open(true);
        mock_elapsed_time(Duration::from_millis(50));
        let page = ElementHandle::find_by_element_id(&window, "MainWindow::page-scroll").next().expect("the page");
        let x = editor().expect("the editor is shown");
        assert!(page.absolute_position().x < x && x < 1600.0, "{x}");
        mock_elapsed_time(Duration::from_millis(500));
        assert_eq!(editor(), Some(page.absolute_position().x));
    }

    /// The editor covers the card it was opened from: the details card for Edit, if the editor
    /// fits in it, and otherwise (or from the hero) the page under the hero. Clicks beside it
    /// go nowhere.
    #[test]
    fn the_editor_covers_the_card_it_was_opened_from() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        let selected = Rc::new(RefCell::new(Vec::new()));
        let s = selected.clone();
        window.on_select(move |id| s.borrow_mut().push(id.to_string()));
        window.set_sections(super::model(vec![super::Section {
            title: "ENP3S0".into(),
            rows: super::model(wifi_rows(3, 0)),
        }]));
        window.set_has_details(true);
        let edit = super::Action { id: "edit".into(), label: "Edit".into(), kind: "secondary".into(), enabled: true };
        window.set_details(super::Details {
            title: "Wired connection 1".into(),
            actions: super::model(vec![edit]),
            ..Default::default()
        });
        window.window().set_size(LogicalSize::new(2000.0, 1070.0));
        window.show().unwrap();
        mock_elapsed_time(Duration::ZERO);
        let bounds = |e: ElementHandle| {
            let (at, size) = (e.absolute_position(), e.size());
            [at.x, at.y, size.width, size.height].map(f32::round)
        };
        let find = |id: &str| ElementHandle::find_by_element_id(&window, id).next();
        let sheet = || find("Editor::sheet").map(bounds);
        let page = || find("MainWindow::page-scroll").map(bounds).expect("the page");
        let details = || ElementHandle::find_by_element_type_name(&window, "DetailsPane").next().map(bounds);
        let press = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
                .expect(label)
                .invoke_accessible_default_action()
        };
        let open = |open: bool| {
            window.set_editor_open(open);
            mock_elapsed_time(Duration::from_millis(500));
        };

        // Edit in the owner's window: over the details card, with the list beside it.
        press("Edit");
        open(true);
        assert_eq!(sheet(), details());
        let list = find("MainWindow::list-card").map(bounds).expect("the list");
        assert!(list[0] + list[2] < sheet().unwrap()[0], "{list:?} {:?}", sheet());
        // A click on the list beside it selects nothing.
        let position = LogicalPosition::new(list[0] + 60.0, list[1] + 40.0);
        window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        assert!(selected.borrow().is_empty(), "{:?}", selected.borrow());

        // A smaller window: the details card is narrower than the editor's form, so the editor
        // covers the list and the details.
        window.window().set_size(LogicalSize::new(1200.0, 760.0));
        mock_elapsed_time(Duration::from_millis(50));
        assert!(details().unwrap()[2] < 736.0);
        assert_eq!(sheet(), Some(page()));
        open(false);
        assert_eq!(sheet(), None);

        // From the hero, even in a wide window: the page under the hero.
        window.window().set_size(LogicalSize::new(2000.0, 1070.0));
        press("Add a connection");
        open(true);
        assert_eq!(sheet(), Some(page()));
        assert!(page()[1] > 0.0 && page()[2] > details().unwrap()[2]);
        open(false);

        // It takes the keyboard as it opens, before it has slid in: Esc closes it.
        let backs = Rc::new(RefCell::new(0));
        let b = backs.clone();
        window.on_editor_back(move || *b.borrow_mut() += 1);
        window.set_editor_open(true);
        mock_elapsed_time(Duration::ZERO);
        mock_elapsed_time(Duration::from_millis(300));
        window.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Escape.into() });
        window.window().dispatch_event(WindowEvent::KeyReleased { text: Key::Escape.into() });
        assert_eq!(*backs.borrow(), 1);
    }

    /// A wide window's width goes to the list, also when it shows an empty state (Wi-Fi off) and
    /// there are no details beside it.
    #[test]
    fn the_network_list_fills_a_wide_window_when_empty() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.set_empty_icon("wifi-off".into());
        window.set_empty_title("Wi-Fi is off".into());
        window.set_empty_text("Wired and VPN connections keep working.".into());
        window.set_empty_action("Turn on Wi-Fi".into());
        window.set_has_details(false);
        window.window().set_size(LogicalSize::new(2000.0, 1070.0));
        window.show().unwrap();
        let list =
            ElementHandle::find_by_element_id(&window, "MainWindow::list-card").next().expect("the list is shown");
        // The window less the sidebar (250) and the page's padding (44).
        assert!(list.size().width > 1600.0, "{}", list.size().width);
    }

    /// Sound chooses the output and the input from a list each, with no button per device; in
    /// the owner's window the volume slider and the lists keep their own widths.
    #[test]
    fn sound_devices_are_chosen_from_a_list() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.set_page("sound".into());
        let names = |n: &[&str]| super::model(n.iter().map(|&n| SharedString::from(n)).collect());
        window.set_sound(super::SoundView {
            state: "list".into(),
            outputs_names: names(&["Speakers", "Samsung TV (HDMI)", "Sony WH-1000XM5"]),
            outputs_details: names(&["Built in · Speakers", "TV or monitor", "Bluetooth"]),
            output: 1,
            inputs_names: names(&["Blue Yeti"]),
            inputs_details: names(&["USB"]),
            input: 0,
            out_volume: 0.5,
            has_balance: true,
            max: 1.0,
            ..super::SoundView::default()
        });
        window.window().set_size(LogicalSize::new(2000.0, 1070.0));
        window.show().unwrap();
        // The control, not its label's text.
        let control = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .find(|e| e.accessible_role() != Some(AccessibleRole::Text))
                .expect(label)
        };
        let width = |label: &str| control(label).size().width;
        assert!(width("Output device") <= 240.0, "{}", width("Output device"));
        assert!(width("Input device") <= 240.0, "{}", width("Input device"));
        assert!(width("Output volume") <= 640.0, "{}", width("Output volume"));
        assert!(ElementHandle::find_by_accessible_label(&window, "Use").next().is_none());
        assert_eq!(control("Output device").accessible_role(), Some(AccessibleRole::Combobox));
        assert_eq!(control("Output device").accessible_value().as_deref(), Some("Samsung TV (HDMI)"));

        // With no card left, the Cards tab goes and the page shows the devices.
        window.set_sound_tab(2);
        super::sound::render(&window, &super::sound::State::default());
        assert_eq!(window.get_sound_tab(), 0);
    }

    /// With Network's sub-items open, the sidebar's pages scroll in the smallest window instead
    /// of pushing Settings and Airplane mode out of it.
    #[test]
    fn the_sidebar_keeps_its_footer_in_a_short_window() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        let entries = super::nav_entries(&super::App::default());
        window.set_nav_groups(super::model(super::nav::groups(&entries, "", "wifi", &[])));
        window.set_hostname("thinkpad-x1".into());
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.show().unwrap();
        let bottom = |label: &str| {
            let e = ElementHandle::find_by_accessible_label(&window, label).next().expect(label);
            e.absolute_position().y + e.size().height
        };
        // The hint bar is 30 px; the footer ends above the window's bottom edge.
        assert!(bottom("Airplane mode") < 590.0, "{}", bottom("Airplane mode"));
        assert!(bottom("Settings") < bottom("Airplane mode"));
        // The last page is still there, below the fold.
        assert!(bottom(&nav_label(&window, "sound")) > 0.0);
    }

    /// The Bluetooth hero has a Visible switch beside the Bluetooth one, with the time it has
    /// left, while Bluetooth is on; a click anywhere on its pill switches it. The adapter dialog
    /// sets how long it lasts.
    #[test]
    fn the_visible_switch_is_in_the_bluetooth_hero() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(1200.0, 760.0));
        window.set_page("bluetooth".into());
        let list = BtPage { state: "list".into(), powered: true, visible: true, ..BtPage::default() };
        window.set_bt(BtPage { visible_left: "2:41".into(), ..list.clone() });
        let acts = Rc::new(RefCell::new(Vec::new()));
        let seen = acts.clone();
        window.on_bt_act(move |verb, argument| seen.borrow_mut().push(format!("{verb} {argument}")));
        window.show().unwrap();
        let switch = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .find(|e| e.accessible_role() == Some(AccessibleRole::Switch))
        };

        let visible = switch("Visible").expect("the Visible switch");
        let bluetooth = switch("Bluetooth").expect("the Bluetooth switch");
        assert_eq!(visible.accessible_checked(), Some(true));
        assert!(visible.absolute_position().x < bluetooth.absolute_position().x);
        let left = ElementHandle::find_by_accessible_label(&window, "2:41").next().expect("the time left");
        assert!(left.absolute_position().x < visible.absolute_position().x);
        // The old card's chips are gone.
        assert_eq!(ElementHandle::find_by_accessible_label(&window, "Pairable").count(), 0);

        // On the label, beside the switch itself.
        let (at, size) = (visible.absolute_position(), visible.size());
        let position = LogicalPosition::new(at.x - 60.0, at.y + size.height / 2.0);
        window.window().dispatch_event(WindowEvent::PointerMoved { position });
        window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        assert_eq!(*acts.borrow(), ["visible "]);

        // Hidden: no time.
        window.set_bt(BtPage { visible: false, ..list.clone() });
        assert_eq!(switch("Visible").and_then(|s| s.accessible_checked()), Some(false));
        assert_eq!(ElementHandle::find_by_accessible_label(&window, "2:41").count(), 0);

        // The adapter dialog: how long, not whether. Its choices wait for Save.
        acts.borrow_mut().clear();
        window.set_bt_adapter_open(true);
        let choices: Vec<String> = ElementHandle::find_by_accessible_label(&window, "Visible for")
            .find(|e| e.accessible_role() == Some(AccessibleRole::RadioGroup))
            .expect("the Visible switch's time")
            .query_descendants()
            .match_accessible_role(AccessibleRole::RadioButton)
            .find_all()
            .iter()
            .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
            .collect();
        assert_eq!(choices, ["3 minutes", "Always"]);
        ElementHandle::find_by_accessible_label(&window, "Always").next().unwrap().invoke_accessible_default_action();
        let pairable = ElementHandle::find_by_accessible_label(&window, "Pairable")
            .find(|e| e.accessible_role() == Some(AccessibleRole::Checkbox))
            .expect("Pairable, a check box");
        pairable.invoke_accessible_default_action();
        assert!(acts.borrow().is_empty());
        assert_eq!((window.get_bt_visible_choice(), window.get_bt_pairable()), (1, true));
        let button = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .find(|e| e.accessible_role() == Some(AccessibleRole::Button))
                .unwrap()
        };
        button("Save").invoke_accessible_default_action();
        assert_eq!(*acts.borrow(), ["adapter-save "]);
        button("Cancel").invoke_accessible_default_action();
        assert_eq!(*acts.borrow(), ["adapter-save ", "adapter-close "]);
        window.set_bt_adapter_open(false);

        // Bluetooth off: only the Bluetooth switch.
        window.set_bt(BtPage { state: "off".into(), ..BtPage::default() });
        assert!(switch("Visible").is_none());
        assert_eq!(switch("Bluetooth").and_then(|s| s.accessible_checked()), Some(false));
    }

    /// In the smallest window with Network's sub-items shown, and more pages than fit, the wheel
    /// brings every page of the sidebar into view, between the search field and Settings, which
    /// stays where it is.
    #[test]
    fn the_sidebar_scrolls_to_every_page() {
        use slint::Model;
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        let entries = super::nav_entries(&super::App::default());
        let mut groups = super::nav::groups(&entries, "", "wifi", &[]);
        // Pages still to come, so that they don't fit whatever pages there are now.
        let item = |label: &str| super::NavItem {
            id: label.into(),
            label: label.into(),
            icon: "settings".into(),
            ..Default::default()
        };
        groups.push(super::NavGroup {
            title: "MORE".into(),
            items: super::model(["Input", "Keyboard", "Mouse", "Power", "Night light"].map(item).to_vec()),
        });
        window.set_nav_groups(super::model(groups));
        window.set_hostname("thinkpad-x1".into());
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.show().unwrap();
        let list = ElementHandle::find_by_element_id(&window, "Sidebar::page-list").next().expect("the pages");
        let (top, end) = (list.absolute_position().y, list.absolute_position().y + list.size().height);
        let labels: Vec<String> = window
            .get_nav_groups()
            .iter()
            .flat_map(|g| g.items.iter().collect::<Vec<_>>())
            .map(|i| if i.sub.is_empty() { i.label.to_string() } else { format!("{}, {}", i.label, i.spoken) })
            .collect();
        let in_view = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label).any(|e| {
                let y = e.absolute_position().y;
                y >= top - 0.5 && y + e.size().height <= end + 0.5
            })
        };
        let wheel = |dy: f32| {
            window.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                position: slint::LogicalPosition::new(120.0, top + 40.0),
                delta_x: 0.0,
                delta_y: dy,
            });
            mock_elapsed_time(Duration::from_millis(300));
        };
        // It doesn't all fit, or there'd be nothing to scroll.
        assert!(!labels.iter().all(|l| in_view(l)));
        let (settings, airplane) = (bottom(&window, "Settings"), bottom(&window, "Airplane mode"));
        assert!(airplane < 590.0, "{airplane}");
        for label in &labels {
            wheel(1000.0);
            for _ in 0..20 {
                if in_view(label) {
                    break;
                }
                wheel(-30.0);
            }
            assert!(in_view(label), "{label} never came into view");
            assert_eq!((bottom(&window, "Settings"), bottom(&window, "Airplane mode")), (settings, airplane));
        }
    }

    /// Tab down the sidebar scrolls its pages, so the control with the ring is in view.
    #[test]
    fn tab_scrolls_the_sidebar_to_the_ring() {
        let window = window();
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        let entries = super::nav_entries(&super::App::default());
        let mut groups = super::nav::groups(&entries, "", "wifi", &[]);
        // After the pages there are, in their places.
        let count = groups.iter().map(|g| g.items.row_count()).sum::<usize>() as i32;
        let items = ["Input", "Keyboard", "Mouse", "Power", "Night light"].iter().zip(count..).map(|(label, place)| {
            super::NavItem {
                id: (*label).into(),
                label: (*label).into(),
                icon: "settings".into(),
                place,
                parent: -1,
                ..Default::default()
            }
        });
        groups.push(super::NavGroup { title: "MORE".into(), items: super::model(items.collect()) });
        window.set_nav_groups(super::model(groups));
        mock_elapsed_time(Duration::from_millis(50));
        let list = ElementHandle::find_by_element_id(&window, "Sidebar::page-list").next().expect("the pages");
        let (top, end) = (list.absolute_position().y, list.absolute_position().y + list.size().height);
        let in_view = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label).any(|e| {
                let y = e.absolute_position().y;
                y >= top - 0.5 && y + e.size().height <= end + 0.5
            })
        };
        assert!(!in_view("Night light"));
        tab_to(&window, &nav_label(&window, "wifi"));
        arrow_to(&window, Key::DownArrow, "Night light");
        mock_elapsed_time(Duration::from_millis(300));
        assert!(in_view("Night light"));
        // And up to the top again.
        arrow_to(&window, Key::UpArrow, &nav_label(&window, "wifi"));
        mock_elapsed_time(Duration::from_millis(300));
        assert!(in_view(&nav_label(&window, "wifi")));
    }

    /// The bottom edge of the first element with this accessible label.
    fn bottom(window: &MainWindow, label: &str) -> f32 {
        let e = ElementHandle::find_by_accessible_label(window, label).next().expect(label);
        e.absolute_position().y + e.size().height
    }

    fn wifi_rows(n: usize, selected: usize) -> Vec<super::Row> {
        (0..n)
            .map(|i| super::Row {
                id: format!("n{i}").into(),
                name: format!("Network {i}").into(),
                meta: "WPA2 · 5 GHz".into(),
                icon: "bars".into(),
                bars: 3,
                selected: i == selected,
                ..Default::default()
            })
            .collect()
    }

    /// In the smallest window, Wi-Fi keeps its hint bar, the button under the list and a
    /// details pane that ends inside the window, however long the list and the details are.
    #[test]
    fn the_network_page_fits_the_smallest_window() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.set_page("wifi".into());
        window.set_count("12 networks in range".into());
        window.set_bottom_label("Connect to hidden network…".into());
        let section = super::Section { title: "AVAILABLE".into(), rows: super::model(wifi_rows(12, 0)) };
        window.set_sections(super::model(vec![section]));
        window.set_has_details(true);
        let toggle = |id: &str| super::Toggle {
            id: id.into(),
            label: "Trusted".into(),
            sub: "Connects and uses its services without asking you first, every time".into(),
            on: false,
        };
        let item =
            |i: usize| super::DetailItem { label: format!("Item {i}").into(), value: "value".into(), mono: false };
        window.set_details(super::Details {
            title: "Network 0".into(),
            toggles: super::model(vec![toggle("a"), toggle("b")]),
            items: super::model((0..12).map(item).collect()),
            ..Default::default()
        });
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.show().unwrap();
        mock_elapsed_time(Duration::from_millis(50));

        // The hint bar is the bottom 30 px.
        assert!(
            bottom(&window, "Connect to hidden network…") <= 570.0,
            "{}",
            bottom(&window, "Connect to hidden network…")
        );
        let pane = ElementHandle::find_by_element_type_name(&window, "DetailsPane").next().expect("the details");
        let (pane_at, pane_size) = (pane.absolute_position(), pane.size());
        assert!(pane_at.y + pane_size.height <= 570.0);
        assert!(pane_at.x + pane_size.width <= 1040.0);
        // The details scroll on their own: the last item is out of view, and the wheel brings
        // it in.
        assert!(ElementHandle::find_by_accessible_label(&window, "Item 11").next().is_none());
        window.window().dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(pane_at.x + 100.0, pane_at.y + 100.0),
            delta_x: 0.0,
            delta_y: -2000.0,
        });
        mock_elapsed_time(Duration::from_millis(500));
        assert!(bottom(&window, "Item 11") <= pane_at.y + pane_size.height);
    }

    /// The bar with Apply stays in the smallest window, below the monitor's settings, with the
    /// banner about the Hyprland config shown too.
    #[test]
    fn the_displays_apply_bar_stays_in_the_smallest_window() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.set_page("displays".into());
        window.set_displays(super::DisplaysView {
            state: "list".into(),
            not_loaded: true,
            can_apply: true,
            ..Default::default()
        });
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.show().unwrap();
        mock_elapsed_time(Duration::from_millis(50));
        let apply = ElementHandle::find_by_accessible_label(&window, "Apply").next().expect("Apply");
        assert!(apply.absolute_position().y > 400.0, "{}", apply.absolute_position().y);
        assert!(bottom(&window, "Apply") <= 570.0, "{}", bottom(&window, "Apply"));
    }

    /// Moving the selection with the arrow keys scrolls the list to the selected row.
    #[test]
    fn the_selected_row_scrolls_into_view() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.set_page("wifi".into());
        let set = |w: &MainWindow, selected: usize| {
            let section = super::Section { title: "AVAILABLE".into(), rows: super::model(wifi_rows(20, selected)) };
            w.set_sections(super::model(vec![section]));
        };
        set(&window, 0);
        let selected = std::rc::Rc::new(std::cell::Cell::new(0));
        let weak = window.as_weak();
        let s = selected.clone();
        window.on_move_selection(move |delta| {
            s.set((s.get() as i32 + delta).clamp(0, 19) as usize);
            set(&weak.unwrap(), s.get());
        });
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.show().unwrap();
        mock_elapsed_time(Duration::from_millis(50));
        // Where a row is, if it's in view (a scrolled-away element isn't found).
        let row = |i: usize| {
            ElementHandle::find_by_accessible_label(&window, &format!("Network {i}, WPA2 · 5 GHz"))
                .next()
                .map(|e| (e.absolute_position().y, e.absolute_position().y + e.size().height))
        };
        let (list_top, _) = row(0).expect("the first row");
        assert_eq!(row(15), None);

        for _ in 0..15 {
            window.window().dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::DownArrow.into(),
            });
            mock_elapsed_time(Duration::from_millis(20));
        }
        assert_eq!(selected.get(), 15);
        // Scrolled: the first row went out of the list, and the selected one is in it.
        assert_eq!(row(0), None);
        let (top, bottom) = row(15).expect("the selected row");
        assert!(top >= list_top && bottom <= 570.0, "{top} {bottom}");
    }

    /// Network's chevron says whether its pages are shown, and folds them without going to a
    /// page.
    /// A press and release at a point of the window.
    fn click(window: &MainWindow, x: f32, y: f32) {
        let position = LogicalPosition::new(x, y);
        window.window().dispatch_event(WindowEvent::PointerMoved { position });
        window.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
        window.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    }

    /// The Wi-Fi and Hotspot switches are pills like Bluetooth's, switched by a click anywhere
    /// on them; the hotspot's fields are a form with its labels in their own column.
    #[test]
    fn the_network_switches_are_pills() {
        let window = window();
        let switched = Rc::new(RefCell::new(Vec::new()));
        let s = switched.clone();
        window.on_wifi_switched(move || s.borrow_mut().push("wifi"));
        let s = switched.clone();
        window.on_hotspot_toggled(move || s.borrow_mut().push("hotspot"));
        window.set_wifi_available(true);
        window.set_hotspot_available(true);
        for (page, label) in [("wifi", "Wi-Fi"), ("hotspot", "Hotspot")] {
            window.set_page(page.into());
            let switch = ElementHandle::find_by_accessible_label(&window, label)
                .find(|e| e.accessible_role() == Some(AccessibleRole::Switch))
                .expect(label);
            // On the label, beside the switch itself.
            let (at, size) = (switch.absolute_position(), switch.size());
            click(&window, at.x - 20.0, at.y + size.height / 2.0);
        }
        assert_eq!(*switched.borrow(), ["wifi", "hotspot"]);

        let x = |role: AccessibleRole| {
            ElementHandle::find_by_accessible_label(&window, "Network name")
                .find(|e| e.accessible_role() == Some(role))
                .map(|e| e.absolute_position().x)
                .expect("the network name")
        };
        // The label's column is 168 px; the text starts 12 px into its field.
        assert_eq!(x(AccessibleRole::TextInput) - x(AccessibleRole::Text), 168.0 + 12.0);
    }

    /// The Wi-Fi hero has no buttons: the pane holds them, once each. Its Metered is the editor's
    /// three-way choice, which goes to Rust as NetworkManager's number. The hotspot's QR code says
    /// it works once the hotspot is on.
    #[test]
    fn the_pane_holds_the_actions_and_metered_has_three_states() {
        let window = window();
        let chosen = Rc::new(RefCell::new(Vec::new()));
        let c = chosen.clone();
        window.on_metered_chosen(move |i| c.borrow_mut().push(i));
        let action = |id: &str, label: &str| super::Action {
            id: id.into(),
            label: label.into(),
            kind: "secondary".into(),
            enabled: true,
        };
        window.set_has_details(true);
        window.set_details(super::Details {
            title: "Lumen Home".into(),
            actions: super::model(vec![action("disconnect", "Disconnect"), action("edit", "Edit")]),
            toggles: super::model(vec![view::toggle("autoconnect", "Connect automatically", "", true)]),
            has_metered: true,
            metered: 0,
            ..Default::default()
        });
        let buttons = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
                .count()
        };
        assert_eq!((buttons("Disconnect"), buttons("Edit"), buttons("Share")), (1, 1, 0));
        let metered = ElementHandle::find_by_accessible_label(&window, "Metered connection")
            .find(|e| e.accessible_role() == Some(AccessibleRole::RadioGroup))
            .expect("Metered, a choice");
        let options: Vec<(String, bool)> = metered
            .query_descendants()
            .match_accessible_role(AccessibleRole::RadioButton)
            .find_all()
            .iter()
            .map(|e| (e.accessible_label().unwrap().to_string(), e.accessible_checked() == Some(true)))
            .collect();
        assert_eq!(options, [("Automatic".into(), true), ("Yes".into(), false), ("No".into(), false)]);
        assert!(ElementHandle::find_by_accessible_label(&window, "Apps may limit background data").next().is_some());
        ElementHandle::find_by_accessible_label(&window, "No").next().unwrap().invoke_accessible_default_action();
        assert_eq!(*chosen.borrow(), [2]);
        // No switch for it any more.
        assert!(
            !ElementHandle::find_by_accessible_label(&window, "Metered connection")
                .any(|e| e.accessible_role() == Some(AccessibleRole::Switch))
        );

        window.set_page("hotspot".into());
        window.set_hotspot_available(true);
        let shown = |text: &str| ElementHandle::find_by_accessible_label(&window, text).next().is_some();
        assert!(shown("Turn the hotspot on to share") && !shown("Scan with a phone camera to join"));
        window.set_hotspot_on(true);
        assert!(!shown("Turn the hotspot on to share") && shown("Scan with a phone camera to join"));
        assert!(shown("Generate"));
    }

    /// The quick popup: a heading for each kind, Bluetooth's switch, the volume, and rows that
    /// react to a click: one that isn't connected connects (as its button does), a connected one
    /// opens Lituus on it.
    #[test]
    fn the_quick_popup_has_headings_switches_and_rows_that_react() {
        use super::{QuickRow, QuickWindow};
        i_slint_backend_testing::init_no_event_loop();
        let popup = QuickWindow::new().unwrap();
        let row = |id: &str, name: &str, active: bool| QuickRow {
            id: id.into(),
            name: name.into(),
            meta: if active { "Connected".into() } else { "WPA2".into() },
            icon: "bars".into(),
            bars: 3,
            active,
            button: if active { "Disconnect".into() } else { "Connect".into() },
        };
        popup.set_wifi_available(true);
        popup.set_wifi_on(true);
        popup.set_networks(super::model(vec![row("wifi:a", "Lumen Home", true), row("wifi:b", "Aster Café", false)]));
        popup.set_vpns(super::model(vec![QuickRow { icon: "vpn".into(), ..row("conn:v", "UZH VPN", false) }]));
        popup.set_bt_available(true);
        popup.set_bt_on(true);
        popup.set_devices(super::model(vec![QuickRow { icon: "headphones".into(), ..row("bt:/h", "Sony", true) }]));
        popup.set_sound_available(true);
        popup.set_volume(0.5);
        let events = Rc::new(RefCell::new(Vec::<String>::new()));
        let e = events.clone();
        popup.on_row_button(move |id| e.borrow_mut().push(format!("button {id}")));
        let e = events.clone();
        popup.on_row_opened(move |id| e.borrow_mut().push(format!("open {id}")));
        let e = events.clone();
        popup.on_bt_switched(move || e.borrow_mut().push("bluetooth".into()));
        let e = events.clone();
        popup.on_volume_moved(move |v| e.borrow_mut().push(format!("volume {v:.2}")));
        popup.show().unwrap();
        let find = |label: &str, role: AccessibleRole| {
            ElementHandle::find_by_accessible_label(&popup, label).find(|e| e.accessible_role() == Some(role))
        };
        for heading in ["Wi-Fi", "VPN", "Bluetooth", "Volume", "Airplane mode"] {
            assert!(find(heading, AccessibleRole::Text).is_some(), "{heading}");
        }
        let press = |label: &str| {
            let item = find(label, AccessibleRole::ListItem).expect(label);
            let (at, size) = (item.absolute_position(), item.size());
            // On the name, away from the button.
            let position = LogicalPosition::new(at.x + 60.0, at.y + size.height / 2.0);
            popup.window().dispatch_event(WindowEvent::PointerMoved { position });
            popup.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
            popup.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
        };
        press("Aster Café, WPA2");
        press("Lumen Home, Connected");
        press("UZH VPN, WPA2");
        find("Bluetooth", AccessibleRole::Switch).expect("Bluetooth's switch").invoke_accessible_default_action();
        find("Volume", AccessibleRole::Slider).expect("the volume").invoke_accessible_increment_action();
        assert_eq!(*events.borrow(), ["button wifi:b", "open wifi:a", "button conn:v", "bluetooth", "volume 0.55"]);
        // One button per row, Disconnect on the connected ones.
        let disconnect = ElementHandle::find_by_accessible_label(&popup, "Disconnect")
            .filter(|e| e.accessible_role() == Some(AccessibleRole::Button))
            .count();
        assert_eq!(disconnect, 2);
    }

    /// The views of a page are tabs, top right in its hero, and a card's views are tabs too; a
    /// value chosen from a few (Rotation, Mode) is a radio group.
    #[test]
    fn views_are_tabs_and_values_are_radio_groups() {
        let window = window();
        let roles = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label)
                .filter_map(|e| e.accessible_role())
                .collect::<Vec<_>>()
        };
        let tabs = |label: &str| -> Vec<String> {
            ElementHandle::find_by_accessible_label(&window, label)
                .find(|e| e.accessible_role() == Some(AccessibleRole::TabList))
                .expect(label)
                .query_descendants()
                .match_accessible_role(AccessibleRole::Tab)
                .find_all()
                .iter()
                .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
                .collect()
        };
        window.set_page("displays".into());
        window.set_displays(super::DisplaysView { state: "list".into(), ..Default::default() });
        assert_eq!(tabs("Displays"), ["Monitor", "Workspaces", "Profiles"]);
        assert_eq!(tabs("Monitor settings"), ["Display", "Colour and HDR", "Advanced"]);
        assert!(roles("Rotation").contains(&AccessibleRole::RadioGroup));
        assert!(!roles("Rotation").contains(&AccessibleRole::TabList));

        window.set_page("sound".into());
        window.set_sound(super::SoundView { state: "list".into(), max: 1.0, ..Default::default() });
        assert_eq!(tabs("Sound"), ["Devices", "Apps"]);
        let views = ElementHandle::find_by_accessible_label(&window, "Sound")
            .find(|e| e.accessible_role() == Some(AccessibleRole::TabList))
            .unwrap();
        let volume = ElementHandle::find_by_accessible_label(&window, "Output volume").next().unwrap();
        // Above the hero's slider, at the right.
        assert!(views.absolute_position().y < volume.absolute_position().y);
        assert!(views.absolute_position().x > 1200.0 - 260.0, "{:?}", views.absolute_position());

        window.set_page("settings".into());
        assert!(roles("Mode").contains(&AccessibleRole::RadioGroup));
    }

    /// The audio choice of a connected headset is a segmented control with a line under each
    /// option, and a choice goes to Rust.
    #[test]
    fn the_bluetooth_audio_choice_is_segmented() {
        let window = window();
        window.set_page("bluetooth".into());
        let acts = Rc::new(RefCell::new(Vec::new()));
        let seen = acts.clone();
        window.on_bt_act(move |verb, argument| seen.borrow_mut().push(format!("{verb} {argument}")));
        let strings = |v: &[&str]| super::model(v.iter().map(|&t| SharedString::from(t)).collect());
        window.set_bt(BtPage {
            state: "list".into(),
            powered: true,
            has_details: true,
            audio_labels: strings(&["High quality", "Headset", "Off"]),
            audio_details: strings(&["A2DP · LDAC", "HFP · microphone on", "Controls only"]),
            audio_selected: 1,
            ..BtPage::default()
        });
        let options = ElementHandle::find_by_accessible_label(&window, "Audio")
            .find(|e| e.accessible_role() == Some(AccessibleRole::RadioGroup))
            .expect("the audio choice")
            .query_descendants()
            .match_accessible_role(AccessibleRole::RadioButton)
            .find_all();
        let labels: Vec<_> = options.iter().filter_map(|e| e.accessible_label()).collect();
        assert_eq!(labels, ["High quality", "Headset", "Off"]);
        assert_eq!(options[1].accessible_checked(), Some(true));
        assert!(ElementHandle::find_by_accessible_label(&window, "HFP · microphone on").next().is_some());
        options[2].invoke_accessible_default_action();
        assert_eq!(*acts.borrow(), ["audio 2"]);
    }

    /// The message above a list is one banner: its title and text read together, and its
    /// button and Dismiss work.
    #[test]
    fn the_message_banner_has_its_button_and_dismiss() {
        let window = window();
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        window.on_message_action_clicked(move || l.borrow_mut().push("action"));
        let l = log.clone();
        window.on_dismiss_message(move || l.borrow_mut().push("dismiss"));
        window.set_message_tone("danger".into());
        window.set_message_title("Couldn't connect to Aster Café".into());
        window.set_message("The network rejected the password.".into());
        window.set_message_action("Try again".into());
        let found = |label: &str| ElementHandle::find_by_accessible_label(&window, label).next().expect(label);
        found("Couldn't connect to Aster Café The network rejected the password.");
        found("Try again").invoke_accessible_default_action();
        found("Dismiss").invoke_accessible_default_action();
        assert_eq!(*log.borrow(), ["action", "dismiss"]);
    }

    #[test]
    fn the_network_chevron_folds_its_pages() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        let entries = super::nav_entries(&super::App::default());
        let show = |folded: &[String]| {
            window.set_nav_groups(super::model(super::nav::groups(&entries, "", "bluetooth", folded)))
        };
        let calls = Rc::new(RefCell::new(Vec::new()));
        let c = calls.clone();
        window.on_nav_fold(move |id| c.borrow_mut().push(format!("fold {id}")));
        let c = calls.clone();
        window.on_navigate(move |id| c.borrow_mut().push(format!("go {id}")));
        show(&[]);
        window.show().unwrap();
        let hotspots = || ElementHandle::find_by_accessible_label(&window, "Hotspot").count();
        let shown = hotspots();

        let chevron = ElementHandle::find_by_accessible_label(&window, "Network pages, expanded").next().unwrap();
        assert_eq!(chevron.accessible_expanded(), Some(true));
        chevron.invoke_accessible_default_action();
        assert_eq!(*calls.borrow(), ["fold network"]);

        show(&["network".to_owned()]);
        let chevron = ElementHandle::find_by_accessible_label(&window, "Network pages, collapsed").next().unwrap();
        assert_eq!(chevron.accessible_expanded(), Some(false));
        // The Hotspot item is gone.
        assert!(hotspots() < shown, "{} {shown}", hotspots());
    }

    /// Settings starts with the app's look, fits the smallest window, and puts the colour files
    /// beside the rest when there's room.
    #[test]
    fn settings_shows_the_appearance_section_first() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.set_page("settings".into());
        window.set_ap_follow_available(true);
        window.set_config_path("~/.config/lituus/config.toml".into());
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.show().unwrap();
        let at = |label: &str| {
            let e = ElementHandle::find_by_accessible_label(&window, label).next().expect(label);
            (e.absolute_position().x, e.absolute_position().y)
        };
        let follow = at("Follow wallpaper");
        let gradient = at("Hero gradient");
        assert!(follow.1 < 520.0 && follow.1 < gradient.1, "{follow:?}");
        // One column: the colour files under the look.
        let files = at("Read in order; the first usable one wins");
        assert!(files.1 > gradient.1, "{files:?}");

        window.window().set_size(LogicalSize::new(2000.0, 1070.0));
        let files = at("Read in order; the first usable one wins");
        let gradient = at("Hero gradient");
        assert!(files.0 > gradient.0 && files.1 < gradient.1, "{files:?}");
    }

    /// The Mode control is as wide as its options, at the end of its row like the switches.
    #[test]
    fn the_mode_control_is_as_wide_as_its_options() {
        i_slint_backend_testing::init_no_event_loop();
        let window = MainWindow::new().unwrap();
        window.set_page("settings".into());
        window.show().unwrap();
        // The left and right edges of the elements with this label, together.
        let span = |label: &str| {
            ElementHandle::find_by_accessible_label(&window, label).fold((f32::MAX, f32::MIN), |(l, r), e| {
                (l.min(e.absolute_position().x), r.max(e.absolute_position().x + e.size().width))
            })
        };
        for (w, h) in [(1040.0, 600.0), (1200.0, 760.0), (2000.0, 1070.0)] {
            window.window().set_size(LogicalSize::new(w, h));
            let (auto, light, gradient) = (span("Auto"), span("Light"), span("Hero gradient"));
            assert!(light.1 - auto.0 < 240.0, "{w}: {auto:?} {light:?}");
            assert!((light.1 - gradient.1).abs() < 8.0, "{w}: {light:?} {gradient:?}");
        }
    }

    /// The control with this label, not its name's text.
    fn control(window: &MainWindow, label: &str, role: AccessibleRole) -> ElementHandle {
        ElementHandle::find_by_accessible_label(window, label).find(|e| e.accessible_role() == Some(role)).expect(label)
    }

    /// In the owner's window the list stops at 520 px beside the details, a switch in the pane
    /// stays near its name, and buttons and fields keep their kind's width instead of the row's.
    #[test]
    fn wide_windows_keep_controls_near_their_names() {
        let window = window();
        window.set_has_details(true);
        window.set_details(super::Details {
            title: "Lumen Home".into(),
            toggles: super::model(vec![view::toggle("autoconnect", "Connect automatically", "", true)]),
            ..Default::default()
        });
        window.set_bottom_label("Connect to hidden network…".into());
        window.window().set_size(LogicalSize::new(2000.0, 1070.0));
        let list = ElementHandle::find_by_element_id(&window, "MainWindow::list-card").next().expect("the list");
        assert!(list.size().width <= 520.0, "{}", list.size().width);
        let name = control(&window, "Connect automatically", AccessibleRole::Text).absolute_position().x;
        let switch = control(&window, "Connect automatically", AccessibleRole::Switch);
        assert!(switch.absolute_position().x - name < 720.0, "{} {name}", switch.absolute_position().x);
        // Drawn 20 px tall, in a 24 px target.
        assert_eq!(switch.size().height, 24.0);
        let hidden = control(&window, "Connect to hidden network…", AccessibleRole::Button).size().width;
        assert!(hidden < 300.0, "{hidden}");

        window.set_page("hotspot".into());
        window.set_hotspot_available(true);
        for field in ["Network name", "Password"] {
            let width = control(&window, field, AccessibleRole::TextInput).size().width;
            assert!(width <= 360.0, "{field}: {width}");
        }

        window.set_page("displays".into());
        window.set_displays(super::DisplaysView { state: "list".into(), ..Default::default() });
        mock_elapsed_time(Duration::from_millis(50));
        let resolution = control(&window, "Resolution", AccessibleRole::Combobox).size().width;
        assert!(resolution <= 360.0, "{resolution}");
        // Turned off, Apply is a grey button, not a faded accent one.
        let apply = control(&window, "Apply", AccessibleRole::Button);
        assert_eq!((apply.accessible_enabled(), apply.computed_opacity()), (Some(false), 1.0));
    }

    /// Settings: the card reaches the bottom of the page, "Follow wallpaper" is turned off (not only
    /// faded) while there are no wallpaper colours, the passwords line isn't a setting row, and a
    /// short window gets the compact hero.
    #[test]
    fn settings_has_a_compact_hero_and_a_card_down_to_the_bottom() {
        let window = window();
        let acted = Rc::new(RefCell::new(Vec::new()));
        let a = acted.clone();
        window.on_ap_act(move |verb, _| a.borrow_mut().push(verb.to_string()));
        window.set_page("settings".into());
        window.set_ap_name("Teal".into());
        window.set_keyring_label("Kept in KWallet".into());
        window.set_ap_follow_available(false);
        window.window().set_size(LogicalSize::new(2000.0, 1070.0));
        let card = ElementHandle::find_by_element_id(&window, "MainWindow::settings-card").next().expect("the card");
        // Above the key hints (30 px) and the page's padding.
        let bottom = card.absolute_position().y + card.size().height;
        assert!(bottom > 1000.0, "{bottom}");
        let follow = control(&window, "Follow wallpaper", AccessibleRole::Switch);
        assert_eq!(follow.accessible_enabled(), Some(false));
        follow.invoke_accessible_default_action();
        assert!(acted.borrow().is_empty(), "{:?}", acted.borrow());
        assert!(
            ElementHandle::find_by_accessible_label(&window, "Passwords \"for me only\": Kept in KWallet.")
                .next()
                .is_some()
        );

        let name_bottom = || {
            let name = control(&window, "Teal", AccessibleRole::Text);
            name.absolute_position().y + name.size().height
        };
        // The page's padding (18 px) and the hero: 206 px, or 136 px below 720 px.
        assert!(name_bottom() > 18.0 + 136.0, "{}", name_bottom());
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        // The window notes its new height on its next turn.
        mock_elapsed_time(Duration::from_millis(50));
        assert!(name_bottom() <= 18.0 + 136.0, "{}", name_bottom());
    }
}
