//! The connection editor's model: NetworkManager's settings map as form fields, checked, and
//! written back as the original map with only the edited keys changed, so settings the editor
//! doesn't show survive. Pure, so it's tested without NetworkManager or a window.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use vinculum_nm::Settings;
use zbus::zvariant::{OwnedValue, Value};

/// What kind of connection is edited, which decides the tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    Wifi,
    Ethernet,
    WireGuard,
    /// NetworkManager-openvpn.
    OpenVpn,
    Vlan,
    Bridge,
    Bond,
    /// Other VPN plugins and the rest: General and IP settings only.
    #[default]
    Other,
}

impl Kind {
    pub fn tabs(self) -> &'static [Tab] {
        match self {
            Self::Wifi => &[Tab::General, Tab::Wifi, Tab::Security, Tab::Ipv4, Tab::Ipv6],
            Self::Ethernet => &[Tab::General, Tab::Ethernet, Tab::Enterprise, Tab::Ipv4, Tab::Ipv6],
            Self::WireGuard => &[Tab::General, Tab::WireGuard, Tab::Peers, Tab::Ipv4, Tab::Ipv6],
            Self::OpenVpn => &[Tab::General, Tab::OpenVpn, Tab::Advanced, Tab::Ipv4, Tab::Ipv6],
            Self::Vlan => &[Tab::General, Tab::Vlan, Tab::Ipv4, Tab::Ipv6],
            Self::Bridge => &[Tab::General, Tab::Bridge, Tab::Ipv4, Tab::Ipv6],
            Self::Bond => &[Tab::General, Tab::Bond, Tab::Ipv4, Tab::Ipv6],
            Self::Other => &[Tab::General, Tab::Ipv4, Tab::Ipv6],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    General,
    Wifi,
    Security,
    Ipv4,
    Ipv6,
    Ethernet,
    /// 802.1X on a wired connection; on Wi-Fi it's part of Security.
    Enterprise,
    WireGuard,
    Peers,
    OpenVpn,
    Advanced,
    Vlan,
    Bridge,
    Bond,
}

impl Tab {
    pub fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Wifi => "wifi",
            Self::Security => "security",
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::Ethernet => "ethernet",
            Self::Enterprise => "8021x",
            Self::WireGuard => "wireguard",
            Self::Peers => "peers",
            Self::OpenVpn => "openvpn",
            Self::Advanced => "advanced",
            Self::Vlan => "vlan",
            Self::Bridge => "bridge",
            Self::Bond => "bond",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Wifi => "Wi-Fi",
            Self::Security => "Security",
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
            Self::Ethernet => "Ethernet",
            Self::Enterprise => "802.1X",
            Self::WireGuard => "WireGuard",
            Self::Peers => "Peers",
            Self::OpenVpn => "OpenVPN",
            Self::Advanced => "Advanced",
            Self::Vlan => "VLAN",
            Self::Bridge => "Bridge",
            Self::Bond => "Bond",
        }
    }
}

/// `802-11-wireless-security.key-mgmt` by the Security choice's index.
pub const SECURITIES: [(&str, &str); 7] = [
    ("None", "Open network"),
    ("Enhanced Open (OWE)", "Encrypted, no password"),
    ("WEP", "Outdated, avoid"),
    ("WPA/WPA2 Personal", "Password, and WPA3 where both sides support it"),
    ("WPA3 Personal", "Password, SAE only"),
    ("WPA/WPA2 Enterprise", "802.1X: PEAP, TTLS, TLS, PWD"),
    ("WPA3 Enterprise", "802.1X, 192-bit"),
];
const KEY_MGMT: [&str; 7] = ["", "owe", "none", "wpa-psk", "sae", "wpa-eap", "wpa-eap-suite-b-192"];

pub const IP4_METHODS: [(&str, &str); 5] = [
    ("auto", "Automatic (DHCP)"),
    ("manual", "Manual"),
    ("link-local", "Link-local"),
    ("shared", "Shared"),
    ("disabled", "Disabled"),
];
pub const IP6_METHODS: [(&str, &str); 6] = [
    ("auto", "Automatic"),
    ("dhcp", "DHCPv6 only"),
    ("manual", "Manual"),
    ("link-local", "Link-local"),
    ("ignore", "Ignore"),
    ("disabled", "Disabled"),
];
/// `ipv6.ip6-privacy` values and labels.
pub const PRIVACY: [(i32, &str); 4] =
    [(-1, "Default"), (0, "Off"), (1, "Prefer public addresses"), (2, "Prefer temporary addresses")];
/// `assigned-mac-address` values and labels; the last one is a fixed address typed in.
pub const MACS: [(&str, &str); 6] = [
    ("", "Default"),
    ("permanent", "Permanent"),
    ("stable", "Random, stable for this network"),
    ("random", "Random on every connect"),
    ("preserve", "Keep the current one"),
    ("fixed", "Fixed address"),
];
pub const WIFI_MODES: [&str; 3] = ["infrastructure", "ap", "adhoc"];
/// `802-1x.eap` choices.
pub const EAP_METHODS: [(&str, &str); 4] = [("peap", "PEAP"), ("ttls", "TTLS"), ("tls", "TLS"), ("pwd", "PWD")];
/// `802-1x.phase2-auth` choices, for PEAP and TTLS.
pub const PHASE2: [(&str, &str); 6] =
    [("mschapv2", "MSCHAPv2"), ("pap", "PAP"), ("chap", "CHAP"), ("mschap", "MSCHAP"), ("gtc", "GTC"), ("md5", "MD5")];
pub const BANDS: [&str; 3] = ["", "bg", "a"];

// NMSettingWiredWakeOnLan bits shown in the editor.
const WOL_UNICAST: u32 = 0x4;
const WOL_BROADCAST: u32 = 0x10;
const WOL_MAGIC: u32 = 0x40;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Address {
    pub address: String,
    pub prefix: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Route {
    pub dest: String,
    pub prefix: String,
    /// Empty: on-link.
    pub gateway: String,
    /// Empty: automatic.
    pub metric: String,
}

/// One family's IP settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IpForm {
    /// Index into IP4_METHODS or IP6_METHODS.
    pub method: usize,
    pub addresses: Vec<Address>,
    pub gateway: String,
    pub dns: Vec<String>,
    /// Comma or space separated.
    pub search: String,
    pub ignore_auto_dns: bool,
    pub routes: Vec<Route>,
    pub ignore_auto_routes: bool,
    /// Use the connection only for its own network (`never-default`).
    pub never_default: bool,
    /// Fail the connection if this family can't be configured (`may-fail` off).
    pub required: bool,
    /// IPv6 only: index into PRIVACY.
    pub privacy: usize,
    /// IPv6 only: stable privacy (true) or EUI-64.
    pub stable_privacy: bool,
}

