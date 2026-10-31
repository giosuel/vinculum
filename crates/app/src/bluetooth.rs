//! The Bluetooth page: what it shows, worked out from a BlueZ snapshot and the page's state
//! (pure, so it's tested without a window); the thread that talks to BlueZ; and the glue that
//! turns the page's clicks into commands for that thread.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use slint::{ComponentHandle, SharedString};
use lituus_bt::{Adapter, AgentEvent, Answer, Ask, Device, DeviceKind, PairingRequest, Snapshot};

use crate::audio;
use crate::{
    APP, Action, App, BtAudio, BtPage, BtPair, BtRow, BtSection, Chip, ChipRow, DetailItem, Details, MainWindow,
    NavItem, Toggle, model, update,
};

/// How long "Look for devices" looks before it stops by itself (discovery costs battery).
const DISCOVERY: Duration = Duration::from_secs(120);
/// "Visible" for this long; "Always" in the adapter dialog is 0, no limit.
const VISIBLE_FOR: u32 = 180;
const NOT_RUNNING_COMMAND: &str = "sudo systemctl enable --now bluetooth";

/// What the Bluetooth thread does for the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Power {
        adapter: String,
        on: bool,
    },
    /// Visible to other devices for `timeout` seconds (0: until switched off).
    Discoverable {
        adapter: String,
        on: bool,
        timeout: u32,
    },
    Pairable {
        adapter: String,
        on: bool,
    },
    AdapterAlias {
        adapter: String,
        alias: String,
    },
    Discover {
        adapter: String,
        on: bool,
    },
    /// Pairs, trusts and connects.
    Pair {
        device: String,
    },
    CancelPairing {
        device: String,
    },
    Connect {
        device: String,
    },
    Disconnect {
        device: String,
    },
    Trust {
        device: String,
        on: bool,
    },
    Block {
        device: String,
        on: bool,
    },
    Rename {
        device: String,
        alias: String,
    },
    Forget {
        adapter: String,
        device: String,
    },
    /// Lifts rfkill's soft block on Bluetooth.
    Unblock,
    /// Reads everything again now ("Check again").
    Refresh,
    /// Switches a sound card's profile (PipeWire), then reads the cards again.
    SetAudio {
        card: String,
        profile: String,
    },
}

impl Command {
    /// The device a command is about, for its row's "Connecting…".
    fn device(&self) -> Option<&str> {
        match self {
            Self::Pair { device } | Self::Connect { device } | Self::Disconnect { device } => Some(device),
            _ => None,
        }
    }
}

/// What the Bluetooth thread tells the page.
#[derive(Debug)]
pub enum Event {
    Snapshot(Result<Snapshot, lituus_bt::Error>),
    Done(Command),
    Failed(Command, lituus_bt::Error),
    Agent(AgentEvent),
    /// rfkill couldn't be written to.
    UnblockFailed(String),
    /// The Bluetooth sound cards; `None` when there's no PipeWire (or PulseAudio) to ask.
    Audio(Option<Vec<audio::Card>>),
    AudioFailed(String),
}

/// A message above the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub title: String,
    pub text: String,
    pub tone: &'static str,
    /// What "Try again" runs.
    pub retry: Option<Command>,
}

/// A request from the pairing agent, or a code it shows, waiting for the user.
#[derive(Debug)]
pub enum Prompt {
    Ask(PairingRequest),
    Display { device: String, code: String, entered: Option<u16> },
}

impl Prompt {
    fn device(&self) -> &str {
        match self {
            Self::Ask(request) => &request.device,
            Self::Display { device, .. } => device,
        }
    }
}

/// The page's state on the UI thread.
#[derive(Default)]
pub struct State {
    pub snapshot: Option<Snapshot>,
    /// Why there's no snapshot: BlueZ isn't running, or the bus failed.
    pub error: Option<String>,
    pub not_running: bool,
    pub selected: Option<String>,
    /// The device whose last pairing failed, and why.
    pub failed: Option<(String, String)>,
    /// Devices with a pairing or connection under way.
    pub busy: HashSet<String>,
    pub message: Option<Message>,
    /// When "Visible" ends, if Lituus switched it on with a limit.
    pub visible_until: Option<Instant>,
    pub prompt: Option<Prompt>,
    pub prompts: VecDeque<Prompt>,
    /// The dialog's fields are cleared once when it opens.
    pub reset_prompt: bool,
    pub adapter_open: bool,
    /// The device being renamed.
    pub rename: Option<String>,
    /// The page is open, so it looks for devices.
    pub open: bool,
    pub commands: Option<tokio::sync::mpsc::UnboundedSender<Command>>,
    pub row_ids: Vec<String>,
    /// The Bluetooth sound cards PipeWire has.
    pub audio: Vec<audio::Card>,
}

impl State {
    fn adapter(&self) -> Option<&Adapter> {
        self.snapshot.as_ref().and_then(Snapshot::adapter)
    }

    fn device(&self, path: &str) -> Option<&Device> {
        self.snapshot.as_ref().and_then(|s| s.device(path))
    }

    fn send(&mut self, command: Command) {
        if let Some(device) = command.device() {
            self.busy.insert(device.to_owned());
        }
        if matches!(command, Command::Pair { .. } | Command::Connect { .. }) {
            self.message = None;
        }
        if let Command::Pair { device } = &command
            && self.failed.as_ref().is_some_and(|(d, _)| d == device)
        {
            self.failed = None;
        }
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }
}

fn s(text: impl Into<SharedString>) -> SharedString {
    text.into()
}

/// The words for a kind of device: in a row's meta, and as its type.
fn kind_label(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Headphones => "headphones",
        DeviceKind::Headset => "headset",
        DeviceKind::Speaker => "speaker",
        DeviceKind::Phone => "phone",
        DeviceKind::Computer => "computer",
        DeviceKind::Keyboard => "keyboard",
        DeviceKind::Mouse => "mouse",
        DeviceKind::Gamepad => "game controller",
        DeviceKind::Other => "device",
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

pub fn icon(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Headphones | DeviceKind::Headset => "headphones",
        DeviceKind::Speaker => "speaker",
        DeviceKind::Phone => "phone",
        DeviceKind::Computer => "computer",
        DeviceKind::Keyboard => "keyboard",
        DeviceKind::Mouse => "mouse",
        DeviceKind::Gamepad => "gamepad",
        DeviceKind::Other => "bluetooth",
    }
}

/// Signal strength as bars, from the RSSI.
fn bars(rssi: i16) -> i32 {
    match rssi {
        -60.. => 4,
        -70..-60 => 3,
        -80..-70 => 2,
        _ => 1,
    }
}

