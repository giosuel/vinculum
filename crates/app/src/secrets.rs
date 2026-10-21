//! Answering NetworkManager's secret agent requests: from the keyring when the password is
//! there, otherwise by asking the user (through the password dialog on the UI thread).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, oneshot};
use vinculum_keyring::{Entry, Keyring};
use vinculum_nm::agent::secret;
use vinculum_nm::{AgentConnection, AgentEvent, KeyMgmt, Refusal, SecretRequest};

/// The Wi-Fi password setting, the only one the password dialog asks for.
pub const WIFI_SECURITY: &str = "802-11-wireless-security";

/// The keyring, opened the first time it's needed (and tried again while there's none: gnome-keyring
/// may start after Vinculum does).
#[derive(Clone, Default)]
pub struct Keyrings(Arc<Mutex<Option<Arc<Keyring>>>>);

impl Keyrings {
    pub async fn get(&self) -> Option<Arc<Keyring>> {
        let mut keyring = self.0.lock().await;
        if keyring.is_none() {
            *keyring = Keyring::session().await.map(Arc::new);
        }
        keyring.clone()
    }
}

/// A password the UI should ask for.
#[derive(Debug)]
pub struct Prompt {
    /// The connection's settings object, which `CancelGetSecrets` names.
    pub connection_path: String,
    pub title: String,
    pub subtitle: String,
    /// NetworkManager says the last password was wrong.
    pub retry: bool,
    pub key_mgmt: KeyMgmt,
    /// The password, or `None` when the user cancelled.
    pub reply: oneshot::Sender<Option<String>>,
}

/// What the agent needs from the UI thread.
pub trait Ui: Clone + Send + 'static {
    fn ask(&self, prompt: Prompt);
    /// NetworkManager doesn't need the password any more: close its dialog.
    fn cancel(&self, connection_path: String);
}

/// The secret key the dialog fills in for a Wi-Fi connection, by its key management.
fn wifi_secret(connection: &AgentConnection) -> Option<(KeyMgmt, &'static str)> {
    match connection.string(WIFI_SECURITY, "key-mgmt")? {
        "wpa-psk" => Some((KeyMgmt::WpaPsk, "psk")),
        "sae" => Some((KeyMgmt::Sae, "psk")),
        "none" => Some((KeyMgmt::Wep, "wep-key0")),
        _ => None,
    }
}

/// What the password dialog asks for, for a request: the secret's key, how it's checked (any
/// non-empty text, for sign-in passwords), and who it's for ("ab12cd@uzh.ch").
fn prompt_secret(request: &SecretRequest) -> Option<(KeyMgmt, &'static str, String)> {
    let c = &request.connection;
    match request.setting.as_str() {
        WIFI_SECURITY => wifi_secret(c).map(|(k, key)| (k, key, String::new())),
        "802-1x" => {
            let tls = c
                .settings
                .get("802-1x")
                .and_then(|x| x.get("eap"))
                .and_then(|v| <Vec<String>>::try_from(v.try_clone().ok()?).ok())
                .is_some_and(|eap| eap.iter().any(|e| e == "tls"));
            let key = if tls { "private-key-password" } else { "password" };
            Some((KeyMgmt::Sae, key, c.string("802-1x", "identity").unwrap_or_default().to_owned()))
        }
        // OpenVPN: the password, or the private key's password with certificates only.
        "vpn" => {
            let data = c
                .settings
                .get("vpn")
                .and_then(|v| v.get("data"))
                .and_then(|d| <HashMap<String, String>>::try_from(d.clone()).ok())?;
            let key =
                if data.get("connection-type").map(String::as_str) == Some("tls") { "cert-pass" } else { "password" };
            Some((KeyMgmt::Sae, key, data.get("username").cloned().unwrap_or_default()))
        }
        _ => None,
    }
}

pub async fn handle(event: AgentEvent, keyrings: &Keyrings, ui: &impl Ui) {
    match event {
        AgentEvent::GetSecrets(request) => get_secrets(request, keyrings, ui).await,
        AgentEvent::Cancel { connection_path, .. } => ui.cancel(connection_path),
        AgentEvent::Save { connection, secrets } => {
            if let Some(keyring) = keyrings.get().await {
                for (setting, key, value) in secrets {
                    let entry = Entry { uuid: &connection.uuid, setting: &setting, key: &key };
                    if let Err(error) = keyring.store(entry, &connection.id, &value).await {
                        eprintln!("vinculum: couldn't save a password for {}: {error}", connection.id);
                    }
                }
            }
        }
        AgentEvent::Delete { connection } => {
            if let Some(keyring) = keyrings.get().await
                && let Err(error) = keyring.delete(&connection.uuid).await
            {
                eprintln!("vinculum: couldn't delete the passwords of {}: {error}", connection.id);
            }
        }
    }
}

