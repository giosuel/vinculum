//! Joining a Wi-Fi network that has no saved connection yet, and following a connection attempt
//! until it works or fails.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::{MatchRule, MessageStream};

use crate::props::Props;
use crate::{BUS_NAME, Client, Error};

/// How a password-protected network checks the password.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMgmt {
    /// WPA/WPA2 Personal, and WPA2/WPA3 transition networks.
    WpaPsk,
    /// WPA3 Personal only.
    Sae,
    Wep,
}

impl KeyMgmt {
    fn as_str(self) -> &'static str {
        match self {
            Self::WpaPsk => "wpa-psk",
            Self::Sae => "sae",
            Self::Wep => "none",
        }
    }

    /// Whether `password` can work: 8 to 63 characters or 64 hex digits for WPA, and a 5 or 13
    /// character key, a 10 or 26 digit hex key, or a passphrase for WEP.
    pub fn accepts(self, password: &str) -> bool {
        let chars = password.chars().count();
        match self {
            Self::WpaPsk => (8..=63).contains(&chars) || (chars == 64 && is_hex(password)),
            Self::Sae => chars >= 1,
            Self::Wep => chars >= 1,
        }
    }
}

fn is_hex(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Where the password is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    /// In the user's keyring, through the secret agent; the connection is the user's own.
    Agent,
    /// In NetworkManager's files, for every user.
    System,
    /// Nowhere: asked for on every connection.
    Ask,
}

impl Storage {
    /// The secret's `<key>-flags`.
    fn flags(self) -> u32 {
        match self {
            Self::Agent => crate::agent::secret::AGENT_OWNED,
            Self::System => 0,
            Self::Ask => crate::agent::secret::NOT_SAVED,
        }
    }
}

/// A new Wi-Fi connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiJoin {
    pub ssid: Vec<u8>,
    /// Made by the caller, so the password can go in the keyring before NetworkManager asks.
    pub uuid: String,
    /// `None` for an open network.
    pub security: Option<(KeyMgmt, String)>,
    pub storage: Storage,
    /// The user the connection belongs to; `None` makes it everyone's.
    pub owner: Option<String>,
    pub autoconnect: bool,
    /// The network doesn't broadcast its name.
    pub hidden: bool,
}

impl WifiJoin {
    fn settings(&self) -> HashMap<&'static str, HashMap<&'static str, Value<'_>>> {
        let name = String::from_utf8_lossy(&self.ssid).into_owned();
        let mut connection = HashMap::from([
            ("type", Value::from("802-11-wireless")),
            ("id", Value::from(name)),
            ("uuid", Value::from(self.uuid.as_str())),
            ("autoconnect", Value::from(self.autoconnect)),
        ]);
        if let Some(owner) = &self.owner {
            connection.insert("permissions", Value::from(vec![format!("user:{owner}:")]));
        }
        let mut wireless =
            HashMap::from([("ssid", Value::from(self.ssid.clone())), ("mode", Value::from("infrastructure"))]);
        if self.hidden {
            wireless.insert("hidden", Value::from(true));
        }
        let mut settings = HashMap::from([("connection", connection), ("802-11-wireless", wireless)]);
        if let Some((key_mgmt, password)) = &self.security {
            let flags = self.storage.flags();
            let mut security = HashMap::from([("key-mgmt", Value::from(key_mgmt.as_str()))]);
            if *key_mgmt == KeyMgmt::Wep {
                // 1: a hex or ASCII key, 2: a passphrase that's hashed into one.
                let chars = password.chars().count();
                let key = matches!(chars, 5 | 13) || (matches!(chars, 10 | 26) && is_hex(password));
                security.insert("wep-key-type", Value::from(if key { 1u32 } else { 2u32 }));
                security.insert("wep-key0", Value::from(password.as_str()));
                security.insert("wep-key-flags", Value::from(flags));
            } else {
                security.insert("psk", Value::from(password.as_str()));
                security.insert("psk-flags", Value::from(flags));
            }
            settings.insert("802-11-wireless-security", security);
        }
        settings
    }
}