/// 802.1X: Enterprise Wi-Fi, or a wired connection that signs in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EapForm {
    /// Wired only: whether it signs in at all.
    pub enabled: bool,
    /// Index into EAP_METHODS.
    pub method: usize,
    /// Index into PHASE2.
    pub phase2: usize,
    pub identity: String,
    pub anonymous: String,
    /// A new password; empty leaves the saved one.
    pub password: String,
    /// 0 for me only (keyring), 1 all users, 2 ask each time.
    pub storage: usize,
    pub system_ca: bool,
    /// File paths.
    pub ca_cert: String,
    pub client_cert: String,
    pub private_key: String,
    /// A new password for the private key; empty leaves the saved one.
    pub key_password: String,
    pub domain: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerForm {
    pub public_key: String,
    pub endpoint: String,
    pub allowed_ips: Vec<String>,
    /// Seconds; empty is off.
    pub keepalive: String,
    /// It has a pre-shared key (kept as it is: secrets aren't in the form).
    pub has_psk: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WgForm {
    pub interface: String,
    /// A new private key; empty leaves the saved one.
    pub private_key: String,
    /// The public key of the saved private key, when it's known.
    pub public_key: String,
    pub listen_port: String,
    pub fwmark: String,
    pub peer_routes: bool,
    pub peers: Vec<PeerForm>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Form {
    // General
    pub name: String,
    pub autoconnect: bool,
    pub priority: String,
    pub only_me: bool,
    /// 0 automatic, 1 yes, 2 no.
    pub metered: usize,
    pub zone: String,
    /// The UUID of the VPN started with this connection, or empty.
    pub vpn: String,
    // Wi-Fi
    pub ssid: String,
    pub mode: usize,
    pub band: usize,
    /// Empty: any access point.
    pub bssid: String,
    pub hidden: bool,
    // Security
    pub security: usize,
    /// A new password; empty leaves the saved one.
    pub password: String,
    /// 0 for me only (keyring), 1 all users, 2 ask each time.
    pub storage: usize,
    // Wi-Fi or Ethernet
    pub mac: usize,
    pub mac_fixed: String,
    pub mtu: String,
    // Ethernet
    pub interface: String,
    /// 0 automatic, 1 manual, 2 ignore.
    pub negotiation: usize,
    pub speed: String,
    pub full_duplex: bool,
    pub wol_magic: bool,
    pub wol_unicast: bool,
    pub wol_broadcast: bool,
    pub ip4: IpForm,
    pub ip6: IpForm,
    pub eap: EapForm,
    pub wg: WgForm,
    pub ovpn: crate::openvpn::OvpnForm,
    pub virt: crate::virt::VirtForm,
}

// Reading values out of the settings map.

fn get<'a>(s: &'a Settings, setting: &str, key: &str) -> Option<&'a Value<'static>> {
    s.get(setting)?.get(key).map(|v| &**v)
}
fn string(s: &Settings, setting: &str, key: &str) -> Option<String> {
    get(s, setting, key).and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned)
}
fn boolean(s: &Settings, setting: &str, key: &str) -> Option<bool> {
    get(s, setting, key).and_then(|v| bool::try_from(v).ok())
}
fn int(s: &Settings, setting: &str, key: &str) -> Option<i64> {
    let v = get(s, setting, key)?;
    i32::try_from(v).map(i64::from).or_else(|_| u32::try_from(v).map(i64::from)).ok()
}
fn strings(s: &Settings, setting: &str, key: &str) -> Vec<String> {
    match get(s, setting, key) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| <&str>::try_from(v).ok().map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}
fn bytes(s: &Settings, setting: &str, key: &str) -> Vec<u8> {
    match get(s, setting, key) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| u8::try_from(v).ok()).collect(),
        _ => Vec::new(),
    }
}
fn dicts(s: &Settings, setting: &str, key: &str) -> Vec<HashMap<String, Value<'static>>> {
    let Some(Value::Array(a)) = get(s, setting, key) else { return Vec::new() };
    a.iter()
        .filter_map(|d| {
            let Value::Dict(d) = d else { return None };
            let mut out = HashMap::new();
            for (k, v) in d.iter() {
                let v = match v {
                    Value::Value(inner) => inner.try_to_owned().ok()?.into(),
                    other => other.try_to_owned().ok()?.into(),
                };
                out.insert(<&str>::try_from(k).ok()?.to_owned(), v);
            }
            Some(out)
        })
        .collect()
}

fn mac_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn parse_mac(text: &str) -> Option<Vec<u8>> {
    let parts: Vec<&str> = text.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    parts.iter().map(|p| if p.len() == 2 { u8::from_str_radix(p, 16).ok() } else { None }).collect()
}

/// `assigned-mac-address`, or the older `cloned-mac-address` bytes.
fn read_mac(s: &Settings, setting: &str) -> (usize, String) {
    let assigned = string(s, setting, "assigned-mac-address").unwrap_or_default();
    let fixed = if assigned.is_empty() { mac_string(&bytes(s, setting, "cloned-mac-address")) } else { assigned };
    match MACS.iter().position(|(v, _)| *v == fixed && !fixed.is_empty()) {
        Some(i) => (i, String::new()),
        None if fixed.is_empty() => (0, String::new()),
        None => (MACS.len() - 1, fixed),
    }
}

/// IPv4 DNS servers are `au` in network byte order; IPv6 ones `aay`. Newer versions also have
/// `dns-data` as strings, which wins when it's there.
fn read_dns(s: &Settings, setting: &str) -> Vec<String> {
    let data = strings(s, setting, "dns-data");
    if !data.is_empty() {
        return data;
    }
    match get(s, setting, "dns") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::U32(n) => Some(Ipv4Addr::from(n.to_ne_bytes()).to_string()),
                Value::Array(b) => {
                    let b: Vec<u8> = b.iter().filter_map(|x| u8::try_from(x).ok()).collect();
                    <[u8; 16]>::try_from(b).ok().map(|b| Ipv6Addr::from(b).to_string())
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn read_ip(s: &Settings, setting: &str, methods: &[(&str, &str)]) -> IpForm {
    let method = string(s, setting, "method").unwrap_or_else(|| "auto".into());
    let addresses = dicts(s, setting, "address-data")
        .into_iter()
        .filter_map(|d| {
            let address = <&str>::try_from(d.get("address")?).ok()?.to_owned();
            let prefix = u32::try_from(d.get("prefix")?).ok()?.to_string();
            Some(Address { address, prefix })
        })
        .collect();
    let routes = dicts(s, setting, "route-data")
        .into_iter()
        .map(|d| {
            let text = |k: &str| d.get(k).and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned).unwrap_or_default();
            let num = |k: &str| d.get(k).and_then(|v| u32::try_from(v).ok()).map(|n| n.to_string()).unwrap_or_default();
            Route { dest: text("dest"), prefix: num("prefix"), gateway: text("next-hop"), metric: num("metric") }
        })
        .collect();
    let privacy = int(s, setting, "ip6-privacy").unwrap_or(-1);
    IpForm {
        method: methods.iter().position(|(m, _)| *m == method).unwrap_or(0),
        addresses,
        gateway: string(s, setting, "gateway").unwrap_or_default(),
        dns: read_dns(s, setting),
        search: strings(s, setting, "dns-search").join(", "),
        ignore_auto_dns: boolean(s, setting, "ignore-auto-dns").unwrap_or(false),
        routes,
        ignore_auto_routes: boolean(s, setting, "ignore-auto-routes").unwrap_or(false),
        never_default: boolean(s, setting, "never-default").unwrap_or(false),
        required: !boolean(s, setting, "may-fail").unwrap_or(true),
        privacy: PRIVACY.iter().position(|(v, _)| i64::from(*v) == privacy).unwrap_or(0),
        // 0 is EUI-64; 1 stable privacy, and the "default" modes are treated like it.
        stable_privacy: int(s, setting, "addr-gen-mode") != Some(0),
    }
}

/// The form for a saved connection's settings.
pub fn read(s: &Settings) -> (Kind, Form) {
    let kind = match string(s, "connection", "type").as_deref() {
        Some("802-11-wireless") => Kind::Wifi,
        Some("802-3-ethernet") => Kind::Ethernet,
        Some("wireguard") => Kind::WireGuard,
        Some("vpn") if crate::openvpn::is_openvpn(s) => Kind::OpenVpn,
        Some("vlan") => Kind::Vlan,
        Some("bridge") => Kind::Bridge,
        Some("bond") => Kind::Bond,
        _ => Kind::Other,
    };
    let hw = hw_setting(kind);
    let (mac, mac_fixed) = read_mac(s, hw);
    let key_mgmt = string(s, "802-11-wireless-security", "key-mgmt");
    let secret_flags = int(s, "802-11-wireless-security", "psk-flags")
        .or_else(|| int(s, "802-11-wireless-security", "wep-key-flags"))
        .unwrap_or(0);
    let wol = int(s, "802-3-ethernet", "wake-on-lan").unwrap_or(1) as u32;
    let auto_negotiate = boolean(s, "802-3-ethernet", "auto-negotiate").unwrap_or(false);
    let speed = int(s, "802-3-ethernet", "speed").unwrap_or(0);
    let form = Form {
        name: string(s, "connection", "id").unwrap_or_default(),
        autoconnect: boolean(s, "connection", "autoconnect").unwrap_or(true),
        priority: int(s, "connection", "autoconnect-priority").unwrap_or(0).to_string(),
        only_me: !strings(s, "connection", "permissions").is_empty(),
        metered: match int(s, "connection", "metered") {
            Some(1 | 3) => 1,
            Some(2 | 4) => 2,
            _ => 0,
        },
        zone: string(s, "connection", "zone").unwrap_or_default(),
        vpn: strings(s, "connection", "secondaries").into_iter().next().unwrap_or_default(),
        ssid: String::from_utf8_lossy(&bytes(s, "802-11-wireless", "ssid")).into_owned(),
        mode: WIFI_MODES.iter().position(|m| Some(*m) == string(s, "802-11-wireless", "mode").as_deref()).unwrap_or(0),
        band: BANDS.iter().position(|b| Some(*b) == string(s, "802-11-wireless", "band").as_deref()).unwrap_or(0),
        bssid: mac_string(&bytes(s, "802-11-wireless", "bssid")),
        hidden: boolean(s, "802-11-wireless", "hidden").unwrap_or(false),
        security: match key_mgmt.as_deref() {
            None => 0,
            Some(k) => KEY_MGMT.iter().position(|m| *m == k).unwrap_or(3),
        },
        password: String::new(),
        storage: if secret_flags & 1 != 0 {
            0
        } else if secret_flags & 2 != 0 {
            2
        } else {
            1
        },
        mac,
        mac_fixed,
        mtu: int(s, hw, "mtu").filter(|&m| m > 0).map(|m| m.to_string()).unwrap_or_default(),
        interface: string(s, "connection", "interface-name").unwrap_or_default(),
        negotiation: if auto_negotiate {
            0
        } else if speed > 0 {
            1
        } else {
            // Neither: NetworkManager leaves the link as it is.
            if s.get("802-3-ethernet").is_some_and(|e| e.contains_key("auto-negotiate")) { 2 } else { 0 }
        },
        speed: if speed > 0 { speed.to_string() } else { String::new() },
        full_duplex: string(s, "802-3-ethernet", "duplex").as_deref() != Some("half"),
        wol_magic: wol & WOL_MAGIC != 0,
        wol_unicast: wol & WOL_UNICAST != 0,
        wol_broadcast: wol & WOL_BROADCAST != 0,
        ip4: read_ip(s, "ipv4", &IP4_METHODS),
        ip6: read_ip(s, "ipv6", &IP6_METHODS),
        eap: read_eap(s),
        wg: read_wg(s),
        ovpn: if kind == Kind::OpenVpn { crate::openvpn::read(s) } else { Default::default() },
        virt: if kind.is_virtual() { crate::virt::read(s, |_| None) } else { Default::default() },
    };
    (kind, form)
}

/// The setting that has the MAC address and MTU.
fn hw_setting(kind: Kind) -> &'static str {
    match kind {
        Kind::Wifi => "802-11-wireless",
        Kind::WireGuard => "wireguard",
        Kind::Ethernet | Kind::OpenVpn | Kind::Other | Kind::Vlan | Kind::Bridge | Kind::Bond => "802-3-ethernet",
    }
}

