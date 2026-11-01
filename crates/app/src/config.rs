//! `~/.config/lituus/config.toml`. Every key is optional; a missing file means the defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: ThemeConfig,
    pub tray: TrayConfig,
    pub notifications: NotificationsConfig,
    pub sound: SoundConfig,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SoundConfig {
    /// Volume sliders go to 150 %.
    pub overamplify: bool,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TrayConfig {
    /// Show the tray icon (when a bar with a tray runs).
    pub enabled: bool,
    /// Closing the window keeps Lituus running in the tray.
    pub keep_running: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self { enabled: true, keep_running: true }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationsConfig {
    /// Connected, disconnected and failed, while the window isn't showing.
    pub enabled: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// Use the wallpaper's colours (the sources) when there are some.
    pub follow_wallpaper: bool,
    /// Where colours come from, in order: the first that exists and is a usable palette wins.
    pub sources: Vec<ColourSource>,
    /// The theme when not following the wallpaper, or when no source is usable.
    pub builtin: Builtin,
    /// Dark or light, for the built-in theme.
    pub mode: Mode,
    /// The accent glow behind each page's hero.
    pub gradient: bool,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            sources: vec![
                ColourSource { path: "~/.config/lituus/colors.json".into() },
                // ML4W writes every matugen colour role here on each wallpaper change.
                ColourSource { path: "~/.config/ml4w/colors/colors.json".into() },
            ],
            follow_wallpaper: true,
            builtin: Builtin::Teal,
            mode: Mode::Auto,
            gradient: true,
        }
    }
}

/// A matugen-style `colors.json`: Material colour roles (`primary`, `surface`, …) as hex strings.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColourSource {
    pub path: String,
}

impl ColourSource {
    pub fn resolved(&self) -> PathBuf {
        expand_home(&self.path)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Builtin {
    #[default]
    Teal,
    Blue,
    Lavender,
    Coral,
    Mono,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Dark or light, as the desktop prefers.
    #[default]
    Auto,
    Dark,
    Light,
}

/// `$XDG_CONFIG_HOME/lituus`, or `~/.config/lituus`.
pub fn dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(config) => PathBuf::from(config).join("lituus"),
        None => expand_home("~/.config/lituus"),
    }
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

/// Reads the config, or the defaults if there's none. A broken file is reported and ignored, so
/// a typo never keeps the app from starting.
pub fn load(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).unwrap_or_else(|error| {
            eprintln!("lituus: ignoring {}: {error}", path.display());
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
}

impl Builtin {
    pub const ALL: [Builtin; 5] = [Builtin::Teal, Builtin::Blue, Builtin::Lavender, Builtin::Coral, Builtin::Mono];

    pub fn key(self) -> &'static str {
        match self {
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Lavender => "lavender",
            Self::Coral => "coral",
            Self::Mono => "mono",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Teal => "Teal",
            Self::Blue => "Blue",
            Self::Lavender => "Lavender",
            Self::Coral => "Coral",
            Self::Mono => "Mono",
        }
    }
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Auto, Mode::Dark, Mode::Light];

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn dark(self, prefers_dark: bool) -> bool {
        match self {
            Self::Auto => prefers_dark,
            Self::Dark => true,
            Self::Light => false,
        }
    }
}

/// Sets one key, keeping the rest of the file as it is (comments and order included).
pub fn set(path: &Path, table: &str, key: &str, value: impl Into<toml_edit::Value>) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let text = with(&text, table, key, value).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

fn with(
    text: &str,
    table: &str,
    key: &str,
    value: impl Into<toml_edit::Value>,
) -> Result<String, toml_edit::TomlError> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let entry = doc.entry(table).or_insert_with(toml_edit::table);
    if let Some(t) = entry.as_table_like_mut() {
        let mut value = value.into();
        // A comment after the old value stays after the new one.
        if let Some(old) = t.get(key).and_then(|item| item.as_value()) {
            *value.decor_mut() = old.decor().clone();
        }
        t.insert(key, toml_edit::Item::Value(value));
    }
    Ok(doc.to_string())
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_has_the_default_sources() {
        let config = parse("").unwrap();
        assert_eq!(config, Config::default());
        let paths: Vec<&str> = config.theme.sources.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["~/.config/lituus/colors.json", "~/.config/ml4w/colors/colors.json"]);
        assert_eq!((config.theme.builtin, config.theme.mode), (Builtin::Teal, Mode::Auto));
        assert!(config.theme.follow_wallpaper && config.theme.gradient);
    }

    #[test]
    fn sources_are_an_ordered_list() {
        let config = parse(
            r#"
            [theme]
            builtin = "coral"
            mode = "light"
            follow_wallpaper = false
            [[theme.sources]]
            path = "~/wal.json"
            [[theme.sources]]
            path = "/etc/colors.json"
            "#,
        )
        .unwrap();
        let paths: Vec<&str> = config.theme.sources.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["~/wal.json", "/etc/colors.json"]);
        assert_eq!((config.theme.builtin, config.theme.mode), (Builtin::Coral, Mode::Light));
        assert!(!config.theme.follow_wallpaper);
    }

    #[test]
    fn settings_keep_the_rest_of_the_file() {
        let text = "# my colours\n[theme]\nmode = \"dark\" # night owl\n";
        let text = with(text, "tray", "keep_running", false).unwrap();
        let text = with(&text, "theme", "mode", "light").unwrap();
        assert_eq!(text, "# my colours\n[theme]\nmode = \"light\" # night owl\n\n[tray]\nkeep_running = false\n");
        let config = parse(&text).unwrap();
        assert_eq!(config.theme.mode, Mode::Light);
        assert!(config.tray.enabled && !config.tray.keep_running && config.notifications.enabled);
    }

    #[test]
    fn typos_are_errors_not_silently_ignored() {
        assert!(parse("[theme]\nbuiltn = \"dark\"").is_err());
        assert!(parse("[theme]\nbuiltin = \"purple\"").is_err());
        assert!(parse("[theme]\nmode = \"dim\"").is_err());
    }
}
