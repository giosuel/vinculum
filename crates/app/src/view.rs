//! What the window shows, worked out from a NetworkManager snapshot and the UI state (page,
//! selection, search). Pure, so it's tested without a window.

use slint::{ModelRc, SharedString, VecModel};
use vinculum_nm::{
    ActiveState, Band, ConnectionKind, Device, DeviceKind, KeyMgmt, Network, SavedConnection, Security, Snapshot,
    WifiJoin, bars, channel,
};

use crate::{Action, DetailItem, Details, NavItem, Row, Section, Toggle};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Wifi,
    Wired,
    Vpn,
    Hotspot,
    Saved,
    Settings,
}

impl Page {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "wifi" => Self::Wifi,
            "wired" => Self::Wired,
            "vpn" => Self::Vpn,
            "hotspot" => Self::Hotspot,
            "saved" => Self::Saved,
            "settings" => Self::Settings,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Wired => "wired",
            Self::Vpn => "vpn",
            Self::Hotspot => "hotspot",
            Self::Saved => "saved",
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
}

/// An empty state shown instead of the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Empty {
    pub icon: &'static str,
    pub title: String,
    pub text: String,
    pub action: String,
}

/// Everything the window shows.
#[derive(Default)]
pub struct View {
    pub nav: Vec<NavItem>,
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
    pub placeholder: String,
    pub hostname: String,
    pub hotspot: Option<HotspotView>,
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
struct RowData {
    id: String,
    name: String,
    meta: String,
    tone: &'static str,
    icon: &'static str,
    bars: u8,
    secure: bool,
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

fn toggle(id: &str, label: &str, sub: &str, on: bool) -> Toggle {
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
    items: Vec<DetailItem>,
    note: String,
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
            items: model(d.items),
            note: s(d.note),
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

fn active_uuid_state<'a>(snapshot: &'a Snapshot, uuid: &str) -> Option<&'a vinculum_nm::ActiveConnection> {
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
    view.nav = nav(snapshot, state.page);
    view.hostname = snapshot.map(|s| s.hostname.clone()).unwrap_or_default();
    let Some(snapshot) = snapshot else {
        view.page_title = page_title(state.page).into();
        view.show_search = false;
        view.banner = match error {
            Some(e) => format!("{e}. Vinculum needs it to manage connections."),
            None => String::new(),
        };
        view.empty = Some(Empty {
            icon: "warning",
            title: if error.is_some() {
                "NetworkManager isn't running".into()
            } else {
                "Reading NetworkManager…".into()
            },
            text: if error.is_some() {
                "Start it with: systemctl enable --now NetworkManager".into()
            } else {
                String::new()
            },
            action: String::new(),
        });
        return view;
    };
    view.page_title = page_title(state.page).into();
    let rows = match state.page {
        Page::Wifi => wifi_page(snapshot, state, &mut view),
        Page::Wired => wired_page(snapshot, &mut view),
        Page::Vpn => vpn_page(snapshot, &mut view),
        Page::Saved => saved_page(snapshot, state, &mut view),
        Page::Hotspot => {
            view.hotspot = Some(hotspot_page(snapshot));
            Vec::new()
        }
        Page::Settings => Vec::new(),
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
                    })
                    .collect(),
            ),
        })
        .collect();
    if let Some(id) = &selected {
        view.details = details(snapshot, state, id).map(Details::from);
    }
    if view.empty.is_none() && view.row_ids.is_empty() && state.page != Page::Settings && view.placeholder.is_empty() {
        view.empty = Some(nothing_here(state.page, !state.search.is_empty()));
    }
    view.selected = selected;
    view
}

fn page_title(page: Page) -> &'static str {
    match page {
        Page::Wifi => "Wi-Fi",
        Page::Wired => "Wired",
        Page::Vpn => "VPN",
        Page::Hotspot => "Hotspot",
        Page::Saved => "Saved connections",
        Page::Settings => "Settings",
    }
}

