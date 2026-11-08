//! hyprpaper.conf (hyprpaper 0.8 and later): `wallpaper { monitor = …  path = …  fit_mode = … }`
//! blocks, which hyprpaper reads at start; what it's told while it runs isn't kept anywhere. Lituus
//! writes one block per monitor it sets, and keeps the rest of the file (splash, other blocks,
//! comments) as it is. A block whose path is a folder is hyprpaper's slideshow: a picture from it
//! every `timeout` seconds, in the folder's order or shuffled.

/// A `wallpaper` block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Empty: hyprpaper's fallback for the monitors without one.
    pub monitor: String,
    pub path: String,
    /// "cover", "contain", "tile" or "fill" (stretched).
    pub fit_mode: String,
    /// A slideshow (`path` is a folder): seconds per picture; 0 for none.
    pub timeout: u32,
    /// The slideshow's order: "default" (the folder's) or "random-shuffle"; empty for none.
    pub order: String,
    /// The slideshow takes the pictures in the folder's subfolders too.
    pub recursive: bool,
}

impl Entry {
    /// One picture on a monitor.
    pub fn picture(monitor: &str, path: &str, fit_mode: &str) -> Entry {
        Entry {
            monitor: monitor.into(),
            path: path.into(),
            fit_mode: fit_mode.into(),
            timeout: 0,
            order: String::new(),
            recursive: false,
        }
    }
}

/// The fit mode as hyprpaper.conf writes it.
pub fn fit_mode(fill: crate::Fill) -> &'static str {
    match fill {
        crate::Fill::Cover | crate::Fill::Center => "cover",
        crate::Fill::Contain => "contain",
        crate::Fill::Stretch => "fill",
        crate::Fill::Tile => "tile",
    }
}

/// A block's span in the file's lines: from its `wallpaper {` line to its `}`, and what it holds.
fn blocks(text: &str) -> Vec<(usize, usize, Entry)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].split('#').next().unwrap_or_default().trim();
        if line.starts_with("wallpaper") && line.trim_start_matches("wallpaper").trim_start().starts_with('{') {
            let start = i;
            let mut entry = Entry::picture("", "", "cover");
            // `wallpaper { monitor = DP-1 }` on one line, or over several.
            let mut body = line.split_once('{').map(|(_, b)| b.to_owned()).unwrap_or_default();
            while !body.contains('}') && i + 1 < lines.len() {
                i += 1;
                body.push('\n');
                body.push_str(lines[i].split('#').next().unwrap_or_default());
            }
            for field in body.split(['\n', '}']) {
                if let Some((k, v)) = field.split_once('=') {
                    let v = v.trim().to_owned();
                    match k.trim() {
                        "monitor" => entry.monitor = v,
                        "path" => entry.path = v,
                        "fit_mode" => entry.fit_mode = v,
                        "timeout" => entry.timeout = v.parse().unwrap_or(0),
                        "order" => entry.order = v,
                        "recursive" => entry.recursive = matches!(v.as_str(), "1" | "true" | "yes" | "on"),
                        _ => {}
                    }
                }
            }
            out.push((start, i, entry));
        }
        i += 1;
    }
    out
}

/// The `wallpaper` blocks in hyprpaper.conf's text.
pub fn entries(text: &str) -> Vec<Entry> {
    blocks(text).into_iter().map(|(_, _, e)| e).collect()
}

/// The file with a block for each of `set` (replacing that monitor's, or added at the end), the
/// rest as it was.
pub fn conf_text(text: &str, set: &[Entry]) -> String {
    let found = blocks(text);
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let block = |e: &Entry| {
        let mut b = format!(
            "wallpaper {{\n    monitor = {}\n    path = {}\n    fit_mode = {}\n",
            e.monitor, e.path, e.fit_mode
        );
        if e.timeout > 0 {
            b.push_str(&format!("    timeout = {}\n", e.timeout));
            if !e.order.is_empty() {
                b.push_str(&format!("    order = {}\n", e.order));
            }
            if e.recursive {
                b.push_str("    recursive = true\n");
            }
        }
        b.push('}');
        b
    };
    let mut added = Vec::new();
    // From the end, so the earlier spans stay where they are.
    let mut replaced: Vec<(usize, usize, String)> = Vec::new();
    for e in set {
        match found.iter().find(|(_, _, f)| f.monitor == e.monitor) {
            Some((start, end, _)) => replaced.push((*start, *end, block(e))),
            None => added.push(block(e)),
        }
    }
    replaced.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    for (start, end, text) in replaced {
        lines.splice(start..=end, text.lines().map(str::to_owned).collect::<Vec<_>>());
    }
    let mut out = lines.join("\n");
    if !added.is_empty() {
        if !out.trim().is_empty() {
            out = format!("{}\n\n", out.trim_end());
        }
        out.push_str(&added.join("\n\n"));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_rest_of_the_file() {
        let text = "# My hyprpaper\nsplash = false\n\nwallpaper {\n    monitor = DP-3\n    path = ~/w/old.jpg # the old one\n}\n\nwallpaper { monitor = DP-4\n path = ~/w/b.png\n fit_mode = tile }\n";
        assert_eq!(
            entries(text),
            [Entry::picture("DP-3", "~/w/old.jpg", "cover"), Entry::picture("DP-4", "~/w/b.png", "tile"),]
        );
        let new = conf_text(
            text,
            &[Entry::picture("DP-3", "/w/alps.jpg", "fill"), Entry::picture("HDMI-A-3", "/w/lake.png", "cover")],
        );
        assert!(new.starts_with("# My hyprpaper\nsplash = false\n\nwallpaper {\n    monitor = DP-3\n    path = /w/alps.jpg\n    fit_mode = fill\n}\n"), "{new}");
        assert!(new.contains("wallpaper { monitor = DP-4\n path = ~/w/b.png\n fit_mode = tile }"), "{new}");
        assert!(
            new.ends_with("\n\nwallpaper {\n    monitor = HDMI-A-3\n    path = /w/lake.png\n    fit_mode = cover\n}\n"),
            "{new}"
        );
        assert_eq!(entries(&new).len(), 3);
        // A slideshow: the folder, a picture every 30 minutes, shuffled, with its subfolders.
        let show = Entry {
            timeout: 1800,
            order: "random-shuffle".into(),
            recursive: true,
            ..Entry::picture("DP-3", "/w", "cover")
        };
        let new = conf_text(&new, std::slice::from_ref(&show));
        assert!(new.contains("wallpaper {\n    monitor = DP-3\n    path = /w\n    fit_mode = cover\n    timeout = 1800\n    order = random-shuffle\n    recursive = true\n}\n"), "{new}");
        assert_eq!(entries(&new)[0], show);
        // And back to one picture.
        let new = conf_text(&new, &[Entry::picture("DP-3", "/w/a.jpg", "cover")]);
        assert!(!new.contains("timeout") && !new.contains("recursive"), "{new}");
        assert_eq!(
            conf_text("", &[Entry::picture("DP-1", "/a", "cover")]),
            "wallpaper {\n    monitor = DP-1\n    path = /a\n    fit_mode = cover\n}\n"
        );
    }
}
