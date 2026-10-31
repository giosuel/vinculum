//! Lituus's secret agent: NetworkManager asks it for passwords it doesn't keep itself (the
//! "for me only" ones, which live in the user's keyring) and for passwords it doesn't have yet
//! (a new network, or a wrong one). The agent only passes the requests on, as [`AgentEvent`]s;
//! the app decides whether to answer from the keyring or ask the user.
//!
//! Method calls are read straight off the connection rather than through zbus's object server,
//! so that a request waiting for the user doesn't hold up `CancelGetSecrets` and the others.

use std::collections::HashMap;

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use zbus::fdo::DBusProxy;
use zbus::message::{Header, Type as MessageType};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, Message, MessageStream};

use crate::props::bytes;
use crate::{BUS_NAME, Error};

/// Where NetworkManager looks for an agent on the agent's connection.
pub const AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const AGENT_IFACE: &str = "org.freedesktop.NetworkManager.SecretAgent";
const AGENT_MANAGER: &str = "/org/freedesktop/NetworkManager/AgentManager";

/// A connection's settings as NetworkManager sends them (`a{sa{sv}}`).
pub type Settings = HashMap<String, HashMap<String, OwnedValue>>;

/// NMSecretAgentGetSecretsFlags.
pub mod request {
    /// The agent may ask the user.
    pub const ALLOW_INTERACTION: u32 = 0x1;
    /// The secrets NetworkManager has are wrong (a rejected password): ask for new ones.
    pub const REQUEST_NEW: u32 = 0x2;
    /// The user started this connection themselves.
    pub const USER_REQUESTED: u32 = 0x4;
}

/// NMSettingSecretFlags, the `<key>-flags` of each secret.
pub mod secret {
    /// Kept by an agent (the keyring), not by NetworkManager.
    pub const AGENT_OWNED: u32 = 0x1;
    /// Not kept anywhere: asked for on every connection.
    pub const NOT_SAVED: u32 = 0x2;
}

/// The connection a request is about, from the settings NetworkManager sent along.
#[derive(Clone, Debug)]
pub struct AgentConnection {
    pub path: String,
    pub uuid: String,
    pub id: String,
    /// `connection.type`, such as `802-11-wireless`.
    pub kind: String,
    /// For Wi-Fi.
    pub ssid: Vec<u8>,
    pub settings: Settings,
}

impl AgentConnection {
    fn new(settings: Settings, path: String) -> Self {
        let string = |group: &str, key: &str| {
            settings.get(group).and_then(|g| g.get(key)).and_then(|v| <&str>::try_from(&**v).ok()).map(str::to_owned)
        };
        Self {
            uuid: string("connection", "uuid").unwrap_or_default(),
            id: string("connection", "id").unwrap_or_default(),
            kind: string("connection", "type").unwrap_or_default(),
            ssid: settings.get("802-11-wireless").and_then(|g| g.get("ssid")).map(|v| bytes(v)).unwrap_or_default(),
            path,
            settings,
        }
    }

    /// A string setting, such as (`802-11-wireless-security`, `key-mgmt`).
    pub fn string(&self, setting: &str, key: &str) -> Option<&str> {
        self.settings.get(setting)?.get(key).and_then(|v| <&str>::try_from(&**v).ok())
    }

    /// The flags of a secret (its `<key>-flags`); for VPN secrets they're in `vpn.data`.
    pub fn secret_flags(&self, setting: &str, key: &str) -> u32 {
        let flags = format!("{key}-flags");
        let group = self.settings.get(setting);
        if setting == "vpn" {
            let data =
                group.and_then(|g| g.get("data")).and_then(|v| <HashMap<String, String>>::try_from(v.clone()).ok());
            return data.and_then(|d| d.get(&flags).and_then(|f| f.parse().ok())).unwrap_or(0);
        }
        group.and_then(|g| g.get(&flags)).and_then(|v| u32::try_from(&**v).ok()).unwrap_or(0)
    }

