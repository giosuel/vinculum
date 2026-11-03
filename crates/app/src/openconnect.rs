//! Cisco AnyConnect (and the other protocols OpenConnect speaks) through
//! NetworkManager-openconnect: its settings as editor fields, signing in with the plugin's own
//! window (which shows a SAML page such as Microsoft 365's in an embedded browser), and
//! openconnect-saml's profiles turned into connections.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use lituus_nm::Settings;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;
use zbus::zvariant::{OwnedValue, Value};

pub const SERVICE: &str = "org.freedesktop.NetworkManager.openconnect";

/// `protocol`, by the Protocol choice: (value, label).
pub const PROTOCOLS: [(&str, &str); 7] = [
    ("anyconnect", "Cisco AnyConnect"),
    ("gp", "Palo Alto GlobalProtect"),
    ("pulse", "Pulse / Ivanti Connect Secure"),
    ("nc", "Juniper Network Connect"),
    ("fortinet", "Fortinet FortiGate"),
    ("f5", "F5 BIG-IP"),
    ("array", "Array Networks"),
];
/// `reported_os`, by the Operating system choice; "" lets OpenConnect say what it runs on.
pub const SYSTEMS: [(&str, &str); 6] = [
    ("", "This computer (Linux)"),
    ("win", "Windows"),
    ("mac-intel", "macOS"),
    ("linux-64", "Linux (64-bit)"),
    ("android", "Android"),
    ("apple-ios", "iOS"),
];
/// What a new connection calls itself: some Cisco servers only ask for the MFA code when the
/// client says it's AnyConnect.
pub const USER_AGENT: &str = "AnyConnect";
/// Why a connection attempt stopped when the sign-in window was closed.
pub const CLOSED: &str = "The sign-in window was closed before you finished signing in.";
/// The secrets the sign-in window hands over for one connection; NetworkManager keeps none of
/// them (that's what the plugin's own editor sets too).
const ONE_TIME: [&str; 4] = ["cookie", "gateway", "gwcert", "resolve"];

/// The folders NetworkManager reads VPN plugins' `.name` files from.
pub const PLUGIN_DIRS: [&str; 4] = [
    "/usr/lib/NetworkManager/VPN",
    "/usr/lib64/NetworkManager/VPN",
    "/etc/NetworkManager/VPN",
    "/usr/local/lib/NetworkManager/VPN",
];

/// A plugin's `.name` file, wherever NetworkManager would find it.
pub fn plugin_file(name: &str) -> Option<PathBuf> {
    PLUGIN_DIRS.iter().map(|dir| Path::new(dir).join(name)).find(|p| p.exists())
}

/// Lituus runs in a Flatpak, which can't start the plugin's sign-in window.
pub fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

/// The plugin's sign-in window (`[GNOME] auth-dialog` in its `.name` file), when it's installed.
/// Debian and Fedora package it apart from the plugin (`…-gnome`).
pub fn auth_dialog() -> Option<PathBuf> {
    // In a Flatpak the plugin's folders are the sandbox's, and its window couldn't start anyway.
    if in_flatpak() {
        return None;
    }
    let text = std::fs::read_to_string(plugin_file("nm-openconnect-service.name")?).ok()?;
    let path = auth_dialog_in(&text)?;
    if path.is_absolute() {
        return path.exists().then_some(path);
    }
    // Relative to the plugin's libexec folder.
    ["/usr/libexec", "/usr/lib", "/usr/lib/NetworkManager"]
        .iter()
        .map(|d| Path::new(d).join(&path))
        .find(|p| p.exists())
}

/// The sign-in window isn't installed. A Flatpak can't tell, and leaves signing in to other agents.
pub fn missing() -> bool {
    !in_flatpak() && auth_dialog().is_none()
}

/// The settings paths of the snapshot's OpenConnect VPNs.
pub fn paths(snap: &lituus_nm::Snapshot) -> std::collections::HashSet<String> {
    snap.connections
        .iter()
        .filter(|c| matches!(&c.kind, lituus_nm::ConnectionKind::Vpn { service, .. } if service == SERVICE))
        .map(|c| c.path.clone())
        .collect()
}

