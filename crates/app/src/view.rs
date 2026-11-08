//! What the window shows, worked out from a NetworkManager snapshot and the UI state (page,
//! selection, search). Pure, so it's tested without a window.

use lituus_nm::{
    ActiveState, Band, ConnectionKind, Device, DeviceKind, KeyMgmt, Network, SavedConnection, Security, Snapshot,
    WifiJoin, bars, channel,
};
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::{Action, DetailItem, Details, Hint, Row, Section, Toggle};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Wifi,
    Wired,
    Vpn,
    Hotspot,
    Bluetooth,
    Displays,
    Sound,
    Keyboard,
    Mouse,
    Shortcuts,
    Wallpaper,
    System,
    Settings,
}

impl Page {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "wifi" => Self::Wifi,
            "wired" => Self::Wired,
            "vpn" => Self::Vpn,
            "hotspot" => Self::Hotspot,
            "bluetooth" => Self::Bluetooth,
            "displays" => Self::Displays,
            "sound" => Self::Sound,
            "keyboard" => Self::Keyboard,
            "mouse" => Self::Mouse,
            "shortcuts" => Self::Shortcuts,
            "wallpaper" => Self::Wallpaper,
            "system" => Self::System,
            // Lituus's own look moved to Settings.
            "settings" | "appearance" => Self::Settings,
            _ => return None,
        })
    }

    /// The page a sidebar item opens: Network opens its sub-page used last, Input its first.
    pub fn open(id: &str, last_network: Self) -> Option<Self> {
        match id {
            "network" if last_network.is_network() => Some(last_network),
            "network" => Some(Self::Wifi),
            "input" => Some(Self::Keyboard),
            _ => Self::from_id(id),
        }
    }

    /// A page under Network in the sidebar.
    pub fn is_network(self) -> bool {
        matches!(self, Self::Wifi | Self::Wired | Self::Vpn | Self::Hotspot)
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Wired => "wired",
            Self::Vpn => "vpn",
            Self::Hotspot => "hotspot",
            Self::Bluetooth => "bluetooth",
            Self::Displays => "displays",
            Self::Sound => "sound",
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
            Self::Shortcuts => "shortcuts",
            Self::Wallpaper => "wallpaper",
            Self::System => "system",
            Self::Settings => "settings",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UiState {
    pub page: Page,
    /// The selected row's id; `None` picks the first one.
    pub selected: Option<String>,
    pub search: String,
    /// The row whose last connection attempt failed, and why.
    pub failed: Option<(String, String)>,
    /// The connections (by UUID) whose sign-in window is open.
    pub signing_in: Vec<String>,
    /// Airplane mode is on (it turned Wi-Fi off, so the Wi-Fi page says so).
    pub airplane: bool,
}

/// Goes to the page of a sidebar item (or the tray's, or the search's), with nothing selected.
/// `last_network` is the Network sub-page used last, which clicking Network opens again.
pub fn navigate(state: &mut UiState, last_network: &mut Page, id: &str) -> Option<Page> {
    let page = Page::open(id, *last_network)?;
    if page.is_network() {
        *last_network = page;
    }
    *state = UiState { page, ..UiState::default() };
    Some(page)
}

/// An empty state shown instead of the list. No icon: still reading, with a spinner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Empty {
    pub icon: &'static str,
    pub title: String,
    pub text: String,
    /// A command that fixes it, shown with a Copy button.
    pub command: String,
    pub action: String,
    /// The page has nothing else to show (Wi-Fi off, no VPN yet), so the hero is only its title
    /// and line; not for a search that finds nothing or a list that's empty for now.
    pub page: bool,
}

/// The command that starts NetworkManager, for its empty state.
pub const NOT_RUNNING_COMMAND: &str = "systemctl enable --now NetworkManager";

/// The short statuses of Network's sub-items in the sidebar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetStatus {
    /// "Lumen Home", "not connected", "off".
    pub wifi: String,
    /// "1000 Mb/s", "connected", "unplugged".
    pub wired: String,
    /// The VPN that's on, or "off".
    pub vpn: String,
    /// "on" or "off".
    pub hotspot: String,
}

/// Everything the window shows.
#[derive(Default)]
pub struct View {
    /// The Network item's short status in the sidebar ("Home", "Off").
    pub net_status: String,
    pub net_items: NetStatus,
    /// The hero of the Network page shown.
    pub net_hero: NetHero,
    /// Left of the search field: "4 networks in range".
    pub count: String,
    pub wifi_available: bool,
    pub page_title: String,
    pub show_wifi_switch: bool,
    pub wifi_on: bool,
    pub show_rescan: bool,
    pub show_add: bool,
    pub show_search: bool,
    pub search_placeholder: String,
    pub bottom_label: String,
    pub sections: Vec<Section>,
    /// The rows' ids in display order, for moving the selection with the arrow keys.
    pub row_ids: Vec<String>,
    pub selected: Option<String>,
    pub details: Option<Details>,
    pub empty: Option<Empty>,
    pub banner: String,
    pub hostname: String,
    pub hotspot: Option<HotspotView>,
}

/// A Network page's hero: the line under its title, and what the page is about (the Wi-Fi
/// network, the cable, the VPN that's on, the hotspot). Its actions are the details pane's, so
/// it has no buttons of its own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetHero {
    pub sub: String,
    pub icon: &'static str,
    pub name: String,
    pub meta: String,
    pub tone: &'static str,
    /// The interface and its address, in monospace.
    pub detail: String,
}

/// The hotspot page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HotspotView {
    /// The hotspot connection, once there is one.
    pub connection: Option<String>,
    /// Its active connection, while it's on.
    pub active: Option<String>,
    /// The Wi-Fi device it runs on, and that device's interface.
    pub device: Option<String>,
    pub interface: String,
    pub ssid: String,
    pub status: String,
    pub warning: String,
}

/// A row before the selection is known.
#[derive(Default)]
struct RowData {
    id: String,
    name: String,
    meta: String,
    tone: &'static str,
    icon: &'static str,
    bars: u8,
    secure: bool,
    /// Under the row before it: a port, or a VLAN.
    indent: bool,
}

fn s(text: impl Into<SharedString>) -> SharedString {
    text.into()
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

fn action(id: &str, label: &str, kind: &str) -> Action {
    Action { id: s(id), label: s(label), kind: s(kind), enabled: true }
}

pub(crate) fn toggle(id: &str, label: &str, sub: &str, on: bool) -> Toggle {
    Toggle { id: s(id), label: s(label), sub: s(sub), on }
}

fn item(label: &str, value: impl Into<SharedString>, mono: bool) -> DetailItem {
    DetailItem { label: s(label), value: value.into(), mono }
}

struct DetailsData {
    title: String,
    status: String,
    status_tone: &'static str,
    status_extra: String,
    actions: Vec<Action>,
    toggles: Vec<Toggle>,
    /// A saved connection's Metered choice, as the editor has it: 0 Automatic, 1 Yes, 2 No.
    metered: Option<i32>,
    items: Vec<DetailItem>,
    note: String,
}

/// The Metered choice of a saved connection (0 Automatic, 1 Yes, 2 No: NetworkManager's numbers).
fn metered_choice(c: &SavedConnection) -> i32 {
    match c.metered {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    }
}

impl From<DetailsData> for Details {
    fn from(d: DetailsData) -> Self {
        Details {
            title: s(d.title),
            status: s(d.status),
            status_tone: s(d.status_tone),
            status_extra: s(d.status_extra),
            actions: model(d.actions),
            toggles: model(d.toggles),
            has_metered: d.metered.is_some(),
            metered: d.metered.unwrap_or_default(),
            items: model(d.items),
            note: s(d.note),
            ..Details::default()
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn bands_label(bands: &[Band]) -> String {
    bands.iter().map(|b| b.label()).collect::<Vec<_>>().join(" / ")
}

/// "WPA2/WPA3 Personal", as the details say it.
pub fn security_long(security: Security) -> String {
    match security {
        Security::Wpa | Security::Wpa2 | Security::Wpa2Wpa3 | Security::Wpa3 => {
            format!("{} Personal", security.label())
        }
        other => other.label().to_owned(),
    }
}

fn active_uuid_state<'a>(snapshot: &'a Snapshot, uuid: &str) -> Option<&'a lituus_nm::ActiveConnection> {
    snapshot.active.iter().find(|a| a.uuid == uuid)
}

fn matches_search(name: &str, search: &str) -> bool {
    search.is_empty() || name.to_lowercase().contains(&search.to_lowercase())
}

/// IP details of a connected device, as label/value pairs.
/// "18.4 MiB" for a byte count.
fn bytes_label(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{n} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}

fn traffic_item(device: &Device) -> Option<DetailItem> {
    device.traffic.map(|(rx, tx)| item("Received / sent", format!("{} / {}", bytes_label(rx), bytes_label(tx)), false))
}

fn ip_items(device: &Device) -> Vec<DetailItem> {
    let mut items = Vec::new();
    if let Some(ip4) = &device.ip4 {
        if let Some((a, p)) = ip4.addresses.first() {
            items.push(item("IPv4 address", format!("{a}/{p}"), true));
        }
        if let Some(gw) = &ip4.gateway {
            items.push(item("Gateway", gw.as_str(), true));
        }
        if !ip4.dns.is_empty() {
            items.push(item("DNS", ip4.dns.join(", "), true));
        }
    }
    if let Some(ip6) = &device.ip6 {
        // The global address if there is one, rather than the link-local fe80:: one.
        let address = ip6.addresses.iter().find(|(a, _)| !a.starts_with("fe80")).or(ip6.addresses.first());
        if let Some((a, p)) = address {
            items.push(item("IPv6 address", format!("{a}/{p}"), true));
        }
    }
    items
}

pub fn build(snapshot: Option<&Snapshot>, error: Option<&str>, state: &UiState) -> View {
    let mut view = View { show_search: true, wifi_on: true, ..View::default() };
    view.hostname = snapshot.map(|s| s.hostname.clone()).unwrap_or_default();
    let Some(snapshot) = snapshot else {
        view.page_title = page_title(state.page).into();
        view.show_search = false;
        view.net_hero = NetHero {
            icon: page_icon(state.page),
            name: if error.is_some() { "Not available".into() } else { String::new() },
            tone: "muted",
            ..NetHero::default()
        };
        view.banner = match error {
            Some(e) => format!("{e}. Lituus needs it to manage connections."),
            None => String::new(),
        };
        view.empty = Some(match error {
            Some(_) => Empty {
                icon: "warning",
                title: "NetworkManager isn't running".into(),
                text: "Lituus manages connections through NetworkManager. Start it with:".into(),
                command: NOT_RUNNING_COMMAND.into(),
                page: true,
                ..Empty::default()
            },
            None => Empty { title: "Reading NetworkManager…".into(), ..Empty::default() },
        });
        return view;
    };
    view.page_title = page_title(state.page).into();
    network_status(snapshot, &mut view);
    let rows = match state.page {
        Page::Wifi => wifi_page(snapshot, state, &mut view),
        Page::Wired => wired_page(snapshot, &mut view),
        Page::Vpn => vpn_page(snapshot, state, &mut view),
        Page::Hotspot => {
            view.show_search = false;
            let hotspot = hotspot_page(snapshot);
            view.net_hero = hotspot_hero(&hotspot);
            view.hotspot = Some(hotspot);
            Vec::new()
        }
        // Bluetooth has a page of its own (bluetooth.rs).
        Page::Bluetooth
        | Page::Displays
        | Page::Sound
        | Page::Keyboard
        | Page::Mouse
        | Page::Shortcuts
        | Page::Wallpaper
        | Page::System
        | Page::Settings => Vec::new(),
    };
    // Rows grouped by section title, the selection resolved, and the selected row's details.
    view.row_ids = rows.iter().flat_map(|(_, r)| r.iter().map(|r| r.id.clone())).collect();
    let selected =
        state.selected.clone().filter(|id| view.row_ids.contains(id)).or_else(|| view.row_ids.first().cloned());
    view.sections = rows
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(title, rows)| Section {
            title: s(title.to_uppercase()),
            rows: model(
                rows.into_iter()
                    .map(|r| Row {
                        selected: selected.as_deref() == Some(r.id.as_str()),
                        id: s(r.id),
                        name: s(r.name),
                        meta: s(r.meta),
                        tone: s(r.tone),
                        icon: s(r.icon),
                        bars: i32::from(r.bars),
                        secure: r.secure,
                        indent: r.indent,
                    })
                    .collect(),
            ),
        })
        .collect();
    if let Some(id) = &selected {
        view.details = details(snapshot, state, id).map(Details::from);
    }
    if view.empty.is_none() && view.row_ids.is_empty() && state.page.is_network() {
        view.empty = Some(nothing_here(state.page, !state.search.is_empty()));
    }
    view.selected = selected;
    view
}

/// The keys that work on a Network page now, for the bar at the bottom: the search, the list,
/// and what Enter and e do to the selected row (the details pane's Connect and Edit).
pub fn keys(view: &View) -> Vec<Hint> {
    let mut keys = Vec::new();
    if view.show_search {
        keys.push(Hint::new("/", "Search"));
    }
    if !view.row_ids.is_empty() {
        keys.push(Hint::new("↑↓", "Select"));
    }
    if let Some(d) = &view.details {
        let action = |id: &str| d.actions.iter().find(|a| a.id == id && a.enabled);
        if let Some(connect) = action("connect") {
            keys.push(Hint::new("↵", &connect.label));
        }
        if action("edit").is_some() {
            keys.push(Hint::new("e", "Edit"));
        }
    }
    keys
}

fn page_title(page: Page) -> &'static str {
    match page {
        Page::Wifi => "Wi-Fi",
        Page::Wired => "Wired",
        Page::Vpn => "VPN",
        Page::Hotspot => "Hotspot",
        Page::Bluetooth => "Bluetooth",
        Page::Displays => "Displays",
        Page::Sound => "Sound",
        Page::Keyboard => "Keyboard",
        Page::Mouse => "Mouse & touchpad",
        Page::Shortcuts => "Shortcuts",
        Page::Wallpaper => "Wallpaper",
        Page::System => "System",
        Page::Settings => "Settings",
    }
}

