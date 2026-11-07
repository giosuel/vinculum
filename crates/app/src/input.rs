//! The Input pages, Keyboard and Mouse & touchpad: Hyprland's `input` settings and per-device
//! blocks (lituus-hypr). A change applies at once and is saved to the Input part of
//! ~/.config/hypr/lituus.lua right away; Revert puts the last change back. Hyprland doesn't say
//! which pointer is a mouse and which a touchpad, so that comes from the kernel (udev's tags).

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use lituus_hypr::{
    ConfigFile, Device, DeviceSettings, Devices, Hyprland, InputSettings, Layout, MouseSettings, OPTIONS,
    TouchpadSettings, Value, config_dir, option_name,
};
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::xkb::{self, Registry};
use crate::{
    HeroInfo, Hint, InputBar, InputRow, InputSection, KeyboardView, LayoutRow, MainWindow, MouseView, PointerSetting,
    update,
};

/// The name of the Input part of Lituus's file.
const SECTION: &str = "Input";
/// xkb switches between at most four layouts.
pub const MAX_LAYOUTS: usize = 4;
/// The most layouts the add dialog lists at once.
const RESULTS: usize = 60;

/// What a pointer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Touchpad,
    /// A trackpoint: set up like a mouse.
    Stick,
    /// The pointer a keyboard or its receiver has besides its keys.
    KeyboardPart,
}

impl Kind {
    fn touchpad(self) -> bool {
        self == Self::Touchpad
    }
}

/// A pointer Hyprland has, with what the kernel says about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pointer {
    /// Hyprland's name, which its device block takes.
    pub name: String,
    /// The kernel's name for it ("Logitech MX Master 3S"), or one made from Hyprland's.
    pub label: String,
    pub kind: Option<Kind>,
    /// "USB", "Bluetooth", "built in", or "" when unknown.
    pub connection: String,
    /// The Bluetooth address, when it's connected over Bluetooth.
    pub address: String,
    pub receiver: bool,
}

impl Pointer {
    fn kind(&self) -> Kind {
        self.kind.unwrap_or(Kind::Mouse)
    }
}

/// A device as the kernel lists it in /proc/bus/input/devices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kernel {
    pub name: String,
    pub bus: u16,
    /// The Bluetooth address for a Bluetooth device.
    pub uniq: String,
    /// Its event node's number, for udev's data on it.
    pub event: Option<u32>,
    /// It has a mouse node (`mouse0`), so it's a pointer.
    pub pointer: bool,
}

/// /proc/bus/input/devices.
pub fn kernel_devices(text: &str) -> Vec<Kernel> {
    text.split("\n\n")
        .filter_map(|block| {
            let mut k = Kernel::default();
            for line in block.lines() {
                if let Some(rest) = line.strip_prefix("I: ") {
                    let bus = rest.split_whitespace().find_map(|f| f.strip_prefix("Bus="));
                    k.bus = bus.and_then(|b| u16::from_str_radix(b, 16).ok()).unwrap_or(0);
                } else if let Some(name) = line.strip_prefix("N: Name=") {
                    k.name = name.trim_matches('"').to_owned();
                } else if let Some(uniq) = line.strip_prefix("U: Uniq=") {
                    k.uniq = uniq.trim().to_owned();
                } else if let Some(handlers) = line.strip_prefix("H: Handlers=") {
                    for h in handlers.split_whitespace() {
                        if let Some(n) = h.strip_prefix("event").and_then(|n| n.parse().ok()) {
                            k.event = Some(n);
                        }
                        k.pointer |= h.starts_with("mouse");
                    }
                }
            }
            (!k.name.is_empty()).then_some(k)
        })
        .collect()
}

/// Hyprland's name for a device the kernel calls `name`.
pub fn hypr_name(name: &str) -> String {
    name.chars().map(|c| if c == ' ' || c == '\n' || c == ',' { '-' } else { c.to_ascii_lowercase() }).collect()
}

/// What udev's tags on an event node (`E:ID_INPUT_TOUCHPAD=1` lines) say it is.
fn udev_kind(data: &str) -> Option<Kind> {
    let tag = |t: &str| data.lines().any(|l| l.trim() == format!("E:{t}=1"));
    if tag("ID_INPUT_TOUCHPAD") {
        Some(Kind::Touchpad)
    } else if tag("ID_INPUT_POINTINGSTICK") {
        Some(Kind::Stick)
    } else if tag("ID_INPUT_MOUSE") || tag("ID_INPUT_TRACKBALL") {
        Some(Kind::Mouse)
    } else {
        None
    }
}

/// Tells what each of Hyprland's pointers is: udev's tag on the kernel's device of that name
/// (what libinput goes by too), else the name; a pointer that's also one of the keyboards (a
/// receiver, a keyboard's touch strip) is a keyboard's part unless udev calls it a mouse.
pub fn classify(devices: &Devices, kernel: &[Kernel], udev: &dyn Fn(u32) -> Option<String>) -> Vec<Pointer> {
    devices
        .mice
        .iter()
        .map(|p| {
            // "-1" and on for the second device of a name.
            let base =
                p.name.rsplit_once('-').filter(|(_, n)| n.parse::<u32>().is_ok()).map_or(p.name.as_str(), |(b, _)| b);
            let named: Vec<&Kernel> =
                kernel.iter().filter(|k| hypr_name(&k.name) == p.name || hypr_name(&k.name) == base).collect();
            let k = named.iter().find(|k| k.pointer).or(named.first()).copied();
            let tagged = k.and_then(|k| k.event).and_then(udev).as_deref().and_then(udev_kind);
            let lower = p.name.as_str();
            let also_keys = devices.keyboards.iter().any(|kb| kb.name == p.name);
            let receiver = lower.contains("receiver") || lower.contains("dongle");
            let kind = if tagged == Some(Kind::Touchpad)
                || ["touchpad", "trackpad", "clickpad", "glidepoint"].iter().any(|w| lower.contains(w))
            {
                Kind::Touchpad
            } else if tagged == Some(Kind::Stick) || lower.contains("trackpoint") || lower.contains("pointing-stick") {
                Kind::Stick
            } else if (also_keys || lower.ends_with("-keyboard") || lower.contains("consumer-control"))
                && (tagged.is_none() || receiver || lower.contains("keyboard"))
            {
                Kind::KeyboardPart
            } else {
                Kind::Mouse
            };
            let connection = match k.map(|k| k.bus) {
                Some(0x03) => "USB",
                Some(0x05) => "Bluetooth",
                Some(0x11 | 0x18 | 0x19 | 0x1d) => "built in",
                Some(0x06) => "virtual",
                _ => "",
            };
            Pointer {
                name: p.name.clone(),
                label: k.map_or_else(|| pretty(&p.name), |k| k.name.clone()),
                kind: Some(kind),
                connection: connection.to_owned(),
                address: k.filter(|k| k.bus == 0x05).map(|k| k.uniq.to_uppercase()).unwrap_or_default(),
                receiver,
            }
        })
        .collect()
}

