//! Keyboard shortcuts: the binds in the user's Hyprland config, read from its files with the file
//! and line each is on (Lua's `hl.bind(…)` calls, hyprlang's `bind = …` lines), and the binds
//! Hyprland has now (`hyprctl binds`), which say nothing of where they come from.
//!
//! Lua isn't run, only read: `local` strings and numbers, `..`, numeric `for` loops (the
//! workspace binds), `require` of the config folder's files and `hl.define_submap` are followed;
//! a bind whose keys or action depend on anything else is still listed, as unread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A bind in the config.
#[derive(Clone, Debug, PartialEq)]
pub struct Bind {
    /// The modifiers, upper-cased as written: "SUPER", "SHIFT". Empty when unread.
    pub mods: Vec<String>,
    /// The key as written: "Q", "left", "XF86AudioMute", "mouse:272", "code:10"; several for a
    /// chord ("A + B"). Empty when Lituus can't read it.
    pub key: String,
    pub action: Action,
    pub flags: Flags,
    pub description: String,
    pub file: PathBuf,
    pub line: usize,
    /// The call's last line, when it takes more than one.
    pub end_line: usize,
    /// The submap it's in; empty for the usual binds.
    pub submap: String,
}

/// What a bind does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// A dispatcher, by its Lua name below `hl.dsp` ("window.close", "exec_cmd", "focus"), with its
    /// arguments; hyprlang's dispatchers are named the same way where Lua has one.
    Dispatcher { name: String, args: Vec<Value> },
    /// A Lua function, or something Lituus can't read.
    Unread,
}

impl Action {
    /// The first argument as text (a command, a layout message).
    pub fn text(&self) -> Option<&str> {
        match self {
            Action::Dispatcher { args, .. } => args.first().and_then(Value::as_text),
            Action::Unread => None,
        }
    }

    /// A field of the first argument, when it's a table (`{ workspace = 3 }`).
    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Action::Dispatcher { args, .. } => args.first().and_then(|a| a.field(name)),
            Action::Unread => None,
        }
    }
}

/// The bind's options that change what it does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub repeating: bool,
    /// It works on the lock screen too.
    pub locked: bool,
    pub release: bool,
    pub long_press: bool,
    /// A mouse drag (moving or resizing a window).
    pub mouse: bool,
}

/// A value in a Lua expression, as far as Lituus follows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Table(Vec<(Option<String>, Value)>),
    /// A name below `hl` ("hl.dsp.window"), not called yet.
    Path(String),
    /// A dispatcher, called: `hl.dsp.window.close()`.
    Dispatcher(String, Vec<Value>),
    Function,
    Unknown,
}

impl Value {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(t) => Some(t),
            _ => None,
        }
    }

    /// Text as Lua's `..` makes it of a string or number.
    fn concat_text(&self) -> Option<String> {
        match self {
            Value::Text(t) => Some(t.clone()),
            Value::Int(i) => Some(i.to_string()),
            Value::Float(f) => Some(if f.fract() == 0.0 { format!("{f:.1}") } else { f.to_string() }),
            _ => None,
        }
    }

    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Table(fields) => fields.iter().find(|(k, _)| k.as_deref() == Some(name)).map(|(_, v)| v),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        matches!(self, Value::Bool(true))
    }
}

/// Modifier names Hyprland takes, and the bit each sets in `hyprctl binds`' modmask.
const MODIFIERS: [(&str, u32); 16] = [
    ("SHIFT", 1),
    ("CAPS", 2),
    ("CTRL", 4),
    ("CONTROL", 4),
    ("ALT", 8),
    ("MOD1", 8),
    ("MOD2", 16),
    ("MOD3", 32),
    ("SUPER", 64),
    ("WIN", 64),
    ("LOGO", 64),
    ("MOD4", 64),
    ("META", 64),
    ("MOD5", 128),
    ("SHIFT_L", 1),
    ("SHIFT_R", 1),
];

/// A modifier's bit, for its name (sided ones too: "SUPER_L").
pub fn modifier_bit(name: &str) -> Option<u32> {
    let upper = name.trim().to_uppercase();
    let base = upper.strip_suffix("_L").or_else(|| upper.strip_suffix("_R")).unwrap_or(&upper);
    MODIFIERS.iter().find(|(n, _)| *n == base || *n == upper).map(|(_, bit)| *bit)
}

impl Bind {
    /// The modifiers as `hyprctl binds` reports them.
    pub fn modmask(&self) -> u32 {
        self.mods.iter().filter_map(|m| modifier_bit(m)).fold(0, |mask, bit| mask | bit)
    }
}

/// Splits Lua's keys ("SUPER + SHIFT + Q") into the modifiers and the key.
fn split_lua_keys(keys: &str) -> (Vec<String>, String) {
    let parts: Vec<&str> = keys.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    let mods = parts.iter().take_while(|p| modifier_bit(p).is_some()).map(|p| p.to_uppercase()).collect::<Vec<_>>();
    let key = parts[mods.len()..].join(" + ");
    // "SUPER + +" binds the plus key.
    let key = if key.is_empty() && keys.trim_end().ends_with("+ +") { "+".to_owned() } else { key };
    (mods, key)
}

