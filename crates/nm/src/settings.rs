//! Reading and writing a connection's settings, for the editor. Writes send the whole map back
//! (NetworkManager replaces it), so the editor starts from what [`Client::settings`] read and
//! changes only the keys the user touched: settings it doesn't know, from plugins or newer
//! NetworkManager versions, survive. Secrets aren't in that map; NetworkManager keeps the ones it
//! has when an update leaves them out.

use std::collections::HashMap;

use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::agent::Settings;
use crate::client::object_path;
use crate::{Client, Error};

const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const SETTINGS: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
/// NMSettingsUpdate2Flags / NMSettingsAddConnection2Flags: keep it on disk.
const TO_DISK: u32 = 0x1;

fn unknown_method(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod")
}

impl Client {
    /// A saved connection's settings, without secrets.
    pub async fn settings(&self, connection: &str) -> Result<Settings, Error> {
        self.call(connection, CONNECTION, "GetSettings", &()).await.map_err(Error::from_dbus)
    }

    /// Replaces a saved connection's settings (and saves them to disk).
    pub async fn update(&self, connection: &str, settings: &Settings) -> Result<(), Error> {
        let args: HashMap<&str, Value> = HashMap::new();
        match self
            .call::<_, HashMap<String, OwnedValue>>(connection, CONNECTION, "Update2", &(settings, TO_DISK, args))
            .await
        {
            Ok(_) => Ok(()),
            // NetworkManager before 1.12.
            Err(error) if unknown_method(&error) => {
                self.call::<_, ()>(connection, CONNECTION, "Update", &(settings,)).await.map_err(Error::from_dbus)
            }
            Err(error) => Err(Error::from_dbus(error)),
        }
    }

    /// Saves a new connection without activating it. Returns its path.
    pub async fn add(&self, settings: &Settings) -> Result<String, Error> {
        let args: HashMap<&str, Value> = HashMap::new();
        let reply: zbus::Result<(OwnedObjectPath, HashMap<String, OwnedValue>)> =
            self.call(SETTINGS, SETTINGS_IFACE, "AddConnection2", &(settings, TO_DISK, args)).await;
        match reply {
            Ok((path, _)) => Ok(path.to_string()),
            Err(error) if unknown_method(&error) => {
                let path: OwnedObjectPath = self
                    .call(SETTINGS, SETTINGS_IFACE, "AddConnection", &(settings,))
                    .await
                    .map_err(Error::from_dbus)?;
                Ok(path.to_string())
            }
            Err(error) => Err(Error::from_dbus(error)),
        }
    }

    /// Applies a changed connection to the device it's active on without reconnecting, where
    /// NetworkManager can (IP settings, say). Fails for changes that need a reconnect.
    pub async fn reapply(&self, device: &str) -> Result<(), Error> {
        let empty: HashMap<&str, HashMap<&str, Value>> = HashMap::new();
        self.call::<_, ()>(device, "org.freedesktop.NetworkManager.Device", "Reapply", &(empty, 0u64, 0u32))
            .await
            .map_err(Error::from_dbus)
    }

    /// Sets the computer's hostname (NetworkManager passes it on to systemd-hostnamed).
    pub async fn save_hostname(&self, hostname: &str) -> Result<(), Error> {
        self.call::<_, ()>(SETTINGS, SETTINGS_IFACE, "SaveHostname", &(hostname,)).await.map_err(Error::from_dbus)
    }

    /// Changes one key of a saved connection, such as `connection.autoconnect`.
    pub async fn set_setting(&self, connection: &str, setting: &str, key: &str, value: Value<'_>) -> Result<(), Error> {
        let mut settings = self.settings(connection).await?;
        let value = OwnedValue::try_from(value).map_err(|e| Error::DBus(e.into()))?;
        settings.entry(setting.to_owned()).or_default().insert(key.to_owned(), value);
        self.update(connection, &settings).await
    }

    /// firewalld's zones, for the General tab; empty without firewalld.
    pub async fn firewall_zones(&self) -> Vec<String> {
        let reply = self
            .connection()
            .call_method(
                Some("org.fedoraproject.FirewallD1"),
                "/org/fedoraproject/FirewallD1",
                Some("org.fedoraproject.FirewallD1.zone"),
                "getZones",
                &(),
            )
            .await;
        reply.ok().and_then(|r| r.body().deserialize::<Vec<String>>().ok()).unwrap_or_default()
    }

    /// The secrets of one setting of a saved connection, by key (asking the agent for agent-owned
    /// ones). For showing a saved password.
    pub async fn secrets(&self, connection: &str, setting: &str) -> Result<HashMap<String, String>, Error> {
        let _ = object_path(connection)?;
        let secrets: Settings =
            self.call(connection, CONNECTION, "GetSecrets", &(setting,)).await.map_err(Error::from_dbus)?;
        Ok(secrets
            .get(setting)
            .map(|values| {
                values
                    .iter()
                    .filter_map(|(k, v)| <&str>::try_from(&**v).ok().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default())
    }
}
