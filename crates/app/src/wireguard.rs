//! WireGuard: `.conf` files (the wg-quick format) in and out, keys, and the NetworkManager
//! settings for a WireGuard connection.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use lituus_nm::Settings;
use x25519_dalek::{PublicKey, StaticSecret};
use zbus::zvariant::{OwnedValue, Value};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub public_key: String,
    pub preshared_key: String,
    /// host:port, or empty when the peer connects to us.
    pub endpoint: String,
    pub allowed_ips: Vec<String>,
    /// Seconds; 0 is off.
    pub keepalive: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conf {
    pub private_key: String,
    /// With prefixes, IPv4 and IPv6 mixed.
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    /// DNS entries that are search domains rather than servers.
    pub search: Vec<String>,
    pub listen_port: u32,
    pub mtu: u32,
    pub fwmark: u32,
    pub peers: Vec<Peer>,
}

fn list(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).collect()
}

fn number(key: &str, value: &str) -> Result<u32, String> {
    let value = value.trim();
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    };
    parsed.ok_or_else(|| format!("{key} = {value} isn't a number"))
}

/// Reads a wg-quick `.conf` file. Keys wg-quick runs as commands (PostUp and the like) are
/// ignored; NetworkManager doesn't run them.
pub fn parse(text: &str) -> Result<Conf, String> {
    let mut conf = Conf::default();
    let mut section = "";
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = match line.to_ascii_lowercase().as_str() {
                "[interface]" => "interface",
                "[peer]" => {
                    conf.peers.push(Peer::default());
                    "peer"
                }
                _ => return Err(format!("Line {}: unknown section {line}", n + 1)),
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("Line {}: expected key = value", n + 1));
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
        match (section, key.as_str()) {
            ("interface", "privatekey") => conf.private_key = value.to_owned(),
            ("interface", "address") => conf.addresses.extend(list(value)),
            ("interface", "dns") => {
                for entry in list(value) {
                    if entry.parse::<std::net::IpAddr>().is_ok() {
                        conf.dns.push(entry)
                    } else {
                        conf.search.push(entry)
                    }
                }
            }
            ("interface", "listenport") => conf.listen_port = number("ListenPort", value)?,
            ("interface", "mtu") => conf.mtu = number("MTU", value)?,
            ("interface", "fwmark") => conf.fwmark = if value == "off" { 0 } else { number("FwMark", value)? },
            ("interface", _) => {}
            ("peer", k) => {
                let peer = conf.peers.last_mut().expect("a [Peer] section");
                match k {
                    "publickey" => peer.public_key = value.to_owned(),
                    "presharedkey" => peer.preshared_key = value.to_owned(),
                    "endpoint" => peer.endpoint = value.to_owned(),
                    "allowedips" => peer.allowed_ips.extend(list(value)),
                    "persistentkeepalive" => {
                        peer.keepalive = if value == "off" { 0 } else { number("PersistentKeepalive", value)? }
                    }
                    _ => {}
                }
            }
            _ => return Err(format!("Line {}: {key} outside a section", n + 1)),
        }
    }
    if conf.private_key.is_empty() {
        return Err("The file has no PrivateKey in [Interface].".into());
    }
    if public_key(&conf.private_key).is_none() {
        return Err("The PrivateKey isn't a WireGuard key.".into());
    }
    if let Some(bad) = conf.peers.iter().find(|p| !is_key(&p.public_key)) {
        return Err(format!("The peer key {} isn't a WireGuard key.", bad.public_key));
    }
    Ok(conf)
}

/// Writes a wg-quick `.conf` file.
pub fn export(conf: &Conf) -> String {
    let mut out = String::from("[Interface]\n");
    out += &format!("PrivateKey = {}\n", conf.private_key);
    if !conf.addresses.is_empty() {
        out += &format!("Address = {}\n", conf.addresses.join(", "));
    }
    let dns: Vec<&str> = conf.dns.iter().chain(&conf.search).map(String::as_str).collect();
    if !dns.is_empty() {
        out += &format!("DNS = {}\n", dns.join(", "));
    }
    if conf.listen_port > 0 {
        out += &format!("ListenPort = {}\n", conf.listen_port);
    }
    if conf.mtu > 0 {
        out += &format!("MTU = {}\n", conf.mtu);
    }
    if conf.fwmark > 0 {
        out += &format!("FwMark = {:#x}\n", conf.fwmark);
    }
    for p in &conf.peers {
        out += &format!("\n[Peer]\nPublicKey = {}\n", p.public_key);
        if !p.preshared_key.is_empty() {
            out += &format!("PresharedKey = {}\n", p.preshared_key);
        }
        if !p.endpoint.is_empty() {
            out += &format!("Endpoint = {}\n", p.endpoint);
        }
        if !p.allowed_ips.is_empty() {
            out += &format!("AllowedIPs = {}\n", p.allowed_ips.join(", "));
        }
        if p.keepalive > 0 {
            out += &format!("PersistentKeepalive = {}\n", p.keepalive);
        }
    }
    out
}

