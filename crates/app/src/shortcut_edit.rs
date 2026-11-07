//! Changing shortcuts: what Lituus keeps in the Shortcuts part of its file (the config's binds it
//! changes or turns off, and binds of its own), and the edit dialog's actions, each made into a
//! Hyprland dispatcher and read back from one.
//!
//! Lituus never edits the user's config. It overrides a bind with `hl.unbind` and, for a change,
//! a new `hl.bind` on the same keys; that only takes effect when Lituus's file loads after the
//! config's binds (the page says so when it doesn't).

use std::path::Path;

use lituus_hypr::{Action, Bind, Flags, LuaValue as Value, lua_string, normalized_keys, parse_lua};

/// The name of Lituus's part of its file.
pub const SECTION: &str = "Shortcuts";

/// One thing in Lituus's part.
#[derive(Clone, Debug, PartialEq)]
pub enum Mine {
    /// The config's binds on `keys` are unbound, and `bind` takes their place (none: they're
    /// turned off).
    Over { keys: String, bind: Option<Bind> },
    /// A bind of Lituus's own; turned off, it stays in the file as a comment.
    Added { bind: Bind, on: bool },
}

impl Mine {
    /// The keys, as Hyprland compares them.
    pub fn keys(&self) -> String {
        match self {
            Mine::Over { keys, .. } => normalized_keys(keys),
            Mine::Added { bind, .. } => normalized_keys(&bind.keys()),
        }
    }
}

/// How a turned-off bind of Lituus's own is kept: commented out, after this.
const OFF: &str = "-- Off: ";

/// Lituus's part, read back: an unbind with a bind on the same keys after it is a change; an
/// unbind alone turns the config's bind off; any other bind is Lituus's own.
pub fn parse(text: &str, file: &Path) -> Vec<Mine> {
    let config = parse_lua(text, file);
    let mut binds: Vec<Option<Bind>> = config.binds.into_iter().map(Some).collect();
    let mut mine = Vec::new();
    for u in &config.unbinds {
        let keys = normalized_keys(&u.keys);
        let bind = binds
            .iter_mut()
            .find(|b| b.as_ref().is_some_and(|b| normalized_keys(&b.keys()) == keys))
            .and_then(Option::take);
        mine.push(Mine::Over { keys: u.keys.clone(), bind });
    }
    mine.extend(binds.into_iter().flatten().map(|bind| Mine::Added { bind, on: true }));
    for line in text.lines() {
        if let Some(lua) = line.trim().strip_prefix(OFF) {
            mine.extend(parse_lua(lua, file).binds.into_iter().map(|bind| Mine::Added { bind, on: false }));
        }
    }
    mine
}

/// The text of Lituus's part for these: the overrides first, then Lituus's own.
pub fn text(mine: &[Mine]) -> String {
    let mut out = String::from(
        "-- Written by Lituus (Shortcuts). Change it in Lituus.\n\
         -- Your config's shortcuts that Lituus changes or turns off, then its own. They take effect\n\
         -- when this file loads after your shortcuts.\n",
    );
    for m in mine {
        if let Mine::Over { keys, bind } = m {
            out.push('\n');
            out.push_str(&format!("hl.unbind({})\n", lua_string(keys)));
            if let Some(lua) = bind.as_ref().and_then(Bind::to_lua) {
                out.push_str(&lua);
                out.push('\n');
            }
        }
    }
    let own: Vec<String> = mine
        .iter()
        .filter_map(|m| match m {
            Mine::Added { bind, on } => bind.to_lua().map(|lua| if *on { lua } else { format!("{OFF}{lua}") }),
            Mine::Over { .. } => None,
        })
        .collect();
    if !own.is_empty() {
        out.push('\n');
        for lua in own {
            out.push_str(&lua);
            out.push('\n');
        }
    }
    out
}

// ---- the dialog's actions ----------------------------------------------------------------------

/// What an action needs besides itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    None,
    Command,
    /// A workspace; moving a window to one also asks whether to follow it.
    Workspace,
    Direction,
    Special,
}

