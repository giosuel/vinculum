//! The palette: the built-in dark and light themes, and palettes made from matugen colour roles,
//! mapped exactly as in the mockups. Warnings keep a fixed amber, so they never read as errors.

use std::collections::HashMap;
use std::path::Path;

use crate::config::{Builtin, ColourSource};

/// RGB, 0–255.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(hex: &str) -> Option<Self> {
        let hex = hex.strip_prefix('#')?;
        if hex.len() != 6 {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self(byte(0)?, byte(2)?, byte(4)?))
    }

    const fn hex(value: u32) -> Self {
        Self((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    /// WCAG relative luminance.
    fn luminance(self) -> f64 {
        let channel = |c: u8| {
            let c = f64::from(c) / 255.0;
            if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(self.0) + 0.7152 * channel(self.1) + 0.0722 * channel(self.2)
    }

    /// WCAG contrast ratio, 1–21.
    pub fn contrast(self, other: Self) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

macro_rules! palette {
    ($($name:ident),* $(,)?) => {
        /// Every colour the UI uses; the names match the `Theme` global in ui/theme.slint.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct Palette { $(pub $name: Rgb),* }
    };
}

palette!(
    bg,
    panel,
    surface,
    border,
    border2,
    ctrl_border,
    ctrl_bg,
    input_bg,
    text,
    muted,
    faint,
    icon_soft,
    accent,
    on_accent,
    accent_soft_bg,
    accent_soft_border,
    off_track,
    knob_off,
    bar_off,
    sel_bg,
    danger,
    warn_bg,
    warn_border,
    warn_text,
);

const AMBER_DARK: (Rgb, Rgb, Rgb) = (Rgb::hex(0x2a2518), Rgb::hex(0x5a4c25), Rgb::hex(0xe9cf8f));

pub const DARK: Palette = Palette {
    bg: Rgb::hex(0x111315),
    panel: Rgb::hex(0x17191c),
    surface: Rgb::hex(0x1b1e21),
    border: Rgb::hex(0x262a2f),
    border2: Rgb::hex(0x2a2e33),
    ctrl_border: Rgb::hex(0x353a40),
    ctrl_bg: Rgb::hex(0x23272b),
    input_bg: Rgb::hex(0x1f2226),
    text: Rgb::hex(0xe8eaed),
    muted: Rgb::hex(0xa0a7af),
    faint: Rgb::hex(0x8b929a),
    icon_soft: Rgb::hex(0xc9ced4),
    accent: Rgb::hex(0x5cc8b0),
    on_accent: Rgb::hex(0x0c1513),
    accent_soft_bg: Rgb::hex(0x1f2a27),
    accent_soft_border: Rgb::hex(0x2c4a42),
    off_track: Rgb::hex(0x3a3f46),
    knob_off: Rgb::hex(0xc9ced4),
    bar_off: Rgb::hex(0x3a3f46),
    sel_bg: Rgb::hex(0x23272b),
    danger: Rgb::hex(0xf08a7a),
    warn_bg: AMBER_DARK.0,
    warn_border: AMBER_DARK.1,
    warn_text: AMBER_DARK.2,
};

pub const LIGHT: Palette = Palette {
    bg: Rgb::hex(0xeef0f2),
    panel: Rgb::hex(0xf7f8f9),
    surface: Rgb::hex(0xffffff),
    border: Rgb::hex(0xdde1e5),
    border2: Rgb::hex(0xe3e6ea),
    ctrl_border: Rgb::hex(0xcfd4da),
    ctrl_bg: Rgb::hex(0xffffff),
    input_bg: Rgb::hex(0xffffff),
    text: Rgb::hex(0x1a1c1f),
    muted: Rgb::hex(0x535a62),
    faint: Rgb::hex(0x5f666e),
    icon_soft: Rgb::hex(0x3d434a),
    accent: Rgb::hex(0x0b7a66),
    on_accent: Rgb::hex(0xffffff),
    accent_soft_bg: Rgb::hex(0xe3f3ef),
    accent_soft_border: Rgb::hex(0xa9d9cc),
    off_track: Rgb::hex(0xc3c9cf),
    knob_off: Rgb::hex(0xffffff),
    bar_off: Rgb::hex(0xc9ced4),
    sel_bg: Rgb::hex(0xeceef0),
    danger: Rgb::hex(0xb3261e),
    warn_bg: Rgb::hex(0xfbf4e3),
    warn_border: Rgb::hex(0xe8d49b),
    warn_text: Rgb::hex(0x6b5410),
};

impl Palette {
    pub fn builtin(builtin: Builtin, prefers_dark: bool) -> Self {
        match builtin {
            Builtin::Dark => DARK,
            Builtin::Light => LIGHT,
            Builtin::Auto if prefers_dark => DARK,
            Builtin::Auto => LIGHT,
        }
    }

    /// From matugen's Material colour roles (as in ML4W's colors.json). `None` if a role is
    /// missing or the result isn't readable (text below 4.5:1 on its background).
    pub fn from_material(roles: &HashMap<String, String>) -> Option<Self> {
        let role = |name: &str| roles.get(name).and_then(|hex| Rgb::parse(hex));
        let palette = Self {
            bg: role("surface")?,
            panel: role("surface_container_low")?,
            surface: role("surface_container")?,
            border: role("surface_container_high")?,
            border2: role("surface_container_highest")?,
            ctrl_border: role("outline_variant")?,
            ctrl_bg: role("surface_container_high")?,
            input_bg: role("surface_container_lowest")?,
            text: role("on_surface")?,
            muted: role("on_surface_variant")?,
            faint: role("outline")?,
            icon_soft: role("on_surface_variant")?,
            accent: role("primary")?,
            on_accent: role("on_primary")?,
            accent_soft_bg: role("secondary_container")?,
            accent_soft_border: role("primary_container")?,
            off_track: role("outline_variant")?,
            knob_off: role("outline")?,
            bar_off: role("outline_variant")?,
            sel_bg: role("surface_container_high")?,
            danger: role("error")?,
            warn_bg: AMBER_DARK.0,
            warn_border: AMBER_DARK.1,
            warn_text: AMBER_DARK.2,
        };
        palette.readable().then_some(palette)
    }

    fn readable(&self) -> bool {
        [self.bg, self.panel, self.surface].iter().all(|&bg| self.text.contrast(bg) >= 4.5)
            && self.on_accent.contrast(self.accent) >= 4.5
    }
}

/// What a colour source gave.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceStatus {
    Missing,
    /// It exists but isn't a usable palette: not JSON, a role missing, or unreadable colours.
    Unusable,
    Used(Palette),
}

pub fn read_source(path: &Path) -> SourceStatus {
    let Ok(text) = std::fs::read_to_string(path) else { return SourceStatus::Missing };
    let Ok(roles) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&text) else {
        return SourceStatus::Unusable;
    };
    let roles: HashMap<String, String> =
        roles.into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned()))).collect();
    Palette::from_material(&roles).map_or(SourceStatus::Unusable, SourceStatus::Used)
}