/// "Logitech MX Master 3S" from "logitech-mx-master-3s", when the kernel doesn't say.
fn pretty(name: &str) -> String {
    const UPPER: [&str; 5] = ["mx", "usb", "hid", "rf", "bt"];
    name.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            if UPPER.contains(&w) || (w.len() <= 6 && w.chars().any(|c| c.is_ascii_digit())) {
                w.to_uppercase()
            } else {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// What the Mouse & touchpad page has selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selected {
    #[default]
    Mice,
    Touchpads,
    Device(String),
}

impl Selected {
    fn id(&self) -> String {
        match self {
            Self::Mice => "mouse".into(),
            Self::Touchpads => "touchpad".into(),
            Self::Device(name) => format!("dev:{name}"),
        }
    }

    fn from_id(id: &str) -> Self {
        match id {
            "touchpad" => Self::Touchpads,
            id => id.strip_prefix("dev:").map_or(Self::Mice, |n| Self::Device(n.to_owned())),
        }
    }
}

/// What the Input pages read from Hyprland, on a thread.
pub struct Read {
    hypr: Hyprland,
    lua: bool,
    devices: Devices,
    /// `input`'s options as Hyprland has them now.
    options: Vec<(String, Value)>,
    pointers: Vec<Pointer>,
}

/// A change for the thread that applies and saves: the settings, and the devices whose own
/// settings went, which Hyprland keeps until it reads its config again, so they're sent the
/// defaults.
struct Job {
    /// `None` saves only (in tests).
    hypr: Option<Hyprland>,
    lua: bool,
    file: ConfigFile,
    settings: InputSettings,
    reset: Vec<Device>,
}

/// What came of a job: the text written, or why not; and the devices afterwards.
type Done = (Result<String, String>, Option<Devices>);

#[derive(Default)]
pub struct State {
    hypr: Option<Hyprland>,
    pub not_running: bool,
    /// Hyprland has been read at least once.
    read: bool,
    pub error: Option<String>,
    lua: bool,
    file: Option<ConfigFile>,
    /// The user's config loads Lituus's file.
    loaded: bool,
    /// The settings as Hyprland has them, with the changes made here.
    pub settings: InputSettings,
    /// The Input part of the file, as last read or written.
    saved: Option<String>,
    devices: Devices,
    pointers: Vec<Pointer>,
    registry: Option<Registry>,
    /// The last change: what it did ("Repeat delay applied"), and the settings before it.
    last: Option<(String, InputSettings)>,
    /// The last change was applied and saved.
    applied: bool,
    reverted: bool,
    pub layout: usize,
    pub selected: Selected,
    pub add_open: bool,
    add_query: String,
    pub own_open: bool,
    /// Devices whose own settings were dropped since Hyprland last read its config.
    reset: Vec<String>,
    worker: Option<mpsc::Sender<Job>>,
    /// Where Hyprland's config and xkeyboard-config's list are, if not the usual places (tests).
    hypr_dir: Option<PathBuf>,
    registry_path: Option<PathBuf>,
}

impl State {
    fn registry(&mut self) -> &Registry {
        let path = self.registry_path.clone();
        self.registry.get_or_insert_with(|| path.map_or_else(Registry::load, |p| Registry::read(&p)))
    }

    /// The text of the file's Input part for the settings now.
    fn text(&self) -> String {
        self.settings.file(self.lua)
    }

    fn file_path(&self) -> String {
        self.file
            .as_ref()
            .map_or_else(|| "~/.config/hypr/lituus.lua".to_owned(), |f| shorten_home(&f.path().display().to_string()))
    }

    /// The pointers of a kind (mice take sticks and keyboards' parts along).
    fn of_kind(&self, touchpad: bool) -> impl Iterator<Item = &Pointer> {
        self.pointers.iter().filter(move |p| p.kind().touchpad() == touchpad)
    }

    fn own(&self, name: &str) -> Option<&Device> {
        self.settings.devices.iter().find(|d| d.name == name)
    }

    /// The list on the Mouse & touchpad page, in order: the defaults, devices with their own
    /// settings, the rest.
    fn rows(&self) -> Vec<Selected> {
        let mut rows = vec![Selected::Mice, Selected::Touchpads];
        rows.extend(self.settings.devices.iter().map(|d| Selected::Device(d.name.clone())));
        rows.extend(
            self.pointers.iter().filter(|p| self.own(&p.name).is_none()).map(|p| Selected::Device(p.name.clone())),
        );
        rows
    }
}

/// Reads Hyprland's devices and input settings (on a thread: the socket may take a moment).
pub fn refresh(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    std::thread::spawn(move || {
        let read = read_hyprland();
        let _ = weak.clone().upgrade_in_event_loop(move |_| update(&weak, move |app| loaded(&mut app.input, read)));
    });
}

fn read_hyprland() -> Result<Read, lituus_hypr::Error> {
    let h = Hyprland::from_env()?;
    let lua = h.version().map(|v| v.lua()).unwrap_or(true);
    let devices = h.devices()?;
    let mut options = Vec::new();
    for o in OPTIONS {
        if let Some(v) = h.option(&option_name(o))? {
            options.push((o.to_owned(), v));
        }
    }
    let pointers = pointers(&devices);
    Ok(Read { hypr: h, lua, devices, options, pointers })
}

/// The pointers, told apart with what the kernel and udev say on this machine.
fn pointers(devices: &Devices) -> Vec<Pointer> {
    let kernel = kernel_devices(&std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default());
    // The event nodes' numbers are 13:64 and on.
    let udev = |event: u32| std::fs::read_to_string(format!("/run/udev/data/c13:{}", 64 + event)).ok();
    classify(devices, &kernel, &udev)
}

fn loaded(state: &mut State, read: Result<Read, lituus_hypr::Error>) {
    match read {
        Ok(read) => {
            let file = ConfigFile::new(state.hypr_dir.clone().unwrap_or_else(config_dir), read.lua);
            state.loaded = file.loaded();
            state.saved = file.read_section(SECTION);
            // The devices' own settings are only in the file: Hyprland doesn't report them.
            let written = state.saved.as_deref().map(|t| InputSettings::parse(t, read.lua)).unwrap_or_default();
            state.settings = InputSettings::from_fields(&read.options, &[]);
            state.settings.devices = written.devices;
            state.file = Some(file);
            state.lua = read.lua;
            state.hypr = Some(read.hypr);
            state.devices = read.devices;
            state.pointers = read.pointers;
            state.not_running = false;
            state.read = true;
            state.layout = state.layout.min(state.settings.keyboard.layouts.len().saturating_sub(1));
            if !state.rows().contains(&state.selected) {
                state.selected = Selected::Mice;
            }
        }
        Err(lituus_hypr::Error::NotRunning) => {
            state.not_running = true;
            state.read = true;
        }
        Err(e) => state.error = Some(e.to_string()),
    }
}

/// Applies the settings live and saves them, on the worker thread (one at a time, and only the
/// latest of the ones waiting: a dragged slider sends many).
fn commit(weak: &slint::Weak<MainWindow>, state: &mut State) {
    let Some(file) = state.file.clone() else { return };
    let hypr = state.hypr.clone();
    let reset = state
        .reset
        .iter()
        .filter(|name| state.own(name).is_none())
        .map(|name| {
            let touchpad = state.pointers.iter().find(|p| p.name == *name).is_some_and(|p| p.kind().touchpad());
            Device { name: name.clone(), settings: defaults(&state.settings, touchpad) }
        })
        .collect();
    let job = Job { hypr, lua: state.lua, file, settings: state.settings.clone(), reset };
    if cfg!(test) {
        let done = run(&job);
        finish(state, done);
        return;
    }
    let worker = state.worker.get_or_insert_with(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let weak = weak.clone();
        std::thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let done = run(&job);
                let w = weak.clone();
                let _ = weak.upgrade_in_event_loop(move |_| update(&w, move |app| finish(&mut app.input, done)));
            }
        });
        tx
    });
    let _ = worker.send(job);
}

/// Sends the settings to Hyprland and writes the file's Input part.
fn run(job: &Job) -> Done {
    let text = job.settings.file(job.lua);
    let mut error = None;
    if let Some(h) = &job.hypr
        && job.lua
    {
        let reset = job.reset.iter().map(|d| InputSettings { devices: vec![d.clone()], ..job.settings.clone() });
        let mut statements = job.settings.lua_live();
        statements.extend(reset.flat_map(|s| s.lua_live().into_iter().skip(1)));
        for s in statements {
            if let Err(e) = h.eval(&s) {
                error = Some(format!("Hyprland didn't take the change: {e}"));
            }
        }
    } else if let Some(h) = &job.hypr {
        let devices = job.settings.devices.iter().chain(&job.reset).cloned().collect();
        let all = InputSettings { devices, ..job.settings.clone() };
        for (k, v) in all.hyprlang_live() {
            if let Err(e) = h.keyword(&k, &v) {
                error = Some(format!("Hyprland didn't take {k}: {e}"));
            }
        }
    }
    let written = match job.file.write_section(SECTION, &text) {
        Ok(()) => Ok(text),
        Err(e) => Err(format!("Couldn't write {}: {e}", job.file.path().display())),
    };
    // Hyprland sets the keyboards up again on its next frame.
    let devices = job.hypr.as_ref().and_then(|h| {
        std::thread::sleep(std::time::Duration::from_millis(150));
        h.devices().ok()
    });
    (
        match (written, error) {
            (Ok(_), Some(e)) => Err(e),
            (w, _) => w,
        },
        devices,
    )
}

fn finish(state: &mut State, (written, devices): Done) {
    state.applied = true;
    match written {
        Ok(text) => {
            state.saved = Some(text);
            state.error = None;
        }
        Err(e) => {
            // Saved or not, the file says what it says now.
            state.saved = state.file.as_ref().and_then(|f| f.read_section(SECTION));
            state.error = Some(e);
        }
    }
    if let Some(d) = devices {
        state.devices = d;
    }
}

/// The settings a device of a kind gets from the defaults.
fn defaults(s: &InputSettings, touchpad: bool) -> DeviceSettings {
    if touchpad {
        DeviceSettings::Touchpad(s.mouse.sensitivity, s.touchpad.clone())
    } else {
        DeviceSettings::Mouse(s.mouse.clone())
    }
}

/// Makes a change: `what` says what it did, for the bar ("Repeat delay applied"). Changes with
/// the same `what` in a row (a dragged slider) are one change for Revert.
fn change(weak: &slint::Weak<MainWindow>, state: &mut State, what: String, edit: impl FnOnce(&mut InputSettings)) {
    let before = state.settings.clone();
    edit(&mut state.settings);
    if state.settings == before {
        return;
    }
    let before = match state.last.take() {
        Some((w, b)) if w == what => b,
        _ => before,
    };
    state.last = Some((what, before));
    state.applied = false;
    state.reverted = false;
    commit(weak, state);
}

