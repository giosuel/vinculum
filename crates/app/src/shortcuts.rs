//! The Shortcuts page (Input › Shortcuts): the binds in the user's Hyprland config, in groups and
//! in words ("Close window", "Go to workspace 3"), with the keys and the file and line each comes
//! from. Hyprland's own list (`hyprctl binds`) says which are in effect, and has the binds made
//! while it runs. It only shows them for now; changing them comes next.

use std::path::{Path, PathBuf};

use lituus_hypr::{Action, Bind, Hyprland, LiveBind, LuaValue as Value, config_dir, modifier_bit, read_binds};
use slint::{ModelRc, SharedString, VecModel};

use crate::{HeroInfo, Hint, MainWindow, ShortcutRow, ShortcutsView, update};

#[derive(Default)]
pub struct State {
    pub not_running: bool,
    /// Read at least once.
    read: bool,
    pub error: Option<String>,
    binds: Vec<Bind>,
    /// What Hyprland has now; `None` when it couldn't say.
    live: Option<Vec<LiveBind>>,
    pub query: String,
    /// Where Hyprland's config is, if not the usual place (tests).
    hypr_dir: Option<PathBuf>,
}

impl State {
    fn dir(&self) -> PathBuf {
        self.hypr_dir.clone().unwrap_or_else(config_dir)
    }
}

/// What was read, on a thread.
pub type Read = Result<(Vec<Bind>, Option<Vec<LiveBind>>), lituus_hypr::Error>;

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
    Ok((read_binds(dir), h.binds().ok()))
}

pub fn loaded(state: &mut State, read: Read) {
    state.read = true;
    match read {
        Ok((binds, live)) => {
            state.binds = binds;
            state.live = live;
            state.not_running = false;
            state.error = None;
        }
        Err(lituus_hypr::Error::NotRunning) => state.not_running = true,
        Err(e) => state.error = Some(e.to_string()),
    }
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
    if has_list(state) { state.binds.len().to_string() } else { String::new() }
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

struct Entry {
    group: String,
    caps: Vec<String>,
    d: Described,
    options: String,
    from: String,
    unread: bool,
    warn: bool,
}

fn entries(state: &State) -> Vec<Entry> {
    let dir = state.dir();
    let mut entries: Vec<Entry> = state
        .binds
        .iter()
        .map(|b| {
            let d = describe(b);
            let mut options: Vec<&str> = options(b);
            let in_effect = state.live.as_ref().is_none_or(|live| live.iter().any(|l| l.matches(b)));
            if !in_effect {
                options.push("Not in effect now");
            }
            Entry {
                group: if b.submap.is_empty() { d.group.to_owned() } else { format!("Submap {}", b.submap) },
                caps: caps(b),
                unread: b.action == Action::Unread,
                d,
                options: options.join(" · "),
                from: from(b, &dir),
                warn: !in_effect,
            }
        })
        .collect();
    // What Hyprland has that the config doesn't: made while it runs (hyprctl eval, a plugin).
    for l in state.live.iter().flatten().filter(|l| !state.binds.iter().any(|b| l.matches(b))) {
        let mods = MODMASK.iter().filter(|(bit, _)| l.modmask & bit != 0).map(|(_, m)| key_name(m));
        let caps = mods.chain(std::iter::once(key_name(&l.key))).collect();
        let title =
            if l.description.is_empty() { "Made while Hyprland runs".to_owned() } else { l.description.clone() };
        let detail =
            l.dispatcher.strip_prefix("HL.Dispatcher(").and_then(|d| d.strip_suffix(')')).unwrap_or(&l.dispatcher);
        entries.push(Entry {
            group: "Not in your config".into(),
            caps,
            d: Described { group: "Other", title, detail: detail.to_owned(), mono: true },
            options: String::new(),
            from: "Gone when Hyprland restarts".into(),
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
        || [&e.d.title, &e.d.detail, &e.group, &e.from].iter().any(|t| t.to_lowercase().contains(&q))
        || e.caps.join(" + ").to_lowercase().contains(&q)
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
    let mut entries = entries(state);
    let total = entries.len();
    let not_in_effect = entries.iter().filter(|e| e.warn).count();
    let mut sub = plural(total, "shortcut", "shortcuts");
    if not_in_effect > 0 {
        sub += &format!(" · {not_in_effect} not in effect");
    }
    v.sub = s(sub);
    let dir = state.dir();
    let config = if dir.join("hyprland.lua").exists() { dir.join("hyprland.lua") } else { dir.join("hyprland.conf") };
    v.hero = match main_key(&state.binds) {
        Some((key, n)) => HeroInfo {
            icon: s("keyboard"),
            name: s(format!("{key} is your main key")),
            meta: s(format!("{n} of {} use it", plural(state.binds.len(), "shortcut", "shortcuts"))),
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
        rows.push(ShortcutRow {
            label: s(""),
            keys: strings(e.caps.clone()),
            title: s(&e.d.title),
            detail: s(&e.d.detail),
            mono: e.d.mono,
            options: s(&e.options),
            from: s(&e.from),
            summary: s([e.d.detail.as_str(), &e.options, &e.from]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")),
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
    v
}

pub fn act(weak: &slint::Weak<MainWindow>, state: &mut State, verb: &str, arg: &str) {
    match verb {
        "query" => state.query = arg.to_owned(),
        "refresh" => refresh(weak),
        _ => {}
    }
}

pub fn render(window: &MainWindow, state: &State) {
    if window.get_page() == "shortcuts" {
        window.set_shortcuts(view(state));
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
        loaded(&mut state, Ok((read_binds(&d), live)));
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
