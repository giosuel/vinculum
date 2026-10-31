//! The editor on the UI thread: opening it with a connection's settings, keeping the window's
//! form and Rust's in step, checking it on every change, and saving.

use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString};
use lituus_nm::{ConnectionKind, DeviceKind, Settings, Snapshot};

use crate::editor::{self, Address, EapForm, Form, IpForm, Kind, PeerForm, Route, Tab, WgForm};
use crate::openvpn::OvpnForm;
use crate::view::Command;
use crate::virt::{PortRow, VirtForm};
use crate::{
    APP, AddressRow, App, EapFormUi, EditorErrors, EditorForm, EditorOptions, EditorTab, IpFormUi, MainWindow,
    OvpnFormUi, PeerRow, PortCandidateUi, PortRowUi, RouteRow, VirtFormUi, WgFormUi, model, send, update,
};

/// What else the editor opens with.
#[derive(Debug, Default)]
pub struct Extras {
    /// The saved WireGuard private key, for its public key and "Show".
    pub wg_key: Option<String>,
    /// A note at the top: what an import did.
    pub banner: String,
}

/// Settings in a [`Command`]: compared by identity, since only the editor makes them.
#[derive(Clone, Debug)]
pub struct Shared(pub Arc<Settings>);

impl PartialEq for Shared {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Shared {}

/// A secret to put in the keyring before saving: (uuid, key, connection name, password).
pub type KeyringSecret = (String, &'static str, String, String);

pub struct EditorState {
    /// `None` for a new connection.
    pub path: Option<String>,
    pub kind: Kind,
    pub original: Settings,
    /// The form as read; `None` for a new connection.
    pub initial: Option<Form>,
    pub form: Form,
    pub uuid: String,
    pub tab: Tab,
    /// Values behind the choices: "" first, for "Default", "None", "Any".
    zones: Vec<String>,
    vpns: Vec<(String, String)>,
    bssids: Vec<(String, String)>,
    interfaces: Vec<String>,
    /// The devices a VLAN can run on.
    parents: Vec<String>,
    /// The add-port dialog is open.
    pub ports_open: bool,
    /// The active connection and its device, if it's up.
    active: Option<(String, Option<String>)>,
    current_ip6: Vec<String>,
    /// The device's IPv4 addresses, gateway and DNS now, for switching to Manual.
    current_ip4: Option<lituus_nm::IpConfig>,
    link: String,
    /// The routes as they were when the dialog opened, for Cancel.
    routes_backup: Option<(bool, Vec<Route>, bool, bool)>,
    routes_open: Option<bool>,
    /// A DNS server that couldn't be added, by family (v4, v6).
    dns_error: (String, String),
    pub busy: bool,
    /// Shown instead of "No changes." until the next edit ("Saved.").
    pub note: Option<(String, &'static str)>,
    /// Why the last save didn't work, until the next edit.
    pub failure: Option<String>,
    /// The form goes to the window on the next render.
    reset: bool,
    wg_key: Option<String>,
    wg_shown: bool,
    banner: String,
}

impl EditorState {
    pub fn new(
        path: Option<String>,
        settings: Settings,
        zones: Vec<String>,
        snap: Option<&Snapshot>,
        new: Option<Kind>,
    ) -> Self {
        let (kind, initial, uuid, original) = match new {
            Some(kind) => {
                let uuid = crate::secrets::new_uuid();
                (kind, None, uuid.clone(), editor::new_settings(kind, &uuid))
            }
            // Imported from a file: new, but filled in already.
            None if path.is_none() => {
                let (kind, form) = editor::read(&settings);
                let uuid = settings
                    .get("connection")
                    .and_then(|c| c.get("uuid"))
                    .and_then(|v| <&str>::try_from(&**v).ok())
                    .map(str::to_owned)
                    .unwrap_or_else(crate::secrets::new_uuid);
                let state_form = form;
                return Self::filled(kind, uuid, settings, state_form, zones, snap);
            }
            None => {
                let (kind, form) = editor::read(&settings);
                let uuid = settings
                    .get("connection")
                    .and_then(|c| c.get("uuid"))
                    .and_then(|v| <&str>::try_from(&**v).ok())
                    .unwrap_or_default()
                    .to_owned();
                (kind, Some(form), uuid, settings)
            }
        };
        let form = match &initial {
            Some(form) => form.clone(),
            None => {
                let name = if kind == Kind::Ethernet { wired_name(snap) } else { String::new() };
                let mut form = editor::new_form(kind, &name);
                if kind.is_virtual() {
                    form.virt = crate::virt::new_form(kind, snap);
                }
                form
            }
        };
        let mut state = Self::blank(path, kind, original, initial, form, uuid, zones);
        if let Some(snap) = snap {
            state.fill_from(snap);
        }
        state
    }

    /// A new connection whose form comes from an imported file.
    fn filled(
        kind: Kind,
        uuid: String,
        settings: Settings,
        form: Form,
        zones: Vec<String>,
        snap: Option<&Snapshot>,
    ) -> Self {
        let mut state = Self::blank(None, kind, settings, None, form, uuid, zones);
        state.tab = kind.tabs().get(1).copied().unwrap_or(Tab::General);
        if let Some(snap) = snap {
            state.fill_from(snap);
        }
        state
    }

    fn blank(
        path: Option<String>,
        kind: Kind,
        original: Settings,
        initial: Option<Form>,
        form: Form,
        uuid: String,
        zones: Vec<String>,
    ) -> Self {
        let new = if initial.is_none() && path.is_none() { Some(kind) } else { None };
        Self {
            path,
            kind,
            original,
            initial,
            form,
            uuid,
            // A new Wi-Fi connection starts with its network's name, a VLAN, bridge or bond with its own tab.
            tab: match new {
                Some(Kind::Wifi) => Tab::Wifi,
                Some(k) if k.is_virtual() => k.tabs()[1],
                _ => Tab::General,
            },
            zones: std::iter::once(String::new()).chain(zones).collect(),
            vpns: vec![(String::new(), "None".into())],
            bssids: vec![(String::new(), "Any (roam between access points)".into())],
            interfaces: vec![String::new()],
            parents: Vec::new(),
            ports_open: false,
            active: None,
            current_ip6: Vec::new(),
            current_ip4: None,
            link: String::new(),
            routes_backup: None,
            routes_open: None,
            dns_error: Default::default(),
            busy: false,
            note: None,
            failure: None,
            reset: true,
            wg_key: None,
            wg_shown: false,
            banner: String::new(),
        }
    }

    /// The choices that come from the snapshot: VPNs, access points, wired devices, and what's
    /// active.
    fn fill_from(&mut self, snap: &Snapshot) {
        for c in &snap.connections {
            if matches!(c.kind, ConnectionKind::WireGuard | ConnectionKind::Vpn { .. }) {
                self.vpns.push((c.uuid.clone(), c.id.clone()));
            }
        }
        if !self.form.vpn.is_empty() && !self.vpns.iter().any(|(u, _)| *u == self.form.vpn) {
            self.vpns.push((self.form.vpn.clone(), self.form.vpn.clone()));
        }
        let ssid = self.form.ssid.as_bytes();
        for d in snap.devices.iter().filter(|d| d.kind == DeviceKind::Wifi) {
            for ap in d.wifi.iter().flat_map(|w| &w.access_points).filter(|ap| ap.ssid == ssid) {
                let band = lituus_nm::Band::from_mhz(ap.frequency).map(|b| b.label()).unwrap_or("?");
                self.bssids.push((
                    ap.bssid.to_uppercase(),
                    format!("{} ({band}, {} %)", ap.bssid.to_uppercase(), ap.strength),
                ));
            }
        }
        if !self.form.bssid.is_empty() && !self.bssids.iter().any(|(b, _)| *b == self.form.bssid) {
            self.bssids.push((self.form.bssid.clone(), format!("{} (out of range)", self.form.bssid)));
        }
        for d in snap.devices.iter().filter(|d| d.kind == DeviceKind::Ethernet) {
            self.interfaces.push(d.interface.clone());
        }
        if !self.form.interface.is_empty() && !self.interfaces.contains(&self.form.interface) {
            self.interfaces.push(self.form.interface.clone());
        }
        for d in snap
            .devices
            .iter()
            .filter(|d| matches!(d.kind, DeviceKind::Ethernet | DeviceKind::Bond | DeviceKind::Bridge))
        {
            self.parents.push(d.interface.clone());
        }
        let parent = &self.form.virt.parent;
        if !parent.is_empty() && !self.parents.contains(parent) {
            // Kept by connection UUID: show that connection's interface.
            let named = snap.connections.iter().find(|c| c.uuid == *parent).and_then(|c| c.interface.clone());
            let parent = named.unwrap_or_else(|| parent.clone());
            self.form.virt.parent = parent.clone();
            if let Some(i) = self.initial.as_mut() {
                i.virt.parent = parent.clone();
            }
            if !self.parents.contains(&parent) {
                self.parents.push(parent);
            }
        }
        if matches!(self.kind, Kind::Bridge | Kind::Bond) && self.path.is_some() {
            let ports = crate::virt::ports_of(snap, &self.uuid, &self.form.virt.interface);
            self.form.virt.ports = ports.clone();
            if let Some(i) = self.initial.as_mut() {
                i.virt.ports = ports;
            }
        }
        if !self.form.zone.is_empty() && !self.zones.contains(&self.form.zone) {
            self.zones.push(self.form.zone.clone());
        }
        let active = snap.active.iter().find(|a| a.uuid == self.uuid);
        self.active = active.map(|a| (a.path.clone(), a.devices.first().cloned()));
        let device = active.and_then(|a| a.devices.first()).and_then(|p| snap.device(p));
        if let Some(device) = device {
            self.current_ip6 =
                device.ip6.iter().flat_map(|ip| &ip.addresses).map(|(a, p)| format!("{a}/{p}")).collect();
            self.current_ip4 = device.ip4.clone();
            if let Some(w) = &device.wired {
                self.link = if w.speed > 0 { format!("Negotiated: {} Mb/s", w.speed) } else { "Not connected".into() };
            }
        }
        if self.link.is_empty() {
            self.link = "Shown when the cable is connected.".into();
        }
    }

    /// Switching to Manual with no addresses starts from what the device has now (or an empty
    /// row to fill in).
    fn start_manual(&mut self, ip4_before: usize, ip6_before: usize) {
        let ip4 = &mut self.form.ip4;
        if ip4_before != 1 && ip4.method == 1 && ip4.addresses.is_empty() {
            match &self.current_ip4 {
                Some(now) if !now.addresses.is_empty() => {
                    ip4.addresses = now
                        .addresses
                        .iter()
                        .map(|(a, p)| Address { address: a.clone(), prefix: p.to_string() })
                        .collect();
                    if ip4.gateway.is_empty() {
                        ip4.gateway = now.gateway.clone().unwrap_or_default();
                    }
                    if ip4.dns.is_empty() {
                        ip4.dns = now.dns.clone();
                    }
                }
                _ => ip4.addresses.push(Address { address: String::new(), prefix: "24".into() }),
            }
            self.reset = true;
        }
        let ip6 = &mut self.form.ip6;
        if ip6_before != 2 && ip6.method == 2 && ip6.addresses.is_empty() {
            ip6.addresses.push(Address { address: String::new(), prefix: "64".into() });
            self.reset = true;
        }
    }

    pub fn errors(&self) -> editor::Errors {
        let mut e = editor::check(self.kind, self.initial.as_ref(), &self.form);
        if !self.dns_error.0.is_empty() {
            e.ip4.dns = self.dns_error.0.clone();
        }
        if !self.dns_error.1.is_empty() {
            e.ip6.dns = self.dns_error.1.clone();
        }
        e
    }

    pub fn dirty(&self) -> bool {
        self.initial.as_ref().is_none_or(|i| *i != self.form)
    }

    fn name(&self) -> String {
        match (self.form.name.trim(), self.kind) {
            ("", Kind::Wifi) => self.form.ssid.clone(),
            ("", Kind::WireGuard) => self.form.wg.interface.clone(),
            ("", Kind::Vlan | Kind::Bridge | Kind::Bond) => self.form.virt.interface.trim().to_owned(),
            (name, _) => name.to_owned(),
        }
    }
}

/// "Wired connection 2": the next name NetworkManager would pick.
fn wired_name(snap: Option<&Snapshot>) -> String {
    let taken: Vec<&str> = snap.map(|s| s.connections.iter().map(|c| c.id.as_str()).collect()).unwrap_or_default();
    (1..).map(|n| format!("Wired connection {n}")).find(|n| !taken.contains(&n.as_str())).unwrap_or_default()
}

fn s(text: impl Into<SharedString>) -> SharedString {
    text.into()
}

fn ip_ui(ip: &IpForm, dns_input: SharedString) -> IpFormUi {
    IpFormUi {
        method: ip.method as i32,
        addresses: model(
            ip.addresses.iter().map(|a| AddressRow { address: s(&a.address), prefix: s(&a.prefix) }).collect(),
        ),
        gateway: s(&ip.gateway),
        dns: model(ip.dns.iter().map(s).collect()),
        dns_input,
        search: s(&ip.search),
        ignore_auto_dns: ip.ignore_auto_dns,
        routes: model(
            ip.routes
                .iter()
                .map(|r| RouteRow {
                    dest: s(&r.dest),
                    prefix: s(&r.prefix),
                    gateway: s(&r.gateway),
                    metric: s(&r.metric),
                })
                .collect(),
        ),
        ignore_auto_routes: ip.ignore_auto_routes,
        never_default: ip.never_default,
        required: ip.required,
        privacy: ip.privacy as i32,
        stable_privacy: ip.stable_privacy,
    }
}

fn ip_form(ui: &IpFormUi) -> IpForm {
    IpForm {
        method: ui.method.max(0) as usize,
        addresses: ui
            .addresses
            .iter()
            .map(|a| Address { address: a.address.trim().to_owned(), prefix: a.prefix.trim().to_owned() })
            .collect(),
        gateway: ui.gateway.trim().to_owned(),
        dns: ui.dns.iter().map(|d| d.to_string()).collect(),
        search: ui.search.to_string(),
        ignore_auto_dns: ui.ignore_auto_dns,
        routes: ui
            .routes
            .iter()
            .map(|r| Route {
                dest: r.dest.trim().to_owned(),
                prefix: r.prefix.trim().to_owned(),
                gateway: r.gateway.trim().to_owned(),
                metric: r.metric.trim().to_owned(),
            })
            .collect(),
        ignore_auto_routes: ui.ignore_auto_routes,
        never_default: ui.never_default,
        required: ui.required,
        privacy: ui.privacy.max(0) as usize,
        stable_privacy: ui.stable_privacy,
    }
}

fn index_of<T>(list: &[T], pick: impl Fn(&T) -> bool) -> i32 {
    list.iter().position(pick).unwrap_or(0) as i32
}

fn to_ui(state: &EditorState, dns_inputs: (SharedString, SharedString)) -> EditorForm {
    let f = &state.form;
    EditorForm {
        name: s(&f.name),
        autoconnect: f.autoconnect,
        priority: s(&f.priority),
        only_me: f.only_me,
        metered: f.metered as i32,
        zone: index_of(&state.zones, |z| *z == f.zone),
        vpn: index_of(&state.vpns, |(u, _)| *u == f.vpn),
        ssid: s(&f.ssid),
        mode: f.mode as i32,
        band: f.band as i32,
        bssid: index_of(&state.bssids, |(b, _)| *b == f.bssid),
        hidden: f.hidden,
        security: f.security as i32,
        password: s(&f.password),
        storage: f.storage as i32,
        mac: f.mac as i32,
        mac_fixed: s(&f.mac_fixed),
        mtu: s(&f.mtu),
        interface: index_of(&state.interfaces, |i| *i == f.interface),
        negotiation: f.negotiation as i32,
        speed: s(&f.speed),
        full_duplex: f.full_duplex,
        wol_magic: f.wol_magic,
        wol_unicast: f.wol_unicast,
        wol_broadcast: f.wol_broadcast,
        ip4: ip_ui(&f.ip4, dns_inputs.0),
        ip6: ip_ui(&f.ip6, dns_inputs.1),
        eap: eap_ui(&f.eap),
        wg: wg_ui(&f.wg),
        ovpn: ovpn_ui(&f.ovpn),
        virt: virt_ui(state, &f.virt),
    }
}

fn virt_ui(state: &EditorState, v: &VirtForm) -> VirtFormUi {
    VirtFormUi {
        interface: s(&v.interface),
        parent: index_of(&state.parents, |p| *p == v.parent),
        vlan_id: s(&v.vlan_id),
        reorder_headers: v.reorder_headers,
        gvrp: v.gvrp,
        mvrp: v.mvrp,
        loose_binding: v.loose_binding,
        stp: v.stp,
        stp_priority: s(&v.stp_priority),
        forward_delay: s(&v.forward_delay),
        hello_time: s(&v.hello_time),
        max_age: s(&v.max_age),
        ageing_time: s(&v.ageing_time),
        vlan_filtering: v.vlan_filtering,
        mode: v.mode as i32,
        // "" (none) first, then the ports.
        primary: v.ports.iter().position(|p| p.interface == v.primary).map_or(0, |i| i as i32 + 1),
        arp: v.arp,
        miimon: s(&v.miimon),
        arp_interval: s(&v.arp_interval),
        arp_targets: s(&v.arp_targets),
        lacp_rate: v.lacp_rate as i32,
        hash: v.hash as i32,
    }
}

fn virt_form(state: &EditorState, u: &VirtFormUi) -> VirtForm {
    let ports = state.form.virt.ports.clone();
    VirtForm {
        interface: u.interface.trim().to_owned(),
        // Only a VLAN has a parent to pick.
        parent: if state.kind == Kind::Vlan {
            state.parents.get(u.parent.max(0) as usize).cloned().unwrap_or_default()
        } else {
            state.form.virt.parent.clone()
        },
        vlan_id: u.vlan_id.trim().to_owned(),
        reorder_headers: u.reorder_headers,
        gvrp: u.gvrp,
        mvrp: u.mvrp,
        loose_binding: u.loose_binding,
        stp: u.stp,
        stp_priority: u.stp_priority.trim().to_owned(),
        forward_delay: u.forward_delay.trim().to_owned(),
        hello_time: u.hello_time.trim().to_owned(),
        max_age: u.max_age.trim().to_owned(),
        ageing_time: u.ageing_time.trim().to_owned(),
        vlan_filtering: u.vlan_filtering,
        mode: u.mode.max(0) as usize,
        primary: match u.primary {
            i if i >= 1 => ports.get(i as usize - 1).map(|p| p.interface.clone()).unwrap_or_default(),
            _ => String::new(),
        },
        arp: u.arp,
        miimon: u.miimon.trim().to_owned(),
        arp_interval: u.arp_interval.trim().to_owned(),
        arp_targets: u.arp_targets.trim().to_owned(),
        lacp_rate: u.lacp_rate.max(0) as usize,
        hash: u.hash.max(0) as usize,
        ports,
    }
}

fn eap_ui(e: &EapForm) -> EapFormUi {
    EapFormUi {
        enabled: e.enabled,
        method: e.method as i32,
        phase2: e.phase2 as i32,
        identity: s(&e.identity),
        anonymous: s(&e.anonymous),
        password: s(&e.password),
        storage: e.storage as i32,
        system_ca: e.system_ca,
        ca_cert: s(&e.ca_cert),
        client_cert: s(&e.client_cert),
        private_key: s(&e.private_key),
        key_password: s(&e.key_password),
        domain: s(&e.domain),
    }
}

fn eap_form(u: &EapFormUi) -> EapForm {
    EapForm {
        enabled: u.enabled,
        method: u.method.max(0) as usize,
        phase2: u.phase2.max(0) as usize,
        identity: u.identity.trim().to_owned(),
        anonymous: u.anonymous.trim().to_owned(),
        password: u.password.to_string(),
        storage: u.storage.max(0) as usize,
        system_ca: u.system_ca,
        ca_cert: u.ca_cert.trim().to_owned(),
        client_cert: u.client_cert.trim().to_owned(),
        private_key: u.private_key.trim().to_owned(),
        key_password: u.key_password.to_string(),
        domain: u.domain.trim().to_owned(),
    }
}

fn wg_ui(w: &WgForm) -> WgFormUi {
    WgFormUi {
        interface: s(&w.interface),
        private_key: s(&w.private_key),
        listen_port: s(&w.listen_port),
        fwmark: s(&w.fwmark),
        peer_routes: w.peer_routes,
        peers: model(
            w.peers
                .iter()
                .map(|p| PeerRow {
                    public_key: s(&p.public_key),
                    endpoint: s(&p.endpoint),
                    allowed_ips: model(p.allowed_ips.iter().map(s).collect()),
                    allowed_input: "".into(),
                    keepalive: s(&p.keepalive),
                    has_psk: p.has_psk,
                })
                .collect(),
        ),
    }
}

fn wg_form(u: &WgFormUi) -> WgForm {
    WgForm {
        interface: u.interface.trim().to_owned(),
        private_key: u.private_key.trim().to_owned(),
        public_key: String::new(),
        listen_port: u.listen_port.trim().to_owned(),
        fwmark: u.fwmark.trim().to_owned(),
        peer_routes: u.peer_routes,
        peers: u
            .peers
            .iter()
            .map(|p| PeerForm {
                public_key: p.public_key.trim().to_owned(),
                endpoint: p.endpoint.trim().to_owned(),
                allowed_ips: p.allowed_ips.iter().map(|a| a.to_string()).collect(),
                keepalive: p.keepalive.trim().to_owned(),
                has_psk: p.has_psk,
            })
            .collect(),
    }
}

const DIRECTIONS: [&str; 3] = ["", "0", "1"];

fn ovpn_ui(o: &OvpnForm) -> OvpnFormUi {
    OvpnFormUi {
        gateway: s(&o.gateway),
        tcp: o.tcp,
        auth: o.auth as i32,
        username: s(&o.username),
        password: s(&o.password),
        key_password: s(&o.key_password),
        storage: o.storage as i32,
        ca: s(&o.ca),
        cert: s(&o.cert),
        key: s(&o.key),
        static_key: s(&o.static_key),
        tls_key: s(&o.tls_key),
        tls_key_mode: o.tls_key_mode as i32,
        tls_key_direction: index_of(&DIRECTIONS, |d| *d == o.tls_key_direction),
        cipher: s(&o.cipher),
        compression: o.compression as i32,
        mtu: s(&o.mtu),
        fragment: s(&o.fragment),
        reneg: s(&o.reneg),
        tap: o.tap,
    }
}

fn ovpn_form(u: &OvpnFormUi) -> OvpnForm {
    OvpnForm {
        gateway: u.gateway.trim().to_owned(),
        tcp: u.tcp,
        auth: u.auth.max(0) as usize,
        username: u.username.trim().to_owned(),
        password: u.password.to_string(),
        key_password: u.key_password.to_string(),
        storage: u.storage.max(0) as usize,
        ca: u.ca.trim().to_owned(),
        cert: u.cert.trim().to_owned(),
        key: u.key.trim().to_owned(),
        static_key: u.static_key.trim().to_owned(),
        tls_key: u.tls_key.trim().to_owned(),
        tls_key_mode: u.tls_key_mode.max(0) as usize,
        tls_key_direction: DIRECTIONS.get(u.tls_key_direction.max(0) as usize).copied().unwrap_or("").to_owned(),
        cipher: u.cipher.trim().to_owned(),
        compression: u.compression.max(0) as usize,
        mtu: u.mtu.trim().to_owned(),
        fragment: u.fragment.trim().to_owned(),
        reneg: u.reneg.trim().to_owned(),
        tap: u.tap,
    }
}

fn from_ui(state: &EditorState, ui: &EditorForm) -> Form {
    let pick = |list: &[String], i: i32| list.get(i.max(0) as usize).cloned().unwrap_or_default();
    Form {
        name: ui.name.to_string(),
        autoconnect: ui.autoconnect,
        priority: ui.priority.trim().to_owned(),
        only_me: ui.only_me,
        metered: ui.metered.max(0) as usize,
        zone: pick(&state.zones, ui.zone),
        vpn: state.vpns.get(ui.vpn.max(0) as usize).map(|(u, _)| u.clone()).unwrap_or_default(),
        ssid: ui.ssid.to_string(),
        mode: ui.mode.max(0) as usize,
        band: ui.band.max(0) as usize,
        bssid: state.bssids.get(ui.bssid.max(0) as usize).map(|(b, _)| b.clone()).unwrap_or_default(),
        hidden: ui.hidden,
        security: ui.security.max(0) as usize,
        password: ui.password.to_string(),
        storage: ui.storage.max(0) as usize,
        mac: ui.mac.max(0) as usize,
        mac_fixed: ui.mac_fixed.trim().to_owned(),
        mtu: ui.mtu.trim().to_owned(),
        interface: pick(&state.interfaces, ui.interface),
        negotiation: ui.negotiation.max(0) as usize,
        speed: ui.speed.trim().to_owned(),
        full_duplex: ui.full_duplex,
        wol_magic: ui.wol_magic,
        wol_unicast: ui.wol_unicast,
        wol_broadcast: ui.wol_broadcast,
        ip4: ip_form(&ui.ip4),
        ip6: ip_form(&ui.ip6),
        eap: eap_form(&ui.eap),
        wg: wg_form(&ui.wg),
        ovpn: ovpn_form(&ui.ovpn),
        virt: virt_form(state, &ui.virt),
    }
}

fn strings(items: impl IntoIterator<Item = impl Into<SharedString>>) -> ModelRc<SharedString> {
    model(items.into_iter().map(Into::into).collect())
}

fn errors_ui(e: &editor::Errors) -> EditorErrors {
    EditorErrors {
        name: s(&e.name),
        priority: s(&e.priority),
        ssid: s(&e.ssid),
        bssid: s(&e.bssid),
        password: s(&e.password),
        mac: s(&e.mac),
        mtu: s(&e.mtu),
        speed: s(&e.speed),
        ip4_addresses: s(&e.ip4.addresses),
        ip4_dns: s(&e.ip4.dns),
        ip4_routes: s(&e.ip4.routes),
        ip6_addresses: s(&e.ip6.addresses),
        ip6_dns: s(&e.ip6.dns),
        ip6_routes: s(&e.ip6.routes),
        eap_identity: s(&e.eap_identity),
        eap_secret: s(&e.eap_secret),
        eap_certs: s(&e.eap_certs),
        wg_interface: s(&e.wg_interface),
        wg_key: s(&e.wg_key),
        wg_port: s(&e.wg_port),
        peers: s(&e.peers),
        ovpn_user: s(&e.ovpn_user),
        ovpn_secret: s(&e.ovpn_secret),
        ovpn_files: s(&e.ovpn_files),
        virt_interface: s(&e.virt_interface),
        virt_settings: s(&e.virt_settings),
        virt_monitor: s(&e.virt_monitor),
    }
}

/// Shows the editor's state in the window.
pub fn render(window: &MainWindow, app: &mut App) {
    window.set_editor_open(app.editor.is_some());
    let keyring = app.keyring;
    let Some(state) = app.editor.as_mut() else { return };
    let errors = state.errors();
    if std::mem::take(&mut state.reset) {
        let form = window.get_editor_form();
        window.set_editor_form(to_ui(state, (form.ip4.dns_input, form.ip6.dns_input)));
        window.set_editor_tab(state.tab.id().into());
        window.set_editor_options(EditorOptions {
            zones: strings(state.zones.iter().map(|z| if z.is_empty() { "Default".to_owned() } else { z.clone() })),
            vpns: strings(state.vpns.iter().map(|(_, n)| n.clone())),
            bssids: strings(state.bssids.iter().map(|(_, l)| l.clone())),
            interfaces: strings(
                state.interfaces.iter().map(|i| if i.is_empty() { "Any wired device".to_owned() } else { i.clone() }),
            ),
            securities: strings(editor::SECURITIES.iter().map(|(l, _)| *l)),
            security_details: strings(editor::SECURITIES.iter().map(|(_, d)| *d)),
            macs: strings(editor::MACS.iter().map(|(_, l)| *l)),
            ip4_methods: strings(editor::IP4_METHODS.iter().map(|(_, l)| *l)),
            ip6_methods: strings(editor::IP6_METHODS.iter().map(|(_, l)| *l)),
            privacy: strings(editor::PRIVACY.iter().map(|(_, l)| *l)),
            current_ip6: strings(state.current_ip6.iter().cloned()),
            link: s(&state.link),
            has_zones: state.zones.len() > 1,
            eap_methods: strings(editor::EAP_METHODS.iter().map(|(_, l)| *l)),
            phase2: strings(editor::PHASE2.iter().map(|(_, l)| *l)),
            wg_public_key: "".into(),
            wg_shown_key: "".into(),
            ovpn_auth: strings(crate::openvpn::AUTH.iter().map(|(_, l)| *l)),
            ovpn_tls_modes: strings(crate::openvpn::TLS_KEY_MODES.iter().map(|(_, l)| *l)),
            ovpn_compression: strings(crate::openvpn::COMPRESSION.iter().map(|(_, l)| *l)),
            banner: s(&state.banner),
            keyring: "".into(),
            parents: strings(state.parents.iter().cloned()),
            bond_modes: strings(crate::virt::BOND_MODES.iter().map(|(_, l)| *l)),
            lacp_rates: strings(crate::virt::LACP_RATES.iter().map(|(_, l)| *l)),
            hash_policies: strings(crate::virt::HASH_POLICIES.iter().map(|(_, l)| *l)),
            ports: model(Vec::new()),
            primaries: model(Vec::new()),
            candidates: model(Vec::new()),
        });
    }
    // The parts of the options that change as you type.
    let mut options = window.get_editor_options();
    let typed = crate::wireguard::public_key(&state.form.wg.private_key);
    let public = typed.or_else(|| state.wg_key.as_deref().and_then(crate::wireguard::public_key)).unwrap_or_default();
    options.wg_public_key = public.into();
    options.wg_shown_key = if state.wg_shown { state.wg_key.clone().unwrap_or_default().into() } else { "".into() };
    // The ports, and the devices the add-port dialog offers.
    let v = &state.form.virt;
    options.ports = model(
        v.ports
            .iter()
            .map(|p| PortRowUi { interface: s(&p.interface), meta: s(&p.meta), new: p.connection.is_none() })
            .collect(),
    );
    options.primaries = strings(std::iter::once("None".to_owned()).chain(v.ports.iter().map(|p| p.interface.clone())));
    if let Some(snap) = &app.snapshot {
        options.candidates = model(
            crate::virt::candidates(snap, state.kind, &state.uuid, v)
                .into_iter()
                .map(|c| PortCandidateUi {
                    interface: s(&c.interface),
                    meta: s(&c.meta),
                    blocked: s(&c.blocked),
                    warning: s(&c.warning),
                    icon: s(c.icon),
                })
                .collect(),
        );
    }
    window.set_editor_ports_open(state.ports_open);
    options.keyring = match keyring {
        Some("KWallet") => "KWallet".into(),
        Some(_) => "your keyring".into(),
        None => "".into(),
    };
    window.set_editor_options(options);
    let kind_label = match state.kind {
        Kind::Wifi => "Wi-Fi connection",
        Kind::Ethernet => "Ethernet connection",
        Kind::WireGuard => "WireGuard VPN",
        Kind::OpenVpn => "OpenVPN",
        Kind::Vlan => "VLAN",
        Kind::Bridge => "Bridge",
        Kind::Bond => "Bond",
        Kind::Other => "Connection",
    };
    let device = state
        .active
        .as_ref()
        .and_then(|(_, d)| d.as_deref())
        .and_then(|d| app.snapshot.as_ref().and_then(|s| s.device(d)).map(|d| d.interface.clone()));
    let mut subtitle = vec![kind_label.to_owned()];
    subtitle.extend(device);
    subtitle.push(if state.path.is_none() {
        "new".into()
    } else if state.active.is_some() {
        "active".into()
    } else {
        "not active".into()
    });
    let name = state.name();
    window.set_editor_title(if name.is_empty() { "New connection".into() } else { name.as_str().into() });
    window.set_editor_subtitle(subtitle.join(" · ").into());
    let by_tab = errors.by_tab(state.kind);
    window.set_editor_tabs(model(
        state
            .kind
            .tabs()
            .iter()
            .map(|t| EditorTab {
                id: t.id().into(),
                label: t.label().into(),
                errors: by_tab.iter().find(|(tab, _)| tab == t).map(|(_, n)| *n as i32).unwrap_or(0),
            })
            .collect(),
    ));
    window.set_editor_errors(errors_ui(&errors));
    let total = errors.total(state.kind);
    let dirty = state.dirty();
    window.set_editor_dirty(dirty && state.path.is_some());
    window.set_editor_valid(total == 0);
    window.set_editor_busy(state.busy);
    window.set_editor_new(state.path.is_none());
    window.set_editor_routes(match state.routes_open {
        Some(true) => "ipv6".into(),
        Some(false) => "ipv4".into(),
        None => "".into(),
    });
    let (footer, tone) = if state.busy {
        ("Saving…".to_owned(), "muted")
    } else if let Some(failure) = &state.failure {
        (failure.clone(), "danger")
    } else if total > 0 {
        let tabs: Vec<&str> = by_tab.iter().filter(|(_, n)| *n > 0).map(|(t, _)| t.label()).collect();
        let errors = if total == 1 { "1 error".to_owned() } else { format!("{total} errors") };
        let tabs = match tabs.split_last() {
            Some((last, [])) => (*last).to_owned(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => String::new(),
        };
        (format!("Fix {errors} in {tabs} to {}.", if state.path.is_some() { "apply" } else { "save" }), "danger")
    } else if let Some((note, tone)) = state.note.clone().filter(|_| !dirty) {
        (note, tone)
    } else if state.path.is_none() {
        ("Not saved yet.".to_owned(), "muted")
    } else if dirty && state.active.is_some() {
        (format!("Applying updates {} right away.", state.name()), "muted")
    } else if dirty {
        ("Applying saves the changes.".to_owned(), "muted")
    } else {
        ("No changes.".to_owned(), "muted")
    };
    window.set_editor_footer(footer.into());
    window.set_editor_footer_tone(tone.into());
}

/// The form as the window has it now.
fn read_form(window: &MainWindow, app: &mut App) {
    let ui = window.get_editor_form();
    if let Some(state) = app.editor.as_mut() {
        state.form = from_ui(state, &ui);
        if let Some(tab) = state.kind.tabs().iter().find(|t| t.id() == window.get_editor_tab().as_str()) {
            state.tab = *tab;
        }
    }
}

pub fn connect(window: &MainWindow) {
    let weak = window.as_weak();
    window.on_editor_edited(move || {
        let Some(window) = weak.upgrade() else { return };
        APP.with(|app| {
            let mut app = app.borrow_mut();
            let before = app.editor.as_ref().map(|s| s.form.clone());
            read_form(&window, &mut app);
            if let (Some(state), Some(before)) = (app.editor.as_mut(), before)
                // Not when the window only took the form Rust gave it.
                && state.form != before
            {
                state.note = None;
                state.failure = None;
                state.dns_error = Default::default();
                state.start_manual(before.ip4.method, before.ip6.method);
                // A VLAN's interface is named after its parent and ID until it's given a name.
                let (b, v) = (&before.virt, &mut state.form.virt);
                if state.kind == Kind::Vlan
                    && (b.parent != v.parent || b.vlan_id != v.vlan_id)
                    && (b.interface.is_empty() || b.interface == crate::virt::vlan_name(&b.parent, &b.vlan_id))
                    && v.interface == b.interface
                {
                    v.interface = crate::virt::vlan_name(&v.parent, &v.vlan_id);
                    state.reset = true;
                }
            }
            render(&window, &mut app);
        });
    });
    let weak = window.as_weak();
    window.on_editor_back(move || update(&weak, close));
    let weak = window.as_weak();
    window.on_editor_revert(move || {
        update(&weak, |app| {
            if let Some(state) = app.editor.as_mut()
                && let Some(initial) = state.initial.clone()
            {
                state.form = initial;
                state.dns_error = Default::default();
                state.reset = true;
            }
        })
    });
    let weak = window.as_weak();
    window.on_editor_add_row(move |what| {
        edit_rows(&weak, |state| {
            let f = &mut state.form;
            match what.as_str() {
                "ip4-address" => f.ip4.addresses.push(Address { address: String::new(), prefix: "24".into() }),
                "ip6-address" => f.ip6.addresses.push(Address { address: String::new(), prefix: "64".into() }),
                "ip4-route" => f.ip4.routes.push(Route { prefix: "24".into(), ..Route::default() }),
                "ip6-route" => f.ip6.routes.push(Route { prefix: "64".into(), ..Route::default() }),
                "peer" => f.wg.peers.push(PeerForm::default()),
                "port" => state.ports_open = true,
                _ => {}
            }
        })
    });
    let weak = window.as_weak();
    window.on_editor_port_chosen(move |interface| {
        edit_rows(&weak, |state| {
            state.ports_open = false;
            if interface.is_empty() {
                return;
            }
            state.form.virt.ports.push(PortRow {
                interface: interface.to_string(),
                meta: "Added when you apply".into(),
                connection: None,
            });
        })
    });
    let weak = window.as_weak();
    window.on_editor_remove_row(move |what, i| {
        edit_rows(&weak, |state| {
            let f = &mut state.form;
            let i = i.max(0) as usize;
            fn remove<T>(list: &mut Vec<T>, i: usize) {
                if i < list.len() {
                    list.remove(i);
                }
            }
            match what.as_str() {
                "ip4-address" => remove(&mut f.ip4.addresses, i),
                "ip6-address" => remove(&mut f.ip6.addresses, i),
                "ip4-route" => remove(&mut f.ip4.routes, i),
                "ip6-route" => remove(&mut f.ip6.routes, i),
                "ip4-dns" => remove(&mut f.ip4.dns, i),
                "ip6-dns" => remove(&mut f.ip6.dns, i),
                "peer" => remove(&mut f.wg.peers, i),
                "port" => {
                    if f.virt.ports.get(i).is_some_and(|p| p.interface == f.virt.primary) {
                        f.virt.primary.clear();
                    }
                    remove(&mut f.virt.ports, i)
                }
                other => {
                    if let Some(peer) = other.strip_prefix("peer-ip:").and_then(|p| p.parse::<usize>().ok())
                        && let Some(peer) = f.wg.peers.get_mut(peer)
                    {
                        remove(&mut peer.allowed_ips, i);
                    }
                }
            }
        })
    });
    let weak = window.as_weak();
    window.on_editor_add_dns(move |family| {
        let Some(window) = weak.upgrade() else { return };
        let v6 = family == "ip6";
        let ui = window.get_editor_form();
        let input = if v6 { ui.ip6.dns_input.to_string() } else { ui.ip4.dns_input.to_string() };
        let input = input.trim().to_owned();
        APP.with(|app| {
            let mut app = app.borrow_mut();
            read_form(&window, &mut app);
            let Some(state) = app.editor.as_mut() else { return };
            let ok = input.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_ipv6() == v6);
            if input.is_empty() {
                return;
            }
            if ok {
                let list = if v6 { &mut state.form.ip6.dns } else { &mut state.form.ip4.dns };
                if !list.contains(&input) {
                    list.push(input);
                }
                state.dns_error = Default::default();
                let mut form = to_ui(state, ("".into(), "".into()));
                // Keep the other family's half-typed server.
                if v6 {
                    form.ip4.dns_input = ui.ip4.dns_input.clone()
                } else {
                    form.ip6.dns_input = ui.ip6.dns_input.clone()
                }
                window.set_editor_form(form);
            } else {
                let text = format!("{input} isn't an {} address.", if v6 { "IPv6" } else { "IPv4" });
                if v6 { state.dns_error.1 = text } else { state.dns_error.0 = text }
            }
            render(&window, &mut app);
        });
    });
    let weak = window.as_weak();
    window.on_editor_edit_routes(move |family| {
        let Some(window) = weak.upgrade() else { return };
        APP.with(|app| {
            let mut app = app.borrow_mut();
            read_form(&window, &mut app);
            if let Some(state) = app.editor.as_mut() {
                let v6 = family == "ipv6";
                let ip = if v6 { &state.form.ip6 } else { &state.form.ip4 };
                state.routes_backup = Some((v6, ip.routes.clone(), ip.ignore_auto_routes, ip.never_default));
                state.routes_open = Some(v6);
            }
            render(&window, &mut app);
        });
    });
    let weak = window.as_weak();
    window.on_editor_routes_done(move |keep| {
        let Some(window) = weak.upgrade() else { return };
        APP.with(|app| {
            let mut app = app.borrow_mut();
            read_form(&window, &mut app);
            if let Some(state) = app.editor.as_mut() {
                if let (false, Some((v6, routes, ignore, never))) = (keep, state.routes_backup.take()) {
                    let ip = if v6 { &mut state.form.ip6 } else { &mut state.form.ip4 };
                    ip.routes = routes;
                    ip.ignore_auto_routes = ignore;
                    ip.never_default = never;
                    state.reset = true;
                }
                state.routes_open = None;
            }
            render(&window, &mut app);
        });
    });
    let weak = window.as_weak();
    window.on_editor_add_allowed(move |peer| {
        let Some(window) = weak.upgrade() else { return };
        let ui = window.get_editor_form();
        let input =
            ui.wg.peers.row_data(peer.max(0) as usize).map(|p| p.allowed_input.trim().to_owned()).unwrap_or_default();
        if input.is_empty() {
            return;
        }
        edit_rows(&weak, |state| {
            if let Some(p) = state.form.wg.peers.get_mut(peer.max(0) as usize)
                && !p.allowed_ips.contains(&input)
            {
                // A bare address is one host.
                let range = if input.contains('/') {
                    input.clone()
                } else if input.contains(':') {
                    format!("{input}/128")
                } else {
                    format!("{input}/32")
                };
                p.allowed_ips.push(range);
            }
        });
    });
    let weak = window.as_weak();
    window.on_editor_wg_generate(move || {
        edit_rows(&weak, |state| state.form.wg.private_key = crate::wireguard::generate());
    });
    let weak = window.as_weak();
    window.on_editor_wg_show_key(move || {
        update(&weak, |app| {
            if let Some(state) = app.editor.as_mut() {
                state.wg_shown = true;
            }
        })
    });
    let weak = window.as_weak();
    window.on_editor_wg_export(move || {
        update(&weak, |app| {
            if let Some(path) = app.editor.as_ref().and_then(|s| s.path.clone()) {
                send(app, Command::ExportWireGuard { connection: path });
            }
        })
    });
    let weak = window.as_weak();
    window.on_editor_choose_file(move |field| {
        let Some(window) = weak.upgrade() else { return };
        update(&weak, |app| {
            read_form(&window, app);
            send(app, Command::ChooseFile { field: field.to_string() });
        })
    });
    let weak = window.as_weak();
    window.on_editor_apply(move || {
        let Some(window) = weak.upgrade() else { return };
        update(&weak, |app| {
            read_form(&window, app);
            apply(app);
        });
    });
}

/// Adds or removes a row (an address, a route, a DNS server) and shows the form again.
fn edit_rows(weak: &slint::Weak<MainWindow>, change: impl FnOnce(&mut EditorState)) {
    let Some(window) = weak.upgrade() else { return };
    APP.with(|app| {
        let mut app = app.borrow_mut();
        read_form(&window, &mut app);
        if let Some(state) = app.editor.as_mut() {
            change(state);
            state.reset = true;
        }
        render(&window, &mut app);
    });
}

fn close(app: &mut App) {
    app.editor = None;
}

fn apply(app: &mut App) {
    let owner = crate::secrets::username().unwrap_or_default();
    let Some(state) = app.editor.as_mut() else { return };
    if state.busy || state.errors().total(state.kind) > 0 {
        return;
    }
    let mut form = state.form.clone();
    if form.name.trim().is_empty() {
        form.name = match state.kind {
            Kind::Wifi => form.ssid.clone(),
            Kind::WireGuard => form.wg.interface.clone(),
            Kind::Vlan | Kind::Bridge | Kind::Bond => form.virt.interface.clone(),
            _ => form.name.clone(),
        };
    }
    let ports = crate::virt::plan(state.kind, state.initial.as_ref().map(|i| &i.virt), &form.virt);
    let settings = editor::write(state.kind, &state.original, state.initial.as_ref(), &form, &owner);
    // A new "for me only" password goes to the keyring first, where NetworkManager will ask for it.
    let secret = (state.kind == Kind::Wifi
        && matches!(form.security, 2..=4)
        && form.storage == 0
        && !form.password.is_empty())
    .then(|| {
        (state.uuid.clone(), if form.security == 2 { "wep-key0" } else { "psk" }, state.name(), form.password.clone())
    });
    state.busy = true;
    let command = Command::SaveConnection {
        path: state.path.clone(),
        settings: Shared(Arc::new(settings)),
        secret,
        reapply: state.active.clone(),
        ports,
    };
    send(app, command);
}

/// The NetworkManager thread saved it (or didn't): show the result, and start again from the
/// saved settings.
/// A saved connection's path and settings, and the new ports' (interface, connection).
pub type Saved = (String, Settings, Vec<(String, String)>);

pub fn saved(app: &mut App, result: Result<Saved, String>, zones: Vec<String>) {
    let snap = app.snapshot.clone();
    let Some(state) = app.editor.as_mut() else { return };
    state.busy = false;
    match result {
        Ok((path, settings, added)) => {
            let tab = state.tab;
            // A key typed or generated is the saved one now.
            let wg_key = Some(state.form.wg.private_key.clone()).filter(|k| !k.is_empty()).or(state.wg_key.clone());
            let mut fresh = EditorState::new(Some(path), settings, zones, snap.as_ref(), None);
            fresh.tab = tab;
            fresh.wg_key = wg_key;
            fresh.note = Some(("Saved.".into(), "accent"));
            // The snapshot may not have the new ports yet: keep the list as it was saved.
            if matches!(fresh.kind, Kind::Bridge | Kind::Bond) {
                let ports: Vec<PortRow> = state
                    .form
                    .virt
                    .ports
                    .iter()
                    .map(|p| PortRow {
                        meta: snap.as_ref().map_or(p.meta.clone(), |s| crate::virt::device_meta(s, &p.interface)),
                        connection: p
                            .connection
                            .clone()
                            .or_else(|| added.iter().find(|(i, _)| *i == p.interface).map(|(_, c)| c.clone())),
                        ..p.clone()
                    })
                    .collect();
                fresh.form.virt.ports = ports.clone();
                if let Some(i) = fresh.initial.as_mut() {
                    i.virt.ports = ports;
                }
            }
            *state = fresh;
        }
        Err(error) => state.failure = Some(error),
    }
}

/// Opens the editor for a saved connection's settings, or a new one of a kind.
pub fn open(
    app: &mut App,
    path: Option<String>,
    settings: Settings,
    zones: Vec<String>,
    new: Option<Kind>,
    extras: Extras,
) {
    let mut state = EditorState::new(path, settings, zones, app.snapshot.as_ref(), new);
    state.wg_key = extras.wg_key;
    state.banner = extras.banner;
    app.editor = Some(state);
}

/// A file was chosen for one of the editor's path fields ("eap.ca-cert", "ovpn.key", …).
pub fn file_chosen(app: &mut App, field: &str, path: Result<Option<String>, String>) {
    let Some(state) = app.editor.as_mut() else { return };
    let path = match path {
        Ok(Some(path)) => path,
        Ok(None) => return,
        Err(error) => {
            state.failure = Some(format!("{error}. You can type the path instead."));
            return;
        }
    };
    let f = &mut state.form;
    let target = match field {
        "eap.ca-cert" => &mut f.eap.ca_cert,
        "eap.client-cert" => &mut f.eap.client_cert,
        "eap.private-key" => &mut f.eap.private_key,
        "ovpn.ca" => &mut f.ovpn.ca,
        "ovpn.cert" => &mut f.ovpn.cert,
        "ovpn.key" => &mut f.ovpn.key,
        "ovpn.static-key" => &mut f.ovpn.static_key,
        "ovpn.tls-key" => &mut f.ovpn.tls_key,
        _ => return,
    };
    *target = path;
    state.reset = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::home;

    /// The window echoes the form back ("changed form" fires when Rust sets it): that's no edit.
    #[test]
    fn the_form_round_trips_through_the_window() {
        let snap = home();
        for kind in [Kind::Wifi, Kind::Ethernet, Kind::WireGuard, Kind::OpenVpn, Kind::Vlan, Kind::Bridge, Kind::Bond] {
            let state = EditorState::new(None, Settings::new(), Vec::new(), Some(&snap), Some(kind));
            let ui = to_ui(&state, (SharedString::new(), SharedString::new()));
            assert_eq!(from_ui(&state, &ui), state.form, "{kind:?}");
        }
    }
}
