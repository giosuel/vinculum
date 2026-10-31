//! What the tray menu and the quick popup show: the Wi-Fi switch, the nearest networks, the VPNs
//! and airplane mode. Their rows use the main window's row IDs, so picking one runs the same
//! action as the list does (a password dialog for a new secured network, and so on).

use lituus_nm::{ActiveState, ConnectionKind, DeviceKind, Snapshot};

use crate::view;

/// How many networks the popup lists; the tray menu puts the rest under "More networks".
pub const NETWORKS: usize = 5;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Quick {
    /// There's a Wi-Fi card, and no hardware switch blocks it.
    pub wifi_available: bool,
    pub wifi_on: bool,
    /// The Wi-Fi line of the tray menu, such as "Wi-Fi: Lumen Home".
    pub status: String,
    pub networks: Vec<QuickNetwork>,
    pub vpns: Vec<QuickVpn>,
    pub airplane: bool,
    pub icon: TrayIcon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuickNetwork {
    /// The main window's row ID (`wifi:…`).
    pub id: String,
    pub name: String,
    /// "WPA2 · 2.4 / 5 GHz", "Saved · 5 GHz".
    pub meta: String,
    pub bars: u8,
    pub active: bool,
    pub connecting: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuickVpn {
    /// The main window's row ID (`conn:…`).
    pub id: String,
    pub name: String,
    pub meta: String,
    /// Connected or connecting.
    pub on: bool,
    pub connected: bool,
}

/// What the tray icon shows, from the most important connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrayIcon {
    #[default]
    Offline,
    Airplane,
    WifiOff,
    Connecting,
    /// Wi-Fi with 0 to 4 bars.
    Wifi(u8),
    Wired,
    Vpn,
}

impl TrayIcon {
    /// The icon theme's name for it (Adwaita, Papirus and Breeze all have these).
    pub fn name(self) -> &'static str {
        match self {
            Self::Offline => "network-offline-symbolic",
            Self::Airplane => "airplane-mode-symbolic",
            Self::WifiOff => "network-wireless-disabled-symbolic",
            Self::Connecting => "network-wireless-acquiring-symbolic",
            Self::Wifi(0) => "network-wireless-signal-none-symbolic",
            Self::Wifi(1) => "network-wireless-signal-weak-symbolic",
            Self::Wifi(2) => "network-wireless-signal-ok-symbolic",
            Self::Wifi(3) => "network-wireless-signal-good-symbolic",
            Self::Wifi(_) => "network-wireless-signal-excellent-symbolic",
            Self::Wired => "network-wired-symbolic",
            Self::Vpn => "network-vpn-symbolic",
        }
    }
}

