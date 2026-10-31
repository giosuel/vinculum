//! VLANs, bridges and bonds: their editor fields, and their ports. A port is a connection of its
//! own (an Ethernet profile with `connection.master` and `slave-type`), the way nmcli makes
//! them, so the bridge or bond brings its ports up with it.

use std::collections::HashMap;

use lituus_nm::{DeviceKind, Settings, Snapshot};
use zbus::zvariant::{OwnedValue, Value};

/// `bond.options` mode, by index: (value, label).
pub const BOND_MODES: [(&str, &str); 7] = [
    ("balance-rr", "Round robin"),
    ("active-backup", "Active backup (one port at a time)"),
    ("balance-xor", "XOR"),
    ("broadcast", "Broadcast"),
    ("802.3ad", "802.3ad (LACP)"),
    ("balance-tlb", "Adaptive transmit load balancing"),
    ("balance-alb", "Adaptive load balancing"),
];
pub const LACP_RATES: [(&str, &str); 2] = [("slow", "Slow (every 30 s)"), ("fast", "Fast (every second)")];
pub const HASH_POLICIES: [(&str, &str); 5] = [
    ("layer2", "Layer 2 (MAC addresses)"),
    ("layer2+3", "Layer 2+3 (MAC and IP addresses)"),
    ("layer3+4", "Layer 3+4 (IP addresses and ports)"),
    ("encap2+3", "Encapsulated 2+3"),
    ("encap3+4", "Encapsulated 3+4"),
];

// `vlan.flags` bits.
const REORDER_HEADERS: u32 = 1;
const GVRP: u32 = 2;
const LOOSE_BINDING: u32 = 4;
const MVRP: u32 = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtForm {
    /// The interface it makes: "br0", "bond0", "enp3s0.20".
    pub interface: String,
    // VLAN
    /// The parent interface's name.
    pub parent: String,
    pub vlan_id: String,
    pub reorder_headers: bool,
    pub gvrp: bool,
    pub mvrp: bool,
    pub loose_binding: bool,
    // Bridge
    pub stp: bool,
    pub stp_priority: String,
    pub forward_delay: String,
    pub hello_time: String,
    pub max_age: String,
    pub ageing_time: String,
    pub vlan_filtering: bool,
    // Bond
    /// Index into BOND_MODES.
    pub mode: usize,
    /// The port used while its link is up (active-backup), or empty.
    pub primary: String,
    /// Link monitoring by ARP instead of MII (carrier).
    pub arp: bool,
    pub miimon: String,
    pub arp_interval: String,
    /// Comma-separated IPv4 addresses.
    pub arp_targets: String,
    pub lacp_rate: usize,
    pub hash: usize,
    // Bridge or bond
    pub ports: Vec<PortRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortRow {
    pub interface: String,
    /// "Ethernet · 1000 Mb/s".
    pub meta: String,
    /// The port's connection; `None` for one added since the editor opened.
    pub connection: Option<String>,
}

fn get<'a>(s: &'a Settings, setting: &str, key: &str) -> Option<&'a Value<'static>> {
    s.get(setting)?.get(key).map(|v| &**v)
}
fn string(s: &Settings, setting: &str, key: &str) -> String {
    get(s, setting, key).and_then(|v| <&str>::try_from(v).ok()).unwrap_or_default().to_owned()
}
fn number(s: &Settings, setting: &str, key: &str) -> Option<u32> {
    get(s, setting, key).and_then(|v| u32::try_from(v).ok())
}
fn bond_options(s: &Settings) -> HashMap<String, String> {
    get(s, "bond", "options").and_then(|v| <HashMap<String, String>>::try_from(v.clone()).ok()).unwrap_or_default()
}