/// hyprlang's modifiers, written any way it takes them ("SUPER_SHIFT", "SUPER SHIFT", "$mainMod").
fn split_conf_mods(mods: &str) -> Vec<String> {
    let mut found = Vec::new();
    for part in mods.split(['_', ' ', '&', '+']).filter(|p| !p.is_empty()) {
        if modifier_bit(part).is_some() {
            found.push(part.to_uppercase());
        }
    }
    found
}

/// The binds in the config in `dir`: hyprland.lua and the files it requires, or else
/// hyprland.conf and the files it sources.
pub fn read(dir: &Path) -> Vec<Bind> {
    let lua = dir.join("hyprland.lua");
    if lua.exists() {
        let mut reader = LuaReader { dir: dir.to_path_buf(), seen: Vec::new(), binds: Vec::new() };
        reader.file(&lua);
        reader.binds
    } else {
        let mut binds = Vec::new();
        let mut vars = HashMap::new();
        read_conf(&dir.join("hyprland.conf"), &mut vars, &mut binds, &mut Vec::new());
        binds
    }
}

// ---- Lua ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Name(String),
    Text(String),
    Num(f64, bool),
    Sym(&'static str),
}

const SYMBOLS: [&str; 28] = [
    "...", "..", "==", "~=", "<=", ">=", "//", "::", ".", "+", "-", "*", "/", "%", "^", "#", "<", ">", "=", "(", ")",
    "{", "}", "[", "]", ";", ":", ",",
];

/// Lua's tokens with the line each starts on; comments dropped.
fn tokens(text: &str) -> Vec<(Tok, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    // `[[`, `[==[`: the number of `=`, if a long bracket starts at i.
    let long_open = |i: usize| -> Option<usize> {
        if chars.get(i) != Some(&'[') {
            return None;
        }
        let eq = chars[i + 1..].iter().take_while(|c| **c == '=').count();
        (chars.get(i + 1 + eq) == Some(&'[')).then_some(eq)
    };
    // The text of a long bracket from i, and where it ends.
    let long_body = |i: usize, eq: usize| -> (String, usize) {
        let close: Vec<char> =
            std::iter::once(']').chain(std::iter::repeat_n('=', eq)).chain(std::iter::once(']')).collect();
        let start = i + eq + 2;
        let mut j = start;
        while j < chars.len() && chars[j..].iter().take(close.len()).ne(close.iter()) {
            j += 1;
        }
        let body: String = chars[start..j.min(chars.len())].iter().collect();
        (body.strip_prefix('\n').map(str::to_owned).unwrap_or(body), (j + close.len()).min(chars.len()))
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            if let Some(eq) = long_open(i + 2) {
                let (body, end) = long_body(i + 2, eq);
                line += body.matches('\n').count() + 1;
                i = end;
            } else {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
        } else if let Some(eq) = long_open(i) {
            let (body, end) = long_body(i, eq);
            out.push((Tok::Text(body.clone()), line));
            line += chars[i..end].iter().filter(|c| **c == '\n').count();
            i = end;
        } else if c == '"' || c == '\'' {
            let start = line;
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    s.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    s.push(chars[i]);
                }
                i += 1;
            }
            i += 1;
            out.push((Tok::Text(s), start));
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            if c == '0' && matches!(chars.get(i + 1), Some('x' | 'X')) {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let hex: String = chars[start + 2..i].iter().collect();
                out.push((Tok::Num(i64::from_str_radix(&hex, 16).unwrap_or(0) as f64, true), line));
                continue;
            }
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric()
                    || chars[i] == '.'
                    || ((chars[i] == '-' || chars[i] == '+') && matches!(chars[i - 1], 'e' | 'E')))
            {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            let int = !text.contains(['.', 'e', 'E']);
            out.push((Tok::Num(text.parse().unwrap_or(0.0), int), line));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push((Tok::Name(chars[start..i].iter().collect()), line));
        } else if let Some(sym) = SYMBOLS.iter().find(|s| chars[i..].iter().take(s.len()).copied().eq(s.chars())) {
            out.push((Tok::Sym(sym), line));
            i += sym.len();
        } else {
            i += 1;
        }
    }
    out
}

struct LuaReader {
    dir: PathBuf,
    seen: Vec<PathBuf>,
    binds: Vec<Bind>,
}

/// One file's tokens being read, with the names it has set.
struct Chunk<'a> {
    reader: &'a mut LuaReader,
    file: PathBuf,
    toks: Vec<(Tok, usize)>,
    pos: usize,
    vars: Vec<HashMap<String, Value>>,
    submap: String,
}

impl LuaReader {
    fn file(&mut self, path: &Path) {
        let Ok(canonical) = path.canonicalize() else { return };
        if self.seen.contains(&canonical) {
            return;
        }
        self.seen.push(canonical);
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let toks = tokens(&text);
        let len = toks.len();
        let mut chunk = Chunk {
            reader: self,
            file: path.to_path_buf(),
            toks,
            pos: 0,
            vars: vec![HashMap::new()],
            submap: String::new(),
        };
        chunk.block(len);
    }

