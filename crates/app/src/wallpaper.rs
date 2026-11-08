//! The Wallpaper page (Desktop › Wallpaper), following the "Wallpaper" mockups: the monitors and
//! the pictures they show in the hero, a folder's pictures in a grid (a click sets one), and one
//! card of settings beside it: which monitors a click sets, how the picture fills them, awww's
//! transition, and the program (awww or hyprpaper, lituus-wallpaper).
//!
//! awww's daemon shows the last pictures again by itself after a restart; hyprpaper forgets what
//! it's told, so Lituus also writes hyprpaper.conf, asking once when there is one already and
//! keeping it as hyprpaper.conf.bak.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lituus_wallpaper::{Env, Fill, Folder, HyprpaperEntry, Program, Set, Status, TRANSITIONS};
use slint::{Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::config::{self, WallpaperConfig};
use crate::{Hint, MainWindow, WallpaperMonitor, WallpaperTile, WallpaperView, update};

/// The thumbnails' size: 16:9, twice what a tile shows at 2000 px.
const THUMB: (u32, u32) = (384, 216);

/// Lituus's mark in hyprpaper.conf: the file is Lituus's to write (asked once).
const MARK: &str = "# Lituus writes the wallpaper blocks in this file; the rest of it is yours.";

/// A monitor, as the hero draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub model: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
}

#[derive(Default)]
pub struct State {
    /// Read at least once.
    read: bool,
    status: Status,
    /// The picture on each monitor.
    current: Vec<(String, PathBuf)>,
    monitors: Vec<Monitor>,
    /// Hyprland's config is Lua (for the autostart line).
    lua: bool,
    settings: WallpaperConfig,
    folder: Option<Folder>,
    /// Pictures' sizes, read with their thumbnails.
    sizes: HashMap<PathBuf, (u32, u32)>,
    pub query: String,
    /// What a click sets: 0 all monitors, else that monitor (1-based).
    target: usize,
    /// A picture waiting for the answer to taking over hyprpaper.conf.
    pub takeover: Option<Set>,
    message: Option<String>,
    /// Where things are, if not the usual places (tests).
    env: Option<Env>,
    config_path: Option<PathBuf>,
    hypr_dir: Option<PathBuf>,
    home: Option<PathBuf>,
}

impl State {
    fn env(&self) -> Env {
        self.env.clone().unwrap_or_else(Env::from_env)
    }

    fn config_path(&self) -> PathBuf {
        self.config_path.clone().unwrap_or_else(config::path)
    }

    fn hypr_dir(&self) -> PathBuf {
        self.hypr_dir.clone().unwrap_or_else(lituus_hypr::config_dir)
    }

    fn home(&self) -> PathBuf {
        self.home.clone().or_else(|| std::env::var_os("HOME").map(PathBuf::from)).unwrap_or_default()
    }

    /// The folder shown: the one chosen, else ~/Pictures/Wallpapers, else ~/Pictures.
    fn folder_path(&self) -> PathBuf {
        if !self.settings.folder.is_empty() {
            return expand(&self.settings.folder, &self.home());
        }
        let pictures = self.home().join("Pictures");
        let wallpapers = pictures.join("Wallpapers");
        if wallpapers.is_dir() { wallpapers } else { pictures }
    }

    fn program(&self) -> Option<Program> {
        self.status.running
    }

    fn fill(&self) -> Fill {
        let fill = Fill::from_id(&self.settings.fill);
        match self.program() {
            Some(p) if !p.fills().contains(&fill) => Fill::Cover,
            _ => fill,
        }
    }

    /// The monitors a click sets: their names, none for all.
    fn targets(&self) -> Vec<String> {
        match self.target.checked_sub(1).and_then(|i| self.monitors.get(i)) {
            Some(m) => vec![m.name.clone()],
            None => Vec::new(),
        }
    }
}

fn expand(path: &str, home: &Path) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_path_buf(),
        None => PathBuf::from(path),
    }
}

