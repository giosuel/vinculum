//! The sidebar: its pages in groups, each with a short status, filtered by the search field.
//! Network's pages (Wi-Fi, Wired, VPN, Hotspot) and Input's (Keyboard, Mouse & touchpad, Shortcuts) are their
//! sub-items, shown under them unless they're folded, and when the search finds them. Settings has its own button at the bottom, so it's
//! listed only as a search result.

use slint::{ModelRc, SharedString, VecModel};

use crate::{NavGroup, NavItem};

/// A page in the sidebar.
pub struct Entry {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub group: &'static str,
    /// More words the search finds it by.
    pub keywords: &'static str,
    pub status: String,
    /// The item it's a sub-item of.
    pub parent: Option<&'static str>,
}

const GROUPS: [&str; 3] = ["CONNECTIONS", "DEVICES", LITUUS];
/// The group of the pages that are only listed while searching.
pub const LITUUS: &str = "LITUUS";

fn matches(entry: &Entry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entry.group != LITUUS;
    }
    // A keyword, or words in a row ("tap to click", "caps lo").
    entry.label.to_lowercase().contains(&query) || format!(" {}", entry.keywords).contains(&format!(" {query}"))
}

/// The items shown, in order: each top-level item (if it or a sub-item matches the search), then
/// its sub-items: the ones the search found, else all of them unless the item is folded.
fn shown<'a>(entries: &'a [Entry], query: &str, folded: &[String]) -> Vec<&'a Entry> {
    let searching = !query.trim().is_empty();
    let mut shown = Vec::new();
    for &group in &GROUPS {
        for e in entries.iter().filter(|e| e.group == group && e.parent.is_none()) {
            let subs: Vec<&Entry> = entries
                .iter()
                .filter(|s| s.parent == Some(e.id))
                .filter(|s| if searching { matches(s, query) } else { !folded.iter().any(|f| f == e.id) })
                .collect();
            if matches(e, query) || !subs.is_empty() {
                shown.push(e);
                shown.extend(subs);
            }
        }
    }
    shown
}

/// The groups that have a page matching the search, in order. `folded`: the items whose
/// sub-items are folded away. The pages are one Tab stop: the current one, or the first when
/// it isn't listed; the arrow keys go by each item's place in the list.
pub fn groups(entries: &[Entry], query: &str, current: &str, folded: &[String]) -> Vec<NavGroup> {
    let current_parent = entries.iter().find(|e| e.id == current).and_then(|e| e.parent);
    let shown = shown(entries, query, folded);
    let current_shown = shown.iter().any(|e| e.id == current);
    // On a sub-page that's folded away, its item stands for it.
    let is_current = |e: &Entry| e.id == current || (current_parent == Some(e.id) && !current_shown);
    let place = |id: &str| shown.iter().position(|e| e.id == id).map_or(-1, |i| i as i32);
    let stop = shown.iter().position(|e| is_current(e)).unwrap_or(0) as i32;
    GROUPS
        .iter()
        .filter_map(|&group| {
            let items: Vec<NavItem> = shown
                .iter()
                .filter(|e| e.group == group)
                .map(|e| {
                    // An item showing sub-items leaves the statuses to them.
                    let subs_shown = shown.iter().any(|s| s.parent == Some(e.id));
                    let on_sub = current_parent == Some(e.id);
                    let sub = if subs_shown { "" } else { e.status.as_str() };
                    NavItem {
                        id: SharedString::from(e.id),
                        label: SharedString::from(e.label),
                        sub: SharedString::from(sub),
                        spoken: SharedString::from(spoken(sub)),
                        icon: SharedString::from(e.icon),
                        current: is_current(e),
                        open: on_sub && current_shown,
                        indent: e.parent.is_some(),
                        foldable: entries.iter().any(|s| s.parent == Some(e.id)),
                        folded: folded.iter().any(|f| f == e.id),
                        stop: place(e.id) == stop,
                        place: place(e.id),
                        parent: e.parent.map_or(-1, place),
                    }
                })
                .collect();
            (!items.is_empty())
                .then(|| NavGroup { title: SharedString::from(group), items: ModelRc::new(VecModel::from(items)) })
        })
        .collect()
}

/// A status as a screen reader says it: the counts in full ("2 on" is "2 connected").
pub fn spoken(status: &str) -> String {
    match status.strip_suffix(" on") {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => format!("{n} connected"),
        _ => status.to_owned(),
    }
}

