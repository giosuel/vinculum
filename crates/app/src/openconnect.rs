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

/// The plugin's sign-in window (`[GNOME] auth-dialog` in its `.name` file), when it's installed.
/// Debian and Fedora package it apart from the plugin (`…-gnome`).
pub fn auth_dialog() -> Option<PathBuf> {
    // In a Flatpak the plugin's folders are the sandbox's, and its window couldn't start anyway.
    if std::env::var_os("FLATPAK_ID").is_some() {
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

/// "Cisco AnyConnect" for a `protocol` value (no value means AnyConnect too).
pub fn protocol_label(protocol: &str) -> &'static str {
    PROTOCOLS.iter().find(|(p, _)| *p == protocol).map_or(PROTOCOLS[0].1, |(_, l)| *l)
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
    if initial.is_none() {
        for key in ONE_TIME {
            set(&format!("{key}-flags"), "2");
        }
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
    /// The window was closed (or the sign-in cancelled) without signing in.
    Closed,
    /// It couldn't run.
    Failed(String),
}

/// What the sign-in window reads on its standard input: the connection's settings and the
/// secrets kept from earlier, the way nm-applet and GNOME Shell send them.
fn dialog_input(data: &HashMap<String, String>, secrets: &HashMap<String, String>) -> String {
    let one_line = |v: &str| v.replace(['\n', '\r'], " ");
    let mut out = String::new();
    let mut sorted: Vec<_> = data.iter().collect();
    sorted.sort();
    for (k, v) in sorted {
        out.push_str(&format!("DATA_KEY={k}\nDATA_VAL={}\n\n", one_line(v)));
    }
    let mut sorted: Vec<_> = secrets.iter().collect();
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
    cancel: oneshot::Receiver<()>,
) -> Result<HashMap<String, String>, SignInError> {
    let mut args = vec!["-u", s.uuid, "-n", s.name, "-s", SERVICE, "-i"];
    if s.again {
        args.push("-r");
    }
    let mut child = tokio::process::Command::new(dialog)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| SignInError::Failed(format!("Couldn't open the sign-in window ({}): {e}.", dialog.display())))?;
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
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
        _ = cancel => return Err(SignInError::Closed),
    };
    // It waits for QUIT before it exits.
    let _ = stdin.write_all(b"QUIT\n\n").await;
    drop(stdin);
    let status = child.wait().await.map_err(|e| SignInError::Failed(format!("The sign-in window failed: {e}.")))?;
    if !status.success() || out.get("cookie").is_none_or(String::is_empty) {
        return Err(SignInError::Closed);
    }
    Ok(out)
}

/// How long a session from signing in waits for NetworkManager's request; one older than this
/// belongs to an attempt that went nowhere.
const READY_FOR: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Default)]
struct Pending {
    /// Sessions from signing in before connecting, by connection UUID, for NetworkManager's next
    /// request, with when they came.
    ready: HashMap<String, (std::time::Instant, HashMap<String, String>)>,
    /// Sign-ins whose window is open: (UUID, connection path, how to close it).
    running: Vec<(String, String, oneshot::Sender<()>)>,
}

/// Sign-ins in progress and their sessions, shared by the secret agent and the commands.
#[derive(Clone, Default)]
pub struct SignIns(Arc<Mutex<Pending>>);

impl SignIns {
    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set_ready(&self, uuid: &str, secrets: HashMap<String, String>) {
        self.lock().ready.insert(uuid.to_owned(), (std::time::Instant::now(), secrets));
    }

    pub fn take_ready(&self, uuid: &str) -> Option<HashMap<String, String>> {
        self.lock().ready.remove(uuid).filter(|(at, _)| at.elapsed() < READY_FOR).map(|(_, secrets)| secrets)
    }