/// How a connection attempt ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Connected,
    /// The user (or something else) stopped it, or no password was given: nothing to report.
    Stopped,
    /// It failed, with the reason in words.
    Failed(String),
}

/// NetworkManager's longest wait is the password dialog, which it gives up on after two minutes.
const ATTEMPT: Duration = Duration::from_secs(150);

impl Client {
    /// Makes and activates a new Wi-Fi connection, through `access_point` (or by scanning for the
    /// name, for a hidden network). Returns the new connection's and the active connection's
    /// paths.
    pub async fn join_wifi(
        &self,
        device: &str,
        access_point: Option<&str>,
        join: &WifiJoin,
    ) -> Result<(String, String), Error> {
        let body = (
            join.settings(),
            crate::client::object_path(device)?,
            crate::client::object_path(access_point.unwrap_or("/"))?,
        );
        let (connection, active): (OwnedObjectPath, OwnedObjectPath) = self
            .call(
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
                "AddAndActivateConnection",
                &body,
            )
            .await
            .map_err(Error::from_dbus)?;
        Ok((connection.to_string(), active.to_string()))
    }

    /// Waits until an active connection is up or has failed, and says why it failed. The device's
    /// reason is the useful one ("the network rejected the password"); the active connection's
    /// is the fallback.
    pub async fn outcome(&self, active: &str, device: Option<&str>) -> Result<Outcome, Error> {
        let conn = self.connection();
        let rule = |path: &str, interface: &'static str| -> zbus::Result<MatchRule<'static>> {
            Ok(MatchRule::builder()
                .msg_type(MessageType::Signal)
                .sender(BUS_NAME)?
                .path(path.to_owned())?
                .interface(interface)?
                .member("StateChanged")?
                .build())
        };
        let active_rule = rule(active, "org.freedesktop.NetworkManager.Connection.Active")?;
        let mut device_signals = match device {
            Some(device) => Some(
                MessageStream::for_match_rule(rule(device, "org.freedesktop.NetworkManager.Device")?, conn, Some(64))
                    .await?,
            ),
            None => None,
        };
        let mut active_signals = MessageStream::for_match_rule(active_rule, conn, Some(64)).await?;
        // It may have finished before the signals were set up.
        match Props::get_all(conn, active, "org.freedesktop.NetworkManager.Connection.Active").await {
            Ok(p) if p.u32("State") == Some(2) => return Ok(Outcome::Connected),
            Ok(_) => {}
            Err(_) => return Ok(Outcome::Failed("The connection went away before it was up.".into())),
        }
        let mut device_reason = None;
        let wait = async {
            loop {
                let next = match device_signals.as_mut() {
                    Some(d) => {
                        stream::select(d.by_ref().map(|m| (true, m)), active_signals.by_ref().map(|m| (false, m)))
                            .next()
                            .await
                    }
                    None => active_signals.next().await.map(|m| (false, m)),
                };
                let Some((from_device, Ok(message))) = next else { return Ok(Outcome::Failed(String::new())) };
                if from_device {
                    // StateChanged(new, old, reason); FAILED is 120.
                    if let Ok((120, _, reason)) = message.body().deserialize::<(u32, u32, u32)>() {
                        device_reason = Some(reason);
                    }
                    continue;
                }
                // StateChanged(state, reason): 2 is activated, 4 deactivated.
                let Ok((state, reason)) = message.body().deserialize::<(u32, u32)>() else { continue };
                match state {
                    2 => return Ok(Outcome::Connected),
                    4 => return Ok(failure(device_reason, reason)),
                    _ => {}
                }
            }
        };
        tokio::time::timeout(ATTEMPT, wait).await.unwrap_or(Ok(Outcome::Failed("It took too long.".into())))
    }
}

