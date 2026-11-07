//! The Shortcuts page (Input › Shortcuts): the binds in the user's Hyprland config, in groups and
//! in words ("Close window", "Go to workspace 3"), with the keys and the file and line each comes
//! from. Hyprland's own list (`hyprctl binds`) says which are in effect, and has the binds made
//! while it runs. A shortcut is changed in a dialog, or turned off with its switch: Lituus writes
//! that to its own file (shortcut_edit.rs) and applies it at once with `hyprctl eval`.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use lituus_hypr::{
    Action, Bind, BindConfig, ConfigFile, Hyprland, LiveBind, LuaValue as Value, config_dir, lua_string, modifier_bit,
    normalized_keys, read_binds,
};
use slint::{ModelRc, SharedString, VecModel};

use crate::input::kept;
use crate::shortcut_edit::{self, DIRECTIONS, Field, Form, KINDS, Mine, SECTION};
use crate::{HeroInfo, Hint, MainWindow, ShortcutDialog, ShortcutRow, ShortcutsView, update};

#[derive(Default)]
pub struct State {
    pub not_running: bool,
    /// Read at least once.
    read: bool,
    pub error: Option<String>,
    /// The user's config, without Lituus's own file.
    config: BindConfig,
    /// What Lituus's part of its file changes and adds.
    mine: Vec<Mine>,
    /// Where the config loads Lituus's file: the file and line of its `require("lituus")`.
    loads: Option<(PathBuf, usize)>,
    /// What Hyprland has now; `None` when it couldn't say.
    live: Option<Vec<LiveBind>>,
    hypr: Option<Hyprland>,
    pub query: String,
    pub dialog: Option<Dialog>,
    /// The bind of Lituus's own about to be deleted (an index into `mine`), once confirmed.
    pub deleting: Option<usize>,
    /// Why the last change couldn't be saved or applied.
    message: Option<String>,
    /// Where Hyprland's config is, if not the usual place (tests).
    hypr_dir: Option<PathBuf>,
}

impl State {
    fn dir(&self) -> PathBuf {
        self.hypr_dir.clone().unwrap_or_else(config_dir)
    }

    fn file(&self) -> ConfigFile {
        ConfigFile::new(self.dir(), true)
    }

    /// The config is Lua, which Lituus can add its part to (hyprlang is read only, for now).
    fn editable(&self) -> bool {
        self.dir().join("hyprland.lua").exists()
    }

    /// What Lituus's part does to the config's bind `b`.
    fn over(&self, b: &Bind) -> Option<&Option<Bind>> {
        let keys = normalized_keys(&b.keys());
        self.mine.iter().find_map(|m| match m {
            Mine::Over { bind, .. } if m.keys() == keys => Some(bind),
            _ => None,
        })
    }
}

/// What a row or the dialog is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// One of the config's binds (an index into `config.binds`).
    Config(usize),
    /// One of Lituus's own (an index into `mine`).
    Added(usize),
}

impl Target {
    fn id(self) -> String {
        match self {
            Target::Config(i) => format!("c{i}"),
            Target::Added(i) => format!("a{i}"),
        }
    }

    fn parse(id: &str) -> Option<Target> {
        let n = id.get(1..)?.parse().ok()?;
        match id.as_bytes().first()? {
            b'c' => Some(Target::Config(n)),
            b'a' => Some(Target::Added(n)),
            _ => None,
        }
    }
}

/// The edit dialog: what it's about, and the form. `unread`: a config bind Lituus can't read,
/// which can only be kept or turned off, or replaced by a new one on its keys (`replacing`).
#[derive(Clone, Debug, PartialEq)]
pub struct Dialog {
    pub target: Target,
    pub form: Form,
    pub unread: bool,
    pub replacing: bool,
    pub use_on: bool,
}

/// What was read, on a thread: the config, Lituus's part, Hyprland's binds.
pub type Read = Result<(BindConfig, Option<String>, Option<Vec<LiveBind>>, Hyprland), lituus_hypr::Error>;

/// Reads the config's binds and Hyprland's (on a thread: the files and the socket may take a
/// moment). Also from the thread that follows Hyprland's events.
pub fn refresh(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    std::thread::spawn(move || {
        let read = read(&config_dir());
        let _ = weak.clone().upgrade_in_event_loop(move |_| update(&weak, move |app| loaded(&mut app.shortcuts, read)));
    });
}

fn read(dir: &Path) -> Read {
    let h = Hyprland::from_env()?;
    let section = ConfigFile::new(dir, true).read_section(SECTION);
    Ok((read_binds(dir), section, h.binds().ok(), h))
}

pub fn loaded(state: &mut State, read: Read) {
    state.read = true;
    match read {
        Ok((mut config, section, live, hypr)) => {
            // Lituus's own file is read as its part, not as the user's binds.
            let own = state.file().path();
            config.binds.retain(|b| b.file != own);
            config.unbinds.retain(|u| u.file != own);
            state.loads = config.requires.iter().find(|(name, _, _)| name == "lituus").map(|(_, f, l)| (f.clone(), *l));
            state.mine = section.map(|t| shortcut_edit::parse(&t, &own)).unwrap_or_default();
            state.config = config;
            state.live = live;
            state.hypr = Some(hypr);
            state.not_running = false;
            state.error = None;
        }
        Err(lituus_hypr::Error::NotRunning) => state.not_running = true,
        Err(e) => state.error = Some(e.to_string()),
    }
}

/// Lituus's part becomes `mine`: written to its file and applied with `lua` (none: Hyprland
/// reads its config again, for a bind of the config's Lituus can't write itself), on a thread.
fn change(weak: Option<&slint::Weak<MainWindow>>, state: &mut State, mine: Vec<Mine>, lua: Option<String>) {
    state.mine = mine;
    state.message = None;
    let text = shortcut_edit::text(&state.mine);
    let file = state.file();
    let hypr = state.hypr.clone();
    let Some(weak) = weak.cloned() else {
        // Tests: written at once, nothing applied.
        if let Err(e) = file.write_section(SECTION, &text) {
            state.message = Some(format!("Couldn't save the change: {e}."));
        }
        return;
    };
    let dir = state.dir();
    std::thread::spawn(move || {
        let done =
            file.write_section(SECTION, &text).map_err(|e| format!("Couldn't save the change: {e}.")).and_then(|()| {
                let Some(h) = hypr else { return Ok(()) };
                match &lua {
                    Some(lua) => h.eval(lua),
                    None => h.request("reload").map(|_| ()),
                }
                .map_err(|e| format!("Saved, but Hyprland didn't take it: {e}."))
            });
        let read = read(&dir);
        let _ = weak.clone().upgrade_in_event_loop(move |_| {
            update(&weak, move |app| {
                loaded(&mut app.shortcuts, read);
                app.shortcuts.message = done.err();
            })
        });
    });
}

