//! Hyprland over its sockets, in pure Rust: the monitors and workspaces, live events, and the
//! monitor and workspace rules Lituus writes to its own config file and applies live.

mod config;
mod ipc;
mod model;
mod rules;

pub use config::{ConfigFile, config_dir};
pub use ipc::{Error, Hyprland};
pub use model::{HardwareDetails, Mode, Monitor, Version, Workspace, WorkspaceRef, scale_is_valid, valid_scales};
pub use rules::{MonitorRule, WorkspaceRule, file};
