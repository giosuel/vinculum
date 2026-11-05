//! Keyboards, mice and touchpads: what Hyprland says about them (`hyprctl -j devices`, `getoption`)
//! and the input settings Lituus writes to its file, as `hl.config({ input = … })` and
//! `hl.device({ … })` (Hyprland 0.55 and later) or as hyprlang `input { … }` and `device { … }`.
//! The names are Hyprland 0.57's (`src/config/values/ConfigValues.cpp`, and `DEVICE_FIELDS` in
//! `src/config/lua/bindings/LuaBindingsConfigRules.cpp`).

use serde::Deserialize;

use crate::rules::{lua_string, number};

/// `hyprctl -j devices`, the parts Lituus uses.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Devices {
    /// Every pointer: mice, touchpads, trackpoints, and the pointer part of keyboards and their
    /// receivers. Hyprland doesn't say which is which.
    pub mice: Vec<Pointer>,
    pub keyboards: Vec<Keyboard>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Pointer {
    pub address: String,
    /// Hyprland's name for it, which `hl.device` takes: the kernel's name in lower case, spaces
    /// as dashes ("logitech-mx-master-3s"), and "-1" and so on for a second one of the same name.
    pub name: String,
    pub default_speed: f64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Keyboard {
    pub address: String,
    pub name: String,
    /// The layouts and variants in use, comma-separated: "ch,us" and ",".
    pub layout: String,
    pub variant: String,
    pub options: String,
    /// The layout in use, as a number into `layout`; Hyprland sends "none" when it can't tell.
    #[serde(deserialize_with = "index_or_none")]
    pub active_layout_index: Option<usize>,
    /// The name of the layout in use, as xkeyboard-config describes it: "German (Switzerland)".
    pub active_keymap: String,
    /// The keyboard typed on last.
    pub main: bool,
}

fn index_or_none<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<usize>, D::Error> {
    Ok(serde_json::Value::deserialize(d)?.as_u64().map(|i| i as usize))
}

impl Devices {
    /// `hyprctl -j devices`; Hyprland writes a keyboard's unknown layout index as a bare `none`.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(&json.replace("\"active_layout_index\": none", "\"active_layout_index\": null"))
    }

    /// The keyboard Hyprland treats as the main one, else the first.
    pub fn main_keyboard(&self) -> Option<&Keyboard> {
        self.keyboards.iter().find(|k| k.main).or(self.keyboards.first())
    }
}

/// A keyboard layout and its variant ("ch", ""), as in `kb_layout` and `kb_variant`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub layout: String,
    pub variant: String,
}

/// What Lituus sets for every keyboard: `input.kb_*`, the repeat, and Num Lock.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyboardSettings {
    /// In order: the first is the one in use after login.
    pub layouts: Vec<Layout>,
    /// xkb options, comma-separated, Lituus's and the user's own.
    pub options: String,
    /// Repeats per second.
    pub repeat_rate: i64,
    /// Milliseconds before a held key repeats.
    pub repeat_delay: i64,
    pub numlock: bool,
}

impl Default for KeyboardSettings {
    /// Hyprland's defaults.
    fn default() -> Self {
        Self {
            layouts: vec![Layout { layout: "us".into(), variant: String::new() }],
            options: String::new(),
            repeat_rate: 25,
            repeat_delay: 600,
            numlock: false,
        }
    }
}

impl KeyboardSettings {
    /// `kb_layout` ("ch,us").
    pub fn layout(&self) -> String {
        self.layouts.iter().map(|l| l.layout.as_str()).collect::<Vec<_>>().join(",")
    }

    /// `kb_variant` (",nodeadkeys"): one per layout, even when empty.
    pub fn variant(&self) -> String {
        self.layouts.iter().map(|l| l.variant.as_str()).collect::<Vec<_>>().join(",")
    }