    /// The secrets in these settings that an agent keeps, as (setting, key, value).
    fn agent_owned(&self) -> Vec<(String, String, String)> {
        let mut found = Vec::new();
        for (setting, values) in &self.settings {
            if setting == "vpn" {
                let secrets = values.get("secrets").and_then(|v| <HashMap<String, String>>::try_from(v.clone()).ok());
                for (key, value) in secrets.unwrap_or_default() {
                    if self.secret_flags(setting, &key) & secret::AGENT_OWNED != 0 {
                        found.push((setting.clone(), key, value));
                    }
                }
                continue;
            }
            for (key, value) in values {
                let Ok(value) = <&str>::try_from(&**value) else { continue };
                if values.contains_key(&format!("{key}-flags"))
                    && self.secret_flags(setting, key) & secret::AGENT_OWNED != 0
                {
                    found.push((setting.clone(), key.clone(), value.to_owned()));
                }
            }
        }
        found.sort();
        found
    }
}

/// Why the agent gave no secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It has none, and couldn't (or wasn't allowed to) ask.
    NoSecrets,
    /// The user cancelled the password dialog.
    UserCanceled,
}

/// NetworkManager wants secrets for one setting of a connection. Answer with [`Self::reply`];
/// dropping it refuses with [`Refusal::NoSecrets`].
#[derive(Debug)]
pub struct SecretRequest {
    pub connection: AgentConnection,
    /// The setting the secrets belong to, such as `802-11-wireless-security`.
    pub setting: String,
    /// Which secrets are missing, when NetworkManager knows.
    pub hints: Vec<String>,
    /// [`request`] flags.
    pub flags: u32,
    reply: oneshot::Sender<Result<HashMap<String, String>, Refusal>>,
}

impl SecretRequest {
    /// The secrets of [`Self::setting`], by key.
    pub fn reply(self, secrets: Result<HashMap<String, String>, Refusal>) {
        let _ = self.reply.send(secrets);
    }

    pub fn interactive(&self) -> bool {
        self.flags & request::ALLOW_INTERACTION != 0
    }

    pub fn request_new(&self) -> bool {
        self.flags & request::REQUEST_NEW != 0
    }

    /// A request for tests of what answers it, with nobody waiting for the answer.
    #[doc(hidden)]
    pub fn for_tests(settings: Settings, setting: &str) -> Self {
        let (reply, _) = oneshot::channel();
        Self {
            connection: AgentConnection::new(settings, "/test".into()),
            setting: setting.into(),
            hints: Vec::new(),
            flags: request::ALLOW_INTERACTION,
            reply,
        }
    }
}

#[derive(Debug)]
pub enum AgentEvent {
    GetSecrets(SecretRequest),
    /// NetworkManager doesn't need the secrets any more (the connection attempt was stopped):
    /// close the dialog for it.
    Cancel {
        connection_path: String,
        setting: String,
    },
    /// Keep these agent-owned secrets, as (setting, key, value).
    Save {
        connection: AgentConnection,
        secrets: Vec<(String, String, String)>,
    },
    /// The connection was deleted: forget its secrets.
    Delete {
        connection: AgentConnection,
    },
}

impl crate::Client {
    /// Registers the agent under `identifier` (the app ID) and hands every request to the
    /// returned receiver. It registers again whenever NetworkManager restarts, and stops when the
    /// receiver is dropped.
    pub async fn register_agent(&self, identifier: &str) -> Result<mpsc::UnboundedReceiver<AgentEvent>, Error> {
        let conn = self.connection().clone();
        let (events, receiver) = mpsc::unbounded_channel();
        let calls = MessageStream::from(&conn);
        let owner_rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, BUS_NAME)?
            .build();
        let mut owners = MessageStream::for_match_rule(owner_rule, &conn, Some(16)).await?;
        register(&conn, identifier).await?;

        let identifier = identifier.to_owned();
        let (c, e) = (conn.clone(), events.clone());
        tokio::spawn(async move {
            // NetworkManager forgets its agents when it stops.
            while let Some(Ok(message)) = owners.next().await {
                if e.is_closed() {
                    break;
                }
                let new_owner = message.body().deserialize::<(String, String, String)>().map(|(_, _, n)| n);
                if new_owner.is_ok_and(|n| !n.is_empty()) {
                    let _ = register(&c, &identifier).await;
                }
            }
        });
        tokio::spawn(serve(conn, calls, events));
        Ok(receiver)
    }
}

async fn register(conn: &Connection, identifier: &str) -> Result<(), Error> {
    conn.call_method(
        Some(BUS_NAME),
        AGENT_MANAGER,
        Some("org.freedesktop.NetworkManager.AgentManager"),
        "RegisterWithCapabilities",
        &(identifier, 0u32),
    )
    .await
    .map_err(Error::from_dbus)?;
    Ok(())
}