async fn get_secrets(request: SecretRequest, keyrings: &Keyrings, ui: &impl Ui) {
    let keyring = keyrings.get().await;
    // A rejected password isn't looked up again.
    if !request.request_new()
        && let Some(keyring) = &keyring
        && let Ok(found) = keyring.find(&request.connection.uuid, &request.setting).await
        && !found.is_empty()
    {
        request.reply(Ok(found));
        return;
    }
    let (Some((key_mgmt, key, user)), true) = (prompt_secret(&request), request.interactive()) else {
        request.reply(Err(Refusal::NoSecrets));
        return;
    };
    let name = String::from_utf8_lossy(&request.connection.ssid).into_owned();
    let name = if name.is_empty() { request.connection.id.clone() } else { name };
    let (reply, answer) = oneshot::channel();
    let sign_in = request.setting != WIFI_SECURITY;
    ui.ask(Prompt {
        connection_path: request.connection.path.clone(),
        title: if sign_in { format!("Sign in to {name}") } else { format!("Connect to {name}") },
        subtitle: match (request.request_new(), user.is_empty()) {
            (true, _) => "That password didn't work.".into(),
            (false, false) => format!("The password for {user}."),
            (false, true) => "The network needs its password.".into(),
        },
        retry: request.request_new(),
        key_mgmt,
        reply,
    });
    let Ok(Some(password)) = answer.await else {
        request.reply(Err(Refusal::UserCanceled));
        return;
    };
    // Agent-owned passwords are ours to keep; NetworkManager keeps the others itself.
    if request.connection.secret_flags(&request.setting, key) & secret::AGENT_OWNED != 0
        && let Some(keyring) = &keyring
    {
        let entry = Entry { uuid: &request.connection.uuid, setting: &request.setting, key };
        if let Err(error) = keyring.store(entry, &request.connection.id, &password).await {
            eprintln!("vinculum: couldn't save the password for {name}: {error}");
        }
    }
    request.reply(Ok(HashMap::from([(key.to_owned(), password)])));
}

/// The secret key a new connection's password goes in.
pub fn join_key(key_mgmt: KeyMgmt) -> &'static str {
    if key_mgmt == KeyMgmt::Wep { "wep-key0" } else { "psk" }
}

/// The user's login name, for a connection only they may use.
pub fn username() -> Option<String> {
    if let Some(user) = std::env::var("USER").ok().filter(|u| !u.is_empty()) {
        return Some(user);
    }
    // No $USER (started by a service manager, say): look the UID up.
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let uid = status.lines().find_map(|l| l.strip_prefix("Uid:"))?.split_whitespace().next()?.to_owned();
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.get(2) == Some(&uid.as_str())).then(|| fields[0].to_owned())
    })
}

/// A random (version 4) UUID for a new connection.
pub fn new_uuid() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut random) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut random, &mut bytes);
    }
    if bytes == [0; 16] {
        // No /dev/urandom: the clock is unique enough for a connection ID.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        bytes = (nanos ^ (u128::from(std::process::id()) << 64)).to_le_bytes();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_look_like_uuids() {
        let (a, b) = (new_uuid(), new_uuid());
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert!("89ab".contains(&a[19..20]));
    }

    fn request(settings: &[(&str, &str, zbus::zvariant::Value<'static>)], setting: &str) -> SecretRequest {
        let mut s: vinculum_nm::Settings = HashMap::new();
        for (group, key, value) in settings {
            s.entry((*group).into())
                .or_default()
                .insert((*key).into(), zbus::zvariant::OwnedValue::try_from(value.clone()).unwrap());
        }
        SecretRequest::for_tests(s, setting)
    }

    #[test]
    fn what_the_dialog_asks_for() {
        let eap = request(
            &[("802-1x", "eap", vec!["peap".to_owned()].into()), ("802-1x", "identity", "ab12cd@uzh.ch".into())],
            "802-1x",
        );
        assert_eq!(prompt_secret(&eap).map(|(_, k, u)| (k, u)), Some(("password", "ab12cd@uzh.ch".to_owned())));
        let tls = request(&[("802-1x", "eap", vec!["tls".to_owned()].into())], "802-1x");
        assert_eq!(prompt_secret(&tls).unwrap().1, "private-key-password");
        let data = HashMap::from([("connection-type", "password"), ("username", "ab12cd")]);
        let vpn = request(&[("vpn", "data", data.into())], "vpn");
        assert_eq!(prompt_secret(&vpn).map(|(_, k, u)| (k, u)), Some(("password", "ab12cd".to_owned())));
        let psk = request(&[(WIFI_SECURITY, "key-mgmt", "wpa-psk".into())], WIFI_SECURITY);
        assert_eq!(prompt_secret(&psk).unwrap().1, "psk");
    }

    #[test]
    fn there_is_a_user() {
        assert!(username().is_some_and(|u| !u.is_empty()));
    }
}