/// The fields of a VLAN, bridge or bond connection. `parent_name` turns a parent given by
/// connection UUID into its interface name.
pub fn read(s: &Settings, parent_name: impl Fn(&str) -> Option<String>) -> VirtForm {
    let flags = number(s, "vlan", "flags").unwrap_or(REORDER_HEADERS);
    let parent = string(s, "vlan", "parent");
    let o = bond_options(s);
    let opt = |k: &str| o.get(k).cloned().unwrap_or_default();
    let n = |key: &str| number(s, "bridge", key).map(|v| v.to_string()).unwrap_or_default();
    VirtForm {
        interface: string(s, "connection", "interface-name"),
        parent: parent_name(&parent).unwrap_or(parent),
        vlan_id: number(s, "vlan", "id").map(|v| v.to_string()).unwrap_or_default(),
        reorder_headers: flags & REORDER_HEADERS != 0,
        gvrp: flags & GVRP != 0,
        mvrp: flags & MVRP != 0,
        loose_binding: flags & LOOSE_BINDING != 0,
        // NetworkManager turns spanning tree on unless told otherwise.
        stp: get(s, "bridge", "stp").and_then(|v| bool::try_from(v).ok()).unwrap_or(s.contains_key("bridge")),
        stp_priority: n("priority"),
        forward_delay: n("forward-delay"),
        hello_time: n("hello-time"),
        max_age: n("max-age"),
        ageing_time: n("ageing-time"),
        vlan_filtering: get(s, "bridge", "vlan-filtering").and_then(|v| bool::try_from(v).ok()).unwrap_or(false),
        mode: BOND_MODES.iter().position(|(m, _)| *m == opt("mode")).unwrap_or(0),
        primary: opt("primary"),
        arp: o.get("arp_interval").is_some_and(|v| v != "0"),
        miimon: opt("miimon"),
        arp_interval: opt("arp_interval"),
        arp_targets: opt("arp_ip_target"),
        lacp_rate: LACP_RATES.iter().position(|(r, _)| *r == opt("lacp_rate")).unwrap_or(0),
        hash: HASH_POLICIES.iter().position(|(h, _)| *h == opt("xmit_hash_policy")).unwrap_or(0),
        ports: Vec::new(),
    }
}

/// What a new one starts with: the names nmcli would pick, spanning tree on, MII monitoring.
pub fn new_form(kind: crate::editor::Kind, snap: Option<&Snapshot>) -> VirtForm {
    use crate::editor::Kind;
    let taken = |name: &str| {
        snap.is_some_and(|s| {
            s.devices.iter().any(|d| d.interface == name)
                || s.connections.iter().any(|c| c.interface.as_deref() == Some(name))
        })
    };
    let free = |prefix: &str| (0..).map(|i| format!("{prefix}{i}")).find(|n| !taken(n)).unwrap_or_default();
    let parent = snap
        .and_then(|s| s.devices.iter().find(|d| d.kind == DeviceKind::Ethernet))
        .map(|d| d.interface.clone())
        .unwrap_or_default();
    match kind {
        Kind::Vlan => VirtForm {
            interface: if parent.is_empty() { String::new() } else { format!("{parent}.1") },
            parent,
            vlan_id: "1".into(),
            reorder_headers: true,
            ..VirtForm::default()
        },
        Kind::Bridge => VirtForm { interface: free("br"), stp: true, ..VirtForm::default() },
        Kind::Bond => VirtForm { interface: free("bond"), mode: 1, miimon: "100".into(), ..VirtForm::default() },
        _ => VirtForm::default(),
    }
}

/// The name a VLAN gets from its parent and ID, while the user hasn't typed one.
pub fn vlan_name(parent: &str, id: &str) -> String {
    format!("{parent}.{}", id.trim())
}

/// The ports of the bridge or bond with this UUID and interface name, as the snapshot has them.
pub fn ports_of(snap: &Snapshot, uuid: &str, interface: &str) -> Vec<PortRow> {
    snap.connections
        .iter()
        .filter(|c| {
            c.port
                .as_ref()
                .is_some_and(|p| p.controller == uuid || (!interface.is_empty() && p.controller == interface))
        })
        .map(|c| {
            let iface = c.interface.clone().unwrap_or_else(|| c.id.clone());
            PortRow { meta: device_meta(snap, &iface), interface: iface, connection: Some(c.path.clone()) }
        })
        .collect()
}

