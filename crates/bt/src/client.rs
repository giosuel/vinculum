//! The connection to BlueZ: one snapshot on request, or a new one after every change.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, DBusError, MatchRule, MessageStream};

use crate::BUS_NAME;
use crate::model::{Adapter, Device, PowerState, Snapshot};
use crate::props::Props;

const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";

/// How long the change signals have to go quiet before a new snapshot is taken. Discovery sends
/// a signal for every advertisement it hears.
const QUIET: Duration = Duration::from_millis(150);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Bluetooth isn't running")]
    NotRunning,
    /// polkit or the bus policy said no.
    #[error("not allowed: {0}")]
    Denied(String),
    /// BlueZ refused, with the last part of its error name (such as `AuthenticationFailed`) and
    /// its message.
    #[error("{message}")]
    Refused { kind: String, message: String },
    #[error(transparent)]
    DBus(#[from] zbus::Error),
}

impl Error {
    pub(crate) fn from_dbus(error: zbus::Error) -> Self {
        let name = match &error {
            zbus::Error::MethodError(name, _, _) => name.to_string(),
            zbus::Error::FDO(fdo) => fdo.name().to_string(),
            _ => String::new(),
        };
        let message = match &error {
            zbus::Error::MethodError(_, Some(message), _) => message.clone(),
            zbus::Error::FDO(fdo) => fdo.description().unwrap_or_default().to_owned(),
            _ => String::new(),
        };
        if name == "org.freedesktop.DBus.Error.ServiceUnknown" {
            Self::NotRunning
        } else if name == "org.freedesktop.DBus.Error.AccessDenied" || name.ends_with(".NotAuthorized") {
            Self::Denied(message)
        } else if let Some(kind) = name.strip_prefix("org.bluez.Error.") {
            Self::Refused { kind: kind.to_owned(), message }
        } else {
            Self::DBus(error)
        }
    }

    /// What went wrong, for people: BlueZ's own messages are terse ("Page Timeout").
    pub fn describe(&self) -> String {
        let Self::Refused { kind, message } = self else { return self.to_string() };
        match kind.as_str() {
            "AuthenticationFailed" | "AuthenticationRejected" => "The device didn't accept the pairing.".into(),
            "AuthenticationCanceled" => "Pairing was cancelled.".into(),
            "AuthenticationTimeout" => "The device didn't answer in time.".into(),
            "ConnectionAttemptFailed" | "Failed" if message.contains("Page Timeout") || message.contains("timeout") => {
                "The device didn't answer. Make sure it's on and nearby.".into()
            }
            "ConnectionAttemptFailed" => "Couldn't reach the device.".into(),
            "InProgress" => "Already busy with this device.".into(),
            "AlreadyExists" => "The device is already paired.".into(),
            "NotReady" => "Bluetooth is off.".into(),
            "Blocked" => "Bluetooth is blocked (airplane mode or a hardware switch).".into(),
            "NotAvailable" | "NotSupported" => "The device doesn't support this.".into(),
            _ if !message.is_empty() => message.clone(),
            _ => kind.clone(),
        }
    }
}

/// `GetManagedObjects`'s reply.
type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[derive(Clone, Debug)]
pub struct Client {
    conn: Connection,
}

impl Client {
    /// BlueZ on the system bus.
    pub async fn system() -> Result<Self, Error> {
        Ok(Self::new(Connection::system().await?))
    }

    /// BlueZ on a given bus (tests use a private one).
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Reads everything once.
    pub async fn snapshot(&self) -> Result<Snapshot, Error> {
        let reply = self
            .conn
            .call_method(Some(BUS_NAME), "/", Some("org.freedesktop.DBus.ObjectManager"), "GetManagedObjects", &())
            .await
            .map_err(Error::from_dbus)?;
        let objects: Objects = reply.body().deserialize().map_err(Error::DBus)?;
        Ok(parse(objects))
    }