    /// `require("monitors")`: monitors.lua (or monitors/init.lua) in the config folder.
    fn require(&mut self, name: &str) {
        let rel = name.replace('.', "/");
        for candidate in [self.dir.join(format!("{rel}.lua")), self.dir.join(&rel).join("init.lua")] {
            if candidate.exists() {
                self.file(&candidate);
                return;
            }
        }
    }
}

const OPENERS: [&str; 4] = ["function", "if", "do", "repeat"];
const CLOSERS: [&str; 2] = ["end", "until"];

impl Chunk<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|(t, _)| t)
    }

    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.pos + n).map(|(t, _)| t)
    }

    fn line(&self) -> usize {
        self.toks.get(self.pos).or(self.toks.last()).map_or(1, |(_, l)| *l)
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }

    fn is_name(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Name(x)) if x == s)
    }

    fn eat_sym(&mut self, s: &str) -> bool {
        let found = self.is_sym(s);
        if found {
            self.pos += 1;
        }
        found
    }

    fn eat_name(&mut self, s: &str) -> bool {
        let found = self.is_name(s);
        if found {
            self.pos += 1;
        }
        found
    }

    fn name(&mut self) -> Option<String> {
        match self.peek() {
            Some(Tok::Name(n)) => {
                let n = n.clone();
                self.pos += 1;
                Some(n)
            }
            _ => None,
        }
    }

    fn get(&self, name: &str) -> Value {
        for scope in self.vars.iter().rev() {
            if let Some(v) = scope.get(name) {
                return v.clone();
            }
        }
        if name == "hl" { Value::Path("hl".into()) } else { Value::Unknown }
    }

    fn set_local(&mut self, name: String, v: Value) {
        self.vars.last_mut().expect("there's always a scope").insert(name, v);
    }

    fn set(&mut self, name: String, v: Value) {
        for scope in self.vars.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(&name) {
                *slot = v;
                return;
            }
        }
        self.vars[0].insert(name, v);
    }

    /// Where the block from here ends: the token after its `end` (or `until`), or `limit`.
    fn block_end(&self, limit: usize) -> usize {
        let mut depth = 1;
        let mut i = self.pos;
        while i < limit {
            if let Tok::Name(n) = &self.toks[i].0 {
                if OPENERS.contains(&n.as_str()) {
                    depth += 1;
                } else if CLOSERS.contains(&n.as_str()) {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
            }
            i += 1;
        }
        limit
    }

    /// Reads statements up to `limit` (a token index).
    fn block(&mut self, limit: usize) {
        while self.pos < limit {
            let before = self.pos;
            self.statement(limit);
            if self.pos == before {
                self.pos += 1;
            }
        }
        self.pos = limit;
    }

    fn statement(&mut self, limit: usize) {
        match self.peek() {
            Some(Tok::Name(n)) if n == "local" => {
                self.pos += 1;
                if self.eat_name("function") {
                    let name = self.name();
                    if let Some(name) = name {
                        self.set_local(name, Value::Function);
                    }
                    self.function_body(limit);
                    return;
                }
                let mut names = Vec::new();
                while let Some(n) = self.name() {
                    names.push(n);
                    // `<const>`, `<close>`.
                    if self.eat_sym("<") {
                        self.name();
                        self.eat_sym(">");
                    }
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                let values = if self.eat_sym("=") { self.expr_list(limit) } else { Vec::new() };
                for (i, n) in names.into_iter().enumerate() {
                    self.set_local(n, values.get(i).cloned().unwrap_or(Value::Unknown));
                }
            }
            Some(Tok::Name(n)) if n == "for" => {
                self.pos += 1;
                self.for_loop(limit);
            }
            Some(Tok::Name(n)) if n == "function" => {
                self.pos += 1;
                // function a.b:c(…)
                while self.name().is_some() && (self.eat_sym(".") || self.eat_sym(":")) {}
                self.function_body(limit);
            }
            Some(Tok::Name(n)) if n == "while" || n == "if" || n == "elseif" || n == "until" || n == "return" => {
                self.pos += 1;
                self.expr(limit);
                self.eat_name("then");
                self.eat_name("do");
            }
            Some(Tok::Name(n)) if ["do", "then", "else", "end", "repeat", "break", "goto"].contains(&n.as_str()) => {
                self.pos += 1;
            }
            Some(Tok::Name(_)) | Some(Tok::Sym("(")) => {
                let start = self.pos;
                let target = self.suffixed(limit);
                if self.is_sym("=") || self.is_sym(",") {
                    // a = …, or a, b = …: only a plain name is kept.
                    let single = self.pos == start + 1;
                    let name = match &self.toks[start].0 {
                        Tok::Name(n) if single => Some(n.clone()),
                        _ => None,
                    };
                    while self.eat_sym(",") {
                        self.suffixed(limit);
                    }
                    if self.eat_sym("=") {
                        let values = self.expr_list(limit);
                        if let Some(name) = name {
                            self.set(name, values.into_iter().next().unwrap_or(Value::Unknown));
                        }
                    }
                }
                let _ = target;
            }
            _ => self.pos += 1,
        }
    }

    fn function_body(&mut self, limit: usize) {
        // The parameters, then the body, read in place: binds made in a function (one that runs
        // at start, say) are listed where they're written.
        if self.eat_sym("(") {
            let mut params = Vec::new();
            while !self.eat_sym(")") && self.pos < limit {
                if let Some(n) = self.name() {
                    params.push(n);
                } else {
                    self.pos += 1;
                }
            }
            let end = self.block_end(limit);
            self.vars.push(params.into_iter().map(|p| (p, Value::Unknown)).collect());
            self.block(end);
            self.vars.pop();
            self.eat_name("end");
        }
    }

    fn for_loop(&mut self, limit: usize) {
        let first = self.name();
        if self.eat_sym("=") {
            let from = self.expr(limit);
            self.eat_sym(",");
            let to = self.expr(limit);
            let step = if self.eat_sym(",") { self.expr(limit) } else { Value::Int(1) };
            self.eat_name("do");
            let body = self.pos;
            let end = self.block_end(limit);
            let ints = match (from, to, step) {
                (Value::Int(a), Value::Int(b), Value::Int(s)) if s != 0 => Some((a, b, s)),
                _ => None,
            };
            match (first, ints) {
                // Each time round, up to 100 (one per workspace is the usual).
                (Some(var), Some((a, b, s))) => {
                    let mut i = a;
                    let mut count = 0;
                    while (s > 0 && i <= b || s < 0 && i >= b) && count < 100 {
                        self.pos = body;
                        self.vars.push(HashMap::from([(var.clone(), Value::Int(i))]));
                        self.block(end);
                        self.vars.pop();
                        i += s;
                        count += 1;
                    }
                }
                (var, _) => {
                    self.pos = body;
                    self.vars.push(var.into_iter().map(|v| (v, Value::Unknown)).collect());
                    self.block(end);
                    self.vars.pop();
                }
            }
            self.pos = end;
            self.eat_name("end");
        } else {
            // for k, v in …: once, with what it goes through unknown.
            let mut names: Vec<String> = first.into_iter().collect();
            while self.eat_sym(",") {
                names.extend(self.name());
            }
            self.eat_name("in");
            self.expr_list(limit);
            self.eat_name("do");
            let end = self.block_end(limit);
            self.vars.push(names.into_iter().map(|v| (v, Value::Unknown)).collect());
            self.block(end);
            self.vars.pop();
            self.eat_name("end");
        }
    }

    fn expr_list(&mut self, limit: usize) -> Vec<Value> {
        let mut values = vec![self.expr(limit)];
        while self.eat_sym(",") {
            values.push(self.expr(limit));
        }
        values
    }

    fn expr(&mut self, limit: usize) -> Value {
        self.binary(limit, 0)
    }

    /// Binary operators by precedence (Lua's, low to high); only `..` and arithmetic are worked
    /// out.
    fn binary(&mut self, limit: usize, level: usize) -> Value {
        const LEVELS: [&[&str]; 7] = [
            &["or"],
            &["and"],
            &["<", ">", "<=", ">=", "~=", "=="],
            &[".."],
            &["+", "-"],
            &["*", "/", "//", "%"],
            &["^"],
        ];
        if level == LEVELS.len() {
            return self.unary(limit);
        }
        let mut left = self.binary(limit, level + 1);
        loop {
            if self.pos >= limit {
                return left;
            }
            let op = match self.peek() {
                Some(Tok::Sym(s)) if LEVELS[level].contains(s) => *s,
                Some(Tok::Name(n)) if LEVELS[level].contains(&n.as_str()) => {
                    LEVELS[level].iter().find(|o| **o == n).expect("found")
                }
                _ => return left,
            };
            self.pos += 1;
            // `..` and `^` are right-associative.
            let right = if op == ".." || op == "^" { self.binary(limit, level) } else { self.binary(limit, level + 1) };
            left = arith(op, left, right);
        }
    }

    fn unary(&mut self, limit: usize) -> Value {
        if self.eat_sym("-") {
            return match self.unary(limit) {
                Value::Int(i) => Value::Int(-i),
                Value::Float(f) => Value::Float(-f),
                _ => Value::Unknown,
            };
        }
        if self.eat_name("not") || self.eat_sym("#") {
            self.unary(limit);
            return Value::Unknown;
        }
        self.suffixed(limit)
    }

    fn primary(&mut self, limit: usize) -> Value {
        let Some(tok) = self.peek().cloned() else { return Value::Unknown };
        match tok {
            Tok::Text(t) => {
                self.pos += 1;
                Value::Text(t)
            }
            Tok::Num(n, int) => {
                self.pos += 1;
                if int { Value::Int(n as i64) } else { Value::Float(n) }
            }
            Tok::Name(n) => match n.as_str() {
                "true" | "false" => {
                    self.pos += 1;
                    Value::Bool(n == "true")
                }
                "nil" => {
                    self.pos += 1;
                    Value::Unknown
                }
                "function" => {
                    self.pos += 1;
                    self.function_body(limit);
                    Value::Function
                }
                _ if is_keyword(&n) => Value::Unknown,
                _ => {
                    self.pos += 1;
                    self.get(&n)
                }
            },
            Tok::Sym("(") => {
                self.pos += 1;
                let v = self.expr(limit);
                self.eat_sym(")");
                v
            }
            Tok::Sym("{") => self.table(limit),
            Tok::Sym("...") => {
                self.pos += 1;
                Value::Unknown
            }
            Tok::Sym(_) => Value::Unknown,
        }
    }

    fn table(&mut self, limit: usize) -> Value {
        self.eat_sym("{");
        let mut fields = Vec::new();
        while self.pos < limit && !self.eat_sym("}") {
            if self.eat_sym("[") {
                let key = self.expr(limit);
                self.eat_sym("]");
                self.eat_sym("=");
                let v = self.expr(limit);
                fields.push((key.as_text().map(str::to_owned), v));
            } else if matches!(self.peek(), Some(Tok::Name(_))) && matches!(self.peek_at(1), Some(Tok::Sym("="))) {
                let key = self.name();
                self.pos += 1;
                let v = self.expr(limit);
                fields.push((key, v));
            } else {
                let before = self.pos;
                let v = self.expr(limit);
                if self.pos == before {
                    self.pos += 1;
                }
                fields.push((None, v));
            }
            if !self.eat_sym(",") {
                self.eat_sym(";");
            }
        }
        Value::Table(fields)
    }

    /// A name or `(…)` with what follows it: `.field`, `[key]`, calls.
    fn suffixed(&mut self, limit: usize) -> Value {
        let line = self.line();
        // `require(…)`, unless the config named something else so.
        let require = self.is_name("require") && self.get("require") == Value::Unknown;
        let mut v = self.primary(limit);
        let mut plain = true;
        while self.pos < limit {
            if self.is_sym(".") {
                // `..` is its own token, so a lone `.` is always a field.
                self.pos += 1;
                let field = self.name().unwrap_or_default();
                v = match v {
                    Value::Path(p) => Value::Path(format!("{p}.{field}")),
                    Value::Table(_) => v.field(&field).cloned().unwrap_or(Value::Unknown),
                    _ => Value::Unknown,
                };
            } else if self.eat_sym("[") {
                self.expr(limit);
                self.eat_sym("]");
                v = Value::Unknown;
            } else if self.eat_sym(":") {
                self.name();
                self.args(limit);
                v = Value::Unknown;
            } else if self.is_sym("(") || self.is_sym("{") || matches!(self.peek(), Some(Tok::Text(_))) {
                v = self.call(v, require && plain, line, limit);
            } else {
                break;
            }
            plain = false;
        }
        v
    }

    fn args(&mut self, limit: usize) -> Vec<Value> {
        match self.peek() {
            Some(Tok::Text(t)) => {
                let t = t.clone();
                self.pos += 1;
                vec![Value::Text(t)]
            }
            Some(Tok::Sym("{")) => vec![self.table(limit)],
            Some(Tok::Sym("(")) => {
                self.pos += 1;
                let mut args = Vec::new();
                while self.pos < limit && !self.eat_sym(")") {
                    let before = self.pos;
                    args.push(self.expr(limit));
                    if !self.eat_sym(",") && self.pos == before {
                        self.pos += 1;
                    }
                }
                args
            }
            _ => Vec::new(),
        }
    }

    fn call(&mut self, callee: Value, require: bool, line: usize, limit: usize) -> Value {
        let path = match &callee {
            Value::Path(p) => Some(p.clone()),
            _ => None,
        };
        // A submap's binds: its name before the function with them is read.
        if path.as_deref() == Some("hl.define_submap") && self.is_sym("(") {
            let save = self.pos;
            self.pos += 1;
            let name = self.expr(limit).as_text().map(str::to_owned);
            self.pos = save;
            let outer = std::mem::replace(&mut self.submap, name.unwrap_or_default());
            self.args(limit);
            self.submap = outer;
            return Value::Unknown;
        }
        let args = self.args(limit);
        let end_line = self.toks.get(self.pos.saturating_sub(1)).map_or(line, |(_, l)| *l);
        match path.as_deref() {
            Some("hl.bind") => {
                self.bind(&args, line, end_line);
                Value::Unknown
            }
            Some(p) if p.starts_with("hl.dsp.") => Value::Dispatcher(p["hl.dsp.".len()..].to_owned(), args),
            _ => {
                if let (true, Some(Value::Text(name))) = (require, args.first()) {
                    self.reader.require(name);
                }
                Value::Unknown
            }
        }
    }

    fn bind(&mut self, args: &[Value], line: usize, end_line: usize) {
        let (mods, key) = match args.first() {
            Some(Value::Text(keys)) => split_lua_keys(keys),
            _ => (Vec::new(), String::new()),
        };
        let action = match args.get(1) {
            Some(Value::Dispatcher(name, args)) => Action::Dispatcher { name: name.clone(), args: args.clone() },
            _ => Action::Unread,
        };
        let opts = args.get(2);
        let flag = |name: &str| opts.and_then(|o| o.field(name)).is_some_and(Value::truthy);
        let flags = Flags {
            repeating: flag("repeating"),
            locked: flag("locked"),
            release: flag("release"),
            long_press: flag("long_press"),
            mouse: flag("mouse"),
        };
        let description = opts
            .and_then(|o| o.field("description").or_else(|| o.field("desc")))
            .and_then(Value::as_text)
            .unwrap_or_default()
            .to_owned();
        let submap = opts
            .and_then(|o| o.field("submap"))
            .and_then(Value::as_text)
            .map_or_else(|| self.submap.clone(), str::to_owned);
        self.reader.binds.push(Bind {
            mods,
            key,
            action,
            flags,
            description,
            file: self.file.clone(),
            line,
            end_line,
            submap,
        });
    }
}

fn is_keyword(n: &str) -> bool {
    matches!(
        n,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "for"
            | "goto"
            | "if"
            | "in"
            | "local"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "until"
            | "while"
    )
}

fn arith(op: &str, a: Value, b: Value) -> Value {
    if op == ".." {
        return match (a.concat_text(), b.concat_text()) {
            (Some(a), Some(b)) => Value::Text(a + &b),
            _ => Value::Unknown,
        };
    }
    let num = |v: &Value| match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        Value::Text(t) => t.trim().parse().ok(),
        _ => None,
    };
    let ints = matches!((&a, &b), (Value::Int(_), Value::Int(_)));
    let (Some(x), Some(y)) = (num(&a), num(&b)) else { return Value::Unknown };
    let int = |f: f64| if ints { Value::Int(f as i64) } else { Value::Float(f) };
    match op {
        "+" => int(x + y),
        "-" => int(x - y),
        "*" => int(x * y),
        "/" => Value::Float(x / y),
        "//" if y != 0.0 => int((x / y).floor()),
        "%" if y != 0.0 => int(x - (x / y).floor() * y),
        "^" => Value::Float(x.powf(y)),
        _ => Value::Unknown,
    }
}

