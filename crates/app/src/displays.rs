//! The Displays page: Hyprland's monitors (lituus-hypr), edited on a copy of their rules; Apply
//! sets them live and asks to keep them, and only then writes ~/.config/hypr/lituus.lua.
//! Without an answer within 15 seconds the old settings come back, so a black screen fixes
//! itself.

use std::time::{Duration, Instant};

use lituus_hypr::{ConfigFile, Hyprland, Mode, Monitor, MonitorRule, config_dir, scale_is_valid, valid_scales};
use slint::{Model, ModelRc, SharedString, VecModel};

use crate::{DisplaysView, LayoutMonitor, MainWindow, update};

/// How long Apply waits for Keep.
pub const KEEP_FOR: Duration = Duration::from_secs(15);
/// How close an edge snaps to another monitor's, in layout pixels.
const SNAP: i32 = 64;

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
    /// The file as last written or read.
    saved: Option<String>,
}

impl State {
    fn lua(&self) -> bool {
        self.file.as_ref().is_none_or(|f| f.lua)
    }

    /// The file's text for these rules.
    fn text(&self, rules: &[MonitorRule]) -> String {
        lituus_hypr::file(self.lua(), None, rules, &[])
    }

    /// Apply is waiting for Keep.
    pub fn keep_pending(&self) -> bool {
        self.keep.is_some()
    }

    fn dirty(&self) -> bool {
        self.edit != self.live
    }
}

/// Reads Hyprland's monitors (on a thread: the socket may take a moment).
pub fn refresh(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    std::thread::spawn(move || {
        let read = (|| {
            let h = Hyprland::from_env()?;
            let lua = h.version().map(|v| v.lua()).unwrap_or(true);
            let monitors = h.monitors()?;
            Ok::<_, lituus_hypr::Error>((h, lua, monitors))
        })();
        let w = weak.clone();
        let _ = weak.upgrade_in_event_loop(move |_| update(&w, move |app| loaded(&mut app.displays, read)));
    });
}

