//! The typed snapshot of NetworkManager's state. Plain data, so the UI can compare and render it
//! without going back to D-Bus. Object paths are kept as strings; "no object" is `None`.

use crate::wifi::Security;

/// Everything Vinculum shows, read at one point in time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub version: String,
    pub networking_enabled: bool,
    /// Wi-Fi switched on in NetworkManager.
    pub wireless_enabled: bool,
    /// Wi-Fi not blocked by a hardware switch or rfkill.
    pub wireless_hardware_enabled: bool,
    pub devices: Vec<Device>,
    /// Saved connection profiles.
    pub connections: Vec<SavedConnection>,
    pub active: Vec<ActiveConnection>,
    /// The active connection that carries the default route.
    pub primary_connection: Option<String>,
}

impl Snapshot {
    pub fn device(&self, path: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.path == path)
    }

    pub fn connection(&self, path: &str) -> Option<&SavedConnection> {
        self.connections.iter().find(|c| c.path == path)
    }

    pub fn active_connection(&self, path: &str) -> Option<&ActiveConnection> {
        self.active.iter().find(|a| a.path == path)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub path: String,
    pub interface: String,
    pub kind: DeviceKind,
    pub state: DeviceState,
    pub driver: String,
    pub hw_address: String,
    pub managed: bool,
    pub active_connection: Option<String>,
    pub ip4: Option<IpConfig>,
    pub ip6: Option<IpConfig>,
    pub wifi: Option<WifiDevice>,
    pub wired: Option<WiredDevice>,
}

/// NMDeviceType, for the kinds Vinculum treats specially.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Ethernet,
    Wifi,
    WireGuard,
    Bridge,
    Bond,
    Vlan,
    Tun,
    Loopback,
    Other(u32),
}

impl From<u32> for DeviceKind {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::Ethernet,
            2 => Self::Wifi,
            10 => Self::Bond,
            11 => Self::Vlan,
            13 => Self::Bridge,
            16 => Self::Tun,
            29 => Self::WireGuard,
            32 => Self::Loopback,
            other => Self::Other(other),
        }
    }
}

/// NMDeviceState.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceState {
    Unknown,
    Unmanaged,
    /// No carrier, or the radio is off.
    Unavailable,
    Disconnected,
    Prepare,
    Config,
    NeedAuth,
    IpConfig,
    IpCheck,
    Secondaries,
    Activated,
    Deactivating,
    Failed,
}

impl From<u32> for DeviceState {
    fn from(value: u32) -> Self {
        match value {
            10 => Self::Unmanaged,
            20 => Self::Unavailable,
            30 => Self::Disconnected,
            40 => Self::Prepare,
            50 => Self::Config,
            60 => Self::NeedAuth,
            70 => Self::IpConfig,
            80 => Self::IpCheck,
            90 => Self::Secondaries,
            100 => Self::Activated,
            110 => Self::Deactivating,
            120 => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

impl DeviceState {
    /// Between "connect" and "connected".
    pub fn is_connecting(self) -> bool {
        (Self::Prepare..=Self::Secondaries).contains(&self)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WifiDevice {
    pub access_points: Vec<AccessPoint>,
    pub active_access_point: Option<String>,
    /// Current bit rate in kb/s.
    pub bitrate: u32,
    /// When the last scan finished, in milliseconds of CLOCK_BOOTTIME; `None` if never.
    pub last_scan: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WiredDevice {
    pub carrier: bool,
    /// Link speed in Mb/s.
    pub speed: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccessPoint {
    pub path: String,
    /// Raw SSID bytes: not necessarily UTF-8.
    pub ssid: Vec<u8>,
    pub bssid: String,
    /// In MHz.
    pub frequency: u32,
    /// 0–100 %.
    pub strength: u8,
    pub security: Security,
    /// In kb/s.
    pub max_bitrate: u32,
}

/// Addresses, gateway and DNS servers of one IP family.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IpConfig {
    /// Address and prefix length.
    pub addresses: Vec<(String, u32)>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedConnection {
    pub path: String,
    pub uuid: String,
    /// The name the user sees.
    pub id: String,
    pub kind: ConnectionKind,
    pub autoconnect: bool,
    pub autoconnect_priority: i32,
    /// Bound to this interface only, if set.
    pub interface: Option<String>,
}

impl SavedConnection {
    pub fn wifi_ssid(&self) -> Option<&[u8]> {
        match &self.kind {
            ConnectionKind::Wifi { ssid } => Some(ssid),
            _ => None,
        }
    }
}

/// The `connection.type` of a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionKind {
    Wifi {
        ssid: Vec<u8>,
    },
    Ethernet,
    WireGuard,
    /// A NetworkManager VPN plugin, e.g. `org.freedesktop.NetworkManager.openvpn`.
    Vpn {
        service: String,
    },
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveConnection {
    pub path: String,
    pub id: String,
    pub uuid: String,
    pub state: ActiveState,
    pub devices: Vec<String>,
    /// The saved connection it was activated from.
    pub connection: Option<String>,
    /// For Wi-Fi, the access point.
    pub specific_object: Option<String>,
    pub vpn: bool,
}

/// NMActiveConnectionState.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveState {
    Unknown,
    Activating,
    Activated,
    Deactivating,
    Deactivated,
}

impl From<u32> for ActiveState {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::Activating,
            2 => Self::Activated,
            3 => Self::Deactivating,
            4 => Self::Deactivated,
            _ => Self::Unknown,
        }
    }
}