// ---- hyprlang ----------------------------------------------------------------------------------

/// hyprlang's dispatchers that Lua names otherwise, and the argument Lua would give them.
fn conf_action(dispatcher: &str, arg: &str, mouse: bool) -> Action {
    let text = |s: &str| vec![Value::Text(s.to_owned())];
    let table = |k: &str, v: Value| vec![Value::Table(vec![(Some(k.to_owned()), v)])];
    let number_or_text =
        |s: &str| s.trim().parse::<i64>().map_or_else(|_| Value::Text(s.trim().to_owned()), Value::Int);
    let direction = |s: &str| {
        Value::Text(
            match s.trim() {
                "l" => "left",
                "r" => "right",
                "u" | "t" => "up",
                "d" | "b" => "down",
                other => other,
            }
            .to_owned(),
        )
    };
    let (name, args) = match dispatcher.trim() {
        "exec" => ("exec_cmd", text(arg)),
        "execr" => ("exec_raw", text(arg)),
        "killactive" | "closewindow" => ("window.close", Vec::new()),
        "forcekillactive" | "killwindow" => ("window.kill", Vec::new()),
        "togglefloating" => ("window.float", table("action", Value::Text("toggle".into()))),
        "fullscreen" => (
            "window.fullscreen",
            if arg.trim() == "1" { table("mode", Value::Text("maximized".into())) } else { Vec::new() },
        ),
        "pseudo" => ("window.pseudo", Vec::new()),
        "pin" => ("window.pin", Vec::new()),
        "centerwindow" => ("window.center", Vec::new()),
        "workspace" => ("focus", table("workspace", number_or_text(arg))),
        "movefocus" => ("focus", table("direction", direction(arg))),
        "focusmonitor" => ("focus", table("monitor", Value::Text(arg.trim().into()))),
        "movetoworkspace" | "movetoworkspacesilent" => ("window.move", table("workspace", number_or_text(arg))),
        "movewindow" if mouse => ("window.drag", Vec::new()),
        "resizewindow" if mouse => ("window.resize", Vec::new()),
        "movewindow" => ("window.move", table("direction", direction(arg))),
        "togglespecialworkspace" => {
            ("workspace.toggle_special", text(if arg.trim().is_empty() { "special" } else { arg.trim() }))
        }
        "togglesplit" => ("layout", text("togglesplit")),
        "layoutmsg" => ("layout", text(arg.trim())),
        "togglegroup" => ("group.toggle", Vec::new()),
        "exit" => ("exit", Vec::new()),
        "submap" => ("submap", text(arg.trim())),
        "dpms" => ("dpms", text(arg.trim())),
        "global" => ("global", text(arg.trim())),
        "pass" => ("pass", text(arg.trim())),
        other => {
            let args = if arg.trim().is_empty() { Vec::new() } else { text(arg.trim()) };
            return Action::Dispatcher { name: other.to_owned(), args };
        }
    };
    Action::Dispatcher { name: name.to_owned(), args }
}