impl Kind {
    pub fn is_virtual(self) -> bool {
        matches!(self, Self::Vlan | Self::Bridge | Self::Bond)
    }
}

/// A certificate setting (`ay`): a NUL-terminated `file://` URI, as NetworkManager keeps them.
fn read_cert(s: &Settings, key: &str) -> String {
    let bytes = bytes(s, "802-1x", key);
    let text = String::from_utf8_lossy(&bytes);
    text.trim_end_matches('\0').strip_prefix("file://").unwrap_or("").to_owned()
}

fn cert_value(path: &str) -> Value<'static> {
    let mut bytes = format!("file://{}", path.trim()).into_bytes();
    bytes.push(0);
    Value::from(bytes)
}

fn read_eap(s: &Settings) -> EapForm {
    let method = strings(s, "802-1x", "eap").into_iter().next().unwrap_or_else(|| "peap".into());
    let phase2 =
        string(s, "802-1x", "phase2-auth").or_else(|| string(s, "802-1x", "phase2-autheap")).unwrap_or_default();
    let flags = int(s, "802-1x", "password-flags").unwrap_or(0);
    EapForm {
        enabled: s.contains_key("802-1x"),
        method: EAP_METHODS.iter().position(|(m, _)| *m == method).unwrap_or(0),
        phase2: PHASE2.iter().position(|(m, _)| *m == phase2).unwrap_or(0),
        identity: string(s, "802-1x", "identity").unwrap_or_default(),
        anonymous: string(s, "802-1x", "anonymous-identity").unwrap_or_default(),
        password: String::new(),
        storage: if flags & 1 != 0 {
            0
        } else if flags & 2 != 0 {
            2
        } else {
            1
        },
        system_ca: boolean(s, "802-1x", "system-ca-certs").unwrap_or(false),
        ca_cert: read_cert(s, "ca-cert"),
        client_cert: read_cert(s, "client-cert"),
        private_key: read_cert(s, "private-key"),
        key_password: String::new(),
        domain: string(s, "802-1x", "domain-suffix-match").unwrap_or_default(),
    }
}

fn read_wg(s: &Settings) -> WgForm {
    let peers = dicts(s, "wireguard", "peers")
        .into_iter()
        .map(|d| {
            let text = |k: &str| d.get(k).and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned).unwrap_or_default();
            let allowed = match d.get("allowed-ips") {
                Some(Value::Array(a)) => a.iter().filter_map(|v| <&str>::try_from(v).ok().map(str::to_owned)).collect(),
                _ => Vec::new(),
            };
            let keepalive = d.get("persistent-keepalive").and_then(|v| u32::try_from(v).ok()).filter(|&k| k > 0);
            PeerForm {
                public_key: text("public-key"),
                endpoint: text("endpoint"),
                allowed_ips: allowed,
                keepalive: keepalive.map(|k| k.to_string()).unwrap_or_default(),
                has_psk: d.contains_key("preshared-key-flags") || d.contains_key("preshared-key"),
            }
        })
        .collect();
    let nonzero = |key: &str| int(s, "wireguard", key).filter(|&n| n > 0).map(|n| n.to_string()).unwrap_or_default();
    WgForm {
        interface: string(s, "connection", "interface-name").unwrap_or_default(),
        private_key: String::new(),
        public_key: String::new(),
        listen_port: nonzero("listen-port"),
        fwmark: nonzero("fwmark"),
        peer_routes: boolean(s, "wireguard", "peer-routes").unwrap_or(true),
        peers,
    }
}

/// A form for a new connection of a kind.
pub fn new_form(kind: Kind, name: &str) -> Form {
    let vpn = matches!(kind, Kind::WireGuard | Kind::OpenVpn);
    Form {
        name: name.to_owned(),
        priority: "0".into(),
        only_me: kind == Kind::Wifi || vpn,
        security: if kind == Kind::Wifi { 3 } else { 0 },
        full_duplex: true,
        wol_magic: false,
        // A VPN is started by hand unless asked otherwise.
        autoconnect: !vpn,
        ip4: IpForm {
            // WireGuard has no DHCP: its address is part of the setup.
            method: if kind == Kind::WireGuard { 1 } else { 0 },
            addresses: if kind == Kind::WireGuard {
                vec![Address { address: String::new(), prefix: "24".into() }]
            } else {
                Vec::new()
            },
            stable_privacy: true,
            ..IpForm::default()
        },
        // "ignore" for WireGuard until it has an IPv6 address.
        ip6: IpForm { method: if kind == Kind::WireGuard { 4 } else { 0 }, stable_privacy: true, ..IpForm::default() },
        wg: WgForm {
            interface: "wg0".into(),
            private_key: crate::wireguard::generate(),
            peer_routes: true,
            ..WgForm::default()
        },
        ovpn: crate::openvpn::OvpnForm { storage: 0, ..Default::default() },
        eap: EapForm { system_ca: true, ..EapForm::default() },
        ..Form::default()
    }
}

