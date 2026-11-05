//! Airplane mode: every radio off (Wi-Fi, mobile broadband and Bluetooth), and back on again.
//!
//! As in KDE Plasma, it's a mode the user turns on, not a reading of the radios: turning Wi-Fi
//! off isn't airplane mode, even with nothing else on. The switch shows it on once every radio
//! has gone off, and it ends when one comes back on, here or elsewhere. Turning it off brings
//! back the radios that were on before (all of them, if none was).

/// Which radios are on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Radios {
    pub wifi: bool,
    pub wwan: bool,
    /// The powered Bluetooth adapters.
    pub bluetooth: Vec<String>,
}

impl Radios {
    pub fn read(nm: Option<&lituus_nm::Snapshot>, bt: Option<&lituus_bt::Snapshot>) -> Self {
        Self {
            wifi: nm.is_some_and(|s| s.wireless_enabled),
            wwan: nm.is_some_and(|s| s.wwan_enabled),
            bluetooth: adapters(bt, |a| a.powered),
        }
    }

    /// Every radio there is, but Bluetooth adapters a switch blocks.
    fn all(bt: Option<&lituus_bt::Snapshot>) -> Self {
        Self { wifi: true, wwan: true, bluetooth: adapters(bt, |a| !a.blocked()) }
    }

    pub fn any(&self) -> bool {
        self.wifi || self.wwan || !self.bluetooth.is_empty()
    }
}

fn adapters(bt: Option<&lituus_bt::Snapshot>, keep: impl Fn(&lituus_bt::Adapter) -> bool) -> Vec<String> {
    bt.map(|s| s.adapters.iter().filter(|a| keep(a)).map(|a| a.path.clone()).collect()).unwrap_or_default()
}

/// Airplane mode, from when it's turned on until it ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Airplane {
    /// The radios that were on, to turn back on.
    before: Radios,
    /// Every radio has been off since.
    settled: bool,
}

/// What turning airplane mode on or off changes.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    /// NetworkManager's Wi-Fi and mobile broadband switches.
    pub wifi: bool,
    pub wwan: bool,
    /// Bluetooth adapters to power on (`true`) or off.
    pub bluetooth: Vec<(String, bool)>,
}

/// Turns airplane mode on or off, from what's on now.
pub fn switch(
    state: &mut Option<Airplane>,
    on: bool,
    nm: Option<&lituus_nm::Snapshot>,
    bt: Option<&lituus_bt::Snapshot>,
) -> Change {
    let now = Radios::read(nm, bt);
    let want = if on {
        // Asked again before it settled: what was on the first time is still what comes back.
        let before = state.take().map_or_else(|| now.clone(), |a| a.before);
        *state = Some(Airplane { before, settled: false });
        Radios::default()
    } else {
        state.take().map(|a| a.before).filter(Radios::any).unwrap_or_else(|| Radios::all(bt))
    };
    let mut bluetooth: Vec<(String, bool)> =
        now.bluetooth.iter().filter(|a| !want.bluetooth.contains(a)).map(|a| (a.clone(), false)).collect();
    bluetooth.extend(want.bluetooth.iter().filter(|a| !now.bluetooth.contains(a)).map(|a| (a.clone(), true)));
    Change { wifi: want.wifi, wwan: want.wwan, bluetooth }
}

/// Follows the radios: airplane mode settles once they're all off, and ends when one comes back
/// on after that. Returns whether the switch shows it on.
pub fn follow(
    state: &mut Option<Airplane>,
    nm: Option<&lituus_nm::Snapshot>,
    bt: Option<&lituus_bt::Snapshot>,
) -> bool {
    let Some(airplane) = state else { return false };
    if !Radios::read(nm, bt).any() {
        airplane.settled = true;
    } else if airplane.settled {
        *state = None;
    }
    state.as_ref().is_some_and(|a| a.settled)
}

#[cfg(test)]
mod tests {
    use lituus_bt::{Adapter, PowerState};

