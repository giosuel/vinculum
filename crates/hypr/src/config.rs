//! Lituus's own file next to the user's Hyprland config: `lituus.lua`, which hyprland.lua loads
//! with `require("lituus")` (Hyprland puts its config folder on Lua's path), or `lituus.conf`
//! with `source =` before Hyprland 0.55. The user's own config is only read, never written.

use std::path::{Path, PathBuf};

/// `$XDG_CONFIG_HOME/hypr`, or `~/.config/hypr`.
pub fn config_dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(config) => PathBuf::from(config).join("hypr"),
        None => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".config/hypr"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigFile {
    pub dir: PathBuf,
    /// Lua (Hyprland 0.55 and later) or hyprlang.
    pub lua: bool,
}

impl ConfigFile {
    pub fn new(dir: impl Into<PathBuf>, lua: bool) -> Self {
        Self { dir: dir.into(), lua }
    }

    /// `lituus.lua` or `lituus.conf`.
    pub fn path(&self) -> PathBuf {
        self.dir.join(if self.lua { "lituus.lua" } else { "lituus.conf" })
    }

    /// The user's main config.
    pub fn main_path(&self) -> PathBuf {
        self.dir.join(if self.lua { "hyprland.lua" } else { "hyprland.conf" })
    }

    /// The line the user's config needs to load Lituus's file.
    pub fn load_line(&self) -> String {
        if self.lua { "require(\"lituus\")".to_owned() } else { "source = ~/.config/hypr/lituus.conf".to_owned() }
    }

    /// Whether the user's config loads Lituus's file: a `require("lituus")` in a Lua file of the
    /// config folder (other than ours), or a `source` of lituus.conf.
    pub fn loaded(&self) -> bool {
        let files: Vec<PathBuf> = if self.lua {
            std::fs::read_dir(&self.dir)
                .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect::<Vec<PathBuf>>())
                .unwrap_or_default()
                .into_iter()
                .filter(|p: &PathBuf| p.extension().is_some_and(|e| e == "lua") && *p != self.path())
                .collect()
        } else {
            vec![self.main_path()]
        };
        files.iter().any(|f| std::fs::read_to_string(f).is_ok_and(|text| self.loads(&text)))
    }

    fn loads(&self, text: &str) -> bool {
        text.lines().map(str::trim).filter(|l| !l.starts_with("--") && !l.starts_with('#')).any(|line| {
            if self.lua {
                let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
                ["require(\"lituus\")", "require('lituus')", "require\"lituus\"", "require'lituus'"]
                    .iter()
                    .any(|r| compact.contains(r))
            } else {
                line.starts_with("source") && line.ends_with("lituus.conf")
            }
        })
    }

    pub fn read(&self) -> Option<String> {
        std::fs::read_to_string(self.path()).ok()
    }

    /// Writes the file whole, through a temporary file, so Hyprland never reads half of it.
    pub fn write(&self, text: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!(".{}.tmp", file_name(&self.path())));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, self.path())
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lituus-hypr-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn finds_the_require_in_any_lua_file() {
        let d = dir("lua");
        let f = ConfigFile::new(&d, true);
        std::fs::write(d.join("hyprland.lua"), "-- require(\"lituus\") is commented out\nrequire(\"monitors\")\n")
            .unwrap();
        assert!(!f.loaded());
        std::fs::write(d.join("monitors.lua"), "require ( 'lituus' )\n").unwrap();
        assert!(f.loaded());
        // Our own file doesn't count.
        std::fs::remove_file(d.join("monitors.lua")).unwrap();
        f.write("require(\"lituus\")\n").unwrap();
        assert!(!f.loaded());
        assert_eq!(f.read().as_deref(), Some("require(\"lituus\")\n"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn finds_the_source_line() {
        let d = dir("conf");
        let f = ConfigFile::new(&d, false);
        std::fs::write(d.join("hyprland.conf"), "monitor = , preferred, auto, 1\n").unwrap();
        assert!(!f.loaded());
        std::fs::write(d.join("hyprland.conf"), "source = ~/.config/hypr/lituus.conf\n").unwrap();
        assert!(f.loaded());
        assert_eq!(f.path(), d.join("lituus.conf"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