/// The base settings of a new connection, before the form is written into them.
pub fn new_settings(kind: Kind, uuid: &str) -> Settings {
    let kind_name = match kind {
        Kind::Wifi => "802-11-wireless",
        Kind::WireGuard => "wireguard",
        Kind::OpenVpn => "vpn",
        Kind::Vlan => "vlan",
        Kind::Bridge => "bridge",
        Kind::Bond => "bond",
        Kind::Ethernet | Kind::Other => "802-3-ethernet",
    };
    let mut s: Settings = HashMap::new();
    put(&mut s, "connection", "type", Value::from(kind_name));
    put(&mut s, "connection", "uuid", Value::from(uuid));
    match kind {
        Kind::Wifi => put(&mut s, "802-11-wireless", "mode", Value::from("infrastructure")),
        Kind::WireGuard => put(&mut s, "wireguard", "peer-routes", Value::from(true)),
        Kind::OpenVpn => put(&mut s, "vpn", "service-type", Value::from(crate::openvpn::SERVICE)),
        Kind::Ethernet | Kind::Other => {
            s.entry("802-3-ethernet".into()).or_default();
        }
        Kind::Vlan | Kind::Bridge | Kind::Bond => {
            s.entry(kind_name.into()).or_default();
        }
    }
    put(&mut s, "ipv4", "method", Value::from("auto"));
    put(&mut s, "ipv6", "method", Value::from("auto"));
    s
}

// Checking.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Errors {
    pub name: String,
    pub priority: String,
    pub ssid: String,
    pub bssid: String,
    pub password: String,
    pub mac: String,
    pub mtu: String,
    pub speed: String,
    pub ip4: IpErrors,
    pub ip6: IpErrors,
    pub eap_identity: String,
    pub eap_secret: String,
    pub eap_certs: String,
    pub wg_interface: String,
    pub wg_key: String,
    pub wg_port: String,
    pub peers: String,
    pub ovpn_user: String,
    pub ovpn_secret: String,
    pub ovpn_files: String,
    pub virt_interface: String,
    pub virt_settings: String,
    pub virt_monitor: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IpErrors {
    pub addresses: String,
    pub dns: String,
    pub routes: String,
}

impl IpErrors {
    fn count(&self) -> usize {
        [&self.addresses, &self.dns, &self.routes].iter().filter(|e| !e.is_empty()).count()
    }
}

impl Errors {
    /// The errors on each tab.
    pub fn by_tab(&self, kind: Kind) -> Vec<(Tab, usize)> {
        let n = |errors: &[&String]| errors.iter().filter(|e| !e.is_empty()).count();
        kind.tabs()
            .iter()
            .map(|&tab| {
                let count = match tab {
                    Tab::General => n(&[&self.name, &self.priority]),
                    Tab::Wifi => n(&[&self.ssid, &self.bssid, &self.mac, &self.mtu]),
                    Tab::Security => n(&[&self.password, &self.eap_identity, &self.eap_secret, &self.eap_certs]),
                    Tab::Ethernet => n(&[&self.mac, &self.mtu, &self.speed]),
                    Tab::Enterprise => n(&[&self.eap_identity, &self.eap_secret, &self.eap_certs]),
                    Tab::WireGuard => n(&[&self.wg_interface, &self.wg_key, &self.wg_port, &self.mtu]),
                    Tab::Peers => n(&[&self.peers]),
                    Tab::OpenVpn => n(&[&self.ovpn_user, &self.ovpn_secret, &self.ovpn_files]),
                    Tab::Advanced => 0,
                    Tab::Vlan => n(&[&self.virt_interface, &self.virt_settings, &self.mtu]),
                    Tab::Bridge => n(&[&self.virt_interface, &self.virt_settings]),
                    Tab::Bond => n(&[&self.virt_interface, &self.virt_monitor]),
                    Tab::Ipv4 => self.ip4.count(),
                    Tab::Ipv6 => self.ip6.count(),
                };
                (tab, count)
            })
            .collect()
    }

    pub fn total(&self, kind: Kind) -> usize {
        self.by_tab(kind).iter().map(|(_, n)| n).sum()
    }
}

/// What's wrong with the form. `initial` is the form as it was read (`None` for a new
/// connection), to know whether a password is needed.
pub fn check(kind: Kind, initial: Option<&Form>, f: &Form) -> Errors {
    let mut e = Errors::default();
    // A Wi-Fi connection without a name is named after its network.
    let fallback = match kind {
        Kind::Wifi => &f.ssid,
        Kind::WireGuard => &f.wg.interface,
        Kind::Vlan | Kind::Bridge | Kind::Bond => &f.virt.interface,
        _ => &f.name,
    };
    if f.name.trim().is_empty() && fallback.trim().is_empty() {
        e.name = "Give the connection a name.".into();
    }
    match f.priority.trim().parse::<i32>() {
        Ok(p) if (-999..=999).contains(&p) => {}
        _ => e.priority = "A whole number from -999 to 999.".into(),
    }
    if kind == Kind::Wifi {
        let len = f.ssid.len();
        if len == 0 || len > 32 {
            e.ssid = "A network name has 1 to 32 bytes.".into();
        }
        if !f.bssid.is_empty() && parse_mac(&f.bssid).is_none() {
            e.bssid = "Not a MAC address.".into();
        }
        e.password = check_password(initial, f);
    }
    if matches!(kind, Kind::Wifi | Kind::Ethernet) && f.mac == MACS.len() - 1 && parse_mac(&f.mac_fixed).is_none() {
        e.mac = "Type a MAC address, like 54:E1:AD:7C:20:9B.".into();
    }
    if matches!(kind, Kind::Wifi | Kind::Ethernet | Kind::WireGuard | Kind::Vlan)
        && !f.mtu.trim().is_empty()
        && !matches!(f.mtu.trim().parse::<u32>(), Ok(576..=9216))
    {
        e.mtu = "From 576 to 9216 bytes, or empty for automatic.".into();
    }
    if kind == Kind::Ethernet && f.negotiation == 1 && !matches!(f.speed.trim().parse::<u32>(), Ok(1..)) {
        e.speed = "Type the speed in Mb/s, such as 1000.".into();
    }
    e.ip4 = check_ip(&f.ip4, false);
    e.ip6 = check_ip(&f.ip6, true);
    if (kind == Kind::Wifi && f.security >= 5) || (kind == Kind::Ethernet && f.eap.enabled) {
        check_eap(kind, initial, f, &mut e);
    }
    if kind == Kind::WireGuard {
        check_wg(initial, &f.wg, &mut e);
    }
    if kind == Kind::OpenVpn {
        (e.ovpn_user, e.ovpn_secret, e.ovpn_files) = crate::openvpn::check(initial.map(|i| &i.ovpn), &f.ovpn);
    }
    if kind.is_virtual() {
        (e.virt_interface, e.virt_settings, e.virt_monitor) = crate::virt::check(kind, &f.virt);
    }
    e
}

fn check_eap(kind: Kind, initial: Option<&Form>, f: &Form, e: &mut Errors) {
    let x = &f.eap;
    if x.identity.trim().is_empty() {
        e.eap_identity = "Type your username (identity).".into();
    }
    let tls = EAP_METHODS[x.method].0 == "tls";
    // A password is needed unless one is saved already for the same way of signing in.
    let had = initial.is_some_and(|i| {
        let was_enterprise = if kind == Kind::Wifi { i.security >= 5 } else { i.eap.enabled };
        was_enterprise && i.eap.method == x.method
    });
    let secret = if tls { &x.key_password } else { &x.password };
    if secret.is_empty() && !had && x.storage != 2 && !tls {
        e.eap_secret = "Type the password.".into();
    }
    if tls && (x.client_cert.trim().is_empty() || x.private_key.trim().is_empty()) {
        e.eap_certs = "TLS needs your certificate and private key files.".into();
    }
}

