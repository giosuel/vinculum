use std::cell::RefCell;
use std::time::{Duration, SystemTime};

use clap::Parser;
use futures_util::StreamExt;
use slint::{ComponentHandle, ModelRc, VecModel};

mod config;
mod theme;
mod view;

slint::include_modules!();

/// The app ID, also the Wayland app_id that Hyprland window rules match on.
const APP_ID: &str = "ch.giosuel.Vinculum";

/// How often the colour sources are checked for changes.
const THEME_POLL: Duration = Duration::from_secs(1);

#[derive(Parser)]
#[command(name = "vinculum", version = env!("APP_VERSION"), about = "Manage Wi-Fi, wired and VPN connections")]
struct Cli {}

/// The UI thread's state: the latest snapshot, and what the user picked.
#[derive(Default)]
struct App {
    snapshot: Option<vinculum_nm::Snapshot>,
    error: Option<String>,
    ui: view::UiState,
    row_ids: Vec<String>,
    /// The palette from the colour sources; `None` means the built-in theme.
    palette: Option<theme::Palette>,
    builtin: config::Builtin,
    /// Commands for the NetworkManager thread.
    commands: Option<tokio::sync::mpsc::UnboundedSender<view::Command>>,
    /// The message above the list, and its tone ("danger", "warn", "info").
    message: Option<(String, &'static str)>,
    /// The command waiting for the confirmation dialog, and the dialog's title, text and button.
    confirm: Option<view::Command>,
    confirm_text: Option<(String, String, String)>,
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
}

fn main() -> Result<(), slint::PlatformError> {
    Cli::parse();
    let window = MainWindow::new()?;
    // Needs the platform, which creating the first window sets up; takes effect when it's shown.
    slint::set_xdg_app_id(APP_ID)?;
    window.set_version(env!("APP_VERSION").into());
    connect_callbacks(&window);
    apply_theme(&window);
    render(&window);
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    APP.with(|app| app.borrow_mut().commands = Some(commands));
    spawn_network_manager(window.as_weak(), receiver);
    spawn_theme_watcher(window.as_weak());
    window.run()
}

fn connect_callbacks(window: &MainWindow) {
    let weak = window.as_weak();
    window.on_navigate(move |id| {
        if let Some(page) = view::Page::from_id(&id) {
            update(&weak, |app| {
                app.ui = view::UiState { page, ..view::UiState::default() };
            });
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
    window.on_activate_selected(move || update(&weak, |app| act(app, "connect")));
    let weak = window.as_weak();
    window.on_toggle(move |_| {
        update(&weak, |app| {
            app.message = Some(("Changing this comes with the connection editor, in a later version.".into(), "info"));
        })
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
}

/// A details button (or Enter, for "connect") on the selected row.
fn act(app: &mut App, action: &str) {
    let (Some(snapshot), Some(selected)) = (app.snapshot.as_ref(), app.ui.selected.as_deref()) else { return };
    match view::intent(snapshot, selected, action) {
        Some(view::Intent::Run(command)) => send(app, command),
        Some(view::Intent::Confirm { title, text, button, command }) => {
            app.confirm = Some(command);
            app.confirm_text = Some((title, text, button));
        }
        Some(view::Intent::Notice(text)) => app.message = Some((text, "info")),
        None => {}
    }
}

fn send(app: &mut App, command: view::Command) {
    app.message = None;
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

fn render(window: &MainWindow) {
    APP.with(|app| {
        let mut app = app.borrow_mut();
        let v = view::build(app.snapshot.as_ref(), app.error.as_deref(), &app.ui);
        window.set_nav(model(v.nav));
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
        let (message, tone) = app.message.clone().unwrap_or_default();
        window.set_message(message.into());
        window.set_message_tone(if tone.is_empty() { "info".into() } else { tone.into() });
        window.set_confirm_open(app.confirm.is_some());
        if let Some((title, text, button)) = &app.confirm_text {
            window.set_confirm_title(title.into());
            window.set_confirm_text(text.into());
            window.set_confirm_button(button.into());
        }
        app.row_ids = v.row_ids;
        // Keep a selection that went away (a network out of range) from sticking.
        app.ui.selected = v.selected;
    });
}

/// Watches NetworkManager on a tokio thread, hands every snapshot to the UI thread, and runs the
/// UI's commands, reporting their failures back.
fn spawn_network_manager(
    weak: slint::Weak<MainWindow>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<view::Command>,
) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        runtime.block_on(async move {
            loop {
                let result = async {
                    let client = vinculum_nm::Client::system().await?;
                    let mut snapshots = Box::pin(client.watch().await?);
                    loop {
                        tokio::select! {
                            snapshot = snapshots.next() => match snapshot {
                                Some(snapshot) => deliver(&weak, snapshot),
                                None => break,
                            },
                            Some(command) = commands.recv() => {
                                let (client, weak) = (client.clone(), weak.clone());
                                tokio::spawn(async move {
                                    if let Err(error) = run(&client, &command).await {
                                        report(&weak, &command, &error);
                                    }
                                });
                            }
                        }
                    }
                    Ok::<(), vinculum_nm::Error>(())
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

async fn run(client: &vinculum_nm::Client, command: &view::Command) -> Result<(), vinculum_nm::Error> {
    use view::Command::*;
    match command {
        SetWifi(on) => client.set_wireless_enabled(*on).await,
        Rescan { device } => client.request_scan(device).await,
        Activate { connection, device, access_point } => {
            client.activate(connection, device.as_deref(), access_point.as_deref()).await.map(drop)
        }
        ConnectOpen { device, access_point, ssid } => client.connect_open(device, access_point, ssid).await.map(drop),
        Deactivate { active } => client.deactivate(active).await,
        Forget { connection } => client.forget(connection).await,
    }
}

/// Shows why a command failed, above the list.
fn report(weak: &slint::Weak<MainWindow>, command: &view::Command, error: &vinculum_nm::Error) {
    use view::Command::*;
    let what = match command {
        SetWifi(true) => "Couldn't turn Wi-Fi on",
        SetWifi(false) => "Couldn't turn Wi-Fi off",
        Rescan { .. } => "Couldn't scan",
        Activate { .. } | ConnectOpen { .. } => "Couldn't connect",
        Deactivate { .. } => "Couldn't disconnect",
        Forget { .. } => "Couldn't forget the connection",
    };
    let (text, tone) = match error {
        vinculum_nm::Error::Denied(_) => (
            format!(
                "{what}: not allowed. NetworkManager asks polkit first, and no polkit agent answered. Start one (for example hyprpolkitagent) and try again."
            ),
            "warn",
        ),
        other => (format!("{what}: {other}."), "danger"),
    };
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| app.borrow_mut().message = Some((text, tone)));
        render(&window);
    });
}

fn deliver(weak: &slint::Weak<MainWindow>, snapshot: Result<vinculum_nm::Snapshot, vinculum_nm::Error>) {
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| {
            let mut app = app.borrow_mut();
            match snapshot {
                Ok(snapshot) => {
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
                let (statuses, palette) = theme::resolve(&config.theme.sources);
                let rows = source_rows(&config, &statuses);
                let builtin = config.theme.builtin;
                let config_path = path.display().to_string();
                let _ = weak.upgrade_in_event_loop(move |window| {
                    APP.with(|app| {
                        let mut app = app.borrow_mut();
                        app.palette = palette;
                        app.builtin = builtin;
                    });
                    apply_theme(&window);
                    window.set_config_path(shorten_home(&config_path).into());
                    window.set_builtin_label(builtin_label(builtin, palette.is_some()).into());
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

fn source_rows(config: &config::Config, statuses: &[theme::SourceStatus]) -> Vec<SourceRowData> {
    let mut used = false;
    let mut rows: Vec<SourceRowData> = config
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
                    ("In use", "accent", vec![p.accent, p.accent_soft_bg, p.surface, p.danger])
                }
                theme::SourceStatus::Used(_) => ("Usable", "muted", Vec::new()),
            };
            SourceRowData { n: i as i32 + 1, label, path: source.path.clone(), status, tone, swatches }
        })
        .collect();
    rows.push(SourceRowData {
        n: rows.len() as i32 + 1,
        label: "Built-in theme".into(),
        path: "Follows the setting below".into(),
        status: if used { "Fallback" } else { "In use" },
        tone: if used { "muted" } else { "accent" },
        swatches: Vec::new(),
    });
    rows
}

fn builtin_label(builtin: config::Builtin, overridden: bool) -> String {
    let label = match builtin {
        config::Builtin::Auto => "Follow the desktop (dark or light)",
        config::Builtin::Dark => "Dark",
        config::Builtin::Light => "Light",
    };
    if overridden { format!("{label}, when no colour source is usable") } else { label.into() }
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
    let (palette, builtin) = APP.with(|app| {
        let app = app.borrow();
        (app.palette, app.builtin)
    });
    let p = palette.unwrap_or_else(|| theme::Palette::builtin(builtin, window.get_prefers_dark()));
    let t = window.global::<Theme>();
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
}