fn auth_dialog_in(name_file: &str) -> Option<PathBuf> {
    let mut gnome = false;
    for line in name_file.lines().map(str::trim) {
        if line.starts_with('[') {
            gnome = line == "[GNOME]";
        } else if gnome && let Some(path) = line.strip_prefix("auth-dialog=") {
            return Some(PathBuf::from(path.trim()));
        }
    }
    None
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OcForm {
    /// The server, with the group after a slash if there is one ("vpn.uzh.ch/staff").
    pub gateway: String,
    /// Index into PROTOCOLS.
    pub protocol: usize,
    pub cert: String,
    pub key: String,
    pub ca: String,
    pub user_agent: String,
    /// Index into SYSTEMS.
    pub system: usize,
    /// The host scan (Cisco Secure Desktop) script; empty for none.
    pub csd: String,
    pub udp: bool,
    pub proxy: String,
    pub mtu: String,
    /// Refuse a server certificate that isn't trusted, instead of asking.
    pub refuse_unknown: bool,
}

impl OcForm {
    pub fn new() -> Self {
        Self { user_agent: USER_AGENT.into(), udp: true, ..Self::default() }
    }
}

fn map(s: &Settings, key: &str) -> HashMap<String, String> {
    s.get("vpn")
        .and_then(|v| v.get(key))
        .and_then(|d| <HashMap<String, String>>::try_from(d.clone()).ok())
        .unwrap_or_default()
}

/// `vpn.data`, the plugin's settings.
pub fn data(s: &Settings) -> HashMap<String, String> {
    map(s, "data")
}

/// `vpn.secrets`, what NetworkManager kept from earlier sign-ins (saved form answers and such).
pub fn secrets(s: &Settings) -> HashMap<String, String> {
    map(s, "secrets")
}

pub fn is_openconnect(s: &Settings) -> bool {
    s.get("vpn").and_then(|v| v.get("service-type")).and_then(|t| <&str>::try_from(&**t).ok()) == Some(SERVICE)
}

/// "Cisco AnyConnect" for a `protocol` value (no value means AnyConnect too); one Lituus doesn't
/// know is shown as it is.
pub fn protocol_label(protocol: &str) -> String {
    match PROTOCOLS.iter().find(|(p, _)| *p == protocol) {
        Some((_, label)) => (*label).to_owned(),
        None if protocol.is_empty() => PROTOCOLS[0].1.to_owned(),
        None => protocol.to_owned(),
    }
}

/// The form after its protocol was changed: a new connection's client name follows it while it's
/// still the default, since only AnyConnect servers want to hear "AnyConnect". Whether it
/// changed the form.
pub fn protocol_changed(before: &OcForm, f: &mut OcForm) -> bool {
    let anyconnect = |protocol: usize| PROTOCOLS[protocol].0 == "anyconnect";
    if before.protocol == f.protocol || f.user_agent != before.user_agent {
        return false;
    }
    match (anyconnect(f.protocol), f.user_agent.as_str()) {
        (false, USER_AGENT) => f.user_agent.clear(),
        (true, "") => f.user_agent = USER_AGENT.into(),
        _ => return false,
    }
    true
}

pub fn read(s: &Settings) -> OcForm {
    let d = data(s);
    let get = |k: &str| d.get(k).cloned().unwrap_or_default();
    OcForm {
        gateway: get("gateway"),
        protocol: PROTOCOLS.iter().position(|(p, _)| *p == get("protocol")).unwrap_or(0),
        cert: get("usercert"),
        key: get("userkey"),
        ca: get("cacert"),
        user_agent: get("useragent"),
        system: SYSTEMS.iter().position(|(o, _)| *o == get("reported_os")).unwrap_or(0),
        csd: if get("enable_csd_trojan") == "yes" { get("csd_wrapper") } else { String::new() },
        udp: get("disable_udp") != "yes",
        proxy: get("proxy"),
        mtu: get("mtu"),
        refuse_unknown: get("prevent_invalid_cert") == "yes",
    }
}

/// Writes the form into the `vpn` setting, changing only the keys that differ from `initial`.
pub fn write(s: &mut Settings, initial: Option<&OcForm>, f: &OcForm) {
    let mut d = data(s);
    macro_rules! edited {
        ($field:ident) => {
            initial.is_none_or(|i| i.$field != f.$field)
        };
    }
    let mut set = |key: &str, value: &str| {
        if value.trim().is_empty() {
            d.remove(key);
        } else {
            d.insert(key.to_owned(), value.trim().to_owned());
        }
    };
    for (key, value, changed) in [
        ("gateway", &f.gateway, edited!(gateway)),
        ("usercert", &f.cert, edited!(cert)),
        ("userkey", &f.key, edited!(key)),
        ("cacert", &f.ca, edited!(ca)),
        ("useragent", &f.user_agent, edited!(user_agent)),
        ("proxy", &f.proxy, edited!(proxy)),
        ("mtu", &f.mtu, edited!(mtu)),
    ] {
        if changed {
            set(key, value);
        }
    }
    if edited!(protocol) {
        set("protocol", PROTOCOLS[f.protocol].0);
    }
    if edited!(system) {
        set("reported_os", SYSTEMS[f.system].0);
    }
    if edited!(cert) {
        set("authtype", if f.cert.trim().is_empty() { "password" } else { "cert" });
    }
    if edited!(csd) {
        set("enable_csd_trojan", if f.csd.trim().is_empty() { "no" } else { "yes" });
        set("csd_wrapper", &f.csd);
    }
    if edited!(udp) {
        set("disable_udp", if f.udp { "" } else { "yes" });
    }
    if edited!(refuse_unknown) {
        set("prevent_invalid_cert", if f.refuse_unknown { "yes" } else { "" });
    }
    // On every save, as the plugin's own editor does: a connection made elsewhere (with nmcli,
    // say) may keep an old cookie, which the sign-in window would hand back as a new one.
    for key in ONE_TIME {
        set(&format!("{key}-flags"), "2");
    }
    let vpn = s.entry("vpn".into()).or_default();
    vpn.insert("service-type".into(), OwnedValue::try_from(Value::from(SERVICE)).expect("a string"));
    vpn.insert("data".into(), OwnedValue::try_from(Value::from(d)).expect("a dict"));
}

/// What's wrong with the form, as (gateway, MTU) problems.
pub fn check(f: &OcForm) -> (String, String) {
    let mut problems = (String::new(), String::new());
    if f.gateway.trim().is_empty() {
        problems.0 = "Type the VPN server, such as vpn.example.org.".into();
    } else if f.gateway.trim().contains(char::is_whitespace) {
        problems.0 = "The server's address can't have spaces.".into();
    }
    if !f.mtu.trim().is_empty() && !matches!(f.mtu.trim().parse::<u32>(), Ok(576..=9216)) {
        problems.1 = "From 576 to 9216 bytes, or empty for automatic.".into();
    }
    problems
}

// ---- Signing in ---------------------------------------------------------------------------------

/// A sign-in for one connection.
pub struct SignIn<'a> {
    pub uuid: &'a str,
    pub name: &'a str,
    pub data: &'a HashMap<String, String>,
    pub secrets: &'a HashMap<String, String>,
    /// NetworkManager said the last sign-in didn't work.
    pub again: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SignInError {
    /// The window was closed without signing in.
    Closed,
    /// Lituus (or NetworkManager) closed the window.
    Cancelled,
    /// It couldn't run, or stopped with an error.
    Failed(String),
}

/// How long the window gets to quit once it's asked to, before it's killed.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// What the sign-in window reads on its standard input: the connection's settings and the
/// secrets kept from earlier, the way nm-applet and GNOME Shell send them. An earlier session
/// isn't sent: the window prints its input back when it's closed, and an old cookie would look
/// like a new sign-in.
fn dialog_input(data: &HashMap<String, String>, secrets: &HashMap<String, String>) -> String {
    let one_line = |v: &str| v.replace(['\n', '\r'], " ");
    let mut out = String::new();
    let mut sorted: Vec<_> = data.iter().collect();
    sorted.sort();
    for (k, v) in sorted {
        out.push_str(&format!("DATA_KEY={k}\nDATA_VAL={}\n\n", one_line(v)));
    }
    let mut sorted: Vec<_> = secrets.iter().filter(|(k, _)| !ONE_TIME.contains(&k.as_str())).collect();
    sorted.sort();
    for (k, v) in sorted {
        out.push_str(&format!("SECRET_KEY={k}\nSECRET_VAL={}\n\n", one_line(v)));
    }
    out.push_str("DONE\n\n");
    out
}

/// Runs the plugin's sign-in window and waits for the session it hands back (`cookie`,
/// `gateway`, `gwcert`, and what it wants kept). `cancel` closes it.
pub async fn sign_in(
    dialog: &Path,
    s: SignIn<'_>,
    mut cancel: oneshot::Receiver<()>,
) -> Result<HashMap<String, String>, SignInError> {
    let mut args = vec!["-u", s.uuid, "-n", s.name, "-s", SERVICE, "-i"];
    if s.again {
        args.push("-r");
    }
    let mut child = tokio::process::Command::new(dialog)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| SignInError::Failed(format!("Couldn't open the sign-in window ({}): {e}.", dialog.display())))?;
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    // Its last words on standard error say why it failed (it never prints the session there).
    let stderr = child.stderr.take().expect("piped");
    let last_words = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut last = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                last = line;
            }
        }
        last
    });
    // It reads everything up to DONE before it shows the window; a closed pipe means it quit.
    let _ = stdin.write_all(dialog_input(s.data, s.secrets).as_bytes()).await;
    let read = async {
        let mut lines = BufReader::new(stdout).lines();
        let mut out = HashMap::new();
        // "key\nvalue\n" pairs, then an empty line.
        while let Ok(Some(key)) = lines.next_line().await {
            if key.is_empty() {
                break;
            }
            let value = lines.next_line().await.ok().flatten().unwrap_or_default();
            out.insert(key, value);
        }
        out
    };
    let out = tokio::select! {
        out = read => out,
        // Only a cancel counts: a sender that went away isn't one.
        Ok(()) = &mut cancel => {
            quit(&mut child, stdin).await;
            return Err(SignInError::Cancelled);
        }
    };
    // Killed after it answered: the answer still counts.
    let Some(status) = quit(&mut child, stdin).await else {
        return out.get("cookie").filter(|c| !c.is_empty()).map(|_| out.clone()).ok_or(SignInError::Closed);
    };
    if !status.success() {
        let words = tokio::time::timeout(QUIT_WAIT, last_words).await.ok().and_then(Result::ok).unwrap_or_default();
        return Err(SignInError::Failed(failure(&words, status.code())));
    }
    if out.get("cookie").is_none_or(String::is_empty) {
        return Err(SignInError::Closed);
    }
    Ok(out)
}