fn check_wg(initial: Option<&Form>, w: &WgForm, e: &mut Errors) {
    let name = w.interface.trim();
    if name.is_empty() || name.len() > 15 || !name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        e.wg_interface = "1 to 15 letters, digits, - or _, such as wg0.".into();
    }
    if w.private_key.trim().is_empty() {
        if initial.is_none() {
            e.wg_key = "Generate a private key, or paste one.".into();
        }
    } else if !crate::wireguard::is_key(&w.private_key) {
        e.wg_key = "Not a WireGuard key (44 characters of base64).".into();
    }
    if !w.listen_port.trim().is_empty() && w.listen_port.trim().parse::<u16>().is_err() {
        e.wg_port = "A port from 1 to 65535, or empty for any.".into();
    }
    for (i, p) in w.peers.iter().enumerate() {
        let which = format!("Peer {}", i + 1);
        let problem = if !crate::wireguard::is_key(&p.public_key) {
            Some(format!("{which}: the public key isn't a WireGuard key."))
        } else if !p.endpoint.trim().is_empty()
            && !p.endpoint.trim().rsplit_once(':').is_some_and(|(h, port)| !h.is_empty() && port.parse::<u16>().is_ok())
        {
            Some(format!("{which}: the endpoint is host:port, such as vpn.example.org:51820."))
        } else if let Some(bad) = p.allowed_ips.iter().find(|a| {
            let (ip, prefix) = a.split_once('/').unwrap_or((a, ""));
            let ip: Option<IpAddr> = ip.parse().ok();
            let max = if ip.is_some_and(|ip| ip.is_ipv6()) { 128 } else { 32 };
            ip.is_none() || prefix.parse::<u32>().map_or(!prefix.is_empty(), |p| p > max)
        }) {
            Some(format!("{which}: {bad} isn't an address range like 10.8.0.0/24."))
        } else if !p.keepalive.trim().is_empty() && p.keepalive.trim().parse::<u16>().is_err() {
            Some(format!("{which}: keepalive is a number of seconds."))
        } else {
            None
        };
        if let Some(problem) = problem {
            e.peers = problem;
            break;
        }
    }
}

fn check_password(initial: Option<&Form>, f: &Form) -> String {
    let needs = matches!(f.security, 2..=4);
    let had_one = initial.is_some_and(|i| i.security == f.security && matches!(i.security, 2..=4));
    if !needs || (f.password.is_empty() && (had_one || f.storage == 2)) {
        return String::new();
    }
    let chars = f.password.chars().count();
    let ok = match f.security {
        2 => chars >= 1,
        3 => (8..=63).contains(&chars) || (chars == 64 && f.password.chars().all(|c| c.is_ascii_hexdigit())),
        _ => chars >= 1,
    };
    match (ok, f.password.is_empty()) {
        (_, true) => "Type the network's password.".into(),
        (false, false) if f.security == 3 => "WPA passwords have 8 to 63 characters.".into(),
        (false, false) => "Type the network's password.".into(),
        (true, _) => String::new(),
    }
}

fn parse_ip(text: &str, v6: bool) -> Option<IpAddr> {
    let ip: IpAddr = text.trim().parse().ok()?;
    (ip.is_ipv6() == v6).then_some(ip)
}

fn in_subnet(ip: Ipv4Addr, network: Ipv4Addr, prefix: u32) -> bool {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    u32::from(ip) & mask == u32::from(network) & mask
}

fn check_ip(ip: &IpForm, v6: bool) -> IpErrors {
    let mut e = IpErrors::default();
    let family = if v6 { "IPv6" } else { "IPv4" };
    let max_prefix = if v6 { 128 } else { 32 };
    let manual = if v6 { ip.method == 2 } else { ip.method == 1 };
    let mut networks = Vec::new();
    for a in &ip.addresses {
        let Some(address) = parse_ip(&a.address, v6) else {
            e.addresses = if a.address.is_empty() {
                "Type each address, or remove its row.".to_owned()
            } else {
                format!("{} isn't an {family} address.", a.address)
            };
            break;
        };
        match a.prefix.trim().parse::<u32>() {
            Ok(p) if p <= max_prefix => networks.push((address, p)),
            _ => {
                e.addresses = format!("The prefix of {} is from 0 to {max_prefix}.", a.address);
                break;
            }
        }
    }
    if e.addresses.is_empty() && manual && ip.addresses.is_empty() {
        e.addresses = "Manual needs at least one address.".into();
    }
    if e.addresses.is_empty() && !ip.gateway.trim().is_empty() {
        match parse_ip(&ip.gateway, v6) {
            None => e.addresses = format!("The gateway {} isn't an {family} address.", ip.gateway.trim()),
            Some(IpAddr::V4(gw)) if !networks.is_empty() => {
                let inside = networks.iter().any(|(a, p)| matches!(a, IpAddr::V4(a) if in_subnet(gw, *a, *p)));
                if !inside {
                    let (IpAddr::V4(a), p) = networks[0] else { unreachable!() };
                    let mask = if p == 0 { 0 } else { u32::MAX << (32 - p) };
                    let net = Ipv4Addr::from(u32::from(a) & mask);
                    e.addresses = format!("The gateway {gw} is outside {net}/{p}.");
                }
            }
            Some(_) => {}
        }
    }
    if let Some(bad) = ip.dns.iter().find(|d| parse_ip(d, v6).is_none()) {
        e.dns = format!("{bad} isn't an {family} address.");
    }
    for r in &ip.routes {
        if r.dest.trim().is_empty() {
            e.routes = "Type each route's destination.".into();
        } else if parse_ip(&r.dest, v6).is_none() {
            e.routes = format!("The destination {} isn't an {family} address.", r.dest);
        } else if !matches!(r.prefix.trim().parse::<u32>(), Ok(p) if p <= max_prefix) {
            e.routes = format!("The prefix of {} is from 0 to {max_prefix}.", r.dest);
        } else if !r.gateway.trim().is_empty() && parse_ip(&r.gateway, v6).is_none() {
            e.routes = format!("The gateway {} isn't an {family} address.", r.gateway);
        } else if !r.metric.trim().is_empty() && r.metric.trim().parse::<u32>().is_err() {
            e.routes = format!("The metric of {} is a whole number.", r.dest);
        }
        if !e.routes.is_empty() {
            break;
        }
    }
    e
}

// Writing.

fn put(s: &mut Settings, setting: &str, key: &str, value: Value<'_>) {
    let value = OwnedValue::try_from(value).expect("no file descriptors in settings");
    s.entry(setting.to_owned()).or_default().insert(key.to_owned(), value);
}

fn remove(s: &mut Settings, setting: &str, key: &str) {
    if let Some(group) = s.get_mut(setting) {
        group.remove(key);
    }
}