fn page_icon(page: Page) -> &'static str {
    match page {
        Page::Wired => "wired",
        Page::Vpn => "vpn",
        Page::Hotspot => "hotspot",
        _ => "wifi",
    }
}

/// "1 VPN", "3 VPNs".
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn nothing_here(page: Page, searching: bool) -> Empty {
    if searching {
        return Empty {
            icon: "search",
            title: "Nothing found".into(),
            text: "No name matches the search.".into(),
            ..Empty::default()
        };
    }
    match page {
        Page::Wired => Empty {
            icon: "wired",
            title: "No wired network card".into(),
            text: "Plug in a USB network adapter to see it here.".into(),
            page: true,
            ..Empty::default()
        },
        Page::Vpn => Empty {
            icon: "vpn",
            title: "No VPN connections".into(),
            text: "Add a WireGuard, OpenVPN or AnyConnect connection with +, or bring one in from a .conf or .ovpn file, or from openconnect-saml.".into(),
            action: "Import a file…".into(),
            page: true,
            ..Empty::default()
        },
        _ => Empty {
            icon: "wifi-off",
            title: "No networks in range".into(),
            text: "Try rescanning.".into(),
            ..Empty::default()
        },
    }
}

/// The hotspot's state, for its page and its switch.
pub fn hotspot_view(snap: &Snapshot) -> HotspotView {
    hotspot_page(snap)
}

fn hotspot_page(snap: &Snapshot) -> HotspotView {
    let device = wifi_device(snap);
    let saved = snap.connections.iter().find(|c| c.hotspot);
    let active = saved.and_then(|c| active_uuid_state(snap, &c.uuid)).filter(|a| a.state != ActiveState::Deactivated);
    // What it shares: the connection with the default route.
    let sharing = snap
        .primary_connection
        .as_deref()
        .and_then(|p| snap.active_connection(p))
        .filter(|a| Some(&a.uuid) != saved.map(|c| &c.uuid))
        .map(|a| {
            let iface = a
                .devices
                .first()
                .and_then(|d| snap.device(d))
                .map(|d| format!(" ({})", d.interface))
                .unwrap_or_default();
            format!("{}{iface}", a.id)
        });
    let (status, warning) = match (active, &sharing) {
        (Some(a), _) if a.state == ActiveState::Activating => ("Starting…".to_owned(), String::new()),
        (Some(_), Some(s)) => (format!("On · sharing {s}"), String::new()),
        (Some(_), None) => ("On · no internet connection to share".to_owned(), String::new()),
        (None, _) => {
            let wifi_up = device.is_some_and(|d| d.active_connection.is_some());
            let warning = if wifi_up {
                "Most Wi-Fi cards can't run a hotspot and stay on a network at the same time, so turning it on may disconnect Wi-Fi.".to_owned()
            } else {
                String::new()
            };
            ("Off".to_owned(), warning)
        }
    };
    HotspotView {
        connection: saved.map(|c| c.path.clone()),
        active: active.map(|a| a.path.clone()),
        device: device.map(|d| d.path.clone()),
        interface: device.map(|d| d.interface.clone()).unwrap_or_default(),
        ssid: saved
            .and_then(|c| c.wifi_ssid())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_else(|| snap.hostname.clone()),
        status,
        warning,
    }
}

pub(crate) fn wifi_device(snapshot: &Snapshot) -> Option<&Device> {
    snapshot.devices.iter().find(|d| d.kind == DeviceKind::Wifi)
}

/// What the computer is online through: the primary connection, or else any device's.
fn online_through(snap: &Snapshot) -> Option<&lituus_nm::ActiveConnection> {
    snap.primary_connection.as_deref().and_then(|p| snap.active_connection(p)).filter(|a| !a.vpn).or_else(|| {
        snap.devices
            .iter()
            .filter(|d| matches!(d.kind, DeviceKind::Wifi | DeviceKind::Ethernet))
            .find_map(|d| d.active_connection.as_deref().and_then(|a| snap.active_connection(a)))
    })
}

/// The active connection on the first device of a kind that has one.
fn active_on(snap: &Snapshot, kind: DeviceKind) -> Option<(&Device, &lituus_nm::ActiveConnection)> {
    snap.devices
        .iter()
        .filter(|d| d.kind == kind)
        .find_map(|d| d.active_connection.as_deref().and_then(|a| snap.active_connection(a)).map(|a| (d, a)))
}

/// The VPNs that are on or coming on: NetworkManager's VPN plugins, and WireGuard.
fn active_vpns(snap: &Snapshot) -> Vec<&lituus_nm::ActiveConnection> {
    snap.active
        .iter()
        .filter(|a| a.state != ActiveState::Deactivated)
        .filter(|a| {
            a.vpn
                || a.connection
                    .as_deref()
                    .and_then(|c| snap.connection(c))
                    .is_some_and(|c| c.kind == ConnectionKind::WireGuard)
        })
        .collect()
}

fn is_hotspot(snap: &Snapshot, active: &lituus_nm::ActiveConnection) -> bool {
    snap.connections.iter().any(|c| c.hotspot && c.uuid == active.uuid)
}

/// The Network item's sidebar status, and its sub-items'.
fn network_status(snap: &Snapshot, view: &mut View) {
    view.wifi_available = wifi_device(snap).is_some();
    view.net_status = if snap.wireless_off() {
        "Off".to_owned()
    } else {
        online_through(snap).map_or_else(|| "Offline".to_owned(), |a| a.id.clone())
    };
    let wifi = match active_on(snap, DeviceKind::Wifi) {
        _ if !view.wifi_available => "none".to_owned(),
        _ if !snap.wireless_hardware_enabled => "blocked".to_owned(),
        _ if !snap.wireless_enabled => "off".to_owned(),
        Some((_, a)) if is_hotspot(snap, a) => "hotspot".to_owned(),
        Some((_, a)) => a.id.clone(),
        None => "not connected".to_owned(),
    };
    let wired_devices: Vec<&Device> = snap.devices.iter().filter(|d| d.kind == DeviceKind::Ethernet).collect();
    let wired = match active_on(snap, DeviceKind::Ethernet) {
        Some((_, a)) if a.state == ActiveState::Activating => "connecting…".to_owned(),
        Some((d, _)) => match d.wired.as_ref().map(|w| w.speed) {
            Some(speed) if speed > 0 => format!("{speed} Mb/s"),
            _ => "connected".to_owned(),
        },
        None if wired_devices.is_empty() => "none".to_owned(),
        None if wired_devices.iter().all(|d| d.wired.as_ref().is_some_and(|w| !w.carrier)) => "unplugged".to_owned(),
        None => "not connected".to_owned(),
    };
    let vpn = match active_vpns(snap).as_slice() {
        [] => "off".to_owned(),
        [one] => one.id.clone(),
        more => format!("{} on", more.len()),
    };
    let hotspot = if hotspot_page(snap).active.is_some() { "on" } else { "off" };
    view.net_items = NetStatus { wifi, wired, vpn, hotspot: hotspot.to_owned() };
}

/// "wlp2s0   192.168.1.42": a device's interface and IPv4 address.
fn device_line(device: Option<&Device>) -> String {
    let Some(d) = device else { return String::new() };
    let ip = d.ip4.as_ref().and_then(|ip| ip.addresses.first()).map(|(a, _)| a.clone()).unwrap_or_default();
    [d.interface.clone(), ip].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("   ")
}

/// The saved Wi-Fi connections, hotspot included.
fn saved_wifi(snap: &Snapshot) -> impl Iterator<Item = &SavedConnection> {
    snap.connections.iter().filter(|c| matches!(c.kind, ConnectionKind::Wifi { .. }))
}

