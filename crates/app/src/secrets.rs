//! Answering NetworkManager's secret agent requests: from the keyring when the password is
//! there, otherwise by asking the user (through the password dialog on the UI thread).

use std::collections::HashMap;
use std::sync::Arc;

use lituus_keyring::{Entry, Keyring};
use lituus_nm::agent::secret;
use lituus_nm::{AgentConnection, AgentEvent, KeyMgmt, Refusal, SecretRequest};
use tokio::sync::{Mutex, oneshot};

use crate::openconnect::{self, SignIn, SignInError, SignIns};

/// The Wi-Fi password setting, the only one the password dialog asks for.
pub const WIFI_SECURITY: &str = "802-11-wireless-security";

/// The keyring, opened the first time it's needed (and tried again while there's none: gnome-keyring
/// may start after Lituus does).
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
    /// A connection's sign-in window (OpenConnect's) opened or closed.
    fn signing_in(&self, uuid: String, on: bool);
    /// A note above the list.
    fn notice(&self, text: String);
    /// NetworkManager doesn't need the password any more: close its dialog.
    fn cancel(&self, connection_path: String);
    /// Connect again: NetworkManager stopped waiting for a sign-in that then finished, and its
    /// session waits in [`SignIns`].
    fn reconnect(&self, connection_path: String);
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

pub async fn handle(event: AgentEvent, keyrings: &Keyrings, sign_ins: &SignIns, ui: &impl Ui) {
    match event {
        AgentEvent::GetSecrets(request) => {
            if request.setting == "vpn" && openconnect::is_openconnect(&request.connection.settings) {
                openconnect_secrets(request, sign_ins, ui, openconnect::auth_dialog(), NM_WAITS).await;
            } else {
                get_secrets(request, keyrings, ui).await;
            }
        }
        AgentEvent::Cancel { connection_path, .. } => {
            sign_ins.cancel(&connection_path);
            ui.cancel(connection_path);
        }
        AgentEvent::Save { connection, secrets } => {
            if let Some(keyring) = keyrings.get().await {
                for (setting, key, value) in secrets {
                    let entry = Entry { uuid: &connection.uuid, setting: &setting, key: &key };
                    if let Err(error) = keyring.store(entry, &connection.id, &value).await {
                        eprintln!("lituus: couldn't save a password for {}: {error}", connection.id);
                    }
                }
            }
        }
        AgentEvent::Delete { connection } => {
            if let Some(keyring) = keyrings.get().await
                && let Err(error) = keyring.delete(&connection.uuid).await
            {
                eprintln!("lituus: couldn't delete the passwords of {}: {error}", connection.id);
            }
        }
    }
}

/// How long NetworkManager waits for an agent's answer (two minutes), less a margin: an answer
/// later than this goes nowhere.
const NM_WAITS: std::time::Duration = std::time::Duration::from_secs(110);