fn loaded(state: &mut State, read: Result<(Hyprland, bool, Vec<Monitor>), lituus_hypr::Error>) {
    match read {
        Ok((h, lua, mut monitors)) => {
            monitors.sort_by_key(|m| (m.disabled, m.x, m.y));
            let file = ConfigFile::new(config_dir(), lua);
            state.loaded = file.loaded();
            state.saved = file.read();
            state.file = Some(file);
            state.hypr = Some(h);
            state.not_running = false;
            state.live = monitors.iter().map(|m| MonitorRule::from_monitor(m, &monitors)).collect();
            // Keep edits in progress across a refresh, unless the monitors changed.
            let same = state.edit.len() == state.live.len()
                && state.edit.iter().zip(&state.live).all(|(a, b)| a.output == b.output);
            if !same || state.keep.is_none() && !state.dirty() {
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
fn send(weak: &slint::Weak<MainWindow>, state: &State, rules: Vec<MonitorRule>) {
    let Some(h) = state.hypr.clone() else { return };
    let lua = state.lua();
    let weak = weak.clone();
    std::thread::spawn(move || {
        let mut error = None;
        for r in &rules {
            let result = if lua { h.eval(&r.lua_live()) } else { h.keyword("monitor", &r.hyprlang()) };
            if let Err(e) = result {
                error = Some(format!("Hyprland didn't take the settings for {}: {e}", r.output));
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

/// Everything the page does: (verb, argument, value).
pub fn act(weak: &slint::Weak<MainWindow>, state: &mut State, verb: &str, arg: &str, value: f32) {
    let i = state.selected;
    let index = arg.parse::<usize>().unwrap_or(0);
    match verb {
        "select" => state.selected = index.min(state.monitors.len().saturating_sub(1)),
        "apply" if state.dirty() && problem(state).is_none() => {
            let rules = changes(&state.live, &state.edit);
            state.keep = Some((Instant::now() + KEEP_FOR, state.live.clone()));
            send(weak, state, rules);
        }
        "keep" => {
            state.keep = None;
            save(state);
        }
        "revert-now" => revert(weak, state),
        "revert" => state.edit = state.live.clone(),
        "save" => save(state),
        "view-config" => state.config_open = true,
        "close-config" => state.config_open = false,
        _ => {
            let Some(rule) = state.edit.get_mut(i) else { return };
            let Some(monitor) = state.monitors.get(i) else { return };
            edit(rule, monitor, &state.monitors, verb, arg, index, value);
        }
    }
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
        send(weak, state, rules);
    }
}

fn save(state: &mut State) {
    let Some(file) = &state.file else { return };
    let text = state.text(&state.live);
    match file.write(&text) {
        Ok(()) => state.saved = Some(text),
        Err(e) => state.error = Some(format!("Couldn't write {}: {e}", file.path().display())),
    }
}

fn edit(rule: &mut MonitorRule, monitor: &Monitor, all: &[Monitor], verb: &str, arg: &str, index: usize, value: f32) {
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
                    rule.scale = nearest_scale(w, h, rule.scale);
                }
            }
        }
        "rate" => {
            if let Some(m) = rates(&modes, rule.mode).get(index) {
                rule.mode = *m;
            }
        }
        "scale" => rule.scale = f64::from(value),
        "scale-step" => {
            // Steps of 0.05 and thirds, the scales people use; Hyprland takes many odd ones too.
            let nice =
                |s: &f64| ((s * 20.0).round() - s * 20.0).abs() < 1e-6 || ((s * 3.0).round() - s * 3.0).abs() < 1e-6;
            let scales: Vec<f64> = valid_scales(rule.mode.width, rule.mode.height).into_iter().filter(nice).collect();
            let next = if value > 0.0 {
                scales.iter().copied().find(|s| *s > rule.scale + 1e-6)
            } else {
                scales.iter().rev().copied().find(|s| *s < rule.scale - 1e-6)
            };
            if let Some(s) = next {
                rule.scale = s;
            }
        }
        "rotation" => rule.transform = (rule.transform & 4) | (index as u8 & 3),
        "flip" => rule.transform ^= 4,
        "x" | "y" => {
            if let Ok(v) = arg.trim().parse::<i32>() {
                if verb == "x" { rule.x = v } else { rule.y = v }
            }
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
            if let (Ok(side), Ok(v)) = (side.parse::<usize>(), text.trim().parse::<i32>())
                && side < 4
            {
                rule.reserved[side] = v.max(0);
            }
        }
        _ => {}
    }
}

/// Moves a monitor on the layout: (x, y) in layout pixels; `done` when it's dropped, which snaps
/// it to the other monitors' edges and centres.
pub fn drag(state: &mut State, index: usize, x: f32, y: f32, done: bool) {
    let others: Vec<(i32, i32, i32, i32)> = state
        .edit
        .iter()
        .enumerate()
        .filter(|(j, r)| *j != index && !r.disabled && r.mirror.is_none())
        .map(|(_, r)| {
            let (w, h) = r.logical_size();
            (r.x, r.y, w, h)
        })
        .collect();
    let Some(rule) = state.edit.get_mut(index) else { return };
    let (w, h) = rule.logical_size();
    let (mut x, mut y) = (x.round() as i32, y.round() as i32);
    if done {
        x = snap(x, w, others.iter().map(|o| (o.0, o.2)));
        y = snap(y, h, others.iter().map(|o| (o.1, o.3)));
    }
    rule.x = x;
    rule.y = y;
    if done {
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
}

/// The start or end that lines up with another monitor's edge or centre, if one is near.
fn snap(pos: i32, len: i32, others: impl Iterator<Item = (i32, i32)>) -> i32 {
    let mut best: Option<(i32, i32)> = None;
    for (o, olen) in others {
        for candidate in [o, o + olen, o - len, o + olen - len, o + (olen - len) / 2] {
            let d = (candidate - pos).abs();
            if d <= SNAP && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((candidate, d));
            }
        }
    }
    best.map_or(pos, |(c, _)| c)
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

fn nearest_scale(w: u32, h: u32, scale: f64) -> f64 {
    valid_scales(w, h).into_iter().min_by(|a, b| (a - scale).abs().total_cmp(&(b - scale).abs())).unwrap_or(1.0)
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
        return "—".to_owned();
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
        v.sub = s("Hyprland isn't running");
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
    v.sub = s(match (state.monitors.len(), on) {
        (1, _) => "1 monitor".to_owned(),
        (n, m) if m == n => format!("{n} monitors"),
        (n, m) => format!("{n} monitors · {m} on"),
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
                format!("{} × {} · {} Hz · {}×", r.mode.width, r.mode.height, r.mode.refresh_text(), short(r.scale))
            }),
            x: r.x as f32,
            y: r.y as f32,
            w: w as f32,
            h: h as f32,
            selected: i == state.selected,
            disabled: r.disabled,
            mirrored,
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
    v.logical = s(format!("Layout pixels · {} × {} · drag to move; edges snap", bx2 - bx, by2 - by));
    v.monitors = ModelRc::new(VecModel::from(layout));

    // The selected monitor.
    let i = state.selected;
    let (Some(r), Some(m)) = (state.edit.get(i), state.monitors.get(i)) else { return v };
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
    v.scale_text = s(short(r.scale));
    v.scale_valid = scale_is_valid(r.mode.width, r.mode.height, r.scale);
    v.scale_hint = s(if v.scale_valid {
        let (w, h) = r.logical_size();
        format!("Looks like {w} × {h}")
    } else {
        format!("Not a scale Hyprland takes; {} is", short(nearest_scale(r.mode.width, r.mode.height, r.scale)))
    });
    let chips: Vec<f64> =
        [1.0, 1.25, 1.5, 2.0].into_iter().filter(|&c| scale_is_valid(r.mode.width, r.mode.height, c)).collect();
    v.scale_chips = strings(chips.iter().map(|c| short(*c)));
    v.scale_chip = chips.iter().position(|c| (c - r.scale).abs() < 1e-6).map_or(-1, |p| p as i32);
    v.rotation = i32::from(r.transform & 3);
    v.flipped = r.transform & 4 != 0;
    v.x_text = s(r.x.to_string());
    v.y_text = s(r.y.to_string());
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
    v.reserved = strings(r.reserved.iter().map(|x| x.to_string()));

    // The bar at the bottom, and Apply's dialog.
    let problem = problem(state);
    let changed = state.edit.iter().zip(&state.live).filter(|(a, b)| a != b).count();
    let saved = state.saved.as_deref() == Some(state.text(&state.live).as_str());
    let (status, tone) = if let Some(e) = &state.error {
        (e.clone(), "danger")
    } else if let Some(p) = &problem {
        (format!("{p}, so Apply is off"), "danger")
    } else if state.keep.is_some() {
        ("Applied; waiting for you to keep it".to_owned(), "accent")
    } else if changed > 0 {
        (if changed == 1 { "1 monitor changed".to_owned() } else { format!("{changed} monitors changed") }, "accent")
    } else if saved {
        ("Everything is applied and saved".to_owned(), "muted")
    } else {
        ("Applied live only, not saved yet".to_owned(), "warn")
    };
    v.status = s(status);
    v.status_tone = s(tone);
    v.can_apply = changed > 0 && problem.is_none() && state.keep.is_none();
    v.can_revert = changed > 0 && state.keep.is_none();
    v.can_save = changed == 0 && !saved && state.keep.is_none();
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
    v.config_text = s(state.text(&state.edit));
    v
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
    /// layout mid-drag, and the drop would land somewhere else.
    static LAYOUT: std::rc::Rc<VecModel<LayoutMonitor>> = std::rc::Rc::new(VecModel::default());
}

pub fn render(window: &MainWindow, state: &State) {
    let mut v = view(state);
    let rows: Vec<LayoutMonitor> = v.monitors.iter().collect();
    LAYOUT.with(|model| {
        if model.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if model.row_data(i).as_ref() != Some(&row) {
                    model.set_row_data(i, row);
                }
            }
        } else {
            model.set_vec(rows);
        }
        v.monitors = ModelRc::from(model.clone());
    });
    window.set_displays(v);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        let monitors: Vec<Monitor> =
            serde_json::from_str(include_str!("../../hypr/tests/fixtures/monitors.json")).unwrap();
        let mut st = State::default();
        loaded(&mut st, Ok((Hyprland::at("/nonexistent"), true, monitors)));
        st
    }

    fn weak() -> slint::Weak<MainWindow> {
        slint::Weak::default()
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
        assert_eq!(v.monitors.row_data(2).unwrap().sub, "3840 × 2160 · 240 Hz · 1.5×");
    }

    #[test]
    fn edits_wait_for_apply() {
        let mut st = state();
        let w = weak();
        act(&w, &mut st, "select", "2", 0.0);
        assert_eq!(view(&st).sel_title, "Samsung Odyssey G81SF");
        act(&w, &mut st, "scale-step", "", 1.0);
        // 1.6 is the next scale up that 3840 × 2160 divides into whole pixels.
        assert!((st.edit[2].scale - 1.6).abs() < 1e-9);
        let v = view(&st);
        assert!(v.can_apply && v.can_revert);
        assert_eq!(v.status, "1 monitor changed");
        assert_eq!(v.scale_hint, "Looks like 2400 × 1350");
        // Down from 1.5 is 4/3, which makes the G8 bigger than the gap to DP-4.
        act(&w, &mut st, "scale-step", "", -1.0);
        act(&w, &mut st, "scale-step", "", -1.0);
        assert!((st.edit[2].scale - 4.0 / 3.0).abs() < 1e-9);
        assert!(view(&st).status.contains("overlap"));
        act(&w, &mut st, "revert", "", 0.0);
        assert_eq!(st.edit, st.live);
    }

    #[test]
    fn a_bad_scale_or_an_overlap_keeps_apply_off() {
        let mut st = state();
        let w = weak();
        act(&w, &mut st, "select", "3", 0.0);
        act(&w, &mut st, "scale", "", 1.3);
        assert!(view(&st).status.contains("can't use scale 1.3"));
        assert!(!view(&st).can_apply);
        act(&w, &mut st, "revert", "", 0.0);
        // DP-4 dragged onto the G8.
        drag(&mut st, 3, 4000.0, 2400.0, false);
        assert!(view(&st).status.contains("overlap"), "{}", view(&st).status);
    }

    #[test]
    fn dropping_snaps_to_an_edge() {
        let mut st = state();
        // DP-4 dropped 30 px right of the G8's right edge, and a little low.
        let g8_right = st.edit[2].x + st.edit[2].logical_size().0;
        drag(&mut st, 3, (g8_right + 30) as f32, 2420.0, true);
        assert_eq!((st.edit[3].x, st.edit[3].y), (g8_right, 2393));
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
    fn no_hyprland() {
        let mut st = State::default();
        loaded(&mut st, Err(lituus_hypr::Error::NotRunning));
        assert_eq!(view(&st).state, "not-running");
        assert_eq!(status(&st), "—");
    }
}