fn shorten(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// What was read, on a thread.
pub struct Read {
    status: Status,
    current: Vec<(String, PathBuf)>,
    monitors: Vec<Monitor>,
    lua: bool,
}

fn read(env: &Env) -> Read {
    let status = lituus_wallpaper::status(env);
    let current = status.running.and_then(|p| lituus_wallpaper::current(env, p).ok()).unwrap_or_default();
    let (monitors, lua) = match lituus_hypr::Hyprland::from_env() {
        Ok(h) => (
            h.monitors()
                .unwrap_or_default()
                .into_iter()
                .filter(|m| !m.disabled)
                .map(|m| {
                    // Where it is and how large, in the layout's own (logical) pixels.
                    let scale = if m.scale > 0.0 { m.scale } else { 1.0 };
                    let (w, h) = if m.transform % 2 == 1 { (m.height, m.width) } else { (m.width, m.height) };
                    Monitor {
                        model: if m.model.is_empty() { m.description.clone() } else { m.model.clone() },
                        name: m.name,
                        x: m.x,
                        y: m.y,
                        width: (f64::from(w) / scale).round() as u32,
                        height: (f64::from(h) / scale).round() as u32,
                        refresh: m.refresh_rate,
                    }
                })
                .collect(),
            h.version().map(|v| v.lua()).unwrap_or(true),
        ),
        Err(_) => (Vec::new(), true),
    };
    Read { status, current, monitors, lua }
}

/// Reads the programs, the pictures shown and the monitors again, on a thread.
pub fn refresh(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    std::thread::spawn(move || {
        let read = read(&Env::from_env());
        let _ =
            weak.clone().upgrade_in_event_loop(move |_| update(&weak, move |app| loaded(&mut app.wallpapers, read)));
    });
}

pub fn loaded(state: &mut State, read: Read) {
    state.read = true;
    state.status = read.status;
    state.current = read.current;
    state.monitors = read.monitors;
    state.lua = read.lua;
    state.target = state.target.min(state.monitors.len());
}

/// Opens the page: the settings from config.toml, the folder's pictures, and their thumbnails.
pub fn open(weak: &slint::Weak<MainWindow>, state: &mut State) {
    state.settings = config::load(&state.config_path()).wallpaper;
    load_folder(weak, state);
    refresh(weak);
}

/// Reads the folder and makes the thumbnails of its pictures (and of the pictures shown), on a
/// thread; each batch of them shows as it's done.
fn load_folder(weak: &slint::Weak<MainWindow>, state: &mut State) {
    let dir = state.folder_path();
    state.folder = lituus_wallpaper::read_folder(&dir, state.settings.subfolders).ok();
    let mut wanted: Vec<PathBuf> = Vec::new();
    if let Some(f) = &state.folder {
        wanted.extend(f.pictures.iter().map(|p| p.path.clone()));
        wanted.extend(f.subfolders.iter().flat_map(|s| s.first.clone()));
    }
    wanted.extend(state.current.iter().map(|(_, p)| p.clone()));
    thumbnails(weak, wanted);
}

thread_local! {
    /// Thumbnails made so far, by picture.
    static THUMBS: RefCell<HashMap<PathBuf, Image>> = RefCell::new(HashMap::new());
}

/// The cache of thumbnails: ~/.cache/lituus/thumbnails.
fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".cache"))
        .join("lituus/thumbnails")
}

/// A picture's thumbnail (its pixels, made on a thread) and its size, if they could be read.
type Made = (PathBuf, Option<SharedPixelBuffer<Rgba8Pixel>>, Option<(u32, u32)>);

fn thumbnails(weak: &slint::Weak<MainWindow>, wanted: Vec<PathBuf>) {
    let wanted: Vec<PathBuf> = THUMBS.with(|t| wanted.into_iter().filter(|p| !t.borrow().contains_key(p)).collect());
    if wanted.is_empty() {
        return;
    }
    let weak = weak.clone();
    std::thread::spawn(move || {
        let cache = cache_dir();
        for batch in wanted.chunks(8) {
            let done: Vec<Made> = batch
                .iter()
                .map(|p| {
                    let size = lituus_wallpaper::dimensions(p);
                    let pixels = lituus_wallpaper::thumbnail(p, &cache, THUMB.0, THUMB.1).ok().and_then(|t| decode(&t));
                    (p.clone(), pixels, size)
                })
                .collect();
            let w = weak.clone();
            let _ = weak.upgrade_in_event_loop(move |_| {
                update(&w, move |app| {
                    THUMBS.with(|t| {
                        let mut t = t.borrow_mut();
                        for (path, pixels, size) in done {
                            if let Some(size) = size {
                                app.wallpapers.sizes.insert(path.clone(), size);
                            }
                            t.insert(path, pixels.map(Image::from_rgba8).unwrap_or_default());
                        }
                    });
                })
            });
        }
    });
}

/// A thumbnail's pixels, for the window's thread to make an image of.
fn decode(path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let img = image::open(path).ok()?.to_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height()))
}

fn thumb(path: &Path) -> Image {
    THUMBS.with(|t| t.borrow().get(path).cloned().unwrap_or_default())
}

// ---- setting a picture -------------------------------------------------------------------------

/// What setting `path` on the chosen monitors takes.
fn set_for(state: &State, path: PathBuf) -> Set {
    let targets = state.targets();
    // As smooth as the monitors set can show it.
    let fps = state
        .monitors
        .iter()
        .filter(|m| targets.is_empty() || targets.contains(&m.name))
        .map(|m| m.refresh.round() as u32)
        .max()
        .unwrap_or(0);
    Set {
        path,
        monitors: targets,
        fill: state.fill(),
        transition: if state.program() == Some(Program::Awww) {
            state.settings.transition.clone()
        } else {
            String::new()
        },
        duration: state.settings.duration as f32,
        fps,
        bar_colour: state.settings.bar_colour.clone(),
    }
}