    /// The current snapshot, then a new one after every change BlueZ reports. Bursts of changes
    /// give one snapshot. When BlueZ stops, the stream yields [`Error::NotRunning`], and continues
    /// once it's back.
    pub async fn watch(&self) -> Result<impl Stream<Item = Result<Snapshot, Error>> + Send + 'static, Error> {
        let from_bluez = MatchRule::builder().msg_type(MessageType::Signal).sender(BUS_NAME)?.build();
        let owner = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, BUS_NAME)?
            .build();
        let signals = stream::select(
            MessageStream::for_match_rule(from_bluez, &self.conn, Some(1024)).await?,
            MessageStream::for_match_rule(owner, &self.conn, Some(16)).await?,
        );
        let client = self.clone();
        Ok(stream::unfold((client, Box::pin(signals), true), |(client, mut signals, first)| async move {
            if !first {
                // A message or a stream error alike means "look again"; the end means the
                // connection is gone.
                let _ = signals.next().await?;
                while let Ok(Some(_)) = tokio::time::timeout(QUIET, signals.next()).await {}
            }
            let snapshot = client.snapshot().await;
            Some((snapshot, (client, signals, false)))
        }))
    }

    /// Switches the adapter's radio on or off.
    pub async fn set_powered(&self, adapter: &str, on: bool) -> Result<(), Error> {
        self.set(adapter, ADAPTER, "Powered", Value::from(on)).await
    }

    /// Lets other devices find this computer, for `timeout` seconds (0: until switched off).
    pub async fn set_discoverable(&self, adapter: &str, on: bool, timeout: u32) -> Result<(), Error> {
        if on {
            self.set(adapter, ADAPTER, "DiscoverableTimeout", Value::from(timeout)).await?;
        }
        self.set(adapter, ADAPTER, "Discoverable", Value::from(on)).await
    }

    /// The name other devices see this computer as.
    pub async fn set_adapter_alias(&self, adapter: &str, alias: &str) -> Result<(), Error> {
        self.set(adapter, ADAPTER, "Alias", Value::from(alias)).await
    }

    /// Looks for devices nearby; they arrive through [`Client::watch`] until
    /// [`Client::stop_discovery`]. BlueZ stops by itself when this connection closes.
    pub async fn start_discovery(&self, adapter: &str) -> Result<(), Error> {
        match self.call(adapter, ADAPTER, "StartDiscovery", &()).await {
            Err(Error::Refused { kind, .. }) if kind == "InProgress" => Ok(()),
            other => other,
        }
    }

    pub async fn stop_discovery(&self, adapter: &str) -> Result<(), Error> {
        match self.call(adapter, ADAPTER, "StopDiscovery", &()).await {
            // It wasn't discovering, or stopped already.
            Err(Error::Refused { kind, .. }) if kind == "NotReady" || kind == "Failed" => Ok(()),
            other => other,
        }
    }

    /// Pairs with a device. BlueZ asks the agent for a code or a confirmation on the way, so this
    /// waits for the user.
    pub async fn pair(&self, device: &str) -> Result<(), Error> {
        self.call(device, DEVICE, "Pair", &()).await
    }

    pub async fn cancel_pairing(&self, device: &str) -> Result<(), Error> {
        self.call(device, DEVICE, "CancelPairing", &()).await
    }

    /// Connects every profile the device and this computer both have.
    pub async fn connect(&self, device: &str) -> Result<(), Error> {
        match self.call(device, DEVICE, "Connect", &()).await {
            Err(Error::Refused { kind, .. }) if kind == "AlreadyConnected" => Ok(()),
            other => other,
        }
    }

    pub async fn disconnect(&self, device: &str) -> Result<(), Error> {
        match self.call(device, DEVICE, "Disconnect", &()).await {
            Err(Error::Refused { kind, .. }) if kind == "NotConnected" => Ok(()),
            other => other,
        }
    }

    /// Connects one profile, such as [`crate::uuid::A2DP_SINK`].
    pub async fn connect_profile(&self, device: &str, uuid: &str) -> Result<(), Error> {
        self.call(device, DEVICE, "ConnectProfile", &(uuid,)).await
    }

    pub async fn disconnect_profile(&self, device: &str, uuid: &str) -> Result<(), Error> {
        self.call(device, DEVICE, "DisconnectProfile", &(uuid,)).await
    }

    /// A trusted device may connect and use services without asking.
    pub async fn set_trusted(&self, device: &str, on: bool) -> Result<(), Error> {
        self.set(device, DEVICE, "Trusted", Value::from(on)).await
    }

    /// A blocked device can't connect at all.
    pub async fn set_blocked(&self, device: &str, on: bool) -> Result<(), Error> {
        self.set(device, DEVICE, "Blocked", Value::from(on)).await
    }

    /// The user's own name for a device; an empty one goes back to the device's.
    pub async fn set_alias(&self, device: &str, alias: &str) -> Result<(), Error> {
        self.set(device, DEVICE, "Alias", Value::from(alias)).await
    }

    /// Forgets a device: unpairs it and removes it from the adapter.
    pub async fn remove(&self, adapter: &str, device: &str) -> Result<(), Error> {
        let path = ObjectPath::try_from(device).map_err(|e| Error::DBus(e.into()))?;
        self.call(adapter, ADAPTER, "RemoveDevice", &(path,)).await
    }

    async fn set(&self, path: &str, interface: &str, property: &str, value: Value<'_>) -> Result<(), Error> {
        self.call(path, "org.freedesktop.DBus.Properties", "Set", &(interface, property, value)).await
    }

    async fn call<B>(&self, path: &str, interface: &str, method: &str, body: &B) -> Result<(), Error>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.conn.call_method(Some(BUS_NAME), path, Some(interface), method, body).await.map_err(Error::from_dbus)?;
        Ok(())
    }
}

