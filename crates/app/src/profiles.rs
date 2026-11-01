//! `~/.config/lituus/displays.toml`: the workspaces bound to monitors, and display profiles,
//! layouts saved under a name that switch on by themselves when exactly their monitors are
//! plugged in. lituus.lua is made from what's in use; this file is what Lituus remembers.

use std::path::{Path, PathBuf};

use lituus_hypr::{Mode, MonitorRule, WorkspaceRule};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Store {
    /// Switch to the profile that matches the monitors plugged in.
    pub switch_on_hotplug: bool,
    /// The workspaces bound to monitors now.
    pub workspaces: Vec<StoredWorkspace>,
    #[serde(rename = "profile")]
    pub profiles: Vec<Profile>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    /// Switches on by itself when exactly its monitors are plugged in.
    pub auto: bool,
    /// A shell command to run after switching to it.
    pub exec: String,
    #[serde(rename = "monitor")]
    pub monitors: Vec<StoredMonitor>,
    #[serde(rename = "workspace")]
    pub workspaces: Vec<StoredWorkspace>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct StoredMonitor {
    pub output: String,
    pub disabled: bool,
    pub mode: String,
    pub position: [i32; 2],
    pub scale: f64,
    pub transform: u8,
    pub mirror: String,
    pub ten_bit: bool,
    pub cm: String,
    pub sdr_brightness: f64,
    pub sdr_saturation: f64,
    /// -1 for the global setting.
    pub vrr: i8,
    pub reserved: [i32; 4],
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct StoredWorkspace {
    pub workspace: String,
    pub monitor: String,
    pub default: bool,
    pub persistent: bool,
}

impl From<&MonitorRule> for StoredMonitor {
    fn from(r: &MonitorRule) -> Self {
        Self {
            output: r.output.clone(),
            disabled: r.disabled,
            mode: r.mode.rule(),
            position: [r.x, r.y],
            scale: r.scale,
            transform: r.transform,
            mirror: r.mirror.clone().unwrap_or_default(),
            ten_bit: r.ten_bit,
            cm: r.cm.clone(),
            sdr_brightness: r.sdr_brightness,
            sdr_saturation: r.sdr_saturation,
            vrr: r.vrr.map_or(-1, |v| v as i8),
            reserved: r.reserved,
        }
    }
}

impl StoredMonitor {
    pub fn rule(&self) -> MonitorRule {
        MonitorRule {
            output: self.output.clone(),
            disabled: self.disabled,
            mode: Mode::parse(&self.mode).unwrap_or_default(),
            x: self.position[0],
            y: self.position[1],
            scale: if self.scale > 0.0 { self.scale } else { 1.0 },
            transform: self.transform,
            mirror: (!self.mirror.is_empty()).then(|| self.mirror.clone()),
            ten_bit: self.ten_bit,
            cm: if self.cm.is_empty() { "srgb".into() } else { self.cm.clone() },
            sdr_brightness: if self.sdr_brightness > 0.0 { self.sdr_brightness } else { 1.0 },
            sdr_saturation: if self.sdr_saturation > 0.0 { self.sdr_saturation } else { 1.0 },
            vrr: (self.vrr >= 0).then_some(self.vrr as u8),
            reserved: self.reserved,
        }
    }
}

impl From<&WorkspaceRule> for StoredWorkspace {
    fn from(w: &WorkspaceRule) -> Self {
        Self {
            workspace: w.workspace.clone(),
            monitor: w.monitor.clone(),
            default: w.default,
            persistent: w.persistent,
        }
    }
}

impl StoredWorkspace {
    pub fn rule(&self) -> WorkspaceRule {
        WorkspaceRule {
            workspace: self.workspace.clone(),
            monitor: self.monitor.clone(),
            default: self.default,
            persistent: self.persistent,
        }
    }
}

impl Profile {
    /// Its monitors, as a sorted list of selectors, to compare with what's plugged in.
    pub fn outputs(&self) -> Vec<String> {
        let mut o: Vec<String> = self.monitors.iter().map(|m| m.output.clone()).collect();
        o.sort();
        o
    }

    /// "4 monitors · switches by itself".
    pub fn summary(&self) -> String {
        let n = match self.monitors.iter().filter(|m| !m.disabled).count() {
            1 => "1 monitor".to_owned(),
            n => format!("{n} monitors"),
        };
        if self.auto { format!("{n} · switches by itself") } else { format!("{n} · by hand") }
    }
}

impl Store {
    /// The auto profile for exactly these monitors (selectors), if any.
    pub fn matching(&self, connected: &[String]) -> Option<&Profile> {
        let mut connected = connected.to_vec();
        connected.sort();
        self.profiles.iter().find(|p| p.auto && p.outputs() == connected)
    }

    /// The profile that is what the monitors show now, if any.
    pub fn active(&self, rules: &[MonitorRule]) -> Option<usize> {
        self.profiles.iter().position(|p| {
            let mut a: Vec<StoredMonitor> = p.monitors.clone();
            let mut b: Vec<StoredMonitor> = rules.iter().map(StoredMonitor::from).collect();
            a.sort_by(|x, y| x.output.cmp(&y.output));
            b.sort_by(|x, y| x.output.cmp(&y.output));
            a == b
        })
    }
}

pub fn path() -> PathBuf {
    crate::config::dir().join("displays.toml")
}

pub fn load(path: &Path) -> Store {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
            eprintln!("lituus: ignoring {}: {e}", path.display());
            Store::default()
        }),
        Err(_) => Store { switch_on_hotplug: true, ..Store::default() },
    }
}

