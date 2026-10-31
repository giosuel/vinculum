//! The hotspot: a Wi-Fi connection in access point mode that shares this computer's internet
//! connection (NetworkManager's "shared" IPv4 method runs dnsmasq for the devices that join).

use std::collections::HashMap;

use lituus_nm::Settings;
use zbus::zvariant::{OwnedValue, Value};

/// `802-11-wireless.band` by the Band choice: automatic, 2.4 GHz, 5 GHz.
pub const BANDS: [&str; 3] = ["", "bg", "a"];

fn put(s: &mut Settings, setting: &str, key: &str, value: Value<'_>) {
    s.entry(setting.to_owned()).or_default().insert(key.to_owned(), OwnedValue::try_from(value).expect("no fds"));
}

/// A new hotspot connection, like `nmcli device wifi hotspot` makes: WPA2 (CCMP only, so phones
/// that can't do WPA3 join too), the password kept by NetworkManager so it starts without a
/// keyring, and the connection the user's own.
pub fn settings(ssid: &str, password: &str, band: usize, uuid: &str, owner: Option<&str>) -> Settings {
    let mut s: Settings = HashMap::new();
    put(&mut s, "connection", "id", Value::from("Hotspot"));
    put(&mut s, "connection", "uuid", Value::from(uuid));
    put(&mut s, "connection", "type", Value::from("802-11-wireless"));
    put(&mut s, "connection", "autoconnect", Value::from(false));
    if let Some(owner) = owner {
        put(&mut s, "connection", "permissions", Value::from(vec![format!("user:{owner}:")]));
    }
    apply(&mut s, ssid, password, band);
    put(&mut s, "802-11-wireless", "mode", Value::from("ap"));
    put(&mut s, "802-11-wireless-security", "key-mgmt", Value::from("wpa-psk"));
    put(&mut s, "802-11-wireless-security", "proto", Value::from(vec!["rsn".to_owned()]));
    put(&mut s, "802-11-wireless-security", "pairwise", Value::from(vec!["ccmp".to_owned()]));
    put(&mut s, "802-11-wireless-security", "group", Value::from(vec!["ccmp".to_owned()]));
    put(&mut s, "802-11-wireless-security", "pmf", Value::from(1i32));
    put(&mut s, "802-11-wireless-security", "psk-flags", Value::from(0u32));
    put(&mut s, "ipv4", "method", Value::from("shared"));
    put(&mut s, "ipv6", "method", Value::from("ignore"));
    s
}

/// Writes the name, password and band into a hotspot's settings.
pub fn apply(s: &mut Settings, ssid: &str, password: &str, band: usize) {
    put(s, "802-11-wireless", "ssid", Value::from(ssid.as_bytes().to_vec()));
    match BANDS[band.min(2)] {
        "" => {
            if let Some(w) = s.get_mut("802-11-wireless") {
                w.remove("band");
                w.remove("channel");
            }
        }
        band => put(s, "802-11-wireless", "band", Value::from(band)),
    }
    put(s, "802-11-wireless-security", "psk", Value::from(password));
}

const WORDS: [&str; 48] = [
    "amber", "birch", "brook", "cedar", "cloud", "coral", "delta", "ember", "fern", "frost", "glade", "harbor",
    "hazel", "heron", "iris", "juniper", "kelp", "lark", "linden", "lotus", "maple", "meadow", "mist", "moss",
    "nectar", "oak", "orchid", "pebble", "pine", "plume", "quartz", "rain", "reed", "ridge", "river", "sage", "shore",
    "slate", "spruce", "stone", "tidal", "thistle", "tulip", "violet", "willow", "wren", "yarrow", "zephyr",
];

/// A password that's easy to read out: two words and a number, such as "tidal-maple-41".
pub fn new_password() -> String {
    let mut bytes = [0u8; 3];
    if let Ok(mut random) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut random, &mut bytes);
    }
    let word = |b: u8| WORDS[b as usize % WORDS.len()];
    format!("{}-{}-{}", word(bytes[0]), word(bytes[1]), 10 + bytes[2] % 90)
}

/// A device on the hotspot, from dnsmasq's lease file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    pub name: String,
    pub address: String,
    /// When the lease ends (seconds since 1970), for how long it's been there.
    pub expires: u64,
}

/// The devices on the hotspot of `interface`, if NetworkManager's lease file can be read.
pub fn clients(interface: &str) -> Vec<Client> {
    std::fs::read_to_string(format!("/var/lib/NetworkManager/dnsmasq-{interface}.leases"))
        .map(|text| parse_leases(&text))
        .unwrap_or_default()
}

/// dnsmasq leases: `expiry mac ip hostname client-id` per line, `*` for an unknown name.
pub fn parse_leases(text: &str) -> Vec<Client> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (expires, mac, ip, name) = (f.first()?, f.get(1)?, f.get(2)?, f.get(3)?);
            Some(Client {
                name: if *name == "*" { mac.to_uppercase() } else { (*name).to_owned() },
                address: (*ip).to_owned(),
                expires: expires.parse().unwrap_or(0),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotspot_settings() {
        let s = settings("thinkpad-x1", "tidal-maple-41", 2, "u-1", Some("giosuel"));
        assert_eq!(<&str>::try_from(&*s["802-11-wireless"]["mode"]).unwrap(), "ap");
        assert_eq!(<&str>::try_from(&*s["802-11-wireless"]["band"]).unwrap(), "a");
        assert_eq!(<&str>::try_from(&*s["ipv4"]["method"]).unwrap(), "shared");
        assert_eq!(<&str>::try_from(&*s["802-11-wireless-security"]["psk"]).unwrap(), "tidal-maple-41");
    }

    #[test]
    fn passwords_and_leases() {
        let p = new_password();
        assert!(p.len() >= 8 && p.matches('-').count() == 2, "{p}");
        let leases =
            "1792600000 aa:bb:cc:dd:ee:ff 10.42.0.12 Pixel-8 01:aa\n1792600100 11:22:33:44:55:66 10.42.0.37 * *\n";
        let c = parse_leases(leases);
        assert_eq!(c[0], Client { name: "Pixel-8".into(), address: "10.42.0.12".into(), expires: 1792600000 });
        assert_eq!(c[1].name, "11:22:33:44:55:66");
    }
}
