//! A Bluetooth headset's audio profile (high quality, headset with microphone, or off), which
//! PipeWire owns, not BlueZ. Read and changed through pipewire-pulse's PulseAudio protocol with
//! the pure-Rust `pulseaudio` crate: one short connection per read or change, so nothing is
//! kept open. Without pipewire-pulse (or PulseAudio) the audio choice is simply not shown.

use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use pulseaudio::protocol::{self, Command};

/// A Bluetooth device's sound card, as PipeWire shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    /// The device's Bluetooth address, upper case with colons.
    pub address: String,
    pub profiles: Vec<Profile>,
    pub active: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub description: String,
    pub priority: u32,
}

/// One of the three choices: the profile it switches to, and what the picker says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub profile: String,
    pub label: &'static str,
    pub sub: String,
}

/// The choices a card offers (High quality, Headset, Off, as far as it has them), and the one
/// in use.
pub fn choices(card: &Card) -> (Vec<Choice>, Option<usize>) {
    let active = card.active.as_deref().unwrap_or("off");
    // The best of a family: the active one if it's of the family, else PipeWire's plain profile
    // (it picks the codec itself), else the highest priority.
    let best = |prefix: &str| {
        let family: Vec<&Profile> = card.profiles.iter().filter(|p| p.name.starts_with(prefix)).collect();
        family
            .iter()
            .find(|p| p.name == active)
            .or_else(|| family.iter().find(|p| p.name == prefix))
            .or_else(|| family.iter().max_by_key(|p| p.priority))
            .copied()
    };
    let mut choices = Vec::new();
    if let Some(p) = best("a2dp-sink") {
        let sub = match codec(&p.description) {
            Some(codec) => format!("A2DP · {codec}"),
            None => "A2DP".to_owned(),
        };
        choices.push(Choice { profile: p.name.clone(), label: "High quality", sub });
    }
    if let Some(p) = best("headset-head-unit") {
        choices.push(Choice { profile: p.name.clone(), label: "Headset", sub: "HFP · microphone on".into() });
    }
    if choices.is_empty() {
        return (choices, None);
    }
    if card.profiles.iter().any(|p| p.name == "off") {
        choices.push(Choice { profile: "off".into(), label: "Off", sub: "Controls only".into() });
    }
    let selected = choices.iter().position(|c| {
        c.profile == active
            || (active.starts_with("a2dp-sink") && c.profile.starts_with("a2dp-sink"))
            || (active.starts_with("headset-head-unit") && c.profile.starts_with("headset-head-unit"))
    });
    (choices, selected)
}