pub fn save(path: &Path, store: &Store) -> std::io::Result<()> {
    let text = format!(
        "# Written by Lituus (Displays): display profiles and workspaces bound to monitors.\n\n{}",
        toml::to_string_pretty(store).map_err(std::io::Error::other)?
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lituus_hypr::Monitor;

    fn rules() -> Vec<MonitorRule> {
        let all: Vec<Monitor> = serde_json::from_str(include_str!("../../hypr/tests/fixtures/monitors.json")).unwrap();
        all.iter().map(|m| MonitorRule::from_monitor(m, &all)).collect()
    }

    #[test]
    fn a_profile_round_trips_through_the_file() {
        let mut r = rules();
        r[1].vrr = Some(2);
        r[3].mirror = Some(r[1].output.clone());
        let profile = Profile {
            name: "Desk + TV".into(),
            auto: true,
            exec: "pkill -SIGUSR2 waybar".into(),
            monitors: r.iter().map(StoredMonitor::from).collect(),
            workspaces: vec![StoredWorkspace {
                workspace: "1".into(),
                monitor: r[1].output.clone(),
                default: true,
                persistent: false,
            }],
        };
        let store = Store { switch_on_hotplug: true, workspaces: Vec::new(), profiles: vec![profile] };
        let dir = std::env::temp_dir().join(format!("lituus-profiles-{}", std::process::id()));
        let file = dir.join("displays.toml");
        save(&file, &store).unwrap();
        let back = load(&file);
        assert_eq!(back, store);
        let rules_back: Vec<MonitorRule> = back.profiles[0].monitors.iter().map(StoredMonitor::rule).collect();
        assert_eq!(rules_back, r);
        assert_eq!(back.active(&r), Some(0));
        assert_eq!(back.profiles[0].summary(), "4 monitors · switches by itself");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_exactly_the_same_monitors_match() {
        let r = rules();
        let profile = |n: usize, auto: bool| Profile {
            name: format!("p{n}"),
            auto,
            monitors: r[..n].iter().map(StoredMonitor::from).collect(),
            ..Profile::default()
        };
        let store = Store { profiles: vec![profile(3, true), profile(4, false), profile(2, true)], ..Store::default() };
        let connected = |n: usize| r[..n].iter().rev().map(|m| m.output.clone()).collect::<Vec<_>>();
        assert_eq!(store.matching(&connected(3)).map(|p| p.name.as_str()), Some("p3"));
        assert_eq!(store.matching(&connected(2)).map(|p| p.name.as_str()), Some("p2"));
        assert_eq!(store.matching(&connected(4)), None, "p4 only by hand");
        assert_eq!(store.matching(&connected(1)), None);
    }

    #[test]
    fn no_file_means_switching_on() {
        assert!(load(Path::new("/nonexistent/displays.toml")).switch_on_hotplug);
    }
}