/// For tools that introspect before calling (NetworkManager doesn't).
const INTROSPECTION: &str = r#"<node>
  <interface name="org.freedesktop.NetworkManager.SecretAgent">
    <method name="GetSecrets">
      <arg name="connection" type="a{sa{sv}}" direction="in"/>
      <arg name="connection_path" type="o" direction="in"/>
      <arg name="setting_name" type="s" direction="in"/>
      <arg name="hints" type="as" direction="in"/>
      <arg name="flags" type="u" direction="in"/>
      <arg name="secrets" type="a{sa{sv}}" direction="out"/>
    </method>
    <method name="CancelGetSecrets">
      <arg name="connection_path" type="o" direction="in"/>
      <arg name="setting_name" type="s" direction="in"/>
    </method>
    <method name="SaveSecrets">
      <arg name="connection" type="a{sa{sv}}" direction="in"/>
      <arg name="connection_path" type="o" direction="in"/>
    </method>
    <method name="DeleteSecrets">
      <arg name="connection" type="a{sa{sv}}" direction="in"/>
      <arg name="connection_path" type="o" direction="in"/>
    </method>
  </interface>
</node>"#;

async fn serve(conn: Connection, mut calls: MessageStream, events: mpsc::UnboundedSender<AgentEvent>) {
    let dbus = DBusProxy::new(&conn).await.ok();
    while let Some(Ok(message)) = calls.next().await {
        if events.is_closed() {
            break;
        }
        let header = message.header();
        if header.message_type() != MessageType::MethodCall || header.path().map(|p| p.as_str()) != Some(AGENT_PATH) {
            continue;
        }
        if header.interface().map(|i| i.as_str()) == Some("org.freedesktop.DBus.Introspectable") {
            let _ = conn.reply(&header, &(INTROSPECTION,)).await;
            continue;
        }
        if header.interface().map(|i| i.as_str()) != Some(AGENT_IFACE) {
            let _ = conn.reply_error(&header, "org.freedesktop.DBus.Error.UnknownMethod", &("Unknown method",)).await;
            continue;
        }
        let (conn, events) = (conn.clone(), events.clone());
        // Only NetworkManager may ask: anyone else could show a password dialog of their own
        // making, or read the keyring through the agent.
        let from_nm = match &dbus {
            Some(dbus) => match dbus.get_name_owner(BUS_NAME.try_into().expect("a valid name")).await {
                Ok(owner) => header.sender().is_some_and(|s| s.as_str() == owner.as_str()),
                Err(_) => false,
            },
            None => false,
        };
        tokio::spawn(async move {
            let header = message.header();
            if !from_nm {
                let _ = conn
                    .reply_error(&header, format!("{AGENT_IFACE}.NotAuthorized"), &("Only NetworkManager may ask",))
                    .await;
                return;
            }
            let result = handle(&message, &header, &events).await;
            let _ = match result {
                Ok(Some(secrets)) => conn.reply(&header, &(secrets,)).await,
                Ok(None) => conn.reply(&header, &()).await,
                Err((name, text)) => conn.reply_error(&header, format!("{AGENT_IFACE}.{name}"), &(text,)).await,
            };
        });
    }
}

/// Secrets to send back (for `GetSecrets`), nothing (for the others), or an error's name and text.
type Handled = Result<Option<HashMap<String, HashMap<String, Value<'static>>>>, (&'static str, &'static str)>;