/// The services Lituus names, by UUID, with what an untrusted device asking for one wants.
fn service(uuid: &str) -> Option<(&'static str, &'static str)> {
    let short = uuid.strip_prefix("0000")?.strip_suffix("-0000-1000-8000-00805f9b34fb")?;
    Some(match short {
        "110b" => ("Audio sink (A2DP)", "play audio"),
        "110a" => ("Audio source (A2DP)", "play audio on this computer"),
        "110e" | "110c" | "110f" => ("Remote control (AVRCP)", "control playback"),
        "111e" => ("Hands-free (HFP)", "use its microphone"),
        "111f" => ("Hands-free gateway", "take calls through this computer"),
        "1108" | "1112" => ("Headset (HSP)", "use its microphone"),
        "1115" => ("Network user (PAN)", "use the network"),
        "1116" => ("Network access (PAN)", "share its network"),
        "1103" => ("Dial-up networking", "share its network"),
        "1105" => ("Send files (OBEX)", "send files"),
        "1106" => ("File transfer (FTP)", "browse files"),
        "112f" | "1130" => ("Phonebook", "read the phonebook"),
        "1132" | "1133" | "1134" => ("Messages", "read messages"),
        "1124" => ("Keyboard or mouse (HID)", "act as a keyboard or mouse"),
        "1812" => ("Keyboard or mouse (HID over LE)", "act as a keyboard or mouse"),
        "180f" => ("Battery", "report its battery"),
        "180a" => ("Device information", "read device information"),
        _ => return None,
    })
}

/// How pairing with a nearby device will go, as a sentence and as a short label.
fn pairing_method(device: &Device) -> (&'static str, &'static str) {
    if device.legacy_pairing {
        return ("It uses an older pairing method, so you'll type a fixed PIN.", "Fixed PIN");
    }
    match device.kind() {
        DeviceKind::Keyboard => ("You'll be asked to type a code on the keyboard.", "Type a code on the device"),
        DeviceKind::Phone | DeviceKind::Computer => ("Both screens show a code to compare.", "Compare a code"),
        DeviceKind::Headphones | DeviceKind::Headset | DeviceKind::Speaker => {
            ("Put it in pairing mode first; it won't show a code.", "No code")
        }
        _ => ("", "Depends on the device"),
    }
}

/// The sidebar's entry.
pub fn nav(state: &State, current: bool) -> NavItem {
    let sub = if state.not_running {
        "Service not running".to_owned()
    } else if state.error.is_some() {
        "Not available".to_owned()
    } else {
        match (state.snapshot.as_ref(), state.adapter()) {
            (None, _) => String::new(),
            (Some(_), None) => "No adapter".to_owned(),
            (Some(_), Some(a)) if a.blocked() => "Blocked".to_owned(),
            (Some(_), Some(a)) if !a.powered => "Off".to_owned(),
            (Some(snap), Some(a)) => match snap.devices_of(&a.path).filter(|d| d.connected).count() {
                0 => "On".to_owned(),
                n => format!("{n} connected"),
            },
        }
    };
    NavItem { id: s("bluetooth"), label: s("Bluetooth"), sub: s(sub), icon: s("bluetooth"), current }
}

/// The empty state for a page that has no list: (state, icon, title, text, command, action).
fn empty(state: &State) -> Option<(&'static str, &'static str, String, String, &'static str, &'static str)> {
    if state.not_running {
        return Some((
            "no-service",
            "monitor",
            "The Bluetooth service isn't running".into(),
            "Lituus talks to BlueZ, which isn't running. Install the bluez package if needed, then start it:".into(),
            NOT_RUNNING_COMMAND,
            "Check again",
        ));
    }
    if let Some(error) = &state.error {
        return Some((
            "no-service",
            "warning",
            "Couldn't reach Bluetooth".into(),
            format!("{error}."),
            "",
            "Check again",
        ));
    }
    let Some(_) = &state.snapshot else {
        return Some(("loading", "", "Reading Bluetooth…".into(), String::new(), "", ""));
    };
    match state.adapter() {
        None => Some((
            "no-adapter",
            "bluetooth-off",
            "No Bluetooth adapter".into(),
            "This computer has no Bluetooth adapter, or its driver isn't loaded. A USB adapter shows up here as soon as it's plugged in.".into(),
            "",
            "",
        )),
        Some(a) if a.blocked() => Some((
            "blocked",
            "plane",
            "Bluetooth is blocked".into(),
            "Airplane mode or a hardware switch turned the Bluetooth radio off (rfkill). If a switch blocks it, flip the switch; Lituus notices right away.".into(),
            "",
            "Unblock",
        )),
        Some(a) if !a.powered => Some((
            "off",
            "bluetooth-off",
            "Bluetooth is off".into(),
            "Paired devices stay paired and reconnect when you turn it back on.".into(),
            "",
            "Turn on Bluetooth",
        )),
        Some(_) => None,
    }
}

/// "Visible · 2:41", "Visible" or "Hidden".
fn visible_label(adapter: &Adapter, until: Option<Instant>, now: Instant) -> String {
    if !adapter.discoverable {
        return "Hidden".into();
    }
    match until.filter(|_| adapter.discoverable_timeout > 0).map(|u| u.saturating_duration_since(now).as_secs()) {
        Some(left) if left > 0 => format!("Visible · {}:{:02}", left / 60, left % 60),
        _ => "Visible".into(),
    }
}

struct RowData {
    id: String,
    name: String,
    meta: String,
    tone: &'static str,
    icon: &'static str,
    battery: i32,
    bars: i32,
    button: &'static str,
}

fn paired_row(device: &Device, state: &State) -> RowData {
    let kind = kind_label(device.kind());
    let busy = state.busy.contains(&device.path);
    let (meta, tone) = if busy && device.connected {
        ("Disconnecting…".to_owned(), "muted")
    } else if busy {
        ("Connecting…".to_owned(), "muted")
    } else if device.blocked {
        (format!("Blocked · {kind}"), "danger")
    } else if device.connected {
        (format!("Connected · {kind}"), "accent")
    } else {
        (format!("Paired · {kind}"), "muted")
    };
    RowData {
        id: device.path.clone(),
        name: device.alias.clone(),
        meta,
        tone,
        icon: icon(device.kind()),
        battery: device.battery.filter(|_| device.connected).map(i32::from).unwrap_or(-1),
        bars: -1,
        button: if !device.connected && !device.blocked && !busy { "Connect" } else { "" },
    }
}

fn nearby_row(device: &Device, state: &State) -> RowData {
    let kind = capitalised(kind_label(device.kind()));
    let busy = state.busy.contains(&device.path);
    let failed = state.failed.as_ref().is_some_and(|(d, _)| d == &device.path);
    let (meta, tone) = if busy {
        ("Pairing…".to_owned(), "muted")
    } else if failed {
        (format!("Couldn't pair · {}", kind.to_lowercase()), "danger")
    } else {
        (kind, "muted")
    };
    RowData {
        id: device.path.clone(),
        name: device.alias.clone(),
        meta,
        tone,
        icon: icon(device.kind()),
        battery: -1,
        bars: device.rssi.map(bars).unwrap_or(-1),
        button: if busy { "" } else { "Pair" },
    }
}

/// Chips break into lines of about the pane's width (Slint has no wrapping layout).
fn chip_rows(chips: Vec<Chip>) -> Vec<ChipRow> {
    const LINE: usize = 52;
    let mut rows: Vec<Vec<Chip>> = Vec::new();
    let mut width = 0;
    for chip in chips {
        // Roughly: characters, plus the dot and padding.
        let w = chip.label.chars().count() + 6;
        if rows.is_empty() || width + w > LINE {
            rows.push(Vec::new());
            width = 0;
        }
        width += w;
        rows.last_mut().expect("a line").push(chip);
    }
    rows.into_iter().map(|chips| ChipRow { chips: model(chips) }).collect()
}

