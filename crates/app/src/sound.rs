//! The Sound page: a thread talks to pipewire-pulse (lituus-sound) and hands the window each new
//! state; the page sends its changes back as commands. Volumes from a dragged slider come fast,
//! so the thread only keeps the last one for each slider.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use lituus_sound::{Session, Snapshot, Volume};
use slint::SharedString;

use crate::{APP, HeroInfo, MainWindow, SoundApp, SoundCard, SoundDevice, SoundView, model, update};

/// What the page asks the sound server for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    DefaultSink(String),
    DefaultSource(String),
    SinkVolume(u32, Volume),
    SourceVolume(u32, Volume),
    StreamVolume(u32, Volume),
    SinkMute(u32, bool),
    SourceMute(u32, bool),
    StreamMute(u32, bool),
    Move(u32, u32),
    SinkPort(u32, String),
    Profile(String, String),
}

impl Command {
    /// Commands that only the last of in a row matters: the same slider, dragged.
    fn replaces(&self, other: &Command) -> bool {
        use Command::*;
        matches!(
            (self, other),
            (SinkVolume(a, _), SinkVolume(b, _)) | (SourceVolume(a, _), SourceVolume(b, _)) | (StreamVolume(a, _), StreamVolume(b, _))
            if a == b
        )
    }
}

enum Message {
    Changed,
    Run(Command),
}

#[derive(Default)]
pub struct State {
    pub snapshot: Option<Snapshot>,
    /// pipewire-pulse isn't there.
    pub not_running: bool,
    pub error: Option<String>,
    pub overamplify: bool,
    commands: Option<Sender<Message>>,
}

/// Starts the sound thread; it reconnects when the server restarts.
pub fn spawn(weak: slint::Weak<MainWindow>) {
    let (tx, rx) = channel();
    APP.with(|app| app.borrow_mut().sound.commands = Some(tx.clone()));
    std::thread::spawn(move || run(weak, tx, rx));
}

fn post(weak: &slint::Weak<MainWindow>, change: impl FnOnce(&mut State) + Send + 'static) {
    let weak2 = weak.clone();
    let _ = weak.upgrade_in_event_loop(move |_| update(&weak2, |app| change(&mut app.sound)));
}

