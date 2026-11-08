//! Wallpapers through the program that draws them: awww (swww renamed, 0.12 and later) or
//! hyprpaper (0.8 and later). Which of them is installed and running, the picture each monitor
//! shows, setting one, and hyprpaper.conf, which hyprpaper reads at start (awww's daemon keeps the
//! last pictures itself). Also the pictures in a folder, and their thumbnails.
//!
//! Both are driven by their own command line (`awww img`, `hyprctl hyprpaper wallpaper`), found
//! on the PATH; `Env` says where to look, so tests can put stand-ins there.

mod hyprpaper;
mod pictures;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub use hyprpaper::{
    Entry as HyprpaperEntry, conf_text as hyprpaper_conf_text, entries as hyprpaper_entries,
    fit_mode as hyprpaper_fit_mode,
};
pub use pictures::{Folder, Picture, Subfolder, dimensions, is_picture, read_folder, thumbnail};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} isn't installed")]
    NotInstalled(&'static str),
    #[error("{0} isn't running")]
    NotRunning(&'static str),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The program said no: what it printed.
    #[error("{0}")]
    Refused(String),
    #[error("{0}'s answer didn't make sense: {1}")]
    Answer(&'static str, String),
}

/// A program that draws wallpapers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Program {
    Awww,
    Hyprpaper,
}

impl Program {
    pub const ALL: [Program; 2] = [Program::Awww, Program::Hyprpaper];

    pub fn name(self) -> &'static str {
        match self {
            Program::Awww => "awww",
            Program::Hyprpaper => "hyprpaper",
        }
    }

    /// The command that starts it, as an autostart line runs it.
    pub fn daemon(self) -> &'static str {
        match self {
            Program::Awww => "awww-daemon",
            Program::Hyprpaper => "hyprpaper",
        }
    }

    pub fn from_name(name: &str) -> Option<Program> {
        Program::ALL.into_iter().find(|p| p.name() == name)
    }

    /// The fills it can show a picture with.
    pub fn fills(self) -> &'static [Fill] {
        match self {
            Program::Awww => &[Fill::Cover, Fill::Contain, Fill::Stretch, Fill::Center],
            Program::Hyprpaper => &[Fill::Cover, Fill::Contain, Fill::Stretch, Fill::Tile],
        }
    }
}

/// How a picture covers its monitor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fill {
    /// Fills the screen, cropping what doesn't fit.
    #[default]
    Cover,
    /// Fits inside, with bars.
    Contain,
    Stretch,
    /// As it is, in the middle (awww only).
    Center,
    /// Repeated (hyprpaper only).
    Tile,
}

impl Fill {
    pub const ALL: [Fill; 5] = [Fill::Cover, Fill::Contain, Fill::Stretch, Fill::Center, Fill::Tile];

    pub fn id(self) -> &'static str {
        match self {
            Fill::Cover => "cover",
            Fill::Contain => "contain",
            Fill::Stretch => "stretch",
            Fill::Center => "center",
            Fill::Tile => "tile",
        }
    }

    pub fn from_id(id: &str) -> Fill {
        Fill::ALL.into_iter().find(|f| f.id() == id).unwrap_or_default()
    }

    /// awww's `--resize`.
    fn awww(self) -> &'static str {
        match self {
            Fill::Cover | Fill::Tile => "crop",
            Fill::Contain => "fit",
            Fill::Stretch => "stretch",
            Fill::Center => "no",
        }
    }

    /// hyprpaper's fit mode, as `hyprctl hyprpaper` takes it (its config calls stretch "fill").
    fn hyprctl(self) -> &'static str {
        match self {
            Fill::Cover | Fill::Center => "cover",
            Fill::Contain => "contain",
            Fill::Stretch => "stretch",
            Fill::Tile => "tile",
        }
    }
}

/// awww's transitions, as `--transition-type` takes them.
pub const TRANSITIONS: [&str; 13] =
    ["none", "simple", "fade", "left", "right", "top", "bottom", "wipe", "wave", "grow", "center", "outer", "random"];