/// The actions the dialog offers, as (id, label, what it needs).
pub const KINDS: [(&str, &str, Field); 22] = [
    ("command", "Run a command", Field::Command),
    ("close", "Close window", Field::None),
    ("kill", "Force-quit the app", Field::None),
    ("float", "Float or tile window", Field::None),
    ("fullscreen", "Fullscreen", Field::None),
    ("maximise", "Maximise", Field::None),
    ("pin", "Pin to every workspace", Field::None),
    ("pseudo", "Pseudotile window", Field::None),
    ("center", "Centre window", Field::None),
    ("split", "Switch split direction", Field::None),
    ("move-ws", "Move window to workspace", Field::Workspace),
    ("move-dir", "Move window in a direction", Field::Direction),
    ("drag", "Move window by dragging", Field::None),
    ("resize", "Resize window by dragging", Field::None),
    ("go-ws", "Go to workspace", Field::Workspace),
    ("next-ws", "Next workspace", Field::None),
    ("prev-ws", "Previous workspace", Field::None),
    ("special", "Show a special workspace", Field::Special),
    ("focus-dir", "Focus in a direction", Field::Direction),
    ("group", "Group or ungroup windows", Field::None),
    ("exit", "Exit Hyprland", Field::None),
    ("reload", "Reload the config", Field::None),
];

/// The directions, as the dialog names them and Hyprland takes them.
pub const DIRECTIONS: [(&str, &str); 4] = [("Left", "left"), ("Right", "right"), ("Up", "up"), ("Down", "down")];

/// The edit dialog's form.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    /// An index into KINDS; `None` keeps the config's own action (`keep`), one the dialog
    /// doesn't offer.
    pub kind: Option<usize>,
    pub keep: Option<Action>,
    pub command: String,
    pub workspace: String,
    pub follow: bool,
    pub direction: usize,
    pub special: String,
    pub description: String,
    pub flags: Flags,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            kind: Some(0),
            keep: None,
            command: String::new(),
            workspace: "1".into(),
            follow: true,
            direction: 0,
            special: "magic".into(),
            description: String::new(),
            flags: Flags::default(),
        }
    }
}

fn kind_index(id: &str) -> Option<usize> {
    KINDS.iter().position(|(k, _, _)| *k == id)
}

fn table(fields: Vec<(&str, Value)>) -> Vec<Value> {
    vec![Value::Table(fields.into_iter().map(|(k, v)| (Some(k.to_owned()), v)).collect())]
}

fn workspace_value(text: &str) -> Value {
    let text = text.trim();
    text.parse::<i64>().map_or_else(|_| Value::Text(text.to_owned()), Value::Int)
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Text(t) => t.clone(),
        Value::Int(i) => i.to_string(),
        _ => String::new(),
    }
}

impl Form {
    /// The form for a bind: its action as one of the dialog's, or kept as it is.
    pub fn of(bind: &Bind) -> Form {
        let mut form = Form { description: bind.description.clone(), flags: bind.flags.clone(), ..Form::default() };
        let Action::Dispatcher { name, args } = &bind.action else {
            form.kind = None;
            form.keep = Some(bind.action.clone());
            return form;
        };
        let field = |f: &str| bind.action.field(f).cloned();
        let text = bind.action.text().unwrap_or_default().to_owned();
        let only_toggle = |f: &str| field(f).is_none_or(|v| v == Value::Text("toggle".into()));
        let id = match name.as_str() {
            "exec_cmd" if args.len() == 1 => {
                form.command = text;
                "command"
            }
            "window.close" => "close",
            "window.kill" => "kill",
            "window.float" if only_toggle("action") => "float",
            "window.fullscreen" if args.is_empty() => "fullscreen",
            "window.fullscreen"
                if field("mode") == Some(Value::Text("maximized".into())) && field("action").is_none() =>
            {
                "maximise"
            }
            "window.pin" => "pin",
            "window.pseudo" => "pseudo",
            "window.center" => "center",
            "layout" if text == "togglesplit" => "split",
            "window.drag" => "drag",
            "window.resize" if args.is_empty() => "resize",
            "window.move" if field("workspace").is_some() => {
                form.workspace = text_of(&field("workspace").unwrap_or(Value::Unknown));
                form.follow = field("follow") != Some(Value::Bool(false));
                "move-ws"
            }
            "window.move" | "focus" if field("direction").is_some() => {
                let d = text_of(&field("direction").unwrap_or(Value::Unknown));
                form.direction = DIRECTIONS.iter().position(|(_, h)| *h == d).unwrap_or(0);
                if name == "focus" { "focus-dir" } else { "move-dir" }
            }
            "focus" if field("workspace") == Some(Value::Text("e+1".into())) => "next-ws",
            "focus" if field("workspace") == Some(Value::Text("e-1".into())) => "prev-ws",
            "focus" if field("workspace").is_some() && field("on_current_monitor").is_none() => {
                form.workspace = text_of(&field("workspace").unwrap_or(Value::Unknown));
                "go-ws"
            }
            "workspace.toggle_special" if args.len() <= 1 => {
                form.special = text;
                "special"
            }
            "group.toggle" => "group",
            "exit" => "exit",
            "reload_config" => "reload",
            _ => "",
        };
        form.kind = kind_index(id);
        if form.kind.is_none() {
            form.keep = Some(bind.action.clone());
        }
        form
    }

