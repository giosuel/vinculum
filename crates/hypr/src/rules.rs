//! Monitor and workspace rules, written as Lua (`hl.monitor`, Hyprland 0.55 and later) or as
//! hyprlang (`monitor =`, older), for Lituus's own file and for applying live.

use crate::model::{Mode, Monitor};

/// How a monitor should be set up.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorRule {
    /// "desc:…" or a connector.
    pub output: String,
    pub disabled: bool,
    pub mode: Mode,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    /// 0–7: normal, 90°, 180°, 270°, and the same flipped.
    pub transform: u8,
    /// The selector of the monitor it shows the same as.
    pub mirror: Option<String>,
    pub ten_bit: bool,
    /// "srgb", "wide", "hdr", "hdredid", "auto"…
    pub cm: String,
    pub sdr_brightness: f64,
    pub sdr_saturation: f64,
    /// 0 off, 1 on, 2 fullscreen only, 3 fullscreen games and video; `None` for the global setting.
    pub vrr: Option<u8>,
    /// Top, right, bottom, left.
    pub reserved: [i32; 4],
}

impl MonitorRule {
    /// The rule that keeps a monitor as it is now.
    pub fn from_monitor(m: &Monitor, all: &[Monitor]) -> Self {
        let mirror = all.iter().find(|o| o.id.to_string() == m.mirror_of).map(Monitor::selector);
        Self {
            output: m.selector(),
            disabled: m.disabled,
            mode: m.mode(),
            x: m.x,
            y: m.y,
            scale: m.scale,
            transform: m.transform,
            mirror,
            ten_bit: m.ten_bit(),
            cm: if m.color_management_preset.is_empty() { "srgb".into() } else { m.color_management_preset.clone() },
            sdr_brightness: m.sdr_brightness,
            sdr_saturation: m.sdr_saturation,
            vrr: None,
            reserved: [0; 4],
        }
    }

    /// `hl.monitor({ … })`, for the file.
    pub fn lua(&self) -> String {
        self.lua_rule(false)
    }

    /// `hl.monitor({ … })` to apply live: Hyprland adds a rule's fields to the monitor's earlier
    /// rule, so this also turns off what earlier ones may have set (disabled, a mirror).
    pub fn lua_live(&self) -> String {
        self.lua_rule(true)
    }

    fn lua_rule(&self, live: bool) -> String {
        if self.disabled {
            return format!("hl.monitor({{ output = {}, disabled = true }})", lua_string(&self.output));
        }
        let mut fields = vec![
            format!("output = {}", lua_string(&self.output)),
            format!("mode = {}", lua_string(&self.mode.rule())),
            format!("position = \"{}x{}\"", self.x, self.y),
            format!("scale = \"{}\"", number(self.scale)),
            format!("transform = {}", self.transform),
            format!("bitdepth = {}", if self.ten_bit { 10 } else { 8 }),
            format!("cm = {}", lua_string(&self.cm)),
        ];
        if (self.sdr_brightness - 1.0).abs() > 1e-6 || (self.sdr_saturation - 1.0).abs() > 1e-6 {
            fields.push(format!("sdrbrightness = {}", number(self.sdr_brightness)));
            fields.push(format!("sdrsaturation = {}", number(self.sdr_saturation)));
        }
        if let Some(vrr) = self.vrr {
            fields.push(format!("vrr = {vrr}"));
        }
        match &self.mirror {
            Some(mirror) => fields.push(format!("mirror = {}", lua_string(mirror))),
            None if live => fields.push("mirror = \"\"".into()),
            None => {}
        }
        if live {
            fields.push("disabled = false".into());
        }
        if self.reserved != [0; 4] {
            let [t, r, b, l] = self.reserved;
            fields.push(format!("reserved_area = {{ top = {t}, right = {r}, bottom = {b}, left = {l} }}"));
        }
        format!("hl.monitor({{ {} }})", fields.join(", "))
    }

    /// `monitor = …`, for Hyprland before 0.55 (without the `monitor = `, as `hyprctl keyword
    /// monitor` takes it).
    pub fn hyprlang(&self) -> String {
        if self.disabled {
            return format!("{}, disable", self.output);
        }
        let mut s = format!("{}, {}, {}x{}, {}", self.output, self.mode.rule(), self.x, self.y, number(self.scale));
        if self.transform != 0 {
            s += &format!(", transform, {}", self.transform);
        }
        if let Some(mirror) = &self.mirror {
            s += &format!(", mirror, {mirror}");
        }
        if self.ten_bit {
            s += ", bitdepth, 10";
        }
        if self.cm != "srgb" {
            s += &format!(", cm, {}", self.cm);
        }
        if (self.sdr_brightness - 1.0).abs() > 1e-6 || (self.sdr_saturation - 1.0).abs() > 1e-6 {
            s += &format!(
                ", sdrbrightness, {}, sdrsaturation, {}",
                number(self.sdr_brightness),
                number(self.sdr_saturation)
            );
        }
        if let Some(vrr) = self.vrr {
            s += &format!(", vrr, {vrr}");
        }
        s
    }

    /// The logical size on the layout.
    pub fn logical_size(&self) -> (i32, i32) {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) =
            ((f64::from(self.mode.width) / scale).round() as i32, (f64::from(self.mode.height) / scale).round() as i32);
        if self.transform % 2 == 1 { (h, w) } else { (w, h) }
    }
}