/// Setting a picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub path: PathBuf,
    /// The monitors; none: all of them.
    pub monitors: Vec<String>,
    pub fill: Fill,
    /// awww only: the transition, its length in seconds, and frames per second (the monitor's
    /// refresh rate; awww's own default is 30).
    pub transition: String,
    pub duration: f32,
    pub fps: u32,
    /// awww only: the colour of the bars of a picture that fits inside, `rrggbb`.
    pub bar_colour: String,
}

/// Where the programs are looked for and how Hyprland's session is found.
#[derive(Clone, Debug)]
pub struct Env {
    pub path: Vec<PathBuf>,
    pub runtime_dir: Option<PathBuf>,
    pub signature: Option<String>,
}

impl Env {
    /// This session's.
    pub fn from_env() -> Env {
        Env {
            path: std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            signature: std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().filter(|s| !s.is_empty()),
        }
    }

    /// The command's file, if it's on the PATH.
    pub fn find(&self, command: &str) -> Option<PathBuf> {
        self.path.iter().map(|d| d.join(command)).find(|p| p.is_file())
    }

    fn command(&self, command: &str) -> Command {
        let mut c = Command::new(self.find(command).unwrap_or_else(|| PathBuf::from(command)));
        c.env("PATH", std::env::join_paths(&self.path).unwrap_or_default());
        if let Some(dir) = &self.runtime_dir {
            c.env("XDG_RUNTIME_DIR", dir);
        }
        if let Some(s) = &self.signature {
            c.env("HYPRLAND_INSTANCE_SIGNATURE", s);
        }
        c.stdin(Stdio::null());
        c
    }

    /// Runs a command and gives what it printed, or why it failed.
    fn run(&self, command: &str, args: &[&str]) -> Result<String, Error> {
        let out = self.command(command).args(args).output()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            Ok(text)
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
            Err(Error::Refused(if err.is_empty() { text.trim().to_owned() } else { err }))
        }
    }

    /// hyprpaper's socket in Hyprland's runtime folder.
    fn hyprpaper_socket(&self) -> Option<PathBuf> {
        Some(self.runtime_dir.as_ref()?.join("hypr").join(self.signature.as_ref()?).join(".hyprpaper.sock"))
    }
}

/// Which programs are installed and which runs, with its version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub installed: Vec<Program>,
    pub running: Option<Program>,
    /// The running one's version ("0.12.1"), if it says.
    pub version: String,
}

/// What's installed and what runs now.
pub fn status(env: &Env) -> Status {
    let installed: Vec<Program> = Program::ALL.into_iter().filter(|p| env.find(p.name()).is_some()).collect();
    let running = installed.iter().copied().find(|p| is_running(env, *p));
    let version = running.and_then(|p| version(env, p)).unwrap_or_default();
    Status { installed, running, version }
}

fn is_running(env: &Env, program: Program) -> bool {
    match program {
        // `awww query` asks the daemon; it fails when there's none.
        Program::Awww => env.run("awww", &["query"]).is_ok(),
        Program::Hyprpaper => env.hyprpaper_socket().is_some_and(|s| s.exists()),
    }
}

/// "awww 0.12.1" or "hyprpaper v0.8.5": the last word, without its "v".
fn version(env: &Env, program: Program) -> Option<String> {
    let text = env.run(program.name(), &["--version"]).ok()?;
    let word = text.split_whitespace().last()?;
    Some(word.trim_start_matches('v').to_owned())
}

/// The picture each monitor shows now, as (monitor, file); a monitor showing a plain colour has
/// none.
pub fn current(env: &Env, program: Program) -> Result<Vec<(String, PathBuf)>, Error> {
    match program {
        Program::Awww => {
            let text = env.run("awww", &["query", "--json"])?;
            parse_awww_query(&text).ok_or(Error::Answer("awww", text))
        }
        Program::Hyprpaper => {
            let text = env.run("hyprctl", &["hyprpaper", "listactive"])?;
            Ok(text
                .lines()
                .filter_map(|l| l.split_once(": "))
                .map(|(m, p)| (m.trim().to_owned(), PathBuf::from(p.trim())))
                .collect())
        }
    }
}