/// Everything the Input pages do: (verb, argument, value).
pub fn act(weak: &slint::Weak<MainWindow>, state: &mut State, verb: &str, arg: &str, value: f32) {
    let index = arg.parse::<usize>().unwrap_or(0);
    let value = f64::from(value);
    match verb {
        "refresh" => refresh(weak),
        "switch-now" => {
            if let Some(h) = state.hypr.clone() {
                match h.switch_layout().and_then(|()| h.devices()) {
                    Ok(d) => state.devices = d,
                    Err(e) => state.error = Some(format!("Couldn't switch the layout: {e}")),
                }
            }
        }
        "revert" => {
            if let Some((_, before)) = state.last.take() {
                // Devices given their own settings by the change lose them again.
                for d in &state.settings.devices {
                    if !before.devices.iter().any(|b| b.name == d.name) {
                        state.reset.push(d.name.clone());
                    }
                }
                state.settings = before;
                state.applied = false;
                state.reverted = true;
                commit(weak, state);
            }
        }
        "save" => commit(weak, state),
        // Keyboard.
        "layout-select" => state.layout = index.min(state.settings.keyboard.layouts.len().saturating_sub(1)),
        "layout-move" => {
            let (from, to) = arg.split_once(':').unwrap_or(("0", "0"));
            let (from, to) = (from.parse().unwrap_or(0), to.parse().unwrap_or(0));
            move_layout(weak, state, from, to);
        }
        "layout-up" | "layout-down" => {
            let from = state.layout;
            let to = if verb == "layout-up" { from.saturating_sub(1) } else { from + 1 };
            move_layout(weak, state, from, to);
        }
        "layout-remove" | "layout-remove-selected" => {
            let i = if verb == "layout-remove" { index } else { state.layout };
            let layouts = &state.settings.keyboard.layouts;
            // One layout always stays.
            if layouts.len() > 1 && i < layouts.len() {
                let layout = state.settings.keyboard.layouts[i].clone();
                let name = describe(state, &layout);
                change(weak, state, format!("{name} removed"), |s| _ = s.keyboard.layouts.remove(i));
                state.layout = state.layout.min(state.settings.keyboard.layouts.len() - 1);
            }
        }
        "add-open" => {
            state.add_open = state.settings.keyboard.layouts.len() < MAX_LAYOUTS;
            state.add_query.clear();
        }
        "add-close" => state.add_open = false,
        "add-query" => state.add_query = arg.to_owned(),
        "add" => {
            state.registry();
            let found = results(state);
            if let Some(l) = found.get(index).cloned()
                && state.settings.keyboard.layouts.len() < MAX_LAYOUTS
            {
                let name = describe(state, &l);
                change(weak, state, format!("{name} added"), |s| s.keyboard.layouts.push(l));
                state.layout = state.settings.keyboard.layouts.len() - 1;
                state.add_open = false;
            }
        }
        "switch-keys" => change(weak, state, "Switch keys applied".into(), |s| {
            s.keyboard.options = xkb::SWITCH.set(&s.keyboard.options, index)
        }),
        "caps" => change(weak, state, "Caps Lock applied".into(), |s| {
            s.keyboard.options = xkb::CAPS.set(&s.keyboard.options, index)
        }),
        "compose" => change(weak, state, "Compose key applied".into(), |s| {
            s.keyboard.options = xkb::COMPOSE.set(&s.keyboard.options, index)
        }),
        "numlock" => {
            change(weak, state, "Num Lock at login applied".into(), |s| s.keyboard.numlock = !s.keyboard.numlock)
        }
        "repeat-delay" => {
            let ms = ((value / 10.0).round() * 10.0).clamp(100.0, 1000.0) as i64;
            change(weak, state, "Repeat delay applied".into(), |s| s.keyboard.repeat_delay = ms);
        }
        "repeat-rate" => {
            let rate = value.round().clamp(5.0, 80.0) as i64;
            change(weak, state, "Repeat rate applied".into(), |s| s.keyboard.repeat_rate = rate);
        }
        // Mouse & touchpad.
        "select" => state.selected = Selected::from_id(arg),
        "own-open" => state.own_open = true,
        "own-close" => state.own_open = false,
        "own" => {
            let name = if arg.is_empty() {
                match &state.selected {
                    Selected::Device(n) => n.clone(),
                    _ => return,
                }
            } else {
                arg.to_owned()
            };
            let Some(p) = state.pointers.iter().find(|p| p.name == name).cloned() else { return };
            if state.own(&name).is_none() {
                let settings = defaults(&state.settings, p.kind().touchpad());
                change(weak, state, format!("{} has its own settings", p.label), |s| {
                    s.devices.push(Device { name: name.clone(), settings })
                });
                state.reset.retain(|r| *r != name);
            }
            state.selected = Selected::Device(name);
            state.own_open = false;
        }
        "defaults" => {
            if let Selected::Device(name) = state.selected.clone() {
                let label = label_of(state, &name);
                change(weak, state, format!("{label} uses the defaults"), |s| s.devices.retain(|d| d.name != name));
                state.reset.push(name);
            }
        }
        _ => pointer_setting(weak, state, verb, index, value),
    }
}

/// A change to one of the selected entry's settings.
fn pointer_setting(weak: &slint::Weak<MainWindow>, state: &mut State, verb: &str, index: usize, value: f64) {
    let selected = state.selected.clone();
    let (what, target) = match &selected {
        Selected::Mice => ("", None),
        Selected::Touchpads => ("", None),
        Selected::Device(name) => (name.as_str(), state.own(name).map(|_| name.clone())),
    };
    if !what.is_empty() && target.is_none() {
        // A device on the defaults has none of its own to change.
        return;
    }
    let label = match verb {
        "speed" => "Speed",
        "accel" => "Acceleration",
        "natural" => "Natural scrolling",
        "scroll-factor" => "Scroll speed",
        "left-handed" => "Left-handed",
        "tap" => "Tap to click",
        "dwt" => "Disable while typing",
        "scroll-method" => "Scroll with",
        "click-method" => "Click with",
        _ => return,
    };
    // Speed, acceleration and left-handed are one setting for mice and touchpads.
    let shared = matches!(verb, "speed" | "accel" | "left-handed");
    let what = match &target {
        Some(name) => format!("{label} for {} applied", label_of(state, name)),
        None if shared => format!("{label} for every mouse and touchpad applied"),
        None if selected == Selected::Touchpads => format!("{label} for every touchpad applied"),
        None => format!("{label} for every mouse applied"),
    };
    let speed = (value * 100.0).round().clamp(-100.0, 100.0) / 100.0;
    let factor = (value * 10.0).round().clamp(1.0, 30.0) / 10.0;
    let touchpads = selected == Selected::Touchpads;
    change(weak, state, what, |s| {
        let (mouse, touchpad): (Option<&mut MouseSettings>, Option<(&mut f64, &mut TouchpadSettings)>) = match &target {
            Some(name) => match s.devices.iter_mut().find(|d| d.name == *name).map(|d| &mut d.settings) {
                Some(DeviceSettings::Mouse(m)) => (Some(m), None),
                Some(DeviceSettings::Touchpad(speed, t)) => (None, Some((speed, t))),
                None => (None, None),
            },
            None if touchpads => (None, Some((&mut s.mouse.sensitivity, &mut s.touchpad))),
            None => (Some(&mut s.mouse), None),
        };
        if let Some(m) = mouse {
            match verb {
                "speed" => m.sensitivity = speed,
                "accel" => m.accel_profile = if index == 0 { "flat" } else { "adaptive" }.into(),
                "natural" => m.natural_scroll = !m.natural_scroll,
                "scroll-factor" => m.scroll_factor = factor,
                "left-handed" => m.left_handed = !m.left_handed,
                _ => {}
            }
        }
        if let Some((sensitivity, t)) = touchpad {
            match verb {
                "speed" => *sensitivity = speed,
                "natural" => t.natural_scroll = !t.natural_scroll,
                "tap" => t.tap_to_click = !t.tap_to_click,
                "dwt" => t.disable_while_typing = !t.disable_while_typing,
                "scroll-method" => t.scroll_method = if index == 0 { "2fg" } else { "edge" }.into(),
                "click-method" => t.clickfinger = index == 1,
                _ => {}
            }
        }
    });
}

fn move_layout(weak: &slint::Weak<MainWindow>, state: &mut State, from: usize, to: usize) {
    let n = state.settings.keyboard.layouts.len();
    if from >= n || to >= n || from == to {
        return;
    }
    let layout = state.settings.keyboard.layouts[from].clone();
    let name = describe(state, &layout);
    change(weak, state, format!("{name} moved"), |s| {
        let l = s.keyboard.layouts.remove(from);
        s.keyboard.layouts.insert(to, l);
    });
    state.layout = to;
}

/// Up and Down on the Mouse & touchpad page.
pub fn move_selection(state: &mut State, delta: i32) {
    let rows = state.rows();
    let at = rows.iter().position(|r| *r == state.selected).unwrap_or(0);
    let next = (at as i64 + i64::from(delta)).clamp(0, rows.len() as i64 - 1) as usize;
    state.selected = rows[next].clone();
}

/// Up and Down on the Keyboard page.
pub fn move_layout_selection(state: &mut State, delta: i32) {
    let n = state.settings.keyboard.layouts.len();
    state.layout = (state.layout as i64 + i64::from(delta)).clamp(0, n.saturating_sub(1) as i64) as usize;
}

/// The add dialog's layouts for its search; those already in use are left out. Without
/// xkeyboard-config's list, what's typed is taken as a layout code and a variant.
fn results(state: &State) -> Vec<Layout> {
    let query = &state.add_query;
    let used = &state.settings.keyboard.layouts;
    let Some(registry) = &state.registry else { return Vec::new() };
    if registry.layouts.is_empty() {
        let mut words = query.split_whitespace();
        return words
            .next()
            .map(|l| Layout { layout: l.to_lowercase(), variant: words.next().unwrap_or("").to_lowercase() })
            .filter(|l| !used.contains(l))
            .into_iter()
            .collect();
    }
    registry
        .search(query)
        .into_iter()
        .map(|l| Layout { layout: l.layout.clone(), variant: l.variant.clone() })
        .filter(|l| !used.contains(l))
        .take(RESULTS)
        .collect()
}

fn describe(state: &mut State, l: &Layout) -> String {
    state.registry().describe(&l.layout, &l.variant)
}

fn label_of(state: &State, name: &str) -> String {
    state.pointers.iter().find(|p| p.name == name).map_or_else(|| pretty(name), |p| p.label.clone())
}

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// The active layout, as a place in the settings' list.
fn active(state: &State) -> Option<usize> {
    state
        .devices
        .main_keyboard()
        .and_then(|k| k.active_layout_index)
        .filter(|i| *i < state.settings.keyboard.layouts.len())
}