/// Asks the window to quit, as NetworkManager's agents do, and kills it if it hasn't within
/// QUIT_WAIT. Its exit status, unless it was killed.
async fn quit(
    child: &mut tokio::process::Child,
    mut stdin: tokio::process::ChildStdin,
) -> Option<std::process::ExitStatus> {
    let _ = stdin.write_all(b"QUIT\n\n").await;
    // Its standard input stays open until it's gone: it complains when it closes early.
    let status = tokio::time::timeout(QUIT_WAIT, child.wait()).await.ok().and_then(Result::ok);
    if status.is_none() {
        let _ = child.kill().await;
    }
    status
}

/// Why the window failed, from its last line on standard error, without GLib's prefix:
/// "(nm-openconnect-auth-dialog:7): Gtk-WARNING **: 12:00:00.000: cannot open display: :0".
fn failure(last_words: &str, code: Option<i32>) -> String {
    let mut words = last_words.rsplit_once("**: ").map_or(last_words, |(_, w)| w).trim();
    if let Some((time, rest)) = words.split_once(": ")
        && !time.is_empty()
        && time.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '.')
    {
        words = rest.trim();
    }
    match (words, code) {
        ("", Some(code)) => format!("The sign-in window stopped with an error (exit status {code})."),
        ("", None) => "The sign-in window stopped unexpectedly.".into(),
        (words, _) => format!("The sign-in window stopped: {}.", words.trim_end_matches('.')),
    }
}