/// The codec in a profile's description: "High Fidelity Playback (A2DP Sink, codec LDAC)".
fn codec(description: &str) -> Option<&str> {
    let (_, rest) = description.split_once("codec ")?;
    Some(rest.trim_end_matches(')').trim()).filter(|c| !c.is_empty())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No PulseAudio socket: pipewire-pulse isn't running (or isn't installed).
    #[error("PipeWire's PulseAudio service isn't running")]
    NotRunning,
    #[error("{0}")]
    Protocol(#[from] protocol::ProtocolError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// One connection to the sound server.
pub struct Session {
    socket: BufReader<UnixStream>,
    version: u16,
    seq: u32,
}

impl Session {
    /// Connects the way PulseAudio clients do: $PULSE_SERVER, else the runtime folder's socket.
    pub fn connect() -> Result<Self, Error> {
        let path = pulseaudio::socket_path_from_env().ok_or(Error::NotRunning)?;
        let socket = UnixStream::connect(path).map_err(|_| Error::NotRunning)?;
        let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        Self::over(socket, cookie)
    }

    /// The handshake, over a connected socket.
    pub fn over(socket: UnixStream, cookie: Vec<u8>) -> Result<Self, Error> {
        // A stuck server mustn't hang the Bluetooth thread's helper for ever.
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut session = Self { socket: BufReader::new(socket), version: protocol::MAX_VERSION, seq: 0 };
        let auth =
            protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
        let reply: protocol::AuthReply = session.ask(Command::Auth(auth))?;
        session.version = protocol::MAX_VERSION.min(reply.version);
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::ApplicationName, c"Lituus");
        let _: protocol::SetClientNameReply = session.ask(Command::SetClientName(props))?;
        Ok(session)
    }

    fn send(&mut self, command: Command) -> Result<(), Error> {
        self.seq += 1;
        protocol::write_command_message(self.socket.get_mut(), self.seq, &command, self.version)?;
        Ok(())
    }

    fn ask<R: protocol::CommandReply>(&mut self, command: Command) -> Result<R, Error> {
        self.send(command)?;
        Ok(protocol::read_reply_message::<R>(&mut self.socket, self.version)?.1)
    }

    /// The Bluetooth sound cards.
    pub fn cards(&mut self) -> Result<Vec<Card>, Error> {
        let cards: protocol::CardInfoList = self.ask(Command::GetCardInfoList)?;
        Ok(cards.into_iter().filter_map(card).collect())
    }

    pub fn set_profile(&mut self, card: &str, profile: &str) -> Result<(), Error> {
        let invalid = |_| protocol::ProtocolError::Invalid("a name with a NUL".into());
        self.send(Command::SetCardProfile(protocol::SetCardProfileParams {
            card_index: None,
            card_name: Some(CString::new(card).map_err(invalid)?),
            profile_name: CString::new(profile).map_err(invalid)?,
        }))?;
        protocol::read_ack_message(&mut self.socket)?;
        Ok(())
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.strip_suffix(b"\0").unwrap_or(bytes)).into_owned()
}

/// A Bluetooth card, or `None` for any other.
fn card(info: protocol::CardInfo) -> Option<Card> {
    let name = info.name.to_string_lossy().into_owned();
    let prop = |key: &str| info.props.get_bytes(CString::new(key).ok()?).map(text);
    // PipeWire sets api.bluez5.address; PulseAudio names its cards bluez_card.AA_BB_….
    let address = prop("api.bluez5.address")
        .or_else(|| name.strip_prefix("bluez_card.").map(|a| a.replace('_', ":")))?
        .to_ascii_uppercase();
    Some(Card {
        name,
        address,
        profiles: info
            .profiles
            .iter()
            .map(|p| Profile {
                name: p.name.to_string_lossy().into_owned(),
                description: p.description.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
                priority: p.priority,
            })
            .collect(),
        active: info.active_profile.map(|a| a.to_string_lossy().into_owned()),
    })
}

/// The Bluetooth cards, or why there are none to show.
pub fn cards() -> Result<Vec<Card>, Error> {
    Session::connect()?.cards()
}