async fn handle(message: &Message, header: &Header<'_>, events: &mpsc::UnboundedSender<AgentEvent>) -> Handled {
    const INVALID: (&str, &str) = ("InvalidConnection", "Couldn't read the request");
    let body = message.body();
    match header.member().map(|m| m.as_str()) {
        Some("GetSecrets") => {
            let (settings, path, setting, hints, flags): (Settings, OwnedObjectPath, String, Vec<String>, u32) =
                body.deserialize().map_err(|_| INVALID)?;
            let (reply, answer) = oneshot::channel();
            let connection = AgentConnection::new(settings, path.to_string());
            let request = SecretRequest { connection, setting: setting.clone(), hints, flags, reply };
            events.send(AgentEvent::GetSecrets(request)).map_err(|_| ("NoSecrets", "The agent is stopping"))?;
            match answer.await {
                Ok(Ok(secrets)) => Ok(Some(secrets_reply(&setting, secrets))),
                Ok(Err(Refusal::UserCanceled)) => Err(("UserCanceled", "The user cancelled")),
                Ok(Err(Refusal::NoSecrets)) | Err(_) => Err(("NoSecrets", "No secrets")),
            }
        }
        Some("CancelGetSecrets") => {
            let (path, setting): (OwnedObjectPath, String) = body.deserialize().map_err(|_| INVALID)?;
            let _ = events.send(AgentEvent::Cancel { connection_path: path.to_string(), setting });
            Ok(None)
        }
        Some("SaveSecrets") => {
            let (settings, path): (Settings, OwnedObjectPath) = body.deserialize().map_err(|_| INVALID)?;
            let connection = AgentConnection::new(settings, path.to_string());
            let secrets = connection.agent_owned();
            if !secrets.is_empty() {
                let _ = events.send(AgentEvent::Save { connection, secrets });
            }
            Ok(None)
        }
        Some("DeleteSecrets") => {
            let (settings, path): (Settings, OwnedObjectPath) = body.deserialize().map_err(|_| INVALID)?;
            let _ = events.send(AgentEvent::Delete { connection: AgentConnection::new(settings, path.to_string()) });
            Ok(None)
        }
        _ => Err(("InvalidConnection", "Unknown method")),
    }
}

/// `{setting: {key: value}}`; VPN secrets go in the VPN setting's `secrets` map instead.
fn secrets_reply(setting: &str, secrets: HashMap<String, String>) -> HashMap<String, HashMap<String, Value<'static>>> {
    let values = if setting == "vpn" {
        HashMap::from([("secrets".to_owned(), Value::from(secrets))])
    } else {
        secrets.into_iter().map(|(k, v)| (k, Value::from(v))).collect()
    };
    HashMap::from([(setting.to_owned(), values)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(groups: &[(&str, &[(&str, Value<'static>)])]) -> Settings {
        groups
            .iter()
            .map(|(g, kv)| {
                let values =
                    kv.iter().map(|(k, v)| ((*k).to_owned(), OwnedValue::try_from(v.clone()).unwrap())).collect();
                ((*g).to_owned(), values)
            })
            .collect()
    }

    #[test]
    fn agent_owned_secrets_are_picked_out() {
        let s = settings(&[
            ("connection", &[("id", "Aster Café".into()), ("uuid", "u1".into()), ("type", "802-11-wireless".into())]),
            ("802-11-wireless", &[("ssid", Value::from(b"Aster Caf\xc3\xa9".to_vec()))]),
            (
                "802-11-wireless-security",
                &[("key-mgmt", "wpa-psk".into()), ("psk", "secret!".into()), ("psk-flags", 1u32.into())],
            ),
            ("802-1x", &[("password", "system".into()), ("password-flags", 0u32.into())]),
        ]);
        let c = AgentConnection::new(s, "/c/1".into());
        assert_eq!((c.uuid.as_str(), c.id.as_str(), c.kind.as_str()), ("u1", "Aster Café", "802-11-wireless"));
        assert_eq!(c.ssid, "Aster Café".as_bytes());
        assert_eq!(c.string("802-11-wireless-security", "key-mgmt"), Some("wpa-psk"));
        assert_eq!(c.agent_owned(), vec![("802-11-wireless-security".into(), "psk".into(), "secret!".into())]);
    }

    #[test]
    fn vpn_secrets_use_the_data_flags() {
        let data = HashMap::from([("password-flags", "1"), ("cert-pass-flags", "0")]);
        let secrets = HashMap::from([("password", "vpn-pass"), ("cert-pass", "x")]);
        let s = settings(&[("vpn", &[("data", Value::from(data)), ("secrets", Value::from(secrets))])]);
        let c = AgentConnection::new(s, "/c/2".into());
        assert_eq!(c.secret_flags("vpn", "password"), 1);
        assert_eq!(c.agent_owned(), vec![("vpn".into(), "password".into(), "vpn-pass".into())]);
        let reply = secrets_reply("vpn", HashMap::from([("password".into(), "vpn-pass".into())]));
        assert!(matches!(reply["vpn"]["secrets"], Value::Dict(_)));
    }
}