/// The Lua that brings back the config's binds on `keys` (none when one can't be written: then
/// Hyprland reads its config again).
fn restore_lua(state: &State, keys: &str) -> Option<String> {
    let norm = normalized_keys(keys);
    let mut lua = vec![format!("hl.unbind({})", lua_string(keys))];
    for b in state.config.binds.iter().filter(|b| normalized_keys(&b.keys()) == norm) {
        lua.push(b.to_lua()?);
    }
    Some(lua.join(" "))
}

/// Lituus's part with the config's bind `b` changed to `bind` (none: turned off), and the Lua
/// that does it now.
fn with_over(state: &State, b: &Bind, bind: Option<Bind>) -> (Vec<Mine>, Option<String>) {
    let keys = b.keys();
    let norm = normalized_keys(&keys);
    let mut mine: Vec<Mine> =
        state.mine.iter().filter(|m| !matches!(m, Mine::Over { .. } if m.keys() == norm)).cloned().collect();
    let mut lua = format!("hl.unbind({})", lua_string(&keys));
    if let Some(l) = bind.as_ref().and_then(Bind::to_lua) {
        lua = format!("{lua} {l}");
    }
    mine.push(Mine::Over { keys, bind });
    (mine, Some(lua))
}

/// Lituus's part without its change to the config's bind `b`, and the Lua that brings it back.
fn without_over(state: &State, b: &Bind) -> (Vec<Mine>, Option<String>) {
    let norm = normalized_keys(&b.keys());
    let mine = state.mine.iter().filter(|m| !matches!(m, Mine::Over { .. } if m.keys() == norm)).cloned().collect();
    (mine, restore_lua(state, &b.keys()))
}

// ---- in words ----------------------------------------------------------------------------------

/// The groups, in the order the page lists them.
const GROUPS: [&str; 8] = ["Apps", "Windows", "Workspaces", "Focus", "Media keys", "Screenshots", "Session", "Other"];

/// A bind's action in words: its group, what it does, and a detail (the command it runs, or the
/// workspace's name); `mono` when the detail is a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    pub group: &'static str,
    pub title: String,
    pub detail: String,
    pub mono: bool,
}

fn described(group: &'static str, title: impl Into<String>) -> Described {
    Described { group, title: title.into(), detail: String::new(), mono: false }
}

/// Apps known by another name than their command's.
const APPS: [(&str, &str); 14] = [
    ("nautilus", "Files"),
    ("thunar", "Files"),
    ("nemo", "Files"),
    ("pcmanfm", "Files"),
    ("hyprlauncher", "the app launcher"),
    ("fuzzel", "the app launcher"),
    ("wofi", "the app launcher"),
    ("rofi", "the app launcher"),
    ("tofi", "the app launcher"),
    ("walker", "the app launcher"),
    ("anyrun", "the app launcher"),
    ("google-chrome-stable", "Chrome"),
    ("code", "VS Code"),
    ("org.gnome.Nautilus", "Files"),
];

/// What a command does, for the ones every config has (the volume keys, the media keys, the
/// screen locker), else "Open Kitty" for an app on its own, else "Run a command".
fn command(cmd: &str) -> Described {
    let c = cmd.trim();
    let words: Vec<&str> = c.split_whitespace().collect();
    let first = words.first().copied().unwrap_or_default();
    let program = first.rsplit('/').next().unwrap_or(first);
    let has = |w: &str| words.contains(&w);
    let up = c.ends_with('+') || c.contains("%+") || has("up") || has("raise") || has("inc");
    let media = |title: &str| described("Media keys", title);
    let mut d = match program {
        "wpctl" | "pactl" | "pamixer" if c.contains("mute") || has("-t") => {
            if c.contains("SOURCE") || c.contains("source") {
                media("Mute the microphone")
            } else {
                media("Mute or unmute")
            }
        }
        "wpctl" | "pactl" | "pamixer" | "amixer" => {
            media(if up { "Turn the volume up" } else { "Turn the volume down" })
        }
        "playerctl" => media(match words.iter().rev().find(|w| !w.starts_with('-')).copied().unwrap_or_default() {
            "play-pause" => "Play or pause",
            "play" => "Play",
            "pause" => "Pause",
            "next" => "Next track",
            "previous" => "Previous track",
            "stop" => "Stop playing",
            _ => "Control what's playing",
        }),
        "brightnessctl" | "light" | "xbacklight" => {
            media(if up { "Make the screen brighter" } else { "Dim the screen" })
        }
        "grim" | "grimblast" | "hyprshot" | "flameshot" | "satty" | "slurp" => {
            described("Screenshots", "Take a screenshot")
        }
        "hyprlock" | "swaylock" | "gtklock" => described("Session", "Lock the screen"),
        "loginctl" if has("lock-session") => described("Session", "Lock the screen"),
        "wlogout" | "nwg-bar" => described("Session", "Power menu"),
        "hyprpicker" => described("Apps", "Pick a colour"),
        _ if c.contains("hyprshutdown") || c.contains("hl.dsp.exit") => described("Session", "Exit Hyprland"),
        _ if words.len() <= 2 && !c.contains(['|', ';', '&', '$', '`']) && !program.is_empty() => {
            let name = APPS
                .iter()
                .find(|(cmd, _)| *cmd == program)
                .map_or_else(|| capitalised(program), |(_, name)| (*name).to_owned());
            described("Apps", format!("Open {name}"))
        }
        _ => described("Apps", "Run a command"),
    };
    d.detail = c.to_owned();
    d.mono = true;
    d
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Text(t) => Some(t.clone()),
        Value::Int(i) => Some(i.to_string()),
        _ => None,
    }
}

/// A workspace in words: "workspace 3", "the next workspace", "the scratchpad".
fn workspace(w: &Value) -> String {
    let w = text_of(w).unwrap_or_default();
    match w.as_str() {
        "e+1" | "r+1" | "m+1" | "+1" => "the next workspace".to_owned(),
        "e-1" | "r-1" | "m-1" | "-1" => "the previous workspace".to_owned(),
        "previous" => "the workspace before".to_owned(),
        "empty" => "an empty workspace".to_owned(),
        "special" | "special:magic" | "special:scratchpad" => "the scratchpad".to_owned(),
        _ => match w.strip_prefix("special:") {
            Some(name) => format!("the special workspace {name}"),
            None => match w.strip_prefix("name:") {
                Some(name) => format!("workspace {name}"),
                None => format!("workspace {w}"),
            },
        },
    }
}

fn direction(d: &str) -> &'static str {
    match d {
        "left" | "l" => "on the left",
        "right" | "r" => "on the right",
        "up" | "u" | "t" => "above",
        _ => "below",
    }
}