/// The words for NMDeviceStateReason (when the device failed) or else NMActiveConnectionStateReason.
fn failure(device: Option<u32>, active: u32) -> Outcome {
    let text = match device {
        // No secrets: the user cancelled the password dialog.
        Some(7) => return Outcome::Stopped,
        Some(8) => "The network rejected the password.",
        Some(9) => "The network's security settings didn't work.",
        Some(10) => "The Wi-Fi service (wpa_supplicant) failed.",
        Some(11) => "The network didn't answer in time.",
        Some(5 | 15..=17) => "Couldn't get an IP address from the network.",
        Some(53) => "The network is out of range.",
        Some(_) | None => match active {
            // Unknown, none, disconnected by the user, device disconnected, connection removed.
            1..=4 | 12 => return Outcome::Stopped,
            10 => return Outcome::Stopped,
            6 => "The IP settings didn't work.",
            7 => "It took too long.",
            8 | 9 => "The VPN service didn't start.",
            11 => "The login was refused.",
            13 => "A connection it depends on failed.",
            _ => "NetworkManager gave no reason.",
        },
    };
    Outcome::Failed(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_checked_like_networkmanager_does() {
        assert!(!KeyMgmt::WpaPsk.accepts("short"));
        assert!(KeyMgmt::WpaPsk.accepts("eight ch"));
        assert!(KeyMgmt::WpaPsk.accepts(&"a".repeat(64)));
        assert!(!KeyMgmt::WpaPsk.accepts(&"z".repeat(64)));
        assert!(!KeyMgmt::WpaPsk.accepts(&"a".repeat(65)));
        assert!(KeyMgmt::Sae.accepts("x"));
        assert!(!KeyMgmt::Sae.accepts(""));
    }

    #[test]
    fn keyring_storage_makes_a_private_agent_owned_connection() {
        let join = WifiJoin {
            ssid: b"Aster Caf\xc3\xa9".to_vec(),
            uuid: "u-1".into(),
            security: Some((KeyMgmt::WpaPsk, "café-pass".into())),
            storage: Storage::Agent,
            owner: Some("giosuel".into()),
            autoconnect: false,
            hidden: true,
        };
        let s = join.settings();
        assert_eq!(s["connection"]["id"], Value::from("Aster Café"));
        assert_eq!(s["connection"]["permissions"], Value::from(vec!["user:giosuel:".to_owned()]));
        assert_eq!(s["connection"]["autoconnect"], Value::from(false));
        assert_eq!(s["802-11-wireless"]["hidden"], Value::from(true));
        assert_eq!(s["802-11-wireless-security"]["psk-flags"], Value::from(1u32));
        assert_eq!(s["802-11-wireless-security"]["key-mgmt"], Value::from("wpa-psk"));
    }

    #[test]
    fn wep_keys_and_passphrases() {
        let mut join = WifiJoin {
            ssid: b"old".to_vec(),
            uuid: "u".into(),
            security: Some((KeyMgmt::Wep, "abcde".into())),
            storage: Storage::System,
            owner: None,
            autoconnect: true,
            hidden: false,
        };
        assert_eq!(join.settings()["802-11-wireless-security"]["wep-key-type"], Value::from(1u32));
        join.security = Some((KeyMgmt::Wep, "a long passphrase".into()));
        let s = join.settings();
        assert_eq!(s["802-11-wireless-security"]["wep-key-type"], Value::from(2u32));
        assert_eq!(s["802-11-wireless-security"]["wep-key-flags"], Value::from(0u32));
        assert!(!s["connection"].contains_key("permissions"));
    }

    #[test]
    fn failures_in_words() {
        assert_eq!(failure(Some(8), 4), Outcome::Failed("The network rejected the password.".into()));
        assert_eq!(failure(Some(7), 4), Outcome::Stopped);
        assert_eq!(failure(None, 3), Outcome::Stopped);
        assert_eq!(failure(None, 7), Outcome::Failed("It took too long.".into()));
    }
}