/// `awww query --json`: `{"": [{"name": "DP-1", …, "displaying": {"image": "/a.png"}}]}`, one list
/// per daemon namespace.
fn parse_awww_query(text: &str) -> Option<Vec<(String, PathBuf)>> {
    #[derive(serde::Deserialize)]
    struct Output {
        name: String,
        displaying: Displaying,
    }
    #[derive(serde::Deserialize)]
    struct Displaying {
        image: Option<String>,
    }
    let namespaces: std::collections::BTreeMap<String, Vec<Output>> = serde_json::from_str(text).ok()?;
    Some(
        namespaces.into_values().flatten().filter_map(|o| Some((o.name, PathBuf::from(o.displaying.image?)))).collect(),
    )
}

/// The arguments of `awww img` for a picture.
fn awww_args(set: &Set) -> Vec<String> {
    let mut args = vec!["img".to_owned(), set.path.display().to_string()];
    if !set.monitors.is_empty() {
        args.extend(["--outputs".to_owned(), set.monitors.join(",")]);
    }
    args.extend(["--resize".to_owned(), set.fill.awww().to_owned()]);
    if matches!(set.fill, Fill::Contain | Fill::Center) && !set.bar_colour.is_empty() {
        args.extend(["--fill-color".to_owned(), set.bar_colour.trim_start_matches('#').to_owned()]);
    }
    if !set.transition.is_empty() {
        args.extend(["--transition-type".to_owned(), set.transition.clone()]);
        args.extend(["--transition-duration".to_owned(), format!("{}", set.duration)]);
        if set.fps > 0 {
            args.extend(["--transition-fps".to_owned(), set.fps.to_string()]);
        }
    }
    args
}

/// Shows a picture: on the monitors given (hyprpaper takes one at a time), at once.
pub fn set(env: &Env, program: Program, set: &Set, monitors: &[String]) -> Result<(), Error> {
    match program {
        Program::Awww => {
            let args = awww_args(set);
            env.run("awww", &args.iter().map(String::as_str).collect::<Vec<_>>()).map(|_| ())
        }
        Program::Hyprpaper => {
            // hyprpaper's empty monitor is its fallback; Lituus names each monitor instead.
            let targets = if set.monitors.is_empty() { monitors.to_vec() } else { set.monitors.clone() };
            for m in targets {
                let arg = format!("{m}, {}, {}", set.path.display(), set.fill.hyprctl());
                let out = env.run("hyprctl", &["hyprpaper", "wallpaper", &arg])?;
                if out.contains("failed") || out.contains("error") {
                    return Err(Error::Refused(out.trim().to_owned()));
                }
            }
            Ok(())
        }
    }
}

