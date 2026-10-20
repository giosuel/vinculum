//! NetworkManager over D-Bus, with no UI: the typed model (devices, access points, connections,
//! settings), its change stream, and the secret agent. Filled in from M1 on.

/// NetworkManager's well-known name on the system bus.
pub const BUS_NAME: &str = "org.freedesktop.NetworkManager";