/// hyprpaper.conf has blocks Lituus didn't write: it asks before writing them.
fn needs_takeover(state: &State) -> bool {
    std::fs::read_to_string(lituus_wallpaper::hyprpaper_conf(&state.hypr_dir()))
        .is_ok_and(|text| !text.contains(MARK) && !text.trim().is_empty())
}

/// Sets a picture, at once, on a thread; with hyprpaper, hyprpaper.conf too (after asking once).
fn set_picture(weak: Option<&slint::Weak<MainWindow>>, state: &mut State, set: Set) {
    let Some(program) = state.program() else { return };
    if program == Program::Hyprpaper && needs_takeover(state) {
        state.takeover = Some(set);
        return;
    }
    let env = state.env();
    let monitors: Vec<String> = state.monitors.iter().map(|m| m.name.clone()).collect();
    let conf = (program == Program::Hyprpaper).then(|| {
        // Every monitor set gets its block; the others keep theirs.
        let names = if set.monitors.is_empty() { monitors.clone() } else { set.monitors.clone() };
        let entries: Vec<HyprpaperEntry> = names
            .into_iter()
            .map(|monitor| HyprpaperEntry {
                monitor,
                path: set.path.display().to_string(),
                fit_mode: lituus_wallpaper::hyprpaper_fit_mode(set.fill).to_owned(),
            })
            .collect();
        (lituus_wallpaper::hyprpaper_conf(&state.hypr_dir()), entries)
    });
    let work = move || -> Result<(), String> {
        lituus_wallpaper::set(&env, program, &set, &monitors).map_err(|e| format!("Couldn't set the picture: {e}."))?;
        if let Some((path, entries)) = conf {
            write_conf(&path, &entries).map_err(|e| format!("Set, but hyprpaper.conf couldn't be saved: {e}."))?;
        }
        Ok(())
    };
    match weak {
        Some(weak) => {
            let weak = weak.clone();
            std::thread::spawn(move || {
                let done = work();
                let w = weak.clone();
                let _ = weak.upgrade_in_event_loop(move |_| update(&w, move |app| app.wallpapers.message = done.err()));
                refresh(&weak);
            });
        }
        None => state.message = work().err(),
    }
}