    /// A sign-in starts; the receiver hears when it's cancelled.
    pub fn start(&self, uuid: &str, path: &str) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.lock().running.push((uuid.to_owned(), path.to_owned(), tx));
        rx
    }

    pub fn finish(&self, uuid: &str) {
        self.lock().running.retain(|(u, _, _)| u != uuid);
    }

    /// Closes the sign-in window of a connection, by UUID or by its settings path.
    pub fn cancel(&self, uuid_or_path: &str) {
        let mut pending = self.lock();
        let (cancelled, kept) = std::mem::take(&mut pending.running)
            .into_iter()
            .partition(|(u, p, _)| u == uuid_or_path || p == uuid_or_path);
        pending.running = kept;
        for (_, _, tx) in cancelled {
            let _ = tx.send(());
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
    let table: toml::Table = text.parse().map_err(|e| format!("The file isn't valid TOML: {e}"))?;
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
    if let Some(p) = table.get("default_profile").and_then(toml::Value::as_table) {
        let gateway = with_group(&string(p, "address"), &string(p, "user_group"));
        let bare = |g: &str| g.trim_start_matches("https://").trim_end_matches('/').to_lowercase();
        if !gateway.is_empty() && !out.iter().any(|(_, f)| bare(&f.gateway) == bare(&gateway)) {
            let name = Some(string(p, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| gateway.clone());
            out.push((name, OcForm { gateway, ..OcForm::new() }));
        }
    }
    if out.is_empty() {
        return Err("The file has no openconnect-saml profiles (a [profiles.…] section with a server).".into());
    }
    Ok(out)
}

/// Whether a file looks like openconnect-saml's config rather than a WireGuard `.conf`.
pub fn is_saml_config(file_name: &str, text: &str) -> bool {
    file_name.ends_with(".toml")
        || text.lines().any(|l| l.trim_start().starts_with("[profiles.") || l.trim() == "[default_profile]")
}

#[cfg(test)]
mod tests {
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
        let f = OcForm {
            gateway: "vpn.uzh.ch/staff".into(),
            protocol: 1,
            system: 1,
            csd: "/usr/libexec/openconnect/csd-post.sh".into(),
            udp: false,
            ..OcForm::new()
        };
        let mut s: Settings = HashMap::new();
        write(&mut s, None, &f);
        assert!(is_openconnect(&s));
        let d = data(&s);
        assert_eq!(d["gateway"], "vpn.uzh.ch/staff");
        assert_eq!((d["protocol"].as_str(), d["reported_os"].as_str()), ("gp", "win"));
        assert_eq!((d["useragent"].as_str(), d["authtype"].as_str()), ("AnyConnect", "password"));
        assert_eq!((d["enable_csd_trojan"].as_str(), d["disable_udp"].as_str()), ("yes", "yes"));
        assert_eq!(d["cookie-flags"], "2");
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
        assert_eq!(check(&OcForm::new()).0, "Type the VPN server, such as vpn.example.org.");
        assert!(!check(&OcForm { gateway: "a".into(), mtu: "40".into(), ..OcForm::new() }).1.is_empty());
    }

    #[test]
    fn the_dialog_gets_settings_and_kept_secrets() {
        let data = HashMap::from([("gateway".to_owned(), "vpn.uzh.ch".to_owned())]);
        let secrets = HashMap::from([("form:main:group_list".to_owned(), "staff\n".to_owned())]);
        assert_eq!(
            dialog_input(&data, &secrets),
            "DATA_KEY=gateway\nDATA_VAL=vpn.uzh.ch\n\nSECRET_KEY=form:main:group_list\nSECRET_VAL=staff \n\nDONE\n\n"
        );
    }

    /// A stand-in for nm-openconnect-auth-dialog: checks its input and answers the way it does.
    fn fake_dialog(dir: &Path, name: &str, answer: &str, exit: i32) -> PathBuf {
        let path = dir.join(name);
        // It waits for QUIT once it has answered; a failing one just exits.
        let wait = if exit == 0 { "while read -r line; do [ \"$line\" = QUIT ] && break; done\n" } else { "" };
        let script = format!(
            "#!/bin/sh\n[ \"$1 $3 $5 $7\" = '-u -n -s -i' ] || exit 9\n\
             while read -r line; do [ \"$line\" = DONE ] && break; done\n\
             printf '{answer}'\n{wait}exit {exit}\n"
        );
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn run(dialog: &Path) -> Result<HashMap<String, String>, SignInError> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (data, secrets) = (HashMap::from([("gateway".to_owned(), "vpn.uzh.ch".to_owned())]), HashMap::new());
        let (_keep, cancel) = oneshot::channel();
        runtime.block_on(sign_in(
            dialog,
            SignIn { uuid: "u-1", name: "UZH VPN", data: &data, secrets: &secrets, again: false },
            cancel,
        ))
    }

    #[test]
    fn signing_in_hands_back_the_session() {
        let dir = std::env::temp_dir().join(format!("lituus-oc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = fake_dialog(
            &dir,
            "ok",
            "cookie\\nwebvpn=abc\\ngateway\\nvpn.uzh.ch\\ngwcert\\npin-sha256:x\\nautoconnect\\n\\n\\n\\n",
            0,
        );
        let out = run(&ok).unwrap();
        assert_eq!((out["cookie"].as_str(), out["gwcert"].as_str()), ("webvpn=abc", "pin-sha256:x"));
        assert_eq!(out["autoconnect"], "", "an empty value is still a value");
        // Closing the window prints no cookie, and exits 0.
        let closed = fake_dialog(&dir, "closed", "gateway\\nvpn.uzh.ch\\n\\n\\n", 0);
        assert_eq!(run(&closed), Err(SignInError::Closed));
        let failed = fake_dialog(&dir, "failed", "", 1);
        assert_eq!(run(&failed), Err(SignInError::Closed));
        assert!(matches!(run(&dir.join("missing")), Err(SignInError::Failed(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_closes_the_window() {
        let signins = SignIns::default();
        let rx = signins.start("u-1", "/org/freedesktop/NetworkManager/Settings/7");
        signins.cancel("/org/freedesktop/NetworkManager/Settings/7");
        assert!(rx.blocking_recv().is_ok());
        signins.set_ready("u-1", HashMap::from([("cookie".into(), "c".into())]));
        assert!(signins.take_ready("u-1").is_some());
        assert!(signins.take_ready("u-1").is_none(), "a session is used once");
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
        assert!(import_saml("schema_version = 2\n").is_err());
        assert!(!is_saml_config("wg0.conf", "[Interface]\nPrivateKey = x\n"));
    }
}