/// Which monitor a workspace opens on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRule {
    pub workspace: String,
    pub monitor: String,
    /// The workspace shown when the monitor turns on.
    pub default: bool,
    /// Kept even when empty.
    pub persistent: bool,
}

impl WorkspaceRule {
    pub fn lua(&self) -> String {
        let mut fields = vec![
            format!("workspace = {}", lua_string(&self.workspace)),
            format!("monitor = {}", lua_string(&self.monitor)),
        ];
        if self.default {
            fields.push("default = true".into());
        }
        if self.persistent {
            fields.push("persistent = true".into());
        }
        format!("hl.workspace_rule({{ {} }})", fields.join(", "))
    }

    pub fn hyprlang(&self) -> String {
        let mut s = format!("{}, monitor:{}", self.workspace, self.monitor);
        if self.default {
            s += ", default:true";
        }
        if self.persistent {
            s += ", persistent:true";
        }
        s
    }
}

/// Lituus's own config file, for Lua or hyprlang; `profile` names the layout in a comment.
pub fn file(lua: bool, profile: Option<&str>, monitors: &[MonitorRule], workspaces: &[WorkspaceRule]) -> String {
    let comment = if lua { "--" } else { "#" };
    let mut out =
        format!("{comment} Written by Lituus (Displays). Change it in Lituus: edits made here are overwritten.\n");
    if let Some(p) = profile {
        out += &format!("{comment} Profile: {p}\n");
    }
    out += "\n";
    for m in monitors {
        out += &if lua { m.lua() } else { format!("monitor = {}", m.hyprlang()) };
        out += "\n";
    }
    if !workspaces.is_empty() {
        out += "\n";
        for w in workspaces {
            out += &if lua { w.lua() } else { format!("workspace = {}", w.hyprlang()) };
            out += "\n";
        }
    }
    out
}

/// A Lua string literal.
fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 1.5, 1, 1.333333: as short as it goes.
fn number(n: f64) -> String {
    let s = format!("{n:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitors() -> Vec<Monitor> {
        serde_json::from_str(include_str!("../tests/fixtures/monitors.json")).unwrap()
    }

    #[test]
    fn lua_rules() {
        let all = monitors();
        let mut g8 = MonitorRule::from_monitor(&all[1], &all);
        assert_eq!(
            g8.lua(),
            r#"hl.monitor({ output = "desc:Samsung Electric Company Odyssey G81SF HNAY500052", mode = "3840x2160@240", position = "3778x2393", scale = "1.5", transform = 0, bitdepth = 8, cm = "srgb" })"#
        );
        g8.ten_bit = true;
        g8.cm = "hdr".into();
        g8.sdr_brightness = 1.2;
        g8.sdr_saturation = 0.98;
        g8.vrr = Some(2);
        g8.mirror = Some("DP-3".into());
        assert_eq!(
            g8.lua(),
            r#"hl.monitor({ output = "desc:Samsung Electric Company Odyssey G81SF HNAY500052", mode = "3840x2160@240", position = "3778x2393", scale = "1.5", transform = 0, bitdepth = 10, cm = "hdr", sdrbrightness = 1.2, sdrsaturation = 0.98, vrr = 2, mirror = "DP-3" })"#
        );
        assert!(g8.lua_live().ends_with(r#"mirror = "DP-3", disabled = false })"#));
        g8.mirror = None;
        assert!(g8.lua_live().ends_with(r#"mirror = "", disabled = false })"#));
        g8.disabled = true;
        assert_eq!(
            g8.lua(),
            r#"hl.monitor({ output = "desc:Samsung Electric Company Odyssey G81SF HNAY500052", disabled = true })"#
        );
    }

    #[test]
    fn hyprlang_rules() {
        let all = monitors();
        let mut dp4 = MonitorRule::from_monitor(&all[2], &all);
        assert_eq!(
            dp4.hyprlang(),
            "desc:Samsung Electric Company Odyssey G61SH HNAL600234, 2560x1440@59.95, 6338x2393, 1"
        );
        dp4.transform = 1;
        dp4.ten_bit = true;
        dp4.vrr = Some(1);
        assert_eq!(
            dp4.hyprlang(),
            "desc:Samsung Electric Company Odyssey G61SH HNAL600234, 2560x1440@59.95, 6338x2393, 1, transform, 1, bitdepth, 10, vrr, 1"
        );
        assert_eq!(dp4.logical_size(), (1440, 2560));
    }

    #[test]
    fn workspace_rules() {
        let w = WorkspaceRule { workspace: "1".into(), monitor: "desc:G8".into(), default: true, persistent: false };
        assert_eq!(w.lua(), r#"hl.workspace_rule({ workspace = "1", monitor = "desc:G8", default = true })"#);
        assert_eq!(w.hyprlang(), "1, monitor:desc:G8, default:true");
    }

    #[test]
    fn the_file() {
        let all = monitors();
        let rules: Vec<MonitorRule> = all.iter().map(|m| MonitorRule::from_monitor(m, &all)).collect();
        let lua = file(true, Some("Desk + TV"), &rules, &[]);
        assert!(lua.starts_with("-- Written by Lituus"));
        assert!(lua.contains("-- Profile: Desk + TV\n"));
        assert_eq!(lua.matches("hl.monitor(").count(), 4);
        let conf = file(false, None, &rules, &[]);
        assert_eq!(conf.matches("\nmonitor = ").count(), 4);
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(lua_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        assert_eq!(number(1.0), "1");
        assert_eq!(number(4.0 / 3.0), "1.333333");
    }
}
