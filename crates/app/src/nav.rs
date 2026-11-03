//! The sidebar: its pages in groups, each with a short status, filtered by the search field.
//! Network's pages (Wi-Fi, Wired, VPN, Hotspot) are its sub-items, shown while one of them is
//! open or found by the search.

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

const GROUPS: [&str; 3] = ["CONNECTIONS", "DEVICES", "PERSONAL"];

fn matches(entry: &Entry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || entry.label.to_lowercase().contains(&query)
        || entry.keywords.split(' ').any(|k| k.starts_with(&query))
}

/// The items shown, in order: each top-level item (if it or a sub-item matches the search), then
/// its sub-items, all of them while one is the current page, else the ones the search found.
fn shown<'a>(entries: &'a [Entry], query: &str, current: &str) -> Vec<&'a Entry> {
    let searching = !query.trim().is_empty();
    let open = entries.iter().find(|e| e.id == current).and_then(|e| e.parent);
    let mut shown = Vec::new();
    for &group in &GROUPS {
        for e in entries.iter().filter(|e| e.group == group && e.parent.is_none()) {
            let subs: Vec<&Entry> = entries
                .iter()
                .filter(|s| s.parent == Some(e.id))
                .filter(|s| if searching { matches(s, query) } else { open == Some(e.id) })
                .collect();
            if matches(e, query) || !subs.is_empty() {
                shown.push(e);
                shown.extend(subs);
            }
        }
    }
    shown
}

/// The groups that have a page matching the search, in order.
pub fn groups(entries: &[Entry], query: &str, current: &str) -> Vec<NavGroup> {
    let open = entries.iter().find(|e| e.id == current).and_then(|e| e.parent);
    let shown = shown(entries, query, current);
    GROUPS
        .iter()
        .filter_map(|&group| {
            let items: Vec<NavItem> = shown
                .iter()
                .filter(|e| e.group == group)
                .map(|e| {
                    // An open item shows its sub-items' statuses instead of its own.
                    let is_open = open == Some(e.id);
                    NavItem {
                        id: SharedString::from(e.id),
                        label: SharedString::from(e.label),
                        sub: SharedString::from(if is_open { "" } else { e.status.as_str() }),
                        icon: SharedString::from(e.icon),
                        current: e.id == current,
                        open: is_open,
                        indent: e.parent.is_some(),
                    }
                })
                .collect();
            (!items.is_empty())
                .then(|| NavGroup { title: SharedString::from(group), items: ModelRc::new(VecModel::from(items)) })
        })
        .collect()
}

/// The page Enter in the search field goes to: the first item whose name matches ("wire" is
/// Wired, not Wi-Fi for "wireless"), else the first found by a keyword.
pub fn first_match<'a>(entries: &'a [Entry], query: &str) -> Option<&'a str> {
    let shown = shown(entries, query, "");
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
                id: "appearance",
                label: "Appearance",
                icon: "palette",
                group: "PERSONAL",
                keywords: "theme colours dark light",
                status: String::new(),
                parent: None,
            },
        ]
    }

    #[test]
    fn groups_keep_their_order_and_drop_when_empty() {
        let g = groups(&entries(), "", "bluetooth");
        let titles: Vec<String> = g.iter().map(|g| g.title.to_string()).collect();
        assert_eq!(titles, ["CONNECTIONS", "PERSONAL"]);
        assert!(g[0].items.row_data(1).unwrap().current);
        assert_eq!(g[0].items.row_data(0).unwrap().sub, "Home");
    }

    #[test]
    fn search_finds_labels_and_keywords() {
        let ids = |q: &str| -> Vec<String> {
            groups(&entries(), q, "")
                .iter()
                .flat_map(|g| g.items.iter().map(|i| i.id.to_string()).collect::<Vec<_>>())
                .collect()
        };
        assert_eq!(ids("blue"), ["bluetooth"]);
        // A sub-item is found under its item.
        assert_eq!(ids("WIFI"), ["network", "wifi"]);
        assert_eq!(ids("dark"), ["appearance"]);
        assert!(ids("zzz").is_empty());
        assert_eq!(first_match(&entries(), "head"), Some("bluetooth"));
        assert_eq!(first_match(&entries(), "wireg"), Some("vpn"));
        assert_eq!(first_match(&entries(), "wire"), Some("wired"));
        assert_eq!(first_match(&entries(), "net"), Some("network"));
        assert_eq!(first_match(&entries(), "zzz"), None);
    }

    #[test]
    fn network_opens_into_its_sub_items_on_their_pages() {
        let items = |current: &str| -> Vec<(String, String, bool, bool, bool)> {
            groups(&entries(), "", current)[0]
                .items
                .iter()
                .map(|i| (i.id.to_string(), i.sub.to_string(), i.current, i.open, i.indent))
                .collect()
        };
        let row = |id: &str, sub: &str, current, open, indent| (id.to_owned(), sub.to_owned(), current, open, indent);
        assert_eq!(
            items("vpn"),
            [
                row("network", "", false, true, false),
                row("wifi", "Home", false, false, true),
                row("wired", "unplugged", false, false, true),
                row("vpn", "off", true, false, true),
                row("bluetooth", "2 on", false, false, false),
            ]
        );
        // Elsewhere, Network is one item with its status.
        assert_eq!(
            items("bluetooth"),
            [row("network", "Home", false, false, false), row("bluetooth", "2 on", true, false, false)]
        );
    }
}