    /// The layouts from `kb_layout` and `kb_variant`.
    pub fn set_layouts(&mut self, layout: &str, variant: &str) {
        let mut variants = variant.split(',').map(str::trim);
        self.layouts = layout
            .split(',')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| Layout { layout: l.to_owned(), variant: variants.next().unwrap_or("").to_owned() })
            .collect();
        // xkb's own default, for an empty `kb_layout`.
        if self.layouts.is_empty() {
            self.layouts = Self::default().layouts;
        }
    }
}

/// What Lituus sets for a mouse: for every mouse (`input.*`, which touchpads share for speed,
/// acceleration and left-handed), or one mouse's own (`hl.device`).
#[derive(Clone, Debug, PartialEq)]
pub struct MouseSettings {
    /// -1 to 1; 0 is libinput's own speed.
    pub sensitivity: f64,
    /// "flat", "adaptive", or "" for libinput's default (adaptive).
    pub accel_profile: String,
    pub natural_scroll: bool,
    /// A multiplier for the wheel.
    pub scroll_factor: f64,
    pub left_handed: bool,
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            sensitivity: 0.0,
            accel_profile: String::new(),
            natural_scroll: false,
            scroll_factor: 1.0,
            left_handed: false,
        }
    }
}

/// What Lituus sets for a touchpad: for every touchpad (`input.touchpad.*`, and the scroll
/// method, which is `input.scroll_method`), or one touchpad's own (with its own speed).
#[derive(Clone, Debug, PartialEq)]
pub struct TouchpadSettings {
    pub natural_scroll: bool,
    pub tap_to_click: bool,
    pub disable_while_typing: bool,
    /// "2fg", "edge", or "" for libinput's default (two fingers).
    pub scroll_method: String,
    /// Click with one, two or three fingers for left, right and middle, instead of the pad's
    /// button areas.
    pub clickfinger: bool,
}

impl Default for TouchpadSettings {
    fn default() -> Self {
        Self {
            natural_scroll: false,
            tap_to_click: true,
            disable_while_typing: true,
            scroll_method: String::new(),
            clickfinger: false,
        }
    }
}

/// A device with its own settings, an `hl.device` block.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    /// Hyprland's name for it.
    pub name: String,
    pub settings: DeviceSettings,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeviceSettings {
    Mouse(MouseSettings),
    /// A touchpad's own speed, and the rest.
    Touchpad(f64, TouchpadSettings),
}

/// Everything in the input part of Lituus's file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputSettings {
    pub keyboard: KeyboardSettings,
    pub mouse: MouseSettings,
    pub touchpad: TouchpadSettings,
    pub devices: Vec<Device>,
}

/// A setting's value, as written.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Number(f64),
    Text(String),
}

impl Value {
    fn bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Number(n) => Some(*n != 0.0),
            Self::Text(t) => match t.as_str() {
                "true" | "yes" | "on" | "1" => Some(true),
                "false" | "no" | "off" | "0" => Some(false),
                _ => None,
            },
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Bool(b) => Some(f64::from(u8::from(*b))),
            Self::Text(t) => t.trim().parse().ok(),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Text(t) => t.clone(),
            Self::Number(n) => number(*n),
            Self::Bool(b) => b.to_string(),
        }
    }

    fn lua(&self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Number(n) => number(*n),
            Self::Text(t) => lua_string(t),
        }
    }

    fn hyprlang(&self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            other => other.text(),
        }
    }
}

