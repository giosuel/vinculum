//! OpenVPN through NetworkManager-openvpn: its settings (the `vpn` setting's `data` and
//! `secrets` maps) as editor fields, and `.ovpn` files turned into them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vinculum_nm::Settings;
use zbus::zvariant::{OwnedValue, Value};

pub const SERVICE: &str = "org.freedesktop.NetworkManager.openvpn";

/// `connection-type`, by the Authentication choice.
pub const AUTH: [(&str, &str); 4] = [
    ("password", "Password"),
    ("tls", "Certificates (TLS)"),
    ("password-tls", "Password + certificates"),
    ("static-key", "Static key"),
];
/// How the TLS key is used, by index: (data key, label).
pub const TLS_KEY_MODES: [(&str, &str); 3] =
    [("ta", "tls-auth"), ("tls-crypt", "tls-crypt"), ("tls-crypt-v2", "tls-crypt-v2")];
/// `compress` (or the older `comp-lzo`) by index.
pub const COMPRESSION: [(&str, &str); 4] = [("", "No"), ("lz4", "LZ4"), ("lzo", "LZO"), ("adaptive", "Adaptive")];

/// Whether NetworkManager-openvpn is installed (NetworkManager lists its VPN plugins in these
/// folders).
pub fn plugin_installed() -> bool {
    // In a Flatpak these are the sandbox's folders, not the system's: let NetworkManager say so
    // when the plugin is missing.
    if std::env::var_os("FLATPAK_ID").is_some() {
        return true;
    }
    [
        "/usr/lib/NetworkManager/VPN",
        "/usr/lib64/NetworkManager/VPN",
        "/etc/NetworkManager/VPN",
        "/usr/local/lib/NetworkManager/VPN",
    ]
    .iter()
    .any(|dir| Path::new(dir).join("nm-openvpn-service.name").exists())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OvpnForm {
    /// host:port, several separated by commas.
    pub gateway: String,
    pub tcp: bool,
    /// Index into AUTH.
    pub auth: usize,
    pub username: String,
    /// New secrets; empty leaves the saved ones.
    pub password: String,
    pub key_password: String,
    /// 0 for me only (keyring), 1 all users, 2 ask each time.
    pub storage: usize,
    pub ca: String,
    pub cert: String,
    pub key: String,
    pub static_key: String,
    /// The TLS key file, its mode (index into TLS_KEY_MODES) and direction ("", "0", "1").
    pub tls_key: String,
    pub tls_key_mode: usize,
    pub tls_key_direction: String,
    pub cipher: String,
    pub compression: usize,
    pub mtu: String,
    pub fragment: String,
    pub reneg: String,
    pub tap: bool,
}

fn data(s: &Settings) -> HashMap<String, String> {
    s.get("vpn")
        .and_then(|v| v.get("data"))
        .and_then(|d| <HashMap<String, String>>::try_from(d.clone()).ok())
        .unwrap_or_default()
}

pub fn is_openvpn(s: &Settings) -> bool {
    s.get("vpn").and_then(|v| v.get("service-type")).and_then(|t| <&str>::try_from(&**t).ok()) == Some(SERVICE)
}

pub fn read(s: &Settings) -> OvpnForm {
    let d = data(s);
    let get = |k: &str| d.get(k).cloned().unwrap_or_default();
    let flags: u32 =
        d.get("password-flags").or_else(|| d.get("cert-pass-flags")).and_then(|f| f.parse().ok()).unwrap_or(0);
    let (tls_key, tls_key_mode) = if !get("tls-crypt-v2").is_empty() {
        (get("tls-crypt-v2"), 2)
    } else if !get("tls-crypt").is_empty() {
        (get("tls-crypt"), 1)
    } else {
        (get("ta"), 0)
    };
    let compression = match (get("compress").as_str(), get("comp-lzo").as_str()) {
        ("lz4" | "lz4-v2", _) => 1,
        ("lzo", _) | (_, "yes") => 2,
        (_, "adaptive") => 3,
        _ => 0,
    };
    OvpnForm {
        gateway: get("remote"),
        tcp: get("proto-tcp") == "yes",
        auth: AUTH.iter().position(|(t, _)| *t == get("connection-type")).unwrap_or(0),
        username: get("username"),
        password: String::new(),
        key_password: String::new(),
        storage: if flags & 1 != 0 {
            0
        } else if flags & 2 != 0 {
            2
        } else {
            1
        },
        ca: get("ca"),
        cert: get("cert"),
        key: get("key"),
        static_key: get("static-key"),
        tls_key,
        tls_key_mode,
        tls_key_direction: if tls_key_mode == 0 { get("ta-dir") } else { String::new() },
        cipher: get("cipher"),
        compression,
        mtu: get("tunnel-mtu"),
        fragment: get("fragment-size"),
        reneg: get("reneg-seconds"),
        tap: get("dev-type") == "tap",
    }
}

/// Writes the form into the `vpn` setting, changing only the keys that differ from `initial`.
pub fn write(s: &mut Settings, initial: Option<&OvpnForm>, f: &OvpnForm) {
    let mut d = data(s);
    let mut secrets: HashMap<String, String> = s
        .get("vpn")
        .and_then(|v| v.get("secrets"))
        .and_then(|x| <HashMap<String, String>>::try_from(x.clone()).ok())
        .unwrap_or_default();
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
    if edited!(gateway) {
        set("remote", &f.gateway);
    }
    if edited!(tcp) {
        set("proto-tcp", if f.tcp { "yes" } else { "" });
    }
    if edited!(auth) {
        set("connection-type", AUTH[f.auth].0);
    }
    if edited!(username) {
        set("username", &f.username);
    }
    for (key, value, changed) in [
        ("ca", &f.ca, edited!(ca)),
        ("cert", &f.cert, edited!(cert)),
        ("key", &f.key, edited!(key)),
        ("static-key", &f.static_key, edited!(static_key)),
        ("cipher", &f.cipher, edited!(cipher)),
        ("tunnel-mtu", &f.mtu, edited!(mtu)),
        ("fragment-size", &f.fragment, edited!(fragment)),
        ("reneg-seconds", &f.reneg, edited!(reneg)),
    ] {
        if changed {
            set(key, value);
        }
    }
    if edited!(tls_key) || edited!(tls_key_mode) || edited!(tls_key_direction) {
        for (key, _) in TLS_KEY_MODES {
            set(key, "");
        }
        set("ta-dir", "");
        set(TLS_KEY_MODES[f.tls_key_mode].0, &f.tls_key);
        if f.tls_key_mode == 0 && !f.tls_key.trim().is_empty() {
            set("ta-dir", &f.tls_key_direction);
        }
    }
    if edited!(compression) {
        set("comp-lzo", "");
        set("compress", COMPRESSION[f.compression].0);
    }
    if edited!(tap) {
        set("dev-type", if f.tap { "tap" } else { "" });
    }
    let flags = match f.storage {
        0 => "1",
        2 => "2",
        _ => "0",
    };
    let uses_password = matches!(AUTH[f.auth].0, "password" | "password-tls");
    let uses_cert = matches!(AUTH[f.auth].0, "tls" | "password-tls");
    if edited!(storage) || edited!(auth) || !f.password.is_empty() || !f.key_password.is_empty() {
        if uses_password {
            set("password-flags", flags);
        }
        if uses_cert {
            set("cert-pass-flags", flags);
        }
    }
    if f.storage != 2 {
        if uses_password && !f.password.is_empty() {
            secrets.insert("password".into(), f.password.clone());
        }
        if uses_cert && !f.key_password.is_empty() {
            secrets.insert("cert-pass".into(), f.key_password.clone());
        }
    }
    let vpn = s.entry("vpn".into()).or_default();
    vpn.insert("service-type".into(), OwnedValue::try_from(Value::from(SERVICE)).expect("a string"));
    vpn.insert("data".into(), OwnedValue::try_from(Value::from(d)).expect("a dict"));
    if !secrets.is_empty() {
        vpn.insert("secrets".into(), OwnedValue::try_from(Value::from(secrets)).expect("a dict"));
    }
}

/// What's wrong with the form, as (username, secret, files) problems.
pub fn check(initial: Option<&OvpnForm>, f: &OvpnForm) -> (String, String, String) {
    let mut problems = (String::new(), String::new(), String::new());
    let (auth, _) = AUTH[f.auth];
    if f.gateway.trim().is_empty() {
        problems.2 = "Type the VPN server (gateway), such as vpn.example.org:1194.".into();
        return problems;
    }
    if matches!(auth, "password" | "password-tls") && f.username.trim().is_empty() {
        problems.0 = "Type your username.".into();
    }
    if matches!(auth, "tls" | "password-tls") && (f.cert.trim().is_empty() || f.key.trim().is_empty()) {
        problems.2 = "Certificates need your certificate and private key files.".into();
    } else if auth == "static-key" && f.static_key.trim().is_empty() {
        problems.2 = "Choose the static key file.".into();
    }
    let _ = initial;
    problems
}

/// A `.ovpn` file, turned into NetworkManager-openvpn's settings. Certificates and keys written
/// inline (`<ca>…</ca>`) are saved as files in `cert_dir`, since the plugin wants paths.
pub struct Imported {
    pub name: String,
    pub form: OvpnForm,
    /// Files to write: (path, contents).
    pub files: Vec<(PathBuf, String)>,
    /// Directives the plugin has no setting for.
    pub ignored: Vec<String>,
}

pub fn import(text: &str, file_name: &str, cert_dir: &Path) -> Result<Imported, String> {
    let stem = Path::new(file_name).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let name = stem.trim_end_matches(".ovpn").trim_end_matches(".conf").to_owned();
    let base = Path::new(file_name).parent().map(Path::to_path_buf).unwrap_or_default();
    let mut f = OvpnForm { storage: 0, ..OvpnForm::default() };
    let mut remotes = Vec::new();
    let mut port = String::new();
    let mut files = Vec::new();
    let mut ignored = Vec::new();
    let mut auth_user_pass = false;
    let mut lines = text.lines().peekable();
    let path_of = |p: &str| -> String {
        let p = p.trim().trim_matches('"');
        let path = Path::new(p);
        if path.is_absolute() { p.to_owned() } else { base.join(path).to_string_lossy().into_owned() }
    };
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        // Inline files: <ca> … </ca>.
        if let Some(tag) = line.strip_prefix('<').and_then(|l| l.strip_suffix('>')).filter(|t| !t.starts_with('/')) {
            let mut body = String::new();
            for inner in lines.by_ref() {
                if inner.trim() == format!("</{tag}>") {
                    break;
                }
                body.push_str(inner);
                body.push('\n');
            }
            let file = cert_dir.join(format!("{tag}.pem"));
            let path = file.to_string_lossy().into_owned();
            match tag {
                "ca" => f.ca = path,
                "cert" => f.cert = path,
                "key" => f.key = path,
                "tls-auth" => {
                    f.tls_key = path;
                    f.tls_key_mode = 0;
                }
                "tls-crypt" => {
                    f.tls_key = path;
                    f.tls_key_mode = 1;
                }
                "tls-crypt-v2" => {
                    f.tls_key = path;
                    f.tls_key_mode = 2;
                }
                "secret" => f.static_key = path,
                other => {
                    ignored.push(format!("<{other}>"));
                    continue;
                }
            }
            files.push((file, body));
            continue;
        }
        let mut words = line.split_whitespace();
        let Some(directive) = words.next() else { continue };
        let args: Vec<&str> = words.collect();
        let arg = |i: usize| args.get(i).copied().unwrap_or("");
        match directive {
            "remote" => {
                let host = arg(0);
                let p = if arg(1).is_empty() { "1194" } else { arg(1) };
                remotes.push(format!("{host}:{p}"));
                if arg(2).starts_with("tcp") {
                    f.tcp = true;
                }
            }
            "port" => port = arg(0).to_owned(),
            "proto" => f.tcp = arg(0).starts_with("tcp"),
            "dev" => f.tap = arg(0).starts_with("tap"),
            "dev-type" => f.tap = arg(0) == "tap",
            "ca" => f.ca = path_of(arg(0)),
            "cert" => f.cert = path_of(arg(0)),
            "key" => f.key = path_of(arg(0)),
            "secret" => f.static_key = path_of(arg(0)),
            "tls-auth" => {
                f.tls_key = path_of(arg(0));
                f.tls_key_mode = 0;
                f.tls_key_direction = arg(1).to_owned();
            }
            "tls-crypt" => {
                f.tls_key = path_of(arg(0));
                f.tls_key_mode = 1;
            }
            "tls-crypt-v2" => {
                f.tls_key = path_of(arg(0));
                f.tls_key_mode = 2;
            }
            "key-direction" => f.tls_key_direction = arg(0).to_owned(),
            "auth-user-pass" => auth_user_pass = true,
            "cipher" | "data-ciphers" => {
                if f.cipher.is_empty() {
                    f.cipher = arg(0).split(':').next().unwrap_or("").to_owned();
                }
            }
            "comp-lzo" => {
                f.compression = if arg(0) == "no" {
                    0
                } else if arg(0) == "adaptive" {
                    3
                } else {
                    2
                }
            }
            "compress" => {
                f.compression = if arg(0).starts_with("lz4") {
                    1
                } else if arg(0) == "lzo" {
                    2
                } else {
                    0
                }
            }
            "tun-mtu" => f.mtu = arg(0).to_owned(),
            "fragment" => f.fragment = arg(0).to_owned(),
            "reneg-sec" => f.reneg = arg(0).to_owned(),
            // Defaults of a client, or things NetworkManager does itself.
            "client"
            | "nobind"
            | "persist-key"
            | "persist-tun"
            | "verb"
            | "mute"
            | "resolv-retry"
            | "remote-cert-tls"
            | "auth-nocache"
            | "pull"
            | "tls-client"
            | "redirect-gateway"
            | "remote-random"
            | "float"
            | "mute-replay-warnings" => {}
            other => ignored.push(other.to_owned()),
        }
    }
    if remotes.is_empty() {
        return Err("The file has no remote (the VPN server).".into());
    }
    if !port.is_empty() {
        for r in &mut remotes {
            if r.ends_with(":1194") {
                *r = format!("{}:{port}", r.trim_end_matches(":1194"));
            }
        }
    }
    f.gateway = remotes.join(", ");
    let has_cert = !f.cert.is_empty() && !f.key.is_empty();
    f.auth = if !f.static_key.is_empty() && !has_cert && !auth_user_pass {
        3
    } else if auth_user_pass && has_cert {
        2
    } else if has_cert {
        1
    } else {
        0
    };
    ignored.sort();
    ignored.dedup();
    Ok(Imported { name, form: f, files, ignored })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
client
dev tun
proto udp
remote vpn.uzh.ch 1194
remote vpn2.uzh.ch 443 tcp
auth-user-pass
cipher AES-256-GCM
compress lz4-v2
reneg-sec 0
script-security 2
<ca>
-----BEGIN CERTIFICATE-----
MIIB
-----END CERTIFICATE-----
</ca>
<tls-crypt>
-----BEGIN OpenVPN Static key V1-----
abcd
-----END OpenVPN Static key V1-----
</tls-crypt>
";

    #[test]
    fn imports_an_ovpn_file() {
        let dir = Path::new("/home/u/.local/share/vinculum/certificates/uni-zurich");
        let i = import(FILE, "uni-zurich.ovpn", dir).unwrap();
        assert_eq!(i.name, "uni-zurich");
        assert_eq!(i.form.gateway, "vpn.uzh.ch:1194, vpn2.uzh.ch:443");
        assert!(i.form.tcp, "one remote asks for TCP");
        assert_eq!(AUTH[i.form.auth].0, "password");
        assert_eq!(i.form.ca, dir.join("ca.pem").to_string_lossy());
        assert_eq!((i.form.tls_key_mode, i.form.compression), (1, 1));
        assert_eq!(i.files.len(), 2);
        assert!(i.files[0].1.contains("BEGIN CERTIFICATE"));
        assert_eq!(i.ignored, ["script-security"]);
        assert_eq!(check(None, &i.form).0, "Type your username.");
    }

    #[test]
    fn settings_round_trip() {
        let i = import(FILE, "uni-zurich.ovpn", Path::new("/c")).unwrap();
        let mut f = i.form.clone();
        f.username = "ab12cd".into();
        f.password = "secret".into();
        let mut s: Settings = HashMap::new();
        write(&mut s, None, &f);
        assert!(is_openvpn(&s));
        let d = data(&s);
        assert_eq!(d["remote"], "vpn.uzh.ch:1194, vpn2.uzh.ch:443");
        assert_eq!(d["password-flags"], "1");
        assert_eq!(d["tls-crypt"], "/c/tls-crypt.pem");
        let back = read(&s);
        assert_eq!(back, OvpnForm { password: String::new(), ..f.clone() });
        // Changing the gateway leaves the rest alone.
        let mut g = back.clone();
        g.gateway = "vpn3.uzh.ch:1194".into();
        let before = data(&s);
        write(&mut s, Some(&back), &g);
        let after = data(&s);
        assert_eq!(after["remote"], "vpn3.uzh.ch:1194");
        assert_eq!(after.len(), before.len());
    }
}