/// The sidebar's statuses: Keyboard's (the layout in use, "CH"), and Mouse & touchpad's ("1 on").
/// None without Hyprland: the pages say why, and "not running" would cut "Mouse & touchpad" short.
pub fn status(state: &State) -> (String, String) {
    if state.not_running || !state.read {
        return (String::new(), String::new());
    }
    let layouts = &state.settings.keyboard.layouts;
    let code = layouts.get(active(state).unwrap_or(0)).map(|l| l.layout.to_uppercase()).unwrap_or_default();
    let on = state.pointers.iter().filter(|p| p.kind() != Kind::KeyboardPart).count();
    (code, if on == 0 { "none".into() } else { format!("{on} on") })
}

fn ordinal(i: usize) -> String {
    match i {
        0 => "first".into(),
        1 => "second".into(),
        2 => "third".into(),
        3 => "fourth".into(),
        n => format!("number {}", n + 1),
    }
}

fn capitalised(text: &str) -> String {
    let mut c = text.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The bar at the bottom of both pages: what the last change did, else where the settings are.
fn bar(state: &State) -> InputBar {
    let path = state.file_path();
    let text_now = state.text();
    let (text, path, tone) = if let Some(e) = &state.error {
        (e.clone(), String::new(), "danger")
    } else if state.reverted {
        ("Reverted, and saved to".to_owned(), path, "accent")
    } else if let Some((what, _)) = &state.last {
        if state.applied {
            (format!("{what} and saved to"), path, "accent")
        } else {
            ("Applying…".to_owned(), String::new(), "accent")
        }
    } else {
        match &state.saved {
            None => ("Changes apply at once and are saved to".to_owned(), path, "faint"),
            Some(t) if *t == text_now => ("Your settings are in".to_owned(), path, "faint"),
            // Set elsewhere in the config, after Lituus's file.
            Some(_) => ("Hyprland's settings differ from".to_owned(), path, "warn"),
        }
    };
    InputBar {
        text: s(text),
        path: s(path),
        tone: s(tone),
        can_revert: state.last.is_some(),
        can_save: state.last.is_none() && state.saved.as_ref().is_some_and(|t| *t != text_now),
        not_loaded: state.file.is_some() && !state.loaded,
        load_line: s(state.file.as_ref().map(ConfigFile::load_line).unwrap_or_default()),
    }
}

fn empty(state: &State, page: &str) -> (SharedString, SharedString, SharedString) {
    if state.not_running {
        (
            s("not-running"),
            s("Hyprland isn't running"),
            s(format!(
                "The {page} page sets up input devices through Hyprland. Open Lituus inside a Hyprland session."
            )),
        )
    } else {
        (s("loading"), s(state.error.clone().unwrap_or_else(|| "Reading Hyprland's settings…".into())), s(""))
    }
}

/// Whether the pages show their settings: Hyprland is running and its settings were read.
fn has_settings(state: &State) -> bool {
    !state.not_running && state.read && state.file.is_some()
}

/// The keys that work on the Keyboard page now, for the bar at the bottom: with two layouts or
/// more, choosing one, moving it and removing it (one always stays).
pub fn keyboard_keys(state: &State) -> Vec<Hint> {
    if !has_settings(state) || state.settings.keyboard.layouts.len() < 2 {
        return Vec::new();
    }
    vec![Hint::new("↑↓", "Select"), Hint::new("Alt ↑↓", "Move layout"), Hint::new("Del", "Remove")]
}

/// The same for Mouse & touchpad: its list.
pub fn mouse_keys(state: &State) -> Vec<Hint> {
    if has_settings(state) { vec![Hint::new("↑↓", "Select")] } else { Vec::new() }
}

pub fn keyboard_view(state: &mut State) -> KeyboardView {
    let mut v = KeyboardView::default();
    if !has_settings(state) {
        (v.state, v.empty_title, v.empty_text) = empty(state, "Keyboard");
        return v;
    }
    state.registry();
    let state = &*state;
    let Some(registry) = &state.registry else { return v };
    v.state = s("list");
    let k = &state.settings.keyboard;
    let names: Vec<String> = k.layouts.iter().map(|l| registry.describe(&l.layout, &l.variant)).collect();
    let active = active(state);
    let current = active.unwrap_or(0);
    let options = &k.options;
    let switch_keys = xkb::SWITCH.label(options).filter(|_| !xkb::SWITCH.current(options).is_empty());
    let next = names.get((current + 1) % names.len().max(1)).cloned().unwrap_or_default();
    let meta = match (names.len(), switch_keys) {
        (0 | 1, _) => "In use · the only layout".to_owned(),
        (_, Some(keys)) => format!("In use · {keys} switches to {next}"),
        (_, None) => format!("In use · Switch now goes to {next}"),
    };
    let code = k.layouts.get(current).map(|l| l.layout.clone()).unwrap_or_default();
    let count = if k.layouts.len() == 1 { "1 layout".to_owned() } else { format!("{} layouts", k.layouts.len()) };
    v.hero = HeroInfo {
        icon: s("keyboard"),
        name: s(names.get(current).cloned().unwrap_or_default()),
        meta: s(meta),
        tone: s("accent"),
        detail: s(format!("{code}   {count}")),
    };
    v.can_switch = k.layouts.len() > 1;
    v.layouts = model(
        names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let place = if i == 0 { "first, so it's the one at login".to_owned() } else { ordinal(i) };
                let in_use = active == Some(i);
                LayoutRow {
                    name: s(name),
                    meta: s(if in_use { format!("In use · {place}") } else { capitalised(&place) }),
                    code: s(match k.layouts[i].variant.as_str() {
                        "" => k.layouts[i].layout.clone(),
                        variant => format!("{} · {variant}", k.layouts[i].layout),
                    }),
                    active: in_use,
                    selected: i == state.layout,
                }
            })
            .collect(),
    );
    v.can_add = k.layouts.len() < MAX_LAYOUTS;
    v.add_note = s(if v.can_add { "" } else { "Up to four: xkb switches between four layouts at most" });
    let (labels, i) = xkb::SWITCH.labels(options, registry);
    v.switch_options = strings(labels);
    v.switch_selected = i as i32;
    let (labels, i) = xkb::CAPS.labels(options, registry);
    v.caps_options = strings(labels);
    v.caps_selected = i as i32;
    let (labels, i) = xkb::COMPOSE.labels(options, registry);
    v.compose_options = strings(labels);
    v.compose_selected = i as i32;
    v.compose_sub =
        s(if i == 0 { "A key that starts a sequence: \" then a types ä" } else { "Compose, \" and a types ä" });
    v.numlock = k.numlock;
    v.repeat_delay = k.repeat_delay as f32;
    v.repeat_delay_text = s(format!("{} ms", k.repeat_delay));
    v.delay_differs = s(if k.repeat_delay == 600 { "" } else { "Changed from 600 ms" });
    v.repeat_rate = k.repeat_rate as f32;
    v.repeat_rate_text = s(format!("{} /s", k.repeat_rate));
    v.rate_differs = s(if k.repeat_rate == 25 { "" } else { "Changed from 25 /s" });
    v.add_open = state.add_open;
    if state.add_open {
        let found = results(state);
        let no_list = registry.layouts.is_empty();
        v.add_results = model(
            found
                .iter()
                .map(|l| LayoutRow {
                    name: s(registry.describe(&l.layout, &l.variant)),
                    meta: s(""),
                    code: s(if l.variant.is_empty() {
                        l.layout.clone()
                    } else {
                        format!("{} · {}", l.layout, l.variant)
                    }),
                    active: false,
                    selected: false,
                })
                .collect(),
        );
        v.add_hint = s(if no_list {
            format!(
                "xkeyboard-config's list ({}) isn't there, so type a layout code and a variant, such as \"us intl\".",
                xkb::EVDEV_XML
            )
        } else if found.is_empty() {
            "No layout matches.".into()
        } else {
            String::new()
        });
    }
    v.bar = bar(state);
    v
}