type Fields = Vec<(&'static str, Value)>;

fn mouse_fields(m: &MouseSettings) -> Fields {
    let mut f = vec![("sensitivity", Value::Number(m.sensitivity))];
    // Hyprland takes only these names here, so an empty one is left out.
    if !m.accel_profile.is_empty() {
        f.push(("accel_profile", Value::Text(m.accel_profile.clone())));
    }
    f.push(("natural_scroll", Value::Bool(m.natural_scroll)));
    f.push(("scroll_factor", Value::Number(m.scroll_factor)));
    f.push(("left_handed", Value::Bool(m.left_handed)));
    f
}

/// A touchpad's fields, as `input.touchpad` names them, but for the scroll method, which is in
/// `input` (and in a device block).
fn touchpad_fields(t: &TouchpadSettings) -> Fields {
    vec![
        ("natural_scroll", Value::Bool(t.natural_scroll)),
        ("tap_to_click", Value::Bool(t.tap_to_click)),
        ("disable_while_typing", Value::Bool(t.disable_while_typing)),
        ("clickfinger_behavior", Value::Bool(t.clickfinger)),
    ]
}

fn device_fields(d: &DeviceSettings) -> Fields {
    match d {
        DeviceSettings::Mouse(m) => mouse_fields(m),
        DeviceSettings::Touchpad(speed, t) => {
            let mut f = vec![("sensitivity", Value::Number(*speed))];
            f.extend(touchpad_fields(t));
            if !t.scroll_method.is_empty() {
                f.push(("scroll_method", Value::Text(t.scroll_method.clone())));
            }
            f
        }
    }
}

/// `input`'s own fields (keyboard and mouse), and `input.touchpad`'s.
fn input_fields(s: &InputSettings) -> (Fields, Fields) {
    let k = &s.keyboard;
    let mut input = vec![
        ("kb_layout", Value::Text(k.layout())),
        ("kb_variant", Value::Text(k.variant())),
        ("kb_options", Value::Text(k.options.clone())),
        ("repeat_rate", Value::Number(k.repeat_rate as f64)),
        ("repeat_delay", Value::Number(k.repeat_delay as f64)),
        ("numlock_by_default", Value::Bool(k.numlock)),
    ];
    input.extend(mouse_fields(&s.mouse));
    if !s.touchpad.scroll_method.is_empty() {
        input.push(("scroll_method", Value::Text(s.touchpad.scroll_method.clone())));
    }
    (input, touchpad_fields(&s.touchpad))
}

fn get<'a>(fields: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    fields.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn read_mouse(fields: &[(String, Value)], m: &mut MouseSettings) {
    if let Some(v) = get(fields, "sensitivity").and_then(Value::number) {
        m.sensitivity = v;
    }
    if let Some(v) = get(fields, "accel_profile") {
        m.accel_profile = v.text();
    }
    if let Some(v) = get(fields, "natural_scroll").and_then(Value::bool) {
        m.natural_scroll = v;
    }
    if let Some(v) = get(fields, "scroll_factor").and_then(Value::number) {
        m.scroll_factor = v;
    }
    if let Some(v) = get(fields, "left_handed").and_then(Value::bool) {
        m.left_handed = v;
    }
}

fn read_touchpad(fields: &[(String, Value)], t: &mut TouchpadSettings) {
    for (key, on) in [
        ("natural_scroll", &mut t.natural_scroll),
        ("tap_to_click", &mut t.tap_to_click),
        ("disable_while_typing", &mut t.disable_while_typing),
        ("clickfinger_behavior", &mut t.clickfinger),
    ] {
        if let Some(v) = get(fields, key).and_then(Value::bool) {
            *on = v;
        }
    }
}

impl InputSettings {
    /// The input part of Lituus's file: a comment, then `hl.config` and the `hl.device` blocks
    /// (Lua), or `input` and `device` blocks (hyprlang).
    pub fn file(&self, lua: bool) -> String {
        let comment = if lua { "--" } else { "#" };
        let mut out =
            format!("{comment} Written by Lituus (Input). Change it in Lituus: edits made here are overwritten.\n\n");
        if lua {
            out += &self.lua_config(true);
            for d in &self.devices {
                out += "\n";
                out += &device_lua(d, true);
                out += "\n";
            }
        } else {
            let (input, touchpad) = input_fields(self);
            out += "input {\n";
            for (k, v) in &input {
                out += &format!("    {k} = {}\n", v.hyprlang());
            }
            out += "    touchpad {\n";
            for (k, v) in &touchpad {
                out += &format!("        {} = {}\n", hyprlang_name(k), v.hyprlang());
            }
            out += "    }\n}\n";
            for d in &self.devices {
                out += &format!("\ndevice {{\n    name = {}\n", d.name);
                for (k, v) in device_fields(&d.settings) {
                    out += &format!("    {} = {}\n", hyprlang_name(k), v.hyprlang());
                }
                out += "}\n";
            }
        }
        out
    }