/// hyprlang's `$name = value` variables put in.
fn substitute(text: &str, vars: &HashMap<String, String>) -> String {
    let mut names: Vec<&String> = vars.keys().collect();
    // The longest first, so $mainModShift isn't read as $mainMod + "Shift".
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut out = text.to_owned();
    for n in names {
        out = out.replace(&format!("${n}"), &vars[n]);
    }
    out
}

fn expand_home(path: &str, dir: &Path) -> PathBuf {
    let path = path.trim();
    if let Some(rest) = path.strip_prefix("~/") {
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(rest)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        dir.join(path)
    }
}

fn read_conf(path: &Path, vars: &mut HashMap<String, String>, binds: &mut Vec<Bind>, seen: &mut Vec<PathBuf>) {
    let Ok(canonical) = path.canonicalize() else { return };
    if seen.contains(&canonical) {
        return;
    }
    seen.push(canonical);
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut submap = String::new();
    for (i, raw) in text.lines().enumerate() {
        // `##` is a literal `#`.
        let line = raw.replace("##", "\u{0}");
        let line = line.split('#').next().unwrap_or_default().replace('\u{0}', "#");
        let Some((name, value)) = line.split_once('=') else { continue };
        let (name, value) = (name.trim(), value.trim());
        if let Some(var) = name.strip_prefix('$') {
            let value = substitute(value, vars);
            vars.insert(var.to_owned(), value);
            continue;
        }
        let value = substitute(value, vars);
        match name {
            "source" => {
                let target = expand_home(&value, dir);
                read_conf(&target, vars, binds, seen);
            }
            "submap" => submap = if value == "reset" { String::new() } else { value.clone() },
            _ if name.starts_with("bind") && name[4..].chars().all(|c| c.is_ascii_lowercase()) => {
                let letters = &name[4..];
                let described = letters.contains('d');
                let mut fields = value.splitn(if described { 5 } else { 4 }, ',').map(str::trim);
                let mods = split_conf_mods(fields.next().unwrap_or_default());
                let key = fields.next().unwrap_or_default().to_owned();
                let description = if described { fields.next().unwrap_or_default().to_owned() } else { String::new() };
                let dispatcher = fields.next().unwrap_or_default();
                let arg = fields.next().unwrap_or_default();
                let mouse = letters.contains('m');
                binds.push(Bind {
                    mods,
                    key,
                    action: conf_action(dispatcher, arg, mouse),
                    flags: Flags {
                        repeating: letters.contains('e'),
                        locked: letters.contains('l'),
                        release: letters.contains('r'),
                        long_press: letters.contains('o'),
                        mouse,
                    },
                    description,
                    file: path.to_path_buf(),
                    line: i + 1,
                    end_line: i + 1,
                    submap: submap.clone(),
                });
            }
            _ => {}
        }
    }
}