/// An OpenConnect VPN: the session Lituus signed in for before connecting, or (for a connection
/// started elsewhere, or one whose session was refused) the plugin's sign-in window now.
async fn openconnect_secrets(
    request: SecretRequest,
    sign_ins: &SignIns,
    ui: &impl Ui,
    dialog: Option<std::path::PathBuf>,
    nm_waits: std::time::Duration,
) {
    let uuid = request.connection.uuid.clone();
    let ready = sign_ins.take_ready(&uuid);
    if let (Some(session), false) = (ready, request.request_new()) {
        request.reply(Ok(session));
        return;
    }
    if !request.interactive() {
        request.reply(Err(Refusal::NoSecrets));
        return;
    }
    let Some(dialog) = dialog else {
        // In a Flatpak, NetworkManager asks the next agent (GNOME Shell's, nm-applet), which can
        // open the window.
        if !openconnect::in_flatpak() {
            ui.notice(openconnect_missing().into());
        }
        request.reply(Err(Refusal::NoSecrets));
        return;
    };
    // Its window is open already, for Lituus's own sign-in before connecting.
    let Some(started) = sign_ins.start(&uuid, &request.connection.path) else {
        request.reply(Err(Refusal::NoSecrets));
        return;
    };
    let settings = &request.connection.settings;
    let (data, kept) = (openconnect::data(settings), openconnect::secrets(settings));
    let begun = std::time::Instant::now();
    ui.signing_in(uuid.clone(), true);
    let result = openconnect::sign_in(
        &dialog,
        SignIn { uuid: &uuid, name: &request.connection.id, data: &data, secrets: &kept, again: request.request_new() },
        started.cancel,
    )
    .await;
    sign_ins.finish(started.id);
    ui.signing_in(uuid.clone(), false);
    match result {
        // NetworkManager gave up on this request: connect again, so the sign-in (and the MFA
        // approval in it) isn't wasted.
        Ok(session) if begun.elapsed() >= nm_waits => {
            sign_ins.set_ready(&uuid, session);
            ui.reconnect(request.connection.path.clone());
        }
        Ok(session) => request.reply(Ok(session)),
        Err(SignInError::Closed | SignInError::Cancelled) => request.reply(Err(Refusal::UserCanceled)),
        Err(SignInError::Failed(reason)) => {
            ui.notice(reason);
            request.reply(Err(Refusal::NoSecrets));
        }
    }
}