    /// `hl.config({ input = { … } })`, over lines for the file or on one line to apply live.
    fn lua_config(&self, pretty: bool) -> String {
        let (input, touchpad) = input_fields(self);
        let fields = |f: &Fields, indent: &str| -> Vec<String> {
            f.iter().map(|(k, v)| format!("{indent}{k} = {},", v.lua())).collect()
        };
        if pretty {
            let mut lines = vec!["hl.config({".to_owned(), "    input = {".to_owned()];
            lines.extend(fields(&input, "        "));
            lines.push("        touchpad = {".to_owned());
            lines.extend(fields(&touchpad, "            "));
            lines.extend(["        },".to_owned(), "    },".to_owned(), "})".to_owned()]);
            lines.join("\n") + "\n"
        } else {
            let one = |f: &Fields| f.iter().map(|(k, v)| format!("{k} = {}", v.lua())).collect::<Vec<_>>().join(", ");
            format!("hl.config({{ input = {{ {}, touchpad = {{ {} }} }} }})", one(&input), one(&touchpad))
        }
    }

    /// What to run to apply the settings live: `hyprctl eval` takes one statement per request.
    /// Every `input` setting at once, so the ones Hyprland only reads when a keyboard is set up
    /// again (the repeat delay) come along with ones that set it up again.
    pub fn lua_live(&self) -> Vec<String> {
        let mut out = vec![self.lua_config(false)];
        out.extend(self.devices.iter().map(|d| device_lua(d, false)));
        out
    }

    /// The same for Hyprland before 0.55: `hyprctl keyword` (name, value) pairs.
    pub fn hyprlang_live(&self) -> Vec<(String, String)> {
        let (input, touchpad) = input_fields(self);
        let mut out: Vec<(String, String)> = input.iter().map(|(k, v)| (format!("input:{k}"), v.hyprlang())).collect();
        out.extend(touchpad.iter().map(|(k, v)| (format!("input:touchpad:{}", hyprlang_name(k)), v.hyprlang())));
        for d in &self.devices {
            out.extend(
                device_fields(&d.settings)
                    .iter()
                    .map(|(k, v)| (format!("device[{}]:{}", d.name, hyprlang_name(k)), v.hyprlang())),
            );
        }
        out
    }

    /// The settings in a file `file` wrote; what it doesn't have keeps Hyprland's defaults.
    pub fn parse(text: &str, lua: bool) -> Self {
        let (input, devices) = if lua { parse_lua(text) } else { parse_hyprlang(text) };
        Self::from_fields(&input, &devices)
    }

