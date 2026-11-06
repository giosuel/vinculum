//! Keyboard layouts and options from xkeyboard-config (`evdev.xml`), and the xkb options the
//! Keyboard page sets: how to switch layouts, what Caps Lock does, the Compose key. Each of those
//! is one option in `kb_options`; the user's other options stay as they are.

use std::path::Path;

/// Where xkeyboard-config keeps its list.
pub const EVDEV_XML: &str = "/usr/share/X11/xkb/rules/evdev.xml";

/// A layout or one of its variants.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XkbLayout {
    /// "ch".
    pub layout: String,
    /// "" for the layout itself, else "fr".
    pub variant: String,
    /// "German (Switzerland)", "French (Switzerland)".
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    pub layouts: Vec<XkbLayout>,
    /// (option, description): "grp:win_space_toggle", "Win+Space".
    pub options: Vec<(String, String)>,
}

impl Registry {
    /// The list on this system; empty when xkeyboard-config isn't installed.
    pub fn load() -> Self {
        Self::read(Path::new(EVDEV_XML))
    }

    pub fn read(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|text| Self::parse(&text)).unwrap_or_default()
    }

    pub fn parse(text: &str) -> Self {
        let root = parse_xml(text);
        let mut registry = Self::default();
        for layout in root.all("layout") {
            let Some(item) = layout.child("configItem") else { continue };
            let name = item.text_of("name");
            if name.is_empty() {
                continue;
            }
            registry.layouts.push(XkbLayout {
                layout: name.clone(),
                variant: String::new(),
                description: item.text_of("description"),
            });
            for variant in layout.child("variantList").map(|v| v.all("variant")).unwrap_or_default() {
                if let Some(item) = variant.child("configItem") {
                    registry.layouts.push(XkbLayout {
                        layout: name.clone(),
                        variant: item.text_of("name"),
                        description: item.text_of("description"),
                    });
                }
            }
        }
        for option in root.all("option") {
            if let Some(item) = option.child("configItem") {
                registry.options.push((item.text_of("name"), item.text_of("description")));
            }
        }
        registry
    }

    /// The name of a layout and variant, else the codes ("ch (fr)").
    pub fn describe(&self, layout: &str, variant: &str) -> String {
        self.layouts
            .iter()
            .find(|l| l.layout == layout && l.variant == variant)
            .map(|l| l.description.clone())
            .unwrap_or_else(|| if variant.is_empty() { layout.to_owned() } else { format!("{layout} ({variant})") })
    }

    /// The layouts whose name or code has every word of `query`, layouts before their variants.
    pub fn search(&self, query: &str) -> Vec<&XkbLayout> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut found: Vec<&XkbLayout> = self
            .layouts
            .iter()
            .filter(|l| {
                let hay = format!("{} {} {}", l.description, l.layout, l.variant).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect();
        // A code typed exactly ("us") comes first.
        found.sort_by_key(|l| (!words.contains(&l.layout), !l.variant.is_empty()));
        found
    }

    fn option_description(&self, option: &str) -> Option<&str> {
        self.options.iter().find(|(o, _)| o == option).map(|(_, d)| d.as_str())
    }
}

/// One of the Keyboard page's choices: which xkb options belong to it, and its values.
pub struct Choice {
    /// The options that are this choice's ("grp:" for the switch keys).
    owns: fn(&str) -> bool,
    /// (option, label); "" is "none of them".
    pub values: &'static [(&'static str, &'static str)],
}

pub const SWITCH: Choice = Choice {
    owns: |o| o.starts_with("grp:"),
    values: &[
        ("", "No keys"),
        ("grp:win_space_toggle", "Super + Space"),
        ("grp:alt_shift_toggle", "Alt + Shift"),
        ("grp:ctrl_shift_toggle", "Ctrl + Shift"),
        ("grp:ctrl_alt_toggle", "Ctrl + Alt"),
        ("grp:alt_space_toggle", "Alt + Space"),
        ("grp:shifts_toggle", "Both Shift keys"),
        ("grp:caps_toggle", "Caps Lock"),
        ("grp:toggle", "Right Alt"),
        ("grp:menu_toggle", "Menu"),
    ],
};

pub const CAPS: Choice = Choice {
    owns: |o| o.starts_with("caps:") || o == "ctrl:nocaps" || o == "ctrl:swapcaps",
    values: &[
        ("", "Caps Lock"),
        ("ctrl:nocaps", "Ctrl"),
        ("ctrl:swapcaps", "Ctrl, and Ctrl as Caps Lock"),
        ("caps:escape", "Escape"),
        ("caps:swapescape", "Escape, and Escape as Caps Lock"),
        ("caps:backspace", "Backspace"),
        ("caps:super", "Super"),
        ("caps:none", "Nothing"),
    ],
};

