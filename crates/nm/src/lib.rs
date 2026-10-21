//! NetworkManager over D-Bus, with no UI: a typed snapshot of devices, access points and
//! connections, and a stream of new snapshots whenever NetworkManager reports a change.
//!
//! Properties are read with `GetAll` and parsed leniently: NetworkManager versions differ in
//! which properties they have, and a missing one shouldn't make the whole snapshot fail.

mod client;
mod model;
mod props;
mod wifi;

pub use client::{Client, Error};
pub use model::{
    AccessPoint, ActiveConnection, ActiveState, ConnectionKind, Device, DeviceKind, DeviceState, IpConfig,
    SavedConnection, Snapshot, WifiDevice, WiredDevice,
};
pub use wifi::{Band, Network, Security, bars, channel, networks};

/// NetworkManager's well-known name on the system bus.
pub const BUS_NAME: &str = "org.freedesktop.NetworkManager";