/// "Last used 3 days ago", from a connection's timestamp.
fn last_used(timestamp: u64, now: u64) -> String {
    const DAY: u64 = 86_400;
    if timestamp == 0 {
        return "Saved".into();
    }
    match now.saturating_sub(timestamp) / DAY {
        0 => "Last used today".into(),
        1 => "Last used yesterday".into(),
        days @ 2..60 => format!("Last used {days} days ago"),
        days @ 60..730 => format!("Last used {} months ago", days / 30),
        days => format!("Last used {} years ago", days / 365),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The Wi-Fi page's hero: the network it's connected to, and its buttons.
fn wifi_hero(snap: &Snapshot, device: &Device, networks: &[Network], out_of_range: usize) -> NetHero {
    let saved_out = match out_of_range {
        0 => None,
        1 => Some("1 saved one out of range".to_owned()),
        n => Some(format!("{n} saved ones out of range")),
    };
    let active = device.active_connection.as_deref().and_then(|a| snap.active_connection(a));
    let hotspot = active.is_some_and(|a| is_hotspot(snap, a));
    let Some(active) = active.filter(|_| !hotspot) else {
        let sub = std::iter::once(count(networks.len(), "network in range", "networks in range"))
            .chain(saved_out)
            .collect::<Vec<_>>()
            .join(" · ");
        let meta = if hotspot { "The hotspot is using the Wi-Fi card" } else { "Pick a network below to connect" };
        return NetHero {
            sub,
            icon: "wifi",
            name: "Not connected".into(),
            meta: meta.into(),
            tone: "muted",
            ..NetHero::default()
        };
    };
    // A hidden network isn't in the list.
    let network = networks.iter().find(|n| n.active);
    let others = match networks.len().saturating_sub(usize::from(network.is_some())) {
        0 => "no other networks in range".to_owned(),
        n => count(n, "other network in range", "other networks in range"),
    };
    let sub = [active.id.clone(), others].into_iter().chain(saved_out).collect::<Vec<_>>().join(" · ");
    let wifi = device.wifi.as_ref();
    // NetworkManager names the access point in use; failing that, the network's strongest.
    let ap = wifi
        .and_then(|w| w.active_access_point.as_deref().and_then(|p| w.access_points.iter().find(|ap| ap.path == p)))
        .or_else(|| network.map(|n| n.strongest()));
    let mut meta = vec![if active.state == ActiveState::Activated { "Connected" } else { "Connecting…" }.to_owned()];
    if let Some(band) = ap.and_then(|ap| Band::from_mhz(ap.frequency)) {
        meta.push(band.label().to_owned());
    }
    if let Some(w) = wifi.filter(|w| w.bitrate > 0) {
        meta.push(format!("{} Mb/s", w.bitrate / 1000));
    }
    NetHero {
        sub,
        icon: "wifi",
        name: active.id.clone(),
        meta: meta.join(" · "),
        tone: "accent",
        detail: device_line(Some(device)),
    }
}

/// The Wired page's hero: the wired connection in use, or the cable's state.
fn wired_hero(snap: &Snapshot) -> NetHero {
    let devices: Vec<&Device> = snap.devices.iter().filter(|d| d.kind == DeviceKind::Ethernet).collect();
    let profiles =
        match snap.connections.iter().filter(|c| c.kind == ConnectionKind::Ethernet && c.port.is_none()).count() {
            0 => "no saved profiles".to_owned(),
            n => count(n, "saved profile", "saved profiles"),
        };
    if let Some((device, active)) = active_on(snap, DeviceKind::Ethernet) {
        let connected = active.state == ActiveState::Activated;
        return NetHero {
            sub: format!("{} · {profiles}", if connected { "Connected" } else { "Connecting" }),
            icon: "wired",
            name: active.id.clone(),
            meta: format!("{}{}", if connected { "Connected" } else { "Connecting…" }, speed(device)),
            tone: "accent",
            detail: device_line(Some(device)),
        };
    }
    let Some(device) = devices.first() else {
        return NetHero {
            sub: "No wired device".into(),
            icon: "wired",
            name: "No wired network card".into(),
            meta: "Plug in a USB network adapter to use one".into(),
            tone: "muted",
            ..NetHero::default()
        };
    };
    let unplugged = devices.iter().all(|d| d.wired.as_ref().is_some_and(|w| !w.carrier));
    let (sub, name, meta) = if unplugged {
        ("Cable unplugged", "Cable unplugged", "Plug in a cable to connect")
    } else {
        ("Cable plugged in", "Not connected", "Pick a profile below to connect")
    };
    NetHero {
        sub: format!("{sub} · {profiles}"),
        icon: "wired",
        name: name.into(),
        meta: meta.into(),
        tone: "muted",
        detail: [device.interface.as_str(), device.hw_address.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("   "),
    }
}

/// The VPN page's hero: the VPN that's on, or else the one used last, to connect.
fn vpn_hero(snap: &Snapshot) -> NetHero {
    let vpns: Vec<&SavedConnection> = snap
        .connections
        .iter()
        .filter(|c| matches!(c.kind, ConnectionKind::WireGuard | ConnectionKind::Vpn { .. }))
        .collect();
    let saved = count(vpns.len(), "saved", "saved");
    let active = active_vpns(snap);
    if let Some(a) = active.first() {
        let connection = snap.connections.iter().find(|c| c.uuid == a.uuid);
        let connected = a.state == ActiveState::Activated;
        let mut meta = vec![if connected { "Connected" } else { "Connecting…" }.to_owned()];
        meta.extend(connection.map(connection_kind_label));
        let on = if active.len() == 1 { "On".to_owned() } else { format!("{} on", active.len()) };
        return NetHero {
            sub: format!("{on} · {saved}"),
            icon: "vpn",
            name: a.id.clone(),
            meta: meta.join(" · "),
            tone: "accent",
            detail: device_line(a.devices.first().and_then(|d| snap.device(d))),
        };
    }
    NetHero {
        sub: if vpns.is_empty() { "Off".into() } else { format!("Off · {saved}") },
        icon: "vpn",
        name: "No VPN on".into(),
        meta: match vpns.len() {
            0 => "Add one with +".into(),
            n => count(n, "VPN saved", "VPNs saved"),
        },
        tone: "muted",
        ..NetHero::default()
    }
}

/// The Hotspot page's hero: whether it's on, and what it shares.
fn hotspot_hero(h: &HotspotView) -> NetHero {
    let hero = NetHero { sub: h.status.clone(), icon: "hotspot", tone: "accent", ..NetHero::default() };
    match (&h.device, &h.active) {
        (None, _) => NetHero {
            name: "No Wi-Fi card".into(),
            meta: "A hotspot needs one to share the connection over".into(),
            tone: "muted",
            ..hero
        },
        (Some(_), None) => NetHero {
            name: "Hotspot off".into(),
            meta: "Share this computer's connection over Wi-Fi".into(),
            tone: "muted",
            ..hero
        },
        (Some(_), Some(_)) => NetHero {
            name: if h.status.starts_with("On") { "Hotspot on".into() } else { "Starting the hotspot…".into() },
            meta: format!("Others join “{}” with the password or the QR code below", h.ssid),
            detail: h.interface.clone(),
            ..hero
        },
    }
}

pub(crate) fn network_id(n: &Network) -> String {
    format!("wifi:{}:{}", hex(&n.ssid), n.security.label())
}

fn wifi_page(snap: &Snapshot, state: &UiState, view: &mut View) -> Vec<(String, Vec<RowData>)> {
    view.show_wifi_switch = true;
    view.wifi_on = snap.wireless_enabled && snap.wireless_hardware_enabled;
    view.show_rescan = true;
    view.search_placeholder = "Search networks".into();
    let saved = saved_wifi(snap).count();
    let saved_line = match saved {
        0 => String::new(),
        n => count(n, "saved network", "saved networks"),
    };
    let off = |sub: &str, name: &str| NetHero {
        sub: sub.into(),
        icon: "wifi",
        name: name.into(),
        meta: saved_line.clone(),
        tone: "muted",
        ..NetHero::default()
    };
    let Some(device) = wifi_device(snap) else {
        view.show_wifi_switch = false;
        view.show_rescan = false;
        view.show_search = false;
        view.net_hero = NetHero { icon: "wifi-off", tone: "muted", ..off("No Wi-Fi card", "No Wi-Fi card") };
        view.empty = Some(Empty {
            icon: "wifi-off",
            title: "No Wi-Fi card".into(),
            text: "Lituus didn't find a Wi-Fi device. Wired and VPN connections are on their own pages.".into(),
            page: true,
            ..Empty::default()
        });
        return Vec::new();
    };
    if !snap.wireless_hardware_enabled {
        view.show_search = false;
        view.net_hero = off("Blocked", "Wi-Fi is blocked");
        view.empty = Some(Empty {
            icon: "wifi-off",
            title: "Wi-Fi is blocked".into(),
            text: "A hardware switch or airplane mode turned the radio off.".into(),
            page: true,
            ..Empty::default()
        });
        return Vec::new();
    }
    if !snap.wireless_enabled {
        view.show_search = false;
        let kept = match saved {
            0 => String::new(),
            1 => " Your saved network is kept.".into(),
            n => format!(" Your {n} saved networks are kept."),
        };
        // Turning Wi-Fi on would leave airplane mode on for the rest, so the way back is to end it.
        let (sub, text, action) = if state.airplane {
            (
                "Off · airplane mode",
                format!("Wi-Fi is off because airplane mode is on.{kept}"),
                "Turn off airplane mode",
            )
        } else {
            ("Off", format!("Turn it on to see networks nearby.{kept}"), "Turn on Wi-Fi")
        };
        view.net_hero = off(sub, "Wi-Fi is off");
        view.empty = Some(Empty {
            icon: "wifi",
            title: "Wi-Fi is off".into(),
            text,
            action: action.into(),
            page: true,
            ..Empty::default()
        });
        return Vec::new();
    }
    view.bottom_label = "Connect to hidden network…".into();
    let networks = snap.networks().into_iter().find(|(p, _)| *p == device.path).map(|(_, n)| n).unwrap_or_default();
    // Saved networks that aren't in range: the old Saved page's Wi-Fi part, the hotspot included.
    let out_of_range: Vec<&SavedConnection> =
        saved_wifi(snap).filter(|c| !networks.iter().any(|n| Some(n.ssid.as_slice()) == c.wifi_ssid())).collect();
    view.net_hero = wifi_hero(snap, device, &networks, out_of_range.len());
    view.count = count(networks.len(), "network in range", "networks in range");
    let connecting = device.state.is_connecting();
    let (mut connected, mut available) = (Vec::new(), Vec::new());
    for n in networks.iter().filter(|n| matches_search(&n.name(), &state.search)) {
        let bands = bands_label(&n.bands);
        let failed = state.failed.as_ref().is_some_and(|(id, _)| *id == network_id(n)) && !n.active;
        let (meta, tone) = if failed {
            (format!("Couldn't connect · {}", n.security.label()), "danger")
        } else if n.active && connecting {
            (format!("Connecting… · {}", n.security.label()), "accent")
        } else if n.active {
            (format!("Connected · {} · {bands}", n.security.label()), "accent")
        } else if n.saved.is_some() {
            (format!("Saved · {} · {bands}", n.security.label()), "muted")
        } else {
            (format!("{} · {bands}", n.security.label()), "muted")
        };
        let row = RowData {
            id: network_id(n),
            name: n.name(),
            meta,
            tone,
            icon: "bars",
            bars: bars(n.strength()),
            secure: n.security.needs_secrets(),
            indent: false,
        };
        if n.active { connected.push(row) } else { available.push(row) }
    }
    let now = now();
    let elsewhere = out_of_range
        .into_iter()
        .filter(|c| matches_search(&c.id, &state.search))
        .map(|c| RowData {
            id: format!("conn:{}", c.uuid),
            name: c.id.clone(),
            meta: if c.hotspot { "This computer's hotspot".into() } else { last_used(c.timestamp, now) },
            tone: "faint",
            icon: if c.hotspot { "hotspot" } else { "wifi" },
            ..RowData::default()
        })
        .collect();
    vec![("Connected".into(), connected), ("Available".into(), available), ("Saved, not in range".into(), elsewhere)]
}

fn connection_kind_label(c: &SavedConnection) -> String {
    match &c.kind {
        ConnectionKind::Wifi { .. } => "Wi-Fi".into(),
        ConnectionKind::Ethernet => "Wired".into(),
        ConnectionKind::Bluetooth { .. } => "Bluetooth".into(),
        ConnectionKind::WireGuard => "WireGuard".into(),
        ConnectionKind::Vpn { service, protocol, .. } if service == crate::openconnect::SERVICE => {
            crate::openconnect::protocol_label(protocol)
        }
        ConnectionKind::Vpn { service, .. } => vpn_service_label(service),
        ConnectionKind::Bridge { .. } => "Bridge".into(),
        ConnectionKind::Bond { .. } => "Bond".into(),
        ConnectionKind::Vlan { id, .. } => format!("VLAN {id}"),
        ConnectionKind::Other(t) => t.clone(),
    }
}

/// "org.freedesktop.NetworkManager.openvpn" → "OpenVPN".
fn vpn_service_label(service: &str) -> String {
    match service.rsplit('.').next().unwrap_or(service) {
        "openvpn" => "OpenVPN".into(),
        "openconnect" => "OpenConnect".into(),
        "strongswan" => "strongSwan".into(),
        "l2tp" => "L2TP".into(),
        "pptp" => "PPTP".into(),
        "vpnc" => "Cisco (vpnc)".into(),
        other => other.into(),
    }
}

fn icon_for(c: &SavedConnection) -> &'static str {
    match c.kind {
        ConnectionKind::Wifi { .. } => "wifi",
        ConnectionKind::Ethernet => "wired",
        ConnectionKind::Bluetooth { .. } => "bluetooth",
        ConnectionKind::WireGuard | ConnectionKind::Vpn { .. } => "vpn",
        ConnectionKind::Bridge { .. } => "bridge",
        ConnectionKind::Bond { .. } => "bond",
        ConnectionKind::Vlan { .. } => "vlan",
        _ if c.port.is_some() => "port",
        ConnectionKind::Other(_) => "saved",
    }
}

fn wired_page(snap: &Snapshot, view: &mut View) -> Vec<(String, Vec<RowData>)> {
    view.show_add = true;
    view.show_search = false;
    view.bottom_label = "Add a wired profile…".into();
    view.net_hero = wired_hero(snap);
    let devices = snap.devices.iter().filter(|d| d.kind == DeviceKind::Ethernet).count();
    if devices > 0 {
        view.count = count(devices, "wired device", "wired devices");
    }
    // Each wired device with its profiles first; bridges, bonds and VLANs after them.
    let mut sections = Vec::new();
    let mut shown_vlans = Vec::new();
    for device in snap.devices.iter().filter(|d| d.kind == DeviceKind::Ethernet) {
        let carrier = device.wired.as_ref().is_some_and(|w| w.carrier);
        let active = device.active_connection.as_deref().and_then(|a| snap.active_connection(a));
        // A port of a bridge or bond is shown there.
        if snap
            .connections
            .iter()
            .any(|c| c.port.is_some() && c.interface.as_deref() == Some(device.interface.as_str()))
        {
            continue;
        }
        let active_uuid = active.map(|a| a.uuid.as_str());
        let mut rows: Vec<RowData> = snap
            .connections
            .iter()
            .filter(|c| {
                c.kind == ConnectionKind::Ethernet
                    && c.port.is_none()
                    && c.interface.as_deref().is_none_or(|i| i == device.interface)
            })
            .map(|c| {
                let active = active_uuid == Some(c.uuid.as_str());
                let (meta, tone) = if active {
                    (format!("Connected{}", speed(device)), "accent")
                } else if !carrier && c.autoconnect {
                    ("Saved · connects when a cable is plugged in".into(), "faint")
                } else if !carrier {
                    ("Saved · cable unplugged".into(), "faint")
                } else {
                    ("Saved".into(), "muted")
                };
                RowData {
                    id: format!("conn:{}", c.uuid),
                    name: c.id.clone(),
                    meta,
                    tone,
                    icon: "wired",
                    ..RowData::default()
                }
            })
            .collect();
        if rows.is_empty() {
            rows.push(RowData {
                id: format!("dev:{}", device.path),
                name: if carrier { "Wired connection".into() } else { "Cable unplugged".into() },
                meta: if carrier { "No saved connection".into() } else { "Plug in a cable to connect".into() },
                tone: "faint",
                icon: "wired",
                ..RowData::default()
            });
        }
        for v in vlans_on(snap, &device.interface) {
            shown_vlans.push(v.uuid.clone());
            rows.push(RowData { indent: true, ..virtual_row(snap, v, 0) });
        }
        sections.push((device_title(device), rows));
    }
    // Then bridges and bonds, each with its ports under it.
    let mut virtual_rows = Vec::new();
    for c in snap
        .connections
        .iter()
        .filter(|c| matches!(c.kind, ConnectionKind::Bridge { .. } | ConnectionKind::Bond { .. }))
    {
        let ports = ports_of(snap, c);
        virtual_rows.push(virtual_row(snap, c, ports.len()));
        for p in ports {
            let interface = p.interface.clone().unwrap_or_else(|| p.id.clone());
            let device = snap.devices.iter().find(|d| d.interface == interface);
            let carrier = device.and_then(|d| d.wired.as_ref()).map(|w| w.carrier);
            let up = active_uuid_state(snap, &p.uuid).is_some_and(|a| a.state == ActiveState::Activated);
            let (meta, tone) = match (device, carrier) {
                (None, _) => (format!("Port of {} · not plugged in", controller_name(c)), "faint"),
                (_, Some(false)) => (format!("Port of {} · cable unplugged", controller_name(c)), "faint"),
                (Some(d), _) => {
                    (format!("Port of {}{}", controller_name(c), speed(d)), if up { "muted" } else { "faint" })
                }
            };
            virtual_rows.push(RowData {
                id: format!("conn:{}", p.uuid),
                name: interface,
                meta,
                tone,
                icon: "port",
                indent: true,
                ..RowData::default()
            });
        }
    }
    if !virtual_rows.is_empty() {
        sections.push(("Bridges and bonds".to_owned(), virtual_rows));
    }
    // VLANs on a bond or bridge, or on a device that isn't there.
    let others: Vec<RowData> = snap
        .connections
        .iter()
        .filter(|c| matches!(c.kind, ConnectionKind::Vlan { .. }) && !shown_vlans.contains(&c.uuid))
        .map(|c| virtual_row(snap, c, 0))
        .collect();
    if !others.is_empty() {
        sections.push(("VLANs".to_owned(), others));
    }
    // What the old Saved page listed and no section above shows: profiles for a device that
    // isn't there, and connections of other kinds (a phone's internet over Bluetooth, say).
    let shown: Vec<String> = sections.iter().flat_map(|(_, rows)| rows.iter().map(|r| r.id.clone())).collect();
    let (mut unplugged, mut other) = (Vec::new(), Vec::new());
    for c in snap.connections.iter().filter(|c| !shown.contains(&format!("conn:{}", c.uuid))) {
        let row = |meta: String| RowData {
            id: format!("conn:{}", c.uuid),
            name: c.id.clone(),
            meta,
            tone: "faint",
            icon: icon_for(c),
            ..RowData::default()
        };
        match &c.kind {
            ConnectionKind::Ethernet | ConnectionKind::Bridge { .. } | ConnectionKind::Bond { .. } => {
                unplugged.push(row(match &c.interface {
                    Some(i) => format!("Saved · only on {i}"),
                    None => format!("Saved · {}", connection_kind_label(c)),
                }));
            }
            ConnectionKind::Bluetooth { .. } | ConnectionKind::Other(_) => {
                let active = active_uuid_state(snap, &c.uuid).is_some_and(|a| a.state == ActiveState::Activated);
                let mut r = row(if active {
                    format!("{} · connected", connection_kind_label(c))
                } else {
                    connection_kind_label(c)
                });
                r.tone = if active { "accent" } else { "muted" };
                other.push(r);
            }
            _ => {}
        }
    }
    for (title, rows) in [("Saved, not plugged in", unplugged), ("Other connections", other)] {
        if !rows.is_empty() {
            sections.push((title.to_owned(), rows));
        }
    }
    sections
}

/// The ports of a bridge or bond: connections whose controller is its UUID or interface.
fn ports_of<'a>(snap: &'a Snapshot, c: &SavedConnection) -> Vec<&'a SavedConnection> {
    snap.connections
        .iter()
        .filter(|p| {
            p.port.as_ref().is_some_and(|port| {
                port.controller == c.uuid || c.interface.as_deref().is_some_and(|i| port.controller == i)
            })
        })
        .collect()
}

