//! The Displays page: Hyprland's monitors (lituus-hypr), edited on a copy of their rules; Apply
//! sets them live and asks to keep them, and only then writes the Displays part of
//! ~/.config/hypr/lituus.lua. Without an answer within 15 seconds the old settings come back,
//! so a black screen fixes itself.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lituus_hypr::{
    ConfigFile, Hyprland, Mode, Monitor, MonitorRule, WorkspaceRule, config_dir, scale_is_valid, valid_scales,
};
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::profiles::{self, Profile, Store, StoredMonitor, StoredWorkspace};
use crate::{Confirm, Confirmed, DisplaysView, DragSnap, LayoutMonitor, MainWindow, ProfileRow, WorkspaceRow, update};

/// The workspaces the Workspaces tab lists.
const WORKSPACES: usize = 10;

/// How long Apply waits for Keep.
pub const KEEP_FOR: Duration = Duration::from_secs(15);

/// The name of the Displays part of Lituus's file.
const SECTION: &str = "Displays";

#[derive(Default)]
pub struct State {
    hypr: Option<Hyprland>,
    pub not_running: bool,
    pub error: Option<String>,
    file: Option<ConfigFile>,
    loaded: bool,
    monitors: Vec<Monitor>,
    /// The rules as the monitors are now, and as edited.
    live: Vec<MonitorRule>,
    edit: Vec<MonitorRule>,
    selected: usize,
    /// After Apply: until when to wait for Keep, and what to go back to.
    keep: Option<(Instant, Vec<MonitorRule>)>,
    pub config_open: bool,
    /// The page's part of the file, as last written or read.
    saved: Option<String>,
    /// displays.toml, and the workspace bindings as edited.
    store: Store,
    store_read: bool,
    ws_edit: Vec<StoredWorkspace>,
    /// Workspace bindings before the last Apply, to go back to.
    ws_before: Option<Vec<StoredWorkspace>>,
    /// The page's own settings as edited, while they differ from displays.toml.
    options: Option<Options>,
    /// What was typed into a field and couldn't be used, by field ("x", "reserved:2", "scale"):
    /// the field keeps it, marked, and the monitor keeps its value.
    drafts: Vec<(String, String)>,
    /// "Other…" is chosen for the scale, so its field shows.
    other_scale: bool,
    /// Where Hyprland's config and displays.toml are, if not the usual places (for tests).
    hypr_dir: Option<PathBuf>,
    store_path: Option<PathBuf>,
}

/// The settings of the page itself, which wait for Apply like everything else on it: switching
/// profiles on hotplug, naming monitors by description, and each profile's "Switch by itself"
/// and command, by the profile's name.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Options {
    hotplug: bool,
    by_description: bool,
    profiles: Vec<(String, bool, String)>,
}

impl Options {
    fn of(store: &Store) -> Self {
        Self {
            hotplug: store.switch_on_hotplug,
            by_description: store.name_by_description,
            profiles: store.profiles.iter().map(|p| (p.name.clone(), p.auto, p.exec.clone())).collect(),
        }
    }

    /// Profile `p`'s "by itself" and command as edited.
    fn profile(&self, p: &Profile) -> (bool, String) {
        self.profiles.iter().find(|e| e.0 == p.name).map_or_else(|| (p.auto, p.exec.clone()), |e| (e.1, e.2.clone()))
    }
}

impl State {
    fn lua(&self) -> bool {
        self.file.as_ref().is_none_or(|f| f.lua)
    }

    /// The file's text for these rules.
    fn text(&self, rules: &[MonitorRule]) -> String {
        let profile = self.store.active(rules).map(|i| self.store.profiles[i].name.clone());
        let ws = self.named_workspaces(&self.store.workspaces);
        lituus_hypr::file(self.lua(), profile.as_deref(), &self.named_rules(rules), &ws)
    }

    /// How the written and applied rules name a monitor that the page knows by its selector:
    /// as that (`desc:…` when it has a serial), or by its port when "Name monitors by
    /// description" is off. Profiles and workspaces keep the selector, so they match either way.
    fn named(&self, selector: &str) -> String {
        match self.monitors.iter().find(|m| m.selector() == selector) {
            Some(m) if !self.store.name_by_description => m.name.clone(),
            _ => selector.to_owned(),
        }
    }

    fn named_rules(&self, rules: &[MonitorRule]) -> Vec<MonitorRule> {
        rules
            .iter()
            .map(|r| MonitorRule {
                output: self.named(&r.output),
                mirror: r.mirror.as_deref().map(|m| self.named(m)),
                ..r.clone()
            })
            .collect()
    }

    fn named_workspaces(&self, workspaces: &[StoredWorkspace]) -> Vec<WorkspaceRule> {
        workspaces.iter().map(|w| WorkspaceRule { monitor: self.named(&w.monitor), ..w.rule() }).collect()
    }

    /// Apply is waiting for Keep.
    pub fn keep_pending(&self) -> bool {
        self.keep.is_some()
    }

    fn dirty(&self) -> bool {
        self.edit != self.live || self.ws_edit != self.store.workspaces || self.options_changed() > 0
    }

    fn options(&self) -> Options {
        self.options.clone().unwrap_or_else(|| Options::of(&self.store))
    }

    /// Edits the page's own settings; they wait for Apply.
    fn edit_options(&mut self, change: impl FnOnce(&mut Options)) {
        let mut options = self.options();
        change(&mut options);
        self.options = (options != Options::of(&self.store)).then_some(options);
    }

    /// How many of the page's own settings differ from displays.toml.
    fn options_changed(&self) -> usize {
        let Some(o) = &self.options else { return 0 };
        let profiles = self.store.profiles.iter().filter(|p| o.profile(p) != (p.auto, p.exec.clone())).count();
        usize::from(o.hotplug != self.store.switch_on_hotplug)
            + usize::from(o.by_description != self.store.name_by_description)
            + profiles
    }

    /// Writes the page's own settings to displays.toml.
    fn apply_options(&mut self) {
        let Some(o) = self.options.take() else { return };
        self.store.switch_on_hotplug = o.hotplug;
        self.store.name_by_description = o.by_description;
        for p in &mut self.store.profiles {
            (p.auto, p.exec) = o.profile(p);
        }
        save_store(self);
    }

    fn draft(&self, field: &str) -> Option<&str> {
        self.drafts.iter().find(|d| d.0 == field).map(|d| d.1.as_str())
    }

    /// Forgets a field's unusable text, or keeps `text` for it.
    fn set_draft(&mut self, field: String, text: Option<&str>) {
        self.drafts.retain(|d| d.0 != field);
        if let Some(text) = text {
            self.drafts.push((field, text.to_owned()));
        }
    }

    /// Forgets what was typed and not used, and goes back to the scale's own entry.
    fn forget_drafts(&mut self) {
        self.drafts.clear();
        self.other_scale = false;
    }
}

/// Reads Hyprland's monitors (on a thread: the socket may take a moment).
pub fn refresh(weak: &slint::Weak<MainWindow>) {
    refresh_then(weak, false);
}

/// After a monitor came or went: reads the monitors, then switches to the profile made for
/// exactly those, if there is one.
pub fn hotplugged(weak: &slint::Weak<MainWindow>) {
    refresh_then(weak, true);
}

fn refresh_then(weak: &slint::Weak<MainWindow>, switch: bool) {
    let weak = weak.clone();
    std::thread::spawn(move || {
        let read = (|| {
            let h = Hyprland::from_env()?;
            let lua = h.version().map(|v| v.lua()).unwrap_or(true);
            let monitors = h.monitors()?;
            Ok::<_, lituus_hypr::Error>((h, lua, monitors))
        })();
        let w = weak.clone();
        let _ = weak.upgrade_in_event_loop(move |_| {
            let w2 = w.clone();
            update(&w, move |app| {
                loaded(&mut app.displays, read);
                if switch {
                    auto_switch(&w2, &mut app.displays);
                }
            })
        });
    });
}

/// Switches to the auto profile for the monitors plugged in, unless they already show it.
fn auto_switch(weak: &slint::Weak<MainWindow>, state: &mut State) {
    if !state.store.switch_on_hotplug || state.keep.is_some() {
        return;
    }
    let connected: Vec<String> = state.monitors.iter().map(Monitor::selector).collect();
    let Some(profile) = state.store.matching(&connected).cloned() else { return };
    let rules = profile_rules(state, &profile);
    let ws = profile.workspaces.clone();
    if rules == state.live && ws == state.store.workspaces {
        return;
    }
    let changed = changes(&state.live, &rules);
    state.edit = rules.clone();
    state.ws_edit = ws.clone();
    state.store.workspaces = ws;
    save_store(state);
    let ws = state.ws_edit.clone();
    send(weak, state, changed, ws);
    if let Some(file) = &state.file {
        let text = state.text(&rules);
        if file.write_section(SECTION, &text).is_ok() {
            state.saved = Some(text);
        }
    }
    if let Some(h) = &state.hypr {
        let _ = h.notify(1, 4000, &format!("Lituus: switched to {}", profile.name));
    }
    if !profile.exec.trim().is_empty() {
        let _ = std::process::Command::new("sh").arg("-c").arg(&profile.exec).spawn();
    }
}

/// A profile's rules for the monitors plugged in now (in the page's order); a monitor the
/// profile doesn't have keeps its rule.
fn profile_rules(state: &State, profile: &Profile) -> Vec<MonitorRule> {
    state
        .live
        .iter()
        .map(|r| profile.monitors.iter().find(|m| m.output == r.output).map_or_else(|| r.clone(), StoredMonitor::rule))
        .collect()
}

fn save_store(state: &mut State) {
    let path = state.store_path.clone().unwrap_or_else(profiles::path);
    if let Err(e) = profiles::save(&path, &state.store) {
        state.error = Some(format!("Couldn't save the profiles: {e}"));
    }
}

fn loaded(state: &mut State, read: Result<(Hyprland, bool, Vec<Monitor>), lituus_hypr::Error>) {
    if !state.store_read {
        state.store = profiles::load(&state.store_path.clone().unwrap_or_else(profiles::path));
        state.ws_edit = state.store.workspaces.clone();
        state.store_read = true;
    }
    match read {
        Ok((h, lua, mut monitors)) => {
            monitors.sort_by_key(|m| (m.disabled, m.x, m.y));
            let file = ConfigFile::new(state.hypr_dir.clone().unwrap_or_else(config_dir), lua);
            state.loaded = file.loaded();
            state.saved = file.read_section(SECTION);
            state.file = Some(file);
            state.hypr = Some(h);
            state.not_running = false;
            let mut live: Vec<MonitorRule> = monitors.iter().map(|m| MonitorRule::from_monitor(m, &monitors)).collect();
            // Hyprland doesn't report a monitor's VRR setting or reserved area: they stay as last
            // read or applied, or else come from Lituus's file.
            let written = state.saved.as_deref().map(lituus_hypr::unreported).unwrap_or_default();
            for (r, m) in live.iter_mut().zip(&monitors) {
                if let Some(old) = state.live.iter().find(|o| o.output == r.output) {
                    (r.vrr, r.reserved) = (old.vrr, old.reserved);
                } else if let Some(w) = written.iter().find(|w| w.output == r.output || w.output == m.name) {
                    (r.vrr, r.reserved) = (w.vrr, w.reserved);
                }
            }
            // Keep edits in progress across a refresh, unless the monitors changed. A change made
            // outside Lituus isn't an edit.
            let editing = state.dirty();
            state.live = live;
            let same = state.edit.len() == state.live.len()
                && state.edit.iter().zip(&state.live).all(|(a, b)| a.output == b.output);
            if !same || state.keep.is_none() && !editing {
                state.edit = state.live.clone();
            }
            state.monitors = monitors;
            state.selected = state.selected.min(state.monitors.len().saturating_sub(1));
        }
        Err(lituus_hypr::Error::NotRunning) => {
            state.not_running = true;
            state.monitors.clear();
        }
        Err(e) => state.error = Some(e.to_string()),
    }
}