fn decode(key: &str) -> Option<[u8; 32]> {
    B64.decode(key.trim()).ok()?.try_into().ok()
}

/// A base64 Curve25519 key.
pub fn is_key(key: &str) -> bool {
    decode(key).is_some()
}

/// The public key that goes with a private one.
pub fn public_key(private: &str) -> Option<String> {
    let secret = StaticSecret::from(decode(private)?);
    Some(B64.encode(PublicKey::from(&secret).as_bytes()))
}

/// A new private key, from the kernel's random numbers.
pub fn generate() -> String {
    let mut bytes = [0u8; 32];
    if let Ok(mut random) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut random, &mut bytes);
    }
    // Clamped the way `wg genkey` does it.
    bytes[0] &= 248;
    bytes[31] = (bytes[31] & 127) | 64;
    B64.encode(bytes)
}

fn put(s: &mut Settings, setting: &str, key: &str, value: Value<'_>) {
    s.entry(setting.to_owned())
        .or_default()
        .insert(key.to_owned(), OwnedValue::try_from(value).expect("no file descriptors"));
}

/// NetworkManager's `wireguard.peers`.
pub fn peers_value(peers: &[Peer]) -> Value<'static> {
    let peers: Vec<HashMap<String, Value<'static>>> = peers
        .iter()
        .map(|p| {
            let mut d = HashMap::from([
                ("public-key".to_owned(), Value::from(p.public_key.clone())),
                ("allowed-ips".to_owned(), Value::from(p.allowed_ips.clone())),
            ]);
            if !p.endpoint.is_empty() {
                d.insert("endpoint".into(), Value::from(p.endpoint.clone()));
            }
            if p.keepalive > 0 {
                d.insert("persistent-keepalive".into(), Value::from(p.keepalive));
            }
            if !p.preshared_key.is_empty() {
                d.insert("preshared-key".into(), Value::from(p.preshared_key.clone()));
                d.insert("preshared-key-flags".into(), Value::from(0u32));
            }
            d
        })
        .collect();
    Value::from(peers)
}

/// The settings of a new WireGuard connection from a `.conf` file, named `name` (the file's
/// name, which wg-quick also uses for the interface). The keys are kept by NetworkManager; the
/// connection belongs to `owner` when given.
pub fn settings(conf: &Conf, name: &str, uuid: &str, owner: Option<&str>) -> Settings {
    let mut s: Settings = HashMap::new();
    put(&mut s, "connection", "id", Value::from(name));
    put(&mut s, "connection", "uuid", Value::from(uuid));
    put(&mut s, "connection", "type", Value::from("wireguard"));
    // Interface names have at most 15 characters.
    let interface: String =
        name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(15).collect();
    put(
        &mut s,
        "connection",
        "interface-name",
        Value::from(if interface.is_empty() { "wg0".to_owned() } else { interface }),
    );
    put(&mut s, "connection", "autoconnect", Value::from(false));
    if let Some(owner) = owner {
        put(&mut s, "connection", "permissions", Value::from(vec![format!("user:{owner}:")]));
    }
    put(&mut s, "wireguard", "private-key", Value::from(conf.private_key.as_str()));
    put(&mut s, "wireguard", "private-key-flags", Value::from(0u32));
    if conf.listen_port > 0 {
        put(&mut s, "wireguard", "listen-port", Value::from(conf.listen_port));
    }
    if conf.mtu > 0 {
        put(&mut s, "wireguard", "mtu", Value::from(conf.mtu));
    }
    if conf.fwmark > 0 {
        put(&mut s, "wireguard", "fwmark", Value::from(conf.fwmark));
    }
    put(&mut s, "wireguard", "peer-routes", Value::from(true));
    put(&mut s, "wireguard", "peers", peers_value(&conf.peers));
    for (setting, v6) in [("ipv4", false), ("ipv6", true)] {
        let addresses: Vec<HashMap<&str, Value>> = conf
            .addresses
            .iter()
            .filter_map(|a| {
                let (ip, prefix) = a.split_once('/').unwrap_or((a.as_str(), if v6 { "128" } else { "32" }));
                let ip: std::net::IpAddr = ip.parse().ok()?;
                (ip.is_ipv6() == v6).then(|| {
                    HashMap::from([
                        ("address", Value::from(ip.to_string())),
                        ("prefix", Value::from(prefix.parse::<u32>().unwrap_or(32))),
                    ])
                })
            })
            .collect();
        let dns: Vec<std::net::IpAddr> =
            conf.dns.iter().filter_map(|d| d.parse().ok()).filter(|d: &std::net::IpAddr| d.is_ipv6() == v6).collect();
        if addresses.is_empty() {
            put(&mut s, setting, "method", Value::from(if v6 { "ignore" } else { "disabled" }));
            continue;
        }
        put(&mut s, setting, "method", Value::from("manual"));
        put(&mut s, setting, "address-data", Value::from(addresses));
        if v6 {
            let dns: Vec<Vec<u8>> = dns
                .iter()
                .filter_map(|d| if let std::net::IpAddr::V6(a) = d { Some(a.octets().to_vec()) } else { None })
                .collect();
            put(&mut s, setting, "dns", Value::from(dns));
        } else {
            let dns: Vec<u32> = dns
                .iter()
                .filter_map(
                    |d| if let std::net::IpAddr::V4(a) = d { Some(u32::from_ne_bytes(a.octets())) } else { None },
                )
                .collect();
            put(&mut s, setting, "dns", Value::from(dns));
        }
        if !conf.search.is_empty() {
            put(&mut s, setting, "dns-search", Value::from(conf.search.clone()));
        }
        // wg-quick sends everything through the tunnel when a peer allows 0.0.0.0/0; a low
        // priority keeps the tunnel's DNS in charge too.
        if !dns.is_empty() {
            put(&mut s, setting, "dns-priority", Value::from(-50i32));
        }
    }
    s
}

