//! A connection to the sound server over the PulseAudio protocol, which pipewire-pulse speaks:
//! read everything into a [`Snapshot`], change volumes, outputs and profiles, and wait for
//! changes on a second connection.

use std::ffi::{CStr, CString};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use pulseaudio::protocol::{self, Command};

use crate::model::{Card, Device, Port, Profile, Snapshot, Stream, Volume};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No PulseAudio socket: pipewire-pulse isn't running (or isn't installed).
    #[error("PipeWire's PulseAudio service isn't running")]
    NotRunning,
    #[error("{0}")]
    Protocol(#[from] protocol::ProtocolError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("a name with a NUL in it")]
    Name,
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
        // pipewire-pulse turns away a handshake without a cookie, and most PipeWire systems have no
        // cookie file; it doesn't check the cookie, so a blank one of the right length does.
        let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).unwrap_or(vec![0; 256]);
        Self::over(socket, cookie)
    }

    /// The handshake, over a connected socket.
    pub fn over(socket: UnixStream, cookie: Vec<u8>) -> Result<Self, Error> {
        // A stuck server mustn't hang the sound thread for ever.
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

    fn run(&mut self, command: Command) -> Result<(), Error> {
        self.send(command)?;
        protocol::read_ack_message(&mut self.socket)?;
        Ok(())
    }

    /// Everything, read afresh.
    pub fn snapshot(&mut self) -> Result<Snapshot, Error> {
        let server: protocol::ServerInfo = self.ask(Command::GetServerInfo)?;
        let sinks: protocol::SinkInfoList = self.ask(Command::GetSinkInfoList)?;
        let sources: protocol::SourceInfoList = self.ask(Command::GetSourceInfoList)?;
        let streams: protocol::SinkInputInfoList = self.ask(Command::GetSinkInputInfoList)?;
        let cards: protocol::CardInfoList = self.ask(Command::GetCardInfoList)?;
        Ok(Snapshot {
            server: match (server.server_name.as_deref(), server.server_version.as_deref()) {
                (Some(n), Some(v)) => format!("{} {}", lossy(n), lossy(v)),
                (Some(n), None) => lossy(n),
                _ => String::new(),
            },
            default_sink: server.default_sink_name.as_deref().map(lossy),
            default_source: server.default_source_name.as_deref().map(lossy),
            sinks: sinks.into_iter().map(sink).collect(),
            sources: sources.into_iter().filter(|s| s.monitor_of_sink_index.is_none()).map(source).collect(),
            streams: streams.into_iter().map(stream).collect(),
            cards: cards.into_iter().map(card).collect(),
        })
    }

    pub fn set_default_sink(&mut self, name: &str) -> Result<(), Error> {
        self.run(Command::SetDefaultSink(cstring(name)?))
    }

    pub fn set_default_source(&mut self, name: &str) -> Result<(), Error> {
        self.run(Command::SetDefaultSource(cstring(name)?))
    }

    pub fn set_sink_volume(&mut self, index: u32, volume: &Volume) -> Result<(), Error> {
        self.run(Command::SetSinkVolume(protocol::SetDeviceVolumeParams {
            device_index: Some(index),
            device_name: None,
            volume: channel_volume(volume),
        }))
    }

    pub fn set_source_volume(&mut self, index: u32, volume: &Volume) -> Result<(), Error> {
        self.run(Command::SetSourceVolume(protocol::SetDeviceVolumeParams {
            device_index: Some(index),
            device_name: None,
            volume: channel_volume(volume),
        }))
    }

    pub fn set_stream_volume(&mut self, index: u32, volume: &Volume) -> Result<(), Error> {
        self.run(Command::SetSinkInputVolume(protocol::SetStreamVolumeParams { index, volume: channel_volume(volume) }))
    }

    pub fn set_sink_mute(&mut self, index: u32, mute: bool) -> Result<(), Error> {
        self.run(Command::SetSinkMute(protocol::SetDeviceMuteParams {
            device_index: Some(index),
            device_name: None,
            mute,
        }))
    }

    pub fn set_source_mute(&mut self, index: u32, mute: bool) -> Result<(), Error> {
        self.run(Command::SetSourceMute(protocol::SetDeviceMuteParams {
            device_index: Some(index),
            device_name: None,
            mute,
        }))
    }

    pub fn set_stream_mute(&mut self, index: u32, mute: bool) -> Result<(), Error> {
        self.run(Command::SetSinkInputMute(protocol::SetStreamMuteParams { index, mute }))
    }

    /// Plays an app through another output.
    pub fn move_stream(&mut self, index: u32, sink: u32) -> Result<(), Error> {
        self.run(Command::MoveSinkInput(protocol::MoveStreamParams {
            index: Some(index),
            device_index: Some(sink),
            device_name: None,
        }))
    }

    pub fn set_sink_port(&mut self, index: u32, port: &str) -> Result<(), Error> {
        self.run(Command::SetSinkPort(protocol::SetPortParams {
            index: Some(index),
            name: None,
            port_name: cstring(port)?,
        }))
    }

    pub fn set_source_port(&mut self, index: u32, port: &str) -> Result<(), Error> {
        self.run(Command::SetSourcePort(protocol::SetPortParams {
            index: Some(index),
            name: None,
            port_name: cstring(port)?,
        }))
    }

    pub fn set_card_profile(&mut self, card: &str, profile: &str) -> Result<(), Error> {
        self.run(Command::SetCardProfile(protocol::SetCardProfileParams {
            card_index: None,
            card_name: Some(cstring(card)?),
            profile_name: cstring(profile)?,
        }))
    }

    /// Turns this connection into a watcher: `changed` is called after every change to the
    /// outputs, inputs, apps, cards or defaults, until the server goes away or `changed` says
    /// to stop.
    pub fn watch(mut self, mut changed: impl FnMut() -> bool) -> Result<(), Error> {
        use protocol::SubscriptionMask as M;
        self.run(Command::Subscribe(M::SINK | M::SOURCE | M::SINK_INPUT | M::SERVER | M::CARD))?;
        // Events come whenever they come.
        self.socket.get_ref().set_read_timeout(None)?;
        loop {
            let event = match protocol::read_command_message(&mut self.socket, self.version) {
                Ok((_, command)) => matches!(command, Command::SubscribeEvent(_)),
                Err(protocol::ProtocolError::Io(e)) => return Err(e.into()),
                // A whole message the crate can't parse (a newer server's, say): still news.
                Err(_) => true,
            };
            if event && !changed() {
                return Ok(());
            }
        }
    }
}