/// Each source's status, in order, and the palette of the first usable one (`None`: built-in).
pub fn resolve(sources: &[ColourSource]) -> (Vec<SourceStatus>, Option<Palette>) {
    let statuses: Vec<SourceStatus> = sources.iter().map(|s| read_source(&s.resolved())).collect();
    let palette = statuses.iter().find_map(|s| match s {
        SourceStatus::Used(p) => Some(*p),
        _ => None,
    });
    (statuses, palette)
}

/// Why the built-in theme choice doesn't apply: the colour source in use, if there is one.
pub fn builtin_note(sources: &[ColourSource], statuses: &[SourceStatus]) -> String {
    let used = sources.iter().zip(statuses).find(|(_, s)| matches!(s, SourceStatus::Used(_)));
    let Some((source, _)) = used else { return String::new() };
    let what = if source.path.contains("ml4w") { "Your ML4W colours are" } else { "Your colour file is" };
    format!("{what} in use. The built-in theme applies when no colour source is found.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_theme_is_greyed_out_while_a_colour_source_is_in_use() {
        let sources = crate::config::ThemeConfig::default().sources;
        let used = SourceStatus::Used(Palette::from_material(&blue()).unwrap());
        assert_eq!(builtin_note(&sources, &[SourceStatus::Missing, SourceStatus::Missing]), "");
        let note = builtin_note(&sources, &[SourceStatus::Missing, used.clone()]);
        assert!(note.starts_with("Your ML4W colours are in use."), "{note}");
        assert!(builtin_note(&sources, &[used, SourceStatus::Missing]).starts_with("Your colour file is"));
    }

    /// The owner's blue ML4W palette from the mockups.
    fn blue() -> HashMap<String, String> {
        [
            ("surface", "#101418"),
            ("surface_container_lowest", "#0b0e12"),
            ("surface_container_low", "#191c20"),
            ("surface_container", "#1d2024"),
            ("surface_container_high", "#272a2f"),
            ("surface_container_highest", "#32353a"),
            ("on_surface", "#e0e2e8"),
            ("on_surface_variant", "#c2c7cf"),
            ("outline", "#8c9199"),
            ("outline_variant", "#42474e"),
            ("primary", "#9ecafc"),
            ("on_primary", "#003256"),
            ("primary_container", "#164974"),
            ("secondary_container", "#3b4857"),
            ("error", "#ffb4ab"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    #[test]
    fn hex_colours() {
        assert_eq!(Rgb::parse("#5cc8b0"), Some(Rgb(0x5c, 0xc8, 0xb0)));
        assert_eq!(Rgb::parse("5cc8b0"), None);
        assert_eq!(Rgb::parse("#5cc8b"), None);
        assert_eq!(Rgb::parse("#zzzzzz"), None);
    }

    #[test]
    fn contrast_ratio() {
        assert!((Rgb(0, 0, 0).contrast(Rgb(255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((Rgb(10, 10, 10).contrast(Rgb(10, 10, 10)) - 1.0).abs() < 0.001);
    }

    #[test]
    fn material_roles_map_like_the_mockups() {
        let p = Palette::from_material(&blue()).expect("a usable palette");
        assert_eq!(p.bg, Rgb::parse("#101418").unwrap());
        assert_eq!(p.accent, Rgb::parse("#9ecafc").unwrap());
        assert_eq!(p.accent_soft_bg, Rgb::parse("#3b4857").unwrap());
        assert_eq!(p.danger, Rgb::parse("#ffb4ab").unwrap());
        // Warnings stay amber whatever the wallpaper.
        assert_eq!(p.warn_text, DARK.warn_text);
    }

    #[test]
    fn missing_roles_or_unreadable_text_fall_back() {
        let mut roles = blue();
        roles.remove("primary");
        assert_eq!(Palette::from_material(&roles), None);
        let mut roles = blue();
        roles.insert("on_surface".into(), "#202428".into());
        assert_eq!(Palette::from_material(&roles), None);
    }

    #[test]
    fn built_in_palettes_are_readable() {
        assert!(DARK.readable());
        assert!(LIGHT.readable());
    }

    #[test]
    fn first_usable_source_wins() {
        let dir = std::env::temp_dir().join(format!("vinculum-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let broken = dir.join("broken.json");
        std::fs::write(&broken, "{ not json").unwrap();
        let good = dir.join("colors.json");
        std::fs::write(&good, serde_json::to_string(&blue()).unwrap()).unwrap();
        let sources = [
            ColourSource { path: dir.join("missing.json").display().to_string() },
            ColourSource { path: broken.display().to_string() },
            ColourSource { path: good.display().to_string() },
        ];
        let (statuses, palette) = resolve(&sources);
        assert_eq!(statuses[0], SourceStatus::Missing);
        assert_eq!(statuses[1], SourceStatus::Unusable);
        assert!(matches!(statuses[2], SourceStatus::Used(_)));
        assert_eq!(palette.map(|p| p.accent), Rgb::parse("#9ecafc"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
