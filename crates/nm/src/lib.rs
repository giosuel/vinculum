//! NetworkManager over D-Bus, with no UI: a typed snapshot of devices, access points and
//! connections, and a stream of new snapshots whenever NetworkManager reports a change.
//!
//! Properties are read with `GetAll` and parsed leniently: NetworkManager versions differ in
//! which properties they have, and a missing one shouldn't make the whole snapshot fail.

pub mod agent;
mod client;
mod join;
mod model;
mod props;
mod settings;
mod wifi;

pub use agent::{AgentConnection, AgentEvent, Refusal, SecretRequest, Settings};
pub use client::{Client, Error};
pub use join::{KeyMgmt, Outcome, Storage, WifiJoin};
pub use model::{
    AccessPoint, ActiveConnection, ActiveState, ConnectionKind, Device, DeviceKind, DeviceState, IpConfig, Port,
    SavedConnection, Snapshot, WifiDevice, WiredDevice,
};
pub use wifi::{Band, Network, Security, bars, channel, networks};

/// NetworkManager's well-known name on the system bus.
pub const BUS_NAME: &str = "org.freedesktop.NetworkManager";