fn cstring(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::Name)
}

fn lossy(s: &CStr) -> String {
    s.to_string_lossy().into_owned()
}

fn prop(props: &protocol::Props, key: &str) -> String {
    let Ok(key) = CString::new(key) else { return String::new() };
    match props.get_bytes(key) {
        Some(bytes) => String::from_utf8_lossy(bytes.strip_suffix(b"\0").unwrap_or(bytes)).into_owned(),
        None => String::new(),
    }
}

fn volume(cvolume: &protocol::ChannelVolume, map: &protocol::ChannelMap) -> Volume {
    let sides = map
        .into_iter()
        .map(|p| {
            let p = format!("{p:?}");
            if p.ends_with("Left") {
                -1
            } else if p.ends_with("Right") {
                1
            } else {
                0
            }
        })
        .collect::<Vec<i8>>();
    let channels: Vec<u32> = cvolume.channels().iter().map(|v| v.as_u32()).collect();
    let sides = if sides.len() == channels.len() { sides } else { vec![0; channels.len()] };
    Volume { channels, sides }
}

fn channel_volume(v: &Volume) -> protocol::ChannelVolume {
    let mut c = protocol::ChannelVolume::empty();
    for &value in &v.channels {
        c.push(protocol::Volume::from_u32_clamped(value));
    }
    c
}

// The crate doesn't export the port type by name, so this is a macro.
macro_rules! ports {
    ($ports:expr) => {
        $ports
            .iter()
            .map(|p| Port {
                name: lossy(&p.name),
                description: p.description.as_deref().map(lossy).unwrap_or_else(|| lossy(&p.name)),
                available: match format!("{:?}", p.available).as_str() {
                    "Yes" => Some(true),
                    "No" => Some(false),
                    _ => None,
                },
            })
            .collect()
    };
}

fn sink(s: protocol::SinkInfo) -> Device {
    Device {
        index: s.index,
        name: lossy(&s.name),
        description: s.description.as_deref().map(lossy).unwrap_or_else(|| lossy(&s.name)),
        icon: prop(&s.props, "device.icon_name"),
        form_factor: prop(&s.props, "device.form_factor"),
        bus: prop(&s.props, "device.bus"),
        volume: volume(&s.cvolume, &s.channel_map),
        muted: s.muted,
        ports: ports!(s.ports),
        active_port: s.ports.get(s.active_port).map(|p| lossy(&p.name)),
        card: s.card_index,
    }
}

fn source(s: protocol::SourceInfo) -> Device {
    Device {
        index: s.index,
        name: lossy(&s.name),
        description: s.description.as_deref().map(lossy).unwrap_or_else(|| lossy(&s.name)),
        icon: prop(&s.props, "device.icon_name"),
        form_factor: prop(&s.props, "device.form_factor"),
        bus: prop(&s.props, "device.bus"),
        volume: volume(&s.cvolume, &s.channel_map),
        muted: s.muted,
        ports: ports!(s.ports),
        active_port: s.ports.get(s.active_port).map(|p| lossy(&p.name)),
        card: s.card_index,
    }
}

fn stream(s: protocol::SinkInputInfo) -> Stream {
    let app = prop(&s.props, "application.name");
    let title = prop(&s.props, "media.name");
    Stream {
        index: s.index,
        app: if app.is_empty() { lossy(&s.name) } else { app },
        title,
        icon: prop(&s.props, "application.icon_name"),
        sink: s.sink_index,
        volume: volume(&s.cvolume, &s.channel_map),
        muted: s.muted,
        corked: s.corked,
    }
}

fn card(c: protocol::CardInfo) -> Card {
    let description = prop(&c.props, "device.description");
    Card {
        index: c.index,
        name: lossy(&c.name),
        description: if description.is_empty() { lossy(&c.name) } else { description },
        profiles: c
            .profiles
            .iter()
            .map(|p| Profile {
                name: lossy(&p.name),
                description: p.description.as_deref().map(lossy).unwrap_or_else(|| lossy(&p.name)),
                available: p.available != 0,
            })
            .collect(),
        active: c.active_profile.as_deref().map(lossy),
    }
}