/// hyprpaper.conf with the blocks set, and Lituus's mark at the top.
fn write_conf(path: &Path, entries: &[HyprpaperEntry]) -> std::io::Result<()> {
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut text = lituus_wallpaper::hyprpaper_conf_text(&old, entries);
    if !text.contains(MARK) {
        text = format!("{MARK}\n{text}");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

/// The user's hyprpaper.conf kept as hyprpaper.conf.bak, before Lituus writes its first blocks.
fn take_over(state: &State) -> std::io::Result<()> {
    let path = lituus_wallpaper::hyprpaper_conf(&state.hypr_dir());
    std::fs::copy(&path, path.with_extension("conf.bak"))?;
    let text = std::fs::read_to_string(&path)?;
    std::fs::write(&path, format!("{MARK}\n{text}"))
}

fn save(state: &mut State, key: &str, value: impl Into<toml_edit::Value>) {
    if let Err(e) = config::set(&state.config_path(), "wallpaper", key, value) {
        state.message = Some(format!("Couldn't save the setting: {e}."));
    }
}

// ---- the page ----------------------------------------------------------------------------------

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "DP-3", "DP-3 and DP-4", "DP-3, DP-4 and DP-5".
fn names(list: &[String]) -> String {
    match list {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn fill_label(fill: Fill) -> &'static str {
    match fill {
        Fill::Cover => "Fill the screen",
        Fill::Contain => "Fit inside",
        Fill::Stretch => "Stretch",
        Fill::Center => "Centre",
        Fill::Tile => "Tile",
    }
}

fn fill_note(fill: Fill) -> &'static str {
    match fill {
        Fill::Cover => "Crops what doesn't fit",
        Fill::Contain => "Bars where it doesn't fit",
        Fill::Stretch => "Its proportions change",
        Fill::Center => "As it is, in the middle",
        Fill::Tile => "Repeated over the screen",
    }
}

fn transition_label(t: &str) -> String {
    match t {
        "none" => "None".into(),
        "simple" => "Cross-fade, quick".into(),
        "fade" => "Fade".into(),
        "left" => "From the left".into(),
        "right" => "From the right".into(),
        "top" => "From the top".into(),
        "bottom" => "From the bottom".into(),
        "wipe" => "Wipe".into(),
        "wave" => "Wave".into(),
        "grow" => "Grow from the middle".into(),
        "center" => "Grow from the centre".into(),
        "outer" => "Shrink to the middle".into(),
        _ => "A different one each time".into(),
    }
}

/// The keys that work on the page.
pub fn keys(state: &State) -> Vec<Hint> {
    if state.program().is_some() { vec![Hint::new("/", "Search pictures")] } else { Vec::new() }
}

pub fn status(state: &State) -> String {
    state.program().map(|p| p.name().to_owned()).unwrap_or_default()
}

pub fn view(state: &State) -> WallpaperView {
    let mut v = WallpaperView::default();
    let home = state.home();
    if !state.read {
        v.state = s("loading");
        v.empty_title = s("Looking for awww and hyprpaper…");
        return v;
    }
    let Some(program) = state.program() else {
        if state.status.installed.is_empty() {
            v.state = s("none-installed");
            v.empty_title = s("No wallpaper program");
            v.empty_text =
                s("Lituus sets wallpapers with awww or hyprpaper. Install one of them from your distribution.");
        } else {
            let p = if state.status.installed.contains(&Program::Awww) { Program::Awww } else { Program::Hyprpaper };
            v.state = s("not-running");
            v.empty_title = s(format!("{} isn't running", p.name()));
            v.empty_text = s(format!(
                "Start it now, and add this line to your Hyprland config so it starts with Hyprland{}.",
                if state.status.installed.len() > 1 { " (or hyprpaper's, if you'd rather)" } else { "" }
            ));
            v.empty_command = s(lituus_wallpaper::autostart_line(p, state.lua));
            v.empty_action = s(format!("Start {}", p.name()));
        }
        return v;
    };
    v.state = s("list");
    v.sub = s(format!(
        "{}{} · {}",
        program.name(),
        if state.status.version.is_empty() { String::new() } else { format!(" {}", state.status.version) },
        match program {
            Program::Awww => "shows the last pictures again when it starts",
            Program::Hyprpaper => "Lituus keeps hyprpaper.conf up to date",
        }
    ));
    // The hero: the monitors where they are, each with its picture.
    let shown = |name: &str| state.current.iter().find(|(m, _)| m == name).map(|(_, p)| p.clone());
    let (min_x, min_y) =
        (state.monitors.iter().map(|m| m.x).min().unwrap_or(0), state.monitors.iter().map(|m| m.y).min().unwrap_or(0));
    let max_x = state.monitors.iter().map(|m| m.x + m.width as i32).max().unwrap_or(1);
    let max_y = state.monitors.iter().map(|m| m.y + m.height as i32).max().unwrap_or(1);
    let (span_x, span_y) = ((max_x - min_x).max(1) as f32, (max_y - min_y).max(1) as f32);
    let targets = state.targets();
    let mut monitors: Vec<WallpaperMonitor> = state
        .monitors
        .iter()
        .map(|m| {
            let file = shown(&m.name);
            WallpaperMonitor {
                name: s(&m.name),
                model: s(&m.model),
                file: s(file
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "No picture".into())),
                x: (m.x - min_x) as f32 / span_x,
                y: (m.y - min_y) as f32 / span_y,
                w: m.width as f32 / span_x,
                h: m.height as f32 / span_y,
                picture: file.as_deref().map(thumb).unwrap_or_default(),
                target: targets.is_empty() || targets.contains(&m.name),
            }
        })
        .collect();
    // Without Hyprland's monitors (another compositor), the program's own list.
    if monitors.is_empty() {
        let n = state.current.len().max(1) as f32;
        monitors = state
            .current
            .iter()
            .enumerate()
            .map(|(i, (name, path))| WallpaperMonitor {
                name: s(name),
                file: s(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                x: i as f32 / n,
                w: 1.0 / n,
                h: 1.0,
                picture: thumb(path),
                target: true,
                ..Default::default()
            })
            .collect();
    }
    v.aspect = if state.monitors.is_empty() { 16.0 / 9.0 * state.current.len().max(1) as f32 } else { span_x / span_y };
    v.monitors = ModelRc::new(VecModel::from(monitors));
    // The folder.
    let dir = state.folder_path();
    v.folder = s(shorten(&dir, &home));
    let mut recent: Vec<String> = state.settings.recent.clone();
    if let Some(parent) = dir.parent().filter(|p| p.starts_with(&home) && *p != home.as_path()) {
        recent.insert(0, shorten(parent, &home));
    }
    recent.retain(|r| expand(r, &home) != dir);
    recent.dedup();
    recent.truncate(6);
    v.recent = strings(recent);
    v.subfolders = state.settings.subfolders;
    v.query = s(&state.query);
    v.sorts = strings(["Newest first".to_owned(), "By name".to_owned()]);
    v.sort = i32::from(state.settings.sort == "name");
    let q = state.query.trim().to_lowercase();
    let mut tiles = Vec::new();
    if let Some(f) = &state.folder {
        for sub in f.subfolders.iter().filter(|sub| q.is_empty() || sub.name.to_lowercase().contains(&q)) {
            tiles.push(WallpaperTile {
                id: s(sub.path.display().to_string()),
                folder: true,
                name: s(&sub.name),
                sub: s(plural(sub.count, "picture", "pictures")),
                pictures: ModelRc::new(VecModel::from(sub.first.iter().map(|p| thumb(p)).collect::<Vec<_>>())),
                ..Default::default()
            });
        }
        let mut pictures: Vec<&lituus_wallpaper::Picture> =
            f.pictures.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q)).collect();
        if state.settings.sort != "name" {
            pictures.sort_by_key(|p| std::cmp::Reverse(p.modified));
        }
        for p in pictures {
            let on: Vec<String> = state.current.iter().filter(|(_, c)| *c == p.path).map(|(m, _)| m.clone()).collect();
            let size = state.sizes.get(&p.path).map(|(w, h)| format!("{w} × {h}")).unwrap_or_default();
            tiles.push(WallpaperTile {
                id: s(p.path.display().to_string()),
                folder: false,
                name: s(&p.name),
                sub: s(if on.is_empty() { size } else { format!("On {}", names(&on)) }),
                accent: !on.is_empty(),
                picture: thumb(&p.path),
                gif: p.gif(),
                ..Default::default()
            });
        }
        v.count = s(format!(
            "{}{}",
            plural(f.pictures.len(), "picture", "pictures"),
            if f.subfolders.is_empty() {
                String::new()
            } else {
                format!(" · {}", plural(f.subfolders.len(), "folder", "folders"))
            }
        ));
        if f.pictures.is_empty() && f.subfolders.is_empty() {
            v.empty_folder = s(if f.others > 0 {
                format!("No pictures here: {} that awww and hyprpaper can't show.", plural(f.others, "file", "files"))
            } else {
                "No pictures here.".into()
            });
        } else if tiles.is_empty() {
            v.empty_folder = s("No picture matches the search.");
        }
    } else {
        v.empty_folder = s(format!("{} can't be read.", shorten(&dir, &home)));
    }
    v.tiles = ModelRc::new(VecModel::from(tiles));
    // The settings.
    v.targets = strings(
        std::iter::once("All monitors".to_owned()).chain(
            state
                .monitors
                .iter()
                .map(|m| if m.model.is_empty() { m.name.clone() } else { format!("{} · {}", m.name, m.model) }),
        ),
    );
    v.target = state.target as i32;
    let fills = program.fills();
    v.fills = strings(fills.iter().map(|f| fill_label(*f).to_owned()));
    let fill = state.fill();
    v.fill = fills.iter().position(|f| *f == fill).unwrap_or(0) as i32;
    v.fill_note = s(fill_note(fill));
    v.awww = program == Program::Awww;
    v.bar_shown = program == Program::Awww;
    v.bar_enabled = matches!(fill, Fill::Contain | Fill::Center);
    v.bar_colour = s(&state.settings.bar_colour);
    v.transitions = strings(TRANSITIONS.iter().map(|t| transition_label(t)));
    v.transition = TRANSITIONS.iter().position(|t| *t == state.settings.transition).unwrap_or(2) as i32;
    v.duration = state.settings.duration as f32;
    v.duration_text = s(format!("{:.1} s", state.settings.duration));
    v.programs = strings(state.status.installed.iter().map(|p| p.name().to_owned()));
    v.program_details = strings(state.status.installed.iter().map(|p| {
        if Some(*p) == state.status.running {
            format!(
                "Running{}",
                if state.status.version.is_empty() { String::new() } else { format!(" · {}", state.status.version) }
            )
        } else {
            "Installed, not running".into()
        }
    }));
    v.program = state.status.installed.iter().position(|p| Some(*p) == state.status.running).unwrap_or(0) as i32;
    v.program_note = s(match program {
        Program::Awww => "Shows the last pictures again when it starts".to_owned(),
        Program::Hyprpaper => {
            format!("Saved in {}", shorten(&lituus_wallpaper::hyprpaper_conf(&state.hypr_dir()), &home))
        }
    });
    v.message = s(state.message.clone().unwrap_or_default());
    v.takeover = state.takeover.is_some();
    v
}