/// What a bind does, in words.
pub fn describe(bind: &Bind) -> Described {
    let Action::Dispatcher { name, .. } = &bind.action else {
        return Described {
            group: "Other",
            title: if bind.description.is_empty() { "Set in your config".into() } else { bind.description.clone() },
            detail: "Lua code Lituus doesn't read".into(),
            mono: false,
        };
    };
    let field = |f: &str| bind.action.field(f);
    let text = bind.action.text().unwrap_or_default();
    let mut d = match name.as_str() {
        "exec_cmd" | "exec_raw" => command(text),
        "window.close" => described("Windows", "Close window"),
        "window.kill" => described("Windows", "Force-quit the app"),
        "window.float" => described(
            "Windows",
            match field("action").and_then(Value::as_text) {
                Some("enable") | Some("on") => "Float window",
                Some("disable") | Some("off") => "Tile window",
                _ => "Float or tile window",
            },
        ),
        "window.fullscreen" => described(
            "Windows",
            match field("mode").and_then(Value::as_text) {
                Some("maximized") | Some("maximize") => "Maximise",
                _ => "Fullscreen",
            },
        ),
        "window.pseudo" => described("Windows", "Pseudotile window"),
        "window.pin" => described("Windows", "Pin to every workspace"),
        "window.center" => described("Windows", "Centre window"),
        "window.drag" => described("Windows", "Move window by dragging"),
        "window.resize" if bind.flags.mouse => described("Windows", "Resize window by dragging"),
        "window.resize" => described("Windows", "Resize window"),
        "window.swap" => described("Windows", "Swap windows"),
        "window.cycle_next" => described("Windows", "Next window"),
        "window.move" => match (field("workspace"), field("direction").and_then(Value::as_text)) {
            (Some(w), _) => described("Workspaces", format!("Move window to {}", workspace(w))),
            (None, Some(dir)) => {
                described("Windows", format!("Move window {}", direction(dir).trim_start_matches("on the ")))
            }
            _ => described("Windows", "Move window"),
        },
        "layout" if text == "togglesplit" => described("Windows", "Switch split direction"),
        "layout" => Described { group: "Windows", title: "Layout".into(), detail: text.to_owned(), mono: true },
        "group.toggle" => described("Windows", "Group or ungroup windows"),
        "group.next" => described("Windows", "Next window in the group"),
        "group.prev" => described("Windows", "Previous window in the group"),
        "group.lock" | "group.lock_active" => described("Windows", "Lock the group"),
        "focus" => match (field("workspace"), field("direction").and_then(Value::as_text), field("monitor")) {
            (Some(w), _, _) => described("Workspaces", format!("Go to {}", workspace(w))),
            (None, Some(dir), _) => described("Focus", format!("Focus the window {}", direction(dir))),
            (None, None, Some(m)) => described("Focus", format!("Focus monitor {}", text_of(m).unwrap_or_default())),
            _ if field("last").is_some() || field("urgent_or_last").is_some() => {
                described("Focus", "Focus the last window")
            }
            _ => described("Focus", "Focus"),
        },
        "workspace.toggle_special" => {
            let special = if text.is_empty() { "special".into() } else { format!("special:{text}") };
            described("Workspaces", format!("Show {}", workspace(&Value::Text(special))))
        }
        "workspace.rename" => described("Workspaces", "Rename workspace"),
        "workspace.move" => described("Workspaces", "Move workspace to a monitor"),
        "exit" => described("Session", "Exit Hyprland"),
        "reload_config" => described("Session", "Reload the config"),
        "submap" if text == "reset" => described("Session", "Leave the submap"),
        "submap" => described("Session", format!("Go to submap {text}")),
        "dpms" => described("Session", "Turn the screens off or on"),
        "global" => {
            Described { group: "Other", title: "An app's own shortcut".into(), detail: text.to_owned(), mono: true }
        }
        other => {
            // The dispatcher's name, readable: "window.toggle_swallow" → "Toggle swallow".
            let last = other.rsplit('.').next().unwrap_or(other).replace('_', " ");
            Described { group: "Other", title: capitalised(&last), detail: format!("hl.dsp.{other}"), mono: true }
        }
    };
    // A description of the user's own says it best; what it runs stays below it.
    if !bind.description.is_empty() {
        if d.detail.is_empty() {
            d.detail = d.title.clone();
            d.mono = false;
        }
        d.title = bind.description.clone();
    }
    d
}

/// A key's name on its key cap: "Super", "←", "Volume up", "Left button".
pub fn key_name(key: &str) -> String {
    let lower = key.to_lowercase();
    let named = match lower.as_str() {
        "super" | "win" | "logo" | "mod4" | "meta" => "Super",
        "super_l" => "Left Super",
        "super_r" => "Right Super",
        "shift" | "shift_l" | "shift_r" => "Shift",
        "ctrl" | "control" | "ctrl_l" | "ctrl_r" | "control_l" | "control_r" => "Ctrl",
        "alt" | "mod1" | "alt_l" => "Alt",
        "alt_r" | "mod5" => "AltGr",
        "caps" => "Caps Lock",
        "mod2" => "Num Lock",
        "left" => "←",
        "right" => "→",
        "up" => "↑",
        "down" => "↓",
        "return" | "enter" => "Enter",
        "space" => "Space",
        "escape" => "Esc",
        "tab" => "Tab",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "print" => "Print",
        "comma" => ",",
        "period" => ".",
        "minus" => "-",
        "equal" => "=",
        "plus" => "+",
        "slash" => "/",
        "backslash" => "\\",
        "semicolon" => ";",
        "apostrophe" => "'",
        "grave" => "`",
        "bracketleft" => "[",
        "bracketright" => "]",
        "xf86audioraisevolume" => "Volume up",
        "xf86audiolowervolume" => "Volume down",
        "xf86audiomute" => "Mute",
        "xf86audiomicmute" => "Mic mute",
        "xf86audioplay" => "Play",
        "xf86audiopause" => "Pause",
        "xf86audionext" => "Next track",
        "xf86audioprev" => "Previous track",
        "xf86audiostop" => "Stop",
        "xf86monbrightnessup" => "Brightness up",
        "xf86monbrightnessdown" => "Brightness down",
        "xf86calculator" => "Calculator",
        "mouse:272" => "Left button",
        "mouse:273" => "Right button",
        "mouse:274" => "Middle button",
        "mouse_down" => "Scroll down",
        "mouse_up" => "Scroll up",
        "mouse_left" => "Scroll left",
        "mouse_right" => "Scroll right",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_owned();
    }
    if let Some(code) = lower.strip_prefix("code:") {
        return format!("Key {code}");
    }
    if let Some(button) = lower.strip_prefix("mouse:") {
        return format!("Button {button}");
    }
    if let Some(rest) = key.strip_prefix("XF86") {
        return rest.to_owned();
    }
    if key.chars().count() == 1 { key.to_uppercase() } else { capitalised(key) }
}

/// The key caps of a bind: each modifier, then the key (each key of a chord).
pub fn caps(bind: &Bind) -> Vec<String> {
    bind.mods.iter().map(|m| key_name(m)).chain(bind.key.split(" + ").filter(|k| !k.is_empty()).map(key_name)).collect()
}