    /// From `input`'s fields ("touchpad.tap_to_click" for `input.touchpad`'s) and the devices'.
    pub fn from_fields(input: &[(String, Value)], devices: &[(String, Vec<(String, Value)>)]) -> Self {
        let mut s = Self::default();
        let k = &mut s.keyboard;
        let text = |key: &str| get(input, key).map(Value::text);
        if let Some(layout) = text("kb_layout") {
            k.set_layouts(&layout, &text("kb_variant").unwrap_or_default());
        }
        if let Some(options) = text("kb_options") {
            k.options = options;
        }
        if let Some(v) = get(input, "repeat_rate").and_then(Value::number) {
            k.repeat_rate = v.round() as i64;
        }
        if let Some(v) = get(input, "repeat_delay").and_then(Value::number) {
            k.repeat_delay = v.round() as i64;
        }
        if let Some(v) = get(input, "numlock_by_default").and_then(Value::bool) {
            k.numlock = v;
        }
        read_mouse(input, &mut s.mouse);
        let touchpad: Vec<(String, Value)> = input
            .iter()
            .filter_map(|(key, v)| key.strip_prefix("touchpad.").map(|k| (lua_name(k), v.clone())))
            .collect();
        read_touchpad(&touchpad, &mut s.touchpad);
        if let Some(v) = text("scroll_method") {
            s.touchpad.scroll_method = v;
        }
        s.devices = devices
            .iter()
            .map(|(name, fields)| {
                let fields: Vec<(String, Value)> = fields.iter().map(|(k, v)| (lua_name(k), v.clone())).collect();
                // A touchpad's block has the touchpad fields: Lituus writes all of them.
                let settings = if get(&fields, "tap_to_click").is_some() {
                    let mut t = TouchpadSettings::default();
                    read_touchpad(&fields, &mut t);
                    if let Some(v) = get(&fields, "scroll_method") {
                        t.scroll_method = v.text();
                    }
                    let speed = get(&fields, "sensitivity").and_then(Value::number).unwrap_or(0.0);
                    DeviceSettings::Touchpad(speed, t)
                } else {
                    let mut m = MouseSettings::default();
                    read_mouse(&fields, &mut m);
                    DeviceSettings::Mouse(m)
                };
                Device { name: name.clone(), settings }
            })
            .collect();
        s
    }
}

/// `hl.device({ name = …, … })`.
fn device_lua(d: &Device, pretty: bool) -> String {
    let mut fields = vec![format!("name = {}", lua_string(&d.name))];
    fields.extend(device_fields(&d.settings).iter().map(|(k, v)| format!("{k} = {}", v.lua())));
    if pretty {
        format!("hl.device({{\n    {},\n}})", fields.join(",\n    "))
    } else {
        format!("hl.device({{ {} }})", fields.join(", "))
    }
}

/// hyprlang spells two of the touchpad settings with dashes.
fn hyprlang_name(key: &str) -> String {
    match key {
        "tap_to_click" => "tap-to-click".into(),
        "tap_and_drag" => "tap-and-drag".into(),
        k => k.into(),
    }
}

fn lua_name(key: &str) -> String {
    key.replace('-', "_")
}

/// The `input` options Lituus sets, as `from_fields` names them; `getoption` takes them as
/// `option_name` spells them.
pub const OPTIONS: [&str; 16] = [
    "kb_layout",
    "kb_variant",
    "kb_options",
    "repeat_rate",
    "repeat_delay",
    "numlock_by_default",
    "sensitivity",
    "accel_profile",
    "natural_scroll",
    "scroll_factor",
    "left_handed",
    "scroll_method",
    "touchpad.natural_scroll",
    "touchpad.tap_to_click",
    "touchpad.disable_while_typing",
    "touchpad.clickfinger_behavior",
];

/// "input:touchpad:tap-to-click", as `getoption` takes an option of `OPTIONS`.
pub fn option_name(option: &str) -> String {
    format!("input:{}", option.replace('.', ":").replace("tap_to_click", "tap-to-click"))
}

/// `getoption`'s answer as a value: `{"option": …, "int": 25, "set": false}`, or "str", "float",
/// "bool". Hyprland writes an empty string as "[[EMPTY]]".
pub fn option_value(answer: &str) -> Option<Value> {
    let v: serde_json::Value = serde_json::from_str(answer).ok()?;
    if let Some(s) = v.get("str").and_then(|s| s.as_str()) {
        return Some(Value::Text(if s == "[[EMPTY]]" { String::new() } else { s.to_owned() }));
    }
    if let Some(b) = v.get("bool").and_then(serde_json::Value::as_bool) {
        return Some(Value::Bool(b));
    }
    v.get("int").or_else(|| v.get("float")).and_then(serde_json::Value::as_f64).map(Value::Number)
}

// The file's own syntax, read back: Lua table literals in `hl.config(…)` and `hl.device(…)`.