/// Sets rules live, on a thread, then reads the monitors again.
fn send(weak: &slint::Weak<MainWindow>, state: &mut State, rules: Vec<MonitorRule>, workspaces: Vec<StoredWorkspace>) {
    // Hyprland doesn't report these back; the next read keeps them from here.
    for r in &rules {
        if let Some(l) = state.live.iter_mut().find(|l| l.output == r.output) {
            (l.vrr, l.reserved) = (r.vrr, r.reserved);
        }
    }
    let Some(h) = state.hypr.clone() else { return };
    let lua = state.lua();
    // Named as in the file, so a rule applied now replaces the one Hyprland read from it.
    let rules = state.named_rules(&rules);
    let workspaces = state.named_workspaces(&workspaces);
    let weak = weak.clone();
    std::thread::spawn(move || {
        let mut error = None;
        for r in &rules {
            let result = if lua { h.eval(&r.lua_live()) } else { h.keyword("monitor", &r.hyprlang()) };
            if let Err(e) = result {
                error = Some(format!("Hyprland didn't take the settings for {}: {e}", r.output));
            }
        }
        for w in &workspaces {
            let result = if lua { h.eval(&w.lua()) } else { h.keyword("workspace", &w.hyprlang()) };
            if let Err(e) = result {
                error = Some(format!("Hyprland didn't take the rule for workspace {}: {e}", w.workspace));
            }
        }
        // Hyprland applies on its next frame.
        std::thread::sleep(Duration::from_millis(300));
        let read = h.monitors().map(|m| (h.clone(), lua, m));
        let w = weak.clone();
        let _ = weak.upgrade_in_event_loop(move |_| {
            update(&w, move |app| {
                app.displays.error = error;
                loaded(&mut app.displays, read);
            })
        });
    });
}

/// The changed rules, in the order to apply them: monitors turned off last, so one stays on.
fn changes(from: &[MonitorRule], to: &[MonitorRule]) -> Vec<MonitorRule> {
    let mut changed: Vec<MonitorRule> = to.iter().zip(from).filter(|(t, f)| t != f).map(|(t, _)| t.clone()).collect();
    changed.sort_by_key(|r| r.disabled);
    changed
}

/// Everything the page does: (verb, argument, value). Updating or deleting a profile only asks:
/// the question comes back for the confirmation dialog, which runs `confirmed`.
pub fn act(weak: &slint::Weak<MainWindow>, state: &mut State, verb: &str, arg: &str, value: f32) -> Option<Confirm> {
    let i = state.selected;
    let index = arg.parse::<usize>().unwrap_or(0);
    match verb {
        "select" => {
            state.selected = index.min(state.monitors.len().saturating_sub(1));
            state.forget_drafts();
        }
        "apply" => apply(weak, state),
        "keep" => {
            state.keep = None;
            state.ws_before = None;
            state.store.workspaces = state.ws_edit.clone();
            save_store(state);
            save(state);
        }
        "revert-now" => revert(weak, state),
        "revert" => {
            state.edit = state.live.clone();
            state.ws_edit = state.store.workspaces.clone();
            state.options = None;
            state.forget_drafts();
        }
        "ws-monitor" | "ws-default" | "ws-persistent" => edit_workspace(state, verb, index, value),
        "switch-hotplug" => state.edit_options(|o| o.hotplug = !o.hotplug),
        "name-by-description" => state.edit_options(|o| o.by_description = !o.by_description),
        "profile-save" => {
            let name = arg.trim();
            if !name.is_empty() {
                let profile = Profile {
                    name: name.to_owned(),
                    auto: true,
                    exec: String::new(),
                    monitors: state.live.iter().map(StoredMonitor::from).collect(),
                    workspaces: state.store.workspaces.clone(),
                };
                match state.store.profiles.iter_mut().find(|p| p.name == name) {
                    Some(old) => *old = Profile { auto: old.auto, exec: old.exec.clone(), ..profile },
                    None => state.store.profiles.push(profile),
                }
                save_store(state);
                save(state);
            }
        }
        "profile-use" => {
            if let Some(p) = state.store.profiles.get(index).cloned() {
                state.edit = profile_rules(state, &p);
                state.ws_edit = p.workspaces.clone();
                apply(weak, state);
            }
        }
        "profile-update" | "profile-delete" => return ask(state, verb, index),
        "profile-auto" | "profile-exec" => {
            let (i, text) = if verb == "profile-exec" { arg.split_once(':').unwrap_or(("", "")) } else { (arg, "") };
            let p = i.parse::<usize>().ok().and_then(|i| state.store.profiles.get(i)).cloned()?;
            state.edit_options(|o| {
                let (auto, exec) = o.profile(&p);
                let (auto, exec) = if verb == "profile-auto" { (!auto, exec) } else { (auto, text.to_owned()) };
                o.profiles.retain(|e| e.0 != p.name);
                o.profiles.push((p.name.clone(), auto, exec));
            });
        }
        "save" => save(state),
        "view-config" => state.config_open = true,
        "close-config" => state.config_open = false,
        "scale" => {
            let rule = state.edit.get_mut(i)?;
            // The last entry is "Other…": its field takes any scale Hyprland can use.
            match nice_scales(rule.mode.width, rule.mode.height).get(index) {
                Some(&scale) => {
                    rule.scale = scale;
                    state.other_scale = false;
                }
                None => state.other_scale = true,
            }
            state.set_draft("scale".into(), None);
        }
        _ => {
            let rule = state.edit.get_mut(i)?;
            let monitor = state.monitors.get(i)?;
            let taken = edit(rule, monitor, &state.monitors, verb, arg, index, value);
            // A typed value that wasn't taken stays in its field, marked.
            let field = match verb {
                "x" | "y" => Some((verb.to_owned(), arg)),
                "scale-text" => Some(("scale".to_owned(), arg)),
                "reserved" => arg.split_once(':').map(|(side, text)| (format!("reserved:{side}"), text)),
                _ => None,
            };
            if let Some((field, text)) = field {
                state.set_draft(field, (!taken).then_some(text));
            }
        }
    }
    None
}

/// The question before profile `index` is overwritten or deleted.
fn ask(state: &State, verb: &str, index: usize) -> Option<Confirm> {
    let name = state.store.profiles.get(index)?.name.clone();
    Some(if verb == "profile-delete" {
        Confirm {
            title: format!("Delete {name}?"),
            text: "This deletes its saved monitor layout and workspaces.".into(),
            button: "Delete".into(),
            destructive: true,
            then: Confirmed::Displays("profile-delete", name),
        }
    } else {
        Confirm {
            title: format!("Update {name}?"),
            text: "Its saved monitor layout and workspaces are replaced by the ones in use now.".into(),
            button: "Update".into(),
            destructive: false,
            then: Confirmed::Displays("profile-update", name),
        }
    })
}

/// Runs a profile change the confirmation dialog asked about.
pub fn confirmed(state: &mut State, verb: &str, name: &str) {
    if change_profile(state, verb, name) {
        save_store(state);
    }
}

/// Updates the profile called `name` to the monitors and workspaces in use now, or deletes it.
/// By name: the list may have changed while the dialog was open.
fn change_profile(state: &mut State, verb: &str, name: &str) -> bool {
    let Some(index) = state.store.profiles.iter().position(|p| p.name == name) else { return false };
    match verb {
        "profile-update" => {
            let profile = &mut state.store.profiles[index];
            profile.monitors = state.live.iter().map(StoredMonitor::from).collect();
            profile.workspaces = state.store.workspaces.clone();
        }
        "profile-delete" => {
            state.store.profiles.remove(index);
        }
        _ => return false,
    }
    true
}

fn apply(weak: &slint::Weak<MainWindow>, state: &mut State) {
    if !state.dirty() || problem(state).is_some() || state.keep.is_some() {
        return;
    }
    // The page's own settings can't leave a screen black: they go to displays.toml now, and
    // without monitor changes there's nothing for Hyprland to try, so nothing to keep.
    let renamed = state.options().by_description != state.store.name_by_description;
    state.apply_options();
    if state.edit == state.live && state.ws_edit == state.store.workspaces {
        // Lituus's file names the monitors the other way now, if it has written one.
        if renamed && state.saved.is_some() {
            save(state);
        }
        return;
    }
    let rules = changes(&state.live, &state.edit);
    let ws: Vec<StoredWorkspace> =
        state.ws_edit.iter().filter(|w| !state.store.workspaces.contains(w)).cloned().collect();
    state.keep = Some((Instant::now() + KEEP_FOR, state.live.clone()));
    state.ws_before = Some(state.store.workspaces.clone());
    send(weak, state, rules, ws);
}

/// The Workspaces tab: row `index` is workspace index + 1.
fn edit_workspace(state: &mut State, verb: &str, index: usize, value: f32) {
    let name = (index + 1).to_string();
    let pos = state.ws_edit.iter().position(|w| w.workspace == name);
    match verb {
        "ws-monitor" => {
            let choice = value as usize;
            match (choice, pos) {
                (0, Some(p)) => {
                    state.ws_edit.remove(p);
                }
                (0, None) => {}
                (c, _) => {
                    let Some(monitor) = state.monitors.get(c - 1).map(Monitor::selector) else { return };
                    match pos {
                        Some(p) => state.ws_edit[p].monitor = monitor,
                        None => state.ws_edit.push(StoredWorkspace {
                            workspace: name,
                            monitor,
                            default: false,
                            persistent: false,
                        }),
                    }
                }
            }
        }
        "ws-default" => {
            if let Some(p) = pos {
                let on = !state.ws_edit[p].default;
                // One default per monitor.
                let monitor = state.ws_edit[p].monitor.clone();
                for w in &mut state.ws_edit {
                    if w.monitor == monitor {
                        w.default = false;
                    }
                }
                state.ws_edit[p].default = on;
            }
        }
        _ => {
            if let Some(p) = pos {
                state.ws_edit[p].persistent = !state.ws_edit[p].persistent;
            }
        }
    }
    state.ws_edit.sort_by_key(|w| w.workspace.parse::<u32>().unwrap_or(u32::MAX));
}