pub const COMPOSE: Choice = Choice {
    owns: |o| o.starts_with("compose:"),
    values: &[
        ("", "Off"),
        ("compose:ralt", "Right Alt"),
        ("compose:rctrl", "Right Ctrl"),
        ("compose:rwin", "Right Super"),
        ("compose:menu", "Menu"),
        ("compose:caps", "Caps Lock"),
        ("compose:sclk", "Scroll Lock"),
        ("compose:prsc", "Print Screen"),
    ],
};

impl Choice {
    /// This choice's option in `options` ("" for none).
    pub fn current<'a>(&self, options: &'a str) -> &'a str {
        split(options).find(|o| (self.owns)(o)).unwrap_or("")
    }

    /// The labels to show, with an option the user set that isn't one of the values (another
    /// `grp:` one, say) at the end, named as xkeyboard-config names it; and the one in use.
    pub fn labels(&self, options: &str, registry: &Registry) -> (Vec<String>, usize) {
        let mut labels: Vec<String> = self.values.iter().map(|(_, l)| (*l).to_owned()).collect();
        let current = self.current(options);
        let index = match self.values.iter().position(|(o, _)| *o == current) {
            Some(i) => i,
            None => {
                labels.push(registry.option_description(current).unwrap_or(current).to_owned());
                labels.len() - 1
            }
        };
        (labels, index)
    }

    /// The options with this choice set to value `index` (past the values: kept as it is).
    pub fn set(&self, options: &str, index: usize) -> String {
        let Some((value, _)) = self.values.get(index) else { return options.to_owned() };
        let mut out: Vec<&str> = split(options).filter(|o| !(self.owns)(o)).collect();
        if !value.is_empty() {
            out.push(value);
        }
        out.join(",")
    }

    /// The label of the value set, if it's one of the values.
    pub fn label(&self, options: &str) -> Option<&'static str> {
        let current = self.current(options);
        self.values.iter().find(|(o, _)| *o == current).map(|(_, l)| *l)
    }
}

fn split(options: &str) -> impl Iterator<Item = &str> {
    options.split(',').map(str::trim).filter(|o| !o.is_empty())
}

/// An element of the XML, with its text and children; attributes aren't needed.
#[derive(Debug, Default)]
struct Element {
    name: String,
    text: String,
    children: Vec<Element>,
}

impl Element {
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    fn text_of(&self, name: &str) -> String {
        self.child(name).map(|c| c.text.trim().to_owned()).unwrap_or_default()
    }

    /// Every element called `name` below this one, not looking inside those.
    fn all(&self, name: &str) -> Vec<&Element> {
        let mut out = Vec::new();
        for c in &self.children {
            if c.name == name {
                out.push(c);
            } else {
                out.extend(c.all(name));
            }
        }
        out
    }
}

/// Just enough XML for xkeyboard-config's files: elements, text and the five entities; comments,
/// the declaration and the doctype are skipped.
fn parse_xml(text: &str) -> Element {
    let mut stack = vec![Element::default()];
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        if let Some(top) = stack.last_mut() {
            top.text += &unescape(&rest[..start]);
        }
        rest = &rest[start..];
        let skip = if rest.starts_with("<!--") {
            rest.find("-->").map(|e| e + 3)
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            rest.find('>').map(|e| e + 1)
        } else {
            None
        };
        if let Some(n) = skip {
            rest = &rest[n..];
            continue;
        }
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            // Closes the element; a stray closing tag is ignored.
            if stack.len() > 1 && stack.last().is_some_and(|e| e.name == name.trim()) {
                let done = stack.pop().unwrap_or_default();
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(done);
                }
            }
            continue;
        }
        let empty = tag.ends_with('/');
        let name = tag.trim_end_matches('/').split_whitespace().next().unwrap_or("").to_owned();
        let element = Element { name, ..Element::default() };
        if empty {
            if let Some(parent) = stack.last_mut() {
                parent.children.push(element);
            }
        } else {
            stack.push(element);
        }
    }
    // Unclosed elements, in a broken file, still count.
    while stack.len() > 1 {
        let done = stack.pop().unwrap_or_default();
        if let Some(parent) = stack.last_mut() {
            parent.children.push(done);
        }
    }
    stack.pop().unwrap_or_default()
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVDEV: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xkbConfigRegistry SYSTEM "xkb.dtd">
<xkbConfigRegistry version="1.1">
  <modelList>
    <model><configItem><name>pc105</name><description>Generic 105-key PC</description></configItem></model>
  </modelList>
  <layoutList>
    <layout>
      <configItem>
        <name>ch</name>
        <!-- Keyboard indicator for German layouts -->
        <shortDescription>de</shortDescription>
        <description>German (Switzerland)</description>
        <languageList><iso639Id>deu</iso639Id></languageList>
      </configItem>
      <variantList>
        <variant>
          <configItem>
            <name>fr</name>
            <description>French (Switzerland)</description>
          </configItem>
        </variant>
      </variantList>
    </layout>
    <layout>
      <configItem popularity="exotic">
        <name>us</name>
        <description>English (US)</description>
      </configItem>
      <variantList>
        <variant><configItem><name>intl</name><description>English (US, intl., with dead keys)</description></configItem></variant>
        <variant><configItem><name>dvorak</name><description>English (Dvorak)</description></configItem></variant>
      </variantList>
    </layout>
    <layout><configItem><name>rs</name><description>Serbian &amp; more</description></configItem></layout>
  </layoutList>
  <optionList>
    <group allowMultipleSelection="true">
      <configItem><name>grp</name><description>Switching to another layout</description></configItem>
      <option><configItem><name>grp:win_space_toggle</name><description>Win+Space</description></configItem></option>
      <option><configItem><name>grp:lctrl_lwin_toggle</name><description>Left Ctrl+Left Win</description></configItem></option>
    </group>
  </optionList>