    /// What the form's action needs.
    pub fn field(&self) -> Field {
        self.kind.map_or(Field::None, |k| KINDS[k].2)
    }

    /// Dragging with the mouse: Hyprland takes no other option with it.
    pub fn mouse(&self) -> bool {
        self.kind.is_some_and(|k| matches!(KINDS[k].0, "drag" | "resize"))
    }

    /// Why the form can't be saved, if it can't.
    pub fn problem(&self) -> Option<&'static str> {
        match self.field() {
            Field::Command if self.command.trim().is_empty() => Some("Type the command to run"),
            Field::Workspace if self.workspace.trim().is_empty() => Some("Type a workspace"),
            Field::Special if self.special.trim().is_empty() => Some("Type the special workspace's name"),
            _ => None,
        }
    }

    /// The action the form makes.
    pub fn action(&self) -> Action {
        let Some(k) = self.kind else { return self.keep.clone().unwrap_or(Action::Unread) };
        let dsp = |name: &str, args: Vec<Value>| Action::Dispatcher { name: name.to_owned(), args };
        let dir = Value::Text(DIRECTIONS[self.direction.min(3)].1.to_owned());
        match KINDS[k].0 {
            "command" => dsp("exec_cmd", vec![Value::Text(self.command.trim().to_owned())]),
            "close" => dsp("window.close", vec![]),
            "kill" => dsp("window.kill", vec![]),
            "float" => dsp("window.float", table(vec![("action", Value::Text("toggle".into()))])),
            "fullscreen" => dsp("window.fullscreen", vec![]),
            "maximise" => dsp("window.fullscreen", table(vec![("mode", Value::Text("maximized".into()))])),
            "pin" => dsp("window.pin", vec![]),
            "pseudo" => dsp("window.pseudo", vec![]),
            "center" => dsp("window.center", vec![]),
            "split" => dsp("layout", vec![Value::Text("togglesplit".into())]),
            "move-ws" => {
                let mut fields = vec![("workspace", workspace_value(&self.workspace))];
                if !self.follow {
                    fields.push(("follow", Value::Bool(false)));
                }
                dsp("window.move", table(fields))
            }
            "move-dir" => dsp("window.move", table(vec![("direction", dir)])),
            "drag" => dsp("window.drag", vec![]),
            "resize" => dsp("window.resize", vec![]),
            "go-ws" => dsp("focus", table(vec![("workspace", workspace_value(&self.workspace))])),
            "next-ws" => dsp("focus", table(vec![("workspace", Value::Text("e+1".into()))])),
            "prev-ws" => dsp("focus", table(vec![("workspace", Value::Text("e-1".into()))])),
            "special" => dsp("workspace.toggle_special", vec![Value::Text(self.special.trim().to_owned())]),
            "focus-dir" => dsp("focus", table(vec![("direction", dir)])),
            "group" => dsp("group.toggle", vec![]),
            "exit" => dsp("exit", vec![]),
            _ => dsp("reload_config", vec![]),
        }
    }

    /// The options as Hyprland takes them: a mouse drag has no others, and a bind that repeats
    /// can't run on release.
    pub fn flags(&self) -> Flags {
        let mut f = self.flags.clone();
        f.mouse = self.mouse();
        if f.mouse {
            f.repeating = false;
            f.release = false;
            f.locked = false;
            f.long_press = false;
        }
        if f.repeating {
            f.release = false;
            f.long_press = false;
        }
        f
    }

    /// The bind the form makes on `like`'s keys.
    pub fn bind(&self, like: &Bind) -> Bind {
        Bind {
            action: self.action(),
            flags: self.flags(),
            description: self.description.trim().to_owned(),
            ..like.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn bind(lua: &str) -> Bind {
        parse_lua(lua, Path::new("hyprland.lua")).binds.remove(0)
    }

    #[test]
    fn the_part_reads_back_as_written() {
        let close = bind(r#"hl.bind("SUPER + Q", hl.dsp.window.close())"#);
        let firefox = bind(r#"hl.bind("SUPER + B", hl.dsp.exec_cmd("firefox"), { description = "Browser" })"#);
        let mine = vec![
            Mine::Over {
                keys: "SUPER + D".into(),
                bind: Some(bind(r#"hl.bind("SUPER + D", hl.dsp.exec_cmd("fuzzel"))"#)),
            },
            Mine::Over { keys: "SUPER + J".into(), bind: None },
            Mine::Added { bind: firefox.clone(), on: true },
            Mine::Added { bind: close.clone(), on: false },
        ];
        let written = text(&mine);
        assert!(written.contains("hl.unbind(\"SUPER + J\")\n"), "{written}");
        assert!(written.contains("-- Off: hl.bind(\"SUPER + Q\", hl.dsp.window.close())"), "{written}");
        let file = PathBuf::from("lituus.lua");
        let again = parse(&written, &file);
        let strip = |m: &[Mine]| -> Vec<(String, Option<Action>, bool)> {
            m.iter()
                .map(|m| match m {
                    Mine::Over { keys, bind } => (keys.clone(), bind.as_ref().map(|b| b.action.clone()), true),
                    Mine::Added { bind, on } => (bind.keys(), Some(bind.action.clone()), *on),
                })
                .collect()
        };
        assert_eq!(strip(&again), strip(&mine));
        assert_eq!(again[0].keys(), "super+d");
    }

    /// Each of the dialog's actions, made and read back, is the same action.
    #[test]
    fn the_actions_read_back() {
        let like = bind(r#"hl.bind("SUPER + X", hl.dsp.exit())"#);
        for (k, (id, _, _)) in KINDS.iter().enumerate() {
            let form = Form {
                kind: Some(k),
                command: "kitty".into(),
                workspace: "3".into(),
                follow: false,
                direction: 2,
                ..Form::default()
            };
            let made = form.bind(&like);
            let again = Form::of(&made);
            assert_eq!(again.kind, Some(k), "{id}: {:?}", made.action);
            assert_eq!(again.action(), made.action, "{id}");
        }
        // An action the dialog doesn't offer is kept.
        let swallow = bind(r#"hl.bind("SUPER + W", hl.dsp.window.toggle_swallow())"#);
        let form = Form::of(&swallow);
        assert_eq!((form.kind, form.action()), (None, swallow.action.clone()));
    }

    #[test]
    fn options_hyprland_takes_together() {
        let form = Form {
            kind: kind_index("drag"),
            flags: Flags { repeating: true, locked: true, ..Flags::default() },
            ..Form::default()
        };
        assert_eq!(form.flags(), Flags { mouse: true, ..Flags::default() });
        let form = Form { flags: Flags { repeating: true, release: true, ..Flags::default() }, ..Form::default() };
        assert_eq!(form.flags(), Flags { repeating: true, ..Flags::default() });
        assert_eq!(Form::default().problem(), Some("Type the command to run"));
    }
}
