//! `~/.config/vinculum/config.toml`. Every key is optional; a missing file means the defaults.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: ThemeConfig,
    pub tray: TrayConfig,
    pub notifications: NotificationsConfig,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TrayConfig {
    /// Show the tray icon (when a bar with a tray runs).
    pub enabled: bool,
    /// Closing the window keeps Vinculum running in the tray.
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
    /// Where colours come from, in order: the first that exists and is a usable palette wins.
    pub sources: Vec<ColourSource>,
    /// The theme when no source is usable.
    pub builtin: Builtin,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            sources: vec![
                ColourSource { path: "~/.config/vinculum/colors.json".into() },
                // ML4W writes every matugen colour role here on each wallpaper change.
                ColourSource { path: "~/.config/ml4w/colors/colors.json".into() },
            ],
            builtin: Builtin::Auto,
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
    /// Dark or light, as the desktop prefers.
    #[default]
    Auto,
    Dark,
    Light,
}

/// `$XDG_CONFIG_HOME/vinculum`, or `~/.config/vinculum`.
pub fn dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(config) => PathBuf::from(config).join("vinculum"),
        None => expand_home("~/.config/vinculum"),
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
            eprintln!("vinculum: ignoring {}: {error}", path.display());
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
}

impl Builtin {
    pub const ALL: [Builtin; 3] = [Builtin::Auto, Builtin::Dark, Builtin::Light];

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
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
        assert_eq!(paths, ["~/.config/vinculum/colors.json", "~/.config/ml4w/colors/colors.json"]);
        assert_eq!(config.theme.builtin, Builtin::Auto);
    }

    #[test]
    fn sources_are_an_ordered_list() {
        let config = parse(
            r#"
            [theme]
            builtin = "light"
            [[theme.sources]]
            path = "~/wal.json"
            [[theme.sources]]
            path = "/etc/colors.json"
            "#,
        )
        .unwrap();
        let paths: Vec<&str> = config.theme.sources.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["~/wal.json", "/etc/colors.json"]);
        assert_eq!(config.theme.builtin, Builtin::Light);
    }

    #[test]
    fn settings_keep_the_rest_of_the_file() {
        let text = "# my colours\n[theme]\nbuiltin = \"dark\" # night owl\n";
        let text = with(text, "tray", "keep_running", false).unwrap();
        let text = with(&text, "theme", "builtin", "light").unwrap();
        assert_eq!(text, "# my colours\n[theme]\nbuiltin = \"light\" # night owl\n\n[tray]\nkeep_running = false\n");
        let config = parse(&text).unwrap();
        assert_eq!(config.theme.builtin, Builtin::Light);
        assert!(config.tray.enabled && !config.tray.keep_running && config.notifications.enabled);
    }

    #[test]
    fn typos_are_errors_not_silently_ignored() {
        assert!(parse("[theme]\nbuiltn = \"dark\"").is_err());
        assert!(parse("[theme]\nbuiltin = \"blue\"").is_err());
    }
}
