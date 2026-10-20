//! The connection to NetworkManager: one snapshot on request, or a new one after every change.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::future::{try_join_all, try_join3};
use futures_util::{Stream, StreamExt, stream};
use zbus::message::Type as MessageType;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, DBusError, MatchRule, MessageStream};

use crate::BUS_NAME;
use crate::model::{
    AccessPoint, ActiveConnection, ConnectionKind, Device, DeviceKind, IpConfig, SavedConnection, Snapshot, WifiDevice,
    WiredDevice,
};
use crate::props::{Props, bytes};
use crate::wifi::Security;

const MANAGER: &str = "/org/freedesktop/NetworkManager";
const SETTINGS: &str = "/org/freedesktop/NetworkManager/Settings";
const IFACE: &str = "org.freedesktop.NetworkManager";

/// How long the change signals have to go quiet before a new snapshot is taken. A scan or a
/// connection attempt sends dozens of them in a burst.
const QUIET: Duration = Duration::from_millis(120);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NetworkManager isn't running")]
    NotRunning,
    #[error(transparent)]
    DBus(#[from] zbus::Error),
}

impl Error {
    fn from_dbus(error: zbus::Error) -> Self {
        if error_name(&error).as_deref() == Some("org.freedesktop.DBus.Error.ServiceUnknown") {
            Self::NotRunning
        } else {
            Self::DBus(error)
        }
    }
}

/// The D-Bus error name of a method call that failed, if it's one.
fn error_name(error: &zbus::Error) -> Option<String> {
    match error {
        zbus::Error::MethodError(name, _, _) => Some(name.to_string()),
        zbus::Error::FDO(fdo) => Some(fdo.name().to_string()),
        _ => None,
    }
}

/// An object that disappeared between being listed and being read (an access point going out
/// of range during a scan, say). Such objects are skipped rather than failing the snapshot.
fn vanished(error: &zbus::Error) -> bool {
    matches!(
        error_name(error).as_deref(),
        Some("org.freedesktop.DBus.Error.UnknownObject" | "org.freedesktop.DBus.Error.UnknownMethod")
    )
}

#[derive(Clone, Debug)]
pub struct Client {
    conn: Connection,
}

impl Client {
    /// NetworkManager on the system bus.
    pub async fn system() -> Result<Self, Error> {
        Ok(Self::new(Connection::system().await?))
    }

    /// NetworkManager on a given bus (tests use a private one).
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Reads everything once.
    pub async fn snapshot(&self) -> Result<Snapshot, Error> {
        self.read().await.map_err(Error::from_dbus)
    }