fn run(weak: slint::Weak<MainWindow>, tx: Sender<Message>, rx: Receiver<Message>) {
    loop {
        let (mut session, watcher) = match (Session::connect(), Session::connect()) {
            (Ok(s), Ok(w)) => (s, w),
            (Err(lituus_sound::Error::NotRunning), _) | (_, Err(lituus_sound::Error::NotRunning)) => {
                post(&weak, |s| {
                    s.not_running = true;
                    s.snapshot = None;
                });
                // Drop what was asked meanwhile; check again in a while.
                std::thread::sleep(Duration::from_secs(3));
                while rx.try_recv().is_ok() {}
                continue;
            }
            (Err(e), _) | (_, Err(e)) => {
                let error = e.to_string();
                post(&weak, move |s| s.error = Some(error));
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        let changes = tx.clone();
        std::thread::spawn(move || watcher.watch(move || changes.send(Message::Changed).is_ok()));
        if serve(&weak, &mut session, &rx).is_err() {
            post(&weak, |s| s.snapshot = None);
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/// Answers commands and changes until the connection breaks.
fn serve(
    weak: &slint::Weak<MainWindow>,
    session: &mut Session,
    rx: &Receiver<Message>,
) -> Result<(), lituus_sound::Error> {
    let mut first = true;
    loop {
        let mut commands: Vec<Command> = Vec::new();
        if !first {
            let Ok(message) = rx.recv() else { return Ok(()) };
            let mut queue = vec![message];
            // Whatever else came meanwhile, after a moment for a burst of events to settle.
            std::thread::sleep(Duration::from_millis(15));
            queue.extend(rx.try_iter());
            for m in queue {
                if let Message::Run(c) = m {
                    commands.retain(|old| !c.replaces(old));
                    commands.push(c);
                }
            }
        }
        first = false;
        for command in commands {
            if let Err(e) = execute(session, &command) {
                if matches!(e, lituus_sound::Error::Io(_)) {
                    return Err(e);
                }
                let error = format!("Couldn't change that: {e}");
                post(weak, move |s| s.error = Some(error));
            }
        }
        let snapshot = session.snapshot()?;
        post(weak, move |s| {
            s.not_running = false;
            s.snapshot = Some(snapshot);
        });
    }
}

fn execute(session: &mut Session, command: &Command) -> Result<(), lituus_sound::Error> {
    match command {
        Command::DefaultSink(name) => session.set_default_sink(name),
        Command::DefaultSource(name) => session.set_default_source(name),
        Command::SinkVolume(i, v) => session.set_sink_volume(*i, v),
        Command::SourceVolume(i, v) => session.set_source_volume(*i, v),
        Command::StreamVolume(i, v) => session.set_stream_volume(*i, v),
        Command::SinkMute(i, m) => session.set_sink_mute(*i, *m),
        Command::SourceMute(i, m) => session.set_source_mute(*i, *m),
        Command::StreamMute(i, m) => session.set_stream_mute(*i, *m),
        Command::Move(stream, sink) => session.move_stream(*stream, *sink),
        Command::SinkPort(i, port) => session.set_sink_port(*i, port),
        Command::Profile(card, profile) => session.set_card_profile(card, profile),
    }
}

fn send(state: &State, command: Command) {
    if let Some(tx) = &state.commands {
        let _ = tx.send(Message::Run(command));
    }
}

/// What a click or a drag on the page does: (verb, id, value).
pub fn act(state: &mut State, verb: &str, id: &str, value: f32) -> Option<(&'static str, &'static str, bool)> {
    let snap = state.snapshot.as_ref()?;
    let percent = (value.max(0.0) * 100.0).round() as u32;
    let command = match verb {
        "default-sink" => Command::DefaultSink(id.to_owned()),
        "default-source" => Command::DefaultSource(id.to_owned()),
        "out-volume" => {
            let d = snap.default_sink()?;
            Command::SinkVolume(d.index, d.volume.with_percent(percent))
        }
        "out-mute" => {
            let d = snap.default_sink()?;
            Command::SinkMute(d.index, !d.muted)
        }
        "balance" => {
            let d = snap.default_sink()?;
            Command::SinkVolume(d.index, d.volume.with_balance(value))
        }
        "out-port" => {
            let d = snap.default_sink()?;
            let port = d.ports.get(id.parse::<usize>().ok()?)?;
            Command::SinkPort(d.index, port.name.clone())
        }
        "in-volume" => {
            let d = snap.default_source()?;
            Command::SourceVolume(d.index, d.volume.with_percent(percent))
        }
        "in-mute" => {
            let d = snap.default_source()?;
            Command::SourceMute(d.index, !d.muted)
        }
        "app-volume" | "app-mute" | "app-output" => {
            let s = snap.streams.iter().find(|s| s.index.to_string() == id)?;
            match verb {
                "app-volume" => Command::StreamVolume(s.index, s.volume.with_percent(percent)),
                "app-mute" => Command::StreamMute(s.index, !s.muted),
                _ => Command::Move(s.index, snap.sinks.get(value as usize)?.index),
            }
        }
        "profile" => {
            let c = snap.cards.iter().find(|c| c.name == id)?;
            Command::Profile(c.name.clone(), c.profiles.get(value as usize)?.name.clone())
        }
        "overamplify" => {
            state.overamplify = !state.overamplify;
            return Some(("sound", "overamplify", state.overamplify));
        }
        _ => return None,
    };
    send(state, command);
    None
}

fn icon(kind: &str) -> &'static str {
    match kind {
        "headphones" | "headset" => "headphones",
        "tv" => "tv",
        "webcam" | "microphone" => "mic",
        _ => "volume",
    }
}

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

/// The sidebar's status: the volume of the output in use.
pub fn status(state: &State) -> String {
    match state.snapshot.as_ref().and_then(|s| s.default_sink()) {
        Some(d) if d.muted => "Muted".to_owned(),
        Some(d) => format!("{}%", d.volume.percent()),
        None if state.not_running => "—".to_owned(),
        None => String::new(),
    }
}

pub fn view(state: &State) -> SoundView {
    let max = if state.overamplify { 1.5 } else { 1.0 };
    let mut v = SoundView { max, overamplify: state.overamplify, ..SoundView::default() };
    let Some(snap) = state.snapshot.as_ref() else {
        if state.not_running {
            v.state = s("not-running");
            v.sub = s("Not running");
            v.empty_title = s("PipeWire isn't running");
            v.empty_text = s(
                "Lituus talks to PipeWire's PulseAudio service (pipewire-pulse). Start it with: systemctl --user enable --now pipewire-pulse",
            );
        } else {
            v.state = s("loading");
            v.empty_title = s("Reading PipeWire…");
        }
        return v;
    };
    v.state = s("list");
    let playing = snap.streams.iter().filter(|s| !s.corked).count();
    // "PulseAudio (on PipeWire 1.6.2) 15.0.0" is PipeWire 1.6.2 to the user.
    let server = match snap.server.split_once("(on ").and_then(|(_, r)| r.split_once(')')) {
        Some((pipewire, _)) => pipewire.to_owned(),
        None => snap.server.clone(),
    };
    v.sub = s(match playing {
        0 => server,
        1 => format!("{server} · 1 app playing"),
        n => format!("{server} · {n} apps playing"),
    });
    let default_sink = snap.default_sink().map(|d| d.name.clone());
    let default_source = snap.default_source().map(|d| d.name.clone());
    let port_name = |d: &lituus_sound::Device| {
        d.active_port.as_deref().and_then(|p| d.ports.iter().find(|x| x.name == p)).map(|p| p.description.clone())
    };
    if let Some(d) = snap.default_sink() {
        v.hero = HeroInfo {
            icon: s(icon(d.kind())),
            name: s(&d.description),
            meta: s(port_name(d).unwrap_or_else(|| bus(&d.bus).to_owned())),
            tone: s("muted"),
            detail: s(""),
        };
        v.out_volume = d.volume.percent() as f32 / 100.0;
        v.out_muted = d.muted;
        v.out_ports = model(d.ports.iter().map(|p| s(&p.description)).collect());
        v.out_port =
            d.active_port.as_deref().and_then(|a| d.ports.iter().position(|p| p.name == a)).unwrap_or(0) as i32;
        v.has_balance = d.volume.has_balance();
        v.out_balance = d.volume.balance();
    } else {
        v.hero = HeroInfo {
            icon: s("volume-off"),
            name: s("No output"),
            meta: s("Nothing to play sound through"),
            ..HeroInfo::default()
        };
    }
    let device = |d: &lituus_sound::Device, in_use: bool| {
        let sub = match (port_name(d), bus(&d.bus)) {
            (Some(p), "") => p,
            (Some(p), b) => format!("{b} · {p}"),
            (None, b) => b.to_owned(),
        };
        SoundDevice { id: s(&d.name), name: s(&d.description), sub: s(sub), icon: s(icon(d.kind())), in_use }
    };
    v.outputs = model(snap.sinks.iter().map(|d| device(d, Some(&d.name) == default_sink.as_ref())).collect());
    v.outputs_names = model(snap.sinks.iter().map(|d| s(&d.description)).collect());
    v.inputs = model(snap.sources.iter().map(|d| device(d, Some(&d.name) == default_source.as_ref())).collect());
    if let Some(d) = snap.default_source() {
        v.in_volume = d.volume.percent() as f32 / 100.0;
        v.in_muted = d.muted;
    }
    v.apps = model(
        snap.streams
            .iter()
            .map(|st| SoundApp {
                id: s(st.index.to_string()),
                name: s(&st.app),
                sub: s(if st.corked { format!("Paused · {}", st.title) } else { st.title.clone() }),
                volume: st.volume.percent() as f32 / 100.0,
                muted: st.muted,
                output: snap.sinks.iter().position(|d| d.index == st.sink).unwrap_or(0) as i32,
            })
            .collect(),
    );
    v.cards =
        model(
            snap.cards
                .iter()
                .map(|c| SoundCard {
                    id: s(&c.name),
                    name: s(&c.description),
                    profiles: model(
                        c.profiles
                            .iter()
                            .map(|p| {
                                if p.available {
                                    s(&p.description)
                                } else {
                                    s(format!("{} (unplugged)", p.description))
                                }
                            })
                            .collect(),
                    ),
                    selected: c.active.as_deref().and_then(|a| c.profiles.iter().position(|p| p.name == a)).unwrap_or(0)
                        as i32,
                })
                .collect(),
        );
    v
}

fn bus(bus: &str) -> &'static str {
    match bus {
        "bluetooth" => "Bluetooth",
        "usb" => "USB",
        "pci" => "Built in",
        _ => "",
    }
}

pub fn render(window: &MainWindow, state: &State) {
    window.set_sound(view(state));
}

#[cfg(test)]
mod tests {
    use super::*;
    use lituus_sound::{Device, NORM, Port, Stream};
    use slint::Model;

    fn stereo(percent: u32) -> Volume {
        Volume { channels: vec![NORM * percent / 100; 2], sides: vec![-1, 1] }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            server: "PulseAudio (on PipeWire 1.6.2) 15.0.0".into(),
            default_sink: Some("speakers".into()),
            default_source: Some("yeti".into()),
            sinks: vec![
                Device {
                    index: 40,
                    name: "speakers".into(),
                    description: "Speakers".into(),
                    bus: "pci".into(),
                    volume: stereo(45),
                    ports: vec![
                        Port { name: "analog-output-speaker".into(), description: "Speakers".into(), available: None },
                        Port {
                            name: "analog-output-headphones".into(),
                            description: "Headphones".into(),
                            available: Some(false),
                        },
                    ],
                    active_port: Some("analog-output-speaker".into()),
                    ..Device::default()
                },
                Device {
                    index: 41,
                    name: "bluez_output.AC_80".into(),
                    description: "Sony WH-1000XM5".into(),
                    bus: "bluetooth".into(),
                    form_factor: "headphone".into(),
                    volume: stereo(60),
                    ..Device::default()
                },
            ],
            sources: vec![Device {
                index: 50,
                name: "yeti".into(),
                description: "Blue Yeti".into(),
                bus: "usb".into(),
                volume: stereo(70),
                ..Device::default()
            }],
            streams: vec![Stream {
                index: 7,
                app: "Firefox".into(),
                title: "YouTube".into(),
                sink: 41,
                volume: stereo(80),
                ..Stream::default()
            }],
            cards: Vec::new(),
        }
    }

    #[test]
    fn the_page_shows_the_output_in_use() {
        let state = State { snapshot: Some(snapshot()), ..State::default() };
        let v = view(&state);
        assert_eq!(v.state, "list");
        assert_eq!(v.hero.name, "Speakers");
        assert_eq!(v.hero.meta, "Speakers");
        assert!((v.out_volume - 0.45).abs() < 0.001);
        assert_eq!(v.out_ports.row_count(), 2);
        assert!(v.has_balance);
        let outputs: Vec<(String, bool)> = v.outputs.iter().map(|d| (d.name.to_string(), d.in_use)).collect();
        assert_eq!(outputs, [("Speakers".into(), true), ("Sony WH-1000XM5".into(), false)]);
        assert_eq!(v.outputs.row_data(1).unwrap().icon, "headphones");
        let app = v.apps.row_data(0).unwrap();
        assert_eq!((app.name.as_str(), app.output), ("Firefox", 1));
        assert_eq!(v.sub, "PipeWire 1.6.2 · 1 app playing");
        assert_eq!(status(&state), "45%");
    }

    #[test]
    fn clicks_become_commands() {
        let (tx, rx) = channel();
        let mut state = State { snapshot: Some(snapshot()), commands: Some(tx), ..State::default() };
        let mut sent = |verb: &str, id: &str, value: f32| {
            act(&mut state, verb, id, value);
            match rx.try_recv() {
                Ok(Message::Run(c)) => Some(c),
                _ => None,
            }
        };
        assert_eq!(sent("out-volume", "", 0.3), Some(Command::SinkVolume(40, stereo(30))));
        assert_eq!(sent("out-mute", "", 0.0), Some(Command::SinkMute(40, true)));
        assert_eq!(sent("out-port", "1", 0.0), Some(Command::SinkPort(40, "analog-output-headphones".into())));
        assert_eq!(sent("app-output", "7", 0.0), Some(Command::Move(7, 40)));
        assert_eq!(
            sent("default-sink", "bluez_output.AC_80", 0.0),
            Some(Command::DefaultSink("bluez_output.AC_80".into()))
        );
        assert_eq!(sent("in-volume", "", 0.5), Some(Command::SourceVolume(50, stereo(50))));
        assert_eq!(sent("app-volume", "99", 0.5), None, "no such app");
    }

    #[test]
    fn a_dragged_slider_keeps_only_its_last_volume() {
        let a = Command::SinkVolume(1, stereo(10));
        assert!(Command::SinkVolume(1, stereo(20)).replaces(&a));
        assert!(!Command::SinkVolume(2, stereo(20)).replaces(&a));
        assert!(!Command::SinkMute(1, true).replaces(&a));
    }

    #[test]
    fn no_pipewire_is_said_plainly() {
        let v = view(&State { not_running: true, ..State::default() });
        assert_eq!(v.state, "not-running");
        assert!(v.empty_text.contains("pipewire-pulse"));
    }
}
