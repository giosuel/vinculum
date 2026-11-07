//! Hyprland over its sockets, in pure Rust: the monitors and workspaces, keyboards and pointers,
//! live events, the monitor, workspace and input settings Lituus writes to its own config file
//! and applies live, and the keyboard shortcuts in the user's config.

mod binds;
mod config;
mod input;
mod ipc;
mod model;
mod rules;

pub use binds::{
    Action, Bind, Config as BindConfig, Flags, LiveBind, Unbind, Value as LuaValue, lua_string, modifier_bit,
    normalized_keys, parse_lua, read as read_binds,
};
pub use config::{ConfigFile, config_dir, section, with_section};
pub use input::{
    Device, DeviceSettings, Devices, InputSettings, Keyboard, KeyboardSettings, Layout, MouseSettings, OPTIONS,
    Pointer, TouchpadSettings, Value, option_name,
};
pub use ipc::{Error, Hyprland};
pub use model::{HardwareDetails, Mode, Monitor, Version, Workspace, WorkspaceRef, scale_is_valid, valid_scales};
pub use rules::{MonitorRule, Unreported, WorkspaceRule, file, unreported};