/// The settings to save: `original` with the fields that differ from `initial` (all of them,
/// for a new connection) written in. `owner` is the user for "only me".
pub fn write(kind: Kind, original: &Settings, initial: Option<&Form>, f: &Form, owner: &str) -> Settings {
    let mut s = original.clone();
    macro_rules! edited {
        ($($field:tt)+) => { initial.is_none_or(|i| i.$($field)+ != f.$($field)+) };
    }
    if edited!(name) {
        put(&mut s, "connection", "id", Value::from(f.name.trim()));
    }
    if edited!(autoconnect) {
        put(&mut s, "connection", "autoconnect", Value::from(f.autoconnect));
    }
    if edited!(priority) {
        put(&mut s, "connection", "autoconnect-priority", Value::from(f.priority.trim().parse::<i32>().unwrap_or(0)));
    }
    if edited!(only_me) {
        let permissions = if f.only_me { vec![format!("user:{owner}:")] } else { Vec::new() };
        put(&mut s, "connection", "permissions", Value::from(permissions));
    }
    if edited!(metered) {
        put(&mut s, "connection", "metered", Value::from(f.metered as i32));
    }
    if edited!(zone) {
        if f.zone.is_empty() {
            remove(&mut s, "connection", "zone")
        } else {
            put(&mut s, "connection", "zone", Value::from(f.zone.as_str()))
        }
    }
    if edited!(vpn) {
        let secondaries: Vec<String> = if f.vpn.is_empty() { Vec::new() } else { vec![f.vpn.clone()] };
        put(&mut s, "connection", "secondaries", Value::from(secondaries));
    }
    let hw = hw_setting(kind);
    if kind == Kind::Wifi {
        if edited!(ssid) {
            put(&mut s, hw, "ssid", Value::from(f.ssid.as_bytes().to_vec()));
        }
        if edited!(mode) {
            put(&mut s, hw, "mode", Value::from(WIFI_MODES[f.mode]));
        }
        if edited!(band) {
            if f.band == 0 {
                remove(&mut s, hw, "band");
                remove(&mut s, hw, "channel");
            } else {
                put(&mut s, hw, "band", Value::from(BANDS[f.band]));
            }
        }
        if edited!(bssid) {
            match parse_mac(&f.bssid) {
                Some(b) => put(&mut s, hw, "bssid", Value::from(b)),
                None => remove(&mut s, hw, "bssid"),
            }
        }
        if edited!(hidden) {
            put(&mut s, hw, "hidden", Value::from(f.hidden));
        }
        write_security(&mut s, initial, f);
    }
    if matches!(kind, Kind::Wifi | Kind::Ethernet) && (edited!(mac) || edited!(mac_fixed)) {
        {
            remove(&mut s, hw, "cloned-mac-address");
            let value =
                if f.mac == MACS.len() - 1 { f.mac_fixed.trim().to_uppercase() } else { MACS[f.mac].0.to_owned() };
            if value.is_empty() {
                remove(&mut s, hw, "assigned-mac-address")
            } else {
                put(&mut s, hw, "assigned-mac-address", Value::from(value))
            }
        }
    }
    if matches!(kind, Kind::Wifi | Kind::Ethernet | Kind::WireGuard | Kind::Vlan) && edited!(mtu) {
        put(&mut s, hw, "mtu", Value::from(f.mtu.trim().parse::<u32>().unwrap_or(0)));
    }
    if kind == Kind::Ethernet {
        if edited!(interface) {
            if f.interface.is_empty() {
                remove(&mut s, "connection", "interface-name")
            } else {
                put(&mut s, "connection", "interface-name", Value::from(f.interface.as_str()))
            }
        }
        if edited!(negotiation) || edited!(speed) || edited!(full_duplex) {
            match f.negotiation {
                0 => {
                    put(&mut s, hw, "auto-negotiate", Value::from(true));
                    put(&mut s, hw, "speed", Value::from(0u32));
                    remove(&mut s, hw, "duplex");
                }
                1 => {
                    put(&mut s, hw, "auto-negotiate", Value::from(false));
                    put(&mut s, hw, "speed", Value::from(f.speed.trim().parse::<u32>().unwrap_or(0)));
                    put(&mut s, hw, "duplex", Value::from(if f.full_duplex { "full" } else { "half" }));
                }
                _ => {
                    put(&mut s, hw, "auto-negotiate", Value::from(false));
                    put(&mut s, hw, "speed", Value::from(0u32));
                    remove(&mut s, hw, "duplex");
                }
            }
        }
        if edited!(wol_magic) || edited!(wol_unicast) || edited!(wol_broadcast) {
            // Keep the bits the editor doesn't show; drop "default" (1) now that it's set.
            let old = int(original, hw, "wake-on-lan").unwrap_or(1) as u32;
            let mut wol = old & !(WOL_MAGIC | WOL_UNICAST | WOL_BROADCAST | 0x1);
            for (on, bit) in [(f.wol_magic, WOL_MAGIC), (f.wol_unicast, WOL_UNICAST), (f.wol_broadcast, WOL_BROADCAST)]
            {
                if on {
                    wol |= bit;
                }
            }
            put(&mut s, hw, "wake-on-lan", Value::from(wol));
        }
    }
    if kind == Kind::Ethernet || (kind == Kind::Wifi && f.security >= 5) {
        write_eap(&mut s, kind, initial.map(|i| &i.eap), &f.eap);
    }
    if kind == Kind::WireGuard {
        write_wg(&mut s, initial.map(|i| &i.wg), &f.wg);
    }
    if kind == Kind::OpenVpn && initial.is_none_or(|i| i.ovpn != f.ovpn) {
        crate::openvpn::write(&mut s, initial.map(|i| &i.ovpn), &f.ovpn);
    }
    if kind.is_virtual() {
        crate::virt::write(&mut s, kind, initial.map(|i| &i.virt), &f.virt);
    }
    write_ip(&mut s, "ipv4", false, initial.map(|i| &i.ip4), &f.ip4, original);
    write_ip(&mut s, "ipv6", true, initial.map(|i| &i.ip6), &f.ip6, original);
    s
}

fn write_security(s: &mut Settings, initial: Option<&Form>, f: &Form) {
    const SEC: &str = "802-11-wireless-security";
    let security_changed = initial.is_none_or(|i| i.security != f.security);
    if f.security == 0 {
        if security_changed {
            s.remove(SEC);
            remove(s, "802-11-wireless", "security");
        }
        return;
    }
    if f.security >= 5 {
        if security_changed {
            s.remove(SEC);
            put(s, SEC, "key-mgmt", Value::from(KEY_MGMT[f.security]));
        }
        return;
    }
    if security_changed {
        // A different kind of security: the old one's keys would only confuse NetworkManager.
        s.remove(SEC);
        put(s, SEC, "key-mgmt", Value::from(KEY_MGMT[f.security]));
    }
    let key = if f.security == 2 { "wep-key0" } else { "psk" };
    let flags = match f.storage {
        0 => 1u32,
        2 => 2,
        _ => 0,
    };
    if f.security == 1 {
        return;
    }
    if security_changed || initial.is_some_and(|i| i.storage != f.storage) || !f.password.is_empty() {
        put(s, SEC, &format!("{}-flags", if key == "psk" { "psk" } else { "wep-key" }), Value::from(flags));
    }
    if !f.password.is_empty() && f.storage != 2 {
        if key == "wep-key0" {
            let chars = f.password.chars().count();
            let hex = f.password.chars().all(|c| c.is_ascii_hexdigit());
            let typed = matches!(chars, 5 | 13) || (matches!(chars, 10 | 26) && hex);
            put(s, SEC, "wep-key-type", Value::from(if typed { 1u32 } else { 2u32 }));
        }
        put(s, SEC, key, Value::from(f.password.as_str()));
    }
}

fn write_eap(s: &mut Settings, kind: Kind, initial: Option<&EapForm>, e: &EapForm) {
    const X: &str = "802-1x";
    // A wired connection without 802.1X has no such setting at all.
    if kind == Kind::Ethernet && !e.enabled {
        if initial.is_none_or(|i| i.enabled) {
            s.remove(X);
        }
        return;
    }
    let fresh = initial.is_none_or(|i| !i.enabled && kind == Kind::Ethernet) || !s.contains_key(X);
    macro_rules! edited {
        ($field:ident) => {
            fresh || initial.is_none_or(|i| i.$field != e.$field)
        };
    }
    let tls = EAP_METHODS[e.method].0 == "tls";
    if edited!(method) {
        put(s, X, "eap", Value::from(vec![EAP_METHODS[e.method].0.to_owned()]));
    }
    if edited!(phase2) || edited!(method) {
        if matches!(EAP_METHODS[e.method].0, "peap" | "ttls") {
            put(s, X, "phase2-auth", Value::from(PHASE2[e.phase2].0));
        } else {
            remove(s, X, "phase2-auth");
        }
    }
    let text = |s: &mut Settings, key: &str, value: &str| {
        if value.trim().is_empty() { remove(s, X, key) } else { put(s, X, key, Value::from(value.trim())) }
    };
    if edited!(identity) {
        text(s, "identity", &e.identity);
    }
    if edited!(anonymous) {
        text(s, "anonymous-identity", &e.anonymous);
    }
    if edited!(domain) {
        text(s, "domain-suffix-match", &e.domain);
    }
    if edited!(system_ca) {
        put(s, X, "system-ca-certs", Value::from(e.system_ca));
    }
    for (key, path, changed) in [
        ("ca-cert", &e.ca_cert, edited!(ca_cert)),
        ("client-cert", &e.client_cert, edited!(client_cert)),
        ("private-key", &e.private_key, edited!(private_key)),
    ] {
        if changed {
            if path.trim().is_empty() { remove(s, X, key) } else { put(s, X, key, cert_value(path)) }
        }
    }
    let flags = match e.storage {
        0 => 1u32,
        2 => 2,
        _ => 0,
    };
    let (secret, secret_key) = if tls { (&e.key_password, "private-key-password") } else { (&e.password, "password") };
    if edited!(storage) || !secret.is_empty() {
        put(s, X, &format!("{secret_key}-flags"), Value::from(flags));
    }
    if !secret.is_empty() && e.storage != 2 {
        put(s, X, secret_key, Value::from(secret.as_str()));
    }
}