pub fn device_meta(snap: &Snapshot, interface: &str) -> String {
    let Some(d) = snap.devices.iter().find(|d| d.interface == interface) else { return "Not plugged in".into() };
    let kind = match d.kind {
        DeviceKind::Ethernet => "Ethernet",
        DeviceKind::Bond => "Bond",
        DeviceKind::Vlan => "VLAN",
        DeviceKind::Bridge => "Bridge",
        DeviceKind::Wifi => "Wi-Fi",
        _ => "Device",
    };
    match d.wired.as_ref() {
        Some(w) if !w.carrier => format!("{kind} · cable unplugged"),
        Some(w) if w.speed > 0 => format!("{kind} · {} Mb/s", w.speed),
        _ => kind.to_owned(),
    }
}

/// A device the add-port dialog offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub interface: String,
    pub meta: String,
    /// Why it can't be added, or empty.
    pub blocked: String,
    /// What adding it disconnects, or empty.
    pub warning: String,
    pub icon: &'static str,
}

/// The devices that could be ports of this bridge or bond: wired ones, and for a bridge also
/// bonds and VLANs. One that's in use can be taken over; its connection stays saved.
pub fn candidates(snap: &Snapshot, kind: crate::editor::Kind, own: &str, form: &VirtForm) -> Vec<Candidate> {
    use crate::editor::Kind;
    snap.devices
        .iter()
        .filter(|d| match kind {
            Kind::Bridge => {
                matches!(d.kind, DeviceKind::Ethernet | DeviceKind::Bond | DeviceKind::Vlan | DeviceKind::Wifi)
            }
            _ => matches!(d.kind, DeviceKind::Ethernet | DeviceKind::Wifi),
        })
        .filter(|d| d.interface != own && !form.ports.iter().any(|p| p.interface == d.interface))
        .map(|d| {
            let other_controller = snap.connections.iter().find_map(|c| {
                let port = c.port.as_ref()?;
                (c.interface.as_deref() == Some(d.interface.as_str()) && port.controller != own).then(|| {
                    snap.connections
                        .iter()
                        .find(|x| x.uuid == port.controller)
                        .and_then(|x| x.interface.clone())
                        .unwrap_or_else(|| port.controller.clone())
                })
            });
            let blocked = if d.kind == DeviceKind::Wifi {
                "Wi-Fi can't be a port: most access points drop frames from other devices.".into()
            } else if let Some(other) = other_controller {
                format!("Already a port of {other}")
            } else {
                String::new()
            };
            let active = d.active_connection.as_deref().and_then(|a| snap.active_connection(a));
            let warning = match active {
                Some(a) if blocked.is_empty() => {
                    format!("{} disconnects from {}; that connection stays saved.", d.interface, a.id)
                }
                _ => String::new(),
            };
            let icon = if d.kind == DeviceKind::Wifi { "wifi" } else { "wired" };
            Candidate { interface: d.interface.clone(), meta: device_meta(snap, &d.interface), blocked, warning, icon }
        })
        .collect()
}

fn put(s: &mut Settings, setting: &str, key: &str, value: Value<'_>) {
    let value = OwnedValue::try_from(value).expect("no file descriptors in settings");
    s.entry(setting.to_owned()).or_default().insert(key.to_owned(), value);
}