/// Starts the program, in the background, detached from Lituus.
pub fn start(env: &Env, program: Program) -> Result<(), Error> {
    if env.find(program.daemon()).is_none() {
        return Err(Error::NotInstalled(program.name()));
    }
    env.command(program.daemon()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    Ok(())
}

/// Stops the program (to start the other).
pub fn stop(env: &Env, program: Program) -> Result<(), Error> {
    match program {
        Program::Awww => env.run("awww", &["kill"]).map(|_| ()),
        // hyprpaper has no command to stop it; its process goes by name, as Waypaper does it.
        Program::Hyprpaper => env.run("pkill", &["-x", "hyprpaper"]).map(|_| ()),
    }
}

/// The line that starts it with Hyprland, in Lua or in hyprlang.
pub fn autostart_line(program: Program, lua: bool) -> String {
    if lua {
        format!("hl.on(\"hyprland.start\", function() hl.exec_cmd(\"{}\") end)", program.daemon())
    } else {
        format!("exec-once = {}", program.daemon())
    }
}

/// hyprpaper.conf, in Hyprland's config folder.
pub fn hyprpaper_conf(config_dir: &Path) -> PathBuf {
    config_dir.join("hyprpaper.conf")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A folder with stand-ins for the programs: each prints `answer` and records its arguments.
    fn stand_ins(name: &str, scripts: &[(&str, &str)]) -> (Env, PathBuf) {
        let d = std::env::temp_dir().join(format!("lituus-wallpaper-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bin")).unwrap();
        for (command, body) in scripts {
            let p = d.join("bin").join(command);
            std::fs::write(&p, format!("#!/bin/sh\necho \"$0 $*\" >> {}/calls\n{body}\n", d.display())).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let env = Env { path: vec![d.join("bin")], runtime_dir: Some(d.join("run")), signature: Some("sig".into()) };
        (env, d)
    }

    fn calls(d: &Path) -> Vec<String> {
        std::fs::read_to_string(d.join("calls"))
            .unwrap_or_default()
            .lines()
            .map(|l| l.strip_prefix(&format!("{}/", d.join("bin").display())).unwrap_or(l).to_owned())
            .collect()
    }

    #[test]
    fn awww_running() {
        let query = r##"{
"": [
    {"name": "DP-3", "width": 2560, "height": 1440, "scale": 1, "displaying": {"image": "/w/alps.jpg"}},
    {"name": "HDMI-A-3", "width": 3840, "height": 2160, "scale": 1, "displaying": {"color": "#000000"}}
]
}"##;
        let (env, d) = stand_ins(
            "awww",
            &[
                ("awww", &format!("case \"$1\" in --version) echo 'awww 0.12.1';; query) echo '{query}';; esac")),
                ("awww-daemon", ""),
            ],
        );
        let s = status(&env);
        assert_eq!(
            (s.installed.as_slice(), s.running, s.version.as_str()),
            (&[Program::Awww][..], Some(Program::Awww), "0.12.1")
        );
        assert_eq!(current(&env, Program::Awww).unwrap(), [("DP-3".to_owned(), PathBuf::from("/w/alps.jpg"))]);
        let pic = Set {
            path: "/w/fjord.jpg".into(),
            monitors: vec!["DP-5".into()],
            fill: Fill::Contain,
            transition: "fade".into(),
            duration: 1.5,
            fps: 240,
            bar_colour: "#102030".into(),
        };
        set(&env, Program::Awww, &pic, &[]).unwrap();
        assert!(calls(&d).contains(
            &"awww img /w/fjord.jpg --outputs DP-5 --resize fit --fill-color 102030 --transition-type fade --transition-duration 1.5 --transition-fps 240".to_owned()
        ), "{:?}", calls(&d));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn hyprpaper_running() {
        let (env, d) = stand_ins(
            "hyprpaper",
            &[
                ("hyprpaper", "echo 'hyprpaper v0.8.5'"),
                ("hyprctl", "case \"$2\" in listactive) echo 'DP-3: /w/alps.jpg'; echo 'DP-5: /w/fjord.jpg';; esac"),
            ],
        );
        std::fs::create_dir_all(d.join("run/hypr/sig")).unwrap();
        assert_eq!(status(&env).running, None);
        std::fs::write(d.join("run/hypr/sig/.hyprpaper.sock"), "").unwrap();
        let s = status(&env);
        assert_eq!((s.running, s.version.as_str()), (Some(Program::Hyprpaper), "0.8.5"));
        assert_eq!(current(&env, Program::Hyprpaper).unwrap()[1], ("DP-5".to_owned(), PathBuf::from("/w/fjord.jpg")));
        let pic = Set {
            path: "/w/a.jpg".into(),
            monitors: vec![],
            fill: Fill::Stretch,
            transition: String::new(),
            duration: 0.0,
            fps: 0,
            bar_colour: String::new(),
        };
        set(&env, Program::Hyprpaper, &pic, &["DP-3".into(), "DP-5".into()]).unwrap();
        let c = calls(&d);
        assert!(c.contains(&"hyprctl hyprpaper wallpaper DP-3, /w/a.jpg, stretch".to_owned()), "{c:?}");
        assert!(c.contains(&"hyprctl hyprpaper wallpaper DP-5, /w/a.jpg, stretch".to_owned()), "{c:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn nothing_installed() {
        let (env, d) = stand_ins("none", &[]);
        assert_eq!(status(&env), Status::default());
        assert!(matches!(start(&env, Program::Awww), Err(Error::NotInstalled("awww"))));
        assert_eq!(
            autostart_line(Program::Awww, true),
            "hl.on(\"hyprland.start\", function() hl.exec_cmd(\"awww-daemon\") end)"
        );
        assert_eq!(autostart_line(Program::Hyprpaper, false), "exec-once = hyprpaper");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
