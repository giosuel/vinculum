//! The editor on the UI thread: opening it with a connection's settings, keeping the window's
//! form and Rust's in step, checking it on every change, and saving.

use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString};
use vinculum_nm::{ConnectionKind, DeviceKind, Settings, Snapshot};

use crate::editor::{self, Address, Form, IpForm, Kind, Route, Tab};
use crate::view::Command;
use crate::{
    APP, AddressRow, App, EditorErrors, EditorForm, EditorOptions, EditorTab, IpFormUi, MainWindow, RouteRow, model,
    send, update,
};

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
    /// The active connection and its device, if it's up.
    active: Option<(String, Option<String>)>,
    current_ip6: Vec<String>,
    /// The device's IPv4 addresses, gateway and DNS now, for switching to Manual.
    current_ip4: Option<vinculum_nm::IpConfig>,
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
                editor::new_form(kind, &name)
            }
        };
        let mut state = Self {
            path,
            kind,
            original,
            initial,
            form,
            uuid,
            // A new Wi-Fi connection starts with its network's name.
            tab: if new == Some(Kind::Wifi) { Tab::Wifi } else { Tab::General },
            zones: std::iter::once(String::new()).chain(zones).collect(),
            vpns: vec![(String::new(), "None".into())],
            bssids: vec![(String::new(), "Any (roam between access points)".into())],
            interfaces: vec![String::new()],
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
        };
        if let Some(snap) = snap {
            state.fill_from(snap);
        }
        state
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
                let band = vinculum_nm::Band::from_mhz(ap.frequency).map(|b| b.label()).unwrap_or("?");
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
        if self.form.name.trim().is_empty() { self.form.ssid.clone() } else { self.form.name.trim().to_owned() }
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
    }
}

/// Shows the editor's state in the window.
pub fn render(window: &MainWindow, app: &mut App) {
    window.set_editor_open(app.editor.is_some());
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
        });
    }
    let kind_label = match state.kind {
        Kind::Wifi => "Wi-Fi connection",
        Kind::Ethernet => "Ethernet connection",
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
                _ => {}
            }
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
                _ => {}
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
    if state.kind == Kind::Wifi && form.name.trim().is_empty() {
        form.name = form.ssid.clone();
    }
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
    };
    send(app, command);
}

/// The NetworkManager thread saved it (or didn't): show the result, and start again from the
/// saved settings.
pub fn saved(app: &mut App, result: Result<(String, Settings), String>, zones: Vec<String>) {
    let snap = app.snapshot.clone();
    let Some(state) = app.editor.as_mut() else { return };
    state.busy = false;
    match result {
        Ok((path, settings)) => {
            let tab = state.tab;
            let mut fresh = EditorState::new(Some(path), settings, zones, snap.as_ref(), None);
            fresh.tab = tab;
            fresh.note = Some(("Saved.".into(), "accent"));
            *state = fresh;
        }
        Err(error) => state.failure = Some(error),
    }
}

/// Opens the editor for a saved connection's settings, or a new one of a kind.
pub fn open(app: &mut App, path: Option<String>, settings: Settings, zones: Vec<String>, new: Option<Kind>) {
    let state = EditorState::new(path, settings, zones, app.snapshot.as_ref(), new);
    app.editor = Some(state);
}
