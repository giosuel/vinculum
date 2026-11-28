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
        let mirror = all.iter().find(|o| o.id.to_string() == m.mirror_of).map(|o| o.selector(all));
        Self {
            output: m.selector(all),
            disabled: m.disabled,
            mode: m.mode(),
            x: m.x,
            y: m.y,
            scale: m.exact_scale(),
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

    /// `hl.monitor({ … })`, for the file. An enabled monitor says `disabled = false`: Hyprland
    /// merges the rules for one output, so without it a rule loaded earlier that turns the
    /// monitor off (another tool's monitors.lua) keeps it off.
    pub fn lua(&self) -> String {
        self.lua_rule(None)
    }

    /// `hl.monitor({ … })` to apply live. Hyprland starts the new rule from the monitor's
    /// earlier one and changes only the fields given, so this names every field, those back at
    /// their defaults too (a mirror, the SDR settings, the reserved area). `vrr_global` is what
    /// stands for "the global setting": -1 from Hyprland 0.56 on; 0.55 takes only 0–3, so there
    /// it's misc:vrr's value now (none: left out).
    pub fn lua_live(&self, vrr_global: Option<i8>) -> String {
        self.lua_rule(Some(vrr_global))
    }

    fn lua_rule(&self, live: Option<Option<i8>>) -> String {
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
        if live.is_some() || (self.sdr_brightness - 1.0).abs() > 1e-6 || (self.sdr_saturation - 1.0).abs() > 1e-6 {
            fields.push(format!("sdrbrightness = {}", number(self.sdr_brightness)));
            fields.push(format!("sdrsaturation = {}", number(self.sdr_saturation)));
        }
        match (self.vrr, live) {
            (Some(vrr), _) => fields.push(format!("vrr = {vrr}")),
            (None, Some(Some(global))) => fields.push(format!("vrr = {global}")),
            _ => {}
        }
        match &self.mirror {
            Some(mirror) => fields.push(format!("mirror = {}", lua_string(mirror))),
            None if live.is_some() => fields.push("mirror = \"\"".into()),
            None => {}
        }
        fields.push("disabled = false".into());
        if live.is_some() || self.reserved != [0; 4] {
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

    /// The reserved area for Hyprland before 0.55: a `monitor = …, addreserved, …` of its own
    /// after the rule (top, bottom, left, right), which sets it on the rule of that name. A
    /// rule given again starts without one, so it's always sent live (zeros take one away), and
    /// written only when it isn't all zeros.
    pub fn hyprlang_reserved(&self) -> Option<String> {
        let [t, r, b, l] = self.reserved;
        (!self.disabled).then(|| format!("{}, addreserved, {t}, {b}, {l}, {r}", self.output))
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
        self.lua_rule(false)
    }

    /// `hl.workspace_rule({ … })` to apply live: Hyprland merges it into the rule it has for
    /// the workspace and keeps what it leaves out, so "not the default" and "not persistent"
    /// are said too.
    pub fn lua_live(&self) -> String {
        self.lua_rule(true)
    }

    fn lua_rule(&self, live: bool) -> String {
        let mut fields = vec![
            format!("workspace = {}", lua_string(&self.workspace)),
            format!("monitor = {}", lua_string(&self.monitor)),
        ];
        if self.default || live {
            fields.push(format!("default = {}", self.default));
        }
        if self.persistent || live {
            fields.push(format!("persistent = {}", self.persistent));
        }
        format!("hl.workspace_rule({{ {} }})", fields.join(", "))
    }

    /// Lua that turns off the rule Hyprland has for `workspace` (0.56 and later: the rule
    /// `hl.workspace_rule` merges into, and gives back, can be turned off; a rule without a
    /// monitor wouldn't unbind it).
    pub fn lua_unbind(workspace: &str) -> String {
        format!("hl.workspace_rule({{ workspace = {} }}):set_enabled(false)", lua_string(workspace))
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
        // A line break in the name would end the comment, and the rest would run.
        out += &format!("{comment} Profile: {}\n", p.replace(char::is_control, " "));
    }
    out += "\n";
    for m in monitors {
        out += &if lua { m.lua() } else { format!("monitor = {}", m.hyprlang()) };
        out += "\n";
        if let Some(reserved) = m.hyprlang_reserved().filter(|_| !lua && m.reserved != [0; 4]) {
            out += &format!("monitor = {reserved}\n");
        }
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

/// What a monitor rule in Lituus's file sets that Hyprland doesn't report back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreported {
    pub output: String,
    pub vrr: Option<u8>,
    pub reserved: [i32; 4],
}

/// The VRR setting and reserved area of each monitor rule in a file `file` wrote.
pub fn unreported(text: &str) -> Vec<Unreported> {
    let mut out: Vec<Unreported> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rule) = line.strip_prefix("hl.monitor({") {
            let Some(output) = rule.split_once("output = ").and_then(|(_, o)| lua_unquote(o)) else { continue };
            let vrr = rule.split_once(", vrr = ").and_then(|(_, v)| leading_number(v)).map(|v| v as u8);
            let mut reserved = [0; 4];
            if let Some((_, area)) = rule.split_once("reserved_area = {") {
                for (side, value) in ["top", "right", "bottom", "left"].iter().zip(&mut reserved) {
                    if let Some((_, v)) = area.split_once(&format!("{side} = ")) {
                        *value = leading_number(v).unwrap_or(0);
                    }
                }
            }
            out.push(Unreported { output, vrr, reserved });
        } else if let Some(rule) = line.strip_prefix("monitor = ") {
            let fields: Vec<&str> = rule.split(", ").collect();
            let Some(output) = fields.first().map(|o| o.to_string()) else { continue };
            if fields.get(1) == Some(&"addreserved") {
                // Top, bottom, left, right; for the rule before it.
                let n = |i: usize| fields.get(i).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                if let Some(u) = out.iter_mut().rev().find(|u| u.output == output) {
                    u.reserved = [n(2), n(5), n(3), n(4)];
                }
                continue;
            }
            let vrr = fields.iter().position(|f| *f == "vrr").and_then(|i| fields.get(i + 1)?.parse().ok());
            out.push(Unreported { output, vrr, reserved: [0; 4] });
        }
    }
    out
}

/// The number at the start of `s`.
fn leading_number(s: &str) -> Option<i32> {
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '-')).unwrap_or(s.len());
    s[..end].parse().ok()
}

