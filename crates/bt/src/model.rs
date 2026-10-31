//! The typed snapshot of BlueZ's state. Plain data, so the UI can compare and render it without
//! going back to D-Bus.

/// Everything Lituus shows, read at one point in time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub adapters: Vec<Adapter>,
    /// The devices of every adapter: paired ones, and the ones nearby that BlueZ has seen.
    pub devices: Vec<Device>,
}

impl Snapshot {
    /// The adapter Lituus shows: the first powered one, else the first.
    pub fn adapter(&self) -> Option<&Adapter> {
        self.adapters.iter().find(|a| a.powered).or_else(|| self.adapters.first())
    }

    pub fn device(&self, path: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.path == path)
    }

    /// The devices of one adapter.
    pub fn devices_of<'a>(&'a self, adapter: &'a str) -> impl Iterator<Item = &'a Device> + 'a {
        self.devices.iter().filter(move |d| d.adapter == adapter)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Adapter {
    pub path: String,
    /// The kernel's name for it, such as `hci0`.
    pub name: String,
    pub address: String,
    /// The name other devices see.
    pub alias: String,
    pub powered: bool,
    pub power_state: PowerState,
    pub discoverable: bool,
    /// Seconds until it stops being discoverable; 0 is never.
    pub discoverable_timeout: u32,
    pub pairable: bool,
    pub discovering: bool,
}

impl Adapter {
    /// Blocked by rfkill: airplane mode or a hardware switch.
    pub fn blocked(&self) -> bool {
        self.power_state == PowerState::Blocked
    }
}

/// The adapter's `PowerState` (BlueZ 5.66 and later; older ones are read from `Powered`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerState {
    On,
    Off,
    TurningOn,
    TurningOff,
    /// rfkill blocks the radio.
    Blocked,
}

impl PowerState {
    pub(crate) fn parse(value: Option<&str>, powered: bool) -> Self {
        match value {
            Some("on") => Self::On,
            Some("off-enabling") => Self::TurningOn,
            Some("on-disabling") => Self::TurningOff,
            Some("off-blocked") => Self::Blocked,
            Some("off") => Self::Off,
            _ if powered => Self::On,
            _ => Self::Off,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub path: String,
    pub adapter: String,
    pub address: String,
    /// The name the device gives itself, when it has told.
    pub name: Option<String>,
    /// The name to show: the user's own name for it, else its name, else its address.
    pub alias: String,
    /// The freedesktop icon name BlueZ picks from the device class, such as `audio-headphones`.
    pub icon: String,
    pub class: u32,
    pub paired: bool,
    pub bonded: bool,
    pub trusted: bool,
    pub blocked: bool,
    pub connected: bool,
    /// The services it offers, as 128-bit UUIDs in lower case.
    pub uuids: Vec<String>,
    /// Signal strength in dBm, while it's being discovered.
    pub rssi: Option<i16>,
    /// Battery level in percent, from BlueZ's `Battery1`.
    pub battery: Option<u8>,
    /// Pairs with a PIN instead of a passkey (Bluetooth 2.0 and older).
    pub legacy_pairing: bool,
}

/// What kind of thing a device is, for its icon and for what the details page offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Headphones,
    Headset,
    Speaker,
    Phone,
    Computer,
    Keyboard,
    Mouse,
    Gamepad,
    Other,
}

impl Device {
    pub fn kind(&self) -> DeviceKind {
        match self.icon.as_str() {
            "audio-headphones" => DeviceKind::Headphones,
            "audio-headset" => DeviceKind::Headset,
            "audio-card" | "audio-speakers" | "multimedia-player" => DeviceKind::Speaker,
            "phone" => DeviceKind::Phone,
            "computer" => DeviceKind::Computer,
            "input-keyboard" => DeviceKind::Keyboard,
            "input-mouse" | "input-tablet" => DeviceKind::Mouse,
            "input-gaming" => DeviceKind::Gamepad,
            _ => DeviceKind::Other,
        }
    }

    pub fn offers(&self, uuid: &str) -> bool {
        self.uuids.iter().any(|u| u == uuid)
    }

    /// Plays or records sound: headphones, headsets, speakers.
    pub fn audio(&self) -> bool {
        [uuid::A2DP_SINK, uuid::HFP_HANDS_FREE, uuid::HSP_HEADSET].iter().any(|u| self.offers(u))
    }

    /// Shares its internet connection (Bluetooth tethering).
    pub fn tethering(&self) -> bool {
        self.offers(uuid::NAP) || self.offers(uuid::DUN)
    }

    /// Takes files (OBEX Object Push).
    pub fn takes_files(&self) -> bool {
        self.offers(uuid::OBEX_PUSH)
    }
}

/// The service UUIDs Lituus looks for, as BlueZ writes them.
pub mod uuid {
    /// A2DP sink: plays high-quality sound (headphones, speakers).
    pub const A2DP_SINK: &str = "0000110b-0000-1000-8000-00805f9b34fb";
    /// Hands-Free: a headset's microphone and calls.
    pub const HFP_HANDS_FREE: &str = "0000111e-0000-1000-8000-00805f9b34fb";
    /// Headset Profile, the older Hands-Free.
    pub const HSP_HEADSET: &str = "00001108-0000-1000-8000-00805f9b34fb";
    /// Network Access Point: shares an internet connection (PAN).
    pub const NAP: &str = "00001116-0000-1000-8000-00805f9b34fb";
    /// Dial-Up Networking, older phones' tethering.
    pub const DUN: &str = "00001103-0000-1000-8000-00805f9b34fb";
    /// OBEX Object Push: takes files.
    pub const OBEX_PUSH: &str = "00001105-0000-1000-8000-00805f9b34fb";
}