fn write_wg(s: &mut Settings, initial: Option<&WgForm>, w: &WgForm) {
    macro_rules! edited {
        ($field:ident) => {
            initial.is_none_or(|i| i.$field != w.$field)
        };
    }
    if edited!(interface) {
        put(s, "connection", "interface-name", Value::from(w.interface.trim()));
    }
    if !w.private_key.trim().is_empty() {
        put(s, "wireguard", "private-key", Value::from(w.private_key.trim()));
        put(s, "wireguard", "private-key-flags", Value::from(0u32));
    }
    let number = |text: &str| {
        text.trim()
            .strip_prefix("0x")
            .map_or_else(|| text.trim().parse().ok(), |h| u32::from_str_radix(h, 16).ok())
            .unwrap_or(0u32)
    };
    if edited!(listen_port) {
        put(s, "wireguard", "listen-port", Value::from(number(&w.listen_port)));
    }
    if edited!(fwmark) {
        put(s, "wireguard", "fwmark", Value::from(number(&w.fwmark)));
    }
    if edited!(peer_routes) {
        put(s, "wireguard", "peer-routes", Value::from(w.peer_routes));
    }
    if edited!(peers) {
        let peers: Vec<crate::wireguard::Peer> = w
            .peers
            .iter()
            .map(|p| crate::wireguard::Peer {
                public_key: p.public_key.trim().to_owned(),
                preshared_key: String::new(),
                endpoint: p.endpoint.trim().to_owned(),
                allowed_ips: p.allowed_ips.clone(),
                keepalive: p.keepalive.trim().parse().unwrap_or(0),
            })
            .collect();
        put(s, "wireguard", "peers", crate::wireguard::peers_value(&peers));
    }
}