fn services(device: &Device) -> Vec<ChipRow> {
    let mut seen = HashSet::new();
    let chips = device
        .uuids
        .iter()
        .filter_map(|u| service(u))
        .filter(|(label, _)| seen.insert(*label))
        .map(|(label, _)| Chip { label: s(label), on: device.connected })
        .collect();
    chip_rows(chips)
}

fn action(id: &str, label: &str, kind: &str, enabled: bool) -> Action {
    Action { id: s(id), label: s(label), kind: s(kind), enabled }
}

fn item(label: &str, value: impl Into<SharedString>, mono: bool) -> DetailItem {
    DetailItem { label: s(label), value: value.into(), mono }
}

/// The audio choice for a connected headset or speaker: the options, and the one in use.
fn audio_choice(device: &Device, state: &State) -> Option<(Vec<audio::Choice>, Option<usize>)> {
    if !device.connected || !device.audio() {
        return None;
    }
    let card = state.audio.iter().find(|c| c.address.eq_ignore_ascii_case(&device.address))?;
    let (choices, selected) = audio::choices(card);
    (!choices.is_empty()).then_some((choices, selected))
}

fn details(device: &Device, adapter: &Adapter, state: &State) -> Details {
    let busy = state.busy.contains(&device.path);
    let kind = capitalised(kind_label(device.kind()));
    let mut d = Details {
        title: s(&device.alias),
        chips_label: s("Services"),
        chips: model(services(device)),
        ..Details::default()
    };
    let mut items = Vec::new();
    if device.paired {
        d.renamable = true;
        let (status, tone) = match () {
            _ if busy && device.connected => ("Disconnecting…", "muted"),
            _ if busy => ("Connecting…", "muted"),
            _ if device.blocked => ("Blocked", "danger"),
            _ if device.connected => ("Connected", "accent"),
            _ => ("Paired", "muted"),
        };
        d.status = s(status);
        d.status_tone = s(tone);
        if let Some(battery) = device.battery.filter(|_| device.connected) {
            d.status_extra = s(format!("· battery {battery} %"));
            items.push(item("Battery", format!("{battery} %"), false));
        }
        let connect = if device.connected {
            action("disconnect", "Disconnect", "secondary", !busy)
        } else {
            action("connect", "Connect", "primary", !busy && !device.blocked)
        };
        d.actions = model(vec![connect, action("forget", "Forget", "danger", true)]);
        d.toggles = model(vec![
            Toggle {
                id: s("trusted"),
                label: s("Trusted"),
                sub: s("Connects and uses its services without asking you first"),
                on: device.trusted,
            },
            Toggle {
                id: s("blocked"),
                label: s("Blocked"),
                sub: s("Refuse every connection from this device"),
                on: device.blocked,
            },
        ]);
        items.push(item("Address", &device.address, true));
        items.push(item("Type", kind, false));
        items.push(item("Adapter", &adapter.name, false));
    } else {
        let failed = state.failed.as_ref().filter(|(d, _)| d == &device.path);
        d.status = s("Not paired");
        d.status_tone = s(if failed.is_some() { "danger" } else { "faint" });
        d.status_extra = s(if failed.is_some() {
            "· last attempt failed"
        } else if device.rssi.is_some() {
            "· in range"
        } else {
            ""
        });
        let label = if busy {
            "Pairing…"
        } else if failed.is_some() {
            "Try again"
        } else {
            "Pair"
        };
        d.actions = model(vec![action("pair", label, "primary", !busy)]);
        let (how, method) = pairing_method(device);
        d.intro = s(format!("Pairing lets {} and this computer remember each other. {how}", device.alias).trim_end());
        if let Some(rssi) = device.rssi {
            // -100 dBm and below is 0 %, -40 dBm and above 100 %.
            let percent = ((i32::from(rssi) + 100) * 100 / 60).clamp(0, 100);
            items.push(item("Signal", format!("{percent} % ({rssi} dBm)"), false));
        }
        items.push(item("Address", &device.address, true));
        items.push(item("Type", kind, false));
        items.push(item("Pairing", method, false));
    }
    d.items = model(items);
    d
}

/// The page, worked out from the state; also the rows' ids in order, and the selection.
pub fn page(state: &State, now: Instant) -> (BtPage, Vec<String>, Option<String>) {
    let mut page = BtPage { message_tone: s("info"), ..BtPage::default() };
    if let Some((kind, icon, title, text, command, action)) = empty(state) {
        page.state = s(kind);
        page.empty_icon = s(icon);
        page.empty_title = s(title);
        page.empty_text = s(text);
        page.empty_command = s(command);
        page.empty_action = s(action);
        page.powered = false;
        return (page, Vec::new(), None);
    }
    let (Some(snap), Some(adapter)) = (state.snapshot.as_ref(), state.adapter()) else {
        return (page, Vec::new(), None);
    };
    page.state = s("list");
    page.powered = true;
    page.alias = s(&adapter.alias);
    page.adapter = s(&adapter.name);
    page.visible = adapter.discoverable;
    page.visible_label = s(visible_label(adapter, state.visible_until, now));
    page.pairable = adapter.pairable;

    let mut mine: Vec<&Device> = snap.devices_of(&adapter.path).filter(|d| d.paired).collect();
    mine.sort_by_key(|d| (!d.connected, d.alias.to_lowercase()));
    // Nameless devices nearby are mostly beacons and the like, nothing to pair with.
    let mut nearby: Vec<&Device> = snap.devices_of(&adapter.path).filter(|d| !d.paired && d.name.is_some()).collect();
    nearby.sort_by_key(|d| (std::cmp::Reverse(d.rssi.unwrap_or(i16::MIN)), d.alias.to_lowercase()));

    let mine: Vec<RowData> = mine.iter().map(|d| paired_row(d, state)).collect();
    let nearby: Vec<RowData> = nearby.iter().map(|d| nearby_row(d, state)).collect();
    let ids: Vec<String> = mine.iter().chain(&nearby).map(|r| r.id.clone()).collect();
    let selected = state.selected.clone().filter(|id| ids.contains(id)).or_else(|| ids.first().cloned());
    let rows = |rows: Vec<RowData>| {
        model(
            rows.into_iter()
                .map(|r| BtRow {
                    selected: selected.as_deref() == Some(r.id.as_str()),
                    id: s(r.id),
                    name: s(r.name),
                    meta: s(r.meta),
                    tone: s(r.tone),
                    icon: s(r.icon),
                    battery: r.battery,
                    bars: r.bars,
                    button: s(r.button),
                })
                .collect(),
        )
    };
    let mut sections = Vec::new();
    if !mine.is_empty() {
        sections.push(BtSection { title: s("MY DEVICES"), rows: rows(mine), ..BtSection::default() });
    }
    sections.push(BtSection {
        title: s("NEARBY"),
        rows: rows(nearby),
        busy: s(if adapter.discovering { "Looking for devices" } else { "" }),
        empty: s(if adapter.discovering {
            "Nothing yet. Put the device in pairing mode, close to this computer."
        } else {
            "Look for devices with the button at the top."
        }),
    });
    page.sections = model(sections);
    if let Some(device) = selected.as_deref().and_then(|id| snap.device(id)) {
        page.has_details = true;
        page.details = details(device, adapter, state);
        if let Some((choices, chosen)) = audio_choice(device, state) {
            page.audio = model(choices.into_iter().map(|c| BtAudio { label: s(c.label), sub: s(c.sub) }).collect());
            page.audio_selected = chosen.map_or(-1, |i| i as i32);
        }
    }
    if let Some(m) = &state.message {
        page.message = s(&m.text);
        page.message_title = s(&m.title);
        page.message_tone = s(m.tone);
        page.message_action = s(if m.retry.is_some() { "Try again" } else { "" });
    }
    (page, ids, selected)
}