pub fn build(snap: Option<&Snapshot>) -> Quick {
    let Some(snap) = snap else { return Quick { status: "NetworkManager isn't running".into(), ..Quick::default() } };
    let device = view::wifi_device(snap);
    let wifi_available = device.is_some() && snap.wireless_hardware_enabled;
    let wifi_on = wifi_available && snap.wireless_enabled;
    let mut networks = Vec::new();
    if let Some(device) = device.filter(|_| wifi_on) {
        let connecting = device.state.is_connecting();
        let found = snap.networks().into_iter().find(|(p, _)| *p == device.path).map(|(_, n)| n).unwrap_or_default();
        for n in found {
            let bands: Vec<&str> = n.bands.iter().map(|b| b.label().trim_end_matches(" GHz")).collect();
            let bands = if bands.is_empty() { String::new() } else { format!(" · {} GHz", bands.join(" / ")) };
            let what = if n.saved.is_some() { "Saved" } else { n.security.label() };
            networks.push(QuickNetwork {
                id: view::network_id(&n),
                name: n.name(),
                meta: format!("{what}{bands}"),
                bars: lituus_nm::bars(n.strength()),
                active: n.active,
                connecting: n.active && connecting,
            });
        }
    }
    let active_state = |uuid: &str| snap.active.iter().find(|a| a.uuid == uuid).map(|a| a.state);
    let vpns: Vec<QuickVpn> = snap
        .connections
        .iter()
        .filter(|c| matches!(c.kind, ConnectionKind::WireGuard | ConnectionKind::Vpn { .. }))
        .map(|c| {
            let state = active_state(&c.uuid);
            let kind = if c.kind == ConnectionKind::WireGuard { "WireGuard" } else { "VPN" };
            let meta = match state {
                Some(ActiveState::Activated) => format!("{kind} · connected"),
                Some(ActiveState::Activating) => format!("{kind} · connecting…"),
                _ => kind.to_owned(),
            };
            QuickVpn {
                id: format!("conn:{}", c.uuid),
                name: c.id.clone(),
                meta,
                on: matches!(state, Some(ActiveState::Activated | ActiveState::Activating)),
                connected: state == Some(ActiveState::Activated),
            }
        })
        .collect();
    let connected = networks.iter().find(|n| n.active);
    let status = if device.is_none() {
        "Wi-Fi: no device".to_owned()
    } else if !snap.wireless_hardware_enabled {
        "Wi-Fi: blocked".to_owned()
    } else if !snap.wireless_enabled {
        "Wi-Fi: off".to_owned()
    } else {
        match connected {
            Some(n) if n.connecting => format!("Wi-Fi: connecting to {}…", n.name),
            Some(n) => format!("Wi-Fi: {}", n.name),
            None => "Wi-Fi: not connected".to_owned(),
        }
    };
    let icon = icon(snap, connected, vpns.iter().any(|v| v.connected));
    Quick { wifi_available, wifi_on, status, networks, vpns, airplane: snap.airplane(), icon }
}

fn icon(snap: &Snapshot, wifi: Option<&QuickNetwork>, vpn: bool) -> TrayIcon {
    let wired = snap.devices.iter().any(|d| {
        d.kind == DeviceKind::Ethernet
            && d.active_connection
                .as_deref()
                .and_then(|a| snap.active_connection(a))
                .is_some_and(|a| a.state == ActiveState::Activated)
    });
    match wifi {
        _ if vpn => TrayIcon::Vpn,
        _ if wired => TrayIcon::Wired,
        Some(n) if n.connecting => TrayIcon::Connecting,
        Some(n) => TrayIcon::Wifi(n.bars),
        None if snap.airplane() => TrayIcon::Airplane,
        None if view::wifi_device(snap).is_some() && !snap.wireless_enabled => TrayIcon::WifiOff,
        None => TrayIcon::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::home;

    #[test]
    fn home_network_and_its_neighbours() {
        let q = build(Some(&home()));
        assert!(q.wifi_on);
        assert_eq!(q.status, "Wi-Fi: Lumen Home");
        let names: Vec<&str> = q.networks.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["Lumen Home", "Aster Café", "Bibliothek-Free"]);
        assert_eq!(q.networks[0].meta, "Saved · 5 GHz");
        assert_eq!(q.networks[1].meta, "WPA2 · 2.4 GHz");
        assert!(q.networks[0].active && !q.networks[1].active);
        assert_eq!(q.networks[0].bars, 4);
        assert_eq!(q.icon, TrayIcon::Wifi(4));
        assert_eq!(q.icon.name(), "network-wireless-signal-excellent-symbolic");
    }

    #[test]
    fn wifi_off_and_airplane_mode() {
        let mut snap = home();
        snap.wireless_enabled = false;
        snap.active.clear();
        snap.devices[0].active_connection = None;
        let q = build(Some(&snap));
        assert_eq!(q.status, "Wi-Fi: off");
        assert!(q.networks.is_empty());
        assert_eq!(q.icon, TrayIcon::WifiOff);
        snap.wwan_enabled = false;
        assert_eq!(build(Some(&snap)).icon, TrayIcon::Airplane);
        assert_eq!(build(None).icon, TrayIcon::Offline);
    }
}