/// Writes the fields of `kind` into its settings. A VLAN's parent is kept by interface name, as
/// nmcli writes it.
pub fn write(s: &mut Settings, kind: crate::editor::Kind, initial: Option<&VirtForm>, f: &VirtForm) {
    use crate::editor::Kind;
    macro_rules! edited {
        ($field:ident) => {
            initial.is_none_or(|i| i.$field != f.$field)
        };
    }
    if edited!(interface) {
        put(s, "connection", "interface-name", Value::from(f.interface.trim()));
    }
    match kind {
        Kind::Vlan => {
            if edited!(parent) {
                put(s, "vlan", "parent", Value::from(f.parent.as_str()));
            }
            if edited!(vlan_id) {
                put(s, "vlan", "id", Value::from(f.vlan_id.trim().parse::<u32>().unwrap_or(0)));
            }
            if edited!(reorder_headers) || edited!(gvrp) || edited!(mvrp) || edited!(loose_binding) {
                let mut flags = 0;
                for (on, bit) in [
                    (f.reorder_headers, REORDER_HEADERS),
                    (f.gvrp, GVRP),
                    (f.mvrp, MVRP),
                    (f.loose_binding, LOOSE_BINDING),
                ] {
                    if on {
                        flags |= bit;
                    }
                }
                put(s, "vlan", "flags", Value::from(flags));
            }
        }
        Kind::Bridge => {
            if edited!(stp) {
                put(s, "bridge", "stp", Value::from(f.stp));
            }
            for (key, value, changed) in [
                ("priority", &f.stp_priority, edited!(stp_priority)),
                ("forward-delay", &f.forward_delay, edited!(forward_delay)),
                ("hello-time", &f.hello_time, edited!(hello_time)),
                ("max-age", &f.max_age, edited!(max_age)),
                ("ageing-time", &f.ageing_time, edited!(ageing_time)),
            ] {
                if changed {
                    match value.trim().parse::<u32>() {
                        Ok(v) => put(s, "bridge", key, Value::from(v)),
                        Err(_) => {
                            if let Some(b) = s.get_mut("bridge") {
                                b.remove(key);
                            }
                        }
                    }
                }
            }
            if edited!(vlan_filtering) {
                put(s, "bridge", "vlan-filtering", Value::from(f.vlan_filtering));
            }
        }
        Kind::Bond if initial.is_none_or(|i| bond_fields(i) != bond_fields(f)) => {
            let mut o = bond_options(s);
            let (mode, _) = BOND_MODES[f.mode.min(BOND_MODES.len() - 1)];
            o.insert("mode".into(), mode.into());
            // Only the options the mode uses: NetworkManager refuses the others.
            for key in ["primary", "miimon", "arp_interval", "arp_ip_target", "lacp_rate", "xmit_hash_policy"] {
                o.remove(key);
            }
            let mut set = |key: &str, value: &str| {
                if !value.trim().is_empty() {
                    o.insert(key.into(), value.trim().into());
                }
            };
            if mode == "active-backup" {
                set("primary", &f.primary);
            }
            if f.arp && !matches!(mode, "802.3ad" | "balance-tlb" | "balance-alb") {
                set("arp_interval", &f.arp_interval);
                let targets: Vec<&str> =
                    f.arp_targets.split([',', ' ']).map(str::trim).filter(|t| !t.is_empty()).collect();
                set("arp_ip_target", &targets.join(","));
            } else {
                set("miimon", &f.miimon);
            }
            if mode == "802.3ad" {
                set("lacp_rate", LACP_RATES[f.lacp_rate.min(1)].0);
            }
            if matches!(mode, "802.3ad" | "balance-xor" | "balance-tlb") {
                set("xmit_hash_policy", HASH_POLICIES[f.hash.min(HASH_POLICIES.len() - 1)].0);
            }
            put(s, "bond", "options", Value::from(o));
        }
        _ => {}
    }
    if matches!(kind, Kind::Bridge | Kind::Bond) {
        // The ports come up with it.
        put(s, "connection", "autoconnect-slaves", Value::from(1i32));
    }
}

fn bond_fields(f: &VirtForm) -> (usize, &str, bool, &str, &str, &str, usize, usize) {
    (f.mode, &f.primary, f.arp, &f.miimon, &f.arp_interval, &f.arp_targets, f.lacp_rate, f.hash)
}