/// The pairing dialog for a prompt.
pub fn pair_dialog(prompt: &Prompt, device: Option<&Device>) -> BtPair {
    let name = device.map(|d| d.alias.clone()).unwrap_or_else(|| "the device".into());
    let mut dialog = BtPair {
        name: s(&name),
        address: s(device.map(|d| d.address.as_str()).unwrap_or_default()),
        icon: s(device.map(|d| icon(d.kind())).unwrap_or("bluetooth")),
        typed: -1,
        ..BtPair::default()
    };
    let digits = |code: &str| model(code.chars().map(|c| s(c.to_string())).collect());
    match prompt {
        Prompt::Ask(request) => match &request.ask {
            Ask::Confirm(passkey) => {
                dialog.mode = s("confirm");
                dialog.title = s(format!("Pair with {name}?"));
                dialog.text = s(format!("Check that {name} shows the same code, then confirm on both devices."));
                dialog.digits = digits(&format!("{passkey:06}"));
                dialog.accept = s("Codes match");
            }
            Ask::PinCode => {
                dialog.mode = s("pin");
                dialog.title = s(format!("Enter the PIN for {name}"));
                dialog.text = s("This device uses an older pairing method that needs a fixed PIN.");
                dialog.hint = s("Older car kits and headsets often use 0000 or 1234; check the manual if it fails.");
                dialog.accept = s("Pair");
            }
            Ask::Passkey => {
                dialog.mode = s("passkey");
                dialog.title = s(format!("Enter the code for {name}"));
                dialog.text = s(format!("Type the six-digit code {name} shows."));
                dialog.accept = s("Pair");
            }
            Ask::Authorize => {
                dialog.mode = s("authorize");
                dialog.title = s(format!("Pair with {name}?"));
                dialog.text = s(format!("{name} doesn't show a code. Pair only if you just put it in pairing mode."));
                dialog.accept = s("Pair");
            }
            Ask::Service(uuid) => {
                let (label, verb) = service(uuid).unwrap_or(("a service", "connect"));
                dialog.mode = s("service");
                dialog.title = s(format!("Allow {name} to {verb}?"));
                dialog.text =
                    s(format!("{name} is paired but not trusted, so Lituus asks before it connects to a service."));
                // "audio source (A2DP)": only the first letter goes small.
                let mut chars = label.chars();
                let label: String = chars.next().into_iter().flat_map(char::to_lowercase).chain(chars).collect();
                dialog.wants = s(format!("Wants: {label}"));
                dialog.always = s(format!("Always allow {name}"));
                dialog.accept = s("Allow");
            }
        },
        Prompt::Display { code, entered, .. } => {
            dialog.mode = s("display");
            match entered {
                Some(n) => {
                    dialog.title = s(format!("Type this code on {name}"));
                    dialog.text = s("Then press Enter on the keyboard. The boxes fill in as you type.");
                    dialog.typed = i32::from(*n);
                }
                None => {
                    dialog.title = s(format!("Type this PIN on {name}"));
                    dialog.text = s("Then press Enter on it.");
                }
            }
            dialog.digits = digits(code);
        }
    }
    dialog
}

pub fn render(window: &MainWindow, app: &mut App) {
    let state = &mut app.bt;
    let (page, ids, selected) = page(state, Instant::now());
    window.set_bt(page);
    state.row_ids = ids;
    state.selected = selected;
    window.set_bt_pair_open(state.prompt.is_some());
    if let Some(prompt) = &state.prompt {
        window.set_bt_pair(pair_dialog(prompt, state.device(prompt.device())));
        if std::mem::take(&mut state.reset_prompt) {
            window.set_bt_pair_code("".into());
            window.set_bt_pair_always(false);
        }
    }
    window.set_bt_adapter_open(state.adapter_open);
    if let Some(a) = state.adapter() {
        window.set_bt_adapter_label(format!("{} · {}", a.name, a.address).into());
        window.set_bt_pairable(a.pairable);
        window.set_bt_visible_choice(match (a.discoverable, a.discoverable_timeout) {
            (false, _) => 0,
            (true, 0) => 2,
            (true, _) => 1,
        });
    }
    window.set_bt_rename_open(state.rename.is_some());
    if let Some(device) = state.rename.as_deref().and_then(|d| state.device(d)) {
        window.set_bt_rename_title(format!("Rename {}", device.alias).into());
        let own = device.name.clone().unwrap_or_else(|| device.address.clone());
        window.set_bt_rename_hint(
            format!("Only this computer uses the name. Leave it empty for the device's own, {own}.").into(),
        );
    }
}

/// The selection moved with the arrow keys.
pub fn move_selection(state: &mut State, delta: i32) {
    let ids = &state.row_ids;
    if ids.is_empty() {
        return;
    }
    let current = state.selected.as_ref().and_then(|s| ids.iter().position(|id| id == s)).unwrap_or(0);
    let next = (current as i64 + i64::from(delta)).clamp(0, ids.len() as i64 - 1) as usize;
    state.selected = Some(ids[next].clone());
}

/// Enter on the selected row: connect or disconnect a paired device, pair a nearby one.
pub fn activate(state: &mut State) {
    let Some(device) = state.selected.as_deref().and_then(|id| state.device(id)).cloned() else { return };
    if state.busy.contains(&device.path) {
        return;
    }
    let command = match (device.paired, device.connected) {
        (true, true) => Command::Disconnect { device: device.path },
        (true, false) if !device.blocked => Command::Connect { device: device.path },
        (false, _) => Command::Pair { device: device.path },
        _ => return,
    };
    state.send(command);
}

/// Opening or leaving the page starts or stops looking for devices.
pub fn set_open(state: &mut State, open: bool) {
    if state.open == open {
        return;
    }
    state.open = open;
    let adapter = state.adapter().filter(|a| a.powered).map(|a| a.path.clone());
    if let Some(adapter) = adapter {
        state.send(Command::Discover { adapter, on: open });
    }
}

/// Shows the next prompt, if none is open.
fn next_prompt(state: &mut State) {
    if state.prompt.is_none() {
        state.prompt = state.prompts.pop_front();
        state.reset_prompt = state.prompt.is_some();
    }
}

/// The open dialog's answer, or its cancellation.
fn answer_prompt(state: &mut State, accept: bool, code: &str, always: bool) {
    let Some(prompt) = state.prompt.take() else { return };
    match prompt {
        Prompt::Ask(request) => {
            let device = request.device.clone();
            let answer = match (&request.ask, accept) {
                (_, false) => {
                    if matches!(request.ask, Ask::Service(_)) {
                        Answer::Reject
                    } else {
                        Answer::Cancel
                    }
                }
                (Ask::PinCode, true) => Answer::Pin(code.trim().to_owned()),
                (Ask::Passkey, true) => match code.trim().parse::<u32>() {
                    Ok(passkey) if passkey <= 999_999 => Answer::Passkey(passkey),
                    // Not a code: the dialog stays.
                    _ => {
                        state.prompt = Some(Prompt::Ask(request));
                        return;
                    }
                },
                (Ask::Service(_), true) => {
                    if always {
                        state.send(Command::Trust { device: device.clone(), on: true });
                    }
                    Answer::Accept
                }
                (_, true) => Answer::Accept,
            };
            request.reply(answer);
        }
        Prompt::Display { device, .. } => {
            if !accept {
                state.send(Command::CancelPairing { device });
            }
        }
    }
    next_prompt(state);
}