fn dicts(v: Option<&OwnedValue>) -> Vec<HashMap<String, OwnedValue>> {
    v.and_then(|v| <Vec<HashMap<String, OwnedValue>>>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

fn text(d: &HashMap<String, OwnedValue>, key: &str) -> String {
    d.get(key).and_then(|v| <&str>::try_from(&**v).ok()).map(str::to_owned).unwrap_or_default()
}

/// The pre-shared keys in a `GetSecrets("wireguard")` answer, by peer public key.
pub fn preshared_keys(secrets: &Settings) -> HashMap<String, String> {
    dicts(secrets.get("wireguard").and_then(|w| w.get("peers")))
        .iter()
        .map(|p| (text(p, "public-key"), text(p, "preshared-key")))
        .filter(|(_, psk)| !psk.is_empty())
        .collect()
}

/// Puts the saved pre-shared keys back into edited peers, which the editor writes without them.
pub fn keep_preshared_keys(settings: &mut Settings, secrets: &Settings) {
    let keys = preshared_keys(secrets);
    let Some(wg) = settings.get_mut("wireguard") else { return };
    let peers = dicts(wg.get("peers"));
    if keys.is_empty() || peers.is_empty() {
        return;
    }
    let peers: Vec<HashMap<String, Value<'static>>> = peers
        .into_iter()
        .map(|p| {
            let mut out: HashMap<String, Value<'static>> =
                p.iter().map(|(k, v)| (k.clone(), Value::from(v.try_clone().expect("no fds")))).collect();
            // Value::from(OwnedValue) nests a variant: unwrap it again.
            for v in out.values_mut() {
                if let Value::Value(inner) = v {
                    *v = (**inner).try_clone().expect("no fds");
                }
            }
            if let Some(psk) = keys.get(&text(&p, "public-key"))
                && !out.contains_key("preshared-key")
            {
                out.insert("preshared-key".into(), Value::from(psk.clone()));
                out.insert("preshared-key-flags".into(), Value::from(0u32));
            }
            out
        })
        .collect();
    wg.insert("peers".into(), OwnedValue::try_from(Value::from(peers)).expect("no fds"));
}

/// A `.conf` file's contents from a saved connection and its secrets.
pub fn from_settings(s: &Settings, secrets: &Settings) -> Conf {
    let (_, form) = crate::editor::read(s);
    let private_key = secrets
        .get("wireguard")
        .and_then(|w| w.get("private-key"))
        .and_then(|v| <&str>::try_from(&**v).ok())
        .unwrap_or_default()
        .to_owned();
    let psks = preshared_keys(secrets);
    let addresses =
        form.ip4.addresses.iter().chain(&form.ip6.addresses).map(|a| format!("{}/{}", a.address, a.prefix)).collect();
    Conf {
        private_key,
        addresses,
        dns: form.ip4.dns.iter().chain(&form.ip6.dns).cloned().collect(),
        search: form.ip4.search.split([',', ' ']).filter(|d| !d.is_empty()).map(str::to_owned).collect(),
        listen_port: form.wg.listen_port.parse().unwrap_or(0),
        mtu: form.mtu.parse().unwrap_or(0),
        fwmark: form.wg.fwmark.parse().unwrap_or(0),
        peers: form
            .wg
            .peers
            .iter()
            .map(|p| Peer {
                public_key: p.public_key.clone(),
                preshared_key: psks.get(&p.public_key).cloned().unwrap_or_default(),
                endpoint: p.endpoint.clone(),
                allowed_ips: p.allowed_ips.clone(),
                keepalive: p.keepalive.parse().unwrap_or(0),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Alice's keys from RFC 7748, section 6.1.
    const KEY: &str = "dwdtCnMYpX08FsFyUbJmRd9ML4frwJkqsXf7pR25LCo=";
    const PUB: &str = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo=";

    const FILE: &str = "\
# A home VPN
[Interface]
PrivateKey = dwdtCnMYpX08FsFyUbJmRd9ML4frwJkqsXf7pR25LCo=
Address = 10.8.0.2/24, fd00:8::2/64
DNS = 10.8.0.1, home.arpa
ListenPort = 51820
PostUp = iptables -A FORWARD

[Peer]
PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=
PresharedKey = /UwcSPg38hW/D9Y3tcS1FOV0K1wuURMbS0sesJEP5ak=
Endpoint = vpn.example.org:51820
AllowedIPs = 10.8.0.0/24, 192.168.50.0/24
PersistentKeepalive = 25
";

    #[test]
    fn public_keys_match_wg() {
        assert_eq!(public_key(KEY).as_deref(), Some(PUB));
        let new = generate();
        assert!(is_key(&new) && public_key(&new).is_some());
        assert_ne!(new, generate());
    }

    #[test]
    fn reads_and_writes_wg_quick_files() {
        let conf = parse(FILE).unwrap();
        assert_eq!(conf.addresses, ["10.8.0.2/24", "fd00:8::2/64"]);
        assert_eq!(
            (conf.dns.clone(), conf.search.clone()),
            (vec!["10.8.0.1".to_owned()], vec!["home.arpa".to_owned()])
        );
        assert_eq!(conf.listen_port, 51820);
        assert_eq!(conf.peers.len(), 1);
        let peer = &conf.peers[0];
        assert_eq!((peer.endpoint.as_str(), peer.keepalive), ("vpn.example.org:51820", 25));
        assert_eq!(peer.allowed_ips, ["10.8.0.0/24", "192.168.50.0/24"]);
        assert_eq!(parse(&export(&conf)).unwrap(), conf);
        assert!(parse("[Interface]\nAddress = 10.0.0.1/24\n").unwrap_err().contains("PrivateKey"));
        assert!(parse("[Interface]\nPrivateKey = nope\n").is_err());
    }

    #[test]
    fn settings_for_network_manager() {
        let conf = parse(FILE).unwrap();
        let s = settings(&conf, "wg-home", "u-1", Some("giosuel"));
        assert_eq!(<&str>::try_from(&*s["connection"]["type"]).unwrap(), "wireguard");
        assert_eq!(<&str>::try_from(&*s["connection"]["interface-name"]).unwrap(), "wg-home");
        assert_eq!(<&str>::try_from(&*s["ipv4"]["method"]).unwrap(), "manual");
        assert_eq!(<&str>::try_from(&*s["ipv6"]["method"]).unwrap(), "manual");
        assert_eq!(u32::try_from(&*s["wireguard"]["listen-port"]).unwrap(), 51820);
        let (kind, form) = crate::editor::read(&s);
        assert_eq!(kind, crate::editor::Kind::WireGuard);
        assert_eq!(form.ip4.addresses[0].address, "10.8.0.2");
        assert_eq!(form.wg.peers[0].allowed_ips, conf.peers[0].allowed_ips);
        assert_eq!(form.wg.interface, "wg-home");
        // Exported again, from the settings and the secrets NetworkManager would give back.
        let mut secrets: Settings = HashMap::new();
        put(&mut secrets, "wireguard", "private-key", Value::from(KEY));
        put(&mut secrets, "wireguard", "peers", peers_value(&conf.peers));
        let back = from_settings(&s, &secrets);
        assert_eq!(back.private_key, KEY);
        assert_eq!(back.peers, conf.peers);
        assert_eq!(back.addresses, conf.addresses);
    }

    #[test]
    fn edited_peers_keep_their_preshared_keys() {
        let conf = parse(FILE).unwrap();
        let mut secrets: Settings = HashMap::new();
        put(&mut secrets, "wireguard", "peers", peers_value(&conf.peers));
        // The editor writes peers without secrets.
        let bare: Vec<Peer> =
            conf.peers.iter().map(|p| Peer { preshared_key: String::new(), keepalive: 10, ..p.clone() }).collect();
        let mut s: Settings = HashMap::new();
        put(&mut s, "wireguard", "peers", peers_value(&bare));
        keep_preshared_keys(&mut s, &secrets);
        let mut secrets_after: Settings = HashMap::new();
        secrets_after.insert("wireguard".into(), s["wireguard"].clone());
        assert_eq!(preshared_keys(&secrets_after)[&conf.peers[0].public_key], conf.peers[0].preshared_key);
    }
}