#[derive(Debug)]
enum Lua {
    Value(Value),
    Table(Vec<(String, Lua)>),
}

struct LuaReader<'a> {
    s: &'a [u8],
    i: usize,
}

impl LuaReader<'_> {
    fn space(&mut self) {
        loop {
            while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                while self.s.get(self.i).is_some_and(|c| *c != b'\n') {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.space();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn word(&mut self) -> String {
        self.space();
        let start = self.i;
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'.' || *c == b'-') {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn string(&mut self, quote: u8) -> Option<String> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            match *self.s.get(self.i)? {
                c if c == quote => {
                    self.i += 1;
                    return Some(String::from_utf8_lossy(&out).into_owned());
                }
                b'\\' => {
                    self.i += 1;
                    out.push(match *self.s.get(self.i)? {
                        b'n' => b'\n',
                        c => c,
                    });
                }
                c => out.push(c),
            }
            self.i += 1;
        }
    }

    fn value(&mut self) -> Option<Lua> {
        self.space();
        match *self.s.get(self.i)? {
            b'{' => self.table(),
            q @ (b'"' | b'\'') => self.string(q).map(|s| Lua::Value(Value::Text(s))),
            _ => {
                let w = self.word();
                Some(Lua::Value(match w.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    w => Value::Number(w.parse().ok()?),
                }))
            }
        }
    }

    fn table(&mut self) -> Option<Lua> {
        if !self.eat(b'{') {
            return None;
        }
        let mut fields = Vec::new();
        loop {
            if self.eat(b'}') {
                return Some(Lua::Table(fields));
            }
            let key = if self.eat(b'[') {
                self.space();
                let q = *self.s.get(self.i)?;
                let k = self.string(q)?;
                self.eat(b']');
                k
            } else {
                self.word()
            };
            if key.is_empty() || !self.eat(b'=') {
                return None;
            }
            fields.push((key, self.value()?));
            if !self.eat(b',') && !self.eat(b';') {
                return self.eat(b'}').then_some(Lua::Table(fields));
            }
        }
    }
}

/// The fields under a table, flattened: `touchpad = { tap_to_click = true }` as
/// "touchpad.tap_to_click".
fn flatten(prefix: &str, fields: Vec<(String, Lua)>, out: &mut Vec<(String, Value)>) {
    for (k, v) in fields {
        let key = if prefix.is_empty() { k } else { format!("{prefix}.{k}") };
        match v {
            Lua::Value(v) => out.push((key, v)),
            Lua::Table(t) => flatten(&key, t, out),
        }
    }
}

type Parsed = (Vec<(String, Value)>, Vec<(String, Vec<(String, Value)>)>);

fn parse_lua(text: &str) -> Parsed {
    let mut input = Vec::new();
    let mut devices = Vec::new();
    for (i, _) in text.match_indices("hl.") {
        let rest = &text[i..];
        let (name, args) = if let Some(a) = rest.strip_prefix("hl.config(") {
            ("config", a)
        } else if let Some(a) = rest.strip_prefix("hl.device(") {
            ("device", a)
        } else {
            continue;
        };
        let Some(Lua::Table(fields)) = (LuaReader { s: args.as_bytes(), i: 0 }).table() else { continue };
        let mut flat = Vec::new();
        flatten("", fields, &mut flat);
        if name == "config" {
            input.extend(flat.into_iter().filter_map(|(k, v)| k.strip_prefix("input.").map(|k| (k.to_owned(), v))));
        } else if let Some(device) = get(&flat, "name").map(Value::text) {
            devices.push((device, flat.into_iter().filter(|(k, _)| k != "name").collect()));
        }
    }
    (input, devices)
}