    /// The current snapshot, then a new one after every change NetworkManager reports. Bursts of
    /// changes give one snapshot. When NetworkManager stops, the stream yields
    /// [`Error::NotRunning`], and continues once it's back.
    pub async fn watch(&self) -> Result<impl Stream<Item = Result<Snapshot, Error>> + Send + 'static, Error> {
        let from_nm = MatchRule::builder().msg_type(MessageType::Signal).sender(BUS_NAME)?.build();
        let owner = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, BUS_NAME)?
            .build();
        let signals = stream::select(
            MessageStream::for_match_rule(from_nm, &self.conn, Some(1024)).await?,
            MessageStream::for_match_rule(owner, &self.conn, Some(16)).await?,
        );
        let client = self.clone();
        Ok(stream::unfold((client, Box::pin(signals), true), |(client, mut signals, first)| async move {
            if !first {
                // Wait for a change, then for the burst to end.
                // A message or a stream error alike means "look again"; the end means the
                // connection is gone.
                let _ = signals.next().await?;
                while let Ok(Some(_)) = tokio::time::timeout(QUIET, signals.next()).await {}
            }
            let snapshot = client.snapshot().await;
            Some((snapshot, (client, signals, false)))
        }))
    }

    async fn call<B, R>(&self, path: &str, interface: &str, method: &str, body: &B) -> zbus::Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let reply = self.conn.call_method(Some(BUS_NAME), path, Some(interface), method, body).await?;
        reply.body().deserialize()
    }

    async fn read(&self) -> zbus::Result<Snapshot> {
        let nm = Props::get_all(&self.conn, MANAGER, IFACE).await?;
        let device_paths: Vec<zbus::zvariant::OwnedObjectPath> = self.call(MANAGER, IFACE, "GetDevices", &()).await?;
        let connection_paths: Vec<zbus::zvariant::OwnedObjectPath> =
            self.call(SETTINGS, "org.freedesktop.NetworkManager.Settings", "ListConnections", &()).await?;
        let active_paths = nm.paths("ActiveConnections");
        let (devices, connections, active) = try_join3(
            try_join_all(device_paths.iter().map(|p| skip_vanished(self.device(p.as_str())))),
            try_join_all(connection_paths.iter().map(|p| skip_vanished(self.saved_connection(p.as_str())))),
            try_join_all(active_paths.iter().map(|p| skip_vanished(self.active_connection(p)))),
        )
        .await?;
        Ok(Snapshot {
            version: nm.string("Version").unwrap_or_default(),
            networking_enabled: nm.bool("NetworkingEnabled").unwrap_or(true),
            wireless_enabled: nm.bool("WirelessEnabled").unwrap_or(false),
            wireless_hardware_enabled: nm.bool("WirelessHardwareEnabled").unwrap_or(true),
            devices: devices.into_iter().flatten().collect(),
            connections: connections.into_iter().flatten().collect(),
            active: active.into_iter().flatten().collect(),
            primary_connection: nm.path("PrimaryConnection"),
        })
    }

    async fn device(&self, path: &str) -> zbus::Result<Device> {
        let props = Props::get_all(&self.conn, path, "org.freedesktop.NetworkManager.Device").await?;
        let kind = DeviceKind::from(props.u32("DeviceType").unwrap_or(0));
        // Older NetworkManager versions have the MAC address only on the type's own interface.
        let mut hw_address = props.string("HwAddress").unwrap_or_default();
        let (wifi, wired) = match kind {
            DeviceKind::Wifi => {
                let (wifi, address) = self.wifi_device(path).await?;
                hw_address = if hw_address.is_empty() { address } else { hw_address };
                (Some(wifi), None)
            }
            DeviceKind::Ethernet => {
                let wired = Props::get_all(&self.conn, path, "org.freedesktop.NetworkManager.Device.Wired").await?;
                if hw_address.is_empty() {
                    hw_address = wired.string("HwAddress").unwrap_or_default();
                }
                (
                    None,
                    Some(WiredDevice {
                        carrier: wired.bool("Carrier").unwrap_or(false),
                        speed: wired.u32("Speed").unwrap_or(0),
                    }),
                )
            }
            _ => (None, None),
        };
        let ip4 = match props.path("Ip4Config") {
            Some(p) => skip_vanished(self.ip_config(&p, "org.freedesktop.NetworkManager.IP4Config")).await?,
            None => None,
        };
        let ip6 = match props.path("Ip6Config") {
            Some(p) => skip_vanished(self.ip_config(&p, "org.freedesktop.NetworkManager.IP6Config")).await?,
            None => None,
        };
        Ok(Device {
            path: path.to_owned(),
            interface: props.string("Interface").unwrap_or_default(),
            kind,
            state: props.u32("State").unwrap_or(0).into(),
            driver: props.string("Driver").unwrap_or_default(),
            hw_address,
            managed: props.bool("Managed").unwrap_or(true),
            active_connection: props.path("ActiveConnection"),
            ip4,
            ip6,
            wifi,
            wired,
        })
    }

    /// The Wi-Fi details, and the MAC address the wireless interface reports.
    async fn wifi_device(&self, path: &str) -> zbus::Result<(WifiDevice, String)> {
        const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
        let props = Props::get_all(&self.conn, path, WIRELESS).await?;
        let ap_paths: Vec<zbus::zvariant::OwnedObjectPath> =
            self.call(path, WIRELESS, "GetAllAccessPoints", &()).await?;
        let aps = try_join_all(ap_paths.iter().map(|p| skip_vanished(self.access_point(p.as_str())))).await?;
        let wifi = WifiDevice {
            access_points: aps.into_iter().flatten().collect(),
            active_access_point: props.path("ActiveAccessPoint"),
            bitrate: props.u32("Bitrate").unwrap_or(0),
            last_scan: props.i64("LastScan").filter(|&t| t >= 0),
        };
        Ok((wifi, props.string("HwAddress").unwrap_or_default()))
    }

    async fn access_point(&self, path: &str) -> zbus::Result<AccessPoint> {
        let p = Props::get_all(&self.conn, path, "org.freedesktop.NetworkManager.AccessPoint").await?;
        Ok(AccessPoint {
            path: path.to_owned(),
            ssid: p.bytes("Ssid"),
            bssid: p.string("HwAddress").unwrap_or_default(),
            frequency: p.u32("Frequency").unwrap_or(0),
            strength: p.u8("Strength").unwrap_or(0),
            security: Security::from_flags(
                p.u32("Flags").unwrap_or(0),
                p.u32("WpaFlags").unwrap_or(0),
                p.u32("RsnFlags").unwrap_or(0),
            ),
            max_bitrate: p.u32("MaxBitrate").unwrap_or(0),
        })
    }

    async fn ip_config(&self, path: &str, interface: &str) -> zbus::Result<IpConfig> {
        let p = Props::get_all(&self.conn, path, interface).await?;
        let addresses = p
            .dicts("AddressData")
            .iter()
            .filter_map(|d| {
                let address = <&str>::try_from(d.get("address")?).ok()?.to_owned();
                let prefix = u32::try_from(d.get("prefix")?).ok()?;
                Some((address, prefix))
            })
            .collect();
        let dns = p
            .dicts("NameserverData")
            .iter()
            .filter_map(|d| <&str>::try_from(d.get("address")?).ok().map(str::to_owned))
            .collect();
        Ok(IpConfig { addresses, gateway: p.string("Gateway").filter(|g| !g.is_empty()), dns })
    }

    async fn saved_connection(&self, path: &str) -> zbus::Result<SavedConnection> {
        let settings: HashMap<String, HashMap<String, OwnedValue>> =
            self.call(path, "org.freedesktop.NetworkManager.Settings.Connection", "GetSettings", &()).await?;
        let get = |group: &str, key: &str| settings.get(group).and_then(|g| g.get(key));
        let string =
            |group: &str, key: &str| get(group, key).and_then(|v| <&str>::try_from(&**v).ok()).map(str::to_owned);
        let kind = match string("connection", "type").unwrap_or_default().as_str() {
            "802-11-wireless" => {
                ConnectionKind::Wifi { ssid: get("802-11-wireless", "ssid").map(|v| bytes(v)).unwrap_or_default() }
            }
            "802-3-ethernet" => ConnectionKind::Ethernet,
            "wireguard" => ConnectionKind::WireGuard,
            "vpn" => ConnectionKind::Vpn { service: string("vpn", "service-type").unwrap_or_default() },
            other => ConnectionKind::Other(other.to_owned()),
        };
        Ok(SavedConnection {
            path: path.to_owned(),
            uuid: string("connection", "uuid").unwrap_or_default(),
            id: string("connection", "id").unwrap_or_default(),
            kind,
            // NetworkManager's defaults for keys a profile doesn't set.
            autoconnect: get("connection", "autoconnect").and_then(|v| bool::try_from(&**v).ok()).unwrap_or(true),
            autoconnect_priority: get("connection", "autoconnect-priority")
                .and_then(|v| i32::try_from(&**v).ok())
                .unwrap_or(0),
            interface: string("connection", "interface-name").filter(|i| !i.is_empty()),
        })
    }

    async fn active_connection(&self, path: &str) -> zbus::Result<ActiveConnection> {
        let p = Props::get_all(&self.conn, path, "org.freedesktop.NetworkManager.Connection.Active").await?;
        Ok(ActiveConnection {
            path: path.to_owned(),
            id: p.string("Id").unwrap_or_default(),
            uuid: p.string("Uuid").unwrap_or_default(),
            state: p.u32("State").unwrap_or(0).into(),
            devices: p.paths("Devices"),
            connection: p.path("Connection"),
            specific_object: p.path("SpecificObject"),
            vpn: p.bool("Vpn").unwrap_or(false),
        })
    }
}

async fn skip_vanished<T>(read: impl Future<Output = zbus::Result<T>>) -> zbus::Result<Option<T>> {
    match read.await {
        Ok(value) => Ok(Some(value)),
        Err(error) if vanished(&error) => Ok(None),
        Err(error) => Err(error),
    }
}
