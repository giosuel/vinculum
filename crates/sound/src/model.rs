//! What the sound server has: outputs and inputs, the apps playing, and the sound cards.

/// PulseAudio's 100 % on a channel.
pub const NORM: u32 = 0x10000;

/// Everything the Sound page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// "PulseAudio (on PipeWire 1.6.2)".
    pub server: String,
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
    pub sinks: Vec<Device>,
    /// Inputs, without the monitors of the outputs.
    pub sources: Vec<Device>,
    /// The apps playing (sink inputs).
    pub streams: Vec<Stream>,
    pub cards: Vec<Card>,
}

impl Snapshot {
    pub fn sink(&self, name: &str) -> Option<&Device> {
        self.sinks.iter().find(|d| d.name == name)
    }

    pub fn source(&self, name: &str) -> Option<&Device> {
        self.sources.iter().find(|d| d.name == name)
    }

    /// The output in use: the default, else the first.
    pub fn default_sink(&self) -> Option<&Device> {
        self.default_sink.as_deref().and_then(|n| self.sink(n)).or_else(|| self.sinks.first())
    }

    pub fn default_source(&self) -> Option<&Device> {
        self.default_source.as_deref().and_then(|n| self.source(n)).or_else(|| self.sources.first())
    }

    pub fn card(&self, index: u32) -> Option<&Card> {
        self.cards.iter().find(|c| c.index == index)
    }
}

/// An output (sink) or an input (source).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub index: u32,
    pub name: String,
    pub description: String,
    /// The freedesktop icon name ("audio-headphones"), when the server gives one.
    pub icon: String,
    /// "headphones", "headset", "speaker", "hdmi", "tv", "webcam", "microphone"…
    pub form_factor: String,
    /// "pci", "usb", "bluetooth".
    pub bus: String,
    pub volume: Volume,
    pub muted: bool,
    pub ports: Vec<Port>,
    pub active_port: Option<String>,
    pub card: Option<u32>,
}

impl Device {
    /// What kind of thing it is, for its icon: "headphones", "headset", "speaker", "tv",
    /// "webcam" or "microphone".
    pub fn kind(&self) -> &'static str {
        let f = self.form_factor.as_str();
        let i = self.icon.as_str();
        let port = self.active_port.as_deref().unwrap_or("");
        if f == "headset" || i.contains("headset") {
            "headset"
        } else if f == "headphone" || f == "headphones" || i.contains("headphone") || port.contains("headphone") {
            "headphones"
        } else if f == "tv" || f == "hdmi" || i.contains("video-display") || port.contains("hdmi") {
            "tv"
        } else if f == "webcam" || i.contains("camera") {
            "webcam"
        } else if f == "microphone" || i.contains("microphone") || i.contains("input") || port.contains("mic") {
            "microphone"
        } else {
            "speaker"
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub description: String,
    /// `Some(false)`: unplugged; `None`: the server doesn't know.
    pub available: Option<bool>,
}

/// An app playing sound.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stream {
    pub index: u32,
    /// "Firefox".
    pub app: String,
    /// What it plays: "YouTube – Music".
    pub title: String,
    pub icon: String,
    /// The output it plays through.
    pub sink: u32,
    pub volume: Volume,
    pub muted: bool,
    /// Paused.
    pub corked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub profiles: Vec<Profile>,
    pub active: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub available: bool,
}

/// A volume per channel, in PulseAudio's units (`NORM` is 100 %); with the channels' positions
/// for the balance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    pub channels: Vec<u32>,
    /// -1 left, 1 right, 0 anything else; one per channel.
    pub sides: Vec<i8>,
}

impl Volume {
    /// The loudest channel, in percent.
    pub fn percent(&self) -> u32 {
        let max = self.channels.iter().copied().max().unwrap_or(0);
        ((u64::from(max) * 100 + u64::from(NORM) / 2) / u64::from(NORM)) as u32
    }

    fn side_max(&self, side: i8) -> Option<u32> {
        self.channels.iter().zip(&self.sides).filter(|(_, s)| **s == side).map(|(v, _)| *v).max()
    }

    /// -1 (all left) to 1 (all right); 0 when it has no left and right.
    pub fn balance(&self) -> f32 {
        match (self.side_max(-1), self.side_max(1)) {
            (Some(l), Some(r)) if l.max(r) > 0 => {
                if r >= l {
                    1.0 - l as f32 / r as f32
                } else {
                    r as f32 / l as f32 - 1.0
                }
            }
            _ => 0.0,
        }
    }

    pub fn has_balance(&self) -> bool {
        self.sides.contains(&-1) && self.sides.contains(&1)
    }

    /// The same balance at another loudness.
    pub fn with_percent(&self, percent: u32) -> Volume {
        self.shaped(u64::from(percent) * u64::from(NORM) / 100, self.balance())
    }

    /// The same loudness with another balance.
    pub fn with_balance(&self, balance: f32) -> Volume {
        let max = u64::from(self.channels.iter().copied().max().unwrap_or(0));
        self.shaped(max, balance.clamp(-1.0, 1.0))
    }

    fn shaped(&self, max: u64, balance: f32) -> Volume {
        let max = max.min(u64::from(u32::MAX)) as f32;
        let channels = self
            .sides
            .iter()
            .map(|side| {
                let factor = match side {
                    -1 if balance > 0.0 => 1.0 - balance,
                    1 if balance < 0.0 => 1.0 + balance,
                    _ => 1.0,
                };
                (max * factor).round() as u32
            })
            .collect();
        Volume { channels, sides: self.sides.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(l: u32, r: u32) -> Volume {
        Volume { channels: vec![l, r], sides: vec![-1, 1] }
    }

    #[test]
    fn percent_is_the_loudest_channel() {
        assert_eq!(stereo(NORM, NORM).percent(), 100);
        assert_eq!(stereo(NORM / 2, NORM / 4).percent(), 50);
        assert_eq!(Volume::default().percent(), 0);
    }

    #[test]
    fn balance_keeps_through_volume_changes() {
        let v = stereo(NORM, NORM / 2);
        assert!((v.balance() + 0.5).abs() < 0.01, "{}", v.balance());
        let louder = v.with_percent(80);
        assert_eq!(louder.percent(), 80);
        assert!((louder.balance() + 0.5).abs() < 0.01);
        let centred = louder.with_balance(0.0);
        assert_eq!(centred.channels[0], centred.channels[1]);
        let right = centred.with_balance(1.0);
        assert_eq!(right.channels[0], 0);
        assert_eq!(right.percent(), 80);
    }

    #[test]
    fn mono_has_no_balance() {
        let mono = Volume { channels: vec![NORM], sides: vec![0] };
        assert!(!mono.has_balance());
        assert_eq!(mono.with_percent(30).channels, [NORM * 30 / 100]);
    }

    #[test]
    fn device_kinds() {
        let d = |form: &str, icon: &str| Device { form_factor: form.into(), icon: icon.into(), ..Device::default() };
        assert_eq!(d("headphone", "").kind(), "headphones");
        assert_eq!(d("", "audio-headset-bluetooth").kind(), "headset");
        assert_eq!(d("", "video-display").kind(), "tv");
        assert_eq!(d("", "audio-input-microphone").kind(), "microphone");
        assert_eq!(d("", "audio-card-analog-pci").kind(), "speaker");
    }
}