/// The page Enter in the search field goes to: the first item whose name matches ("wire" is
/// Wired, not Wi-Fi for "wireless"), else the first found by a keyword.
pub fn first_match<'a>(entries: &'a [Entry], query: &str) -> Option<&'a str> {
    let shown = shown(entries, query, &[]);
    let query = query.trim().to_lowercase();
    let by_name = shown.iter().find(|e| e.label.to_lowercase().contains(&query));
    by_name.or_else(|| shown.iter().find(|e| matches(e, &query))).map(|e| e.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn entries() -> Vec<Entry> {
        vec![
            Entry {
                id: "network",
                label: "Network",
                icon: "wifi",
                group: "CONNECTIONS",
                keywords: "internet connections",
                status: "Home".into(),
                parent: None,
            },
            Entry {
                id: "wifi",
                label: "Wi-Fi",
                icon: "wifi",
                group: "CONNECTIONS",
                keywords: "wifi wireless",
                status: "Home".into(),
                parent: Some("network"),
            },
            Entry {
                id: "wired",
                label: "Wired",
                icon: "wired",
                group: "CONNECTIONS",
                keywords: "ethernet",
                status: "unplugged".into(),
                parent: Some("network"),
            },
            Entry {
                id: "vpn",
                label: "VPN",
                icon: "vpn",
                group: "CONNECTIONS",
                keywords: "wireguard",
                status: "off".into(),
                parent: Some("network"),
            },
            Entry {
                id: "bluetooth",
                label: "Bluetooth",
                icon: "bluetooth",
                group: "CONNECTIONS",
                keywords: "pair headphones",
                status: "2 on".into(),
                parent: None,
            },
            Entry {
                id: "mouse",
                label: "Mouse & touchpad",
                icon: "mouse",
                group: "DEVICES",
                keywords: "natural scrolling tap to click",
                status: "1 on".into(),
                parent: None,
            },
            Entry {
                id: "settings",
                label: "Settings",
                icon: "settings",
                group: LITUUS,
                keywords: "appearance theme colours dark light",
                status: String::new(),
                parent: None,
            },
        ]
    }

    #[test]
    fn groups_keep_their_order_and_drop_when_empty() {
        let g = groups(&entries(), "", "bluetooth", &[]);
        let titles: Vec<String> = g.iter().map(|g| g.title.to_string()).collect();
        // Settings is the button below the list, not one of its pages.
        assert_eq!(titles, ["CONNECTIONS", "DEVICES"]);
        assert!(g[0].items.row_data(4).unwrap().current);
        assert_eq!(g[0].items.row_data(4).unwrap().sub, "2 on");
    }

    #[test]
    fn search_finds_labels_and_keywords() {
        let ids = |q: &str| -> Vec<String> {
            groups(&entries(), q, "", &["network".to_owned()])
                .iter()
                .flat_map(|g| g.items.iter().map(|i| i.id.to_string()).collect::<Vec<_>>())
                .collect()
        };
        assert_eq!(ids("blue"), ["bluetooth"]);
        // A sub-item is found under its item, even when that's folded.
        assert_eq!(ids("WIFI"), ["network", "wifi"]);
        assert_eq!(ids(""), ["network", "bluetooth", "mouse"]);
        // The app's own look is in Settings.
        assert_eq!(ids("dark"), ["settings"]);
        assert_eq!(ids("Appearance"), ["settings"]);
        assert!(ids("zzz").is_empty());
        assert_eq!(first_match(&entries(), "head"), Some("bluetooth"));
        assert_eq!(first_match(&entries(), "wireg"), Some("vpn"));
        assert_eq!(first_match(&entries(), "wire"), Some("wired"));
        assert_eq!(first_match(&entries(), "net"), Some("network"));
        assert_eq!(first_match(&entries(), "theme"), Some("settings"));
        assert_eq!(first_match(&entries(), "zzz"), None);
        // Words in a row, the last one begun.
        assert_eq!(first_match(&entries(), "tap to cl"), Some("mouse"));
        assert_eq!(first_match(&entries(), "Natural Scrolling"), Some("mouse"));
        assert_eq!(first_match(&entries(), "to click"), Some("mouse"));
        assert_eq!(first_match(&entries(), "scrolling tap"), Some("mouse"));
        assert_eq!(first_match(&entries(), "o click"), None);
    }

    #[test]
    fn network_folds_its_sub_items() {
        let items = |current: &str, folded: &[&str]| -> Vec<(String, String, bool, bool, bool)> {
            let folded: Vec<String> = folded.iter().map(|f| f.to_string()).collect();
            groups(&entries(), "", current, &folded)[0]
                .items
                .iter()
                .map(|i| (i.id.to_string(), i.sub.to_string(), i.current, i.open, i.folded))
                .collect()
        };
        let row = |id: &str, sub: &str, current, open, folded| (id.to_owned(), sub.to_owned(), current, open, folded);
        // Unfolded, on one of its pages or not, so the list below never moves.
        assert_eq!(
            items("vpn", &[]),
            [
                row("network", "", false, true, false),
                row("wifi", "Home", false, false, false),
                row("wired", "unplugged", false, false, false),
                row("vpn", "off", true, false, false),
                row("bluetooth", "2 on", false, false, false),
            ]
        );
        assert_eq!(items("bluetooth", &[])[0], row("network", "", false, false, false));
        assert_eq!(items("bluetooth", &[]).len(), 5);
        // Folded, Network is one item with its status, and stands for its current sub-page.
        assert_eq!(
            items("vpn", &["network"]),
            [row("network", "Home", true, false, true), row("bluetooth", "2 on", false, false, false)]
        );
        assert_eq!(items("bluetooth", &["network"])[0], row("network", "Home", false, false, true));
        // Only Network has a chevron.
        let g = groups(&entries(), "", "", &[]);
        let foldable: Vec<bool> = g[0].items.iter().map(|i| i.foldable).collect();
        assert_eq!(foldable, [true, false, false, false, false]);
    }

    /// The list is one Tab stop, the current page; the arrow keys go by place, and ← from a
    /// sub-item to its item's place.
    #[test]
    fn the_current_page_is_the_tab_stop() {
        let items = |query: &str, current: &str, folded: &[&str]| -> Vec<(String, bool, i32, i32)> {
            let folded: Vec<String> = folded.iter().map(|f| f.to_string()).collect();
            groups(&entries(), query, current, &folded)
                .iter()
                .flat_map(|g| g.items.iter().map(|i| (i.id.to_string(), i.stop, i.place, i.parent)).collect::<Vec<_>>())
                .collect()
        };
        let row = |id: &str, stop, place, parent| (id.to_owned(), stop, place, parent);
        assert_eq!(
            items("", "wired", &[]),
            [
                row("network", false, 0, -1),
                row("wifi", false, 1, 0),
                row("wired", true, 2, 0),
                row("vpn", false, 3, 0),
                row("bluetooth", false, 4, -1),
                row("mouse", false, 5, -1),
            ]
        );
        // Folded, Network stands for its page; the places close up.
        assert_eq!(
            items("", "wired", &["network"]),
            [row("network", true, 0, -1), row("bluetooth", false, 1, -1), row("mouse", false, 2, -1)]
        );
        // Settings has its own button: the first page is the stop.
        assert_eq!(items("", "settings", &[])[0], row("network", true, 0, -1));
        assert_eq!(items("blue", "mouse", &[]), [row("bluetooth", true, 0, -1)]);
    }

    #[test]
    fn the_search_shows_sub_items_of_a_folded_item() {
        let folded = ["network".to_owned()];
        let items = |query: &str, current: &str| -> Vec<(String, bool)> {
            groups(&entries(), query, current, &folded)[0].items.iter().map(|i| (i.id.to_string(), i.current)).collect()
        };
        assert_eq!(items("ether", ""), [("network".to_owned(), false), ("wired".to_owned(), false)]);
        // The current page is among the results: it's marked, not Network.
        assert_eq!(items("ether", "wired"), [("network".to_owned(), false), ("wired".to_owned(), true)]);
        // It isn't: Network stands for it.
        assert_eq!(items("wireg", "wifi"), [("network".to_owned(), true), ("vpn".to_owned(), false)]);
    }

    #[test]
    fn counts_are_said_in_full() {
        assert_eq!(spoken("2 on"), "2 connected");
        assert_eq!(spoken("On"), "On");
        assert_eq!(spoken("Lumen Home"), "Lumen Home");
        assert_eq!(spoken("Turned on"), "Turned on");
        let mut e = entries();
        e[2].status = "1 on".into();
        let item = groups(&e, "", "", &[])[0].items.iter().find(|i| i.id == "wired").unwrap();
        assert_eq!((item.sub.as_str(), item.spoken.as_str()), ("1 on", "1 connected"));
    }
}