/// The VLANs on an interface (by its name, or by the UUID of a connection on it).
fn vlans_on<'a>(snap: &'a Snapshot, interface: &str) -> Vec<&'a SavedConnection> {
    snap.connections
        .iter()
        .filter(|c| match &c.kind {
            ConnectionKind::Vlan { parent, .. } => {
                parent == interface
                    || snap.connections.iter().any(|p| p.uuid == *parent && p.interface.as_deref() == Some(interface))
            }
            _ => false,
        })
        .collect()
}

fn controller_name(c: &SavedConnection) -> String {
    c.interface.clone().unwrap_or_else(|| c.id.clone())
}

/// A bridge, bond or VLAN row: "Connected · bridge · 2 ports · 192.168.1.57/24".
fn virtual_row(snap: &Snapshot, c: &SavedConnection, ports: usize) -> RowData {
    let active = active_uuid_state(snap, &c.uuid);
    let device = active.and_then(|a| a.devices.first()).and_then(|p| snap.device(p));
    let what = match &c.kind {
        ConnectionKind::Bridge { .. } => "bridge".to_owned(),
        ConnectionKind::Bond { mode } => mode.clone(),
        ConnectionKind::Vlan { id, .. } => format!("VLAN {id}"),
        _ => connection_kind_label(c),
    };
    let mut parts = Vec::new();
    let tone = match active.map(|a| a.state) {
        Some(ActiveState::Activated) => {
            parts.push("Connected".to_owned());
            "accent"
        }
        Some(ActiveState::Activating) => {
            parts.push("Connecting…".to_owned());
            "accent"
        }
        _ => {
            parts.push("Saved".to_owned());
            "muted"
        }
    };
    parts.push(what);
    if matches!(c.kind, ConnectionKind::Bridge { .. } | ConnectionKind::Bond { .. }) {
        parts.push(match ports {
            0 => "no ports".to_owned(),
            1 => "1 port".to_owned(),
            n => format!("{n} ports"),
        });
    }
    if let Some((address, prefix)) = device.and_then(|d| d.ip4.as_ref()).and_then(|ip| ip.addresses.first()) {
        parts.push(format!("{address}/{prefix}"));
    }
    RowData {
        id: format!("conn:{}", c.uuid),
        name: c.id.clone(),
        meta: parts.join(" · "),
        tone,
        icon: icon_for(c),
        ..RowData::default()
    }
}

fn speed(device: &Device) -> String {
    match device.wired.as_ref().map(|w| w.speed) {
        Some(speed) if speed > 0 => format!(" · {speed} Mb/s"),
        _ => String::new(),
    }
}

/// "enp3s0 · e1000e": the interface, and the driver when it says something.
fn device_title(device: &Device) -> String {
    if device.driver.is_empty() || device.driver == "dbusmock" {
        device.interface.clone()
    } else {
        format!("{} · {}", device.interface, device.driver)
    }
}

fn vpn_page(snap: &Snapshot, state: &UiState, view: &mut View) -> Vec<(String, Vec<RowData>)> {
    view.show_add = true;
    view.search_placeholder = "Search VPNs".into();
    view.bottom_label = "Import a VPN connection…".into();
    view.net_hero = vpn_hero(snap);
    let vpns: Vec<&SavedConnection> = snap
        .connections
        .iter()
        .filter(|c| matches!(c.kind, ConnectionKind::WireGuard | ConnectionKind::Vpn { .. }))
        .collect();
    view.show_search = !vpns.is_empty();
    if !vpns.is_empty() {
        view.count = count(vpns.len(), "VPN", "VPNs");
    }
    let (mut wireguard, mut others) = (Vec::new(), Vec::new());
    for c in vpns.into_iter().filter(|c| matches_search(&c.id, &state.search)) {
        let active = active_uuid_state(snap, &c.uuid);
        let id = format!("conn:{}", c.uuid);
        let (meta, tone) = match active.map(|a| a.state) {
            _ if state.signing_in.contains(&c.uuid) => ("Signing in…".to_owned(), "accent"),
            Some(ActiveState::Activated) => (format!("Connected · {}", connection_kind_label(c)), "accent"),
            Some(ActiveState::Activating) => ("Connecting…".to_owned(), "accent"),
            _ if state.failed.as_ref().is_some_and(|(f, why)| *f == id && why == crate::openconnect::CLOSED) => {
                ("Couldn't sign in".to_owned(), "danger")
            }
            _ if state.failed.as_ref().is_some_and(|(f, _)| *f == id) => ("Couldn't connect".to_owned(), "danger"),
            _ => (format!("Off · {}", connection_kind_label(c)), "muted"),
        };
        let row = RowData { id, name: c.id.clone(), meta, tone, icon: "vpn", bars: 0, secure: false, indent: false };
        if c.kind == ConnectionKind::WireGuard { wireguard.push(row) } else { others.push(row) }
    }
    vec![("WireGuard".into(), wireguard), ("VPN".into(), others)]
}

fn details(snap: &Snapshot, state: &UiState, id: &str) -> Option<DetailsData> {
    if let Some(rest) = id.strip_prefix("wifi:") {
        return wifi_details(snap, state, rest);
    }
    if let Some(uuid) = id.strip_prefix("conn:") {
        return connection_details(snap, state, uuid);
    }
    if let Some(path) = id.strip_prefix("dev:") {
        let device = snap.device(path)?;
        let carrier = device.wired.as_ref().is_some_and(|w| w.carrier);
        return Some(DetailsData {
            title: device.interface.clone(),
            status: if carrier { "Not connected".into() } else { "Cable unplugged".into() },
            status_tone: "faint",
            status_extra: String::new(),
            actions: vec![action("add-wired", "Add a connection", "secondary")],
            toggles: Vec::new(),
            metered: None,
            items: vec![
                item("Interface", device.interface.as_str(), true),
                item("MAC address", device.hw_address.as_str(), true),
            ],
            note: String::new(),
        });
    }
    None
}