/// What the Bluetooth thread reported.
pub fn handle(app: &mut App, event: Event) {
    let state = &mut app.bt;
    match event {
        Event::Snapshot(Ok(snapshot)) => {
            // Opened before BlueZ was read: look for devices now.
            let start = state.open && state.adapter().is_none_or(|a| !a.powered);
            state.snapshot = Some(snapshot);
            state.error = None;
            state.not_running = false;
            if start && let Some(adapter) = state.adapter().filter(|a| a.powered).map(|a| a.path.clone()) {
                state.send(Command::Discover { adapter, on: true });
            }
            if state.adapter().is_none_or(|a| !a.discoverable) {
                state.visible_until = None;
            }
        }
        Event::Snapshot(Err(error)) => {
            state.not_running = matches!(error, lituus_bt::Error::NotRunning);
            state.error = Some(error.to_string());
            state.snapshot = None;
            state.busy.clear();
        }
        Event::Done(command) => {
            if let Some(device) = command.device() {
                state.busy.remove(device);
            }
            if let Command::Pair { device } = &command {
                // The keyboard's code is no longer needed.
                if state.prompt.as_ref().is_some_and(|p| matches!(p, Prompt::Display { device: d, .. } if d == device))
                {
                    state.prompt = None;
                    next_prompt(state);
                }
                state.prompts.retain(|p| !matches!(p, Prompt::Display { device: d, .. } if d == device));
            }
        }
        Event::Failed(command, error) => {
            if let Some(device) = command.device() {
                state.busy.remove(device);
            }
            let name = command.device().and_then(|d| state.device(d)).map(|d| d.alias.clone());
            let name = name.unwrap_or_else(|| "the device".into());
            let reason = error.describe();
            match &command {
                Command::Pair { device } => {
                    if state.prompt.as_ref().is_some_and(|p| p.device() == device) {
                        state.prompt = None;
                        next_prompt(state);
                    }
                    state.prompts.retain(|p| p.device() != device);
                    let cancelled =
                        matches!(&error, lituus_bt::Error::Refused { kind, .. } if kind == "AuthenticationCanceled");
                    if !cancelled {
                        state.failed = Some((device.clone(), reason.clone()));
                        state.message = Some(Message {
                            title: format!("Couldn't pair with {name}"),
                            text: reason,
                            tone: "danger",
                            retry: Some(command.clone()),
                        });
                    }
                }
                Command::Connect { .. } => {
                    state.message = Some(Message {
                        title: format!("Couldn't connect to {name}"),
                        text: reason,
                        tone: "danger",
                        retry: Some(command.clone()),
                    })
                }
                // Stopping a search that had stopped, say: nothing the user did.
                Command::Discover { on: false, .. } => {}
                _ => state.message = Some(Message { title: String::new(), text: reason, tone: "danger", retry: None }),
            }
        }
        Event::Agent(AgentEvent::Ask(request)) => {
            state.prompts.push_back(Prompt::Ask(request));
            next_prompt(state);
        }
        Event::Agent(AgentEvent::Display { device, code, entered }) => {
            // A new count for the open dialog, or a new code.
            match &mut state.prompt {
                Some(Prompt::Display { device: d, code: c, entered: e }) if *d == device => {
                    *c = code;
                    *e = entered;
                }
                _ => {
                    state.prompts.push_back(Prompt::Display { device, code, entered });
                    next_prompt(state);
                }
            }
        }
        Event::Agent(AgentEvent::Cancel) => {
            // BlueZ gave up on the request waiting for the user: dropping it answers "rejected".
            if matches!(state.prompt, Some(Prompt::Ask(_))) {
                state.prompt = None;
                next_prompt(state);
            }
        }
        Event::Audio(cards) => state.audio = cards.unwrap_or_default(),
        Event::AudioFailed(error) => {
            state.message = Some(Message {
                title: "Couldn't switch the audio".into(),
                text: format!("{error}."),
                tone: "danger",
                retry: None,
            })
        }
        Event::UnblockFailed(error) => {
            state.message = None;
            // Shown on the empty state, which has no message: put it in the error line instead.
            state.error = None;
            state.message = Some(Message {
                title: "Couldn't unblock Bluetooth".into(),
                text: format!("{error}. Run: rfkill unblock bluetooth"),
                tone: "danger",
                retry: None,
            });
        }
    }
}

/// A click or key on the page, or in one of its dialogs.
pub fn act(window: &MainWindow, verb: &str, argument: &str) {
    let weak = window.as_weak();
    let (code, always, adapter_name, rename) = (
        window.get_bt_pair_code().to_string(),
        window.get_bt_pair_always(),
        window.get_bt_adapter_name().trim().to_owned(),
        window.get_bt_rename_name().trim().to_owned(),
    );
    if verb == "adapter" {
        let alias = APP.with(|app| app.borrow().bt.adapter().map(|a| a.alias.clone()).unwrap_or_default());
        window.set_bt_adapter_name(alias.into());
    }
    if verb == "rename" {
        let alias = APP.with(|app| {
            let app = app.borrow();
            app.bt.selected.as_deref().and_then(|d| app.bt.device(d)).map(|d| d.alias.clone()).unwrap_or_default()
        });
        window.set_bt_rename_name(alias.into());
    }
    update(&weak, |app| {
        let state = &mut app.bt;
        let adapter = state.adapter().map(|a| (a.path.clone(), a.powered, a.discoverable, a.pairable, a.alias.clone()));
        let selected = state.selected.as_deref().and_then(|id| state.device(id)).cloned();
        match verb {
            "select" => state.selected = Some(argument.to_owned()),
            "row-button" => {
                state.selected = Some(argument.to_owned());
                activate(state);
            }
            "power" => {
                if let Some((adapter, powered, ..)) = adapter {
                    state.send(Command::Power { adapter, on: !powered });
                }
            }
            "rescan" | "key" if verb == "rescan" || argument == "s" => {
                if let Some((adapter, true, ..)) = adapter {
                    state.send(Command::Discover { adapter, on: true });
                }
            }
            "key" => {
                // p: pair the selected device.
                if let Some(device) = selected.filter(|d| !d.paired && !state.busy.contains(&d.path)) {
                    state.send(Command::Pair { device: device.path });
                }
            }
            "visible" => {
                if let Some((adapter, _, discoverable, ..)) = adapter {
                    set_visible(state, adapter, if discoverable { 0 } else { 1 });
                }
            }
            "adapter-visible" => {
                if let Some((adapter, ..)) = adapter {
                    set_visible(state, adapter, argument.parse().unwrap_or(0));
                }
            }
            "pairable" => {
                if let Some((adapter, _, _, pairable, _)) = adapter {
                    state.send(Command::Pairable { adapter, on: !pairable });
                }
            }
            "adapter" => state.adapter_open = true,
            "adapter-close" => state.adapter_open = false,
            "adapter-name" => {
                if let Some((adapter, .., alias)) = adapter
                    && !adapter_name.is_empty()
                    && adapter_name != alias
                {
                    state.send(Command::AdapterAlias { adapter, alias: adapter_name });
                }
            }
            "detail" => {
                let Some(device) = selected else { return };
                match argument {
                    "connect" | "disconnect" | "pair" => activate(state),
                    "forget" => {
                        if let Some((adapter, ..)) = adapter {
                            state.send(Command::Forget { adapter, device: device.path });
                        }
                    }
                    _ => {}
                }
            }
            "toggle" => {
                let Some(device) = selected else { return };
                match argument {
                    "trusted" => state.send(Command::Trust { device: device.path, on: !device.trusted }),
                    "blocked" => state.send(Command::Block { device: device.path, on: !device.blocked }),
                    _ => {}
                }
            }
            "rename" => state.rename = selected.map(|d| d.path),
            "rename-cancel" => state.rename = None,
            "rename-save" => {
                if let Some(device) = state.rename.take() {
                    state.send(Command::Rename { device, alias: rename });
                }
            }
            "empty-action" => match window_state(state) {
                "off" => {
                    if let Some((adapter, ..)) = adapter {
                        state.send(Command::Power { adapter, on: true });
                    }
                }
                "blocked" => state.send(Command::Unblock),
                _ => state.send(Command::Refresh),
            },
            "message-action" => {
                if let Some(command) = state.message.take().and_then(|m| m.retry) {
                    state.send(command);
                }
            }
            "dismiss" => state.message = None,
            "audio" => {
                let Some(device) = selected else { return };
                let Some((choices, _)) = audio_choice(&device, state) else { return };
                let Some(choice) = argument.parse::<usize>().ok().and_then(|i| choices.get(i)) else { return };
                let Some(card) = state.audio.iter_mut().find(|c| c.address.eq_ignore_ascii_case(&device.address))
                else {
                    return;
                };
                // Shown at once; the cards are read again once it's switched.
                card.active = Some(choice.profile.clone());
                let (card, profile) = (card.name.clone(), choice.profile.clone());
                state.send(Command::SetAudio { card, profile });
            }
            "pair-accept" => answer_prompt(state, true, &code, always),
            "pair-cancel" => answer_prompt(state, false, &code, always),
            _ => {}
        }
    });
}