pub fn render(window: &MainWindow, state: &State) {
    if window.get_page() == "wallpaper" {
        // A new model each time: kept as one (as the Input pages do), its rows' thumbnails and
        // lines didn't show their changes in the running app.
        window.set_wallpaper(view(state));
    }
}

pub fn act(weak: &slint::Weak<MainWindow>, state: &mut State, verb: &str, arg: &str, value: f32) {
    act_with(Some(weak), state, verb, arg, value);
}

fn act_with(weak: Option<&slint::Weak<MainWindow>>, state: &mut State, verb: &str, arg: &str, value: f32) {
    let index = value.max(0.0) as usize;
    match verb {
        "set" => {
            let path = PathBuf::from(arg);
            let set = set_for(state, path);
            set_picture(weak, state, set);
        }
        "random" => {
            let pictures: Vec<PathBuf> =
                state.folder.iter().flat_map(|f| f.pictures.iter().map(|p| p.path.clone())).collect();
            if !pictures.is_empty() {
                // Not the one shown now, if there's another.
                let now: Vec<&PathBuf> = state.current.iter().map(|(_, p)| p).collect();
                let others: Vec<&PathBuf> = pictures.iter().filter(|p| !now.contains(p)).collect();
                let pool = if others.is_empty() { pictures.iter().collect() } else { others };
                let seed =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
                let set = set_for(state, pool[seed as usize % pool.len()].clone());
                set_picture(weak, state, set);
            }
        }
        "folder" => {
            let dir = expand(arg, &state.home());
            let old = shorten(&state.folder_path(), &state.home());
            let shown = shorten(&dir, &state.home());
            state.settings.folder = shown.clone();
            let mut recent: Vec<String> =
                std::iter::once(old).chain(state.settings.recent.clone()).filter(|r| *r != shown).collect();
            recent.dedup();
            recent.truncate(6);
            state.settings.recent = recent.clone();
            state.query.clear();
            save(state, "folder", shown);
            let _ = config::set_list(&state.config_path(), "wallpaper", "recent", &recent);
            if let Some(weak) = weak {
                load_folder(weak, state);
            } else {
                state.folder = lituus_wallpaper::read_folder(&state.folder_path(), state.settings.subfolders).ok();
            }
        }
        "choose-folder" => {
            if let Some(weak) = weak {
                choose_folder(weak.clone(), state.folder_path());
            }
        }
        "subfolders" => {
            state.settings.subfolders = !state.settings.subfolders;
            save(state, "subfolders", state.settings.subfolders);
            match weak {
                Some(weak) => load_folder(weak, state),
                None => {
                    state.folder = lituus_wallpaper::read_folder(&state.folder_path(), state.settings.subfolders).ok()
                }
            }
        }
        "query" => state.query = arg.to_owned(),
        "sort" => {
            state.settings.sort = if index == 1 { "name".into() } else { "newest".into() };
            save(state, "sort", state.settings.sort.clone());
        }
        "target" => state.target = index.min(state.monitors.len()),
        "fill" => {
            if let Some(fill) = state.program().and_then(|p| p.fills().get(index).copied()) {
                state.settings.fill = fill.id().into();
                save(state, "fill", fill.id());
                // The pictures shown take it at once.
                reapply(weak, state);
            }
        }
        "bar" => {
            let colour: String =
                arg.trim().trim_start_matches('#').chars().filter(char::is_ascii_hexdigit).take(6).collect();
            if colour.len() == 6 {
                state.settings.bar_colour = colour.clone();
                save(state, "bar_colour", colour);
                reapply(weak, state);
            }
        }
        "transition" => {
            if let Some(t) = TRANSITIONS.get(index) {
                state.settings.transition = (*t).into();
                save(state, "transition", *t);
            }
        }
        "duration" => {
            state.settings.duration = (f64::from(value) * 10.0).round() / 10.0;
            save(state, "duration", state.settings.duration);
        }
        "program" => {
            if let Some(p) = state.status.installed.get(index).copied() {
                switch(weak, state, p);
            }
        }
        "start" => {
            let p = if state.status.installed.contains(&Program::Awww) { Program::Awww } else { Program::Hyprpaper };
            let env = state.env();
            match lituus_wallpaper::start(&env, p) {
                Ok(()) => {
                    if let Some(weak) = weak {
                        refresh_soon(weak);
                    }
                }
                Err(e) => state.message = Some(format!("Couldn't start {}: {e}.", p.name())),
            }
        }
        "takeover-yes" => {
            if let Some(set) = state.takeover.take() {
                match take_over(state) {
                    Ok(()) => set_picture(weak, state, set),
                    Err(e) => {
                        state.message = Some(format!("hyprpaper.conf couldn't be kept as hyprpaper.conf.bak: {e}."))
                    }
                }
            }
        }
        "takeover-no" => state.takeover = None,
        _ => {}
    }
}

