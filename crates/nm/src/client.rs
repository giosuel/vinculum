//! The connection to NetworkManager: one snapshot on request, or a new one after every change.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::future::{try_join_all, try_join3};
use futures_util::{Stream, StreamExt, stream};
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, DBusError, MatchRule, MessageStream};

use crate::BUS_NAME;
use crate::model::{
    AccessPoint, ActiveConnection, ConnectionKind, Device, DeviceKind, IpConfig, Port, SavedConnection, Snapshot,
    WifiDevice, WiredDevice,
};
use crate::props::{Props, bytes};
use crate::wifi::Security;

const MANAGER: &str = "/org/freedesktop/NetworkManager";
const SETTINGS: &str = "/org/freedesktop/NetworkManager/Settings";
const IFACE: &str = "org.freedesktop.NetworkManager";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";

/// How long the change signals have to go quiet before a new snapshot is taken. A scan or a
/// connection attempt sends dozens of them in a burst.
const QUIET: Duration = Duration::from_millis(120);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("NetworkManager isn't running")]
    NotRunning,
    /// polkit said no, or had no agent to ask the user with.
    #[error("not allowed: {0}")]
    Denied(String),
    /// NetworkManager refused the request, with its reason.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    DBus(#[from] zbus::Error),
}

impl Error {
    pub(crate) fn from_dbus(error: zbus::Error) -> Self {
        let name = error_name(&error).unwrap_or_default();
        let message = match &error {
            zbus::Error::MethodError(_, Some(message), _) => message.clone(),
            zbus::Error::FDO(fdo) => fdo.description().unwrap_or_default().to_owned(),
            _ => String::new(),
        };
        if name == "org.freedesktop.DBus.Error.ServiceUnknown" {
            Self::NotRunning
        } else if name.ends_with(".PermissionDenied") || name == "org.freedesktop.DBus.Error.AccessDenied" {
            Self::Denied(message)
        } else if name.starts_with("org.freedesktop.NetworkManager") && !message.is_empty() {
            Self::Refused(message)
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

    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
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

    /// Switches Wi-Fi on or off (what airplane mode does to Wi-Fi, but only Wi-Fi).
    pub async fn set_wireless_enabled(&self, on: bool) -> Result<(), Error> {
        self.conn
            .call_method(
                Some(BUS_NAME),
                MANAGER,
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &(IFACE, "WirelessEnabled", Value::from(on)),
            )
            .await
            .map_err(Error::from_dbus)?;
        Ok(())
    }

    /// Makes NetworkManager count a device's traffic, updated every `refresh_ms`.
    pub async fn count_traffic(&self, device: &str, refresh_ms: u32) -> Result<(), Error> {
        self.conn
            .call_method(
                Some(BUS_NAME),
                device,
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &("org.freedesktop.NetworkManager.Device.Statistics", "RefreshRateMs", Value::from(refresh_ms)),
            )
            .await
            .map_err(Error::from_dbus)?;
        Ok(())
    }

    /// Airplane mode: Wi-Fi and mobile broadband off (or back on).
    pub async fn set_airplane(&self, on: bool) -> Result<(), Error> {
        for property in ["WirelessEnabled", "WwanEnabled"] {
            self.conn
                .call_method(
                    Some(BUS_NAME),
                    MANAGER,
                    Some("org.freedesktop.DBus.Properties"),
                    "Set",
                    &(IFACE, property, Value::from(!on)),
                )
                .await
                .map_err(Error::from_dbus)?;
        }
        Ok(())
    }

    /// Asks a Wi-Fi device to scan; new access points arrive through [`Client::watch`].
    pub async fn request_scan(&self, device: &str) -> Result<(), Error> {
        let options: HashMap<&str, Value> = HashMap::new();
        self.call::<_, ()>(device, WIRELESS, "RequestScan", &(options,)).await.map_err(Error::from_dbus)
    }

    /// Activates a saved connection, on `device` (or wherever NetworkManager picks, with `None`)
    /// and, for Wi-Fi, through a given access point. Returns the active connection's path.
    pub async fn activate(
        &self,
        connection: &str,
        device: Option<&str>,
        specific: Option<&str>,
    ) -> Result<String, Error> {
        let body =
            (object_path(connection)?, object_path(device.unwrap_or("/"))?, object_path(specific.unwrap_or("/"))?);
        let active: OwnedObjectPath =
            self.call(MANAGER, IFACE, "ActivateConnection", &body).await.map_err(Error::from_dbus)?;
        Ok(active.to_string())
    }

    /// Joins a network that needs no password (open, or Enhanced Open): NetworkManager makes the
    /// profile from the access point. Returns the new connection's and the active connection's
    /// paths.
    pub async fn connect_open(&self, device: &str, access_point: &str, ssid: &[u8]) -> Result<(String, String), Error> {
        let name = String::from_utf8_lossy(ssid).into_owned();
        let settings: HashMap<&str, HashMap<&str, Value>> = HashMap::from([
            ("connection", HashMap::from([("type", Value::from("802-11-wireless")), ("id", Value::from(name))])),
            (
                "802-11-wireless",
                HashMap::from([("ssid", Value::from(ssid.to_vec())), ("mode", Value::from("infrastructure"))]),
            ),
        ]);
        let body = (settings, object_path(device)?, object_path(access_point)?);
        let (connection, active): (OwnedObjectPath, OwnedObjectPath) =
            self.call(MANAGER, IFACE, "AddAndActivateConnection", &body).await.map_err(Error::from_dbus)?;
        Ok((connection.to_string(), active.to_string()))
    }

    /// Uses a phone's internet over Bluetooth (PAN): makes a connection for it on NetworkManager's
    /// Bluetooth device for the phone, and activates it. Returns the new connection's and the
    /// active connection's paths.
    pub async fn tether(&self, device: &str, address: &str, name: &str) -> Result<(String, String), Error> {
        let bdaddr: Vec<u8> = address.split(':').filter_map(|b| u8::from_str_radix(b, 16).ok()).collect::<Vec<u8>>();
        let settings: HashMap<&str, HashMap<&str, Value>> = HashMap::from([
            (
                "connection",
                HashMap::from([
                    ("type", Value::from("bluetooth")),
                    ("id", Value::from(format!("{name} (Bluetooth)"))),
                    ("autoconnect", Value::from(false)),
                ]),
            ),
            ("bluetooth", HashMap::from([("bdaddr", Value::from(bdaddr)), ("type", Value::from("panu"))])),
            ("ipv4", HashMap::from([("method", Value::from("auto"))])),
            ("ipv6", HashMap::from([("method", Value::from("auto"))])),
        ]);
        let body = (settings, object_path(device)?, object_path("/")?);
        let (connection, active): (OwnedObjectPath, OwnedObjectPath) =
            self.call(MANAGER, IFACE, "AddAndActivateConnection", &body).await.map_err(Error::from_dbus)?;
        Ok((connection.to_string(), active.to_string()))
    }

    /// Disconnects an active connection; the device stays available for others.
    pub async fn deactivate(&self, active_connection: &str) -> Result<(), Error> {
        self.call::<_, ()>(MANAGER, IFACE, "DeactivateConnection", &(object_path(active_connection)?,))
            .await
            .map_err(Error::from_dbus)
    }

    /// Deletes a saved connection (disconnecting it first if it's active).
    pub async fn forget(&self, connection: &str) -> Result<(), Error> {
        self.call::<_, ()>(connection, "org.freedesktop.NetworkManager.Settings.Connection", "Delete", &())
            .await
            .map_err(Error::from_dbus)
    }

    pub(crate) async fn call<B, R>(&self, path: &str, interface: &str, method: &str, body: &B) -> zbus::Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let reply = self.conn.call_method(Some(BUS_NAME), path, Some(interface), method, body).await?;
        reply.body().deserialize()
    }

    async fn read(&self) -> zbus::Result<Snapshot> {
        let nm = Props::get_all(&self.conn, MANAGER, IFACE).await?;
        let settings = Props::get_all(&self.conn, SETTINGS, "org.freedesktop.NetworkManager.Settings").await?;
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
            wwan_enabled: nm.bool("WwanEnabled").unwrap_or(true),
            devices: devices.into_iter().flatten().collect(),
            connections: connections.into_iter().flatten().collect(),
            active: active.into_iter().flatten().collect(),
            primary_connection: nm.path("PrimaryConnection"),
            hostname: settings.string("Hostname").unwrap_or_default(),
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
            DeviceKind::Bluetooth => {
                // The phone's Bluetooth address.
                if hw_address.is_empty() {
                    let bt =
                        Props::get_all(&self.conn, path, "org.freedesktop.NetworkManager.Device.Bluetooth").await?;
                    hw_address = bt.string("HwAddress").unwrap_or_default();
                }
                (None, None)
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
        // Counted only after count_traffic switched it on; older versions have no such interface.
        let traffic = match Props::get_all(&self.conn, path, "org.freedesktop.NetworkManager.Device.Statistics").await {
            Ok(stats) if stats.u32("RefreshRateMs").unwrap_or(0) > 0 => {
                Some((stats.u64("RxBytes").unwrap_or(0), stats.u64("TxBytes").unwrap_or(0)))
            }
            _ => None,
        };
        Ok(Device {
            traffic,
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
            "bluetooth" => ConnectionKind::Bluetooth {
                address: get("bluetooth", "bdaddr")
                    .map(|v| bytes(v).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"))
                    .unwrap_or_default(),
            },
            "wireguard" => ConnectionKind::WireGuard,
            "vpn" => ConnectionKind::Vpn { service: string("vpn", "service-type").unwrap_or_default() },
            // NetworkManager turns spanning tree on unless told otherwise.
            "bridge" => ConnectionKind::Bridge {
                stp: get("bridge", "stp").and_then(|v| bool::try_from(&**v).ok()).unwrap_or(true),
            },
            "bond" => ConnectionKind::Bond {
                mode: get("bond", "options")
                    .and_then(|v| <HashMap<String, String>>::try_from(v.clone()).ok())
                    .and_then(|o| o.get("mode").cloned())
                    .unwrap_or_else(|| "balance-rr".into()),
            },
            "vlan" => ConnectionKind::Vlan {
                parent: string("vlan", "parent").unwrap_or_default(),
                id: get("vlan", "id").and_then(|v| u32::try_from(&**v).ok()).unwrap_or(0),
            },
            other => ConnectionKind::Other(other.to_owned()),
        };
        // Newer NetworkManager says controller and port-type; both are kept in step.
        let controller = string("connection", "controller").or_else(|| string("connection", "master"));
        let port_type = string("connection", "port-type").or_else(|| string("connection", "slave-type"));
        let port = match (controller, port_type) {
            (Some(controller), Some(kind)) if !controller.is_empty() && !kind.is_empty() => {
                Some(Port { controller, kind })
            }
            _ => None,
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
            hotspot: string("802-11-wireless", "mode").as_deref() == Some("ap"),
            port,
            // NMMetered: 1 yes, 2 no, 3 guessed yes, 4 guessed no; 0 (unknown) means automatic.
            metered: match get("connection", "metered").and_then(|v| i32::try_from(&**v).ok()) {
                Some(1 | 3) => Some(true),
                Some(2 | 4) => Some(false),
                _ => None,
            },
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

pub(crate) fn object_path(path: &str) -> Result<ObjectPath<'_>, Error> {
    ObjectPath::try_from(path).map_err(|e| Error::DBus(e.into()))
}

async fn skip_vanished<T>(read: impl Future<Output = zbus::Result<T>>) -> zbus::Result<Option<T>> {
    match read.await {
        Ok(value) => Ok(Some(value)),
        Err(error) if vanished(&error) => Ok(None),
        Err(error) => Err(error),
    }
}