fn nothing_here(page: Page, searching: bool) -> Empty {
    if searching {
        return Empty {
            icon: "search",
            title: "Nothing found".into(),
            text: "No name matches the search.".into(),
            action: String::new(),
        };
    }
    match page {
        Page::Wired => Empty {
            icon: "wired",
            title: "No wired network card".into(),
            text: "Plug in a USB network adapter to see it here.".into(),
            action: String::new(),
        },
        Page::Vpn => Empty {
            icon: "vpn",
            title: "No VPN connections".into(),
            text: "Add a WireGuard or OpenVPN connection with +, or bring one in from a .conf or .ovpn file.".into(),
            action: "Import a file…".into(),
        },
        _ => Empty {
            icon: "wifi-off",
            title: "No networks in range".into(),
            text: "Try rescanning.".into(),
            action: String::new(),
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

fn wifi_device(snapshot: &Snapshot) -> Option<&Device> {
    snapshot.devices.iter().find(|d| d.kind == DeviceKind::Wifi)
}

fn nav(snapshot: Option<&Snapshot>, page: Page) -> Vec<NavItem> {
    let item = |id: Page, icon: &str, sub: String| NavItem {
        id: s(id.id()),
        label: s(page_title(id)),
        sub: s(sub),
        icon: s(icon),
        current: id == page,
    };
    let Some(snap) = snapshot else {
        return [
            (Page::Wifi, "wifi"),
            (Page::Wired, "wired"),
            (Page::Vpn, "vpn"),
            (Page::Hotspot, "hotspot"),
            (Page::Saved, "saved"),
        ]
        .into_iter()
        .map(|(p, i)| item(p, i, String::new()))
        .collect();
    };
    let active_on = |kind: DeviceKind| {
        snap.devices
            .iter()
            .filter(|d| d.kind == kind)
            .find_map(|d| d.active_connection.as_deref().and_then(|a| snap.active_connection(a)))
            .map(|a| a.id.clone())
    };
    let wifi = if wifi_device(snap).is_none() {
        "No device".into()
    } else if !snap.wireless_hardware_enabled {
        "Blocked".into()
    } else if !snap.wireless_enabled {
        "Off".into()
    } else {
        active_on(DeviceKind::Wifi).unwrap_or_else(|| "Not connected".into())
    };
    let wired_devices: Vec<&Device> = snap.devices.iter().filter(|d| d.kind == DeviceKind::Ethernet).collect();
    let wired = match active_on(DeviceKind::Ethernet) {
        Some(id) => id,
        None if wired_devices.is_empty() => "No device".into(),
        None if wired_devices.iter().all(|d| d.wired.as_ref().is_some_and(|w| !w.carrier)) => "Cable unplugged".into(),
        None => "Not connected".into(),
    };
    let vpns: Vec<String> = snap
        .active
        .iter()
        .filter(|a| {
            a.vpn
                || a.connection
                    .as_deref()
                    .and_then(|c| snap.connection(c))
                    .is_some_and(|c| c.kind == ConnectionKind::WireGuard)
        })
        .map(|a| a.id.clone())
        .collect();
    let saved = match snap.connections.len() {
        1 => "1 connection".to_owned(),
        n => format!("{n} connections"),
    };
    vec![
        item(Page::Wifi, "wifi", wifi),
        item(Page::Wired, "wired", wired),
        item(Page::Vpn, "vpn", if vpns.is_empty() { "Off".into() } else { vpns.join(", ") }),
        item(Page::Hotspot, "hotspot", if hotspot_page(snap).active.is_some() { "On".into() } else { "Off".into() }),
        item(Page::Saved, "saved", saved),
    ]
}

fn network_id(n: &Network) -> String {
    format!("wifi:{}:{}", hex(&n.ssid), n.security.label())
}

fn wifi_page(snap: &Snapshot, state: &UiState, view: &mut View) -> Vec<(String, Vec<RowData>)> {
    view.show_wifi_switch = true;
    view.wifi_on = snap.wireless_enabled && snap.wireless_hardware_enabled;
    view.show_rescan = true;
    view.search_placeholder = "Search networks".into();
    let Some(device) = wifi_device(snap) else {
        view.show_wifi_switch = false;
        view.show_rescan = false;
        view.show_search = false;
        view.empty = Some(Empty {
            icon: "wifi-off",
            title: "No Wi-Fi card".into(),
            text: "Vinculum didn't find a Wi-Fi device. Wired and VPN connections are in their own sections.".into(),
            action: String::new(),
        });
        return Vec::new();
    };
    if !snap.wireless_hardware_enabled {
        view.show_search = false;
        view.empty = Some(Empty {
            icon: "wifi-off",
            title: "Wi-Fi is blocked".into(),
            text: "A hardware switch or airplane mode turned the radio off.".into(),
            action: String::new(),
        });
        return Vec::new();
    }
    if !snap.wireless_enabled {
        view.show_search = false;
        view.empty = Some(Empty {
            icon: "wifi-off",
            title: "Wi-Fi is off".into(),
            text: "Wired and VPN connections keep working.".into(),
            action: "Turn on Wi-Fi".into(),
        });
        return Vec::new();
    }
    view.bottom_label = "Connect to hidden network…".into();
    let networks = snap.networks().into_iter().find(|(p, _)| *p == device.path).map(|(_, n)| n).unwrap_or_default();
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
        };
        if n.active { connected.push(row) } else { available.push(row) }
    }
    vec![("Connected".into(), connected), ("Available".into(), available)]
}

fn connection_kind_label(c: &SavedConnection) -> String {
    match &c.kind {
        ConnectionKind::Wifi { .. } => "Wi-Fi".into(),
        ConnectionKind::Ethernet => "Wired".into(),
        ConnectionKind::WireGuard => "WireGuard".into(),
        ConnectionKind::Vpn { service } => vpn_service_label(service),
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
        ConnectionKind::WireGuard | ConnectionKind::Vpn { .. } => "vpn",
        ConnectionKind::Other(_) => "saved",
    }
}

fn wired_page(snap: &Snapshot, view: &mut View) -> Vec<(String, Vec<RowData>)> {
    view.show_add = true;
    view.show_search = false;
    let mut sections = Vec::new();
    for device in snap.devices.iter().filter(|d| d.kind == DeviceKind::Ethernet) {
        let carrier = device.wired.as_ref().is_some_and(|w| w.carrier);
        let active_uuid =
            device.active_connection.as_deref().and_then(|a| snap.active_connection(a)).map(|a| a.uuid.as_str());
        let mut rows: Vec<RowData> = snap
            .connections
            .iter()
            .filter(|c| {
                c.kind == ConnectionKind::Ethernet && c.interface.as_deref().is_none_or(|i| i == device.interface)
            })
            .map(|c| {
                let active = active_uuid == Some(c.uuid.as_str());
                let (meta, tone) = if active {
                    (format!("Connected{}", speed(device)), "accent")
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
                    bars: 0,
                    secure: false,
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
                bars: 0,
                secure: false,
            });
        }
        sections.push((device_title(device), rows));
    }
    sections
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

fn vpn_page(snap: &Snapshot, view: &mut View) -> Vec<(String, Vec<RowData>)> {
    view.show_add = true;
    view.show_search = false;
    view.bottom_label = "Import a WireGuard or OpenVPN file…".into();
    let (mut wireguard, mut others) = (Vec::new(), Vec::new());
    for c in
        snap.connections.iter().filter(|c| matches!(c.kind, ConnectionKind::WireGuard | ConnectionKind::Vpn { .. }))
    {
        let active = active_uuid_state(snap, &c.uuid);
        let (meta, tone) = match active.map(|a| a.state) {
            Some(ActiveState::Activated) => ("Connected".to_owned(), "accent"),
            Some(ActiveState::Activating) => ("Connecting…".to_owned(), "accent"),
            _ if matches!(c.kind, ConnectionKind::WireGuard) => ("Off".to_owned(), "muted"),
            _ => (format!("Off · {}", connection_kind_label(c)), "muted"),
        };
        let row = RowData {
            id: format!("conn:{}", c.uuid),
            name: c.id.clone(),
            meta,
            tone,
            icon: "vpn",
            bars: 0,
            secure: false,
        };
        if c.kind == ConnectionKind::WireGuard { wireguard.push(row) } else { others.push(row) }
    }
    vec![("WireGuard".into(), wireguard), ("VPN".into(), others)]
}

/// Whether a connection belongs in a group of the Saved page.
type KindFilter = fn(&ConnectionKind) -> bool;

fn saved_page(snap: &Snapshot, state: &UiState, view: &mut View) -> Vec<(String, Vec<RowData>)> {
    view.show_add = true;
    view.search_placeholder = "Search saved connections".into();
    view.bottom_label = "Import a connection file…".into();
    let groups: [(&str, KindFilter); 4] = [
        ("Wi-Fi", |k| matches!(k, ConnectionKind::Wifi { .. })),
        ("Wired", |k| matches!(k, ConnectionKind::Ethernet)),
        ("VPN", |k| matches!(k, ConnectionKind::WireGuard | ConnectionKind::Vpn { .. })),
        ("Other", |k| matches!(k, ConnectionKind::Other(_))),
    ];
    groups
        .into_iter()
        .map(|(title, belongs)| {
            let rows = snap
                .connections
                .iter()
                .filter(|c| belongs(&c.kind) && matches_search(&c.id, &state.search))
                .map(|c| {
                    let active = active_uuid_state(snap, &c.uuid).is_some_and(|a| a.state == ActiveState::Activated);
                    RowData {
                        id: format!("conn:{}", c.uuid),
                        name: c.id.clone(),
                        meta: if active {
                            format!("{} · connected", connection_kind_label(c))
                        } else {
                            connection_kind_label(c)
                        },
                        tone: if active { "accent" } else { "muted" },
                        icon: icon_for(c),
                        bars: 0,
                        secure: false,
                    }
                })
                .collect();
            (title.to_owned(), rows)
        })
        .collect()
}

fn details(snap: &Snapshot, state: &UiState, id: &str) -> Option<DetailsData> {
    if let Some(rest) = id.strip_prefix("wifi:") {
        return wifi_details(snap, state, rest);
    }
    if let Some(uuid) = id.strip_prefix("conn:") {
        return connection_details(snap, state.page, uuid);
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
        Some(c) => vec![
            toggle("autoconnect", "Connect automatically", "", c.autoconnect),
            toggle("metered", "Metered connection", "Apps may limit background data", c.metered == Some(true)),
        ],
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
        items,
        note: String::new(),
    })
}

fn connection_details(snap: &Snapshot, page: Page, uuid: &str) -> Option<DetailsData> {
    let c = snap.connections.iter().find(|c| c.uuid == uuid)?;
    let active = active_uuid_state(snap, uuid);
    let device = active.and_then(|a| a.devices.first()).and_then(|p| snap.device(p));
    let (status, tone) = match active.map(|a| a.state) {
        Some(ActiveState::Activated) => ("Connected", "accent"),
        Some(ActiveState::Activating) => ("Connecting…", "accent"),
        Some(ActiveState::Deactivating) => ("Disconnecting…", "muted"),
        _ => ("Not connected", "faint"),
    };
    let connected = active.is_some_and(|a| a.state == ActiveState::Activated);
    let extra = match device {
        Some(d) if connected && c.kind == ConnectionKind::Ethernet => speed(d).trim_start_matches(" · ").to_owned(),
        _ => String::new(),
    };
    let mut actions = vec![if connected {
        action("disconnect", "Disconnect", "secondary")
    } else {
        action("connect", "Connect", "primary")
    }];
    actions.push(action("edit", "Edit", "secondary"));
    actions.push(action("forget", "Forget", "danger"));
    let mut toggles = vec![toggle("autoconnect", "Connect automatically", "", c.autoconnect)];
    if page != Page::Vpn {
        toggles.push(toggle("metered", "Metered connection", "", c.metered == Some(true)));
    }
    let mut items = Vec::new();
    if let (Some(d), true) = (device, connected) {
        items.extend(ip_items(d));
        items.extend(traffic_item(d));
        items.push(item("Interface", d.interface.as_str(), true));
        if !d.hw_address.is_empty() {
            items.push(item("MAC address", d.hw_address.as_str(), true));
        }
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
    SetAirplane(bool),
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
    },
    /// One key of a saved connection, from the details pane's switches.
    SetSetting {
        connection: String,
        setting: &'static str,
        key: &'static str,
        value: SettingValue,
    },
    SaveHostname(String),
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
    Forget {
        connection: String,
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

/// The switches in the details pane ("autoconnect", "metered") as a command.
pub fn toggle_command(snap: &Snapshot, selected: &str, id: &str) -> Option<Command> {
    let c = saved_connection(snap, selected)?;
    let (key, value) = match id {
        "autoconnect" => ("autoconnect", SettingValue::Bool(!c.autoconnect)),
        // NMMetered: 1 yes, 2 no.
        "metered" => ("metered", SettingValue::I32(if c.metered == Some(true) { 2 } else { 1 })),
        _ => return None,
    };
    Some(Command::SetSetting { connection: c.path.clone(), setting: "connection", key, value })
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

/// Airplane mode on or off.
pub fn airplane_switch(snap: &Snapshot) -> Command {
    Command::SetAirplane(!snap.airplane())
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
mod tests {
    use super::*;
    use slint::Model;
    use vinculum_nm::{AccessPoint, ActiveConnection, DeviceState, IpConfig, WifiDevice, WiredDevice};

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
    fn home() -> Snapshot {
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
        assert_eq!(view.nav[0].sub, "Lumen Home");
        assert_eq!(view.nav[1].sub, "Cable unplugged");
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
        assert_eq!(view.empty.map(|e| e.title), Some("Nothing found".into()));
    }

    #[test]
    fn wifi_off_and_blocked_are_empty_states() {
        let mut snap = home();
        snap.wireless_enabled = false;
        let view = build(Some(&snap), None, &UiState::default());
        assert_eq!(view.empty.as_ref().map(|e| e.action.as_str()), Some("Turn on Wi-Fi"));
        assert_eq!(view.nav[0].sub, "Off");
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
    fn network_manager_missing() {
        let view = build(None, Some("NetworkManager isn't running"), &UiState::default());
        assert!(view.banner.starts_with("NetworkManager isn't running"));
        assert_eq!(view.empty.map(|e| e.title), Some("NetworkManager isn't running".into()));
        assert!(view.sections.is_empty());
    }

    #[test]
    fn saved_page_groups_by_type() {
        let state = UiState { page: Page::Saved, ..UiState::default() };
        let view = build(Some(&home()), None, &state);
        assert_eq!(rows(&view), [("WI-FI".into(), vec!["Lumen Home".into()])]);
        assert_eq!(view.nav[4].sub, "1 connection");
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
    fn airplane_mode_is_wifi_and_mobile_broadband_off() {
        let mut snap = home();
        assert_eq!(airplane_switch(&snap), Command::SetAirplane(true));
        snap.wireless_enabled = false;
        assert!(!snap.airplane(), "mobile broadband is still on");
        snap.wwan_enabled = false;
        assert_eq!(airplane_switch(&snap), Command::SetAirplane(false));
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
        assert!(matches!(
            toggle_command(&snap, &ids[0], "metered"),
            Some(Command::SetSetting { key: "metered", value: SettingValue::I32(1), .. })
        ));
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