fn wifi_details(snap: &Snapshot, state: &UiState, id: &str) -> Option<DetailsData> {
    let device = wifi_device(snap)?;
    let networks = snap.networks().into_iter().find(|(p, _)| *p == device.path)?.1;
    let n = networks.into_iter().find(|n| network_id(n) == format!("wifi:{id}"))?;
    let ap = n.strongest().clone();
    let saved = n.saved.as_deref().and_then(|uuid| snap.connections.iter().find(|c| c.uuid == uuid));
    let connecting = n.active && device.state.is_connecting();
    let band_line = |mhz: u32| {
        let band = Band::from_mhz(mhz).map(|b| b.label()).unwrap_or("?");
        match channel(mhz) {
            Some(c) => format!("{band} · channel {c}"),
            None => band.to_owned(),
        }
    };
    let failed = !n.active && state.failed.as_ref().is_some_and(|(f, _)| *f == network_id(&n));
    let (status, tone, extra) = if failed {
        ("Not connected".to_owned(), "danger", "· last attempt failed".to_owned())
    } else if connecting {
        ("Connecting…".to_owned(), "accent", String::new())
    } else if n.active {
        let wifi = device.wifi.as_ref();
        let bitrate =
            wifi.map(|w| w.bitrate / 1000).filter(|&b| b > 0).map(|b| format!(" · {b} Mb/s")).unwrap_or_default();
        let band = Band::from_mhz(ap.frequency).map(|b| b.label()).unwrap_or("");
        ("Connected".to_owned(), "accent", format!("· {band}{bitrate}"))
    } else if saved.is_some() {
        ("Saved".to_owned(), "muted", String::new())
    } else {
        ("Not connected".to_owned(), "faint", String::new())
    };
    let mut actions = Vec::new();
    if n.active {
        actions.push(action("disconnect", "Disconnect", "secondary"));
    } else {
        actions.push(action("connect", "Connect", "primary"));
    }
    if saved.is_some() {
        actions.push(action("edit", "Edit", "secondary"));
        if n.active {
            actions.push(action("share", "Share", "secondary"));
        }
        actions.push(action("forget", "Forget", "danger"));
    }
    let toggles = match saved {
        Some(c) => vec![toggle("autoconnect", "Connect automatically", "", c.autoconnect)],
        None => Vec::new(),
    };
    let mut items = vec![
        item("Signal", format!("{} %", ap.strength), false),
        item("Security", security_long(n.security), false),
        item("Band", if n.active { band_line(ap.frequency) } else { bands_label(&n.bands) }, false),
    ];
    if n.active {
        items.extend(ip_items(device));
        items.extend(traffic_item(device));
        items.push(item("BSSID", ap.bssid.as_str(), true));
        items.push(item("Interface", device.interface.as_str(), true));
        if !device.hw_address.is_empty() {
            items.push(item("MAC address", device.hw_address.as_str(), true));
        }
    } else {
        let count = n.access_points.len();
        items.push(item("Access points", if count == 1 { "1".to_owned() } else { count.to_string() }, false));
        items.push(item(if count == 1 { "BSSID" } else { "Strongest BSSID" }, ap.bssid.as_str(), true));
    }
    Some(DetailsData {
        title: n.name(),
        status,
        status_tone: tone,
        status_extra: extra,
        actions,
        toggles,
        metered: saved.map(metered_choice),
        items,
        note: String::new(),
    })
}

fn connection_details(snap: &Snapshot, state: &UiState, uuid: &str) -> Option<DetailsData> {
    let page = state.page;
    let c = snap.connections.iter().find(|c| c.uuid == uuid)?;
    let active = active_uuid_state(snap, uuid);
    let device = active.and_then(|a| a.devices.first()).and_then(|p| snap.device(p));
    let signing_in = state.signing_in.iter().any(|u| u == uuid);
    let failed = state.failed.as_ref().filter(|(f, _)| *f == format!("conn:{uuid}")).map(|(_, why)| why.as_str());
    let (status, tone) = match active.map(|a| a.state) {
        _ if signing_in => ("Signing in · finish in the sign-in window", "accent"),
        Some(ActiveState::Activated) => ("Connected", "accent"),
        Some(ActiveState::Activating) => ("Connecting…", "accent"),
        Some(ActiveState::Deactivating) => ("Disconnecting…", "muted"),
        _ if failed == Some(crate::openconnect::CLOSED) => {
            ("Couldn't sign in: the sign-in window was closed", "danger")
        }
        _ if failed.is_some() => ("Couldn't connect", "danger"),
        _ => ("Not connected", "faint"),
    };
    let connected = active.is_some_and(|a| a.state == ActiveState::Activated);
    let extra = match device {
        Some(d) if connected && c.kind == ConnectionKind::Ethernet => speed(d).trim_start_matches(" · ").to_owned(),
        _ => String::new(),
    };
    let mut actions = vec![if signing_in {
        action("cancel-sign-in", "Cancel", "secondary")
    } else if connected {
        action("disconnect", "Disconnect", "secondary")
    } else if failed.is_some() {
        action("connect", "Try again", "primary")
    } else {
        action("connect", "Connect", "primary")
    }];
    actions.push(action("edit", "Edit", "secondary"));
    if !signing_in {
        actions.push(action("forget", "Forget", "danger"));
    }
    let toggles = vec![toggle("autoconnect", "Connect automatically", "", c.autoconnect)];
    let metered = (page != Page::Vpn).then(|| metered_choice(c));
    let mut items = Vec::new();
    if let ConnectionKind::Vpn { gateway, .. } = &c.kind
        && !gateway.is_empty()
    {
        items.push(item("Gateway", gateway.as_str(), true));
    }
    if let (Some(d), true) = (device, connected) {
        items.extend(ip_items(d));
        items.extend(traffic_item(d));
        items.push(item("Interface", d.interface.as_str(), true));
        if !d.hw_address.is_empty() {
            items.push(item("MAC address", d.hw_address.as_str(), true));
        }
    }
    match &c.kind {
        ConnectionKind::Bridge { stp } => {
            let ports: Vec<String> = ports_of(snap, c).iter().map(|p| controller_name(p)).collect();
            items.push(item("Ports", if ports.is_empty() { "None".to_owned() } else { ports.join(", ") }, true));
            items.push(item("Spanning tree", if *stp { "On" } else { "Off" }, false));
        }
        ConnectionKind::Bond { mode } => {
            let ports: Vec<String> = ports_of(snap, c).iter().map(|p| controller_name(p)).collect();
            items.push(item("Ports", if ports.is_empty() { "None".to_owned() } else { ports.join(", ") }, true));
            let label = crate::virt::BOND_MODES.iter().find(|(m, _)| m == mode).map_or(mode.as_str(), |(_, l)| *l);
            items.push(item("Mode", label, false));
        }
        ConnectionKind::Vlan { parent, id } => {
            let parent = snap.connections.iter().find(|p| p.uuid == *parent).map_or(parent.clone(), controller_name);
            items.push(item("VLAN ID", id.to_string(), true));
            items.push(item("Runs on", parent, true));
        }
        _ => {}
    }
    if let Some(port) = &c.port {
        let controller = snap
            .connections
            .iter()
            .find(|x| x.uuid == port.controller)
            .map_or(port.controller.clone(), controller_name);
        items.push(item("Port of", controller, true));
    }
    items.push(item("Type", connection_kind_label(c), false));
    items.push(item("Priority", c.autoconnect_priority.to_string(), true));
    if let Some(i) = &c.interface {
        items.push(item("Only on", i.as_str(), true));
    }
    items.push(item("UUID", c.uuid.as_str(), true));
    let note = if page == Page::Vpn {
        "WireGuard is built into NetworkManager. Other VPN types appear once their NetworkManager plugin is installed."
            .into()
    } else {
        String::new()
    };
    Some(DetailsData {
        title: c.id.clone(),
        status: status.into(),
        status_tone: tone,
        status_extra: if extra.is_empty() { String::new() } else { format!("· {extra}") },
        actions,
        toggles,
        metered,
        items,
        note,
    })
}

/// A change asked of NetworkManager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    SetWifi(bool),
    Rescan {
        device: String,
    },
    Activate {
        connection: String,
        device: Option<String>,
        access_point: Option<String>,
    },
    ConnectOpen {
        device: String,
        access_point: String,
        ssid: Vec<u8>,
    },
    /// A new network with a password (or a hidden one, with no access point).
    Join {
        device: String,
        access_point: Option<String>,
        join: WifiJoin,
    },
    /// Airplane mode on or off: Wi-Fi and mobile broadband switched to these (Bluetooth is the
    /// Bluetooth thread's; see `airplane.rs`).
    SetAirplane {
        on: bool,
        wifi: bool,
        wwan: bool,
    },
    /// Read a connection's settings (or none, for a new one) and open the editor.
    OpenEditor {
        connection: Option<String>,
        new: Option<crate::editor::Kind>,
    },
    /// Open the editor on a new connection to an Enterprise (802.1X) network.
    NewEnterprise {
        ssid: Vec<u8>,
        /// WPA3 Enterprise 192-bit.
        suite_b: bool,
    },
    /// Save the editor's settings: update (or add, without a path), keyring secret first, then
    /// reapply to the active connection.
    SaveConnection {
        path: Option<String>,
        settings: crate::editing::Shared,
        secret: Option<crate::editing::KeyringSecret>,
        reapply: Option<(String, Option<String>)>,
        /// A bridge's or bond's ports to add or let go.
        ports: Option<crate::virt::PortPlan>,
    },
    /// One key of a saved connection, from the details pane's switches.
    SetSetting {
        connection: String,
        setting: &'static str,
        key: &'static str,
        value: SettingValue,
    },
    /// The computer's name and network name (systemd-hostnamed; without it, NetworkManager sets
    /// the network name).
    SaveNames {
        name: String,
        network: String,
    },
    /// The file dialog, for one of the editor's path fields.
    ChooseFile {
        field: String,
    },
    /// Pick a WireGuard .conf or OpenVPN .ovpn file and open it in the editor as a new connection.
    ImportFile,
    /// Save a WireGuard connection as a .conf file.
    ExportWireGuard {
        connection: String,
    },
    /// Show a Wi-Fi network's QR code, with its password.
    ShareWifi {
        connection: String,
        name: String,
    },
    /// Read the hotspot's name, password and band for its page.
    LoadHotspot {
        connection: String,
    },
    /// Turn the hotspot on with these settings (making its connection, the first time).
    StartHotspot {
        connection: Option<String>,
        device: String,
        ssid: String,
        password: String,
        band: usize,
    },
    StopHotspot {
        active: String,
    },
    Deactivate {
        active: String,
    },
    /// Close an OpenConnect VPN's sign-in window, by its UUID.
    CancelSignIn {
        uuid: String,
    },
    Forget {
        connection: String,
    },
    /// A phone's internet over Bluetooth: a new connection on NetworkManager's device for it.
    Tether {
        device: String,
        address: String,
        name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingValue {
    Bool(bool),
    I32(i32),
}

/// What pressing a button means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    Run(Command),
    /// Ask first ("Forget Office-Guest?"), then run it.
    Confirm {
        title: String,
        text: String,
        button: String,
        command: Command,
    },
    /// Ask for the password (and where to keep it) first.
    Join(JoinTarget),
}

/// A secured network to join, for the password dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinTarget {
    pub device: String,
    pub access_point: String,
    pub ssid: Vec<u8>,
    pub name: String,
    pub security: Security,
    pub key_mgmt: KeyMgmt,
    pub strength: u8,
}

/// How a network checks the password, for the kinds the password dialog handles.
pub fn key_mgmt(security: Security) -> Option<KeyMgmt> {
    match security {
        Security::Wpa | Security::Wpa2 | Security::Wpa2Wpa3 => Some(KeyMgmt::WpaPsk),
        Security::Wpa3 => Some(KeyMgmt::Sae),
        Security::Wep => Some(KeyMgmt::Wep),
        Security::Open | Security::Owe | Security::Enterprise | Security::Wpa3Enterprise => None,
    }
}