/// The page's state name ("list", "off", …), for the empty state's button.
fn window_state(state: &State) -> &'static str {
    empty(state).map(|(kind, ..)| kind).unwrap_or("list")
}

/// Visible: 0 hidden, 1 for three minutes, 2 always.
fn set_visible(state: &mut State, adapter: String, choice: i32) {
    let timeout = if choice == 2 { 0 } else { VISIBLE_FOR };
    state.visible_until = (choice == 1).then(|| Instant::now() + Duration::from_secs(u64::from(VISIBLE_FOR)));
    state.send(Command::Discoverable { adapter, on: choice != 0, timeout });
}

/// Whether the page shows a countdown, so it's drawn again every second.
pub fn counting_down(app: &App) -> bool {
    app.bt.open && app.bt.visible_until.is_some_and(|u| u > Instant::now())
}

fn deliver(weak: &slint::Weak<MainWindow>, event: Event) {
    let _ = weak.upgrade_in_event_loop(move |window| {
        APP.with(|app| handle(&mut app.borrow_mut(), event));
        crate::render(&window);
    });
}

/// Watches BlueZ on a thread of its own, hands every snapshot to the page, runs the page's
/// commands, and passes the pairing agent's requests on.
pub fn spawn(weak: slint::Weak<MainWindow>, mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        runtime.block_on(async move {
            loop {
                let result = async {
                    let client = lituus_bt::Client::system().await?;
                    let mut snapshots = Box::pin(client.watch().await?);
                    // Without the agent (another one is the default, say) pairing devices that
                    // ask for nothing still works.
                    let mut agent = match client.register_agent().await {
                        Ok(agent) => Some(agent),
                        Err(error) => {
                            eprintln!("lituus: couldn't register the Bluetooth agent: {error}");
                            None
                        }
                    };
                    // Each search stops by itself after a while, unless a newer one started.
                    let discovery = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                    loop {
                        tokio::select! {
                            snapshot = snapshots.next() => match snapshot {
                                Some(snapshot) => {
                                    // A headset that connected (or went) changes the sound cards.
                                    let audio = snapshot.as_ref().is_ok_and(|s| s.devices.iter().any(|d| d.connected && d.audio()));
                                    deliver(&weak, Event::Snapshot(snapshot));
                                    if audio {
                                        read_audio(&weak);
                                    } else {
                                        deliver(&weak, Event::Audio(Some(Vec::new())));
                                    }
                                }
                                None => break,
                            },
                            Some(command) = commands.recv() => {
                                if let Command::SetAudio { card, profile } = &command {
                                    let (card, profile, weak) = (card.clone(), profile.clone(), weak.clone());
                                    tokio::spawn(async move {
                                        let set = tokio::task::spawn_blocking(move || audio::set_profile(&card, &profile)).await;
                                        if let Ok(Err(error)) = set {
                                            deliver(&weak, Event::AudioFailed(error.to_string()));
                                        }
                                        read_audio(&weak);
                                    });
                                    continue;
                                }
                                if command == Command::Refresh {
                                    deliver(&weak, Event::Snapshot(client.snapshot().await));
                                    continue;
                                }
                                if let Command::Discover { adapter, on: true } = &command {
                                    let generation = discovery.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                                    let (client, discovery, adapter) = (client.clone(), discovery.clone(), adapter.clone());
                                    tokio::spawn(async move {
                                        tokio::time::sleep(DISCOVERY).await;
                                        if discovery.load(std::sync::atomic::Ordering::SeqCst) == generation {
                                            let _ = client.stop_discovery(&adapter).await;
                                        }
                                    });
                                }
                                let (client, weak) = (client.clone(), weak.clone());
                                tokio::spawn(async move {
                                    let event = match run(&client, &command).await {
                                        Ok(()) => Event::Done(command),
                                        Err(Failure::Bluez(error)) => Event::Failed(command, error),
                                        Err(Failure::Rfkill(error)) => Event::UnblockFailed(error),
                                    };
                                    deliver(&weak, event);
                                });
                            }
                            Some(event) = async { agent.as_mut()?.recv().await } => deliver(&weak, Event::Agent(event)),
                        }
                    }
                    Ok::<(), lituus_bt::Error>(())
                }
                .await;
                if let Err(error) = result {
                    deliver(&weak, Event::Snapshot(Err(error)));
                }
                // BlueZ's bus connection broke, or there's no system bus: try again shortly. A
                // command sent meanwhile ("Check again") tries at once.
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                    Some(_) = commands.recv() => {}
                }
            }
        });
    });
}

/// Reads the sound cards in the background and hands them to the page.
fn read_audio(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    tokio::spawn(async move {
        let cards = tokio::task::spawn_blocking(audio::cards).await;
        // No sound server (or a broken one): the audio choice isn't shown.
        deliver(&weak, Event::Audio(cards.ok().and_then(Result::ok)));
    });
}

