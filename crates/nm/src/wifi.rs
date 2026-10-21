//! Wi-Fi details derived from access point properties: security type, band and channel, signal
//! bars, and the grouping of access points into the networks a user picks from.

use std::collections::BTreeMap;

use crate::model::{AccessPoint, SavedConnection, Snapshot};

// NM80211ApFlags and NM80211ApSecurityFlags (nm-dbus-types).
const AP_FLAGS_PRIVACY: u32 = 0x1;
const SEC_KEY_MGMT_PSK: u32 = 0x100;
const SEC_KEY_MGMT_802_1X: u32 = 0x200;
const SEC_KEY_MGMT_SAE: u32 = 0x400;
const SEC_KEY_MGMT_OWE: u32 = 0x800;
const SEC_KEY_MGMT_OWE_TM: u32 = 0x1000;
const SEC_KEY_MGMT_EAP_SUITE_B_192: u32 = 0x2000;

/// How a network is secured, as far as its beacons tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Security {
    Open,
    /// Enhanced Open: encrypted, no password.
    Owe,
    Wep,
    Wpa,
    Wpa2,
    /// WPA3 where the client supports it, WPA2 otherwise (transition mode).
    Wpa2Wpa3,
    Wpa3,
    Enterprise,
    Wpa3Enterprise,
}

impl Security {
    /// From an access point's `Flags`, `WpaFlags` and `RsnFlags`.
    pub fn from_flags(flags: u32, wpa: u32, rsn: u32) -> Self {
        let any = wpa | rsn;
        if any & SEC_KEY_MGMT_802_1X != 0 {
            if any & SEC_KEY_MGMT_EAP_SUITE_B_192 != 0 { Self::Wpa3Enterprise } else { Self::Enterprise }
        } else if rsn & SEC_KEY_MGMT_SAE != 0 {
            if rsn & SEC_KEY_MGMT_PSK != 0 { Self::Wpa2Wpa3 } else { Self::Wpa3 }
        } else if rsn & SEC_KEY_MGMT_PSK != 0 {
            Self::Wpa2
        } else if wpa & SEC_KEY_MGMT_PSK != 0 {
            Self::Wpa
        } else if rsn & SEC_KEY_MGMT_OWE != 0 && rsn & SEC_KEY_MGMT_OWE_TM == 0 {
            Self::Owe
        } else if flags & AP_FLAGS_PRIVACY != 0 {
            Self::Wep
        } else {
            Self::Open
        }
    }

    /// Whether connecting asks for something (a password, or 802.1X credentials).
    pub fn needs_secrets(self) -> bool {
        !matches!(self, Self::Open | Self::Owe)
    }

    /// The short name the UI shows, e.g. "WPA2/WPA3".
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Owe => "Enhanced Open",
            Self::Wep => "WEP",
            Self::Wpa => "WPA",
            Self::Wpa2 => "WPA2",
            Self::Wpa2Wpa3 => "WPA2/WPA3",
            Self::Wpa3 => "WPA3",
            Self::Enterprise => "Enterprise",
            Self::Wpa3Enterprise => "WPA3 Enterprise",
        }
    }
}

/// A Wi-Fi frequency band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Band {
    Ghz2_4,
    Ghz5,
    Ghz6,
    Ghz60,
}

impl Band {
    /// The band of a frequency in MHz, if it's a Wi-Fi one.
    pub fn from_mhz(mhz: u32) -> Option<Self> {
        match mhz {
            2400..=2500 => Some(Self::Ghz2_4),
            4900..=5899 => Some(Self::Ghz5),
            5925..=7125 => Some(Self::Ghz6),
            57000..=71000 => Some(Self::Ghz60),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ghz2_4 => "2.4 GHz",
            Self::Ghz5 => "5 GHz",
            Self::Ghz6 => "6 GHz",
            Self::Ghz60 => "60 GHz",
        }
    }
}