/// The pictures shown, set again (a new fill or bar colour), each on its monitor.
fn reapply(weak: Option<&slint::Weak<MainWindow>>, state: &mut State) {
    let current = state.current.clone();
    let target = state.target;
    for (monitor, path) in current {
        state.target = state.monitors.iter().position(|m| m.name == monitor).map_or(0, |i| i + 1);
        if state.target == 0 && !state.monitors.is_empty() {
            continue;
        }
        let mut set = set_for(state, path);
        set.monitors = vec![monitor];
        set.transition.clear();
        set_picture(weak, state, set);
    }
    state.target = target;
}

/// Stops the program running and starts `to`, then shows the same pictures with it.
fn switch(weak: Option<&slint::Weak<MainWindow>>, state: &mut State, to: Program) {
    if state.program() == Some(to) {
        return;
    }
    let env = state.env();
    let from = state.program();
    let current = state.current.clone();
    let monitors: Vec<String> = state.monitors.iter().map(|m| m.name.clone()).collect();
    let fill = state.fill();
    let Some(weak) = weak.cloned() else { return };
    std::thread::spawn(move || {
        if let Some(from) = from {
            let _ = lituus_wallpaper::stop(&env, from);
        }
        let started = lituus_wallpaper::start(&env, to);
        // Give it a moment to come up, then the same pictures.
        for _ in 0..30 {
            if lituus_wallpaper::status(&env).running == Some(to) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        for (monitor, path) in current {
            let set = Set {
                path,
                monitors: vec![monitor],
                fill,
                transition: String::new(),
                duration: 0.0,
                fps: 0,
                bar_colour: String::new(),
            };
            let _ = lituus_wallpaper::set(&env, to, &set, &monitors);
        }
        let w = weak.clone();
        let _ = weak.upgrade_in_event_loop(move |_| {
            update(&w, move |app| {
                app.wallpapers.message = started.err().map(|e| format!("Couldn't start {}: {e}.", to.name()))
            })
        });
        refresh(&weak);
    });
}

/// Reads the programs again in a moment (one just started).
fn refresh_soon(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(800));
        refresh(&weak);
    });
}

