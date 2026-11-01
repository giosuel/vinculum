//! Against a real PipeWire: set LITUUS_SOUND_TEST=1 with PULSE_SERVER (or XDG_RUNTIME_DIR)
//! pointing at a pipewire-pulse that has at least one output, such as the null sinks made by
//! `pw-cli create-node adapter '{ factory.name=support.null-audio-sink media.class=Audio/Sink … }'`.
//! Skipped otherwise.

use lituus_sound::Session;

#[test]
fn reads_and_changes_a_real_server() {
    if std::env::var_os("LITUUS_SOUND_TEST").is_none() {
        return;
    }
    let mut session = Session::connect().expect("pipewire-pulse");
    let snap = session.snapshot().unwrap();
    assert!(snap.server.contains("PipeWire") || snap.server.contains("pulse"), "{}", snap.server);
    let sink = snap.default_sink().expect("an output").clone();
    let before = sink.volume.clone();
    session.set_sink_volume(sink.index, &before.with_percent(37)).unwrap();
    session.set_sink_mute(sink.index, true).unwrap();
    let after = session.snapshot().unwrap();
    let changed = after.sink(&sink.name).unwrap();
    assert_eq!(changed.volume.percent(), 37);
    assert!(changed.muted);
    session.set_sink_volume(sink.index, &before).unwrap();
    session.set_sink_mute(sink.index, sink.muted).unwrap();
    if let Some(other) = snap.sinks.iter().find(|s| s.name != sink.name) {
        session.set_default_sink(&other.name).unwrap();
        // The session manager (WirePlumber) makes the change, a moment later.
        let mut default = None;
        for _ in 0..40 {
            default = session.snapshot().unwrap().default_sink;
            if default.as_deref() == Some(other.name.as_str()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(default.as_deref(), Some(other.name.as_str()));
        session.set_default_sink(&sink.name).unwrap();
    }
}

#[test]
fn a_watcher_hears_changes() {
    if std::env::var_os("LITUUS_SOUND_TEST").is_none() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let watcher = Session::connect().unwrap();
    std::thread::spawn(move || watcher.watch(move || tx.send(()).is_ok()));
    // Give it time to subscribe.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let mut session = Session::connect().unwrap();
    let sink = session.snapshot().unwrap().default_sink().unwrap().clone();
    session.set_sink_mute(sink.index, !sink.muted).unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(3)).expect("an event");
    session.set_sink_mute(sink.index, sink.muted).unwrap();
}