fn parse_hyprlang(text: &str) -> Parsed {
    let mut input = Vec::new();
    let mut devices: Vec<(String, Vec<(String, Value)>)> = Vec::new();
    // The blocks we're in: "input", "touchpad", "device".
    let mut blocks: Vec<String> = Vec::new();
    let mut device: Option<(String, Vec<(String, Value)>)> = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(block) = line.strip_suffix('{') {
            blocks.push(block.trim().to_owned());
            if block.trim() == "device" {
                device = Some((String::new(), Vec::new()));
            }
            continue;
        }
        if line == "}" {
            if blocks.pop().as_deref() == Some("device")
                && let Some(d) = device.take()
                && !d.0.is_empty()
            {
                devices.push(d);
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), Value::Text(value.trim().to_owned()));
        let path = blocks.iter().map(String::as_str).chain([key]).collect::<Vec<_>>().join(":");
        if let Some(d) = device.as_mut() {
            if key == "name" {
                d.0 = value.text();
            } else {
                d.1.push((key.to_owned(), value));
            }
        } else if let Some(k) = path.strip_prefix("input:") {
            input.push((lua_name(&k.replace(':', ".")), value));
        }
    }
    (input, devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field different from its default and from the others of its kind, so a swap shows.
    fn settings() -> InputSettings {
        InputSettings {
            keyboard: KeyboardSettings {
                layouts: vec![
                    Layout { layout: "ch".into(), variant: String::new() },
                    Layout { layout: "us".into(), variant: "intl".into() },
                ],
                options: "grp:win_space_toggle,ctrl:nocaps,lv3:ralt_switch".into(),
                repeat_rate: 33,
                repeat_delay: 280,
                numlock: true,
            },
            mouse: MouseSettings {
                sensitivity: -0.25,
                accel_profile: "flat".into(),
                natural_scroll: true,
                scroll_factor: 1.5,
                left_handed: true,
            },
            touchpad: TouchpadSettings {
                natural_scroll: true,
                tap_to_click: false,
                disable_while_typing: false,
                scroll_method: "edge".into(),
                clickfinger: true,
            },
            devices: vec![
                Device {
                    name: "logitech-mx-master-3s".into(),
                    settings: DeviceSettings::Mouse(MouseSettings {
                        sensitivity: 0.4,
                        accel_profile: "adaptive".into(),
                        natural_scroll: false,
                        scroll_factor: 0.7,
                        left_handed: false,
                    }),
                },
                Device {
                    name: "elan0670:00-04f3:3150-touchpad".into(),
                    settings: DeviceSettings::Touchpad(
                        0.15,
                        TouchpadSettings {
                            natural_scroll: false,
                            tap_to_click: true,
                            disable_while_typing: true,
                            scroll_method: "2fg".into(),
                            clickfinger: false,
                        },
                    ),
                },
            ],
        }
    }

    #[test]
    fn the_file_reads_back_as_written() {
        let s = settings();
        for lua in [true, false] {
            let text = s.file(lua);
            assert_eq!(InputSettings::parse(&text, lua), s, "{text}");
        }
        let lua = s.file(true);
        assert!(lua.starts_with("-- Written by Lituus (Input)."));
        assert!(lua.contains("        kb_layout = \"ch,us\",\n        kb_variant = \",intl\",\n"), "{lua}");
        assert!(lua.contains("            tap_to_click = false,\n"), "{lua}");
        assert!(lua.contains("hl.device({\n    name = \"logitech-mx-master-3s\",\n    sensitivity = 0.4,\n"), "{lua}");
        let conf = s.file(false);
        assert!(conf.contains("    kb_options = grp:win_space_toggle,ctrl:nocaps,lv3:ralt_switch\n"), "{conf}");
        assert!(conf.contains("        tap-to-click = false\n"), "{conf}");
        assert!(
            conf.contains("device {\n    name = elan0670:00-04f3:3150-touchpad\n    sensitivity = 0.15\n"),
            "{conf}"
        );
    }

    #[test]
    fn defaults_are_hyprlands() {
        assert_eq!(InputSettings::parse("", true), InputSettings::default());
        let d = InputSettings::default();
        assert_eq!((d.keyboard.layout(), d.keyboard.repeat_delay, d.keyboard.repeat_rate), ("us".into(), 600, 25));
        // Empty choices aren't written: Hyprland takes only its names for them.
        let text = d.file(true);
        assert!(!text.contains("accel_profile") && !text.contains("scroll_method"), "{text}");
    }

    #[test]
    fn live_forms() {
        let s = settings();
        let live = s.lua_live();
        assert_eq!(live.len(), 3);
        assert!(
            live[0].starts_with("hl.config({ input = { kb_layout = \"ch,us\", kb_variant = \",intl\""),
            "{}",
            live[0]
        );
        assert!(live[0].ends_with("touchpad = { natural_scroll = true, tap_to_click = false, disable_while_typing = false, clickfinger_behavior = true } } })"), "{}", live[0]);
        assert_eq!(
            live[1],
            "hl.device({ name = \"logitech-mx-master-3s\", sensitivity = 0.4, accel_profile = \"adaptive\", natural_scroll = false, scroll_factor = 0.7, left_handed = false })"
        );
        let keywords = s.hyprlang_live();
        assert!(keywords.contains(&("input:repeat_delay".into(), "280".into())));
        assert!(keywords.contains(&("input:touchpad:tap-to-click".into(), "false".into())));
        assert!(keywords.contains(&("device[elan0670:00-04f3:3150-touchpad]:scroll_method".into(), "2fg".into())));
    }

    #[test]
    fn lua_written_by_hand_reads_too() {
        let text = r#"
            -- a comment
            hl.config({ input = { kb_layout = 'de', ["repeat_rate"] = 40; touchpad = { tap_to_click = false } } })
            hl.monitor({ output = "DP-1", disabled = true })
            hl.device({ name = "my-mouse", sensitivity = -1 })
        "#;
        let s = InputSettings::parse(text, true);
        assert_eq!(s.keyboard.layouts, [Layout { layout: "de".into(), variant: String::new() }]);
        assert_eq!(s.keyboard.repeat_rate, 40);
        assert!(!s.touchpad.tap_to_click);
        assert_eq!(s.devices.len(), 1);
        assert_eq!(
            s.devices[0].settings,
            DeviceSettings::Mouse(MouseSettings { sensitivity: -1.0, ..MouseSettings::default() })
        );
    }

    #[test]
    fn options_from_getoption() {
        assert_eq!(
            option_value(r#"{"option": "input:kb_layout", "str": "ch,us", "set": true }"#),
            Some(Value::Text("ch,us".into()))
        );
        assert_eq!(
            option_value(r#"{"option": "input:kb_variant", "str": "[[EMPTY]]", "set": false }"#),
            Some(Value::Text(String::new()))
        );
        assert_eq!(
            option_value(r#"{"option": "input:repeat_rate", "int": 25, "set": false }"#),
            Some(Value::Number(25.0))
        );
        assert_eq!(
            option_value(r#"{"option": "input:sensitivity", "float": -0.300000, "set": true }"#),
            Some(Value::Number(-0.3))
        );
        assert_eq!(
            option_value(r#"{"option": "input:left_handed", "bool": true, "set": true }"#),
            Some(Value::Bool(true))
        );
        assert_eq!(option_value("no such option"), None);
    }

    #[test]
    fn reads_hyprctl_devices() {
        let d = Devices::parse(include_str!("../tests/fixtures/devices.json")).unwrap();
        assert_eq!(d.mice.len(), 2);
        let main = d.main_keyboard().unwrap();
        assert_eq!((main.layout.as_str(), main.active_layout_index), ("ch,us", Some(0)));
        assert_eq!(main.active_keymap, "German (Switzerland)");
        assert_eq!(d.keyboards.iter().find(|k| k.name == "power-button").unwrap().active_layout_index, Some(0));
        assert!(d.keyboards.iter().any(|k| k.active_layout_index.is_none()));
    }
}