/// What's wrong: (interface, VLAN ID or timers, link monitoring).
pub fn check(kind: crate::editor::Kind, f: &VirtForm) -> (String, String, String) {
    use crate::editor::Kind;
    let mut e = (String::new(), String::new(), String::new());
    let name = f.interface.trim();
    if name.is_empty() || name.len() > 15 || name.contains(['/', ' ', ':']) || name == "." || name == ".." {
        e.0 = "1 to 15 characters, without spaces, slashes or colons.".into();
    }
    let range = |text: &str, low: u32, high: u32| {
        text.trim().is_empty() || text.trim().parse::<u32>().is_ok_and(|v| (low..=high).contains(&v))
    };
    match kind {
        Kind::Vlan => {
            if f.parent.trim().is_empty() {
                e.1 = "Pick the device the VLAN runs on.".into();
            } else if !f.vlan_id.trim().parse::<u32>().is_ok_and(|v| (1..=4094).contains(&v)) {
                e.1 = "A VLAN ID is from 1 to 4094.".into();
            }
        }
        Kind::Bridge => {
            let ok = range(&f.stp_priority, 0, 65535)
                && range(&f.forward_delay, 2, 30)
                && range(&f.hello_time, 1, 10)
                && range(&f.max_age, 6, 40)
                && range(&f.ageing_time, 0, 1_000_000);
            if !ok {
                e.1 = "Priority 0 to 65535; forward delay 2 to 30 s; hello time 1 to 10 s; max age 6 to 40 s.".into();
            }
        }
        Kind::Bond => {
            let mode = BOND_MODES[f.mode.min(BOND_MODES.len() - 1)].0;
            if f.arp && !matches!(mode, "802.3ad" | "balance-tlb" | "balance-alb") {
                let targets: Vec<&str> =
                    f.arp_targets.split([',', ' ']).map(str::trim).filter(|t| !t.is_empty()).collect();
                if !range(&f.arp_interval, 1, 1_000_000) || f.arp_interval.trim().is_empty() {
                    e.2 = "Type how often to check, in milliseconds.".into();
                } else if targets.is_empty() || targets.iter().any(|t| t.parse::<std::net::Ipv4Addr>().is_err()) {
                    e.2 = "ARP monitoring needs one or more IPv4 addresses to ask, such as 192.168.1.1.".into();
                }
            } else if !range(&f.miimon, 0, 1_000_000) {
                e.2 = "Milliseconds between checks, such as 100.".into();
            }
        }
        _ => {}
    }
    e
}

/// What saving does to the ports: new port connections, and ports that are let go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortPlan {
    /// "bridge" or "bond".
    pub kind: &'static str,
    /// The bridge's or bond's interface name, for the new ports' names.
    pub controller: String,
    /// Interfaces to add.
    pub add: Vec<String>,
    /// Port connections to delete.
    pub remove: Vec<String>,
}

pub fn plan(kind: crate::editor::Kind, initial: Option<&VirtForm>, f: &VirtForm) -> Option<PortPlan> {
    use crate::editor::Kind;
    let kind = match kind {
        Kind::Bridge => "bridge",
        Kind::Bond => "bond",
        _ => return None,
    };
    let before = initial.map(|i| i.ports.as_slice()).unwrap_or_default();
    let add: Vec<String> = f.ports.iter().filter(|p| p.connection.is_none()).map(|p| p.interface.clone()).collect();
    let remove: Vec<String> = before
        .iter()
        .filter(|old| !f.ports.iter().any(|p| p.connection == old.connection))
        .filter_map(|old| old.connection.clone())
        .collect();
    (!add.is_empty() || !remove.is_empty()).then(|| PortPlan {
        kind,
        controller: f.interface.trim().to_owned(),
        add,
        remove,
    })
}