    use super::*;
    use crate::view::tests::home;

    fn bluez(powered: bool) -> lituus_bt::Snapshot {
        let adapter = Adapter {
            path: "/org/bluez/hci0".into(),
            name: "hci0".into(),
            address: "00:1A:7D:DA:71:13".into(),
            alias: "thinkpad-x1".into(),
            powered,
            power_state: if powered { PowerState::On } else { PowerState::Off },
            discoverable: false,
            discoverable_timeout: 180,
            pairable: true,
            discovering: false,
        };
        lituus_bt::Snapshot { adapters: vec![adapter], devices: Vec::new() }
    }

    #[test]
    fn wifi_off_is_not_airplane_mode() {
        let mut nm = home();
        nm.wireless_enabled = false;
        nm.wwan_enabled = false;
        let mut state = None;
        // Nothing on at all, but the user only turned Wi-Fi off.
        assert!(!follow(&mut state, Some(&nm), Some(&bluez(false))));
        assert!(!follow(&mut state, Some(&nm), None));
    }

    #[test]
    fn airplane_mode_turns_every_radio_off_and_back_on() {
        let (mut nm, mut bt) = (home(), bluez(true));
        nm.wwan_enabled = false;
        let mut state = None;
        let change = switch(&mut state, true, Some(&nm), Some(&bt));
        assert_eq!(change, Change { wifi: false, wwan: false, bluetooth: vec![("/org/bluez/hci0".into(), false)] });
        // Wi-Fi goes off first: not every radio is off yet.
        nm.wireless_enabled = false;
        assert!(!follow(&mut state, Some(&nm), Some(&bt)));
        bt = bluez(false);
        assert!(follow(&mut state, Some(&nm), Some(&bt)));
        // Off again: what was on comes back, mobile broadband stays off.
        let change = switch(&mut state, false, Some(&nm), Some(&bt));
        assert_eq!(change, Change { wifi: true, wwan: false, bluetooth: vec![("/org/bluez/hci0".into(), true)] });
        assert!(!follow(&mut state, Some(&nm), Some(&bt)));
    }

    #[test]
    fn a_radio_coming_back_on_ends_airplane_mode() {
        let mut nm = home();
        let mut state = None;
        switch(&mut state, true, Some(&nm), Some(&bluez(false)));
        nm.wireless_enabled = false;
        nm.wwan_enabled = false;
        assert!(follow(&mut state, Some(&nm), Some(&bluez(false))));
        // Bluetooth turned on from its page: airplane mode is over, and stays over when
        // Bluetooth goes off again.
        assert!(!follow(&mut state, Some(&nm), Some(&bluez(true))));
        assert_eq!(state, None);
        assert!(!follow(&mut state, Some(&nm), Some(&bluez(false))));
    }

    #[test]
    fn airplane_mode_with_nothing_on_brings_everything_back() {
        let mut nm = home();
        nm.wireless_enabled = false;
        nm.wwan_enabled = false;
        let bt = bluez(false);
        let mut state = None;
        assert_eq!(switch(&mut state, true, Some(&nm), Some(&bt)).bluetooth, []);
        assert!(follow(&mut state, Some(&nm), Some(&bt)));
        let change = switch(&mut state, false, Some(&nm), Some(&bt));
        assert_eq!(change, Change { wifi: true, wwan: true, bluetooth: vec![("/org/bluez/hci0".into(), true)] });
    }

    #[test]
    fn asking_twice_keeps_what_was_on_the_first_time() {
        let mut nm = home();
        let mut state = None;
        switch(&mut state, true, Some(&nm), Some(&bluez(true)));
        // Wi-Fi went off, Bluetooth didn't (yet); the switch is pressed again.
        nm.wireless_enabled = false;
        switch(&mut state, true, Some(&nm), Some(&bluez(true)));
        assert!(switch(&mut state, false, Some(&nm), Some(&bluez(false))).wifi);
    }
}