/// Why there's no sign-in window: the OpenConnect plugin (or its window) isn't installed, or
/// Lituus runs in a Flatpak, which can't start it.
pub fn openconnect_missing() -> &'static str {
    if openconnect::in_flatpak() {
        "No sign-in window came up. The Flatpak can't open NetworkManager-openconnect's window itself, so it leaves signing in to GNOME Shell or nm-applet, and neither answered. Start nm-applet, or use one of Lituus's other packages (AUR, .deb, .rpm, Nix)."
    } else {
        "Cisco AnyConnect needs NetworkManager's OpenConnect plugin, with its sign-in window: networkmanager-openconnect on Arch, network-manager-openconnect-gnome on Debian and Ubuntu, NetworkManager-openconnect-gnome on Fedora. Install it, then try again."
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
            eprintln!("lituus: couldn't save the password for {name}: {error}");
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
        let mut s: lituus_nm::Settings = HashMap::new();
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

    /// What the agent told the UI.
    #[derive(Clone, Default)]
    struct Seen(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl Seen {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    impl Ui for Seen {
        fn ask(&self, prompt: Prompt) {
            self.0.lock().unwrap().push(format!("ask {}", prompt.title));
        }
        fn signing_in(&self, uuid: String, on: bool) {
            self.0.lock().unwrap().push(format!("signing in {uuid} {on}"));
        }
        fn notice(&self, text: String) {
            self.0.lock().unwrap().push(format!("notice {text}"));
        }
        fn cancel(&self, connection_path: String) {
            self.0.lock().unwrap().push(format!("cancel {connection_path}"));
        }
        fn reconnect(&self, connection_path: String) {
            self.0.lock().unwrap().push(format!("reconnect {connection_path}"));
        }
    }

    /// NetworkManager's request for an AnyConnect VPN's secrets, with these flags.
    fn anyconnect(flags: u32) -> (SecretRequest, oneshot::Receiver<lituus_nm::agent::Answer>) {
        let mut s: lituus_nm::Settings = HashMap::new();
        let mut put = |group: &str, key: &str, value: zbus::zvariant::Value<'static>| {
            s.entry(group.into()).or_default().insert(key.into(), zbus::zvariant::OwnedValue::try_from(value).unwrap());
        };
        put("connection", "uuid", "u-uzh".into());
        put("connection", "id", "UZH VPN".into());
        put("connection", "type", "vpn".into());
        put("vpn", "service-type", openconnect::SERVICE.into());
        put("vpn", "data", HashMap::from([("gateway", "vpn.uzh.ch")]).into());
        SecretRequest::for_tests_answered(s, "vpn", flags)
    }

    const SESSION: &str = "cookie\\nwebvpn=abc\\ngateway\\nvpn.uzh.ch\\n\\n\\n";

    #[test]
    fn anyconnect_hands_over_the_session_it_signed_in_for() {
        use lituus_nm::agent::request::{ALLOW_INTERACTION, REQUEST_NEW};
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (sign_ins, ui) = (SignIns::default(), Seen::default());
        let dir = openconnect::tests::scratch("agent");
        let ok = openconnect::tests::fake_dialog(&dir, "ok", &openconnect::tests::answers(SESSION));
        let wait = std::time::Duration::from_secs(60);
        let answer = |flags: u32, dialog: Option<&std::path::Path>, wait: std::time::Duration| {
            let (request, answer) = anyconnect(flags);
            runtime.block_on(openconnect_secrets(request, &sign_ins, &ui, dialog.map(ToOwned::to_owned), wait));
            answer.blocking_recv()
        };
        let session = HashMap::from([("cookie".to_owned(), "from before".to_owned())]);

        // The session from signing in before connecting, once, even without interaction.
        sign_ins.set_ready("u-uzh", session.clone());
        assert_eq!(answer(0, None, wait), Ok(Ok(session.clone())));
        assert_eq!(answer(0, None, wait), Ok(Err(Refusal::NoSecrets)), "no window without interaction");
        assert!(ui.take().is_empty());

        // Not when NetworkManager says it didn't work: the window opens again.
        sign_ins.set_ready("u-uzh", session.clone());
        let new = answer(ALLOW_INTERACTION | REQUEST_NEW, Some(&ok), wait).unwrap().unwrap();
        assert_eq!(new["cookie"], "webvpn=abc");
        assert_eq!(ui.take(), ["signing in u-uzh true", "signing in u-uzh false"]);

        // A closed window is the user's cancel; one that fails says why.
        let closed = openconnect::tests::fake_dialog(&dir, "closed", &openconnect::tests::answers("\\n\\n"));
        assert_eq!(answer(ALLOW_INTERACTION, Some(&closed), wait), Ok(Err(Refusal::UserCanceled)));
        ui.take();
        let failed = openconnect::tests::fake_dialog(&dir, "failed", "echo 'No gateway configured' >&2\nexit 1");
        assert_eq!(answer(ALLOW_INTERACTION, Some(&failed), wait), Ok(Err(Refusal::NoSecrets)));
        assert_eq!(ui.take()[2], "notice The sign-in window stopped: No gateway configured.");

        // NetworkManager stopped waiting before the sign-in finished: no answer, but a new attempt
        // that the session waits for.
        assert!(answer(ALLOW_INTERACTION, Some(&ok), std::time::Duration::ZERO).is_err(), "nobody to answer");
        assert_eq!(ui.take()[2], "reconnect /test");
        assert_eq!(sign_ins.take_ready("u-uzh").unwrap()["cookie"], "webvpn=abc");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn networkmanager_cancelling_closes_the_sign_in_window() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (sign_ins, ui, keyrings) = (SignIns::default(), Seen::default(), Keyrings::default());
        let dir = openconnect::tests::scratch("agent-cancel");
        let open = openconnect::tests::fake_dialog(&dir, "open", openconnect::tests::STAYS_OPEN);
        let (request, answer) = anyconnect(lituus_nm::agent::request::ALLOW_INTERACTION);
        let begun = std::time::Instant::now();
        runtime.block_on(async {
            let cancel = async {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let event = AgentEvent::Cancel { connection_path: "/test".into(), setting: "vpn".into() };
                handle(event, &keyrings, &sign_ins, &ui).await;
            };
            tokio::join!(openconnect_secrets(request, &sign_ins, &ui, Some(open), NM_WAITS), cancel);
        });
        assert_eq!(answer.blocking_recv(), Ok(Err(Refusal::UserCanceled)));
        assert!(begun.elapsed().as_secs() < 2, "{:?}", begun.elapsed());
        assert_eq!(ui.take(), ["signing in u-uzh true", "cancel /test", "signing in u-uzh false"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn there_is_a_user() {
        assert!(username().is_some_and(|u| !u.is_empty()));
    }
}