/// A port connection for `interface` under the controller with this UUID.
pub fn port_settings(plan: &PortPlan, controller_uuid: &str, interface: &str, uuid: &str) -> Settings {
    let mut s: Settings = HashMap::new();
    put(&mut s, "connection", "id", Value::from(format!("{} port {interface}", plan.controller)));
    put(&mut s, "connection", "uuid", Value::from(uuid));
    put(&mut s, "connection", "type", Value::from("802-3-ethernet"));
    put(&mut s, "connection", "interface-name", Value::from(interface));
    put(&mut s, "connection", "master", Value::from(controller_uuid));
    put(&mut s, "connection", "slave-type", Value::from(plan.kind));
    put(&mut s, "connection", "autoconnect", Value::from(true));
    s.entry("802-3-ethernet".into()).or_default();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Kind;

    fn new_bond() -> VirtForm {
        VirtForm { interface: "bond0".into(), mode: 1, miimon: "100".into(), ..VirtForm::default() }
    }

    #[test]
    fn a_bond_writes_only_the_options_its_mode_uses() {
        let mut s = Settings::new();
        let mut f = new_bond();
        f.primary = "enp3s0".into();
        f.hash = 2;
        write(&mut s, Kind::Bond, None, &f);
        let o = bond_options(&s);
        assert_eq!(o.get("mode").map(String::as_str), Some("active-backup"));
        assert_eq!(o.get("primary").map(String::as_str), Some("enp3s0"));
        assert_eq!(o.get("miimon").map(String::as_str), Some("100"));
        assert!(!o.contains_key("xmit_hash_policy") && !o.contains_key("lacp_rate"));
        let back = read(&s, |_| None);
        assert_eq!((back.mode, back.primary.as_str(), back.miimon.as_str()), (1, "enp3s0", "100"));

        f.mode = 4;
        f.lacp_rate = 1;
        let mut s = Settings::new();
        write(&mut s, Kind::Bond, None, &f);
        let o = bond_options(&s);
        assert_eq!(o.get("lacp_rate").map(String::as_str), Some("fast"));
        assert_eq!(o.get("xmit_hash_policy").map(String::as_str), Some("layer3+4"));
        assert!(!o.contains_key("primary"));
    }

    #[test]
    fn arp_monitoring_needs_targets() {
        let mut f = new_bond();
        f.arp = true;
        f.arp_interval = "200".into();
        assert!(!check(Kind::Bond, &f).2.is_empty());
        f.arp_targets = "192.168.1.1, 192.168.1.2".into();
        assert_eq!(check(Kind::Bond, &f), Default::default());
        let mut s = Settings::new();
        write(&mut s, Kind::Bond, None, &f);
        assert_eq!(bond_options(&s).get("arp_ip_target").map(String::as_str), Some("192.168.1.1,192.168.1.2"));
        assert!(!bond_options(&s).contains_key("miimon"));
    }

    #[test]
    fn vlans_and_bridges() {
        let f = VirtForm {
            interface: "enp3s0.20".into(),
            parent: "enp3s0".into(),
            vlan_id: "20".into(),
            reorder_headers: true,
            mvrp: true,
            ..VirtForm::default()
        };
        let mut s = Settings::new();
        write(&mut s, Kind::Vlan, None, &f);
        assert_eq!(number(&s, "vlan", "flags"), Some(REORDER_HEADERS | MVRP));
        assert_eq!(read(&s, |_| None), f);
        assert!(!check(Kind::Vlan, &VirtForm { vlan_id: "4095".into(), ..f.clone() }).1.is_empty());
        assert!(!check(Kind::Vlan, &VirtForm { interface: "a-very-long-name0".into(), ..f }).0.is_empty());

        let b = VirtForm { interface: "br0".into(), stp: true, forward_delay: "15".into(), ..VirtForm::default() };
        let mut s = Settings::new();
        write(&mut s, Kind::Bridge, None, &b);
        assert_eq!(number(&s, "bridge", "forward-delay"), Some(15));
        assert_eq!(number(&s, "bridge", "priority"), None);
        assert!(!check(Kind::Bridge, &VirtForm { hello_time: "11".into(), ..b }).1.is_empty());
    }

    #[test]
    fn saving_adds_and_lets_go_of_ports() {
        let kept = PortRow { interface: "enp3s0".into(), meta: String::new(), connection: Some("/c/p1".into()) };
        let gone = PortRow { interface: "enp4s0".into(), meta: String::new(), connection: Some("/c/p2".into()) };
        let new = PortRow { interface: "enx00e04c".into(), meta: String::new(), connection: None };
        let initial = VirtForm { interface: "br0".into(), ports: vec![kept.clone(), gone], ..VirtForm::default() };
        let f = VirtForm { ports: vec![kept, new], ..initial.clone() };
        let p = plan(Kind::Bridge, Some(&initial), &f).unwrap();
        assert_eq!((p.add, p.remove), (vec!["enx00e04c".to_owned()], vec!["/c/p2".to_owned()]));
        assert_eq!(plan(Kind::Bridge, Some(&initial), &initial), None);
        let plan = PortPlan { kind: "bridge", controller: "br0".into(), ..PortPlan::default() };
        let s = port_settings(&plan, "u-br0", "enp4s0", "u-p");
        assert_eq!(string(&s, "connection", "id"), "br0 port enp4s0");
        assert_eq!(string(&s, "connection", "master"), "u-br0");
        assert_eq!(string(&s, "connection", "slave-type"), "bridge");
    }
}