</xkbConfigRegistry>
"#;

    #[test]
    fn reads_layouts_variants_and_options() {
        let r = Registry::parse(EVDEV);
        let names: Vec<(&str, &str)> = r.layouts.iter().map(|l| (l.layout.as_str(), l.variant.as_str())).collect();
        assert_eq!(names, [("ch", ""), ("ch", "fr"), ("us", ""), ("us", "intl"), ("us", "dvorak"), ("rs", "")]);
        assert_eq!(r.describe("ch", ""), "German (Switzerland)");
        assert_eq!(r.describe("us", "intl"), "English (US, intl., with dead keys)");
        assert_eq!(r.describe("rs", ""), "Serbian & more");
        // What the list doesn't have.
        assert_eq!(r.describe("xx", "yy"), "xx (yy)");
        assert_eq!(r.options.len(), 2);
        assert_eq!(r.options[0], ("grp:win_space_toggle".into(), "Win+Space".into()));
    }

    #[test]
    fn a_missing_list_is_empty() {
        let r = Registry::read(Path::new("/nonexistent/evdev.xml"));
        assert!(r.layouts.is_empty());
        assert_eq!(r.describe("ch", ""), "ch");
        assert!(Registry::parse("<xkbConfigRegistry><layoutList><layout><configItem>").layouts.is_empty());
    }

    #[test]
    fn search_finds_names_and_codes() {
        let r = Registry::parse(EVDEV);
        let found = |q: &str| r.search(q).iter().map(|l| format!("{}:{}", l.layout, l.variant)).collect::<Vec<_>>();
        assert_eq!(found("switz"), ["ch:", "ch:fr"]);
        assert_eq!(found("french switz"), ["ch:fr"]);
        assert_eq!(found("us"), ["us:", "us:intl", "us:dvorak"]);
        assert_eq!(found("DVORAK"), ["us:dvorak"]);
        assert!(found("klingon").is_empty());
    }

    #[test]
    fn options_merge_and_keep_the_users_own() {
        let options = "lv3:ralt_switch,grp:alt_shift_toggle,caps:escape,terminate:ctrl_alt_bksp";
        assert_eq!(SWITCH.current(options), "grp:alt_shift_toggle");
        assert_eq!(SWITCH.label(options), Some("Alt + Shift"));
        let switched = SWITCH.set(options, 1);
        assert_eq!(switched, "lv3:ralt_switch,caps:escape,terminate:ctrl_alt_bksp,grp:win_space_toggle");
        // Caps Lock as Ctrl replaces caps:escape, which is the same choice.
        let caps = CAPS.set(&switched, 1);
        assert_eq!(caps, "lv3:ralt_switch,terminate:ctrl_alt_bksp,grp:win_space_toggle,ctrl:nocaps");
        assert_eq!(CAPS.current(&caps), "ctrl:nocaps");
        let compose = COMPOSE.set(&caps, 1);
        assert_eq!(compose, "lv3:ralt_switch,terminate:ctrl_alt_bksp,grp:win_space_toggle,ctrl:nocaps,compose:ralt");
        // Back to none: only that option goes.
        assert_eq!(COMPOSE.set(&compose, 0), caps);
        assert_eq!(CAPS.set(&SWITCH.set(&caps, 0), 0), "lv3:ralt_switch,terminate:ctrl_alt_bksp");
        assert_eq!(SWITCH.set("", 1), "grp:win_space_toggle");
        // A value past the list keeps the options.
        assert_eq!(SWITCH.set(options, 99), options);
    }

    #[test]
    fn an_option_lituus_doesnt_offer_is_shown_by_its_name() {
        let r = Registry::parse(EVDEV);
        let (labels, i) = SWITCH.labels("grp:lctrl_lwin_toggle", &r);
        assert_eq!(labels[i], "Left Ctrl+Left Win");
        assert_eq!(labels.len(), SWITCH.values.len() + 1);
        assert_eq!(SWITCH.label("grp:lctrl_lwin_toggle"), None);
        let (labels, i) = CAPS.labels("caps:hyper", &r);
        assert_eq!(labels[i], "caps:hyper");
        let (_, i) = COMPOSE.labels("", &r);
        assert_eq!(i, 0);
    }
}