fn parse(objects: Objects) -> Snapshot {
    let mut snapshot = Snapshot::default();
    for (path, mut interfaces) in objects {
        let path = path.to_string();
        if let Some(props) = interfaces.remove(ADAPTER) {
            snapshot.adapters.push(adapter(path.clone(), props.into()));
        }
        if let Some(props) = interfaces.remove(DEVICE) {
            let battery = interfaces.remove(BATTERY).map(Props::from);
            snapshot.devices.push(device(path, props.into(), battery));
        }
    }
    // GetManagedObjects' order is a hash map's.
    snapshot.adapters.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.devices.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot
}

fn adapter(path: String, p: Props) -> Adapter {
    let powered = p.bool("Powered").unwrap_or(false);
    Adapter {
        name: path.rsplit('/').next().unwrap_or_default().to_owned(),
        path,
        address: p.string("Address").unwrap_or_default(),
        alias: p.string("Alias").or_else(|| p.string("Name")).unwrap_or_default(),
        powered,
        power_state: PowerState::parse(p.string("PowerState").as_deref(), powered),
        discoverable: p.bool("Discoverable").unwrap_or(false),
        discoverable_timeout: p.u32("DiscoverableTimeout").unwrap_or(180),
        pairable: p.bool("Pairable").unwrap_or(true),
        discovering: p.bool("Discovering").unwrap_or(false),
    }
}

fn device(path: String, p: Props, battery: Option<Props>) -> Device {
    let address = p.string("Address").unwrap_or_default();
    let name = p.string("Name").filter(|n| !n.is_empty());
    Device {
        adapter: p
            .path("Adapter")
            .unwrap_or_else(|| path.rsplit_once('/').map(|(a, _)| a.to_owned()).unwrap_or_default()),
        path,
        alias: p.string("Alias").filter(|a| !a.is_empty()).or_else(|| name.clone()).unwrap_or_else(|| address.clone()),
        address,
        name,
        icon: p.string("Icon").unwrap_or_default(),
        class: p.u32("Class").unwrap_or(0),
        paired: p.bool("Paired").unwrap_or(false),
        bonded: p.bool("Bonded").unwrap_or(false),
        trusted: p.bool("Trusted").unwrap_or(false),
        blocked: p.bool("Blocked").unwrap_or(false),
        connected: p.bool("Connected").unwrap_or(false),
        uuids: p.strings("UUIDs").into_iter().map(|u| u.to_ascii_lowercase()).collect(),
        rssi: p.i16("RSSI"),
        battery: battery.and_then(|b| b.u8("Percentage")),
        legacy_pairing: p.bool("LegacyPairing").unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(values: &[(&str, Value<'static>)]) -> HashMap<String, OwnedValue> {
        values.iter().map(|(k, v)| ((*k).to_owned(), OwnedValue::try_from(v.clone()).unwrap())).collect()
    }

    #[test]
    fn devices_get_a_name_to_show_and_their_battery() {
        let headphones = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA").unwrap();
        let nameless = OwnedObjectPath::try_from("/org/bluez/hci0/dev_BB").unwrap();
        let objects: Objects = HashMap::from([
            (
                headphones,
                HashMap::from([
                    (
                        DEVICE.to_owned(),
                        props(&[
                            ("Address", "AA:00:00:00:00:01".into()),
                            ("Name", "WH-1000XM5".into()),
                            ("Alias", "".into()),
                            ("Icon", "audio-headphones".into()),
                            ("UUIDs", Value::from(vec!["0000110B-0000-1000-8000-00805F9B34FB"])),
                        ]),
                    ),
                    (BATTERY.to_owned(), props(&[("Percentage", 80u8.into())])),
                ]),
            ),
            (nameless, HashMap::from([(DEVICE.to_owned(), props(&[("Address", "BB:00:00:00:00:02".into())]))])),
        ]);
        let snapshot = parse(objects);
        let [headphones, nameless] = &snapshot.devices[..] else { panic!("two devices") };
        assert_eq!((headphones.alias.as_str(), headphones.battery), ("WH-1000XM5", Some(80)));
        assert_eq!(headphones.adapter, "/org/bluez/hci0");
        assert!(headphones.audio() && !headphones.tethering());
        assert_eq!(headphones.kind(), crate::DeviceKind::Headphones);
        assert_eq!((nameless.alias.as_str(), nameless.name.as_deref()), ("BB:00:00:00:00:02", None));
    }

    #[test]
    fn power_state_falls_back_to_powered() {
        assert_eq!(PowerState::parse(Some("off-blocked"), false), PowerState::Blocked);
        assert_eq!(PowerState::parse(None, true), PowerState::On);
        assert_eq!(PowerState::parse(None, false), PowerState::Off);
    }

    #[test]
    fn bluez_errors_read_as_sentences() {
        let refused = |kind: &str, message: &str| Error::Refused { kind: kind.into(), message: message.into() };
        assert_eq!(refused("AuthenticationFailed", "").describe(), "The device didn't accept the pairing.");
        assert_eq!(
            refused("Failed", "Page Timeout").describe(),
            "The device didn't answer. Make sure it's on and nearby."
        );
        assert_eq!(
            refused("Failed", "br-connection-profile-unavailable").describe(),
            "br-connection-profile-unavailable"
        );
    }
}