// ---- Hyprland now ------------------------------------------------------------------------------

/// A bind as `hyprctl binds` reports it: no file or line, and for Lua no readable action.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct LiveBind {
    pub modmask: u32,
    pub key: String,
    pub keycode: i64,
    pub submap: String,
    pub description: String,
    pub dispatcher: String,
    pub arg: String,
    pub mouse: bool,
    pub locked: bool,
    pub release: bool,
    pub repeat: bool,
}

impl LiveBind {
    /// The same bind as one read from the config: the modifiers, and the key (keysym names are
    /// matched without case, as Hyprland does).
    pub fn matches(&self, bind: &Bind) -> bool {
        self.modmask == bind.modmask()
            && self.submap == bind.submap
            && (self.key.eq_ignore_ascii_case(&bind.key)
                || bind.key.strip_prefix("code:").and_then(|c| c.parse::<i64>().ok()) == Some(self.keycode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lituus-hypr-binds-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn dispatcher(b: &Bind) -> (&str, &[Value]) {
        match &b.action {
            Action::Dispatcher { name, args } => (name, args),
            Action::Unread => ("", &[]),
        }
    }

    /// Hyprland 0.57's own example config.
    #[test]
    fn reads_the_default_config() {
        let d = dir("default");
        std::fs::write(d.join("hyprland.lua"), include_str!("../tests/fixtures/default.lua")).unwrap();
        let binds = read(&d);
        assert_eq!(binds.len(), 48, "{binds:#?}");
        let first = &binds[0];
        assert_eq!((first.mods.as_slice(), first.key.as_str(), first.line), (&["SUPER".to_owned()][..], "Q", 268));
        assert_eq!(dispatcher(first), ("exec_cmd", &[Value::Text("kitty".into())][..]));
        // Assigned to a local: still a bind.
        assert_eq!(dispatcher(&binds[1]), ("window.close", &[][..]));
        // The loop: 1 to 9, then 0 for workspace 10.
        let ws: Vec<&Bind> = binds.iter().filter(|b| b.line == 288).collect();
        assert_eq!(ws.len(), 10);
        assert_eq!(ws.iter().map(|b| b.key.as_str()).collect::<String>(), "1234567890");
        assert_eq!(ws[9].action.field("workspace"), Some(&Value::Int(10)));
        let moved: Vec<&Bind> = binds.iter().filter(|b| b.line == 289).collect();
        assert_eq!(moved[0].mods, ["SUPER", "SHIFT"]);
        let drag = binds.iter().find(|b| b.key == "mouse:272").unwrap();
        assert!(drag.flags.mouse);
        assert_eq!(dispatcher(drag).0, "window.drag");
        let volume = binds.iter().find(|b| b.key == "XF86AudioRaiseVolume").unwrap();
        assert!(volume.flags.locked && volume.flags.repeating && volume.mods.is_empty());
        assert_eq!(volume.action.text(), Some("wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 5%+"));
        assert_eq!(
            binds.iter().find(|b| b.key == "V").unwrap().action.field("action"),
            Some(&Value::Text("toggle".into()))
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn follows_requires_submaps_and_reads_what_it_can() {
        let d = dir("lua");
        std::fs::write(
            d.join("hyprland.lua"),
            r#"local mod = "SUPER"
local apps = { term = "foot" }
require("binds")
hl.bind(mod .. " + T", function() hl.dispatch(hl.dsp.exec_cmd("foot")) end) -- a function
hl.bind(
    mod .. " + B",
    hl.dsp.exec_cmd("firefox"),
    { description = "Browser" }
)
hl.define_submap("resize", function()
    hl.bind("right", hl.dsp.window.resize({ x = 10, y = 0 }), { repeating = true })
    hl.bind("escape", hl.dsp.submap("reset"))
end)
--[[ hl.bind("SUPER + X", hl.dsp.exit()) ]]
"#,
        )
        .unwrap();
        std::fs::write(d.join("binds.lua"), "hl.bind('ALT + code:10', hl.dsp.focus({ workspace = 1 }))\n").unwrap();
        let binds = read(&d);
        let keys: Vec<(&str, usize, &str)> =
            binds.iter().map(|b| (b.key.as_str(), b.line, b.submap.as_str())).collect();
        assert_eq!(
            keys,
            [("code:10", 1, ""), ("T", 4, ""), ("B", 5, ""), ("right", 11, "resize"), ("escape", 12, "resize")]
        );
        assert_eq!(binds[0].file, d.join("binds.lua"));
        assert_eq!(binds[1].action, Action::Unread);
        assert_eq!((binds[2].description.as_str(), binds[2].end_line), ("Browser", 9));
        assert!(binds[3].flags.repeating);
        assert_eq!(binds[0].modmask(), 8);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn reads_hyprlang() {
        let d = dir("conf");
        std::fs::write(
            d.join("hyprland.conf"),
            "$mainMod = SUPER # the main key\n\
             bind = $mainMod, Q, exec, kitty\n\
             bind = $mainMod SHIFT, 2, movetoworkspace, 2\n\
             source = ./more.conf\n\
             bindel = , XF86AudioRaiseVolume, exec, wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+\n\
             bindm = $mainMod, mouse:272, movewindow\n\
             submap = resize\n\
             binde = , right, resizeactive, 10 0\n\
             submap = reset\n",
        )
        .unwrap();
        std::fs::write(d.join("more.conf"), "bindd = SUPER_SHIFT, B, Browser, exec, firefox\n").unwrap();
        let binds = read(&d);
        assert_eq!(binds.len(), 6);
        assert_eq!(dispatcher(&binds[0]), ("exec_cmd", &[Value::Text("kitty".into())][..]));
        assert_eq!(binds[1].mods, ["SUPER", "SHIFT"]);
        assert_eq!(binds[1].action.field("workspace"), Some(&Value::Int(2)));
        assert_eq!(
            (binds[2].description.as_str(), binds[2].file.file_name().unwrap().to_str()),
            ("Browser", Some("more.conf"))
        );
        assert!(binds[3].flags.locked && binds[3].flags.repeating);
        assert_eq!(dispatcher(&binds[4]).0, "window.drag");
        assert_eq!((binds[5].submap.as_str(), dispatcher(&binds[5]).0), ("resize", "resizeactive"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn matches_what_hyprland_reports() {
        let live: Vec<LiveBind> = serde_json::from_str(include_str!("../tests/fixtures/binds.json")).unwrap();
        let d = dir("live");
        std::fs::write(d.join("hyprland.lua"), include_str!("../tests/fixtures/default.lua")).unwrap();
        let binds = read(&d);
        let unmatched: Vec<&LiveBind> = live.iter().filter(|l| !binds.iter().any(|b| l.matches(b))).collect();
        assert_eq!(unmatched.len(), 1, "{unmatched:?}");
        assert_eq!(unmatched[0].key, "B");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn keys() {
        assert_eq!(split_lua_keys("SUPER + SHIFT + Q"), (vec!["SUPER".into(), "SHIFT".into()], "Q".into()));
        assert_eq!(split_lua_keys("XF86AudioMute"), (vec![], "XF86AudioMute".into()));
        assert_eq!(split_lua_keys("super_l + A + B"), (vec!["SUPER_L".into()], "A + B".into()));
        assert_eq!(split_conf_mods("SUPER_SHIFT"), ["SUPER", "SHIFT"]);
        assert_eq!(modifier_bit("ctrl"), Some(4));
    }
}
