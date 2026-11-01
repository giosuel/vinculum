//! What Hyprland says about its monitors and workspaces (`hyprctl -j monitors all`, `workspaces`,
//! `version`), as in Hyprland 0.57's `src/ipc/s1/Commands.cpp`.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Monitor {
    pub id: i64,
    /// The connector: "DP-5".
    pub name: String,
    /// Make, model and serial: "Samsung Electric Company Odyssey G81SF HNAY500052". Rules match
    /// it with `desc:`, which survives the monitor moving to another port.
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    /// The mode in pixels; 0 when disabled.
    pub width: u32,
    pub height: u32,
    /// In millimetres.
    pub physical_width: u32,
    pub physical_height: u32,
    pub refresh_rate: f64,
    pub x: i32,
    pub y: i32,
    pub active_workspace: WorkspaceRef,
    /// Left, top, right, bottom: what bars keep free.
    pub reserved: [i32; 4],
    pub scale: f64,
    pub transform: u8,
    pub focused: bool,
    pub dpms_status: bool,
    pub vrr: bool,
    pub disabled: bool,
    /// "XRGB2101010" for 10 bits.
    pub current_format: String,
    /// The id of the monitor it mirrors, or "none".
    pub mirror_of: String,
    /// "3840x2160@240.00Hz".
    pub available_modes: Vec<String>,
    pub color_management_preset: String,
    pub sdr_brightness: f64,
    pub sdr_saturation: f64,
    pub hardware_details: HardwareDetails,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorkspaceRef {
    pub id: Option<i64>,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct HardwareDetails {
    pub hdr: bool,
    pub bt2020: bool,
    pub vrr_capable: bool,
}

impl Monitor {
    pub fn modes(&self) -> Vec<Mode> {
        let mut modes: Vec<Mode> = self.available_modes.iter().filter_map(|m| Mode::parse(m)).collect();
        modes.dedup();
        modes
    }

    /// The mode in use.
    pub fn mode(&self) -> Mode {
        Mode { width: self.width, height: self.height, refresh: (self.refresh_rate * 1000.0).round() as u32 }
    }

    /// The size on the layout, after scale and rotation.
    pub fn logical_size(&self) -> (i32, i32) {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = ((f64::from(self.width) / scale).round() as i32, (f64::from(self.height) / scale).round() as i32);
        if self.transform % 2 == 1 { (h, w) } else { (w, h) }
    }

    pub fn ten_bit(&self) -> bool {
        self.current_format.contains("2101010")
    }

    /// The diagonal in inches, from the physical size; `None` when the monitor doesn't say.
    pub fn inches(&self) -> Option<u32> {
        (self.physical_width > 0 && self.physical_height > 0).then(|| {
            let mm = f64::from(self.physical_width).hypot(f64::from(self.physical_height));
            (mm / 25.4).round() as u32
        })
    }

    /// The rule selector that finds this monitor wherever it's plugged in: by description when
    /// it has a serial, else by connector.
    pub fn selector(&self) -> String {
        if self.serial.is_empty() || self.description.is_empty() {
            self.name.clone()
        } else {
            format!("desc:{}", self.description)
        }
    }
}

/// A display mode; `refresh` in millihertz.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

impl Mode {
    /// "3840x2160@240.00Hz" (or without "Hz", or without the refresh rate).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_end_matches("Hz");
        let (size, refresh) = match s.split_once('@') {
            Some((size, r)) => (size, (r.parse::<f64>().ok()? * 1000.0).round() as u32),
            None => (s, 0),
        };
        let (w, h) = size.split_once('x')?;
        Some(Self { width: w.parse().ok()?, height: h.parse().ok()?, refresh })
    }

    /// "60" or "59.95": the refresh rate as Hyprland takes it in a rule.
    pub fn refresh_text(&self) -> String {
        let hz = f64::from(self.refresh) / 1000.0;
        let text = format!("{hz:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }

    /// "3840x2160@240", for a rule.
    pub fn rule(&self) -> String {
        if self.refresh == 0 {
            format!("{}x{}", self.width, self.height)
        } else {
            format!("{}x{}@{}", self.width, self.height, self.refresh_text())
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub monitor: String,
    #[serde(rename = "monitorID")]
    pub monitor_id: Option<i64>,
    pub windows: u32,
    #[serde(rename = "ispersistent")]
    pub persistent: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Version {
    /// "0.57.0".
    pub version: String,
    pub tag: String,
}

impl Version {
    /// (major, minor, patch), from `version` or else the tag.
    pub fn numbers(&self) -> Option<(u32, u32, u32)> {
        let v = if self.version.is_empty() { self.tag.trim_start_matches('v') } else { self.version.as_str() };
        let mut parts = v.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse::<u32>());
        Some((parts.next()?.ok()?, parts.next()?.ok()?, parts.next().and_then(Result::ok).unwrap_or(0)))
    }

    /// Hyprland 0.55 and later are configured in Lua; older ones in hyprlang.
    pub fn lua(&self) -> bool {
        self.numbers().is_none_or(|(major, minor, _)| major > 0 || minor >= 55)
    }
}

/// The scales whose logical size comes out in whole pixels in both directions, as Hyprland
/// wants, from 1 to 3 in steps of 1/120 (the fractional-scale protocol's), rounded for showing.
pub fn valid_scales(width: u32, height: u32) -> Vec<f64> {
    (120u32..=360)
        .filter(|&n| (u64::from(width) * 120) % u64::from(n) == 0 && (u64::from(height) * 120) % u64::from(n) == 0)
        .map(|n| f64::from(n) / 120.0)
        .collect()
}

pub fn scale_is_valid(width: u32, height: u32, scale: f64) -> bool {
    valid_scales(width, height).iter().any(|s| (s - scale).abs() < 0.001)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Monitor> {
        serde_json::from_str(include_str!("../tests/fixtures/monitors.json")).unwrap()
    }

    #[test]
    fn reads_hyprctl_monitors() {
        let monitors = fixture();
        let g8 = &monitors[1];
        assert_eq!((g8.name.as_str(), g8.serial.as_str()), ("DP-5", "HNAY500052"));
        assert_eq!(g8.mode(), Mode { width: 3840, height: 2160, refresh: 240_000 });
        assert_eq!(g8.logical_size(), (2560, 1440));
        assert_eq!(g8.selector(), "desc:Samsung Electric Company Odyssey G81SF HNAY500052");
        assert_eq!(g8.modes().len(), 6);
        assert!(g8.focused && g8.hardware_details.hdr);
        assert_eq!(g8.inches(), Some(27));
        assert_eq!(monitors[3].logical_size(), (2560, 1440));
        assert_eq!(monitors[0].mode().rule(), "2560x1440@239.76");
    }

    #[test]
    fn modes_parse_and_print() {
        let m = Mode::parse("2560x1440@59.95Hz").unwrap();
        assert_eq!((m.width, m.refresh), (2560, 59_950));
        assert_eq!(m.rule(), "2560x1440@59.95");
        assert_eq!(Mode::parse("3840x2160@60.00Hz").unwrap().rule(), "3840x2160@60");
        assert_eq!(Mode::parse("1920x1080").unwrap().rule(), "1920x1080");
        assert_eq!(Mode::parse("garbage"), None);
    }

    #[test]
    fn scales_hyprland_takes() {
        assert!(scale_is_valid(3840, 2160, 1.5));
        assert!(scale_is_valid(3840, 2160, 1.25));
        assert!(scale_is_valid(2560, 1440, 1.0));
        assert!(!scale_is_valid(2560, 1440, 1.3));
        let s = valid_scales(2560, 1440);
        assert!(s.contains(&1.25) && s.contains(&2.0) && !s.contains(&1.5));
    }

    #[test]
    fn versions() {
        let v = |s: &str| Version { version: s.into(), tag: String::new() };
        assert!(v("0.57.0").lua());
        assert!(v("0.55.2").lua());
        assert!(!v("0.54.3").lua());
        assert_eq!(Version { version: String::new(), tag: "v0.48.1".into() }.numbers(), Some((0, 48, 1)));
    }
}