/// The desktop's folder dialog, then that folder.
fn choose_folder(weak: slint::Weak<MainWindow>, from: PathBuf) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        let chosen = runtime.block_on(crate::portal::choose_folder("Choose a folder of pictures", Some(&from)));
        let w = weak.clone();
        let _ = weak.upgrade_in_event_loop(move |_| {
            update(&w, |app| match chosen {
                Ok(Some(dir)) => act(&w, &mut app.wallpapers, "folder", &dir.display().to_string(), 0.0),
                Ok(None) => {}
                Err(e) => app.wallpapers.message = Some(format!("{e}.")),
            })
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lituus-wallpaper-page-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A home with pictures, stand-ins for the programs, and a state for the page.
    fn state_in(name: &str, program: Option<Program>) -> (State, PathBuf) {
        let d = tmp(name);
        let home = d.join("home");
        std::fs::create_dir_all(home.join("Pictures/Wallpapers/Nature")).unwrap();
        for (file, size) in [("alps.png", 64), ("fjord.png", 48), ("Nature/forest.png", 32)] {
            image::RgbImage::from_pixel(size, size * 9 / 16, image::Rgb([20, 80, 160]))
                .save(home.join("Pictures/Wallpapers").join(file))
                .unwrap();
        }
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = |cmd: &str, body: &str| {
            let p = bin.join(cmd);
            std::fs::write(&p, format!("#!/bin/sh\necho \"{cmd} $*\" >> {}/calls\n{body}\n", d.display())).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        let alps = home.join("Pictures/Wallpapers/alps.png");
        if program == Some(Program::Awww) {
            script(
                "awww",
                &format!(
                    "case \"$1\" in --version) echo 'awww 0.12.1';; query) echo '{{\"\": [{{\"name\": \"DP-3\", \"displaying\": {{\"image\": \"{}\"}}}}]}}';; esac",
                    alps.display()
                ),
            );
        }
        if program == Some(Program::Hyprpaper) {
            script("hyprpaper", "echo 'hyprpaper v0.8.5'");
            script("hyprctl", "");
            std::fs::create_dir_all(d.join("run/hypr/sig")).unwrap();
            std::fs::write(d.join("run/hypr/sig/.hyprpaper.sock"), "").unwrap();
        }
        let env = Env { path: vec![bin], runtime_dir: Some(d.join("run")), signature: Some("sig".into()) };
        let mut state = State {
            env: Some(env.clone()),
            config_path: Some(d.join("config.toml")),
            hypr_dir: Some(d.join("hypr")),
            home: Some(home),
            ..Default::default()
        };
        let mut r = read(&env);
        r.monitors = vec![
            Monitor {
                name: "DP-3".into(),
                model: "Odyssey G61SH".into(),
                width: 2560,
                height: 1440,
                refresh: 239.76,
                ..Default::default()
            },
            Monitor {
                name: "DP-5".into(),
                model: "Odyssey G81SF".into(),
                x: 2560,
                width: 3840,
                height: 2160,
                refresh: 60.0,
                ..Default::default()
            },
        ];
        loaded(&mut state, r);
        state.folder = lituus_wallpaper::read_folder(&state.folder_path(), false).ok();
        (state, d)
    }

    fn calls(d: &Path) -> Vec<String> {
        std::fs::read_to_string(d.join("calls")).unwrap_or_default().lines().map(str::to_owned).collect()
    }

    #[test]
    fn the_page_with_awww() {
        use slint::Model;
        let (mut state, d) = state_in("awww", Some(Program::Awww));
        let v = view(&state);
        assert_eq!(v.state, "list");
        assert_eq!(v.sub, "awww 0.12.1 · shows the last pictures again when it starts");
        assert_eq!(v.folder, "~/Pictures/Wallpapers");
        assert_eq!(v.count, "2 pictures · 1 folder");
        let tiles: Vec<(String, String)> = v.tiles.iter().map(|t| (t.name.to_string(), t.sub.to_string())).collect();
        assert_eq!(tiles[0], ("Nature".to_owned(), "1 picture".to_owned()));
        assert!(tiles.contains(&("alps.png".to_owned(), "On DP-3".to_owned())), "{tiles:?}");
        let m = v.monitors.row_data(1).unwrap();
        assert_eq!((m.name.as_str(), m.file.as_str()), ("DP-5", "No picture"));
        assert!((m.x - 0.4).abs() < 0.01 && (m.w - 0.6).abs() < 0.01, "{} {}", m.x, m.w);
        // A click on DP-5 alone, with awww's transition at that monitor's refresh rate.
        act_with(None, &mut state, "target", "", 2.0);
        let fjord = state.folder_path().join("fjord.png");
        act_with(None, &mut state, "set", &fjord.display().to_string(), 0.0);
        assert!(
            calls(&d).iter().any(|c| c
                == &format!("awww img {} --outputs DP-5 --resize crop --transition-type fade --transition-duration 1.5 --transition-fps 60", fjord.display())),
            "{:?}",
            calls(&d)
        );
        // Settings go to config.toml.
        act_with(None, &mut state, "transition", "", 7.0);
        act_with(None, &mut state, "subfolders", "", 0.0);
        let saved = config::load(&state.config_path()).wallpaper;
        assert_eq!((saved.transition.as_str(), saved.subfolders), ("wipe", true));
        assert_eq!(view(&state).count, "3 pictures");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// With hyprpaper, hyprpaper.conf is written too; one that was there already is kept as
    /// hyprpaper.conf.bak, after asking.
    #[test]
    fn hyprpaper_and_its_config() {
        let (mut state, d) = state_in("hyprpaper", Some(Program::Hyprpaper));
        let conf = d.join("hypr/hyprpaper.conf");
        std::fs::create_dir_all(d.join("hypr")).unwrap();
        std::fs::write(&conf, "splash = false\nwallpaper {\n    monitor = DP-3\n    path = /old.png\n}\n").unwrap();
        let alps = state.folder_path().join("alps.png");
        act_with(None, &mut state, "set", &alps.display().to_string(), 0.0);
        assert!(view(&state).takeover);
        assert!(calls(&d).iter().all(|c| !c.contains("wallpaper")), "{:?}", calls(&d));
        act_with(None, &mut state, "takeover-yes", "", 0.0);
        let c = calls(&d);
        assert!(c.contains(&format!("hyprctl hyprpaper wallpaper DP-3, {}, cover", alps.display())), "{c:?}");
        assert!(c.contains(&format!("hyprctl hyprpaper wallpaper DP-5, {}, cover", alps.display())), "{c:?}");
        let text = std::fs::read_to_string(&conf).unwrap();
        assert!(text.starts_with(MARK) && text.contains("splash = false"), "{text}");
        assert_eq!(lituus_wallpaper::hyprpaper_entries(&text).len(), 2);
        assert!(std::fs::read_to_string(d.join("hypr/hyprpaper.conf.bak")).unwrap().contains("/old.png"));
        // Asked once.
        act_with(None, &mut state, "set", &alps.display().to_string(), 0.0);
        assert!(state.takeover.is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The grid says which monitors show a picture, and follows when that changes.
    #[test]
    fn the_grid_follows_the_pictures_shown() {
        use i_slint_backend_testing::ElementHandle;
        use slint::ComponentHandle;
        i_slint_backend_testing::init_no_event_loop();
        let (mut state, d) = state_in("grid", Some(Program::Awww));
        let window = MainWindow::new().unwrap();
        window.set_page("wallpaper".into());
        window.window().set_size(slint::LogicalSize::new(2000.0, 1070.0));
        window.show().unwrap();
        let has = |label: &str| ElementHandle::find_by_accessible_label(&window, label).next().is_some();
        render(&window, &state);
        assert!(has("alps.png, On DP-3"));
        let fjord = state.folder_path().join("fjord.png");
        state.current = vec![("DP-3".into(), fjord)];
        render(&window, &state);
        assert!(has("fjord.png, On DP-3"));
        assert!(!has("alps.png, On DP-3"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn nothing_running() {
        let (state, d) = state_in("none", None);
        let v = view(&state);
        assert_eq!((v.state.as_str(), v.empty_title.as_str()), ("none-installed", "No wallpaper program"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