/// "−0.30": the speed as the page shows it.
fn speed_text(v: f64) -> String {
    format!("{v:.2}").replace('-', "−")
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn accel_text(a: &str) -> &'static str {
    if a == "flat" { "flat" } else { "adaptive" }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    match n {
        0 => format!("no {one}"),
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// A slider setting.
fn slider(
    id: &str,
    label: &str,
    sub: &str,
    value: f64,
    (minimum, maximum, step): (f64, f64, f64),
    text: String,
    differs: String,
) -> PointerSetting {
    PointerSetting {
        id: s(id),
        kind: s("slider"),
        label: s(label),
        sub: s(sub),
        differs: s(differs),
        value: value as f32,
        minimum: minimum as f32,
        maximum: maximum as f32,
        step: step as f32,
        text: s(text),
        ..PointerSetting::default()
    }
}

fn switch(id: &str, label: &str, sub: &str, on: bool, differs: String) -> PointerSetting {
    PointerSetting {
        id: s(id),
        kind: s("switch"),
        label: s(label),
        sub: s(sub),
        differs: s(differs),
        on,
        ..PointerSetting::default()
    }
}

fn choice(id: &str, label: &str, sub: &str, options: [&str; 2], selected: usize, differs: String) -> PointerSetting {
    PointerSetting {
        id: s(id),
        kind: s("choice"),
        label: s(label),
        sub: s(sub),
        differs: s(differs),
        options: strings(options.iter().map(|o| (*o).to_owned())),
        selected: selected as i32,
        ..PointerSetting::default()
    }
}

/// "Every mouse: 0.00" under a device's setting that isn't the default's.
fn differs<T: PartialEq>(mine: T, default: T, of: &str, shown: impl Fn(&T) -> String) -> String {
    if mine == default { String::new() } else { format!("{of}: {}", shown(&default)) }
}

/// A mouse's settings; `default` for a device's, to mark the ones that differ.
fn mouse_settings(m: &MouseSettings, default: Option<&MouseSettings>) -> (Vec<PointerSetting>, usize) {
    let d = default.unwrap_or(m);
    let of = "Every mouse";
    let rows = vec![
        slider(
            "speed",
            "Speed",
            "",
            m.sensitivity,
            (-1.0, 1.0, 0.05),
            speed_text(m.sensitivity),
            differs(m.sensitivity, d.sensitivity, of, |v| speed_text(*v)),
        ),
        choice(
            "accel",
            "Acceleration",
            "",
            ["Flat", "Adaptive"],
            usize::from(m.accel_profile != "flat"),
            differs(accel_text(&m.accel_profile), accel_text(&d.accel_profile), of, |v| (*v).to_owned()),
        ),
        switch(
            "natural",
            "Natural scrolling",
            "Content moves with the wheel, like on a phone",
            m.natural_scroll,
            differs(m.natural_scroll, d.natural_scroll, of, |v| on_off(*v).into()),
        ),
        slider(
            "scroll-factor",
            "Scroll speed",
            "",
            m.scroll_factor,
            (0.1, 3.0, 0.1),
            format!("{:.1}×", m.scroll_factor),
            differs(m.scroll_factor, d.scroll_factor, of, |v| format!("{v:.1}×")),
        ),
        switch(
            "left-handed",
            "Left-handed",
            "Swap the left and right buttons",
            m.left_handed,
            differs(m.left_handed, d.left_handed, of, |v| on_off(*v).into()),
        ),
    ];
    let n = rows.iter().filter(|r| !r.differs.is_empty()).count();
    (rows, n)
}

fn scroll_text(t: &TouchpadSettings) -> String {
    if t.scroll_method == "edge" { "edge".into() } else { "two fingers".into() }
}

fn touchpad_settings(
    speed: f64,
    t: &TouchpadSettings,
    default: Option<(f64, &TouchpadSettings)>,
) -> (Vec<PointerSetting>, usize) {
    let (ds, d) = default.unwrap_or((speed, t));
    let of = "Every touchpad";
    let shared = if default.is_none() { "The same setting as for every mouse" } else { "" };
    let rows = vec![
        slider(
            "speed",
            "Speed",
            shared,
            speed,
            (-1.0, 1.0, 0.05),
            speed_text(speed),
            differs(speed, ds, of, |v| speed_text(*v)),
        ),
        switch(
            "natural",
            "Natural scrolling",
            "Content moves with your fingers",
            t.natural_scroll,
            differs(t.natural_scroll, d.natural_scroll, of, |v| on_off(*v).into()),
        ),
        switch(
            "tap",
            "Tap to click",
            "",
            t.tap_to_click,
            differs(t.tap_to_click, d.tap_to_click, of, |v| on_off(*v).into()),
        ),
        switch(
            "dwt",
            "Disable while typing",
            "",
            t.disable_while_typing,
            differs(t.disable_while_typing, d.disable_while_typing, of, |v| on_off(*v).into()),
        ),
        choice(
            "scroll-method",
            "Scroll with",
            "",
            ["Two fingers", "Edge"],
            usize::from(t.scroll_method == "edge"),
            differs(scroll_text(t), scroll_text(d), of, Clone::clone),
        ),
        choice(
            "click-method",
            "Click with",
            "Fingers: 1 left, 2 right, 3 middle",
            ["Button areas", "Fingers"],
            usize::from(t.clickfinger),
            differs(t.clickfinger, d.clickfinger, of, |v| if *v { "fingers".into() } else { "button areas".into() }),
        ),
    ];
    let n = rows.iter().filter(|r| !r.differs.is_empty()).count();
    (rows, n)
}

/// A device's line in the list: its own settings, else what it is.
fn device_meta(state: &State, name: &str, p: Option<&Pointer>, bt: &[lituus_bt::Device]) -> String {
    let connection = p.map(|p| connection(p, bt)).unwrap_or_default();
    let mut parts = match state.own(name).map(|d| &d.settings) {
        Some(DeviceSettings::Mouse(m)) => {
            vec![format!("Speed {}", speed_text(m.sensitivity)), accel_text(&m.accel_profile).to_owned()]
        }
        Some(DeviceSettings::Touchpad(speed, t)) => {
            vec![
                format!("Speed {}", speed_text(*speed)),
                if t.tap_to_click { "tap to click".into() } else { "no tap to click".into() },
            ]
        }
        None => vec![match p.map(Pointer::kind) {
            Some(Kind::Touchpad) => "Touchpad".to_owned(),
            Some(Kind::Stick) => "Pointing stick".to_owned(),
            Some(Kind::KeyboardPart) if p.is_some_and(|p| p.receiver) => "Mouse part of a keyboard receiver".to_owned(),
            Some(Kind::KeyboardPart) => "Mouse part of a keyboard".to_owned(),
            _ => "Mouse".to_owned(),
        }],
    };
    if p.is_none() {
        parts.push("not connected".into());
    } else if !connection.is_empty() && p.is_some_and(|p| p.kind() != Kind::KeyboardPart) {
        parts.push(connection);
    }
    parts.join(" · ")
}

/// "Bluetooth", "USB", "built in": from the kernel, else from BlueZ by name.
fn connection(p: &Pointer, bt: &[lituus_bt::Device]) -> String {
    if !p.connection.is_empty() {
        return p.connection.clone();
    }
    if bluez(p, bt).is_some() { "Bluetooth".into() } else { String::new() }
}

/// The BlueZ device a pointer is: by address, else by name.
fn bluez<'a>(p: &Pointer, bt: &'a [lituus_bt::Device]) -> Option<&'a lituus_bt::Device> {
    bt.iter().filter(|d| d.connected).find(|d| {
        (!p.address.is_empty() && d.address.eq_ignore_ascii_case(&p.address))
            || [Some(&d.alias), d.name.as_ref()].into_iter().flatten().any(|n| {
                let n = hypr_name(n);
                p.name == n || p.name.ends_with(&format!("-{n}"))
            })
    })
}

pub fn mouse_view(state: &State, bt: &[lituus_bt::Device]) -> MouseView {
    let mut v = MouseView::default();
    if !has_settings(state) {
        (v.state, v.empty_title, v.empty_text) = empty(state, "Mouse & touchpad");
        return v;
    }
    v.state = s("list");
    let mice = state.pointers.iter().filter(|p| matches!(p.kind(), Kind::Mouse | Kind::Stick)).count();
    let touchpads = state.of_kind(true).count();
    v.sub = s(format!("{} · {}", plural(mice, "mouse", "mice"), plural(touchpads, "touchpad", "touchpads")));
    // The hero: Hyprland doesn't say which pointer was used last, so the first mouse, else
    // touchpad.
    let hero = state
        .pointers
        .iter()
        .find(|p| matches!(p.kind(), Kind::Mouse | Kind::Stick))
        .or_else(|| state.of_kind(true).next());
    v.hero = match hero {
        Some(p) => {
            let mut meta = vec!["Connected".to_owned()];
            let c = connection(p, bt);
            if !c.is_empty() {
                meta.push(c);
            }
            if let Some(b) = bluez(p, bt).and_then(|d| d.battery) {
                meta.push(format!("battery {b} %"));
            }
            HeroInfo {
                icon: s(if p.kind().touchpad() { "touchpad" } else { "mouse" }),
                name: s(&p.label),
                meta: s(meta.join(" · ")),
                tone: s("accent"),
                detail: s(&p.name),
            }
        }
        None => HeroInfo {
            icon: s("mouse"),
            name: s("No mouse or touchpad"),
            meta: s("Plug one in, or connect one over Bluetooth"),
            tone: s("muted"),
            detail: s(""),
        },
    };

    // The list.
    let row = |sel: Selected, name: String, meta: String, icon: &str| InputRow {
        id: s(sel.id()),
        name: s(name),
        meta: s(meta),
        icon: s(icon),
        selected: sel == state.selected,
    };
    let m = &state.settings.mouse;
    let t = &state.settings.touchpad;
    let defaults = vec![
        row(
            Selected::Mice,
            "Every mouse".into(),
            format!("Speed {} · {}", speed_text(m.sensitivity), accel_text(&m.accel_profile)),
            "mouse",
        ),
        row(
            Selected::Touchpads,
            "Every touchpad".into(),
            if touchpads == 0 {
                "No touchpad connected".into()
            } else {
                format!(
                    "Speed {} · {}",
                    speed_text(m.sensitivity),
                    if t.tap_to_click { "tap to click" } else { "no tap to click" }
                )
            },
            "touchpad",
        ),
    ];
    let icon = |p: Option<&Pointer>, d: Option<&Device>| match (p.map(Pointer::kind), d.map(|d| &d.settings)) {
        (Some(Kind::Touchpad), _) | (None, Some(DeviceSettings::Touchpad(..))) => "touchpad",
        (Some(Kind::KeyboardPart), _) => "keyboard",
        _ => "mouse",
    };
    let own: Vec<InputRow> = state
        .settings
        .devices
        .iter()
        .map(|d| {
            let p = state.pointers.iter().find(|p| p.name == d.name);
            row(
                Selected::Device(d.name.clone()),
                label_of(state, &d.name),
                device_meta(state, &d.name, p, bt),
                icon(p, Some(d)),
            )
        })
        .collect();
    let rest: Vec<InputRow> = state
        .pointers
        .iter()
        .filter(|p| state.own(&p.name).is_none())
        .map(|p| {
            row(
                Selected::Device(p.name.clone()),
                p.label.clone(),
                device_meta(state, &p.name, Some(p), bt),
                icon(Some(p), None),
            )
        })
        .collect();
    let mut sections = vec![InputSection { title: s("DEFAULTS"), rows: model(defaults) }];
    if !own.is_empty() {
        sections.push(InputSection { title: s("WITH THEIR OWN SETTINGS"), rows: model(own) });
    }
    if !rest.is_empty() {
        sections.push(InputSection { title: s("USING THE DEFAULTS"), rows: model(rest.clone()) });
    }
    v.sections = model(sections);
    if state.own_open {
        v.own_open = true;
        v.own_rows = model(
            rest.into_iter()
                .map(|r| InputRow { id: s(r.id.trim_start_matches("dev:")), selected: false, ..r })
                .collect(),
        );
    }

    // The details of the one selected.
    let users = |touchpad: bool| state.of_kind(touchpad).filter(|p| state.own(&p.name).is_none()).count();
    match &state.selected {
        Selected::Mice => {
            v.title = s("Every mouse");
            let total = state.of_kind(false).count();
            v.status = s(match (total, users(false)) {
                (0, _) => "No mouse connected. These apply to any mouse you connect.".to_owned(),
                (_, 0) => "Every mouse here has its own settings; these apply to any other you connect.".to_owned(),
                (_, n) => format!("Used by {}", plural(n, "mouse", "mice")),
            });
            v.status_tone = s("muted");
            let (rows, _) = mouse_settings(m, None);
            v.settings = model(rows);
            v.note = s("Speed, acceleration and left-handed are for touchpads too: Hyprland has one of each for both.");
        }
        Selected::Touchpads => {
            v.title = s("Every touchpad");
            v.status = s(match (touchpads, users(true)) {
                (0, _) => "No touchpad connected. These apply to any touchpad you connect.".to_owned(),
                (_, 0) => "Every touchpad here has its own settings; these apply to any other you connect.".to_owned(),
                (_, n) => format!("Used by {}", plural(n, "touchpad", "touchpads")),
            });
            v.status_tone = s("muted");
            let (rows, _) = touchpad_settings(m.sensitivity, t, None);
            v.settings = model(rows);
        }
        Selected::Device(name) => {
            let p = state.pointers.iter().find(|p| p.name == *name);
            v.title = s(label_of(state, name));
            v.device = s(format!("device: {name}"));
            match state.own(name).map(|d| &d.settings) {
                Some(settings) => {
                    let (rows, n, of) = match settings {
                        DeviceSettings::Mouse(mine) => {
                            let (r, n) = mouse_settings(mine, Some(m));
                            (r, n, "Every mouse")
                        }
                        DeviceSettings::Touchpad(speed, mine) => {
                            let (r, n) = touchpad_settings(*speed, mine, Some((m.sensitivity, t)));
                            (r, n, "Every touchpad")
                        }
                    };
                    let connected = if p.is_none() { " · not connected" } else { "" };
                    v.status = s(match n {
                        0 => format!("Own settings · the same as {of}{connected}"),
                        1 => format!("Own settings · 1 differs from {of}{connected}"),
                        n => format!("Own settings · {n} differ from {of}{connected}"),
                    });
                    v.status_tone = s("accent");
                    v.action = s("Use the defaults");
                    v.settings = model(rows);
                }
                None => {
                    let of = if p.is_some_and(|p| p.kind().touchpad()) { "Every touchpad" } else { "Every mouse" };
                    v.status = s(format!("Uses the settings of {of}"));
                    v.status_tone = s("muted");
                    v.action = s("Give it its own settings");
                    v.note = s(format!("Its own settings start as {of}'s; then they're changed for it alone."));
                }
            }
        }
    }
    v.bar = bar(state);
    v
}