/// The channel number of a frequency in MHz (IEEE 802.11 numbering per band).
pub fn channel(mhz: u32) -> Option<u32> {
    match Band::from_mhz(mhz)? {
        Band::Ghz2_4 if mhz == 2484 => Some(14),
        Band::Ghz2_4 => mhz.checked_sub(2407).map(|d| d / 5),
        Band::Ghz5 => mhz.checked_sub(5000).map(|d| d / 5),
        Band::Ghz6 => mhz.checked_sub(5950).map(|d| d / 5),
        Band::Ghz60 => mhz.checked_sub(56160).map(|d| d / 2160),
    }
}

/// Signal strength (0–100 %) as 0–4 bars.
pub fn bars(strength: u8) -> u8 {
    match strength {
        0 => 0,
        1..=24 => 1,
        25..=49 => 2,
        50..=74 => 3,
        _ => 4,
    }
}

/// One network as the user sees it: every access point with the same SSID and security.
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub ssid: Vec<u8>,
    pub security: Security,
    /// The access points, strongest first.
    pub access_points: Vec<AccessPoint>,
    /// Every band any of the access points is on, ascending.
    pub bands: Vec<Band>,
    /// A saved connection for this network, if there is one.
    pub saved: Option<String>,
    /// Whether the device is connected to it now.
    pub active: bool,
}

impl Network {
    /// The SSID for display: invalid UTF-8 becomes U+FFFD.
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.ssid).into_owned()
    }

    pub fn strongest(&self) -> &AccessPoint {
        &self.access_points[0]
    }

    pub fn strength(&self) -> u8 {
        self.strongest().strength
    }
}

/// Groups a Wi-Fi device's access points into networks, as the network list shows them: the
/// connected one first, then saved ones, then by signal strength. Hidden networks (empty SSID)
/// are left out; they're joined by name instead.
pub fn networks(access_points: &[AccessPoint], active_ap: Option<&str>, saved: &[SavedConnection]) -> Vec<Network> {
    let mut groups: BTreeMap<(Vec<u8>, Security), Vec<AccessPoint>> = BTreeMap::new();
    for ap in access_points.iter().filter(|ap| !ap.ssid.is_empty()) {
        groups.entry((ap.ssid.clone(), ap.security)).or_default().push(ap.clone());
    }
    let mut networks: Vec<Network> = groups
        .into_iter()
        .map(|((ssid, security), mut aps)| {
            aps.sort_by(|a, b| b.strength.cmp(&a.strength).then_with(|| a.bssid.cmp(&b.bssid)));
            let mut bands: Vec<Band> = aps.iter().filter_map(|ap| Band::from_mhz(ap.frequency)).collect();
            bands.sort();
            bands.dedup();
            let active = active_ap.is_some_and(|path| aps.iter().any(|ap| ap.path == path));
            let saved = saved.iter().find(|c| c.wifi_ssid() == Some(&ssid[..])).map(|c| c.uuid.clone());
            Network { ssid, security, access_points: aps, bands, saved, active }
        })
        .collect();
    networks.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| b.saved.is_some().cmp(&a.saved.is_some()))
            .then_with(|| b.strength().cmp(&a.strength()))
            .then_with(|| a.ssid.cmp(&b.ssid))
    });
    networks
}

