//! Hyprland's sockets in `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/`: `.socket.sock`
//! answers one request per connection (what `hyprctl` sends), `.socket2.sock` streams events.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::model::{Monitor, Version, Workspace};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Not inside a Hyprland session.
    #[error("Hyprland isn't running")]
    NotRunning,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("Hyprland's answer didn't make sense: {0}")]
    Json(#[from] serde_json::Error),
    /// Hyprland said no: the text it answered.
    #[error("{0}")]
    Refused(String),
}

/// A running Hyprland.
#[derive(Clone, Debug)]
pub struct Hyprland {
    dir: PathBuf,
}

impl Hyprland {
    /// The Hyprland this session runs in.
    pub fn from_env() -> Result<Self, Error> {
        let signature =
            std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").filter(|s| !s.is_empty()).ok_or(Error::NotRunning)?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or(Error::NotRunning)?;
        let dir = runtime.join("hypr").join(signature);
        if !dir.join(".socket.sock").exists() {
            return Err(Error::NotRunning);
        }
        Ok(Self { dir })
    }

    /// The Hyprland whose sockets are in `dir` (for tests).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// One request, as `hyprctl` sends it ("j/monitors all"), and the whole answer.
    pub fn request(&self, request: &str) -> Result<String, Error> {
        let mut socket = UnixStream::connect(self.dir.join(".socket.sock")).map_err(|_| Error::NotRunning)?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        socket.write_all(request.as_bytes())?;
        let mut answer = String::new();
        socket.read_to_string(&mut answer)?;
        Ok(answer)
    }

    fn json<T: serde::de::DeserializeOwned>(&self, request: &str) -> Result<T, Error> {
        Ok(serde_json::from_str(&self.request(&format!("j/{request}"))?)?)
    }

    /// All monitors, disabled ones too.
    pub fn monitors(&self) -> Result<Vec<Monitor>, Error> {
        self.json("monitors all")
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>, Error> {
        self.json("workspaces")
    }

    pub fn version(&self) -> Result<Version, Error> {
        self.json("version")
    }

    /// Runs Lua in Hyprland's config (0.55 and later): `hl.monitor({ … })` applies at once.
    pub fn eval(&self, lua: &str) -> Result<(), Error> {
        ok(self.request(&format!("eval {lua}"))?)
    }

    /// Sets a hyprlang keyword live (before 0.55): `keyword monitor DP-1, preferred, auto, 1`.
    pub fn keyword(&self, keyword: &str, value: &str) -> Result<(), Error> {
        ok(self.request(&format!("keyword {keyword} {value}"))?)
    }

    /// Shows a note in the corner, as `hyprctl notify` does: icon 1 info, 5 ok; colour 0 is the
    /// default.
    pub fn notify(&self, icon: i32, millis: u32, text: &str) -> Result<(), Error> {
        ok(self.request(&format!("notify {icon} {millis} 0 {text}"))?)
    }

    /// Calls `event(name, data)` for each event ("monitoradded", "DP-1") until it returns false
    /// or Hyprland goes away.
    pub fn events(&self, mut event: impl FnMut(&str, &str) -> bool) -> Result<(), Error> {
        let socket = UnixStream::connect(self.dir.join(".socket2.sock")).map_err(|_| Error::NotRunning)?;
        for line in BufReader::new(socket).lines() {
            let line = line?;
            let (name, data) = line.split_once(">>").unwrap_or((line.as_str(), ""));
            if !event(name, data) {
                break;
            }
        }
        Ok(())
    }
}

fn ok(answer: String) -> Result<(), Error> {
    if answer.trim() == "ok" { Ok(()) } else { Err(Error::Refused(answer.trim().to_owned())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A stand-in Hyprland: answers each request with `answer(request)`, and sends `events`.
    fn fake(answer: fn(&str) -> String, events: &'static str) -> (Hyprland, std::thread::JoinHandle<Vec<String>>) {
        let dir =
            std::env::temp_dir().join(format!("lituus-hypr-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let requests = UnixListener::bind(dir.join(".socket.sock")).unwrap();
        let events_socket = UnixListener::bind(dir.join(".socket2.sock")).unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = events_socket.accept() {
                let _ = s.write_all(events.as_bytes());
            }
        });
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..3 {
                let (mut s, _) = requests.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                s.write_all(answer(&request).as_bytes()).unwrap();
                seen.push(request);
            }
            seen
        });
        (Hyprland::at(dir), handle)
    }

    #[test]
    fn requests_and_events() {
        let (h, server) = fake(
            |r| match r {
                "j/monitors all" => include_str!("../tests/fixtures/monitors.json").to_owned(),
                r if r.starts_with("eval hl.monitor") => "ok".to_owned(),
                _ => "unknown request".to_owned(),
            },
            "workspace>>2\nmonitoradded>>DP-1\nmonitorremoved>>HDMI-A-3\n",
        );
        assert_eq!(h.monitors().unwrap().len(), 4);
        h.eval(r#"hl.monitor({ output = "DP-1", disabled = true })"#).unwrap();
        assert!(matches!(h.keyword("monitor", "x"), Err(Error::Refused(t)) if t == "unknown request"));
        let mut seen = Vec::new();
        h.events(|name, data| {
            seen.push(format!("{name}:{data}"));
            seen.len() < 3
        })
        .unwrap();
        assert_eq!(seen, ["workspace:2", "monitoradded:DP-1", "monitorremoved:HDMI-A-3"]);
        assert_eq!(server.join().unwrap()[1], r#"eval hl.monitor({ output = "DP-1", disabled = true })"#);
    }
}