/// The command behind a details button (`action` is its id) for the selected row.
pub fn intent(snap: &Snapshot, selected: &str, action: &str) -> Option<Intent> {
    match action {
        "edit" => {
            let path = saved_connection(snap, selected)?.path.clone();
            return Some(Intent::Run(Command::OpenEditor { connection: Some(path), new: None }));
        }
        "share" => {
            let c = saved_connection(snap, selected)?;
            return Some(Intent::Run(Command::ShareWifi { connection: c.path.clone(), name: c.id.clone() }));
        }
        "add-wired" => {
            return Some(Intent::Run(Command::OpenEditor {
                connection: None,
                new: Some(crate::editor::Kind::Ethernet),
            }));
        }
        _ => {}
    }
    if let Some(id) = selected.strip_prefix("wifi:") {
        let device = wifi_device(snap)?;
        let network = snap
            .networks()
            .into_iter()
            .find(|(p, _)| *p == device.path)?
            .1
            .into_iter()
            .find(|n| network_id(n) == format!("wifi:{id}"))?;
        let saved = network.saved.as_deref().and_then(|uuid| snap.connections.iter().find(|c| c.uuid == uuid));
        return match action {
            "connect" => Some(match saved {
                Some(c) => Intent::Run(Command::Activate {
                    connection: c.path.clone(),
                    device: Some(device.path.clone()),
                    access_point: Some(network.strongest().path.clone()),
                }),
                None if !network.security.needs_secrets() => Intent::Run(Command::ConnectOpen {
                    device: device.path.clone(),
                    access_point: network.strongest().path.clone(),
                    ssid: network.ssid.clone(),
                }),
                None => match key_mgmt(network.security) {
                    Some(key_mgmt) => Intent::Join(JoinTarget {
                        device: device.path.clone(),
                        access_point: network.strongest().path.clone(),
                        ssid: network.ssid.clone(),
                        name: network.name(),
                        security: network.security,
                        key_mgmt,
                        strength: network.strongest().strength,
                    }),
                    // An Enterprise network needs more than a password: the editor, filled in.
                    None => Intent::Run(Command::NewEnterprise {
                        ssid: network.ssid.clone(),
                        suite_b: network.security == Security::Wpa3Enterprise,
                    }),
                },
            }),
            "disconnect" => Some(Intent::Run(Command::Deactivate { active: device.active_connection.clone()? })),
            "forget" => saved.map(forget),
            _ => None,
        };
    }
    if let Some(uuid) = selected.strip_prefix("conn:") {
        let c = snap.connections.iter().find(|c| c.uuid == uuid)?;
        let active = active_uuid_state(snap, uuid);
        return match action {
            "connect" => {
                // A wired profile goes on its own (or the first) Ethernet port; NetworkManager
                // picks the device for VPNs.
                let device = match c.kind {
                    ConnectionKind::Ethernet => snap
                        .devices
                        .iter()
                        .filter(|d| d.kind == DeviceKind::Ethernet)
                        .find(|d| c.interface.as_deref().is_none_or(|i| i == d.interface))
                        .map(|d| d.path.clone()),
                    _ => None,
                };
                Some(Intent::Run(Command::Activate { connection: c.path.clone(), device, access_point: None }))
            }
            "disconnect" => Some(Intent::Run(Command::Deactivate { active: active?.path.clone() })),
            "cancel-sign-in" => Some(Intent::Run(Command::CancelSignIn { uuid: c.uuid.clone() })),
            "forget" => Some(forget(c)),
            _ => None,
        };
    }
    None
}

fn forget(c: &SavedConnection) -> Intent {
    let password = matches!(c.kind, ConnectionKind::Wifi { .. });
    Intent::Confirm {
        title: format!("Forget {}?", c.id),
        text: if password {
            "This deletes the saved password and settings. You'll need the password to connect again.".into()
        } else {
            "This deletes the connection and its settings.".into()
        },
        button: "Forget".into(),
        command: Command::Forget { connection: c.path.clone() },
    }
}

/// The Wi-Fi switch, or the "Turn on Wi-Fi" button.
pub fn wifi_switch(snap: &Snapshot) -> Command {
    Command::SetWifi(!snap.wireless_enabled)
}

/// The saved connection behind a row, if it has one.
pub fn saved_connection<'a>(snap: &'a Snapshot, id: &str) -> Option<&'a SavedConnection> {
    if id.starts_with("wifi:") {
        let device = wifi_device(snap)?;
        let networks = snap.networks().into_iter().find(|(p, _)| *p == device.path)?.1;
        let uuid = networks.into_iter().find(|n| network_id(n) == id)?.saved?;
        return snap.connections.iter().find(|c| c.uuid == uuid);
    }
    let uuid = id.strip_prefix("conn:")?;
    snap.connections.iter().find(|c| c.uuid == uuid)
}

/// The switches in the details pane ("autoconnect") as a command.
pub fn toggle_command(snap: &Snapshot, selected: &str, id: &str) -> Option<Command> {
    let c = saved_connection(snap, selected)?;
    let (key, value) = match id {
        "autoconnect" => ("autoconnect", SettingValue::Bool(!c.autoconnect)),
        _ => return None,
    };
    Some(Command::SetSetting { connection: c.path.clone(), setting: "connection", key, value })
}

/// The details pane's Metered choice as a command: 0 Automatic, 1 Yes, 2 No, which are
/// NetworkManager's own numbers (NMMetered).
pub fn metered_command(snap: &Snapshot, selected: &str, choice: i32) -> Option<Command> {
    let c = saved_connection(snap, selected)?;
    let value = SettingValue::I32(choice.clamp(0, 2));
    Some(Command::SetSetting { connection: c.path.clone(), setting: "connection", key: "metered", value })
}

/// The name a row shows, for messages about it.
pub fn row_name(snap: &Snapshot, id: &str) -> Option<String> {
    if id.starts_with("wifi:") {
        let device = wifi_device(snap)?;
        let networks = snap.networks().into_iter().find(|(p, _)| *p == device.path)?.1;
        return networks.into_iter().find(|n| network_id(n) == id).map(|n| n.name());
    }
    let uuid = id.strip_prefix("conn:")?;
    snap.connections.iter().find(|c| c.uuid == uuid).map(|c| c.id.clone())
}

/// The Wi-Fi device a hidden network is joined on.
pub fn hidden_network_device(snap: &Snapshot) -> Option<String> {
    wifi_device(snap).map(|d| d.path.clone())
}