enum Failure {
    Bluez(lituus_bt::Error),
    Rfkill(String),
}

impl From<lituus_bt::Error> for Failure {
    fn from(error: lituus_bt::Error) -> Self {
        Self::Bluez(error)
    }
}

async fn run(client: &lituus_bt::Client, command: &Command) -> Result<(), Failure> {
    match command {
        Command::Power { adapter, on } => client.set_powered(adapter, *on).await?,
        Command::Discoverable { adapter, on, timeout } => client.set_discoverable(adapter, *on, *timeout).await?,
        Command::Pairable { adapter, on } => client.set_pairable(adapter, *on).await?,
        Command::AdapterAlias { adapter, alias } => client.set_adapter_alias(adapter, alias).await?,
        Command::Discover { adapter, on: true } => client.start_discovery(adapter).await?,
        Command::Discover { adapter, on: false } => client.stop_discovery(adapter).await?,
        Command::Pair { device } => {
            client.pair(device).await?;
            // Trusted, so it reconnects by itself, as other Bluetooth managers do.
            client.set_trusted(device, true).await?;
            // A device with nothing this computer can use (a phone without audio, say) refuses;
            // it's still paired.
            let _ = client.connect(device).await;
        }
        Command::CancelPairing { device } => client.cancel_pairing(device).await?,
        Command::Connect { device } => client.connect(device).await?,
        Command::Disconnect { device } => client.disconnect(device).await?,
        Command::Trust { device, on } => client.set_trusted(device, *on).await?,
        Command::Block { device, on } => client.set_blocked(device, *on).await?,
        Command::Rename { device, alias } => client.set_alias(device, alias).await?,
        Command::Forget { adapter, device } => client.remove(adapter, device).await?,
        Command::Unblock => unblock().map_err(|e| Failure::Rfkill(e.to_string()))?,
        // Both handled before they get here.
        Command::Refresh | Command::SetAudio { .. } => {}
    }
    Ok(())
}

/// Lifts rfkill's soft block on every Bluetooth radio, as `rfkill unblock bluetooth` does: one
/// `struct rfkill_event` (idx, type, op, soft, hard) written to /dev/rfkill. Desktop sessions
/// may write it (systemd gives the logged-in user access).
fn unblock() -> std::io::Result<()> {
    const TYPE_BLUETOOTH: u8 = 2;
    const OP_CHANGE_ALL: u8 = 3;
    use std::io::Write;
    let event = [0, 0, 0, 0, TYPE_BLUETOOTH, OP_CHANGE_ALL, 0, 0];
    std::fs::OpenOptions::new().write(true).open("/dev/rfkill")?.write_all(&event)
}

#[cfg(test)]
mod tests {
    use slint::Model;
    use lituus_bt::{PowerState, uuid};

    use super::*;

    fn adapter() -> Adapter {
        Adapter {
            path: "/org/bluez/hci0".into(),
            name: "hci0".into(),
            address: "00:1A:7D:DA:71:13".into(),
            alias: "thinkpad-x1".into(),
            powered: true,
            power_state: PowerState::On,
            discoverable: false,
            discoverable_timeout: 180,
            pairable: true,
            discovering: true,
        }
    }

    fn device(n: u8, alias: &str, icon: &str) -> Device {
        Device {
            path: format!("/org/bluez/hci0/dev_{n}"),
            adapter: "/org/bluez/hci0".into(),
            address: format!("AA:BB:CC:00:00:{n:02X}"),
            name: Some(alias.into()),
            alias: alias.into(),
            icon: icon.into(),
            class: 0,
            paired: false,
            bonded: false,
            trusted: false,
            blocked: false,
            connected: false,
            uuids: Vec::new(),
            rssi: None,
            battery: None,
            legacy_pairing: false,
        }
    }

    /// The mockup's: headphones connected, a phone paired, a keyboard and a beacon nearby.
    fn state() -> State {
        let mut headphones = device(1, "Sony WH-1000XM5", "audio-headphones");
        headphones.paired = true;
        headphones.connected = true;
        headphones.battery = Some(82);
        headphones.uuids = vec![uuid::A2DP_SINK.into(), uuid::HFP_HANDS_FREE.into()];
        let mut phone = device(2, "Pixel 8", "phone");
        phone.paired = true;
        phone.battery = Some(54);
        let mut keyboard = device(3, "MX Keys S", "input-keyboard");
        keyboard.rssi = Some(-58);
        let mut beacon = device(4, "", "");
        beacon.name = None;
        beacon.alias = "AA-BB-CC-00-00-04".into();
        beacon.rssi = Some(-40);
        let mut far = device(5, "Pixel 7a", "phone");
        far.rssi = Some(-75);
        State {
            snapshot: Some(Snapshot {
                adapters: vec![adapter()],
                devices: vec![phone, keyboard, beacon, headphones, far],
            }),
            ..State::default()
        }
    }

    fn rows(page: &BtPage) -> Vec<(String, Vec<String>)> {
        page.sections
            .iter()
            .map(|s| (s.title.to_string(), s.rows.iter().map(|r| format!("{} · {}", r.name, r.meta)).collect()))
            .collect()
    }

    #[test]
    fn paired_devices_come_first_then_named_ones_nearby_by_signal() {
        let (page, ids, selected) = page(&state(), Instant::now());
        assert_eq!(page.state, "list");
        assert_eq!(
            rows(&page),
            [
                (
                    "MY DEVICES".into(),
                    vec!["Sony WH-1000XM5 · Connected · headphones".into(), "Pixel 8 · Paired · phone".into()]
                ),
                ("NEARBY".into(), vec!["MX Keys S · Keyboard".into(), "Pixel 7a · Phone".into()]),
            ]
        );
        assert_eq!(ids.len(), 4);
        assert_eq!(selected.as_deref(), Some("/org/bluez/hci0/dev_1"));
        let headphones = page.sections.row_data(0).unwrap().rows.row_data(0).unwrap();
        // A connected device shows its battery; a paired one that isn't, a Connect button.
        assert_eq!((headphones.battery, headphones.button.as_str()), (82, ""));
        let phone = page.sections.row_data(0).unwrap().rows.row_data(1).unwrap();
        assert_eq!((phone.battery, phone.button.as_str()), (-1, "Connect"));
        let keyboard = page.sections.row_data(1).unwrap().rows.row_data(0).unwrap();
        assert_eq!((keyboard.bars, keyboard.button.as_str()), (4, "Pair"));
        assert_eq!(page.sections.row_data(1).unwrap().busy, "Looking for devices");
    }

    #[test]
    fn details_of_a_connected_and_a_nearby_device() {
        let (page, ..) = page(&state(), Instant::now());
        let d = page.details;
        assert_eq!(
            (d.title.as_str(), d.status.as_str(), d.status_extra.as_str()),
            ("Sony WH-1000XM5", "Connected", "· battery 82 %")
        );
        assert!(d.renamable);
        let actions: Vec<String> = d.actions.iter().map(|a| a.label.to_string()).collect();
        assert_eq!(actions, ["Disconnect", "Forget"]);
        let chips: Vec<String> =
            d.chips.iter().flat_map(|r| r.chips.iter().map(|c| c.label.to_string()).collect::<Vec<_>>()).collect();
        assert_eq!(chips, ["Audio sink (A2DP)", "Hands-free (HFP)"]);

        let mut state = state();
        state.selected = Some("/org/bluez/hci0/dev_3".into());
        let d = super::page(&state, Instant::now()).0.details;
        assert_eq!((d.status.as_str(), d.status_extra.as_str()), ("Not paired", "· in range"));
        assert_eq!(
            d.intro,
            "Pairing lets MX Keys S and this computer remember each other. You'll be asked to type a code on the keyboard."
        );
        let items: Vec<String> = d.items.iter().map(|i| format!("{}: {}", i.label, i.value)).collect();
        assert_eq!(
            items,
            [
                "Signal: 70 % (-58 dBm)",
                "Address: AA:BB:CC:00:00:03",
                "Type: Keyboard",
                "Pairing: Type a code on the device"
            ]
        );
    }