/// Every second of the countdown, and its end.
pub fn tick(weak: &slint::Weak<MainWindow>, state: &mut State) {
    if let Some((deadline, _)) = &state.keep
        && Instant::now() >= *deadline
    {
        revert(weak, state);
    }
}

fn revert(weak: &slint::Weak<MainWindow>, state: &mut State) {
    if let Some((_, previous)) = state.keep.take() {
        let rules = changes(&state.live, &previous);
        state.edit = previous;
        let ws = state.ws_before.take().unwrap_or_else(|| state.store.workspaces.clone());
        state.ws_edit = ws.clone();
        send(weak, state, rules, ws);
    }
}

fn save(state: &mut State) {
    let Some(file) = &state.file else { return };
    let text = state.text(&state.live);
    match file.write_section(SECTION, &text) {
        Ok(()) => state.saved = Some(text),
        Err(e) => state.error = Some(format!("Couldn't write {}: {e}", file.path().display())),
    }
}

/// Changes the edited rule; false when the text typed for it isn't a value it can take.
fn edit(
    rule: &mut MonitorRule,
    monitor: &Monitor,
    all: &[Monitor],
    verb: &str,
    arg: &str,
    index: usize,
    value: f32,
) -> bool {
    let modes = monitor.modes();
    match verb {
        "enabled" => rule.disabled = !rule.disabled,
        "resolution" => {
            if let Some(&(w, h)) = sizes(&modes).get(index) {
                // The fastest refresh rate at that size.
                let best = modes.iter().filter(|m| (m.width, m.height) == (w, h)).max_by_key(|m| m.refresh);
                if let Some(m) = best {
                    rule.mode = *m;
                }
                if !scale_is_valid(w, h, rule.scale) {
                    rule.scale = nearest_scale(w, h, rule.scale, &nice_scales(w, h));
                }
            }
        }
        "rate" => {
            if let Some(m) = rates(&modes, rule.mode).get(index) {
                rule.mode = *m;
            }
        }
        "scale-text" => {
            let (w, h) = (rule.mode.width, rule.mode.height);
            match parse_scale(arg) {
                // Exactly the one Hyprland takes, for "1.333".
                Some(s) if scale_is_valid(w, h, s) => rule.scale = nearest_scale(w, h, s, &valid_scales(w, h)),
                _ => return false,
            }
        }
        "rotation" => rule.transform = (rule.transform & 4) | (index as u8 & 3),
        "flip" => rule.transform ^= 4,
        "x" | "y" => {
            let Ok(v) = arg.trim().parse::<i32>() else { return false };
            if verb == "x" { rule.x = v } else { rule.y = v }
        }
        "mirror" => {
            let others: Vec<&Monitor> = all.iter().filter(|m| m.name != monitor.name).collect();
            rule.mirror = if index == 0 { None } else { others.get(index - 1).map(|m| m.selector()) };
        }
        "colour" => rule.cm = COLOUR_MODES.get(index).map_or("srgb", |c| c.0).to_owned(),
        "bitdepth" => rule.ten_bit = index == 1,
        "sdr-brightness" => rule.sdr_brightness = (f64::from(value) * 100.0).round() / 100.0,
        "sdr-saturation" => rule.sdr_saturation = (f64::from(value) * 100.0).round() / 100.0,
        "vrr" => rule.vrr = if index == 0 { None } else { Some(index as u8 - 1) },
        "reserved" => {
            let (side, text) = arg.split_once(':').unwrap_or(("0", arg));
            match (side.parse::<usize>(), text.trim().parse::<i32>()) {
                (Ok(side), Ok(v)) if side < 4 && v >= 0 => rule.reserved[side] = v,
                _ => return false,
            }
        }
        _ => {}
    }
    true
}

/// A monitor on the layout, in layout pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    /// Shares a stretch of edge with `o`, so the pointer can cross from one to the other.
    fn touches(&self, o: &Rect) -> bool {
        let side = (self.x == o.x + o.w || o.x == self.x + self.w) && self.y < o.y + o.h && o.y < self.y + self.h;
        let top = (self.y == o.y + o.h || o.y == self.y + self.h) && self.x < o.x + o.w && o.x < self.x + self.w;
        side || top
    }
}

/// The monitors a dragged one lines up with and lands next to: the others that are on and not
/// mirrored.
fn others(state: &State, index: usize) -> Vec<Rect> {
    state
        .edit
        .iter()
        .enumerate()
        .filter(|(j, r)| *j != index && !r.disabled && r.mirror.is_none())
        .map(|(_, r)| {
            let (w, h) = r.logical_size();
            Rect { x: r.x, y: r.y, w, h }
        })
        .collect()
}

/// Where a monitor being dragged to (x, y) shows, in layout pixels: its edges and centre snap to
/// the other monitors' within `threshold`, unless `free`. Nothing changes until it's dropped.
pub fn snap(state: &State, index: usize, x: f32, y: f32, threshold: f32, free: bool) -> DragSnap {
    let mut at = DragSnap { x: x.round(), y: y.round(), ..DragSnap::default() };
    let Some(rule) = state.edit.get(index) else { return at };
    if free {
        return at;
    }
    let (w, h) = rule.logical_size();
    let others = others(state, index);
    let sx = snap_axis(x, w, others.iter().map(|o| (o.x, o.w)), threshold);
    let sy = snap_axis(y, h, others.iter().map(|o| (o.y, o.h)), threshold);
    if let Some((pos, line)) = sx {
        at.x = pos as f32;
        at.v = true;
        at.vx = line;
    }
    if let Some((pos, line)) = sy {
        at.y = pos as f32;
        at.h = true;
        at.hy = line;
    }
    // The lines run over the dragged monitor and every other one with an edge or centre on them.
    let (x, y) = (at.x as i32, at.y as i32);
    let on = |line: f32, o: i32, olen: i32| [o as f32, (o + olen) as f32, o as f32 + olen as f32 / 2.0].contains(&line);
    let (mut vy1, mut vy2, mut hx1, mut hx2) = (y, y + h, x, x + w);
    for o in &others {
        if at.v && on(at.vx, o.x, o.w) {
            (vy1, vy2) = (vy1.min(o.y), vy2.max(o.y + o.h));
        }
        if at.h && on(at.hy, o.y, o.h) {
            (hx1, hx2) = (hx1.min(o.x), hx2.max(o.x + o.w));
        }
    }
    (at.vy1, at.vy2, at.hx1, at.hx2) = (vy1 as f32, vy2 as f32, hx1 as f32, hx2 as f32);
    at
}

/// The start on one axis that puts a monitor `len` long, now at `pos`, against or in line with
/// another's edge or centre, if one is within `threshold`: (start, where the line is).
fn snap_axis(pos: f32, len: i32, others: impl Iterator<Item = (i32, i32)>, threshold: f32) -> Option<(i32, f32)> {
    let mut best: Option<(i32, f32, f32)> = None;
    for (o, olen) in others {
        let centre = o as f32 + olen as f32 / 2.0;
        let candidates = [
            // Touching: my start at your end, my end at your start.
            (o + olen, (o + olen) as f32),
            (o - len, o as f32),
            // In line: starts, ends, centres.
            (o, o as f32),
            (o + olen - len, (o + olen) as f32),
            ((centre - len as f32 / 2.0).round() as i32, centre),
        ];
        for (start, line) in candidates {
            let d = (start as f32 - pos).abs();
            if d <= threshold && best.is_none_or(|b| d < b.2) {
                best = Some((start, line, d));
            }
        }
    }
    best.map(|(start, line, _)| (start, line))
}

/// Drops a monitor at (x, y), in layout pixels, rounded to whole ones. If it would overlap
/// another monitor or touch none (Hyprland would leave the pointer no way across), it goes to the
/// nearest place against one instead.
pub fn drop(state: &mut State, index: usize, x: f32, y: f32) {
    let others = others(state, index);
    let Some(rule) = state.edit.get_mut(index) else { return };
    let (w, h) = rule.logical_size();
    let at = place(Rect { x: x.round() as i32, y: y.round() as i32, w, h }, &others);
    rule.x = at.x;
    rule.y = at.y;
    // The layout's top-left corner stays at 0, 0, as Hyprland lays it out.
    let enabled: Vec<&MonitorRule> = state.edit.iter().filter(|r| !r.disabled).collect();
    let min_x = enabled.iter().map(|r| r.x).min().unwrap_or(0);
    let min_y = enabled.iter().map(|r| r.y).min().unwrap_or(0);
    if min_x < 0 || min_y < 0 {
        for r in &mut state.edit {
            r.x -= min_x.min(0);
            r.y -= min_y.min(0);
        }
    }
}

/// `me` if it touches one of `others` and overlaps none, or else the nearest place that does.
fn place(me: Rect, others: &[Rect]) -> Rect {
    let fits = |r: &Rect| others.iter().any(|o| r.touches(o)) && !others.iter().any(|o| r.overlaps(o));
    if others.is_empty() || fits(&me) {
        return me;
    }
    let mut best: Option<(Rect, i64)> = None;
    for o in others {
        // Against its left or right side, or its top or bottom.
        let ys = along(me.y, me.h, o.y, o.h, others.iter().flat_map(|p| [p.y + p.h, p.y - me.h]));
        let xs = along(me.x, me.w, o.x, o.w, others.iter().flat_map(|p| [p.x + p.w, p.x - me.w]));
        let sides = [o.x - me.w, o.x + o.w].into_iter().flat_map(|x| ys.iter().map(move |&y| Rect { x, y, ..me }));
        let ends = [o.y - me.h, o.y + o.h].into_iter().flat_map(|y| xs.iter().map(move |&x| Rect { x, y, ..me }));
        for c in sides.chain(ends).filter(|c| fits(c)) {
            let d = i64::from(c.x - me.x).pow(2) + i64::from(c.y - me.y).pow(2);
            if best.is_none_or(|b| d < b.1) {
                best = Some((c, d));
            }
        }
    }
    // Right of the right-most monitor, in line with its top: always free.
    best.map_or_else(
        || {
            let o = others.iter().max_by_key(|o| o.x + o.w).unwrap_or(&me);
            Rect { x: o.x + o.w, y: o.y, ..me }
        },
        |b| b.0,
    )
}

/// Starts on one axis that keep a monitor `len` long against a side `olen` long at `o`, sharing
/// some of it: where it was dropped (`pos`) if that does, in line with either end of the side, and
/// those of `more` that do (next to other monitors).
fn along(pos: i32, len: i32, o: i32, olen: i32, more: impl Iterator<Item = i32>) -> Vec<i32> {
    let shares = |s: &i32| (o - len + 1..o + olen).contains(s);
    let mut v = vec![o, o + olen - len];
    v.extend(std::iter::once(pos).chain(more).filter(shares));
    v
}