impl Snapshot {
    /// The networks seen by every Wi-Fi device, grouped as [`networks`] does, per device path.
    pub fn networks(&self) -> Vec<(String, Vec<Network>)> {
        self.devices
            .iter()
            .filter_map(|d| {
                let wifi = d.wifi.as_ref()?;
                // The device's active access point, or else the one its active connection is on.
                let active_ap = wifi.active_access_point.as_deref().or_else(|| {
                    let active = self.active_connection(d.active_connection.as_deref()?)?;
                    active.specific_object.as_deref()
                });
                Some((d.path.clone(), networks(&wifi.access_points, active_ap, &self.connections)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConnectionKind;

    fn ap(path: &str, ssid: &str, bssid: &str, mhz: u32, strength: u8, security: Security) -> AccessPoint {
        AccessPoint {
            path: path.into(),
            ssid: ssid.as_bytes().to_vec(),
            bssid: bssid.into(),
            frequency: mhz,
            strength,
            security,
            max_bitrate: 0,
        }
    }

    fn saved_wifi(uuid: &str, ssid: &str) -> SavedConnection {
        SavedConnection {
            path: format!("/c/{uuid}"),
            uuid: uuid.into(),
            id: ssid.into(),
            kind: ConnectionKind::Wifi { ssid: ssid.as_bytes().to_vec() },
            autoconnect: true,
            autoconnect_priority: 0,
            interface: None,
            metered: None,
            hotspot: false,
        }
    }

    #[test]
    fn security_from_beacon_flags() {
        let (psk, sae, x1, b192, owe) = (0x100, 0x400, 0x200, 0x2000, 0x800);
        assert_eq!(Security::from_flags(0, 0, 0), Security::Open);
        assert_eq!(Security::from_flags(1, 0, 0), Security::Wep);
        assert_eq!(Security::from_flags(1, psk, 0), Security::Wpa);
        assert_eq!(Security::from_flags(1, psk, psk), Security::Wpa2);
        assert_eq!(Security::from_flags(1, 0, psk | sae), Security::Wpa2Wpa3);
        assert_eq!(Security::from_flags(1, 0, sae), Security::Wpa3);
        assert_eq!(Security::from_flags(1, 0, x1), Security::Enterprise);
        assert_eq!(Security::from_flags(1, 0, x1 | b192), Security::Wpa3Enterprise);
        assert_eq!(Security::from_flags(1, 0, owe), Security::Owe);
        // An open network advertising OWE transition mode is still open to connect to.
        assert_eq!(Security::from_flags(0, 0, owe | 0x1000), Security::Open);
        assert!(!Security::Owe.needs_secrets());
        assert!(Security::Wpa2.needs_secrets());
    }

    #[test]
    fn bands_and_channels() {
        assert_eq!(Band::from_mhz(2412), Some(Band::Ghz2_4));
        assert_eq!(channel(2412), Some(1));
        assert_eq!(channel(2484), Some(14));
        assert_eq!(Band::from_mhz(5180), Some(Band::Ghz5));
        assert_eq!(channel(5180), Some(36));
        assert_eq!(Band::from_mhz(5955), Some(Band::Ghz6));
        assert_eq!(channel(5955), Some(1));
        assert_eq!(Band::from_mhz(1000), None);
        assert_eq!(channel(1000), None);
    }

    #[test]
    fn signal_bars() {
        assert_eq!([0, 10, 30, 60, 90].map(bars), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn access_points_group_into_networks() {
        let aps = [
            ap("/ap/1", "Home", "AA", 2437, 40, Security::Wpa2),
            ap("/ap/2", "Home", "BB", 5180, 80, Security::Wpa2),
            ap("/ap/3", "Cafe", "CC", 2412, 90, Security::Open),
            ap("/ap/4", "Office", "DD", 5200, 30, Security::Enterprise),
            ap("/ap/5", "", "EE", 2412, 99, Security::Wpa2),
        ];
        let saved = [saved_wifi("u-office", "Office")];
        let nets = networks(&aps, Some("/ap/1"), &saved);
        let names: Vec<String> = nets.iter().map(Network::name).collect();
        // Connected first, then saved, then by strength; the hidden one is left out.
        assert_eq!(names, ["Home", "Office", "Cafe"]);
        let home = &nets[0];
        assert!(home.active);
        assert_eq!(home.strength(), 80);
        assert_eq!(home.strongest().bssid, "BB");
        assert_eq!(home.bands, [Band::Ghz2_4, Band::Ghz5]);
        assert_eq!(nets[1].saved.as_deref(), Some("u-office"));
        assert_eq!(nets[2].saved, None);
    }

    #[test]
    fn same_name_different_security_are_different_networks() {
        let aps =
            [ap("/ap/1", "Lab", "AA", 2412, 50, Security::Open), ap("/ap/2", "Lab", "BB", 2412, 60, Security::Wpa2)];
        assert_eq!(networks(&aps, None, &[]).len(), 2);
    }
}