pub fn set_profile(card: &str, profile: &str) -> Result<(), Error> {
    Session::connect()?.set_profile(card, profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, description: &str, priority: u32) -> Profile {
        Profile { name: name.into(), description: description.into(), priority }
    }

    /// What PipeWire shows for a pair of Sony headphones.
    fn headphones(active: &str) -> Card {
        Card {
            name: "bluez_card.AC_80_0A_3F_92_1D".into(),
            address: "AC:80:0A:3F:92:1D".into(),
            profiles: vec![
                profile("off", "Off", 0),
                profile("a2dp-sink-sbc", "High Fidelity Playback (A2DP Sink, codec SBC)", 18),
                profile("a2dp-sink-aac", "High Fidelity Playback (A2DP Sink, codec AAC)", 19),
                profile("a2dp-sink", "High Fidelity Playback (A2DP Sink, codec LDAC)", 20),
                profile("headset-head-unit-cvsd", "Headset Head Unit (HSP/HFP, codec CVSD)", 1),
                profile("headset-head-unit", "Headset Head Unit (HSP/HFP, codec mSBC)", 2),
            ],
            active: Some(active.into()),
        }
    }

    #[test]
    fn three_choices_from_pipewires_profiles() {
        let (choices, selected) = choices(&headphones("a2dp-sink"));
        let shown: Vec<(&str, &str, &str)> =
            choices.iter().map(|c| (c.label, c.sub.as_str(), c.profile.as_str())).collect();
        assert_eq!(
            shown,
            [
                ("High quality", "A2DP · LDAC", "a2dp-sink"),
                ("Headset", "HFP · microphone on", "headset-head-unit"),
                ("Off", "Controls only", "off"),
            ]
        );
        assert_eq!(selected, Some(0));
        // A codec picked elsewhere (pavucontrol) is still "High quality", with that codec.
        let (choices, selected) = super::choices(&headphones("a2dp-sink-aac"));
        assert_eq!((choices[0].sub.as_str(), selected), ("A2DP · AAC", Some(0)));
        assert_eq!(super::choices(&headphones("headset-head-unit-cvsd")).1, Some(1));
        assert_eq!(super::choices(&headphones("off")).1, Some(2));
        // A keyboard's card (if it had one) has none of these.
        let mut other = headphones("off");
        other.profiles.retain(|p| p.name == "off");
        assert!(super::choices(&other).0.is_empty());
    }

    /// Plays pipewire-pulse on the other end of a socket pair: answers the handshake, lists a
    /// card, and acknowledges a profile change.
    #[test]
    fn reads_cards_and_sets_a_profile_over_the_protocol() {
        let (client, server) = UnixStream::pair().unwrap();
        let fake = std::thread::spawn(move || {
            let mut socket = BufReader::new(server);
            let v = protocol::MAX_VERSION;
            let next = |socket: &mut BufReader<UnixStream>| protocol::read_command_message(socket, v).unwrap();
            let (seq, _) = next(&mut socket);
            let auth = protocol::AuthReply { version: v, ..Default::default() };
            protocol::write_reply_message(socket.get_mut(), seq, &auth, v).unwrap();
            let (seq, _) = next(&mut socket);
            let name = protocol::SetClientNameReply { client_id: 7 };
            protocol::write_reply_message(socket.get_mut(), seq, &name, v).unwrap();
            let (seq, command) = next(&mut socket);
            assert!(matches!(command, Command::GetCardInfoList));
            let mut props = protocol::Props::new();
            props.set_bytes(c"api.bluez5.address", b"ac:80:0a:3f:92:1d\0");
            let bluetooth = protocol::CardInfo {
                index: 1,
                name: c"bluez_card.AC_80_0A_3F_92_1D".into(),
                props,
                owner_module_index: None,
                driver: None,
                ports: Vec::new(),
                profiles: vec![protocol::CardProfileInfo {
                    name: c"a2dp-sink".into(),
                    description: Some(c"High Fidelity Playback (A2DP Sink, codec SBC)".into()),
                    priority: 20,
                    available: 1,
                    num_sinks: 1,
                    num_sources: 0,
                }],
                active_profile: Some(c"a2dp-sink".into()),
            };
            let builtin = protocol::CardInfo {
                index: 0,
                name: c"alsa_card.pci-0000_00_1f.3".into(),
                props: protocol::Props::new(),
                profiles: Vec::new(),
                ..bluetooth.clone()
            };
            let list: protocol::CardInfoList = vec![builtin, bluetooth];
            protocol::write_reply_message(socket.get_mut(), seq, &list, v).unwrap();
            let (seq, command) = next(&mut socket);
            let Command::SetCardProfile(params) = command else { panic!("a profile change") };
            protocol::write_ack_message(socket.get_mut(), seq).unwrap();
            (params.card_name, params.profile_name)
        });
        let mut session = Session::over(client, Vec::new()).unwrap();
        let cards = session.cards().unwrap();
        assert_eq!(cards.len(), 1, "only the Bluetooth card");
        assert_eq!((cards[0].address.as_str(), cards[0].active.as_deref()), ("AC:80:0A:3F:92:1D", Some("a2dp-sink")));
        session.set_profile("bluez_card.AC_80_0A_3F_92_1D", "headset-head-unit").unwrap();
        let (card, profile) = fake.join().unwrap();
        assert_eq!(card.as_deref(), Some(c"bluez_card.AC_80_0A_3F_92_1D"));
        assert_eq!(profile.as_c_str(), c"headset-head-unit");
    }
}