/// The Lua string literal at the start of `s`, unescaped.
fn lua_unquote(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.chars().peekable();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                d @ '0'..='9' => {
                    let mut code = d.to_digit(10).unwrap_or(0);
                    for _ in 0..2 {
                        match chars.peek().and_then(|c| c.to_digit(10)) {
                            Some(n) => {
                                code = code * 10 + n;
                                chars.next();
                            }
                            None => break,
                        }
                    }
                    out.extend(char::from_u32(code));
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
}

/// A Lua string literal. Control characters are escaped: a carriage return pasted into a
/// field would end the line, and with it the string and the whole file.
pub(crate) fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            '\n' => out += "\\n",
            '\r' => out += "\\r",
            '\t' => out += "\\t",
            c if c.is_ascii_control() => out += &format!("\\{:03}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 1.5, 1, 1.333333: as short as it goes.
pub(crate) fn number(n: f64) -> String {
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
            r#"hl.monitor({ output = "desc:Samsung Electric Company Odyssey G81SF HNAY500052", mode = "3840x2160@240", position = "3778x2393", scale = "1.5", transform = 0, bitdepth = 8, cm = "srgb", disabled = false })"#
        );
        g8.ten_bit = true;
        g8.cm = "hdr".into();
        g8.sdr_brightness = 1.2;
        g8.sdr_saturation = 0.98;
        g8.vrr = Some(2);
        g8.mirror = Some("DP-3".into());
        assert_eq!(
            g8.lua(),
            r#"hl.monitor({ output = "desc:Samsung Electric Company Odyssey G81SF HNAY500052", mode = "3840x2160@240", position = "3778x2393", scale = "1.5", transform = 0, bitdepth = 10, cm = "hdr", sdrbrightness = 1.2, sdrsaturation = 0.98, vrr = 2, mirror = "DP-3", disabled = false })"#
        );
        assert!(g8.lua_live(Some(-1)).contains(r#"mirror = "DP-3", disabled = false"#));
        g8.mirror = None;
        assert!(g8.lua_live(Some(-1)).contains(r#"mirror = "", disabled = false"#));
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

    /// Hyprland starts a live rule from the monitor's earlier one, so the live form names the
    /// settings that are back at their defaults too (an Apply, Go back now, the timeout).
    #[test]
    fn the_live_rule_also_sets_what_went_back_to_its_default() {
        let all = monitors();
        let rule = MonitorRule::from_monitor(&all[1], &all);
        let live = rule.lua_live(Some(-1));
        assert!(live.contains("sdrbrightness = 1, sdrsaturation = 1"), "{live}");
        assert!(live.contains("reserved_area = { top = 0, right = 0, bottom = 0, left = 0 }"), "{live}");
        // "Global setting": -1 from 0.56 on, misc:vrr's value on 0.55.
        assert!(live.contains("vrr = -1"), "{live}");
        assert!(rule.lua_live(Some(2)).contains("vrr = 2"));
        assert!(!rule.lua_live(None).contains("vrr"));
        // The file leaves the defaults out.
        assert!(!rule.lua().contains("sdrbrightness") && !rule.lua().contains("reserved_area"));
    }

    #[test]
    fn workspace_rules() {
        let w = WorkspaceRule { workspace: "1".into(), monitor: "desc:G8".into(), default: true, persistent: false };
        assert_eq!(w.lua(), r#"hl.workspace_rule({ workspace = "1", monitor = "desc:G8", default = true })"#);
        assert_eq!(w.hyprlang(), "1, monitor:desc:G8, default:true");
        assert_eq!(WorkspaceRule::lua_unbind("3"), r#"hl.workspace_rule({ workspace = "3" }):set_enabled(false)"#);
    }

    /// Hyprland merges a workspace rule into the one it has for the workspace and keeps what
    /// the new one leaves out (CWorkspaceRule::mergeLeft), so the live form says what's off.
    #[test]
    fn a_live_workspace_rule_says_what_was_turned_off() {
        let rule = WorkspaceRule { workspace: "2".into(), monitor: "DP-3".into(), default: false, persistent: false };
        assert_eq!(
            rule.lua_live(),
            r#"hl.workspace_rule({ workspace = "2", monitor = "DP-3", default = false, persistent = false })"#
        );
    }

    /// Before 0.55 the reserved area is an `addreserved` line after the rule, read back with it.
    #[test]
    fn hyprlang_has_a_reserved_area() {
        let all = monitors();
        let mut rules: Vec<MonitorRule> = all.iter().map(|m| MonitorRule::from_monitor(m, &all)).collect();
        rules[1].reserved = [40, 0, 10, 12];
        assert_eq!(
            rules[1].hyprlang_reserved().as_deref(),
            Some("desc:Samsung Electric Company Odyssey G81SF HNAY500052, addreserved, 40, 10, 12, 0")
        );
        let conf = file(false, None, &rules, &[]);
        assert_eq!(conf.matches("addreserved").count(), 1, "{conf}");
        assert_eq!(unreported(&conf)[1].reserved, [40, 0, 10, 12]);
        rules[1].disabled = true;
        assert_eq!(rules[1].hyprlang_reserved(), None);
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
    fn what_hyprland_doesnt_report_comes_back_from_the_file() {
        let all = monitors();
        let mut rules: Vec<MonitorRule> = all.iter().map(|m| MonitorRule::from_monitor(m, &all)).collect();
        rules[1].vrr = Some(2);
        rules[1].reserved = [40, 0, 0, 12];
        rules[2].output = r#"a "quoted" \ name"#.into();
        for lua in [true, false] {
            let back = unreported(&file(lua, Some("Desk"), &rules, &[]));
            assert_eq!(back.len(), 4);
            assert_eq!(back[0], Unreported { output: rules[0].output.clone(), vrr: None, reserved: [0; 4] });
            assert_eq!(back[1], Unreported { output: rules[1].output.clone(), vrr: Some(2), reserved: [40, 0, 0, 12] });
            assert_eq!(back[2].output, rules[2].output);
        }
    }

    #[test]
    fn strings_are_escaped() {
        assert_eq!(lua_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
        // A carriage return would end the line in Lua, and the string with it.
        assert_eq!(lua_string("a\rb\u{1}c"), r#""a\rb\001c""#);
        assert_eq!(lua_unquote(&lua_string("a\rb\u{1}c\t\"")).as_deref(), Some("a\rb\u{1}c\t\""));
        let profile = file(true, Some("Desk\r\nos.exit()"), &[], &[]);
        assert!(profile.contains("-- Profile: Desk  os.exit()\n"), "{profile}");
        assert_eq!(number(1.0), "1");
        assert_eq!(number(4.0 / 3.0), "1.333333");
    }
}