/// How long a session from signing in waits for NetworkManager's request. It asks within seconds
/// of being told to connect; an older one belongs to an attempt that went nowhere.
const READY_FOR: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Default)]
struct Pending {
    /// Sessions from signing in before connecting, by connection UUID, for NetworkManager's next
    /// request, with when they came.
    ready: HashMap<String, (std::time::Instant, HashMap<String, String>)>,
    /// Sign-ins whose window is open.
    running: Vec<Running>,
    /// The number of the next sign-in.
    next: u64,
}

struct Running {
    id: u64,
    uuid: String,
    /// The connection's settings path.
    path: String,
    cancel: oneshot::Sender<()>,
}

/// A sign-in that started: its number, for [`SignIns::finish`], and what hears when it's
/// cancelled.
pub struct Started {
    pub id: u64,
    pub cancel: oneshot::Receiver<()>,
}

/// Sign-ins in progress and their sessions, shared by the secret agent and the commands.
#[derive(Clone, Default)]
pub struct SignIns(Arc<Mutex<Pending>>);

impl SignIns {
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        let mut pending = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.ready.retain(|_, (at, _)| at.elapsed() < READY_FOR);
        pending
    }

    pub fn set_ready(&self, uuid: &str, secrets: HashMap<String, String>) {
        self.lock().ready.insert(uuid.to_owned(), (std::time::Instant::now(), secrets));
    }

    pub fn take_ready(&self, uuid: &str) -> Option<HashMap<String, String>> {
        self.lock().ready.remove(uuid).map(|(_, secrets)| secrets)
    }

    /// A session waits for this connection.
    pub fn has_ready(&self, uuid: &str) -> bool {
        self.lock().ready.contains_key(uuid)
    }

    /// A sign-in starts, unless the connection's window is open already.
    pub fn start(&self, uuid: &str, path: &str) -> Option<Started> {
        let mut pending = self.lock();
        if pending.running.iter().any(|r| r.uuid == uuid) {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        pending.next += 1;
        let id = pending.next;
        pending.running.push(Running { id, uuid: uuid.to_owned(), path: path.to_owned(), cancel: tx });
        Some(Started { id, cancel: rx })
    }

    pub fn finish(&self, id: u64) {
        self.lock().running.retain(|r| r.id != id);
    }

    /// Closes the sign-in window of a connection, by UUID or by its settings path.
    pub fn cancel(&self, uuid_or_path: &str) {
        let mut pending = self.lock();
        let (cancelled, kept) = std::mem::take(&mut pending.running)
            .into_iter()
            .partition(|r| r.uuid == uuid_or_path || r.path == uuid_or_path);
        pending.running = kept;
        for r in cancelled {
            let _ = r.cancel.send(());
        }
    }
}

