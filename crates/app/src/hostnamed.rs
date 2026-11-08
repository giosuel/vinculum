//! The computer's two names, through systemd-hostnamed: the name (the "pretty" hostname, any
//! text, for this page and Bluetooth) and the network name (the static hostname: a–z, 0–9 and
//! hyphens, what the router and other computers see). One admin password covers both: hostnamed's
//! set-static-hostname action also allows the pretty one.

use zbus::Connection;

const DESTINATION: &str = "org.freedesktop.hostname1";
const PATH: &str = "/org/freedesktop/hostname1";
const IFACE: &str = "org.freedesktop.hostname1";

/// The network name a name suggests: lower case, ä as ae, other letters without their accents,
/// anything else a hyphen, at most 63 characters. "Giosuel's desk" → "giosuels-desk".
pub fn network_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        let mapped: &str = match c.to_lowercase().next().unwrap_or(c) {
            'ä' | 'æ' => "ae",
            'ö' | 'ø' | 'œ' => "oe",
            'ü' => "ue",
            'ß' => "ss",
            'à' | 'á' | 'â' | 'ã' | 'å' => "a",
            'ç' => "c",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ñ' => "n",
            'ò' | 'ó' | 'ô' | 'õ' => "o",
            'ù' | 'ú' | 'û' => "u",
            'ý' | 'ÿ' => "y",
            // Apostrophes vanish ("Giosuel's" → "giosuels").
            '\'' | '’' => "",
            l if l.is_ascii_alphanumeric() => {
                out.push(l);
                continue;
            }
            _ => "-",
        };
        out.push_str(mapped);
    }
    // One hyphen at a time, none at the ends.
    let mut collapsed = String::new();
    for c in out.chars() {
        if c == '-' && (collapsed.is_empty() || collapsed.ends_with('-')) {
            continue;
        }
        collapsed.push(c);
    }
    let mut name: String = collapsed.trim_end_matches('-').chars().take(63).collect();
    while name.ends_with('-') {
        name.pop();
    }
    name
}

/// What's wrong with a network name, if anything: one label of a–z, 0–9 and hyphens, 1 to 63
/// long, no hyphen at either end. Capitals are allowed (they're lowered when saved).
pub fn network_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Type a network name.");
    }
    if name.len() > 63 {
        return Some("At most 63 characters.");
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Some("Only letters a–z, digits and hyphens, such as giosuels-desk.");
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Some("It can't start or end with a hyphen.");
    }
    None
}

/// Why a call failed, in words.
fn problem(e: zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, _, _)
            if name.as_str().ends_with("NotAuthorized") || name.as_str().contains("Interactive") =>
        {
            "The admin password wasn't given, so the names stay as they were.".into()
        }
        zbus::Error::MethodError(_, Some(text), _) => text,
        other => other.to_string(),
    }
}

/// Whether hostnamed answers on this bus.
pub async fn available(conn: &Connection) -> bool {
    conn.call_method(Some(DESTINATION), PATH, Some("org.freedesktop.DBus.Peer"), "Ping", &()).await.is_ok()
}

/// The two names hostnamed has now: (name, network name); none when it doesn't answer.
pub async fn names(conn: &Connection) -> Option<(String, String)> {
    let reply = conn
        .call_method(Some(DESTINATION), PATH, Some("org.freedesktop.DBus.Properties"), "GetAll", &(IFACE,))
        .await
        .ok()?;
    let props: std::collections::HashMap<String, zbus::zvariant::OwnedValue> = reply.body().deserialize().ok()?;
    let get = |k: &str| props.get(k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
    Some((get("PrettyHostname"), get("StaticHostname")))
}

/// Sets both names, the network name first (it's the one that needs the password).
pub async fn set_names(conn: &Connection, name: &str, network: &str) -> Result<(), String> {
    let network = network.to_lowercase();
    conn.call_method(Some(DESTINATION), PATH, Some(IFACE), "SetStaticHostname", &(network.as_str(), true))
        .await
        .map_err(problem)?;
    // The pretty name is only worth keeping when it says more than the network name.
    let pretty = if name.trim() == network { "" } else { name.trim() };
    conn.call_method(Some(DESTINATION), PATH, Some(IFACE), "SetPrettyHostname", &(pretty, true))
        .await
        .map_err(problem)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_names_from_names() {
        assert_eq!(network_name("Giosuel's desk"), "giosuels-desk");
        assert_eq!(network_name("Büro – Zürich 2"), "buero-zuerich-2");
        assert_eq!(network_name("  ThinkPad P1 (Gen 7)  "), "thinkpad-p1-gen-7");
        assert_eq!(network_name("ü"), "ue");
        assert_eq!(network_name("!!!"), "");
        assert_eq!(network_name(&"a".repeat(80)).len(), 63);
    }

    #[test]
    fn network_name_rules() {
        assert_eq!(network_name_problem("giosuels-desk"), None);
        assert_eq!(network_name_problem("Orbis"), None);
        assert!(network_name_problem("giosuel_desk!").is_some());
        assert!(network_name_problem("-orbis").is_some());
        assert!(network_name_problem("a.b").is_some());
        assert!(network_name_problem("").is_some());
    }
}