fn options(bind: &Bind) -> Vec<&'static str> {
    let f = &bind.flags;
    let mut o = Vec::new();
    if f.mouse {
        o.push("Mouse drag");
    }
    if f.repeating {
        o.push("Repeats");
    }
    if f.locked {
        o.push("On the lock screen");
    }
    if f.release {
        o.push("On release");
    }
    if f.long_press {
        o.push("Long press");
    }
    o
}

/// "hyprland.lua · line 12", with the path from the config folder (or from home).
fn from(bind: &Bind, dir: &Path) -> String {
    let file = bind
        .file
        .strip_prefix(dir)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| shorten_home(&bind.file.display().to_string()));
    if bind.end_line > bind.line {
        format!("{file} · lines {}–{}", bind.line, bind.end_line)
    } else {
        format!("{file} · line {}", bind.line)
    }
}

fn shorten_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

// ---- the page ----------------------------------------------------------------------------------

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

fn strings(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn has_list(state: &State) -> bool {
    state.read && !state.not_running && state.error.is_none()
}

/// The keys that work on the page: the search field's.
pub fn keys(state: &State) -> Vec<Hint> {
    if has_list(state) { vec![Hint::new("/", "Search")] } else { Vec::new() }
}

pub fn status(state: &State) -> String {
    if has_list(state) { state.config.binds.len().to_string() } else { String::new() }
}

/// The main key: the modifier most binds use.
fn main_key(binds: &[Bind]) -> Option<(String, usize)> {
    let mut counts: Vec<(u32, String, usize)> = Vec::new();
    for b in binds {
        for m in &b.mods {
            let Some(bit) = modifier_bit(m) else { continue };
            match counts.iter_mut().find(|(b, _, _)| *b == bit) {
                Some(c) => c.2 += 1,
                None => counts.push((bit, key_name(m), 1)),
            }
        }
    }
    counts.into_iter().max_by_key(|(_, _, n)| *n).map(|(_, name, n)| (name, n))
}

/// The modifiers in `hyprctl binds`' modmask, in the order key caps show them.
const MODMASK: [(u32, &str); 8] =
    [(64, "SUPER"), (4, "CTRL"), (8, "ALT"), (1, "SHIFT"), (128, "MOD5"), (2, "CAPS"), (16, "MOD2"), (32, "MOD3")];

/// What Lituus did to a row: changed, added, turned off.
const CHANGED: &str = "Changed in Lituus";
const ADDED: &str = "Added in Lituus";
const TURNED_OFF: &str = "Turned off in Lituus";

struct Entry {
    target: Option<Target>,
    group: String,
    caps: Vec<String>,
    d: Described,
    options: String,
    from: String,
    /// What Lituus did to it, if anything.
    marker: &'static str,
    on: bool,
    unread: bool,
    warn: bool,
}

/// How many of Hyprland's binds are on `b`'s keys; `None` when Hyprland couldn't say.
fn live_count(state: &State, b: &Bind) -> Option<usize> {
    state.live.as_ref().map(|live| live.iter().filter(|l| l.matches(b)).count())
}

fn entries(state: &State) -> Vec<Entry> {
    let dir = state.dir();
    let mut entries: Vec<Entry> = Vec::new();
    for (i, b) in state.config.binds.iter().enumerate() {
        let over = state.over(b);
        let shown = match over {
            Some(Some(new)) => new,
            _ => b,
        };
        let d = describe(shown);
        let mut options: Vec<&str> = options(shown);
        let count = live_count(state, b);
        // In effect: Hyprland has it (once, when Lituus changed it), or doesn't, when it's off.
        let in_effect = match (over, count) {
            (_, None) => true,
            (Some(None), Some(n)) => n == 0,
            (Some(Some(_)), Some(n)) => n == 1,
            (None, Some(n)) => n > 0,
        };
        let marker = match over {
            Some(Some(_)) => CHANGED,
            Some(None) => TURNED_OFF,
            None => "",
        };
        if !in_effect {
            options.push("Not in effect now");
        }
        entries.push(Entry {
            target: Some(Target::Config(i)),
            group: if b.submap.is_empty() { d.group.to_owned() } else { format!("Submap {}", b.submap) },
            caps: caps(b),
            unread: shown.action == Action::Unread,
            d,
            options: options.join(" · "),
            from: from(b, &dir),
            marker,
            on: !matches!(over, Some(None)),
            warn: !in_effect,
        });
    }
    for (i, m) in state.mine.iter().enumerate() {
        let Mine::Added { bind, on } = m else { continue };
        let d = describe(bind);
        let in_effect = !on || live_count(state, bind).is_none_or(|n| n == 1);
        let mut options = options(bind);
        if !in_effect {
            options.push("Not in effect now");
        }
        entries.push(Entry {
            target: Some(Target::Added(i)),
            group: d.group.to_owned(),
            caps: caps(bind),
            unread: false,
            d,
            options: options.join(" · "),
            from: from(bind, &dir),
            marker: ADDED,
            on: *on,
            warn: !in_effect,
        });
    }
    // What Hyprland has that neither the config nor Lituus's part has: made while it runs
    // (hyprctl eval, a plugin).
    let known = |l: &LiveBind| {
        state.config.binds.iter().any(|b| l.matches(b))
            || state.mine.iter().any(|m| match m {
                Mine::Added { bind, .. } | Mine::Over { bind: Some(bind), .. } => l.matches(bind),
                Mine::Over { bind: None, .. } => false,
            })
    };
    for l in state.live.iter().flatten().filter(|l| !known(l)) {
        let mods = MODMASK.iter().filter(|(bit, _)| l.modmask & bit != 0).map(|(_, m)| key_name(m));
        let caps = mods.chain(std::iter::once(key_name(&l.key))).collect();
        let title =
            if l.description.is_empty() { "Made while Hyprland runs".to_owned() } else { l.description.clone() };
        let detail =
            l.dispatcher.strip_prefix("HL.Dispatcher(").and_then(|d| d.strip_suffix(')')).unwrap_or(&l.dispatcher);
        entries.push(Entry {
            target: None,
            group: "Not in your config".into(),
            caps,
            d: Described { group: "Other", title, detail: detail.to_owned(), mono: true },
            options: String::new(),
            from: "Gone when Hyprland restarts".into(),
            marker: "",
            on: true,
            unread: false,
            warn: false,
        });
    }
    entries
}

fn group_order(group: &str) -> usize {
    GROUPS.iter().position(|g| *g == group).unwrap_or(if group.starts_with("Submap") {
        GROUPS.len()
    } else {
        GROUPS.len() + 1
    })
}

fn matches(e: &Entry, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || [&e.d.title, &e.d.detail, &e.group, &e.from, &e.marker.to_owned()]
            .iter()
            .any(|t| t.to_lowercase().contains(&q))
        || e.caps.join(" + ").to_lowercase().contains(&q)
}

/// The changes in Lituus's part that Hyprland's config would undo when it loads again: the
/// overridden binds of the file that `require`s Lituus's, after that line.
fn loads_too_early(state: &State) -> Vec<&Bind> {
    let Some((file, line)) = &state.loads else { return Vec::new() };
    state.config.binds.iter().filter(|b| state.over(b).is_some() && b.file == *file && b.line > *line).collect()
}

pub fn view(state: &State) -> ShortcutsView {
    let mut v = ShortcutsView::default();
    if !has_list(state) {
        if state.not_running {
            v.state = s("not-running");
            v.empty_title = s("Hyprland isn't running");
            v.empty_text = s("The shortcuts are Hyprland's. Open Lituus inside a Hyprland session.");
        } else {
            v.state = s("loading");
            v.empty_title = s(state.error.clone().unwrap_or_else(|| "Reading your shortcuts…".into()));
        }
        return v;
    }
    v.state = s("list");
    v.editable = state.editable();
    let mut entries = entries(state);
    let total = entries.len();
    let changed = entries.iter().filter(|e| !e.marker.is_empty()).count();
    let not_in_effect = entries.iter().filter(|e| e.warn).count();
    let mut sub = plural(total, "shortcut", "shortcuts");
    if changed > 0 {
        sub += &format!(" · {changed} changed in Lituus");
    }
    if not_in_effect > 0 {
        sub += &format!(" · {not_in_effect} not in effect");
    }
    v.sub = s(sub);
    let dir = state.dir();
    let config = if dir.join("hyprland.lua").exists() { dir.join("hyprland.lua") } else { dir.join("hyprland.conf") };
    let binds = &state.config.binds;
    v.hero = match main_key(binds) {
        Some((key, n)) => HeroInfo {
            icon: s("keyboard"),
            name: s(format!("{key} is your main key")),
            meta: s(format!("{n} of {} use it", plural(binds.len(), "shortcut", "shortcuts"))),
            tone: s("accent"),
            detail: s(shorten_home(&config.display().to_string())),
        },
        None => HeroInfo {
            icon: s("keyboard"),
            name: s("No shortcuts yet"),
            meta: s("Your Hyprland config has none"),
            tone: s("faint"),
            detail: s(shorten_home(&config.display().to_string())),
        },
    };
    // Lituus's part only works when the config loads it, after its shortcuts.
    let file = state.file();
    v.load_line = s(file.load_line());
    if !state.mine.is_empty() && state.loads.is_none() {
        v.not_loaded = true;
    } else if let (Some((f, line)), early) = (&state.loads, loads_too_early(state))
        && !early.is_empty()
    {
        let name = f.strip_prefix(&dir).map_or_else(|_| f.display().to_string(), |p| p.display().to_string());
        v.banner_title = s(format!(
            "{} here {} take effect",
            plural(early.len(), "change", "changes"),
            if early.len() == 1 { "doesn't" } else { "don't" }
        ));
        v.banner_text = s(format!(
            "{name} loads Lituus's file on line {line}, before your shortcuts, so your config's versions still run as well. \
             Move the line to the end of {name}."
        ));
    }
    v.message = s(state.message.clone().unwrap_or_default());
    entries.retain(|e| matches(e, &state.query));
    // Stable: in each group, in the config's order; going to workspaces before moving windows
    // to them, which a config's loop makes in turn.
    entries.sort_by_key(|e| (group_order(&e.group), e.d.title.starts_with("Move window to")));
    let mut rows = Vec::new();
    let mut last = None;
    for e in &entries {
        if last != Some(&e.group) {
            rows.push(ShortcutRow { label: s(e.group.to_uppercase()), ..Default::default() });
            last = Some(&e.group);
        }
        let shown_from = if e.marker.is_empty() { e.from.clone() } else { e.marker.to_owned() };
        rows.push(ShortcutRow {
            label: s(""),
            id: s(e.target.map(Target::id).unwrap_or_default()),
            keys: strings(e.caps.clone()),
            title: s(&e.d.title),
            detail: s(&e.d.detail),
            mono: e.d.mono,
            options: s(&e.options),
            from: s(&shown_from),
            marker: !e.marker.is_empty(),
            summary: s([e.d.detail.as_str(), &e.options, &shown_from]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")),
            on: e.on,
            can_restore: e.marker == CHANGED,
            can_delete: e.marker == ADDED,
            unread: e.unread,
            warn: e.warn,
        });
    }
    v.count = s(if state.query.trim().is_empty() {
        String::new()
    } else if entries.is_empty() {
        "No shortcut matches".to_owned()
    } else {
        plural(entries.len(), "shortcut matches", "shortcuts match")
    });
    v.rows = ModelRc::new(VecModel::from(rows));
    v.query = s(&state.query);
    if let Some(d) = &state.dialog {
        v.dialog = dialog_view(state, d);
    }
    if let Some(Mine::Added { bind, .. }) = state.deleting.and_then(|i| state.mine.get(i)) {
        v.deleting = s(format!("Delete {}?", caps(bind).join(" + ")));
        v.deleting_text = s(format!(
            "{} goes away and {} does nothing again. It was added in Lituus, so your config doesn't have it.",
            describe(bind).title,
            caps(bind).join(" + ")
        ));
    }
    v
}

/// The config's own text of a bind: its lines in the file.
fn snippet(b: &Bind) -> String {
    let text = std::fs::read_to_string(&b.file).unwrap_or_default();
    let lines: Vec<&str> =
        text.lines().skip(b.line.saturating_sub(1)).take(b.end_line.max(b.line) - b.line + 1).collect();
    // As indented in the file, less the indent they share.
    let indent =
        lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    lines.iter().map(|l| l.get(indent..).unwrap_or(l.trim())).collect::<Vec<_>>().join("\n")
}

fn dialog_view(state: &State, d: &Dialog) -> ShortcutDialog {
    let dir = state.dir();
    let (bind, config_bind) = match d.target {
        Target::Config(i) => {
            let b = &state.config.binds[i];
            (b.clone(), Some(b))
        }
        Target::Added(i) => match state.mine.get(i) {
            Some(Mine::Added { bind, .. }) => (bind.clone(), None),
            _ => return ShortcutDialog::default(),
        },
    };
    let f = &d.form;
    let mut v = ShortcutDialog { open: true, unread: d.unread && !d.replacing, ..Default::default() };
    v.keys = strings(caps(&bind));
    let over = config_bind.and_then(|b| state.over(b));
    let (word, tone) = match (d.target, over) {
        (Target::Added(_), _) => (ADDED, "accent"),
        (_, Some(Some(_))) => (CHANGED, "accent"),
        (_, Some(None)) => (TURNED_OFF, "faint"),
        _ if d.unread && !d.replacing => ("Lituus can't read what it does", "warn"),
        _ => ("From your config", "faint"),
    };
    v.word = s(word);
    v.word_tone = s(tone);
    v.title = s(if d.unread && !d.replacing {
        "Set in your config".to_owned()
    } else if d.replacing {
        "Replace the shortcut".to_owned()
    } else {
        describe(&f.bind(&bind)).title
    });
    // The actions: the config's own first when the dialog doesn't offer it.
    let mut kinds: Vec<String> = Vec::new();
    if let Some(keep) = &f.keep {
        kinds.push(format!("As in your config: {}", describe(&Bind { action: keep.clone(), ..bind.clone() }).title));
    }
    kinds.extend(KINDS.iter().map(|(_, label, _)| (*label).to_owned()));
    let offset = usize::from(f.keep.is_some());
    v.kinds = strings(kinds);
    v.kind = f.kind.map_or(0, |k| k + offset) as i32;
    v.field = s(match f.field() {
        Field::None => "",
        Field::Command => "command",
        Field::Workspace => "workspace",
        Field::Direction => "direction",
        Field::Special => "special",
    });
    v.follow_shown = f.kind.is_some_and(|k| KINDS[k].0 == "move-ws");
    v.command = s(&f.command);
    v.workspace = s(&f.workspace);
    v.follow = f.follow;
    v.directions = strings(DIRECTIONS.iter().map(|(label, _)| (*label).to_owned()).collect());
    v.direction = f.direction as i32;
    v.special = s(&f.special);
    v.description = s(&f.description);
    let flags = f.flags();
    v.locked = flags.locked;
    v.repeating = flags.repeating;
    v.release = flags.release;
    v.apps = flags.non_consuming;
    v.mouse = f.mouse();
    v.release_note = s(if v.mouse {
        "Not with a mouse drag"
    } else if flags.repeating {
        "Can't while it repeats"
    } else {
        "Instead of when the keys go down"
    });
    v.mouse_note = s("Not with a mouse drag");
    v.problem = s(f.problem().unwrap_or_default());
    // Where it comes from.
    match (d.target, config_bind) {
        (Target::Config(_), Some(b)) => {
            let place = from(b, &dir);
            if matches!(over, Some(Some(_))) {
                v.source = s(format!("{CHANGED} · {}", shorten_home(&state.file().path().display().to_string())));
                v.source_note = s(format!("Your config's version, {place}:"));
                v.can_restore = !d.unread;
            } else {
                v.source = s(format!("From your config · {}", shorten_home(&b.file.display().to_string())));
                v.source_note = s(place);
            }
            v.snippet = s(snippet(b));
        }
        _ => v.source = s(format!("{ADDED} · {}", shorten_home(&state.file().path().display().to_string()))),
    }
    v.use_on = d.use_on;
    v
}

/// Opens the dialog for a row.
fn open(state: &mut State, target: Target) {
    let (bind, unread, use_on) = match target {
        Target::Config(i) => {
            let Some(b) = state.config.binds.get(i) else { return };
            let over = state.over(b).cloned();
            let shown = match &over {
                Some(Some(new)) => new.clone(),
                _ => b.clone(),
            };
            (shown, b.action == Action::Unread && !matches!(over, Some(Some(_))), !matches!(over, Some(None)))
        }
        Target::Added(i) => match state.mine.get(i) {
            Some(Mine::Added { bind, on }) => (bind.clone(), false, *on),
            _ => return,
        },
    };
    state.dialog = Some(Dialog { target, form: Form::of(&bind), unread, replacing: false, use_on });
}

/// Saves the dialog.
fn save(weak: Option<&slint::Weak<MainWindow>>, state: &mut State) {
    let Some(d) = state.dialog.clone() else { return };
    if (!d.unread || d.replacing) && d.form.problem().is_some() {
        return;
    }
    state.dialog = None;
    match d.target {
        Target::Config(i) => {
            let Some(b) = state.config.binds.get(i).cloned() else { return };
            if d.unread && !d.replacing {
                // Kept, or turned off.
                let (mine, lua) = if d.use_on { without_over(state, &b) } else { with_over(state, &b, None) };
                if mine != state.mine {
                    change(weak, state, mine, lua);
                }
                return;
            }
            let new = d.form.bind(&b);
            // The config's own version again: nothing of Lituus's left.
            let same = new.action == b.action && new.flags == b.flags && new.description == b.description;
            let (mine, lua) = if same { without_over(state, &b) } else { with_over(state, &b, Some(new)) };
            change(weak, state, mine, lua);
        }
        Target::Added(i) => {
            let Some(Mine::Added { bind, on }) = state.mine.get(i).cloned() else { return };
            let new = d.form.bind(&bind);
            let mut mine = state.mine.clone();
            mine[i] = Mine::Added { bind: new.clone(), on };
            let lua = format!(
                "hl.unbind({}) {}",
                lua_string(&bind.keys()),
                if on { new.to_lua().unwrap_or_default() } else { String::new() }
            );
            change(weak, state, mine, Some(lua));
        }
    }
}

/// The row's switch: the config's bind or Lituus's own turned off, or on again.
fn toggle(weak: Option<&slint::Weak<MainWindow>>, state: &mut State, target: Target) {
    match target {
        Target::Config(i) => {
            let Some(b) = state.config.binds.get(i).cloned() else { return };
            let (mine, lua) = match state.over(&b) {
                Some(None) => without_over(state, &b),
                _ => with_over(state, &b, None),
            };
            change(weak, state, mine, lua);
        }
        Target::Added(i) => {
            let Some(Mine::Added { bind, on }) = state.mine.get(i).cloned() else { return };
            let mut mine = state.mine.clone();
            mine[i] = Mine::Added { bind: bind.clone(), on: !on };
            let lua =
                if on { format!("hl.unbind({})", lua_string(&bind.keys())) } else { bind.to_lua().unwrap_or_default() };
            change(weak, state, mine, Some(lua));
        }
    }
}

/// The dialog's form, changed by one of its controls.
fn edit(state: &mut State, verb: &str, arg: &str) {
    let Some(d) = &mut state.dialog else { return };
    let f = &mut d.form;
    match verb {
        "d-kind" => {
            let i: usize = arg.parse().unwrap_or(0);
            let offset = usize::from(f.keep.is_some());
            f.kind = if i < offset { None } else { Some(i - offset).filter(|k| *k < KINDS.len()) };
        }
        "d-command" => f.command = arg.to_owned(),
        "d-workspace" => f.workspace = arg.to_owned(),
        "d-special" => f.special = arg.to_owned(),
        "d-direction" => f.direction = arg.parse().unwrap_or(0),
        "d-follow" => f.follow = !f.follow,
        "d-description" => f.description = arg.to_owned(),
        "d-flag" => match arg {
            "locked" => f.flags.locked = !f.flags.locked,
            "repeating" => f.flags.repeating = !f.flags.repeating,
            "release" => f.flags.release = !f.flags.release,
            "apps" => f.flags.non_consuming = !f.flags.non_consuming,
            _ => {}
        },
        "d-use" => d.use_on = !d.use_on,
        "d-replace" => {
            d.replacing = true;
            f.kind = Some(0);
            f.keep = None;
        }
        _ => {}
    }
}

pub fn act(weak: &slint::Weak<MainWindow>, state: &mut State, verb: &str, arg: &str) {
    act_with(Some(weak), state, verb, arg);
}

fn act_with(weak: Option<&slint::Weak<MainWindow>>, state: &mut State, verb: &str, arg: &str) {
    let target = Target::parse(arg);
    match verb {
        "query" => state.query = arg.to_owned(),
        "refresh" => {
            if let Some(weak) = weak {
                refresh(weak);
            }
        }
        "open" if state.editable() => {
            if let Some(t) = target {
                open(state, t);
            }
        }
        "toggle" if state.editable() => {
            if let Some(t) = target {
                toggle(weak, state, t);
            }
        }
        "restore" => {
            if let Some(Target::Config(i)) = target
                && let Some(b) = state.config.binds.get(i).cloned()
            {
                let (mine, lua) = without_over(state, &b);
                change(weak, state, mine, lua);
            }
        }
        "delete" => {
            if let Some(Target::Added(i)) = target {
                state.deleting = Some(i);
            }
        }
        "delete-yes" => {
            if let Some(i) = state.deleting.take()
                && let Some(Mine::Added { bind, .. }) = state.mine.get(i).cloned()
            {
                let mut mine = state.mine.clone();
                mine.remove(i);
                change(weak, state, mine, Some(format!("hl.unbind({})", lua_string(&bind.keys()))));
            }
        }
        "delete-no" => state.deleting = None,
        "d-save" => save(weak, state),
        "d-cancel" => state.dialog = None,
        "d-restore" => {
            // The form takes the config's version; Save puts it back.
            if let Some(d) = &mut state.dialog
                && let Target::Config(i) = d.target
                && let Some(b) = state.config.binds.get(i)
            {
                d.form = Form::of(b);
            }
        }
        _ if verb.starts_with("d-") => edit(state, verb, arg),
        _ => {}
    }
}

thread_local! {
    /// The rows, and the dialog's lists of actions and directions, each kept as one model: the
    /// window renders again now and then (a network's signal, a device), and a new model would
    /// rebuild the row (or the open list) between a click's press and its release.
    static ROWS: Rc<VecModel<ShortcutRow>> = Rc::new(VecModel::default());
    static KINDS_SHOWN: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
    static DIRECTIONS_SHOWN: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
}

pub fn render(window: &MainWindow, state: &State) {
    if window.get_page() == "shortcuts" {
        let mut v = view(state);
        v.rows = ROWS.with(|m| kept(m, &v.rows));
        v.dialog.kinds = KINDS_SHOWN.with(|m| kept(m, &v.dialog.kinds));
        v.dialog.directions = DIRECTIONS_SHOWN.with(|m| kept(m, &v.dialog.directions));
        window.set_shortcuts(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lituus-shortcuts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn state_with(name: &str, config: &str, live: Option<&str>) -> (State, PathBuf) {
        let d = dir(name);
        std::fs::write(d.join("hyprland.lua"), config).unwrap();
        let mut state = State { hypr_dir: Some(d.clone()), ..Default::default() };
        let live = live.map(|l| serde_json::from_str(l).unwrap());
        let section = state.file().read_section(SECTION);
        loaded(&mut state, Ok((read_binds(&d), section, live, Hyprland::at(&d))));
        (state, d)
    }

    fn titles(v: &ShortcutsView) -> Vec<String> {
        use slint::Model;
        v.rows.iter().map(|r| if r.label.is_empty() { r.title.to_string() } else { format!("# {}", r.label) }).collect()
    }

    /// Hyprland 0.57's example config, in words and in groups.
    #[test]
    fn the_default_config_in_words() {
        let (state, d) = state_with(
            "default",
            include_str!("../../hypr/tests/fixtures/default.lua"),
            Some(include_str!("../../hypr/tests/fixtures/binds.json")),
        );
        let v = view(&state);
        assert_eq!(v.state, "list");
        // Its 48, and the one made with hyprctl eval.
        assert_eq!(v.sub, "49 shortcuts");
        assert_eq!(v.hero.name, "Super is your main key");
        assert_eq!(v.hero.meta, "38 of 48 shortcuts use it");
        let t = titles(&v);
        let at = |s: &str| t.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s}: {t:?}"));
        for (title, group) in [
            ("Open Kitty", "# APPS"),
            ("Open Dolphin", "# APPS"),
            ("Open the app launcher", "# APPS"),
            ("Close window", "# WINDOWS"),
            ("Float or tile window", "# WINDOWS"),
            ("Switch split direction", "# WINDOWS"),
            ("Move window by dragging", "# WINDOWS"),
            ("Go to workspace 10", "# WORKSPACES"),
            ("Move window to the scratchpad", "# WORKSPACES"),
            ("Show the scratchpad", "# WORKSPACES"),
            ("Go to the next workspace", "# WORKSPACES"),
            ("Focus the window on the left", "# FOCUS"),
            ("Turn the volume up", "# MEDIA KEYS"),
            ("Mute the microphone", "# MEDIA KEYS"),
            ("Make the screen brighter", "# MEDIA KEYS"),
            ("Previous track", "# MEDIA KEYS"),
            ("Exit Hyprland", "# SESSION"),
            ("Open Firefox", "# NOT IN YOUR CONFIG"),
        ] {
            let i = at(title);
            let label = t[..i].iter().rev().find(|x| x.starts_with("# ")).unwrap();
            assert_eq!(label, group, "{title}");
        }
        use slint::Model;
        let row = |title: &str| v.rows.iter().find(|r| r.title == title).unwrap();
        let ws10 = row("Go to workspace 10");
        assert_eq!(ws10.keys.iter().collect::<Vec<_>>(), ["Super", "0"]);
        assert_eq!(ws10.from, "hyprland.lua · line 288");
        let volume = row("Turn the volume up");
        assert_eq!(volume.keys.iter().collect::<Vec<_>>(), ["Volume up"]);
        assert_eq!(volume.options, "Repeats · On the lock screen");
        assert!(volume.mono && volume.detail.starts_with("wpctl set-volume"));
        assert_eq!(row("Move window by dragging").keys.iter().collect::<Vec<_>>(), ["Super", "Left button"]);
        assert_eq!(row("Open Firefox").keys.iter().collect::<Vec<_>>(), ["Super", "Shift", "B"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn search_and_what_isnt_in_effect() {
        let (mut state, d) = state_with(
            "search",
            "hl.bind(\"SUPER + Q\", hl.dsp.window.close())\nhl.bind(\"SUPER + T\", function() end)\n\
             if false then\n  hl.bind(\"SUPER + X\", hl.dsp.exit())\nend\n",
            Some(r#"[{"modmask": 64, "key": "Q"}, {"modmask": 64, "key": "T"}]"#),
        );
        let v = view(&state);
        assert_eq!(v.sub, "3 shortcuts · 1 not in effect");
        use slint::Model;
        let exit = v.rows.iter().find(|r| r.title == "Exit Hyprland").unwrap();
        assert!(exit.warn && exit.options == "Not in effect now");
        assert_eq!(exit.summary, "Not in effect now · hyprland.lua · line 4");
        let unread = v.rows.iter().find(|r| r.unread).unwrap();
        assert_eq!((unread.title.as_str(), unread.from.as_str()), ("Set in your config", "hyprland.lua · line 2"));
        state.query = "close".into();
        assert_eq!(titles(&view(&state)), ["# WINDOWS", "Close window"]);
        state.query = "super + t".into();
        assert_eq!(view(&state).count, "1 shortcut matches");
        state.query = "nothing like it".into();
        assert_eq!(view(&state).count, "No shortcut matches");
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn row(v: &ShortcutsView, title: &str) -> ShortcutRow {
        use slint::Model;
        v.rows.iter().find(|r| r.title == title).unwrap_or_else(|| panic!("{title}: {:?}", titles(v)))
    }

    /// Reads the page again, as after Hyprland applied the change (without the live list).
    fn reread(state: &mut State, d: &Path) {
        let query = std::mem::take(&mut state.query);
        let section = state.file().read_section(SECTION);
        loaded(state, Ok((read_binds(d), section, None, Hyprland::at(d))));
        state.query = query;
    }

    /// Changing a bind of the config: Lituus's part unbinds it and binds the new one, the row says
    /// so, and Back to your config's version takes it away again.
    #[test]
    fn changing_a_shortcut() {
        let config = "local mainMod = \"SUPER\"\nhl.bind(mainMod .. \" + D\", hl.dsp.exec_cmd(\"kitty\"))\n\
                      hl.bind(mainMod .. \" + J\", hl.dsp.layout(\"togglesplit\"))\nrequire(\"lituus\")\n";
        let (mut state, d) = state_with("change", config, None);
        let id = row(&view(&state), "Open Kitty").id.to_string();
        act_with(None, &mut state, "open", &id);
        let dv = view(&state).dialog;
        assert!(dv.open && !dv.unread);
        assert_eq!(
            (dv.field.as_str(), dv.command.as_str(), dv.word.as_str()),
            ("command", "kitty", "From your config")
        );
        assert_eq!(dv.snippet, "hl.bind(mainMod .. \" + D\", hl.dsp.exec_cmd(\"kitty\"))");
        act_with(None, &mut state, "d-command", "fuzzel");
        act_with(None, &mut state, "d-flag", "locked");
        act_with(None, &mut state, "d-description", "Open the app launcher");
        act_with(None, &mut state, "d-save", "");
        assert!(state.dialog.is_none());
        let written = state.file().read_section(SECTION).unwrap();
        assert!(
            written.contains("hl.unbind(\"SUPER + D\")\nhl.bind(\"SUPER + D\", hl.dsp.exec_cmd(\"fuzzel\"), { locked = true, description = \"Open the app launcher\" })"),
            "{written}"
        );
        reread(&mut state, &d);
        let v = view(&state);
        let r = row(&v, "Open the app launcher");
        assert_eq!((r.from.as_str(), r.marker, r.can_restore), ("Changed in Lituus", true, true));
        assert_eq!(v.sub, "2 shortcuts · 1 changed in Lituus");
        // Lituus's file loads after the config's binds: no banner.
        assert_eq!(v.banner_title, "");
        act_with(None, &mut state, "restore", &r.id);
        reread(&mut state, &d);
        assert!(state.mine.is_empty());
        assert_eq!(row(&view(&state), "Open Kitty").from, "hyprland.lua · line 2");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The switch turns a bind off, and on again; with Lituus's file loaded before the binds, the
    /// banner says the change doesn't take effect.
    #[test]
    fn turning_one_off_and_the_load_order() {
        let config = "require(\"lituus\")\nhl.bind(\"SUPER + J\", hl.dsp.layout(\"togglesplit\"))\n";
        let (mut state, d) = state_with("off", config, None);
        let id = row(&view(&state), "Switch split direction").id.to_string();
        act_with(None, &mut state, "toggle", &id);
        reread(&mut state, &d);
        let v = view(&state);
        let r = row(&v, "Switch split direction");
        assert_eq!((r.on, r.from.as_str()), (false, "Turned off in Lituus"));
        assert_eq!(v.banner_title, "1 change here doesn't take effect");
        assert!(v.banner_text.starts_with("hyprland.lua loads Lituus's file on line 1"), "{}", v.banner_text);
        act_with(None, &mut state, "toggle", &id);
        reread(&mut state, &d);
        assert!(row(&view(&state), "Switch split direction").on);
        assert!(state.mine.is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A bind made by Lua Lituus doesn't read: kept, turned off, or replaced by a new one.
    #[test]
    fn a_bind_lituus_cant_read() {
        let config = "hl.bind(\"SUPER + T\", function() end)\n";
        let (mut state, d) = state_with("unread", config, None);
        let id = row(&view(&state), "Set in your config").id.to_string();
        act_with(None, &mut state, "open", &id);
        assert!(view(&state).dialog.unread);
        act_with(None, &mut state, "d-replace", "");
        let dv = view(&state).dialog;
        assert!(!dv.unread && dv.title == "Replace the shortcut");
        // A command to run is needed before it saves.
        act_with(None, &mut state, "d-save", "");
        assert!(state.dialog.is_some());
        act_with(None, &mut state, "d-command", "foot");
        act_with(None, &mut state, "d-save", "");
        reread(&mut state, &d);
        // Not loaded by the config: the page says so.
        let v = view(&state);
        assert!(v.not_loaded);
        assert_eq!(row(&v, "Open Foot").from, "Changed in Lituus");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn not_running() {
        let mut state = State::default();
        loaded(&mut state, Err(lituus_hypr::Error::NotRunning));
        assert_eq!(view(&state).state, "not-running");
        assert!(keys(&state).is_empty());
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name("XF86AudioMicMute"), "Mic mute");
        assert_eq!(key_name("code:10"), "Key 10");
        assert_eq!(key_name("q"), "Q");
        assert_eq!(key_name("Return"), "Enter");
        assert_eq!(key_name("XF86Launch1"), "Launch1");
        assert_eq!(command("nautilus --new-window").title, "Open Files");
        assert_eq!(command("grim -g \"$(slurp)\" - | wl-copy").group, "Screenshots");
        assert_eq!(command("notify-send hi | tee x").title, "Run a command");
    }
}