/// Rescanning on the Wi-Fi device, if there is one.
pub fn rescan(snap: &Snapshot) -> Option<Command> {
    wifi_device(snap).map(|d| Command::Rescan { device: d.path.clone() })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lituus_nm::{AccessPoint, ActiveConnection, DeviceState, IpConfig, WifiDevice, WiredDevice};
    use slint::Model;

    fn ap(path: &str, ssid: &str, mhz: u32, strength: u8, security: Security) -> AccessPoint {
        AccessPoint {
            path: path.into(),
            ssid: ssid.as_bytes().to_vec(),
            bssid: format!("BB:{path}"),
            frequency: mhz,
            strength,
            security,
            max_bitrate: 0,
        }
    }

    /// Connected to "Lumen Home", two more networks in range, an unplugged Ethernet port.
    pub(crate) fn home() -> Snapshot {
        let wifi = Device {
            path: "/d/wlan".into(),
            interface: "wlp2s0".into(),
            kind: DeviceKind::Wifi,
            state: DeviceState::Activated,
            driver: "iwlwifi".into(),
            hw_address: "9C:B6:D0:4A:12:7E".into(),
            managed: true,
            active_connection: Some("/a/1".into()),
            ip4: Some(IpConfig {
                addresses: vec![("192.168.1.42".into(), 24)],
                gateway: Some("192.168.1.1".into()),
                dns: vec!["9.9.9.9".into()],
            }),
            ip6: Some(IpConfig {
                addresses: vec![("fe80::1".into(), 64), ("fd12:3a::42".into(), 64)],
                gateway: None,
                dns: vec![],
            }),
            wifi: Some(WifiDevice {
                access_points: vec![
                    ap("/ap/1", "Lumen Home", 5180, 78, Security::Wpa2Wpa3),
                    ap("/ap/2", "Aster Café", 2412, 61, Security::Wpa2),
                    ap("/ap/3", "Bibliothek-Free", 5200, 40, Security::Open),
                ],
                active_access_point: Some("/ap/1".into()),
                bitrate: 866_000,
                last_scan: None,
            }),
            wired: None,
            traffic: None,
        };
        let eth = Device {
            path: "/d/eth".into(),
            interface: "enp3s0".into(),
            kind: DeviceKind::Ethernet,
            state: DeviceState::Unavailable,
            driver: "e1000e".into(),
            hw_address: "54:E1:AD:7C:20:9B".into(),
            managed: true,
            active_connection: None,
            ip4: None,
            ip6: None,
            wifi: None,
            wired: Some(WiredDevice { carrier: false, speed: 0 }),
            traffic: Some((19_293_798, 2_202_009)),
        };
        Snapshot {
            version: "1.48".into(),
            networking_enabled: true,
            wireless_enabled: true,
            wireless_hardware_enabled: true,
            wwan_enabled: true,
            devices: vec![wifi, eth],
            connections: vec![SavedConnection {
                path: "/c/1".into(),
                uuid: "u-home".into(),
                id: "Lumen Home".into(),
                kind: ConnectionKind::Wifi { ssid: b"Lumen Home".to_vec() },
                autoconnect: true,
                autoconnect_priority: 0,
                interface: None,
                metered: None,
                hotspot: false,
                port: None,
                timestamp: 0,
            }],
            active: vec![ActiveConnection {
                path: "/a/1".into(),
                id: "Lumen Home".into(),
                uuid: "u-home".into(),
                state: ActiveState::Activated,
                devices: vec!["/d/wlan".into()],
                connection: Some("/c/1".into()),
                specific_object: Some("/ap/1".into()),
                vpn: false,
            }],
            primary_connection: Some("/a/1".into()),
            hostname: "thinkpad-x1".into(),
        }
    }

    fn rows(view: &View) -> Vec<(String, Vec<String>)> {
        view.sections
            .iter()
            .map(|s| (s.title.to_string(), s.rows.iter().map(|r| r.name.to_string()).collect()))
            .collect()
    }

    fn detail(view: &View, label: &str) -> Option<String> {
        view.details.as_ref()?.items.iter().find(|i| i.label == label).map(|i| i.value.to_string())
    }

    #[test]
    fn wifi_page_lists_connected_then_available() {
        let view = build(Some(&home()), None, &UiState::default());
        assert_eq!(
            rows(&view),
            [
                ("CONNECTED".into(), vec!["Lumen Home".into()]),
                ("AVAILABLE".into(), vec!["Aster Café".into(), "Bibliothek-Free".into()])
            ]
        );
        // The connected network is selected and its details shown.
        let d = view.details.as_ref().unwrap();
        assert_eq!(d.title, "Lumen Home");
        assert_eq!(d.status, "Connected");
        assert_eq!(d.status_extra, "· 5 GHz · 866 Mb/s");
        assert_eq!(detail(&view, "Security").as_deref(), Some("WPA2/WPA3 Personal"));
        assert_eq!(detail(&view, "Band").as_deref(), Some("5 GHz · channel 36"));
        assert_eq!(detail(&view, "IPv4 address").as_deref(), Some("192.168.1.42/24"));
        assert_eq!(detail(&view, "IPv6 address").as_deref(), Some("fd12:3a::42/64"));
        let actions: Vec<String> = d.actions.iter().map(|a| a.label.to_string()).collect();
        assert_eq!(actions, ["Disconnect", "Edit", "Share", "Forget"]);
        assert_eq!(view.net_status, "Lumen Home");
        assert_eq!(
            view.net_items,
            NetStatus {
                wifi: "Lumen Home".into(),
                wired: "unplugged".into(),
                vpn: "off".into(),
                hotspot: "off".into()
            }
        );
        assert_eq!(view.count, "3 networks in range");
        assert_eq!(view.net_hero.sub, "Lumen Home · 2 other networks in range");
        assert_eq!(view.net_hero.name, "Lumen Home");
        assert_eq!(view.net_hero.meta, "Connected · 5 GHz · 866 Mb/s");
        assert_eq!(view.net_hero.detail, "wlp2s0   192.168.1.42");
        // The hero has no buttons: Share, Edit and Disconnect are the pane's, once each.
        let d = view.details.as_ref().unwrap();
        let actions: Vec<String> = d.actions.iter().map(|a| a.label.to_string()).collect();
        assert_eq!(actions, ["Disconnect", "Edit", "Share", "Forget"]);
        assert_eq!(view.hostname, "thinkpad-x1");
    }

    #[test]
    fn selecting_an_unsaved_network_offers_connect() {
        let mut state = UiState::default();
        let view = build(Some(&home()), None, &state);
        state.selected = Some(view.row_ids[1].clone());
        let view = build(Some(&home()), None, &state);
        let d = view.details.unwrap();
        assert_eq!(d.title, "Aster Café");
        assert_eq!(d.status, "Not connected");
        let actions: Vec<String> = d.actions.iter().map(|a| a.label.to_string()).collect();
        assert_eq!(actions, ["Connect"]);
        assert_eq!(d.toggles.row_count(), 0);
    }

    pub(crate) fn saved(
        uuid: &str,
        id: &str,
        kind: ConnectionKind,
        interface: &str,
        port: Option<&str>,
    ) -> SavedConnection {
        SavedConnection {
            path: format!("/c/{uuid}"),
            uuid: uuid.into(),
            id: id.into(),
            kind,
            autoconnect: true,
            autoconnect_priority: 0,
            interface: Some(interface.into()),
            metered: None,
            hotspot: false,
            port: port.map(|c| lituus_nm::Port { controller: c.into(), kind: "bridge".into() }),
            timestamp: 0,
        }
    }

    #[test]
    fn an_anyconnect_vpn_signs_in_and_says_why_it_stopped() {
        let mut snap = home();
        let kind = ConnectionKind::Vpn {
            service: crate::openconnect::SERVICE.into(),
            gateway: "vpn.uzh.ch/staff".into(),
            protocol: "anyconnect".into(),
        };
        snap.connections.push(saved("u-uzh", "UZH VPN", kind, "", None));
        let mut state = UiState { page: Page::Vpn, selected: Some("conn:u-uzh".into()), ..UiState::default() };
        let meta = |view: &View| {
            view.sections.iter().find(|s| s.title == "VPN").unwrap().rows.row_data(0).unwrap().meta.to_string()
        };
        let actions = |view: &View| -> Vec<String> {
            view.details.as_ref().unwrap().actions.iter().map(|a| a.label.to_string()).collect()
        };
        let view = build(Some(&snap), None, &state);
        assert_eq!(meta(&view), "Off · Cisco AnyConnect");
        assert_eq!(detail(&view, "Gateway").as_deref(), Some("vpn.uzh.ch/staff"));
        assert_eq!(detail(&view, "Type").as_deref(), Some("Cisco AnyConnect"));
        assert_eq!(actions(&view), ["Connect", "Edit", "Forget"]);

        state.signing_in = vec!["u-uzh".into()];
        let view = build(Some(&snap), None, &state);
        assert_eq!(meta(&view), "Signing in…");
        assert_eq!(view.details.as_ref().unwrap().status, "Signing in · finish in the sign-in window");
        assert_eq!(actions(&view), ["Cancel", "Edit"]);
        assert_eq!(
            intent(&snap, "conn:u-uzh", "cancel-sign-in"),
            Some(Intent::Run(Command::CancelSignIn { uuid: "u-uzh".into() }))
        );

        state.signing_in.clear();
        state.failed = Some(("conn:u-uzh".into(), crate::openconnect::CLOSED.into()));
        let view = build(Some(&snap), None, &state);
        assert_eq!(meta(&view), "Couldn't sign in");
        assert_eq!(view.details.as_ref().unwrap().status, "Couldn't sign in: the sign-in window was closed");
        assert_eq!(actions(&view), ["Try again", "Edit", "Forget"]);

        // Every VPN row says its type, and a protocol Lituus doesn't know is shown as it is.
        state.failed = None;
        snap.connections.push(saved("u-wg", "wg0", ConnectionKind::WireGuard, "wg0", None));
        let kind = ConnectionKind::Vpn {
            service: crate::openconnect::SERVICE.into(),
            gateway: "vpn.example.org".into(),
            protocol: "pulse-ng".into(),
        };
        snap.connections.push(saved("u-new", "New VPN", kind, "", None));
        let view = build(Some(&snap), None, &state);
        let metas = |title: &str| -> Vec<String> {
            let rows = &view.sections.iter().find(|s| s.title == title).unwrap().rows;
            (0..rows.row_count()).map(|i| rows.row_data(i).unwrap().meta.to_string()).collect()
        };
        assert_eq!(metas("WIREGUARD"), ["Off · WireGuard"]);
        assert_eq!(metas("VPN"), ["Off · Cisco AnyConnect", "Off · pulse-ng"]);
    }

    #[test]
    fn bridges_list_their_ports_and_vlans_sit_under_their_device() {
        let mut snap = home();
        snap.connections.push(saved("u-br", "br0", ConnectionKind::Bridge { stp: true }, "br0", None));
        snap.connections.push(saved("u-p1", "br0 port enp4s0", ConnectionKind::Ethernet, "enp4s0", Some("u-br")));
        snap.connections.push(saved(
            "u-v20",
            "VLAN 20 · Lab",
            ConnectionKind::Vlan { parent: "enp3s0".into(), id: 20 },
            "enp3s0.20",
            None,
        ));
        let state = UiState { page: Page::Wired, ..UiState::default() };
        let view = build(Some(&snap), None, &state);
        // The wired device first (the owner, 2026-11-06: "lets put the wire connections above the
        // bridges and bonds").
        assert_eq!(
            rows(&view),
            [
                ("ENP3S0 · E1000E".into(), vec!["Cable unplugged".into(), "VLAN 20 · Lab".into()]),
                ("BRIDGES AND BONDS".into(), vec!["br0".into(), "enp4s0".into()]),
            ]
        );
        let bridge = &view.sections[1].rows;
        assert_eq!(bridge.row_data(0).unwrap().meta, "Saved · bridge · 1 port");
        let port = bridge.row_data(1).unwrap();
        assert!(port.indent);
        assert_eq!((port.meta.as_str(), port.icon.as_str()), ("Port of br0 · not plugged in", "port"));
        assert!(view.sections[0].rows.row_data(1).unwrap().indent);
        // The first row is selected: the wired profile, not the bridge.
        snap.connections.push(saved("u-eth", "Wired connection 1", ConnectionKind::Ethernet, "enp3s0", None));
        snap.connections.push(saved(
            "u-bond",
            "bond0",
            ConnectionKind::Bond { mode: "active-backup".into() },
            "bond0",
            None,
        ));
        snap.connections.push(saved(
            "u-v30",
            "VLAN 30",
            ConnectionKind::Vlan { parent: "br0".into(), id: 30 },
            "br0.30",
            None,
        ));
        let view = build(Some(&snap), None, &state);
        assert_eq!(
            rows(&view),
            [
                ("ENP3S0 · E1000E".into(), vec!["Wired connection 1".into(), "VLAN 20 · Lab".into()]),
                ("BRIDGES AND BONDS".into(), vec!["br0".into(), "enp4s0".into(), "bond0".into()]),
                ("VLANS".into(), vec!["VLAN 30".into()]),
            ]
        );
        assert!(view.sections[0].rows.row_data(0).unwrap().selected);
        let state = UiState { page: Page::Wired, selected: Some("conn:u-br".into()), ..UiState::default() };
        let view = build(Some(&snap), None, &state);
        assert_eq!(detail(&view, "Ports").as_deref(), Some("enp4s0"));
        assert_eq!(detail(&view, "Spanning tree").as_deref(), Some("On"));
    }

    #[test]
    fn an_enterprise_network_opens_the_editor() {
        let mut snap = home();
        let wifi = snap.devices[0].wifi.as_mut().unwrap();
        wifi.access_points.push(ap("/ap/9", "eduroam", 5500, 70, Security::Enterprise));
        let view = build(Some(&snap), None, &UiState::default());
        let id = view.row_ids.iter().find(|id| id.contains(&hex(b"eduroam"))).unwrap();
        assert_eq!(
            intent(&snap, id, "connect"),
            Some(Intent::Run(Command::NewEnterprise { ssid: b"eduroam".to_vec(), suite_b: false }))
        );
    }

    #[test]
    fn search_filters_and_reports_nothing_found() {
        let state = UiState { search: "bib".into(), ..UiState::default() };
        let view = build(Some(&home()), None, &state);
        assert_eq!(view.row_ids.len(), 1);
        let state = UiState { search: "zzz".into(), ..UiState::default() };
        let view = build(Some(&home()), None, &state);
        // The page is still there: the hero keeps its height while typing.
        assert_eq!(view.empty.map(|e| (e.title, e.page)), Some(("Nothing found".into(), false)));
    }

    #[test]
    fn wifi_off_and_blocked_are_empty_states() {
        let mut snap = home();
        snap.wireless_enabled = false;
        let view = build(Some(&snap), None, &UiState::default());
        let empty = view.empty.as_ref().unwrap();
        assert_eq!(empty.action, "Turn on Wi-Fi");
        assert_eq!(empty.text, "Turn it on to see networks nearby. Your saved network is kept.");
        assert!(empty.page);
        assert_eq!((view.net_hero.sub.as_str(), view.net_hero.name.as_str()), ("Off", "Wi-Fi is off"));
        assert_eq!(view.net_hero.meta, "1 saved network");
        assert_eq!(view.net_items.wifi, "off");
        assert!(!view.show_search);
        // Off because of airplane mode: it says so, and the way back is to end airplane mode.
        let view = build(Some(&snap), None, &UiState { airplane: true, ..UiState::default() });
        let empty = view.empty.as_ref().unwrap();
        assert_eq!(empty.action, "Turn off airplane mode");
        assert_eq!(empty.text, "Wi-Fi is off because airplane mode is on. Your saved network is kept.");
        assert_eq!(view.net_hero.sub, "Off · airplane mode");
        snap.wireless_hardware_enabled = false;
        let view = build(Some(&snap), None, &UiState::default());
        assert_eq!(view.empty.as_ref().map(|e| e.title.as_str()), Some("Wi-Fi is blocked"));
    }

    #[test]
    fn wired_page_shows_the_unplugged_port() {
        let state = UiState { page: Page::Wired, ..UiState::default() };
        let view = build(Some(&home()), None, &state);
        assert_eq!(rows(&view), [("ENP3S0 · E1000E".into(), vec!["Cable unplugged".into()])]);
        assert_eq!(view.details.unwrap().status, "Cable unplugged");
    }

    #[test]
    fn appearance_is_part_of_settings() {
        // Lituus's own look moved to Settings; the old id still finds it.
        assert_eq!(Page::from_id("appearance"), Some(Page::Settings));
        assert_eq!(Page::from_id("settings").map(Page::id), Some("settings"));
    }

    #[test]
    fn network_manager_missing() {
        let view = build(None, Some("NetworkManager isn't running"), &UiState::default());
        assert!(view.banner.starts_with("NetworkManager isn't running"));
        let empty = view.empty.unwrap();
        assert_eq!(empty.title, "NetworkManager isn't running");
        // The command, for its Copy button.
        assert_eq!((empty.command.as_str(), empty.page), (NOT_RUNNING_COMMAND, true));
        assert!(view.sections.is_empty());
        // Still reading: a spinner (no icon), and the hero as it will be.
        let view = build(None, None, &UiState::default());
        assert_eq!(view.empty.map(|e| (e.icon, e.page)), Some(("", false)));
    }

    /// The bar lists only the keys that work: Enter says what it does to the selected row, and
    /// e is there when the row can be edited.
    #[test]
    fn the_bar_lists_the_keys_that_work() {
        let shown =
            |view: &View| -> Vec<String> { keys(view).iter().map(|h| format!("{} {}", h.key, h.label)).collect() };
        let mut state = UiState::default();
        let view = build(Some(&home()), None, &state);
        // The connected network: Enter would only connect it again.
        assert_eq!(shown(&view), ["/ Search", "↑↓ Select", "e Edit"]);
        state.selected = Some(view.row_ids[1].clone());
        assert_eq!(shown(&build(Some(&home()), None, &state)), ["/ Search", "↑↓ Select", "↵ Connect"]);
        // Wi-Fi off: no list to move in or search.
        let mut snap = home();
        snap.wireless_enabled = false;
        assert!(shown(&build(Some(&snap), None, &UiState::default())).is_empty());
    }

    #[test]
    fn network_opens_the_sub_page_used_last() {
        let (mut state, mut last) = (UiState::default(), Page::default());
        // Wi-Fi at first.
        assert_eq!(navigate(&mut state, &mut last, "network"), Some(Page::Wifi));
        state.search = "lumen".into();
        assert_eq!(navigate(&mut state, &mut last, "vpn"), Some(Page::Vpn));
        assert_eq!((state.page, last, state.search.as_str()), (Page::Vpn, Page::Vpn, ""));
        // Another page leaves the memory alone; Network goes back to VPN.
        assert_eq!(navigate(&mut state, &mut last, "bluetooth"), Some(Page::Bluetooth));
        assert_eq!(last, Page::Vpn);
        assert_eq!(navigate(&mut state, &mut last, "network"), Some(Page::Vpn));
        // There's no Saved page any more.
        assert_eq!(navigate(&mut state, &mut last, "saved"), None);
        assert_eq!(state.page, Page::Vpn);
        assert_eq!(Page::open("network", Page::Settings), Some(Page::Wifi));
        assert!([Page::Wifi, Page::Wired, Page::Vpn, Page::Hotspot].iter().all(|p| p.is_network()));
        assert!(!Page::Bluetooth.is_network());
    }

    fn with_saved(snap: &mut Snapshot, uuid: &str, id: &str, kind: ConnectionKind, timestamp: u64) {
        snap.connections.push(SavedConnection {
            interface: None,
            autoconnect: false,
            timestamp,
            ..saved(uuid, id, kind, "", None)
        });
    }

    #[test]
    fn saved_wifi_out_of_range_is_the_last_section_of_wifi() {
        let mut snap = home();
        let now = now();
        with_saved(
            &mut snap,
            "u-office",
            "Office",
            ConnectionKind::Wifi { ssid: b"Office".to_vec() },
            now - 3 * 86_400,
        );
        // The Lumen Home profile is in range, so it isn't listed twice.
        let view = build(Some(&snap), None, &UiState::default());
        let (title, names) = rows(&view).pop().unwrap();
        assert_eq!((title.as_str(), names), ("SAVED, NOT IN RANGE", vec!["Office".to_owned()]));
        let office = view.sections.last().unwrap().rows.row_data(0).unwrap();
        assert_eq!((office.meta.as_str(), office.icon.as_str()), ("Last used 3 days ago", "wifi"));
        assert_eq!(view.net_hero.sub, "Lumen Home · 2 other networks in range · 1 saved one out of range");
        // Its details still edit, forget and connect it.
        let state = UiState { selected: Some("conn:u-office".into()), ..UiState::default() };
        let d = build(Some(&snap), None, &state).details.unwrap();
        let actions: Vec<String> = d.actions.iter().map(|a| a.label.to_string()).collect();
        assert_eq!(actions, ["Connect", "Edit", "Forget"]);
        assert_eq!(d.toggles.row_data(0).unwrap().label, "Connect automatically");
        // Metered is the editor's three-way choice, under the switch; NetworkManager decides here.
        assert_eq!(d.toggles.row_count(), 1);
        assert!(d.has_metered);
        assert_eq!(d.metered, 0);
        // The search finds it too.
        let state = UiState { search: "off".into(), ..UiState::default() };
        assert_eq!(rows(&build(Some(&snap), None, &state)), [("SAVED, NOT IN RANGE".into(), vec!["Office".into()])]);
        assert_eq!(last_used(0, now), "Saved");
        assert_eq!(last_used(now - 3600, now), "Last used today");
        assert_eq!(last_used(now - 90 * 86_400, now), "Last used 3 months ago");
    }

    #[test]
    fn the_wired_page_keeps_what_the_saved_page_listed() {
        let mut snap = home();
        with_saved(&mut snap, "u-lab", "Static (lab)", ConnectionKind::Ethernet, 0);
        snap.connections.push(saved("u-dock", "Dock", ConnectionKind::Ethernet, "enx00e04c", None));
        with_saved(&mut snap, "u-bt", "Phone", ConnectionKind::Bluetooth { address: "01:02".into() }, 0);
        with_saved(&mut snap, "u-gsm", "Mobile", ConnectionKind::Other("gsm".into()), 0);
        let state = UiState { page: Page::Wired, ..UiState::default() };
        let view = build(Some(&snap), None, &state);
        assert_eq!(
            rows(&view),
            [
                ("ENP3S0 · E1000E".into(), vec!["Static (lab)".into()]),
                ("SAVED, NOT PLUGGED IN".into(), vec!["Dock".into()]),
                ("OTHER CONNECTIONS".into(), vec!["Phone".into(), "Mobile".into()]),
            ]
        );
        assert_eq!(view.sections[1].rows.row_data(0).unwrap().meta, "Saved · only on enx00e04c");
        assert_eq!(view.count, "1 wired device");
        assert_eq!(view.bottom_label, "Add a wired profile…");
        assert!(!view.show_search);
        let hero = &view.net_hero;
        assert_eq!((hero.sub.as_str(), hero.name.as_str()), ("Cable unplugged · 2 saved profiles", "Cable unplugged"));
        assert_eq!(hero.detail, "enp3s0   54:E1:AD:7C:20:9B");
        // What to do is a hint, not a state, so it isn't in the accent.
        assert_eq!((hero.meta.as_str(), hero.tone), ("Plug in a cable to connect", "muted"));
        // Plugged in and connected: the hero is the connection, with its speed.
        let eth = &mut snap.devices[1];
        eth.wired = Some(WiredDevice { carrier: true, speed: 1000 });
        eth.active_connection = Some("/a/lab".into());
        snap.active.push(ActiveConnection {
            path: "/a/lab".into(),
            id: "Static (lab)".into(),
            uuid: "u-lab".into(),
            state: ActiveState::Activated,
            devices: vec!["/d/eth".into()],
            connection: Some("/c/u-lab".into()),
            specific_object: None,
            vpn: false,
        });
        let view = build(Some(&snap), None, &state);
        assert_eq!(view.net_items.wired, "1000 Mb/s");
        assert_eq!(
            (view.net_hero.name.as_str(), view.net_hero.meta.as_str()),
            ("Static (lab)", "Connected · 1000 Mb/s")
        );
        assert_eq!(view.net_hero.tone, "accent");
    }

    #[test]
    fn the_vpn_hero_shows_the_vpn_that_is_on() {
        let mut snap = home();
        with_saved(&mut snap, "u-wg", "Home (WireGuard)", ConnectionKind::WireGuard, 100);
        let openconnect = ConnectionKind::Vpn {
            service: crate::openconnect::SERVICE.into(),
            gateway: "vpn.uzh.ch".into(),
            protocol: "anyconnect".into(),
        };
        with_saved(&mut snap, "u-uzh", "UZH VPN", openconnect, 200);
        let openvpn =
            ConnectionKind::Vpn { service: "openvpn".into(), gateway: String::new(), protocol: String::new() };
        with_saved(&mut snap, "u-ovpn", "Office", openvpn, 0);
        let mut state = UiState { page: Page::Vpn, ..UiState::default() };
        let view = build(Some(&snap), None, &state);
        let hero = &view.net_hero;
        assert_eq!(
            (hero.sub.as_str(), hero.name.as_str(), hero.meta.as_str()),
            ("Off · 3 saved", "No VPN on", "3 VPNs saved")
        );
        assert_eq!(hero.tone, "muted");
        assert_eq!((view.count.as_str(), view.search_placeholder.as_str()), ("3 VPNs", "Search VPNs"));
        assert!(view.show_search);
        state.search = "uzh".into();
        assert_eq!(build(Some(&snap), None, &state).row_ids, ["conn:u-uzh"]);
        // On: the hero is that VPN, and the sidebar names it.
        snap.active.push(ActiveConnection {
            path: "/a/wg".into(),
            id: "Home (WireGuard)".into(),
            uuid: "u-wg".into(),
            state: ActiveState::Activated,
            devices: vec![],
            connection: Some("/c/u-wg".into()),
            specific_object: None,
            vpn: false,
        });
        let view = build(Some(&snap), None, &UiState { page: Page::Vpn, ..UiState::default() });
        assert_eq!(view.net_items.vpn, "Home (WireGuard)");
        assert_eq!(view.net_hero.sub, "On · 3 saved");
        assert_eq!((view.net_hero.meta.as_str(), view.net_hero.tone), ("Connected · WireGuard", "accent"));
    }

    #[test]
    fn the_hotspot_hero() {
        let state = UiState { page: Page::Hotspot, ..UiState::default() };
        let view = build(Some(&home()), None, &state);
        let hero = &view.net_hero;
        assert_eq!((hero.sub.as_str(), hero.name.as_str()), ("Off", "Hotspot off"));
        assert_eq!(hero.tone, "muted");
        assert!(view.count.is_empty() && view.sections.is_empty());
        assert_eq!(view.net_items.hotspot, "off");
    }

    #[test]
    fn buttons_become_commands() {
        let snap = home();
        let ids = build(Some(&snap), None, &UiState::default()).row_ids;
        // Connected "Lumen Home": disconnect it, or forget it after asking.
        assert_eq!(
            intent(&snap, &ids[0], "disconnect"),
            Some(Intent::Run(Command::Deactivate { active: "/a/1".into() }))
        );
        assert!(matches!(
            intent(&snap, &ids[0], "forget"),
            Some(Intent::Confirm { command: Command::Forget { .. }, .. })
        ));
        // The open library network is joined directly; the secured café one asks for the password.
        let library = ids.iter().find(|id| id.ends_with(":Open")).unwrap();
        assert!(matches!(intent(&snap, library, "connect"), Some(Intent::Run(Command::ConnectOpen { .. }))));
        let Some(Intent::Join(cafe)) = intent(&snap, &ids[1], "connect") else { panic!("the password dialog") };
        assert_eq!((cafe.name.as_str(), cafe.key_mgmt), ("Aster Café", KeyMgmt::WpaPsk));
        assert_eq!(wifi_switch(&snap), Command::SetWifi(false));
        assert_eq!(rescan(&snap), Some(Command::Rescan { device: "/d/wlan".into() }));
    }

    #[test]
    fn saved_wifi_reconnects_through_its_strongest_access_point() {
        let mut snap = home();
        snap.active.clear();
        snap.devices[0].active_connection = None;
        snap.devices[0].wifi.as_mut().unwrap().active_access_point = None;
        let ids = build(Some(&snap), None, &UiState::default()).row_ids;
        assert_eq!(
            intent(&snap, &ids[0], "connect"),
            Some(Intent::Run(Command::Activate {
                connection: "/c/1".into(),
                device: Some("/d/wlan".into()),
                access_point: Some("/ap/1".into())
            }))
        );
    }

    #[test]
    fn a_failed_attempt_marks_its_row_and_details() {
        let snap = home();
        let ids = build(Some(&snap), None, &UiState::default()).row_ids;
        let cafe = ids[1].clone();
        assert_eq!(row_name(&snap, &cafe).as_deref(), Some("Aster Café"));
        let state = UiState {
            selected: Some(cafe.clone()),
            failed: Some((cafe, "The network rejected the password.".into())),
            ..UiState::default()
        };
        let v = build(Some(&snap), None, &state);
        let row = v.sections.iter().flat_map(|s| s.rows.iter()).find(|r| r.name == "Aster Café").unwrap();
        assert_eq!((row.meta.as_str(), row.tone.as_str()), ("Couldn't connect · WPA2", "danger"));
        let d = v.details.unwrap();
        assert_eq!((d.status.as_str(), d.status_tone.as_str()), ("Not connected", "danger"));
    }

    #[test]
    fn hidden_networks_are_joined_on_the_wifi_device() {
        let mut snap = home();
        snap.wireless_enabled = false;
        assert_eq!(hidden_network_device(&snap).as_deref(), Some("/d/wlan"));
    }

    #[test]
    fn edit_and_the_details_switches_become_commands() {
        let snap = home();
        let ids = build(Some(&snap), None, &UiState::default()).row_ids;
        assert_eq!(
            intent(&snap, &ids[0], "edit"),
            Some(Intent::Run(Command::OpenEditor { connection: Some("/c/1".into()), new: None }))
        );
        // The unsaved café network has nothing to edit.
        assert_eq!(intent(&snap, &ids[1], "edit"), None);
        assert_eq!(
            toggle_command(&snap, &ids[0], "autoconnect"),
            Some(Command::SetSetting {
                connection: "/c/1".into(),
                setting: "connection",
                key: "autoconnect",
                value: SettingValue::Bool(false)
            })
        );
        assert_eq!(toggle_command(&snap, &ids[0], "metered"), None);
        // Metered: the choice is NetworkManager's number, so Automatic is 0 again.
        for (choice, value) in [(0, 0), (1, 1), (2, 2), (7, 2)] {
            assert_eq!(
                metered_command(&snap, &ids[0], choice),
                Some(Command::SetSetting {
                    connection: "/c/1".into(),
                    setting: "connection",
                    key: "metered",
                    value: SettingValue::I32(value)
                })
            );
        }
        // The unsaved café network has none.
        assert_eq!(metered_command(&snap, &ids[1], 1), None);
        let state = UiState { selected: Some(ids[1].clone()), ..UiState::default() };
        assert!(!build(Some(&snap), None, &state).details.unwrap().has_metered);
        // A saved network shows its own choice.
        let mut metered = snap.clone();
        metered.connections[0].metered = Some(false);
        assert_eq!(build(Some(&metered), None, &UiState::default()).details.unwrap().metered, 2);
    }

    #[test]
    fn traffic_in_words() {
        assert_eq!(bytes_label(512), "512 B");
        assert_eq!(bytes_label(19_293_798), "18.4 MiB");
        let snap = home();
        assert_eq!(traffic_item(&snap.devices[1]).unwrap().value, "18.4 MiB / 2.1 MiB");
    }

    #[test]
    fn the_hotspot_page() {
        let snap = home();
        let h = hotspot_view(&snap);
        assert!(h.connection.is_none() && h.active.is_none());
        assert_eq!(h.device.as_deref(), Some("/d/wlan"));
        assert_eq!(h.status, "Off");
        assert!(h.warning.contains("disconnect Wi-Fi"), "Wi-Fi is connected: {}", h.warning);
    }
}