/// What keeps Apply off: an invalid scale, or monitors on top of each other.
fn problem(state: &State) -> Option<String> {
    for (r, m) in state.edit.iter().zip(&state.monitors) {
        if !r.disabled && !scale_is_valid(r.mode.width, r.mode.height, r.scale) {
            return Some(format!("{}: Hyprland can't use scale {}", m.name, short(r.scale)));
        }
    }
    let rects: Vec<(usize, i32, i32, i32, i32)> = state
        .edit
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.disabled && r.mirror.is_none())
        .map(|(i, r)| {
            let (w, h) = r.logical_size();
            (i, r.x, r.y, w, h)
        })
        .collect();
    for (k, a) in rects.iter().enumerate() {
        for b in &rects[k + 1..] {
            if a.1 < b.1 + b.3 && b.1 < a.1 + a.3 && a.2 < b.2 + b.4 && b.2 < a.2 + a.4 {
                let name = |i: usize| state.monitors.get(i).map(|m| m.name.clone()).unwrap_or_default();
                return Some(format!("{} and {} overlap", name(a.0), name(b.0)));
            }
        }
    }
    None
}

const COLOUR_MODES: [(&str, &str); 5] = [
    ("srgb", "sRGB"),
    ("wide", "Wide gamut (BT.2020)"),
    ("hdr", "HDR (wide gamut, PQ)"),
    ("hdredid", "HDR from the monitor's EDID"),
    ("auto", "Automatic"),
];
const VRR: [&str; 5] = ["Global setting", "Off", "On", "Fullscreen only", "Fullscreen games and video"];

fn sizes(modes: &[Mode]) -> Vec<(u32, u32)> {
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for m in modes {
        if !sizes.contains(&(m.width, m.height)) {
            sizes.push((m.width, m.height));
        }
    }
    sizes
}

fn rates(modes: &[Mode], current: Mode) -> Vec<Mode> {
    let mut rates: Vec<Mode> =
        modes.iter().filter(|m| (m.width, m.height) == (current.width, current.height)).copied().collect();
    if !rates.contains(&current) {
        rates.push(current);
    }
    rates.sort_by_key(|m| std::cmp::Reverse(m.refresh));
    rates
}

/// The scales the Scale list offers at w × h: those Hyprland takes that are steps of 0.05 or
/// thirds, the ones people use, up to 200 %. Hyprland takes many odd ones too, and bigger ones,
/// under "Other…".
fn nice_scales(w: u32, h: u32) -> Vec<f64> {
    let nice = |s: &f64| ((s * 20.0).round() - s * 20.0).abs() < 1e-6 || ((s * 3.0).round() - s * 3.0).abs() < 1e-6;
    valid_scales(w, h).into_iter().filter(|s| nice(s) && *s <= 2.0 + 1e-6).collect()
}

/// Of `scales`, the one nearest to `scale`.
fn nearest_scale(w: u32, h: u32, scale: f64, scales: &[f64]) -> f64 {
    let pick = |scales: &[f64]| scales.iter().copied().min_by(|a, b| (a - scale).abs().total_cmp(&(b - scale).abs()));
    pick(scales).or_else(|| pick(&valid_scales(w, h))).unwrap_or(1.0)
}

/// A typed scale: "1.25", or as a percentage, "125 %" (also without the %, from 10 up).
fn parse_scale(text: &str) -> Option<f64> {
    let text = text.trim();
    let (number, percent) = match text.strip_suffix('%') {
        Some(n) => (n.trim(), true),
        None => (text, false),
    };
    let n: f64 = number.replace(',', ".").parse().ok().filter(|n: &f64| n.is_finite() && *n > 0.0)?;
    Some(if percent || n >= 10.0 { n / 100.0 } else { n })
}

/// A scale as a percentage, as the Scale list shows it: "125 %", "133 %".
fn percent(scale: f64) -> String {
    format!("{} %", (scale * 100.0).round())
}