fn shorten_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

thread_local! {
    /// The layouts and the selected entry's settings, each kept as one model: replacing it would
    /// rebuild the row being dragged (a layout's handle, a slider), and the drag would stop.
    static LAYOUTS: Rc<VecModel<LayoutRow>> = Rc::new(VecModel::default());
    static SETTINGS: Rc<VecModel<PointerSetting>> = Rc::new(VecModel::default());
}

/// `rows` in `model`: the rows that changed are replaced, the others left as they are.
pub fn kept<T: Clone + PartialEq + 'static>(model: &Rc<VecModel<T>>, rows: &ModelRc<T>) -> ModelRc<T> {
    let rows: Vec<T> = rows.iter().collect();
    if model.row_count() == rows.len() {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        model.set_vec(rows);
    }
    ModelRc::from(model.clone())
}

pub fn render(window: &MainWindow, state: &mut State, bt: Option<&lituus_bt::Snapshot>) {
    let page = window.get_page();
    if page == "keyboard" {
        let mut v = keyboard_view(state);
        v.layouts = LAYOUTS.with(|m| kept(m, &v.layouts));
        window.set_keyboard(v);
    }
    if page == "mouse" {
        let mut v = mouse_view(state, bt.map(|b| b.devices.as_slice()).unwrap_or_default());
        v.settings = SETTINGS.with(|m| kept(m, &v.settings));
        window.set_mouse(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Devices {
        Devices::parse(include_str!("../../hypr/tests/fixtures/devices.json")).unwrap()
    }

    /// The owner's desk: the MX Master 3S over Bluetooth, and a keyboard's receiver.
    const PROC: &str = "I: Bus=0005 Vendor=046d Product=b034 Version=0001
N: Name=\"Logitech MX Master 3S\"
P: Phys=bc:6e:e2:40:12:aa
S: Sysfs=/devices/virtual/misc/uhid/0005:046D:B034.0004/input/input21
U: Uniq=d2:8b:5e:1a:44:07
H: Handlers=sysrq kbd leds event7 mouse1
B: PROP=0

I: Bus=0003 Vendor=046d Product=c548 Version=0111
N: Name=\"Logitech USB Receiver\"
P: Phys=usb-0000:00:14.0-3/input0
U: Uniq=
H: Handlers=sysrq kbd leds event4
B: PROP=0

I: Bus=0003 Vendor=046d Product=c548 Version=0111
N: Name=\"Logitech USB Receiver\"
P: Phys=usb-0000:00:14.0-3/input1
U: Uniq=
H: Handlers=event5 mouse0
B: PROP=0

I: Bus=0018 Vendor=04f3 Product=3150 Version=0100
N: Name=\"ELAN0670:00 04F3:3150 Touchpad\"
U: Uniq=
H: Handlers=event9 mouse2
B: PROP=5
";

    fn udev(event: u32) -> Option<String> {
        match event {
            7 => Some("E:ID_INPUT=1\nE:ID_INPUT_MOUSE=1\nE:ID_INPUT_KEY=1\n".into()),
            5 => Some("E:ID_INPUT=1\nE:ID_INPUT_MOUSE=1\n".into()),
            9 => Some("E:ID_INPUT=1\nE:ID_INPUT_TOUCHPAD=1\nE:ID_INPUT_WIDTH_MM=100\n".into()),
            _ => None,
        }
    }

    fn laptop() -> Devices {
        let mut d = devices();
        d.mice.push(lituus_hypr::Pointer { name: "elan0670:00-04f3:3150-touchpad".into(), ..Default::default() });
        // No kernel device by this name: told by its name.
        d.mice.push(lituus_hypr::Pointer { name: "tpps/2-elan-trackpoint".into(), ..Default::default() });
        d
    }

    /// A state read from the fixtures, with Hyprland's config in a new folder.
    fn state_in(name: &str) -> (State, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lituus-input-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut st =
            State { hypr_dir: Some(dir.clone()), registry_path: Some("/nonexistent".into()), ..State::default() };
        st.registry = Some(Registry::parse(EVDEV));
        let devices = devices();
        let pointers = classify(&devices, &kernel_devices(PROC), &udev);
        let options = vec![
            ("kb_layout".to_owned(), Value::Text("ch,us".into())),
            ("kb_variant".to_owned(), Value::Text(",".into())),
            ("kb_options".to_owned(), Value::Text("grp:win_space_toggle,lv3:ralt_switch".into())),
            ("repeat_delay".to_owned(), Value::Number(600.0)),
        ];
        loaded(&mut st, Ok(Read { hypr: Hyprland::at("/nonexistent"), lua: true, devices, options, pointers }));
        // Saved only: there's no Hyprland to send the settings to.
        st.hypr = None;
        (st, dir)
    }

    const EVDEV: &str = "<layoutList>
<layout><configItem><name>ch</name><description>German (Switzerland)</description></configItem>
<variantList><variant><configItem><name>fr</name><description>French (Switzerland)</description></configItem></variant></variantList></layout>
<layout><configItem><name>us</name><description>English (US)</description></configItem></layout>
<layout><configItem><name>de</name><description>German</description></configItem></layout>
<layout><configItem><name>fr</name><description>French</description></configItem></layout>
<layout><configItem><name>it</name><description>Italian</description></configItem></layout>
</layoutList>";

    fn weak() -> slint::Weak<MainWindow> {
        slint::Weak::default()
    }

    #[test]
    fn pointers_are_told_apart() {
        let p = classify(&laptop(), &kernel_devices(PROC), &udev);
        let kinds: Vec<(&str, Kind, &str)> =
            p.iter().map(|p| (p.label.as_str(), p.kind(), p.connection.as_str())).collect();
        assert_eq!(
            kinds,
            [
                ("Logitech MX Master 3S", Kind::Mouse, "Bluetooth"),
                // Also one of the keyboards.
                ("Logitech USB Receiver", Kind::KeyboardPart, "USB"),
                ("ELAN0670:00 04F3:3150 Touchpad", Kind::Touchpad, "built in"),
                ("TPPS/2 Elan Trackpoint", Kind::Stick, ""),
            ]
        );
        assert_eq!(p[0].address, "D2:8B:5E:1A:44:07");
        assert!(p[1].receiver);
        // A gaming mouse with keys is among the keyboards too, but udev calls it a mouse.
        let mut d = devices();
        d.mice[0].name = "razer-deathadder".into();
        d.keyboards[0].name = "razer-deathadder".into();
        let k = vec![Kernel {
            name: "Razer DeathAdder".into(),
            event: Some(5),
            pointer: true,
            bus: 3,
            ..Kernel::default()
        }];
        assert_eq!(classify(&d, &k, &udev)[0].kind(), Kind::Mouse);
        // Without udev's word, one of the keyboards is a keyboard's part.
        assert_eq!(classify(&d, &[], &|_| None)[0].kind(), Kind::KeyboardPart);
        assert_eq!(hypr_name("Logitech MX Master 3S"), "logitech-mx-master-3s");
        assert_eq!(pretty("logitech-mx-master-3s"), "Logitech MX Master 3S");
    }

    #[test]
    fn the_keyboard_page() {
        let (mut st, dir) = state_in("keyboard");
        let v = keyboard_view(&mut st);
        assert_eq!(v.state, "list");
        assert_eq!(v.hero.name, "German (Switzerland)");
        assert_eq!(v.hero.meta, "In use · Super + Space switches to English (US)");
        assert_eq!(v.hero.detail, "ch   2 layouts");
        let rows: Vec<(String, String)> = v.layouts.iter().map(|r| (r.name.to_string(), r.meta.to_string())).collect();
        assert_eq!(
            rows,
            [
                ("German (Switzerland)".into(), "In use · first, so it's the one at login".into()),
                ("English (US)".into(), "Second".into())
            ]
        );
        assert_eq!(v.switch_options.row_data(v.switch_selected as usize).unwrap(), "Super + Space");
        assert_eq!(v.caps_options.row_data(v.caps_selected as usize).unwrap(), "Caps Lock");
        assert_eq!(v.bar.text, "Changes apply at once and are saved to");
        assert_eq!(v.bar.path, dir.join("lituus.lua").display().to_string());
        assert!(!v.bar.can_revert);
        assert_eq!(status(&st), ("CH".into(), "1 on".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_is_saved_at_once_and_reverts() {
        let (mut st, dir) = state_in("change");
        st.hypr = Some(Hyprland::at("/nonexistent"));
        let w = weak();
        // A dragged slider: many moves, one change.
        for ms in [500.0, 410.0, 303.0] {
            act(&w, &mut st, "repeat-delay", "", ms);
        }
        assert_eq!(st.settings.keyboard.repeat_delay, 300);
        let file = std::fs::read_to_string(dir.join("lituus.lua")).unwrap();
        assert!(file.contains("repeat_delay = 300,"), "{file}");
        let v = keyboard_view(&mut st);
        assert_eq!(v.delay_differs, "Changed from 600 ms");
        // Hyprland isn't there to take it, but the file has it.
        assert!(v.bar.text.contains("Hyprland didn't take the change"), "{}", v.bar.text);
        st.error = None;
        st.hypr = None;
        assert_eq!(keyboard_view(&mut st).bar.text, "Repeat delay applied and saved to");
        act(&w, &mut st, "caps", "1", 0.0);
        assert_eq!(st.settings.keyboard.options, "grp:win_space_toggle,lv3:ralt_switch,ctrl:nocaps");
        act(&w, &mut st, "revert", "", 0.0);
        // Only the last change goes back.
        assert_eq!(st.settings.keyboard.options, "grp:win_space_toggle,lv3:ralt_switch");
        assert_eq!(st.settings.keyboard.repeat_delay, 300);
        let v = keyboard_view(&mut st);
        assert_eq!(v.bar.text, "Reverted, and saved to");
        assert!(!v.bar.can_revert);
        assert!(!std::fs::read_to_string(dir.join("lituus.lua")).unwrap().contains("ctrl:nocaps"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn layouts_are_added_moved_and_removed() {
        let (mut st, dir) = state_in("layouts");
        let w = weak();
        act(&w, &mut st, "add-open", "", 0.0);
        act(&w, &mut st, "add-query", "switz", 0.0);
        let v = keyboard_view(&mut st);
        // German (Switzerland) is in use already.
        let found: Vec<String> = v.add_results.iter().map(|r| r.name.to_string()).collect();
        assert_eq!(found, ["French (Switzerland)"]);
        act(&w, &mut st, "add", "0", 0.0);
        assert!(!st.add_open);
        assert_eq!(st.settings.keyboard.layout(), "ch,us,ch");
        assert_eq!(st.settings.keyboard.variant(), ",,fr");
        // Alt+Up twice: the new one is first, so it's the one at login.
        act(&w, &mut st, "layout-up", "", 0.0);
        act(&w, &mut st, "layout-up", "", 0.0);
        assert_eq!(st.settings.keyboard.layout(), "ch,ch,us");
        assert_eq!(st.settings.keyboard.variant(), "fr,,");
        assert_eq!(st.layout, 0);
        act(&w, &mut st, "layout-move", "0:2", 0.0);
        assert_eq!(st.settings.keyboard.variant(), ",,fr");
        act(&w, &mut st, "layout-remove", "1", 0.0);
        assert_eq!(st.settings.keyboard.layout(), "ch,ch");
        // Four at most, and one at least.
        for q in ["german", "italian"] {
            act(&w, &mut st, "add-open", "", 0.0);
            act(&w, &mut st, "add-query", q, 0.0);
            act(&w, &mut st, "add", "0", 0.0);
        }
        assert_eq!(st.settings.keyboard.layouts.len(), 4);
        let v = keyboard_view(&mut st);
        assert!(!v.can_add && !v.add_note.is_empty());
        act(&w, &mut st, "add-open", "", 0.0);
        assert!(!st.add_open);
        for _ in 0..5 {
            act(&w, &mut st, "layout-remove", "0", 0.0);
        }
        assert_eq!(st.settings.keyboard.layouts.len(), 1);
        let file = std::fs::read_to_string(dir.join("lituus.lua")).unwrap();
        assert_eq!(InputSettings::parse(&file, true).keyboard.layouts, st.settings.keyboard.layouts);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_xkeyboard_config_a_code_is_typed() {
        let (mut st, dir) = state_in("no-list");
        st.registry = Some(Registry::default());
        act(&weak(), &mut st, "add-open", "", 0.0);
        act(&weak(), &mut st, "add-query", "US intl", 0.0);
        let v = keyboard_view(&mut st);
        assert!(v.add_hint.contains("isn't there"), "{}", v.add_hint);
        assert_eq!(v.add_results.row_data(0).unwrap().code, "us · intl");
        act(&weak(), &mut st, "add", "0", 0.0);
        assert_eq!(st.settings.keyboard.variant(), ",,intl");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_device_gets_its_own_settings_and_shows_what_differs() {
        let (mut st, dir) = state_in("own");
        let w = weak();
        let v = mouse_view(&st, &[]);
        let titles: Vec<String> = v.sections.iter().map(|s| s.title.to_string()).collect();
        assert_eq!(titles, ["DEFAULTS", "USING THE DEFAULTS"]);
        assert_eq!(v.sub, "1 mouse · no touchpad");
        assert_eq!(v.hero.name, "Logitech MX Master 3S");
        assert_eq!(v.hero.meta, "Connected · Bluetooth");
        assert_eq!(v.status, "Used by 2 mice");
        act(&w, &mut st, "select", "dev:logitech-mx-master-3s", 0.0);
        assert_eq!(mouse_view(&st, &[]).action, "Give it its own settings");
        // A device on the defaults has nothing of its own to change.
        act(&w, &mut st, "speed", "", -0.3);
        assert!(st.settings.devices.is_empty());
        act(&w, &mut st, "own", "", 0.0);
        let v = mouse_view(&st, &[]);
        assert_eq!(v.status, "Own settings · the same as Every mouse");
        act(&w, &mut st, "speed", "", -0.3);
        act(&w, &mut st, "accel", "0", 0.0);
        let v = mouse_view(&st, &[]);
        assert_eq!(v.status, "Own settings · 2 differ from Every mouse");
        let differs: Vec<String> = v.settings.iter().map(|r| r.differs.to_string()).collect();
        assert_eq!(differs, ["Every mouse: 0.00", "Every mouse: adaptive", "", "", ""]);
        assert_eq!(v.sections.row_data(1).unwrap().rows.row_data(0).unwrap().meta, "Speed −0.30 · flat · Bluetooth");
        assert_eq!(v.bar.text, "Acceleration for Logitech MX Master 3S applied and saved to");
        // The defaults change; the device keeps its own.
        act(&w, &mut st, "select", "mouse", 0.0);
        act(&w, &mut st, "natural", "", 0.0);
        assert!(st.settings.mouse.natural_scroll);
        assert_eq!(
            st.settings.devices[0].settings,
            DeviceSettings::Mouse(MouseSettings {
                sensitivity: -0.3,
                accel_profile: "flat".into(),
                ..MouseSettings::default()
            })
        );
        let file = std::fs::read_to_string(dir.join("lituus.lua")).unwrap();
        assert!(file.contains("hl.device({\n    name = \"logitech-mx-master-3s\",\n    sensitivity = -0.3,\n    accel_profile = \"flat\""), "{file}");
        // A restart reads the device's settings from the file.
        let mut again = State { hypr_dir: st.hypr_dir.clone(), registry: st.registry.clone(), ..State::default() };
        let devices = devices();
        let pointers = classify(&devices, &kernel_devices(PROC), &udev);
        loaded(
            &mut again,
            Ok(Read { hypr: Hyprland::at("/nonexistent"), lua: true, devices, options: vec![], pointers }),
        );
        assert_eq!(again.settings.devices, st.settings.devices);
        // Back on the defaults: the block goes, and Hyprland is sent the defaults for it.
        act(&w, &mut st, "select", "dev:logitech-mx-master-3s", 0.0);
        act(&w, &mut st, "defaults", "", 0.0);
        assert!(st.settings.devices.is_empty());
        assert_eq!(st.reset, ["logitech-mx-master-3s"]);
        assert!(!std::fs::read_to_string(dir.join("lituus.lua")).unwrap().contains("hl.device"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_touchpad_and_a_touchpad_of_its_own() {
        let (mut st, dir) = state_in("touchpad");
        st.pointers = classify(&laptop(), &kernel_devices(PROC), &udev);
        let w = weak();
        act(&w, &mut st, "select", "touchpad", 0.0);
        act(&w, &mut st, "tap", "", 0.0);
        act(&w, &mut st, "click-method", "1", 0.0);
        assert!(!st.settings.touchpad.tap_to_click && st.settings.touchpad.clickfinger);
        let v = mouse_view(&st, &[]);
        assert_eq!(v.sub, "2 mice · 1 touchpad");
        assert_eq!(v.status, "Used by 1 touchpad");
        let ids: Vec<String> = v.settings.iter().map(|r| r.id.to_string()).collect();
        assert_eq!(ids, ["speed", "natural", "tap", "dwt", "scroll-method", "click-method"]);
        // The touchpads' speed is the mice's: Hyprland has one for both.
        act(&w, &mut st, "speed", "", 0.5);
        assert_eq!(st.settings.mouse.sensitivity, 0.5);
        let name = "elan0670:00-04f3:3150-touchpad";
        act(&w, &mut st, "own", name, 0.0);
        act(&w, &mut st, "speed", "", 0.2);
        let v = mouse_view(&st, &[]);
        assert_eq!(v.title, "ELAN0670:00 04F3:3150 Touchpad");
        assert_eq!(v.settings.row_data(0).unwrap().differs, "Every touchpad: 0.50");
        assert_eq!(st.settings.devices[0].settings, DeviceSettings::Touchpad(0.2, st.settings.touchpad.clone()));
        let file = std::fs::read_to_string(dir.join("lituus.lua")).unwrap();
        assert_eq!(InputSettings::parse(&file, true), st.settings);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn up_and_down_go_through_the_list() {
        let (mut st, dir) = state_in("rows");
        move_selection(&mut st, 1);
        assert_eq!(st.selected, Selected::Touchpads);
        move_selection(&mut st, 5);
        assert_eq!(st.selected, Selected::Device("logitech-usb-receiver".into()));
        move_selection(&mut st, -1);
        assert_eq!(st.selected, Selected::Device("logitech-mx-master-3s".into()));
        move_layout_selection(&mut st, 3);
        assert_eq!(st.layout, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The layouts' keys show with two layouts, not with one, which can't move or go.
    #[test]
    fn the_bar_lists_the_keys_that_work() {
        let (mut st, dir) = state_in("keys");
        let shown = |st: &State| -> Vec<String> {
            keyboard_keys(st).iter().map(|h| format!("{} {}", h.key, h.label)).collect()
        };
        assert_eq!(shown(&st), ["↑↓ Select", "Alt ↑↓ Move layout", "Del Remove"]);
        assert_eq!(mouse_keys(&st).len(), 1);
        act(&weak(), &mut st, "layout-remove", "1", 0.0);
        assert!(shown(&st).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_hyprland() {
        let mut st = State::default();
        loaded(&mut st, Err(lituus_hypr::Error::NotRunning));
        assert_eq!(keyboard_view(&mut st).state, "not-running");
        let v = mouse_view(&st, &[]);
        assert_eq!(v.state, "not-running");
        assert_eq!(v.empty_title, "Hyprland isn't running");
        assert_eq!(status(&st), (String::new(), String::new()));
        // No keys for lists that aren't there.
        assert!(keyboard_keys(&st).is_empty() && mouse_keys(&st).is_empty());
        assert_eq!(State::default().settings, InputSettings::default());
        assert_eq!(status(&State::default()), (String::new(), String::new()));
    }

    /// Where an element with this label is (the first, if several), and its size.
    fn place(window: &MainWindow, label: &str) -> (f32, f32, f32) {
        use i_slint_backend_testing::ElementHandle;
        let e = ElementHandle::find_by_accessible_label(window, label).next().unwrap_or_else(|| panic!("{label}"));
        (e.absolute_position().x, e.absolute_position().y, e.size().width)
    }

    /// One column in the smallest window, two as in the mockup, three in the owner's; the
    /// controls stay inside the window.
    #[test]
    fn the_keyboard_page_takes_columns_as_the_window_grows() {
        use slint::{ComponentHandle, LogicalSize};
        i_slint_backend_testing::init_no_event_loop();
        let (mut st, dir) = state_in("columns");
        let window = MainWindow::new().unwrap();
        window.set_page("keyboard".into());
        window.set_keyboard(keyboard_view(&mut st));
        window.show().unwrap();
        // The smallest width, tall enough that every card is in view.
        for (w, h, columns) in [(1040.0, 1400.0, 1), (1200.0, 760.0, 2), (2000.0, 1070.0, 3)] {
            window.window().set_size(LogicalSize::new(w, h));
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));
            let (lx, ly, _) = place(&window, "Layouts");
            let (tx, ty, _) = place(&window, "Typing");
            let (_, _, add) = place(&window, "Add a layout…");
            match columns {
                1 => assert!(tx == lx && ty > ly, "{w}: {lx},{ly} {tx},{ty}"),
                2 => assert!(tx > lx + 300.0 && (ty - ly).abs() < 1.0, "{w}: {lx} {tx}"),
                _ => {
                    // Keys is below the fold in narrower windows; here it's the third column.
                    let (kx, ky, _) = place(&window, "Keys");
                    assert!(kx > tx + 300.0 && (ky - ty).abs() < 1.0, "{w}: {tx} {kx}");
                }
            }
            // The cards share the width: no layout row runs past the window.
            assert!(lx + add < w, "{w}: {lx} + {add}");
            // Add is as wide as its label, not the card.
            assert!(add < 200.0, "{w}: {add}");
        }
        // In three columns, every card reaches the bottom of the page (above the bar) and scrolls
        // inside (the owner, 2026-11-07).
        let bottom = |name: &str| {
            let card = i_slint_backend_testing::ElementHandle::find_by_element_type_name(&window, name)
                .next()
                .unwrap_or_else(|| panic!("{name}"));
            card.absolute_position().y + card.size().height
        };
        for name in ["LayoutsCard", "TypingCard", "KeysCard"] {
            assert!((bottom(name) - bottom("KeysCard")).abs() < 1.0, "{name}: {}", bottom(name));
        }
        assert!(bottom("KeysCard") > 950.0, "{}", bottom("KeysCard"));
        // In two, the list and Keys below Typing do.
        window.window().set_size(LogicalSize::new(1200.0, 1400.0));
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));
        assert!(
            (bottom("LayoutsCard") - bottom("KeysCard")).abs() < 1.0,
            "{} {}",
            bottom("LayoutsCard"),
            bottom("KeysCard")
        );
        assert!(bottom("KeysCard") > 1280.0, "{}", bottom("KeysCard"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// In the smallest window, a setting's name keeps its room beside the slider.
    #[test]
    fn the_mouse_settings_fit_the_smallest_window() {
        use slint::{ComponentHandle, LogicalSize};
        i_slint_backend_testing::init_no_event_loop();
        let (st, dir) = state_in("small");
        let window = MainWindow::new().unwrap();
        window.set_page("mouse".into());
        window.set_mouse(mouse_view(&st, &[]));
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.show().unwrap();
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(50));
        let (x, _, width) = place(&window, "Scroll speed");
        assert!(width >= 112.0, "the name is {width} px");
        let sliders: Vec<(f32, f32, f32)> =
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Scroll speed")
                .map(|e| (e.absolute_position().x, e.absolute_position().y, e.size().width))
                .collect();
        assert!(sliders.iter().all(|(sx, _, sw)| sx + sw <= 1040.0), "{sliders:?}");
        assert!(sliders.iter().any(|(sx, _, _)| *sx > x + 100.0), "{sliders:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_displays_part_of_the_file_stays() {
        let (mut st, dir) = state_in("both");
        let displays = "-- Written by Lituus (Displays). Change it in Lituus: edits made here are overwritten.\n\nhl.monitor({ output = \"DP-1\", disabled = true })\n";
        std::fs::write(dir.join("lituus.lua"), displays).unwrap();
        act(&weak(), &mut st, "numlock", "", 0.0);
        let file = std::fs::read_to_string(dir.join("lituus.lua")).unwrap();
        assert!(file.starts_with(displays), "{file}");
        assert_eq!(lituus_hypr::section(&file, "Displays").as_deref(), Some(displays));
        assert!(lituus_hypr::section(&file, "Input").unwrap().contains("numlock_by_default = true"));
        std::fs::remove_dir_all(&dir).unwrap();

        // hyprlang's, with the user's own lines before the first part.
        let (mut st, dir) = state_in("both-conf");
        (st.lua, st.file) = (false, Some(ConfigFile::new(&dir, false)));
        let displays = "# Mine.\n\n# Written by Lituus (Displays). Change it in Lituus: edits made here are overwritten.\n\nmonitor = DP-1, disable\n";
        std::fs::write(dir.join("lituus.conf"), displays).unwrap();
        act(&weak(), &mut st, "numlock", "", 0.0);
        let file = std::fs::read_to_string(dir.join("lituus.conf")).unwrap();
        assert!(file.starts_with(displays), "{file}");
        assert!(lituus_hypr::section(&file, "Input").unwrap().contains("numlock_by_default = true"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