    #[test]
    fn a_failed_pairing_marks_the_row_and_offers_to_try_again() {
        let mut state = state();
        let keyboard = "/org/bluez/hci0/dev_3".to_owned();
        state.send(Command::Pair { device: keyboard.clone() });
        let mut app = App { bt: state, ..App::default() };
        let refused = lituus_bt::Error::Refused { kind: "AuthenticationFailed".into(), message: String::new() };
        handle(&mut app, Event::Failed(Command::Pair { device: keyboard.clone() }, refused));
        let state = &mut app.bt;
        state.selected = Some(keyboard.clone());
        let (page, ..) = page(state, Instant::now());
        assert_eq!(page.message_title, "Couldn't pair with MX Keys S");
        assert_eq!(page.message, "The device didn't accept the pairing.");
        assert_eq!(page.message_action, "Try again");
        assert_eq!(rows(&page)[1].1[0], "MX Keys S · Couldn't pair · keyboard");
        assert_eq!(page.details.actions.row_data(0).unwrap().label, "Try again");
        assert_eq!(page.details.status_extra, "· last attempt failed");

        // Cancelling isn't a failure to report.
        let cancelled = lituus_bt::Error::Refused { kind: "AuthenticationCanceled".into(), message: String::new() };
        app.bt.message = None;
        app.bt.failed = None;
        handle(&mut app, Event::Failed(Command::Pair { device: keyboard }, cancelled));
        assert!(app.bt.message.is_none() && app.bt.failed.is_none());
    }

    #[test]
    fn the_states_without_a_list() {
        let state_of = |state: &State| page(state, Instant::now()).0;
        let mut state = state();
        state.snapshot.as_mut().unwrap().adapters[0].powered = false;
        let page = state_of(&state);
        assert_eq!((page.state.as_str(), page.empty_action.as_str()), ("off", "Turn on Bluetooth"));
        assert_eq!(nav(&state, true).sub, "Off");

        state.snapshot.as_mut().unwrap().adapters[0].power_state = PowerState::Blocked;
        assert_eq!(state_of(&state).state, "blocked");
        assert_eq!(nav(&state, true).sub, "Blocked");

        state.snapshot.as_mut().unwrap().adapters.clear();
        assert_eq!(state_of(&state).empty_title, "No Bluetooth adapter");

        let not_running =
            State { not_running: true, error: Some("Bluetooth isn't running".into()), ..State::default() };
        let page = state_of(&not_running);
        assert_eq!((page.state.as_str(), page.empty_command.as_str()), ("no-service", NOT_RUNNING_COMMAND));
        assert_eq!(nav(&not_running, false).sub, "Service not running");

        assert_eq!(state_of(&State::default()).state, "loading");
        assert_eq!(nav(&super::tests::state(), false).sub, "1 connected");
    }

    #[test]
    fn visible_counts_down_from_when_lituus_switched_it_on() {
        let mut a = adapter();
        let now = Instant::now();
        assert_eq!(visible_label(&a, None, now), "Hidden");
        a.discoverable = true;
        assert_eq!(visible_label(&a, Some(now + Duration::from_secs(161)), now), "Visible · 2:41");
        // Switched on elsewhere, or with no limit.
        assert_eq!(visible_label(&a, None, now), "Visible");
        a.discoverable_timeout = 0;
        assert_eq!(visible_label(&a, Some(now + Duration::from_secs(161)), now), "Visible");
    }

    #[test]
    fn pairing_dialogs_answer_bluez() {
        let state = state();
        let keyboard = state.device("/org/bluez/hci0/dev_3").cloned();
        let display =
            Prompt::Display { device: "/org/bluez/hci0/dev_3".into(), code: "305718".into(), entered: Some(3) };
        let dialog = pair_dialog(&display, keyboard.as_ref());
        assert_eq!(
            (dialog.mode.as_str(), dialog.title.as_str(), dialog.typed),
            ("display", "Type this code on MX Keys S", 3)
        );
        assert_eq!(dialog.digits.iter().collect::<Vec<_>>(), ["3", "0", "5", "7", "1", "8"]);

        // A passkey that isn't one keeps the dialog open; a good one answers.
        let (reply, mut answer) = tokio::sync::oneshot::channel();
        let request = lituus_bt::agent::PairingRequest::for_tests("/org/bluez/hci0/dev_3", Ask::Passkey, reply);
        let mut state = state;
        state.prompt = Some(Prompt::Ask(request));
        answer_prompt(&mut state, true, "12ab", false);
        assert!(state.prompt.is_some());
        answer_prompt(&mut state, true, " 042 ", false);
        assert!(state.prompt.is_none());
        assert_eq!(answer.try_recv().unwrap(), Answer::Passkey(42));
    }

    #[test]
    fn a_connected_headset_gets_the_audio_choice() {
        let mut state = state();
        let card = |address: &str| audio::Card {
            name: "bluez_card.x".into(),
            address: address.into(),
            profiles: vec![
                audio::Profile { name: "a2dp-sink".into(), description: "(A2DP Sink, codec LDAC)".into(), priority: 2 },
                audio::Profile { name: "headset-head-unit".into(), description: String::new(), priority: 1 },
                audio::Profile { name: "off".into(), description: String::new(), priority: 0 },
            ],
            active: Some("headset-head-unit".into()),
        };
        // No card for it (no PipeWire): no choice.
        assert_eq!(page(&state, Instant::now()).0.audio.row_count(), 0);
        state.audio = vec![card("aa:bb:cc:00:00:01")];
        let page = page(&state, Instant::now()).0;
        let options: Vec<String> = page.audio.iter().map(|o| format!("{} {}", o.label, o.sub)).collect();
        assert_eq!(options, ["High quality A2DP · LDAC", "Headset HFP · microphone on", "Off Controls only"]);
        assert_eq!(page.audio_selected, 1);
        // Not for the phone, which isn't connected.
        state.selected = Some("/org/bluez/hci0/dev_2".into());
        assert_eq!(super::page(&state, Instant::now()).0.audio.row_count(), 0);
    }

    #[test]
    fn chips_break_into_lines() {
        let chip = |l: &str| Chip { label: s(l), on: false };
        let lines = chip_rows(vec![
            chip("Network access (PAN)"),
            chip("Send files (OBEX)"),
            chip("Audio source (A2DP)"),
            chip("Hands-free gateway"),
            chip("Phonebook"),
        ]);
        let counts: Vec<usize> = lines.iter().map(|l| l.chips.row_count()).collect();
        assert_eq!(counts, [2, 2, 1]);
    }
}