fn short(n: f64) -> String {
    let s = format!("{n:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

/// The sidebar's status: how many monitors are on.
pub fn status(state: &State) -> String {
    if state.not_running {
        return "Not running".to_owned();
    }
    match state.monitors.iter().filter(|m| !m.disabled).count() {
        0 => String::new(),
        n => n.to_string(),
    }
}

pub fn view(state: &State) -> DisplaysView {
    let mut v = DisplaysView::default();
    if state.not_running {
        v.state = s("not-running");
        // The card says why; the hero's line only the state.
        v.sub = s("Not running");
        v.empty_title = s("Hyprland isn't running");
        v.empty_text = s("The Displays page sets up monitors through Hyprland. Open Lituus inside a Hyprland session.");
        return v;
    }
    if state.monitors.is_empty() {
        v.state = s("loading");
        v.empty_title = s(state.error.clone().unwrap_or_else(|| "Reading Hyprland's monitors…".into()));
        return v;
    }
    v.state = s("list");
    let on = state.edit.iter().filter(|r| !r.disabled).count();
    let count = match (state.monitors.len(), on) {
        (1, _) => "1 monitor".to_owned(),
        (n, m) if m == n => format!("{n} monitors"),
        (n, m) => format!("{n} monitors · {m} on"),
    };
    v.sub = s(match state.store.active(&state.live) {
        Some(k) => format!("{count} · profile {}", state.store.profiles[k].name),
        None => count,
    });
    // The layout, and its bounds.
    let mut bounds: Option<(i32, i32, i32, i32)> = None;
    let mut layout = Vec::new();
    for (i, (r, m)) in state.edit.iter().zip(&state.monitors).enumerate() {
        let (w, h) = r.logical_size();
        let mirrored = r.mirror.is_some();
        if !r.disabled && !mirrored {
            bounds = Some(match bounds {
                None => (r.x, r.y, r.x + w, r.y + h),
                Some((a, b, c, d)) => (a.min(r.x), b.min(r.y), c.max(r.x + w), d.max(r.y + h)),
            });
        }
        layout.push(LayoutMonitor {
            n: i as i32 + 1,
            port: s(&m.name),
            name: s(&m.model),
            sub: s(if r.disabled {
                "Off".to_owned()
            } else if mirrored {
                format!(
                    "Mirrors {}",
                    state
                        .monitors
                        .iter()
                        .find(|o| r.mirror.as_deref() == Some(o.selector().as_str()))
                        .map_or("", |o| o.name.as_str())
                )
            } else {
                // Short enough for the smallest monitor on the layout; the card has the details.
                format!("{} × {} · {} Hz", r.mode.width, r.mode.height, (f64::from(r.mode.refresh) / 1000.0).round())
            }),
            x: r.x as f32,
            y: r.y as f32,
            w: w as f32,
            h: h as f32,
            selected: i == state.selected,
            disabled: r.disabled,
            mirrored,
            workspaces: s(state
                .ws_edit
                .iter()
                .filter(|w| w.monitor == r.output)
                .map(|w| if w.default { format!("{}★", w.workspace) } else { w.workspace.clone() })
                .collect::<Vec<_>>()
                .join("  ")),
        });
    }
    let (bx, by, bx2, by2) = bounds.unwrap_or((0, 0, 1920, 1080));
    // Monitors that are off or mirrored stand aside, after the others, at their own size.
    let mut aside = bx2 + 200;
    for (l, r) in layout.iter_mut().zip(&state.edit) {
        if r.disabled || r.mirror.is_some() {
            l.x = aside as f32;
            l.y = by as f32;
            aside += l.w as i32 + 120;
        }
    }
    let right = if aside > bx2 + 200 { aside - 120 } else { bx2 };
    v.bounds_x = bx as f32;
    v.bounds_y = by as f32;
    v.bounds_w = (right - bx).max(1) as f32;
    v.bounds_h = (by2 - by).max(1) as f32;
    v.logical = s(format!("Layout pixels · {} × {} · drag to move, hold Shift not to snap", bx2 - bx, by2 - by));
    v.monitors = ModelRc::new(VecModel::from(layout));

    // The selected monitor.
    let i = state.selected;
    let (Some(r), Some(m)) = (state.edit.get(i), state.monitors.get(i)) else { return v };
    let live = state.live.get(i);
    v.sel_n = i as i32 + 1;
    v.sel_title = s(format!("{} {}", short_make(&m.make), m.model).trim());
    v.sel_sub = s([
        Some(m.name.clone()),
        m.inches().map(|i| format!("{i}\"")),
        (!m.serial.is_empty()).then(|| format!("serial {}", m.serial)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · "));
    v.sel_enabled = !r.disabled;
    let modes = m.modes();
    let sz = sizes(&modes);
    v.resolutions = strings(
        sz.iter().enumerate().map(
            |(k, (w, h))| {
                if k == 0 { format!("{w} × {h} (preferred)") } else { format!("{w} × {h}") }
            },
        ),
    );
    v.resolution = sz.iter().position(|&x| x == (r.mode.width, r.mode.height)).unwrap_or(0) as i32;
    let rs = rates(&modes, r.mode);
    v.rates = strings(rs.iter().map(|m| format!("{} Hz", m.refresh_text())));
    v.rate = rs.iter().position(|x| *x == r.mode).unwrap_or(0) as i32;
    // The scale: a list of the usual ones, and "Other…" with a field for the rest.
    let (mw, mh) = (r.mode.width, r.mode.height);
    let looks = |scale: f64| {
        let (w, h) = MonitorRule { scale, ..r.clone() }.logical_size();
        format!("Looks like {w} × {h}")
    };
    let nice = nice_scales(mw, mh);
    let listed = nice.iter().position(|n| (n - r.scale).abs() < 1e-6);
    v.scales = strings(nice.iter().map(|n| percent(*n)).chain(std::iter::once("Other…".to_owned())));
    v.scale_details = strings(nice.iter().map(|n| looks(*n)));
    v.scale = listed.filter(|_| !state.other_scale).unwrap_or(nice.len()) as i32;
    v.scale_other = state.other_scale || listed.is_none();
    let typed = state.draft("scale");
    v.scale_text = s(typed.map_or_else(|| short(r.scale), str::to_owned));
    v.scale_valid = typed.is_none() && scale_is_valid(mw, mh, r.scale);
    let applied = live.is_none_or(|l| (l.scale - r.scale).abs() < 1e-6);
    v.scale_hint = s(match typed.map(parse_scale) {
        Some(None) => "Type a scale, such as 1.25 or 125 %".to_owned(),
        Some(Some(t)) => cant_scale(mw, mh, t),
        None if !v.scale_valid => cant_scale(mw, mh, r.scale),
        None if !applied => format!("{} · not applied yet", looks(r.scale)),
        None => looks(r.scale),
    });
    v.rotation = i32::from(r.transform & 3);
    v.flipped = r.transform & 4 != 0;
    // Position: what was typed if it wasn't a number.
    let field = |name: &str, value: i32| state.draft(name).map_or_else(|| value.to_string(), str::to_owned);
    v.x_text = s(field("x", r.x));
    v.y_text = s(field("y", r.y));
    v.x_bad = state.draft("x").is_some();
    v.y_bad = state.draft("y").is_some();
    v.position_hint = s(if v.x_bad || v.y_bad {
        "Type whole numbers of layout pixels"
    } else if live.is_some_and(|l| (l.x, l.y) != (r.x, r.y)) {
        "Not applied yet"
    } else {
        ""
    });
    let others: Vec<&Monitor> = state.monitors.iter().filter(|o| o.name != m.name).collect();
    v.mirrors =
        strings(std::iter::once("Off".to_owned()).chain(others.iter().map(|o| format!("{} · {}", o.name, o.model))));
    v.mirror =
        r.mirror.as_deref().and_then(|sel| others.iter().position(|o| o.selector() == sel)).map_or(0, |p| p as i32 + 1);
    v.colour_modes = strings(COLOUR_MODES.iter().map(|c| c.1.to_owned()));
    v.colour_mode = COLOUR_MODES.iter().position(|c| c.0 == r.cm).unwrap_or(0) as i32;
    v.hdr = r.cm.starts_with("hdr");
    v.hdr_capable = m.hardware_details.hdr;
    v.ten_bit = r.ten_bit;
    v.sdr_brightness = r.sdr_brightness as f32;
    v.sdr_saturation = r.sdr_saturation as f32;
    v.vrr_options = strings(VRR.iter().map(|x| (*x).to_owned()));
    v.vrr = r.vrr.map_or(0, |x| i32::from(x) + 1);
    v.vrr_capable = m.hardware_details.vrr_capable;
    v.reserved = strings((0..4).map(|k| field(&format!("reserved:{k}"), r.reserved[k])));
    let bad: Vec<bool> = (0..4).map(|k| state.draft(&format!("reserved:{k}")).is_some()).collect();
    v.reserved_hint = s(if bad.contains(&true) {
        "Type whole numbers of pixels, 0 or more"
    } else if live.is_some_and(|l| l.reserved != r.reserved) {
        "Not applied yet"
    } else {
        "Space tiled windows keep free, besides bars"
    });
    v.reserved_bad = ModelRc::new(VecModel::from(bad));

    // The bar at the bottom, and Apply's dialog.
    let problem = problem(state);
    let changed = state.edit.iter().zip(&state.live).filter(|(a, b)| a != b).count()
        + workspaces_changed(&state.ws_edit, &state.store.workspaces)
        + state.options_changed();
    let saved = state.saved.as_deref() == Some(state.text(&state.live).as_str());
    let not_loaded = state.file.is_some() && !state.loaded;
    let (status, tone) = if let Some(e) = &state.error {
        (e.clone(), "danger")
    } else if let Some(p) = &problem {
        (format!("{p}, so Apply is off"), "danger")
    } else if state.keep.is_some() {
        ("Applied; waiting for you to keep it".to_owned(), "accent")
    } else if changed == 1 {
        ("1 change not applied".to_owned(), "accent")
    } else if changed > 1 {
        (format!("{changed} changes not applied"), "accent")
    } else if state.saved.is_none() {
        // Lituus hasn't written its file yet, and nothing changed.
        ("No changes".to_owned(), "faint")
    } else if saved && not_loaded {
        // Written, but Hyprland's config doesn't read it (the banner above says so).
        ("Applied; lost when Hyprland restarts".to_owned(), "warn")
    } else if saved {
        ("Everything is applied and saved".to_owned(), "muted")
    } else {
        // Changed outside Lituus, or by "Name monitors by description".
        ("The config file has older settings than the monitors now".to_owned(), "warn")
    };
    v.status = s(status);
    v.status_tone = s(tone);
    let dirty = changed > 0;
    v.can_apply = dirty && problem.is_none() && state.keep.is_none();
    v.can_revert = dirty && state.keep.is_none();
    v.can_save = !dirty && state.saved.is_some() && !saved && state.keep.is_none();
    if let Some((deadline, _)) = &state.keep {
        let left = deadline.saturating_duration_since(Instant::now());
        v.keep_open = true;
        v.keep_seconds = left.as_secs_f32().ceil() as i32;
        v.keep_fraction = left.as_secs_f32() / KEEP_FOR.as_secs_f32();
    }
    if let Some(file) = &state.file {
        v.not_loaded = !state.loaded;
        v.load_line = s(file.load_line());
        v.config_path = s(shorten_home(&file.path().display().to_string()));
        v.config_main = s(shorten_home(&file.main_path().display().to_string()));
    }
    v.config_open = state.config_open;
    let options = state.options();
    v.by_description = options.by_description;
    // Workspaces.
    v.ws_monitors = strings(
        std::iter::once("Any monitor".to_owned())
            .chain(state.monitors.iter().map(|m| format!("{} · {}", m.name, m.model))),
    );
    v.workspaces = ModelRc::new(VecModel::from(
        (1..=WORKSPACES)
            .map(|n| {
                let w = state.ws_edit.iter().find(|w| w.workspace == n.to_string());
                WorkspaceRow {
                    name: s(n.to_string()),
                    monitor: w
                        .and_then(|w| state.monitors.iter().position(|m| m.selector() == w.monitor))
                        .map_or(0, |p| p as i32 + 1),
                    bound: w.is_some(),
                    default: w.is_some_and(|w| w.default),
                    persistent: w.is_some_and(|w| w.persistent),
                }
            })
            .collect::<Vec<_>>(),
    ));
    // Profiles.
    let active = state.store.active(&state.live);
    v.switch_hotplug = options.hotplug;
    v.profiles = ModelRc::new(VecModel::from(
        state
            .store
            .profiles
            .iter()
            .enumerate()
            .map(|(k, p)| {
                let (auto, exec) = options.profile(p);
                ProfileRow {
                    name: s(&p.name),
                    summary: s(p.summary()),
                    active: active == Some(k),
                    auto,
                    exec_hint: s(if exec != p.exec { "Not applied yet" } else { "" }),
                    exec: s(exec),
                }
            })
            .collect::<Vec<_>>(),
    ));
    v.profile = s(active.map(|k| state.store.profiles[k].name.clone()).unwrap_or_default());
    v.config_text = s(state.text(&state.edit));
    v
}

/// Why a scale can't be used at w × h.
fn cant_scale(w: u32, h: u32, scale: f64) -> String {
    let nearest = nearest_scale(w, h, scale, &valid_scales(w, h));
    format!("Hyprland can't use {} at {w} × {h}; {} is the nearest", short(scale), short(nearest))
}

/// How many workspaces are bound differently in `a` than in `b`.
fn workspaces_changed(a: &[StoredWorkspace], b: &[StoredWorkspace]) -> usize {
    let mut names: Vec<&str> = a.iter().chain(b).map(|w| w.workspace.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let find = |list: &[StoredWorkspace], n: &str| list.iter().find(|w| w.workspace == n).cloned();
    names.into_iter().filter(|n| find(a, n) != find(b, n)).count()
}

fn short_make(make: &str) -> &str {
    make.strip_suffix(" Electric Company").unwrap_or(make)
}

fn shorten_home(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

thread_local! {
    /// The layout's monitors, kept as one model: replacing it would rebuild the monitors on the
    /// layout mid-drag, and the drop would land somewhere else. The same for the workspace and
    /// profile rows and the reserved area, whose fields would lose the focus and what's being
    /// typed in them.
    static LAYOUT: Rc<VecModel<LayoutMonitor>> = Rc::new(VecModel::default());
    static WORKSPACE_ROWS: Rc<VecModel<WorkspaceRow>> = Rc::new(VecModel::default());
    static PROFILE_ROWS: Rc<VecModel<ProfileRow>> = Rc::new(VecModel::default());
    static RESERVED: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
}

/// `rows` in `model`, changing only the rows that differ.
fn kept<T: Clone + PartialEq + 'static>(model: &Rc<VecModel<T>>, rows: &ModelRc<T>) -> ModelRc<T> {
    let rows: Vec<T> = rows.iter().collect();
    if model.row_count() == rows.len() {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        model.set_vec(rows);
    }
    ModelRc::from(model.clone())
}

pub fn render(window: &MainWindow, state: &State) {
    let mut v = view(state);
    v.monitors = LAYOUT.with(|model| kept(model, &v.monitors));
    v.workspaces = WORKSPACE_ROWS.with(|model| kept(model, &v.workspaces));
    v.profiles = PROFILE_ROWS.with(|model| kept(model, &v.profiles));
    v.reserved = RESERVED.with(|model| kept(model, &v.reserved));
    window.set_displays(v);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        let monitors: Vec<Monitor> =
            serde_json::from_str(include_str!("../../hypr/tests/fixtures/monitors.json")).unwrap();
        // Not the real displays.toml or Hyprland config.
        let mut st = State { store_read: true, hypr_dir: Some("/nonexistent".into()), ..State::default() };
        loaded(&mut st, Ok((Hyprland::at("/nonexistent"), true, monitors)));
        st
    }

    fn weak() -> slint::Weak<MainWindow> {
        slint::Weak::default()
    }

    fn fixture() -> Vec<Monitor> {
        serde_json::from_str(include_str!("../../hypr/tests/fixtures/monitors.json")).unwrap()
    }

    /// A state with Hyprland's config and displays.toml in a new folder.
    fn state_in(name: &str) -> (State, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lituus-displays-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Hyprland's config loads Lituus's file.
        std::fs::write(dir.join("hyprland.lua"), "require(\"lituus\")\n").unwrap();
        let mut st =
            State { hypr_dir: Some(dir.clone()), store_path: Some(dir.join("displays.toml")), ..State::default() };
        loaded(&mut st, Ok((Hyprland::at("/nonexistent"), true, fixture())));
        (st, dir)
    }

    /// Hyprland's monitors read again.
    fn reread(st: &mut State, monitors: &[Monitor]) {
        loaded(st, Ok((Hyprland::at("/nonexistent"), true, monitors.to_vec())));
    }

    #[test]
    fn the_layout_is_ordered_left_to_right() {
        let st = state();
        let ports: Vec<String> = st.monitors.iter().map(|m| m.name.clone()).collect();
        assert_eq!(ports, ["DP-3", "HDMI-A-3", "DP-5", "DP-4"]);
        let v = view(&st);
        assert_eq!(v.state, "list");
        assert_eq!(v.sub, "4 monitors");
        assert_eq!((v.bounds_w, v.bounds_h), (7680.0, 2880.0));
        assert_eq!(v.monitors.row_data(2).unwrap().sub, "3840 × 2160 · 240 Hz");
        // 239.76 Hz, rounded on the layout.
        assert_eq!(v.monitors.row_data(0).unwrap().sub, "2560 × 1440 · 240 Hz");
    }

    /// A small tile's text shrinks with it, the port elides instead of running past the edge,
    /// and the lines that don't fit go: the number and the port stay.
    #[test]
    fn small_tiles_keep_their_text_inside() {
        use i_slint_backend_testing::ElementHandle;
        use slint::{ComponentHandle, LogicalSize};
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(1040.0, 600.0));
        window.set_page("displays".into());
        // One monitor far off to the right, so that the layout draws them all small.
        let mut monitors = fixture();
        monitors.iter_mut().find(|m| m.name == "DP-4").unwrap().x += 14000;
        let mut st = state();
        reread(&mut st, &monitors);
        window.set_displays(view(&st));
        window.show().unwrap();
        let v = view(&st);
        for m in v.monitors.iter() {
            let label = format!("{}, {}, {}", m.port, m.name, m.sub);
            let tile = ElementHandle::find_by_accessible_label(&window, &label).next().unwrap();
            let (t, ts) = (tile.absolute_position(), tile.size());
            assert!(ts.height < 60.0, "{label} is {ts:?}, not small");
            let inside = |e: &ElementHandle| {
                let (p, s) = (e.absolute_position(), e.size());
                p.x >= t.x
                    && p.y >= t.y
                    && p.x + s.width <= t.x + ts.width + 0.5
                    && p.y + s.height <= t.y + ts.height + 0.5
            };
            let overlaps = |e: &ElementHandle| {
                let (p, s) = (e.absolute_position(), e.size());
                p.x < t.x + ts.width && t.x < p.x + s.width && p.y < t.y + ts.height && t.y < p.y + s.height
            };
            let port: Vec<_> = ElementHandle::find_by_accessible_label(&window, &m.port).filter(overlaps).collect();
            assert_eq!(port.len(), 1, "{label}: the port");
            assert!(inside(&port[0]), "{label}: the port runs past the edge");
            for text in [&m.name, &m.sub] {
                for e in ElementHandle::find_by_accessible_label(&window, text).filter(overlaps) {
                    assert!(inside(&e), "{label}: {text} is cut off");
                }
            }
        }
    }

    #[test]
    fn edits_wait_for_apply() {
        let mut st = state();
        let w = weak();
        act(&w, &mut st, "select", "2", 0.0);
        assert_eq!(view(&st).sel_title, "Samsung Odyssey G81SF");
        // 160 %, the next scale up that 3840 × 2160 divides into whole pixels.
        act(&w, &mut st, "scale", "5", 0.0);
        assert!((st.edit[2].scale - 1.6).abs() < 1e-9);
        let v = view(&st);
        assert!(v.can_apply && v.can_revert);
        assert_eq!(v.status, "1 change not applied");
        assert_eq!(v.scale_hint, "Looks like 2400 × 1350 · not applied yet");
        // 133 % makes the G8 bigger than the gap to DP-4.
        act(&w, &mut st, "scale", "3", 0.0);
        assert!((st.edit[2].scale - 4.0 / 3.0).abs() < 1e-9);
        assert!(view(&st).status.contains("overlap"));
        act(&w, &mut st, "revert", "", 0.0);
        assert_eq!(st.edit, st.live);
    }

    /// Scale is one list of the usual scales, as percentages with the size they give, and
    /// "Other…" for the rest.
    #[test]
    fn scale_is_a_list_of_percentages_and_other() {
        let mut st = state();
        let w = weak();
        act(&w, &mut st, "select", "2", 0.0);
        let v = view(&st);
        let scales: Vec<String> = v.scales.iter().map(|s| s.to_string()).collect();
        assert_eq!(scales, ["100 %", "120 %", "125 %", "133 %", "150 %", "160 %", "167 %", "200 %", "Other…"]);
        assert_eq!(v.scale_details.row_data(3).unwrap(), "Looks like 2880 × 1620");
        assert_eq!((v.scale, v.scale_other), (4, false));
        assert_eq!(v.scale_hint, "Looks like 2560 × 1440");

        // "Other…" shows the field with the scale in use; an odd scale Hyprland takes goes in.
        act(&w, &mut st, "scale", "8", 0.0);
        let v = view(&st);
        assert_eq!((v.scale, v.scale_other, v.scale_text.as_str()), (8, true, "1.5"));
        act(&w, &mut st, "scale-text", "1.875", 0.0);
        assert!((st.edit[2].scale - 1.875).abs() < 1e-9);
        assert_eq!(view(&st).scale_hint, "Looks like 2048 × 1152 · not applied yet");
        // As a percentage too, and to the exact scale.
        act(&w, &mut st, "scale-text", "133.3 %", 0.0);
        assert!((st.edit[2].scale - 4.0 / 3.0).abs() < 1e-9);
        assert_eq!(view(&st).scale, 8, "still under Other… while it's chosen");

        // One it doesn't take stays in the field, marked, and the monitor keeps its scale.
        act(&w, &mut st, "scale-text", "1.3", 0.0);
        let v = view(&st);
        assert!((st.edit[2].scale - 4.0 / 3.0).abs() < 1e-9);
        assert_eq!(v.scale_text, "1.3");
        assert!(!v.scale_valid);
        assert_eq!(v.scale_hint, "Hyprland can't use 1.3 at 3840 × 2160; 1.333 is the nearest");
        act(&w, &mut st, "scale-text", "big", 0.0);
        assert_eq!(view(&st).scale_hint, "Type a scale, such as 1.25 or 125 %");
        // Picking from the list again forgets it.
        act(&w, &mut st, "scale", "0", 0.0);
        let v = view(&st);
        assert_eq!((v.scale, v.scale_other, v.scale_valid), (0, false, true));
        assert_eq!(st.edit[2].scale, 1.0);
        // An odd scale in use shows as Other… by itself.
        st.edit[2].scale = 1.875;
        assert_eq!((view(&st).scale, view(&st).scale_other), (8, true));
    }

    /// Text that isn't a number stays in its field, marked, and changes nothing; the next
    /// number replaces it. A field that differs from what's applied says so.
    #[test]
    fn typed_fields_keep_what_they_cant_use() {
        let mut st = state();
        let w = weak();
        act(&w, &mut st, "select", "3", 0.0);
        act(&w, &mut st, "x", "65oo", 0.0);
        let v = view(&st);
        assert_eq!((v.x_text.as_str(), v.x_bad, v.y_bad), ("65oo", true, false));
        assert_eq!(v.position_hint, "Type whole numbers of layout pixels");
        assert_eq!(st.edit, st.live);
        assert_eq!(v.status, "No changes");
        act(&w, &mut st, "x", " 6400 ", 0.0);
        let v = view(&st);
        assert_eq!((v.x_text.as_str(), v.x_bad), ("6400", false));
        assert_eq!(v.position_hint, "Not applied yet");

        act(&w, &mut st, "reserved", "2:-4", 0.0);
        let v = view(&st);
        assert_eq!(v.reserved.row_data(2).unwrap(), "-4");
        assert_eq!(v.reserved_bad.row_data(2), Some(true));
        assert_eq!(v.reserved_hint, "Type whole numbers of pixels, 0 or more");
        act(&w, &mut st, "reserved", "2:40", 0.0);
        assert_eq!(st.edit[3].reserved, [0, 0, 40, 0]);
        assert_eq!(view(&st).reserved_hint, "Not applied yet");
        // Another monitor, or Revert, forgets the text.
        act(&w, &mut st, "x", "x", 0.0);
        act(&w, &mut st, "select", "2", 0.0);
        act(&w, &mut st, "select", "3", 0.0);
        assert!(!view(&st).x_bad);
        act(&w, &mut st, "revert", "", 0.0);
        assert_eq!(view(&st).position_hint, "");
    }

    #[test]
    fn a_bad_scale_or_an_overlap_keeps_apply_off() {
        let mut st = state();
        let w = weak();
        // A scale that isn't valid (from a profile, say).
        st.edit[3].scale = 1.3;
        assert!(view(&st).status.contains("can't use scale 1.3"));
        assert!(!view(&st).can_apply);
        act(&w, &mut st, "revert", "", 0.0);
        // DP-4 moved onto the G8.
        act(&w, &mut st, "select", "3", 0.0);
        act(&w, &mut st, "x", "4000", 0.0);
        assert!(view(&st).status.contains("overlap"), "{}", view(&st).status);
    }

    // The fixture's monitors, all 2560 × 1440 in layout pixels: DP-3 at 1218, 2393, HDMI-A-3 at
    // 3778, 953, the G8 (DP-5) under it at 3778, 2393, and DP-4 right of that at 6338, 2393.

    #[test]
    fn a_dragged_monitor_snaps_against_an_edge() {
        let st = state();
        // DP-4 50 px right of the G8 and well above it: its left edge goes to the G8's right one.
        let at = snap(&st, 3, 6388.0, 1993.0, 100.0, false);
        assert_eq!((at.x, at.y), (6338.0, 1993.0));
        assert!(at.v && !at.h);
        // The line is at that edge, over DP-4 and both monitors whose right edge it is.
        assert_eq!((at.vx, at.vy1, at.vy2), (6338.0, 953.0, 3833.0));
    }

    #[test]
    fn a_dragged_monitor_snaps_in_line_with_another() {
        // DP-4 at half the size (1280 × 720), far right of the others and 30 px below HDMI-A-3's
        // top: the tops line up.
        let mut st = state();
        st.edit[3].scale = 2.0;
        let at = snap(&st, 3, 6838.0, 983.0, 100.0, false);
        assert_eq!((at.x, at.y), (6838.0, 953.0));
        assert!(at.h && !at.v);
        assert_eq!((at.hy, at.hx1, at.hx2), (953.0, 3778.0, 8118.0));
        // 12 px off the G8's centre.
        let at = snap(&st, 3, 4430.0, 4000.0, 100.0, false);
        assert_eq!((at.x, at.vx), (4418.0, 5058.0));
    }

    #[test]
    fn snapping_needs_the_edge_near_and_no_shift() {
        let st = state();
        // Farther than the threshold.
        let at = snap(&st, 3, 6388.0, 1993.0, 40.0, false);
        assert_eq!((at.x, at.y, at.v, at.h), (6388.0, 1993.0, false, false));
        // Shift held.
        let at = snap(&st, 3, 6388.0, 1993.0, 100.0, true);
        assert_eq!((at.x, at.y, at.v, at.h), (6388.0, 1993.0, false, false));
        // On whole layout pixels either way.
        let at = snap(&st, 3, 6388.4, 1993.6, 100.0, true);
        assert_eq!((at.x, at.y), (6388.0, 1994.0));
        // Snapping changes nothing; only the drop does.
        assert_eq!(st.edit, st.live);
    }

    #[test]
    fn a_drop_on_another_monitor_goes_next_to_it() {
        let mut st = state();
        // DP-4 dropped 538 px into the G8: it goes back against its right edge, at the same height.
        drop(&mut st, 3, 5800.0, 2300.0);
        assert_eq!((st.edit[3].x, st.edit[3].y), (6338, 2300));
        assert!(!view(&st).status.contains("overlap"));
        // Dropped onto DP-3 and the G8 at once, nearest their bottom edges: under them.
        drop(&mut st, 3, 2400.0, 3300.0);
        assert_eq!((st.edit[3].x, st.edit[3].y), (2400, 3833));
    }

    #[test]
    fn a_drop_away_from_the_others_goes_against_the_nearest() {
        let mut st = state();
        // 662 px right of the G8.
        drop(&mut st, 3, 7000.4, 2392.6);
        assert_eq!((st.edit[3].x, st.edit[3].y), (6338, 2393));
        // Far under DP-3: up against its bottom.
        drop(&mut st, 3, 1218.0, 5000.0);
        assert_eq!((st.edit[3].x, st.edit[3].y), (1218, 3833));
        // Only touching at a corner is not touching.
        drop(&mut st, 3, 6338.0, 3833.0);
        assert_eq!((st.edit[3].x, st.edit[3].y), (6338, 3833 - 1440));
    }

    #[test]
    fn a_drop_where_it_fits_stays_there() {
        let mut st = state();
        // Right of HDMI-A-3, sharing some of its edge with it, and on top of nothing.
        drop(&mut st, 3, 6338.0, 900.0);
        assert_eq!((st.edit[3].x, st.edit[3].y), (6338, 900));
        // Left of DP-3 and past the layout's edge: everything moves right, so it starts at 0.
        drop(&mut st, 3, -1342.0, 2393.0);
        assert_eq!((st.edit[3].x, st.edit[3].y), (0, 2393));
        assert_eq!(st.edit[0].x, 2560);
    }

    /// A drag moves the tile 1:1 with the pointer, from where it was grabbed, and asks Rust only
    /// where it lines up (`snap`, which changes nothing); the drop is the only change. A press
    /// that moves less than the dead zone only selects.
    #[test]
    fn a_drag_moves_only_the_tile_until_the_drop() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::{PointerEventButton, WindowEvent};
        use slint::{ComponentHandle, LogicalPosition, LogicalSize};
        use std::cell::RefCell;
        use std::rc::Rc;
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(1200.0, 760.0));
        window.set_page("displays".into());
        let st = state();
        render(&window, &st);
        window.show().unwrap();
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let c = calls.clone();
        // As it is, so the tile is where the pointer puts it.
        window.on_displays_snap(move |i, x, y, _, _| {
            c.borrow_mut().push(format!("snap {i}"));
            DragSnap { x, y, ..DragSnap::default() }
        });
        let c = calls.clone();
        let drops = Rc::new(RefCell::new(Vec::new()));
        let d = drops.clone();
        window.on_displays_drop(move |i, x, y| {
            c.borrow_mut().push(format!("drop {i}"));
            d.borrow_mut().push((x, y));
        });
        let c = calls.clone();
        window.on_displays_act(move |verb, arg, _| c.borrow_mut().push(format!("{verb} {arg}")));

        let m = view(&st).monitors.row_data(3).unwrap();
        let label = format!("{}, {}, {}", m.port, m.name, m.sub);
        let tile = || ElementHandle::find_by_accessible_label(&window, &label).next().unwrap();
        let (start, size) = (tile().absolute_position(), tile().size());
        let at =
            |dx: f32, dy: f32| LogicalPosition::new(start.x + size.width / 2.0 + dx, start.y + size.height / 2.0 + dy);
        let send = |e: WindowEvent| window.window().dispatch_event(e);
        let button = PointerEventButton::Left;

        // A click that wobbles a little.
        send(WindowEvent::PointerPressed { position: at(0.0, 0.0), button });
        send(WindowEvent::PointerMoved { position: at(3.0, -2.0) });
        assert_eq!(tile().absolute_position(), start);
        send(WindowEvent::PointerReleased { position: at(3.0, -2.0), button });
        assert_eq!(*calls.borrow(), ["select 3"]);
        calls.borrow_mut().clear();

        // A drag.
        send(WindowEvent::PointerPressed { position: at(0.0, 0.0), button });
        for step in 1..=10 {
            let d = step as f32 * 5.0;
            send(WindowEvent::PointerMoved { position: at(d, -d) });
            let p = tile().absolute_position();
            assert!((p.x - (start.x + d)).abs() <= 1.0 && (p.y - (start.y - d)).abs() <= 1.0, "{step}: {p:?}");
        }
        // (The press selects again: this test doesn't render the selection.)
        assert_eq!(calls.borrow()[0], "select 3");
        assert_eq!(calls.borrow()[1..], ["snap 3"; 10], "one per move, and nothing else");
        send(WindowEvent::PointerReleased { position: at(50.0, -50.0), button });
        assert_eq!(calls.borrow().last().unwrap(), "drop 3");
        assert_eq!(calls.borrow().iter().filter(|c| c.starts_with("drop")).count(), 1);
        // 50 px on the screen, in layout pixels (the tile is a 6 px gap smaller than the monitor),
        // give or take a pixel's rounding.
        let k = (size.width + 6.0) / m.w;
        let (x, y) = drops.borrow()[0];
        assert!((x - (m.x + 50.0 / k)).abs() < 1.5 / k && (y - (m.y - 50.0 / k)).abs() < 1.5 / k, "{x}, {y}");
        // Back where the model has it, which the drop (not called here) would have changed.
        assert_eq!(tile().absolute_position(), start);
    }

    /// A field hands over what was typed on Enter, and when Tab takes the focus away; not on
    /// every key, and not when nothing was typed.
    #[test]
    fn position_fields_commit_on_enter_and_on_leaving() {
        use i_slint_backend_testing::ElementHandle;
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};
        use std::cell::RefCell;
        use std::rc::Rc;
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(1200.0, 760.0));
        window.set_page("displays".into());
        render(&window, &state());
        window.show().unwrap();
        let calls = Rc::new(RefCell::new(Vec::<String>::new()));
        let c = calls.clone();
        window.on_displays_act(move |verb, arg, _| c.borrow_mut().push(format!("{verb} {arg}")));
        let send = |e: WindowEvent| window.window().dispatch_event(e);
        let key = |k: SharedString| {
            send(WindowEvent::KeyPressed { text: k.clone() });
            send(WindowEvent::KeyReleased { text: k });
        };
        // The field, not the "X" beside it.
        let field = |label: &str| {
            let e = ElementHandle::find_by_accessible_label(&window, label)
                .find(|e| e.accessible_value().is_some())
                .unwrap();
            let (p, s) = (e.absolute_position(), e.size());
            LogicalPosition::new(p.x + s.width / 2.0, p.y + s.height / 2.0)
        };
        let retype = |label: &str, text: &str| {
            let at = field(label);
            send(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
            send(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
            key(Key::End.into());
            for _ in 0..6 {
                key(Key::Backspace.into());
            }
            for c in text.chars() {
                key(c.to_string().into());
            }
        };

        // The focus leaving shows on the next turn of the event loop.
        let next_turn = || i_slint_backend_testing::mock_elapsed_time(std::time::Duration::ZERO);
        retype("X", "1300");
        next_turn();
        assert!(calls.borrow().is_empty(), "{:?}", calls.borrow());
        key(Key::Tab.into());
        next_turn();
        assert_eq!(*calls.borrow(), ["x 1300"]);
        // Tab on from Y, with nothing typed: nothing to hand over.
        key(Key::Tab.into());
        next_turn();
        assert_eq!(calls.borrow().len(), 1);
        retype("Y", "2000");
        key(Key::Return.into());
        assert_eq!(calls.borrow()[1..], ["y 2000"]);
    }

    #[test]
    fn resolution_picks_the_fastest_rate_and_a_scale_that_fits() {
        let mut st = state();
        let w = weak();
        act(&w, &mut st, "select", "2", 0.0);
        act(&w, &mut st, "resolution", "2", 0.0); // 1920 × 1080
        assert_eq!(st.edit[2].mode, Mode { width: 1920, height: 1080, refresh: 240_000 });
        assert!(scale_is_valid(1920, 1080, st.edit[2].scale));
        act(&w, &mut st, "mirror", "1", 0.0);
        assert_eq!(st.edit[2].mirror.as_deref(), Some("desc:Samsung Electric Company Odyssey G61SH HNAL600836"));
        act(&w, &mut st, "colour", "2", 0.0);
        act(&w, &mut st, "bitdepth", "1", 0.0);
        assert!(st.edit[2].lua_live().contains(r#"bitdepth = 10, cm = "hdr""#));
    }

    #[test]
    fn turning_monitors_off_comes_last() {
        let st = state();
        let mut to = st.live.clone();
        to[0].disabled = true;
        to[1].scale = 2.0;
        let order: Vec<bool> = changes(&st.live, &to).iter().map(|r| r.disabled).collect();
        assert_eq!(order, [false, true]);
    }

    #[test]
    fn workspaces_bind_with_one_default_per_monitor() {
        let mut st = state();
        let w = weak();
        act(&w, &mut st, "ws-monitor", "0", 3.0); // workspace 1 on the G8 (third in the list)
        act(&w, &mut st, "ws-monitor", "1", 3.0);
        act(&w, &mut st, "ws-default", "0", 0.0);
        act(&w, &mut st, "ws-default", "1", 0.0);
        let g8 = st.monitors[2].selector();
        let defaults: Vec<(&str, bool)> = st.ws_edit.iter().map(|w| (w.workspace.as_str(), w.default)).collect();
        assert_eq!(defaults, [("1", false), ("2", true)]);
        assert!(st.ws_edit.iter().all(|w| w.monitor == g8));
        let v = view(&st);
        assert_eq!(v.status, "2 changes not applied");
        assert!(v.can_apply);
        assert_eq!(v.monitors.row_data(2).unwrap().workspaces, "1  2★");
        act(&w, &mut st, "ws-monitor", "0", 0.0);
        assert_eq!(st.ws_edit.len(), 1);
    }

    #[test]
    fn a_profile_for_exactly_these_monitors_switches_by_itself() {
        let mut st = state();
        let mut desk: Vec<MonitorRule> = st.live.clone();
        desk[1].disabled = true; // the TV off
        st.store.switch_on_hotplug = true;
        st.store.profiles.push(Profile {
            name: "Desk".into(),
            auto: true,
            monitors: desk.iter().map(StoredMonitor::from).collect(),
            ..Profile::default()
        });
        // The real displays.toml mustn't be touched: no file, no Hyprland here.
        let profile = st.store.matching(&st.monitors.iter().map(Monitor::selector).collect::<Vec<_>>()).cloned();
        assert_eq!(profile.as_ref().map(|p| p.name.as_str()), Some("Desk"));
        assert_eq!(profile_rules(&st, &profile.unwrap()), desk);
        assert_eq!(st.store.active(&desk), Some(0));
        assert_eq!(st.store.active(&st.live), None);
    }

    #[test]
    fn updating_or_deleting_a_profile_asks_first() {
        let mut st = state();
        let w = weak();
        let mut desk = st.live.clone();
        desk[1].disabled = true;
        let desk: Vec<StoredMonitor> = desk.iter().map(StoredMonitor::from).collect();
        st.store.profiles.push(Profile { name: "Desk".into(), monitors: desk, ..Profile::default() });
        st.store.profiles.push(Profile { name: "TV".into(), ..Profile::default() });
        let before = st.store.profiles.clone();

        // Asking changes nothing, so Cancel and Esc only have to close the dialog.
        let delete = act(&w, &mut st, "profile-delete", "0", 0.0).unwrap();
        assert_eq!(delete.title, "Delete Desk?");
        assert_eq!(delete.then, Confirmed::Displays("profile-delete", "Desk".into()));
        let update = act(&w, &mut st, "profile-update", "0", 0.0).unwrap();
        assert_eq!(update.title, "Update Desk?");
        assert_eq!(update.button, "Update");
        // Only Delete loses something: Update's button isn't red.
        assert!(delete.destructive && !update.destructive);
        assert_eq!(st.store.profiles, before);
        assert_eq!(act(&w, &mut st, "profile-delete", "5", 0.0), None);

        // Accepted, by name: the list may have changed while the dialog was open.
        assert!(change_profile(&mut st, "profile-update", "Desk"));
        let live: Vec<StoredMonitor> = st.live.iter().map(StoredMonitor::from).collect();
        assert_eq!(st.store.profiles[0].monitors, live);
        st.store.profiles.swap(0, 1);
        assert!(change_profile(&mut st, "profile-delete", "Desk"));
        let names: Vec<&str> = st.store.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["TV"]);
        assert!(!change_profile(&mut st, "profile-delete", "Desk"));
    }

    #[test]
    fn nothing_to_save_before_lituus_wrote_its_file() {
        let (st, dir) = state_in("first");
        let v = view(&st);
        assert_eq!(v.status, "No changes");
        assert!(!v.can_save && !v.can_apply && !v.can_revert);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_made_outside_lituus_isnt_an_edit() {
        let mut st = state();
        let mut moved = fixture();
        moved[2].x += 100; // DP-4
        reread(&mut st, &moved);
        assert_eq!(st.edit, st.live);
        assert_eq!(st.edit[3].x, 6438);
        // An edit in progress stays.
        act(&weak(), &mut st, "select", "3", 0.0);
        act(&weak(), &mut st, "x", "7000", 0.0);
        reread(&mut st, &moved);
        assert_eq!(st.edit[3].x, 7000);
    }

    #[test]
    fn a_file_with_older_settings_can_be_saved() {
        let (mut st, dir) = state_in("older");
        std::fs::write(dir.join("lituus.lua"), "-- Written by Lituus (Displays).\n").unwrap();
        reread(&mut st, &fixture());
        let v = view(&st);
        assert_eq!(v.status, "The config file has older settings than the monitors now");
        assert!(v.can_save);
        act(&weak(), &mut st, "save", "", 0.0);
        let v = view(&st);
        assert_eq!(v.status, "Everything is applied and saved");
        assert!(!v.can_save);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apply_then_keep_ends_saved() {
        let (mut st, dir) = state_in("keep");
        let w = weak();
        // DP-4 from 59.95 Hz to its fastest mode, with VRR on.
        act(&w, &mut st, "select", "3", 0.0);
        act(&w, &mut st, "rate", "0", 0.0);
        act(&w, &mut st, "vrr", "2", 0.0);
        assert_eq!(view(&st).status, "1 change not applied");
        act(&w, &mut st, "apply", "", 0.0);
        assert!(st.keep_pending());
        // Hyprland reports the mode it listed as 239.76 Hz as 239.757, and not the VRR setting.
        let mut after = fixture();
        after[2].refresh_rate = 239.757;
        reread(&mut st, &after);
        act(&w, &mut st, "keep", "", 0.0);
        reread(&mut st, &after);
        let v = view(&st);
        assert_eq!(v.status, "Everything is applied and saved");
        assert!(!v.can_apply && !v.can_save);
        assert_eq!(st.live[3].vrr, Some(1));
        // After a restart, the VRR setting comes from the file.
        let mut again = State { hypr_dir: st.hypr_dir.clone(), store_path: st.store_path.clone(), ..State::default() };
        reread(&mut again, &after);
        assert_eq!(again.live[3].vrr, Some(1));
        assert_eq!(view(&again).status, "Everything is applied and saved");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn monitors_can_be_named_by_port() {
        let (mut st, dir) = state_in("names");
        let w = weak();
        // The G8 mirrors DP-3, and workspace 1 opens on the G8.
        let mut mirrored = fixture();
        mirrored[1].mirror_of = "0".into();
        reread(&mut st, &mirrored);
        act(&w, &mut st, "ws-monitor", "0", 3.0);
        act(&w, &mut st, "keep", "", 0.0);
        act(&w, &mut st, "profile-save", "Desk", 0.0);
        let text = st.text(&st.edit);
        assert!(text.contains(r#"output = "desc:Samsung Electric Company Odyssey G61SH HNAL600836""#), "{text}");
        assert!(text.contains(r#"mirror = "desc:Samsung Electric Company Odyssey G61SH HNAL600836""#), "{text}");
        assert!(text.contains(r#"monitor = "desc:Samsung Electric Company Odyssey G81SF HNAY500052""#), "{text}");
        assert_eq!(view(&st).status, "Everything is applied and saved");

        // It waits for Apply, like everything on the page.
        act(&w, &mut st, "name-by-description", "", 0.0);
        let v = view(&st);
        assert!(!v.by_description);
        assert_eq!(v.status, "1 change not applied");
        assert!(st.text(&st.edit).contains("desc:"));
        // Apply has nothing for Hyprland to try, so nothing to keep: it writes the file.
        act(&w, &mut st, "apply", "", 0.0);
        assert!(!st.keep_pending());
        let text = st.text(&st.edit);
        assert!(!text.contains("desc:"), "{text}");
        assert!(text.contains(r#"output = "DP-3""#) && text.contains(r#"mirror = "DP-3""#), "{text}");
        assert!(text.contains(r#"hl.workspace_rule({ workspace = "1", monitor = "DP-5" })"#), "{text}");
        assert!(text.contains("-- Profile: Desk"), "{text}");
        assert_eq!(std::fs::read_to_string(dir.join("lituus.lua")).unwrap(), text);
        assert_eq!(view(&st).status, "Everything is applied and saved");
        // Profiles still match the monitors, and the setting is remembered.
        let connected: Vec<String> = st.monitors.iter().map(Monitor::selector).collect();
        assert_eq!(st.store.matching(&connected).map(|p| p.name.as_str()), Some("Desk"));
        assert_eq!(st.store.active(&st.live), Some(0));
        assert!(!profiles::load(&dir.join("displays.toml")).name_by_description);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// "Switch when monitors are plugged in", "Switch by itself" and "Run after switching" wait
    /// for Apply too; Revert drops them.
    #[test]
    fn profile_options_wait_for_apply() {
        let (mut st, dir) = state_in("options");
        let w = weak();
        // On when there's no displays.toml yet.
        assert!(st.store.switch_on_hotplug);
        act(&w, &mut st, "profile-save", "Desk", 0.0);
        act(&w, &mut st, "switch-hotplug", "", 0.0);
        act(&w, &mut st, "profile-auto", "0", 0.0);
        act(&w, &mut st, "profile-exec", "0:pkill -SIGUSR2 waybar", 0.0);
        let v = view(&st);
        assert!(!v.switch_hotplug);
        let row = v.profiles.row_data(0).unwrap();
        assert!(!row.auto);
        assert_eq!((row.exec.as_str(), row.exec_hint.as_str()), ("pkill -SIGUSR2 waybar", "Not applied yet"));
        assert_eq!(v.status, "2 changes not applied");
        assert!(v.can_apply && v.can_revert);
        // Nothing saved yet.
        let saved = profiles::load(&dir.join("displays.toml"));
        assert!(saved.switch_on_hotplug && saved.profiles[0].auto && saved.profiles[0].exec.is_empty());
        act(&w, &mut st, "revert", "", 0.0);
        assert!(view(&st).switch_hotplug);
        assert!(view(&st).profiles.row_data(0).unwrap().auto);

        // Applied: saved, with no Keep to wait for.
        act(&w, &mut st, "switch-hotplug", "", 0.0);
        act(&w, &mut st, "profile-exec", "0:notify-send Desk", 0.0);
        act(&w, &mut st, "apply", "", 0.0);
        assert!(!st.keep_pending());
        let saved = profiles::load(&dir.join("displays.toml"));
        assert!(!saved.switch_on_hotplug);
        assert_eq!(saved.profiles[0].exec, "notify-send Desk");
        let v = view(&st);
        assert_eq!(v.status, "Everything is applied and saved");
        assert_eq!(v.profiles.row_data(0).unwrap().exec_hint, "");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Saving writes the Displays part only: the Input part and the user's lines before the
    /// first part stay, in Lua and in hyprlang.
    #[test]
    fn saving_keeps_the_input_part() {
        for lua in [true, false] {
            let (mut st, dir) = state_in(if lua { "input-lua" } else { "input-conf" });
            std::fs::write(dir.join("hyprland.conf"), "source = ~/.config/hypr/lituus.conf\n").unwrap();
            let file = ConfigFile::new(&dir, lua);
            let comment = if lua { "--" } else { "#" };
            let input = lituus_hypr::InputSettings::default().file(lua);
            let old = format!("{comment} Written by Lituus (Displays).\n");
            std::fs::write(file.path(), format!("{comment} Mine.\n\n{old}\n{input}")).unwrap();
            loaded(&mut st, Ok((Hyprland::at("/nonexistent"), lua, fixture())));
            assert_eq!(st.saved.as_deref(), Some(old.as_str()));
            assert_eq!(view(&st).status, "The config file has older settings than the monitors now");
            act(&weak(), &mut st, "save", "", 0.0);
            let text = st.text(&st.live);
            assert!(text.contains(if lua { "hl.monitor(" } else { "monitor = " }), "{text}");
            assert_eq!(file.read().unwrap(), format!("{comment} Mine.\n\n{text}\n{input}"));
            assert_eq!(view(&st).status, "Everything is applied and saved");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// The Input part isn't older Displays settings.
    #[test]
    fn an_input_part_doesnt_make_the_file_older() {
        let (mut st, dir) = state_in("input-after");
        act(&weak(), &mut st, "save", "", 0.0);
        let file = ConfigFile::new(&dir, true);
        file.write_section("Input", &lituus_hypr::InputSettings::default().file(true)).unwrap();
        reread(&mut st, &fixture());
        let v = view(&st);
        assert_eq!(v.status, "Everything is applied and saved");
        assert!(!v.can_save);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Switching to a profile on hotplug writes the Displays part only.
    #[test]
    fn switching_on_hotplug_keeps_the_input_part() {
        let (mut st, dir) = state_in("input-hotplug");
        let file = ConfigFile::new(&dir, true);
        let input = lituus_hypr::InputSettings::default().file(true);
        file.write_section("Input", &input).unwrap();
        act(&weak(), &mut st, "profile-save", "Desk", 0.0);
        // DP-4 moved outside Lituus; the profile puts it back.
        let mut moved = fixture();
        moved[2].x += 100;
        reread(&mut st, &moved);
        auto_switch(&weak(), &mut st);
        let text = file.read().unwrap();
        assert!(text.contains(r#"position = "6338x2393""#), "{text}");
        assert_eq!(lituus_hypr::section(&text, "Input").as_deref(), Some(input.as_str()));
        assert_eq!(lituus_hypr::section(&text, "Displays"), st.saved);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With Hyprland's config not loading Lituus's file, the page doesn't say it's saved.
    #[test]
    fn applied_but_not_loaded_isnt_saved() {
        let (mut st, dir) = state_in("unloaded");
        std::fs::remove_file(dir.join("hyprland.lua")).unwrap();
        act(&weak(), &mut st, "save", "", 0.0);
        reread(&mut st, &fixture());
        let v = view(&st);
        assert!(v.not_loaded);
        assert_eq!(v.status, "Applied; lost when Hyprland restarts");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_hyprland() {
        let mut st = State { store_read: true, ..State::default() };
        loaded(&mut st, Err(lituus_hypr::Error::NotRunning));
        assert_eq!(view(&st).state, "not-running");
        assert_eq!(status(&st), "Not running");
    }
}