fn write_ip(s: &mut Settings, setting: &str, v6: bool, initial: Option<&IpForm>, ip: &IpForm, original: &Settings) {
    macro_rules! edited {
        ($field:ident) => {
            initial.is_none_or(|i| i.$field != ip.$field)
        };
    }
    let methods: &[(&str, &str)] = if v6 { &IP6_METHODS } else { &IP4_METHODS };
    if edited!(method) {
        put(s, setting, "method", Value::from(methods[ip.method].0));
    }
    if edited!(addresses) {
        remove(s, setting, "addresses");
        let data: Vec<HashMap<&str, Value>> = ip
            .addresses
            .iter()
            .map(|a| {
                HashMap::from([
                    ("address", Value::from(a.address.trim().to_owned())),
                    ("prefix", Value::from(a.prefix.trim().parse::<u32>().unwrap_or(if v6 { 64 } else { 24 }))),
                ])
            })
            .collect();
        put(s, setting, "address-data", Value::from(data));
    }
    if edited!(gateway) {
        if ip.gateway.trim().is_empty() {
            remove(s, setting, "gateway")
        } else {
            put(s, setting, "gateway", Value::from(ip.gateway.trim()))
        }
    }
    if edited!(dns) {
        let parsed: Vec<IpAddr> = ip.dns.iter().filter_map(|d| parse_ip(d, v6)).collect();
        if v6 {
            let dns: Vec<Vec<u8>> = parsed
                .iter()
                .filter_map(|a| if let IpAddr::V6(a) = a { Some(a.octets().to_vec()) } else { None })
                .collect();
            put(s, setting, "dns", Value::from(dns));
        } else {
            let dns: Vec<u32> = parsed
                .iter()
                .filter_map(|a| if let IpAddr::V4(a) = a { Some(u32::from_ne_bytes(a.octets())) } else { None })
                .collect();
            put(s, setting, "dns", Value::from(dns));
        }
        if original.get(setting).is_some_and(|g| g.contains_key("dns-data")) {
            put(s, setting, "dns-data", Value::from(parsed.iter().map(|a| a.to_string()).collect::<Vec<_>>()));
        }
    }
    if edited!(search) {
        let search: Vec<String> =
            ip.search.split([',', ' ']).map(str::trim).filter(|d| !d.is_empty()).map(str::to_owned).collect();
        put(s, setting, "dns-search", Value::from(search));
    }
    if edited!(ignore_auto_dns) {
        put(s, setting, "ignore-auto-dns", Value::from(ip.ignore_auto_dns));
    }
    if edited!(routes) {
        remove(s, setting, "routes");
        let data: Vec<HashMap<&str, Value>> = ip
            .routes
            .iter()
            .map(|r| {
                let mut d = HashMap::from([
                    ("dest", Value::from(r.dest.trim().to_owned())),
                    ("prefix", Value::from(r.prefix.trim().parse::<u32>().unwrap_or(0))),
                ]);
                if !r.gateway.trim().is_empty() {
                    d.insert("next-hop", Value::from(r.gateway.trim().to_owned()));
                }
                if let Ok(metric) = r.metric.trim().parse::<u32>() {
                    d.insert("metric", Value::from(metric));
                }
                d
            })
            .collect();
        put(s, setting, "route-data", Value::from(data));
    }
    if edited!(ignore_auto_routes) {
        put(s, setting, "ignore-auto-routes", Value::from(ip.ignore_auto_routes));
    }
    if edited!(never_default) {
        put(s, setting, "never-default", Value::from(ip.never_default));
    }
    if edited!(required) {
        put(s, setting, "may-fail", Value::from(!ip.required));
    }
    if v6 {
        if edited!(privacy) {
            put(s, setting, "ip6-privacy", Value::from(PRIVACY[ip.privacy].0));
        }
        if edited!(stable_privacy) {
            put(s, setting, "addr-gen-mode", Value::from(if ip.stable_privacy { 1i32 } else { 0i32 }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(groups: &[(&str, Vec<(&str, Value<'static>)>)]) -> Settings {
        groups
            .iter()
            .map(|(g, kv)| {
                let values =
                    kv.iter().map(|(k, v)| ((*k).to_owned(), OwnedValue::try_from(v.clone()).unwrap())).collect();
                ((*g).to_owned(), values)
            })
            .collect()
    }

    fn home() -> Settings {
        settings(&[
            (
                "connection",
                vec![
                    ("id", "Lumen Home".into()),
                    ("uuid", "7b1c0e52".into()),
                    ("type", "802-11-wireless".into()),
                    ("autoconnect-priority", 0i32.into()),
                    // Something the editor doesn't know about.
                    ("lldp", 1i32.into()),
                ],
            ),
            ("802-11-wireless", vec![("ssid", Value::from(b"Lumen Home".to_vec())), ("mode", "infrastructure".into())]),
            ("802-11-wireless-security", vec![("key-mgmt", "wpa-psk".into()), ("psk-flags", 1u32.into())]),
            (
                "ipv4",
                vec![
                    ("method", "auto".into()),
                    ("dns", Value::from(vec![u32::from_ne_bytes([9, 9, 9, 9]), u32::from_ne_bytes([192, 168, 1, 1])])),
                ],
            ),
            ("ipv6", vec![("method", "auto".into()), ("addr-gen-mode", 1i32.into())]),
            ("proxy", vec![("method", 0i32.into())]),
        ])
    }

    #[test]
    fn reads_a_wifi_connection() {
        let (kind, f) = read(&home());
        assert_eq!(kind, Kind::Wifi);
        assert_eq!((f.name.as_str(), f.ssid.as_str(), f.priority.as_str()), ("Lumen Home", "Lumen Home", "0"));
        assert!(f.autoconnect && !f.only_me);
        assert_eq!((f.security, f.storage), (3, 0));
        assert_eq!(f.ip4.dns, ["9.9.9.9", "192.168.1.1"]);
        assert!(f.ip6.stable_privacy);
        assert_eq!(kind.tabs().len(), 5);
    }

    #[test]
    fn unchanged_form_writes_the_original_back() {
        let original = home();
        let (kind, f) = read(&original);
        assert_eq!(write(kind, &original, Some(&f), &f, "giosuel"), original);
    }

    #[test]
    fn only_edited_keys_change_and_unknown_ones_survive() {
        let original = home();
        let (kind, initial) = read(&original);
        let mut f = initial.clone();
        f.priority = "20".into();
        f.metered = 1;
        f.ip4.method = 1;
        f.ip4.addresses = vec![Address { address: "192.168.1.42".into(), prefix: "24".into() }];
        f.ip4.gateway = "192.168.1.1".into();
        f.ip4.dns = vec!["1.1.1.1".into()];
        let s = write(kind, &original, Some(&initial), &f, "giosuel");
        assert_eq!(i32::try_from(&s["connection"]["autoconnect-priority"]).unwrap(), 20);
        assert_eq!(i32::try_from(&s["connection"]["metered"]).unwrap(), 1);
        assert_eq!(<&str>::try_from(&s["ipv4"]["method"]).unwrap(), "manual");
        assert_eq!(<&str>::try_from(&s["ipv4"]["gateway"]).unwrap(), "192.168.1.1");
        let dns = <Vec<u32>>::try_from(s["ipv4"]["dns"].try_clone().unwrap()).unwrap();
        assert_eq!(dns, [u32::from_ne_bytes([1, 1, 1, 1])]);
        // Untouched: the unknown key, the proxy setting, the security.
        assert_eq!(s["connection"]["lldp"], original["connection"]["lldp"]);
        assert_eq!(s["proxy"], original["proxy"]);
        assert_eq!(s["802-11-wireless-security"], original["802-11-wireless-security"]);
        // Read back, it's the edited form.
        assert_eq!(read(&s).1, f);
    }

    #[test]
    fn the_gateway_must_be_on_an_address_network() {
        let (kind, mut f) = read(&home());
        f.ip4.method = 1;
        f.ip4.addresses = vec![Address { address: "192.168.1.42".into(), prefix: "24".into() }];
        f.ip4.gateway = "192.168.2.1".into();
        let e = check(kind, None, &f);
        assert_eq!(e.ip4.addresses, "The gateway 192.168.2.1 is outside 192.168.1.0/24.");
        assert_eq!(e.by_tab(kind).iter().find(|(t, _)| *t == Tab::Ipv4).unwrap().1, 1);
        f.ip4.gateway = "192.168.1.1".into();
        f.ip4.dns = vec!["9.9.9.9".into(), "2620:fe::fe".into()];
        let e = check(kind, None, &f);
        assert_eq!(e.ip4.addresses, "");
        assert_eq!(e.ip4.dns, "2620:fe::fe isn't an IPv4 address.");
    }

    #[test]
    fn passwords_are_needed_only_when_there_is_none_yet() {
        let original = home();
        let (kind, initial) = read(&original);
        assert_eq!(check(kind, Some(&initial), &initial).total(kind), 0, "the saved one stays");
        let mut f = initial.clone();
        f.security = 4;
        assert_eq!(check(kind, Some(&initial), &f).password, "Type the network's password.");
        f.password = "x".into();
        assert_eq!(check(kind, Some(&initial), &f).total(kind), 0);
        f.security = 3;
        f.password = "short".into();
        assert_eq!(check(kind, Some(&initial), &f).password, "WPA passwords have 8 to 63 characters.");
    }

    #[test]
    fn changing_the_security_replaces_the_old_keys() {
        let original = home();
        let (kind, initial) = read(&original);
        let mut f = initial.clone();
        f.security = 4;
        f.password = "new wpa3 pass".into();
        f.storage = 1;
        let s = write(kind, &original, Some(&initial), &f, "giosuel");
        let sec = &s["802-11-wireless-security"];
        assert_eq!(<&str>::try_from(&sec["key-mgmt"]).unwrap(), "sae");
        assert_eq!(<&str>::try_from(&sec["psk"]).unwrap(), "new wpa3 pass");
        assert_eq!(u32::try_from(&sec["psk-flags"]).unwrap(), 0);
        f.security = 0;
        let s = write(kind, &original, Some(&initial), &f, "giosuel");
        assert!(!s.contains_key("802-11-wireless-security"));
    }

    #[test]
    fn a_new_wired_connection() {
        let f = Form { name: "Office dock".into(), interface: "enp3s0".into(), ..new_form(Kind::Ethernet, "") };
        let s = write(Kind::Ethernet, &new_settings(Kind::Ethernet, "u-1"), None, &f, "giosuel");
        assert_eq!(<&str>::try_from(&s["connection"]["id"]).unwrap(), "Office dock");
        assert_eq!(<&str>::try_from(&s["connection"]["type"]).unwrap(), "802-3-ethernet");
        assert_eq!(<&str>::try_from(&s["connection"]["interface-name"]).unwrap(), "enp3s0");
        assert!(bool::try_from(&s["802-3-ethernet"]["auto-negotiate"]).unwrap());
        assert_eq!(<&str>::try_from(&s["ipv6"]["method"]).unwrap(), "auto");
        assert!(<Vec<String>>::try_from(s["connection"]["permissions"].try_clone().unwrap()).unwrap().is_empty());
    }

    #[test]
    fn mac_addresses_and_wake_on_lan() {
        let mut original = home();
        put(
            &mut original,
            "802-11-wireless",
            "cloned-mac-address",
            Value::from(vec![0x54u8, 0xe1, 0xad, 0x7c, 0x20, 0x9b]),
        );
        let (kind, initial) = read(&original);
        assert_eq!((initial.mac, initial.mac_fixed.as_str()), (5, "54:E1:AD:7C:20:9B"));
        let mut f = initial.clone();
        f.mac = 2;
        let s = write(kind, &original, Some(&initial), &f, "u");
        assert_eq!(<&str>::try_from(&s["802-11-wireless"]["assigned-mac-address"]).unwrap(), "stable");
        assert!(!s["802-11-wireless"].contains_key("cloned-mac-address"));
        f.mac = 5;
        f.mac_fixed = "nope".into();
        assert!(!check(kind, Some(&initial), &f).mac.is_empty());

        let wired = settings(&[
            ("connection", vec![("type", "802-3-ethernet".into()), ("id", "w".into())]),
            ("802-3-ethernet", vec![("wake-on-lan", (0x40u32 | 0x20).into())]),
        ]);
        let (kind, initial) = read(&wired);
        assert!(initial.wol_magic && !initial.wol_unicast);
        let mut f = initial.clone();
        f.wol_broadcast = true;
        let s = write(kind, &wired, Some(&initial), &f, "u");
        assert_eq!(u32::try_from(&s["802-3-ethernet"]["wake-on-lan"]).unwrap(), 0x40 | 0x20 | 0x10, "ARP (0x20) kept");
    }

    #[test]
    fn ipv6_dns_routes_and_privacy() {
        let original = home();
        let (kind, initial) = read(&original);
        let mut f = initial.clone();
        f.ip6.dns = vec!["2620:fe::fe".into()];
        f.ip6.privacy = 3;
        f.ip6.routes =
            vec![Route { dest: "fd00::".into(), prefix: "8".into(), gateway: String::new(), metric: "100".into() }];
        let s = write(kind, &original, Some(&initial), &f, "u");
        let dns = <Vec<Vec<u8>>>::try_from(s["ipv6"]["dns"].try_clone().unwrap()).unwrap();
        assert_eq!(dns[0], "2620:fe::fe".parse::<Ipv6Addr>().unwrap().octets());
        assert_eq!(i32::try_from(&s["ipv6"]["ip6-privacy"]).unwrap(), 2);
        assert_eq!(read(&s).1.ip6, f.ip6);
        f.ip6.routes[0].prefix = "200".into();
        assert_eq!(check(kind, None, &f).ip6.routes, "The prefix of fd00:: is from 0 to 128.");
    }
}