// ---- openconnect-saml ------------------------------------------------------------------------------

/// The config file of openconnect-saml (and openconnect-sso, which it was forked from).
pub fn saml_config() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))?;
    ["openconnect-saml", "openconnect-sso"].iter().map(|app| base.join(app).join("config.toml")).find(|p| p.exists())
}

/// "vpn.uzh.ch" and "staff" → "vpn.uzh.ch/staff".
fn with_group(server: &str, group: &str) -> String {
    let group = group.trim().trim_matches('/');
    let server = server.trim();
    if group.is_empty() { server.to_owned() } else { format!("{}/{group}", server.trim_end_matches('/')) }
}

/// openconnect-saml's profiles (its `[profiles.*]`, or the older `[default_profile]`), as
/// (name, form) pairs.
pub fn import_saml(text: &str) -> Result<Vec<(String, OcForm)>, String> {
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        // Its first line says what's wrong; the rest quotes the file.
        let what = e.to_string().lines().next().unwrap_or_default().trim().trim_end_matches('.').to_owned();
        format!("the file isn't valid TOML ({what}).")
    })?;
    let string = |t: &toml::Table, k: &str| t.get(k).and_then(toml::Value::as_str).unwrap_or("").to_owned();
    let mut out = Vec::new();
    if let Some(profiles) = table.get("profiles").and_then(toml::Value::as_table) {
        for (key, profile) in profiles {
            let Some(p) = profile.as_table() else { continue };
            let server = string(p, "server");
            if server.is_empty() {
                continue;
            }
            let name = Some(string(p, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| key.clone());
            let form = OcForm {
                gateway: with_group(&server, &string(p, "user_group")),
                cert: string(p, "cert"),
                key: string(p, "cert_key"),
                ..OcForm::new()
            };
            out.push((name, form));
        }
    }
    if let Some(p) = table.get("default_profile").and_then(toml::Value::as_table)
        && !string(p, "address").trim().is_empty()
    {
        let gateway = with_group(&string(p, "address"), &string(p, "user_group"));
        let bare = |g: &str| g.trim_start_matches("https://").trim_end_matches('/').to_lowercase();
        if !out.iter().any(|(_, f)| bare(&f.gateway) == bare(&gateway)) {
            let name = Some(string(p, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| gateway.clone());
            out.push((name, OcForm { gateway, ..OcForm::new() }));
        }
    }
    if out.is_empty() {
        return Err("it has no profiles (a [profiles.…] section with a server).".into());
    }
    Ok(out)
}

/// Whether a file looks like openconnect-saml's config rather than a WireGuard `.conf`.
pub fn is_saml_config(file_name: &str, text: &str) -> bool {
    file_name.ends_with(".toml")
        || text.lines().any(|l| l.trim_start().starts_with("[profiles.") || l.trim() == "[default_profile]")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn finds_the_sign_in_window_in_the_name_file() {
        let name = "[VPN Connection]\nname=openconnect\nservice=org.freedesktop.NetworkManager.openconnect\n\
            program=/usr/lib/nm-openconnect-service\n\n[GNOME]\nauth-dialog=/usr/lib/nm-openconnect-auth-dialog\n\
            properties=/usr/lib/NetworkManager/libnm-vpn-plugin-openconnect\n";
        assert_eq!(auth_dialog_in(name), Some(PathBuf::from("/usr/lib/nm-openconnect-auth-dialog")));
        assert_eq!(auth_dialog_in("[VPN Connection]\nauth-dialog=/nope\n"), None);
    }

    #[test]
    fn settings_round_trip() {
        // Every field has its own value, so two swapped in read or write would show.
        let f = OcForm {
            gateway: "vpn.uzh.ch/staff".into(),
            protocol: 1,
            cert: "/home/u/cert.pem".into(),
            key: "/home/u/key.pem".into(),
            ca: "/home/u/ca.pem".into(),
            user_agent: "Lituus test".into(),
            system: 2,
            csd: "/usr/libexec/openconnect/csd-post.sh".into(),
            udp: false,
            proxy: "http://proxy.uzh.ch:3128".into(),
            mtu: "1300".into(),
            refuse_unknown: true,
        };
        let mut s: Settings = HashMap::new();
        write(&mut s, None, &f);
        assert!(is_openconnect(&s));
        let d = data(&s);
        assert_eq!(d["gateway"], "vpn.uzh.ch/staff");
        assert_eq!((d["protocol"].as_str(), d["reported_os"].as_str()), ("gp", "mac-intel"));
        assert_eq!((d["usercert"].as_str(), d["userkey"].as_str()), ("/home/u/cert.pem", "/home/u/key.pem"));
        assert_eq!((d["cacert"].as_str(), d["proxy"].as_str()), ("/home/u/ca.pem", "http://proxy.uzh.ch:3128"));
        assert_eq!((d["useragent"].as_str(), d["authtype"].as_str()), ("Lituus test", "cert"));
        assert_eq!((d["mtu"].as_str(), d["prevent_invalid_cert"].as_str()), ("1300", "yes"));
        assert_eq!((d["enable_csd_trojan"].as_str(), d["disable_udp"].as_str()), ("yes", "yes"));
        assert_eq!(read(&s), f);
        // Changing the gateway leaves the rest (and keys the editor doesn't show) alone.
        let mut d2 = d.clone();
        d2.insert("stoken_source".into(), "totp".into());
        s.get_mut("vpn").unwrap().insert("data".into(), OwnedValue::try_from(Value::from(d2.clone())).unwrap());
        let g = OcForm { gateway: "vpn2.uzh.ch".into(), ..f.clone() };
        write(&mut s, Some(&f), &g);
        let after = data(&s);
        assert_eq!(after["gateway"], "vpn2.uzh.ch");
        assert_eq!(after.len(), d2.len());
        assert_eq!(after["stoken_source"], "totp");
        assert_eq!(read(&s), g);
    }

    #[test]
    fn the_session_is_never_kept() {
        // A connection made with nmcli keeps its cookie (no flags); saving it once fixes that.
        let mut s: Settings = HashMap::new();
        let f = OcForm { gateway: "vpn.uzh.ch".into(), ..OcForm::new() };
        write(&mut s, None, &f);
        let mut d = data(&s);
        for key in ONE_TIME {
            assert_eq!(d.remove(&format!("{key}-flags")).as_deref(), Some("2"), "{key}");
        }
        s.get_mut("vpn").unwrap().insert("data".into(), OwnedValue::try_from(Value::from(d)).unwrap());
        let g = OcForm { mtu: "1400".into(), ..f.clone() };
        write(&mut s, Some(&f), &g);
        let d = data(&s);
        assert!(ONE_TIME.iter().all(|key| d[&format!("{key}-flags")] == "2"));
    }

    #[test]
    fn a_new_connection_says_it_is_anyconnect() {
        let mut s: Settings = HashMap::new();
        write(&mut s, None, &OcForm { gateway: "vpn.example.org".into(), ..OcForm::new() });
        let d = data(&s);
        assert_eq!(d["useragent"], USER_AGENT);
        assert_eq!(d["protocol"], "anyconnect");
        assert!(!d.contains_key("disable_udp"));
        assert_eq!(protocol_label(""), "Cisco AnyConnect");
        assert_eq!(protocol_label("gp"), "Palo Alto GlobalProtect");
        assert_eq!(protocol_label("pulse-ng"), "pulse-ng", "one Lituus doesn't know is shown as it is");
        assert_eq!(check(&OcForm::new()).0, "Type the VPN server, such as vpn.example.org.");
        assert!(!check(&OcForm { gateway: "a".into(), mtu: "40".into(), ..OcForm::new() }).1.is_empty());
    }

    #[test]
    fn only_anyconnect_says_it_is_anyconnect() {
        let before = OcForm::new();
        let mut f = OcForm { protocol: 1, ..before.clone() };
        assert!(protocol_changed(&before, &mut f));
        assert_eq!(f.user_agent, "");
        let mut back = OcForm { protocol: 0, ..f.clone() };
        assert!(protocol_changed(&f, &mut back));
        assert_eq!(back.user_agent, USER_AGENT);
        // A name the user typed stays.
        let typed = OcForm { user_agent: "Cisco AnyConnect VPN Agent".into(), ..before.clone() };
        let mut f = OcForm { protocol: 1, ..typed.clone() };
        assert!(!protocol_changed(&typed, &mut f));
        assert_eq!(f.user_agent, "Cisco AnyConnect VPN Agent");
        let mut same = before.clone();
        assert!(!protocol_changed(&before, &mut same));
    }

    #[test]
    fn the_dialog_gets_settings_and_kept_secrets() {
        let data = HashMap::from([("gateway".to_owned(), "vpn.uzh.ch".to_owned())]);
        let mut secrets = HashMap::from([("form:main:group_list".to_owned(), "staff\n".to_owned())]);
        // An earlier session isn't sent: the window would print it back as if it had signed in.
        for key in ONE_TIME {
            secrets.insert(key.to_owned(), "old".to_owned());
        }
        assert_eq!(
            dialog_input(&data, &secrets),
            "DATA_KEY=gateway\nDATA_VAL=vpn.uzh.ch\n\nSECRET_KEY=form:main:group_list\nSECRET_VAL=staff \n\nDONE\n\n"
        );
    }

    /// A stand-in for nm-openconnect-auth-dialog: checks its arguments (complaining on standard
    /// error and exiting 9 when they're wrong, as no test expects), reads its input, then runs
    /// `then`.
    pub(crate) fn fake_dialog(dir: &Path, name: &str, then: &str) -> PathBuf {
        let path = dir.join(name);
        let script = format!(
            "#!/bin/sh\n[ \"$1 $3 $5 $7\" = '-u -n -s -i' ] || {{ echo 'wrong arguments' >&2; exit 9; }}\n\
             while read -r line; do [ \"$line\" = DONE ] && break; done\n{then}\n"
        );
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    /// Prints `answer`, then waits for QUIT and exits 0, as the real window does.
    pub(crate) fn answers(answer: &str) -> String {
        format!("printf '{answer}'\nwhile read -r line; do [ \"$line\" = QUIT ] && exit 0; done\nexit 0")
    }

    /// The window stays open until it's told to quit; it then prints its input back, no cookie.
    pub(crate) const STAYS_OPEN: &str = "while read -r line; do [ \"$line\" = QUIT ] && break; done\nprintf 'form:main:username\\nab12cd\\n\\n\\n'\nexit 0";

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lituus-oc-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run(dialog: &Path) -> Result<HashMap<String, String>, SignInError> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (data, secrets) = (HashMap::from([("gateway".to_owned(), "vpn.uzh.ch".to_owned())]), HashMap::new());
        // The sender goes away at once: that's no cancel.
        let (_, cancel) = oneshot::channel();
        runtime.block_on(sign_in(
            dialog,
            SignIn { uuid: "u-1", name: "UZH VPN", data: &data, secrets: &secrets, again: false },
            cancel,
        ))
    }

    #[test]
    fn signing_in_hands_back_the_session() {
        let dir = scratch("session");
        let ok = fake_dialog(
            &dir,
            "ok",
            &answers("cookie\\nwebvpn=abc\\ngateway\\nvpn.uzh.ch\\ngwcert\\npin-sha256:x\\nautoconnect\\n\\n\\n\\n"),
        );
        let out = run(&ok).unwrap();
        assert_eq!((out["cookie"].as_str(), out["gwcert"].as_str()), ("webvpn=abc", "pin-sha256:x"));
        assert_eq!(out["autoconnect"], "", "an empty value is still a value");
        // Closing the window prints no cookie, and exits 0.
        let closed = fake_dialog(&dir, "closed", &answers("gateway\\nvpn.uzh.ch\\n\\n\\n"));
        assert_eq!(run(&closed), Err(SignInError::Closed));
        // An error exits 1, and says why on standard error.
        let failed = fake_dialog(
            &dir,
            "failed",
            "echo 'starting' >&2\necho '(nm-openconnect-auth-dialog:7): Gtk-WARNING **: 12:00:00.000: cannot open display: :0' >&2\n\
             echo >&2\nexit 1",
        );
        assert_eq!(
            run(&failed),
            Err(SignInError::Failed("The sign-in window stopped: cannot open display: :0.".into()))
        );
        let silent = fake_dialog(&dir, "silent", "exit 3");
        assert_eq!(
            run(&silent),
            Err(SignInError::Failed("The sign-in window stopped with an error (exit status 3).".into()))
        );
        // Signing in again adds -r after the others.
        let quiet = fake_dialog(&dir, "quiet", "exit 0");
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (_, cancel) = oneshot::channel();
        let empty = HashMap::new();
        let again = SignIn { uuid: "u-1", name: "UZH VPN", data: &empty, secrets: &empty, again: true };
        assert_eq!(runtime.block_on(sign_in(&quiet, again, cancel)), Err(SignInError::Closed));
        assert!(matches!(run(&dir.join("missing")), Err(SignInError::Failed(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_closes_the_window() {
        let dir = scratch("cancel");
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let data = HashMap::new();
        // One that quits when asked, and one that has to be killed.
        for (dialog, at_most) in
            [(fake_dialog(&dir, "open", STAYS_OPEN), 1), (fake_dialog(&dir, "stubborn", "exec sleep 30"), 3)]
        {
            let signins = SignIns::default();
            let started = signins.start("u-1", "/org/freedesktop/NetworkManager/Settings/7").unwrap();
            let begun = std::time::Instant::now();
            let result = runtime.block_on(async {
                let sign_in = sign_in(
                    &dialog,
                    SignIn { uuid: "u-1", name: "UZH VPN", data: &data, secrets: &data, again: false },
                    started.cancel,
                );
                let cancel = async {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    signins.cancel("/org/freedesktop/NetworkManager/Settings/7");
                };
                tokio::join!(sign_in, cancel).0
            });
            assert_eq!(result, Err(SignInError::Cancelled), "{}", dialog.display());
            assert!(begun.elapsed().as_secs() < at_most, "{}: {:?}", dialog.display(), begun.elapsed());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_sign_in_per_connection() {
        let signins = SignIns::default();
        let first = signins.start("u-1", "/s/7").unwrap();
        assert!(signins.start("u-1", "/s/7").is_none(), "its window is open already");
        let other = signins.start("u-2", "/s/8").unwrap();
        // The first one ending leaves the other one alone.
        signins.finish(first.id);
        let again = signins.start("u-1", "/s/7").unwrap();
        signins.cancel("u-1");
        assert!(again.cancel.blocking_recv().is_ok());
        let mut other_cancel = other.cancel;
        assert!(other_cancel.try_recv().is_err(), "not cancelled");
        signins.cancel("/s/8");
        assert!(other_cancel.blocking_recv().is_ok());

        signins.set_ready("u-1", HashMap::from([("cookie".into(), "c".into())]));
        assert!(signins.has_ready("u-1"));
        assert!(signins.take_ready("u-1").is_some());
        assert!(signins.take_ready("u-1").is_none(), "a session is used once");
        // An old one is dropped.
        signins.set_ready("u-1", HashMap::new());
        signins.0.lock().unwrap().ready.get_mut("u-1").unwrap().0 -= READY_FOR;
        assert!(!signins.has_ready("u-1"));
    }

    #[test]
    fn imports_openconnect_saml_profiles() {
        let config = r#"
schema_version = 2
active_profile = "uzh"

[default_profile]
address = "vpn.uzh.ch"
user_group = "staff"
name = "UZH"

[profiles.uzh]
server = "https://vpn.uzh.ch"
user_group = "staff"
name = "UZH VPN"

[profiles.uzh.credentials]
username = "ab12cd@uzh.ch"

[profiles.lab]
server = "lab-vpn.example.org"
cert = "/home/u/lab.pem"
"#;
        assert!(is_saml_config("config.toml", config));
        let profiles = import_saml(config).unwrap();
        let names: Vec<&str> = profiles.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["lab", "UZH VPN"], "the default profile is the same server as uzh");
        assert_eq!(profiles[1].1.gateway, "https://vpn.uzh.ch/staff");
        assert_eq!(profiles[1].1.user_agent, USER_AGENT);
        assert_eq!(profiles[0].1.cert, "/home/u/lab.pem");
        let old = import_saml("[default_profile]\naddress = \"vpn.uzh.ch\"\nuser_group = \"\"\nname = \"\"\n").unwrap();
        assert_eq!(old, [("vpn.uzh.ch".to_owned(), OcForm { gateway: "vpn.uzh.ch".into(), ..OcForm::new() })]);
        // No address: no "/staff" for a gateway.
        assert!(import_saml("[default_profile]\naddress = \"\"\nuser_group = \"staff\"\n").is_err());
        assert!(import_saml("schema_version = 2\n").is_err());
        let broken = import_saml("schema_version = \n").unwrap_err();
        assert!(
            broken.starts_with("the file isn't valid TOML (") && broken.ends_with(").") && !broken.contains('\n'),
            "{broken}"
        );
        assert!(!is_saml_config("wg0.conf", "[Interface]\nPrivateKey = x\n"));
    }
}
