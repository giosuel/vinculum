//! BlueZ over D-Bus, with no UI: a typed snapshot of adapters and devices, a stream of new
//! snapshots whenever BlueZ reports a change, and the pairing agent.
//!
//! Everything is read with one `GetManagedObjects` call and parsed leniently, as in
//! `lituus-nm`: BlueZ versions differ in which properties they have.

pub mod agent;
mod client;
mod model;
mod props;

pub use agent::{AgentEvent, Answer, Ask, PairingRequest};
pub use client::{Client, Error};
pub use model::{Adapter, Device, DeviceKind, PowerState, Snapshot, uuid};

/// BlueZ's well-known name on the system bus.
pub const BUS_NAME: &str = "org.bluez";
