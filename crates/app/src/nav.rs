//! The sidebar: its pages in groups, each with a short status, filtered by the search field.
//! Settings has its own button at the bottom, so it's listed only as a search result.

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
}

const GROUPS: [&str; 3] = ["CONNECTIONS", "DEVICES", LITUUS];
/// The group of the pages that are only listed while searching.
pub const LITUUS: &str = "LITUUS";

fn matches(entry: &Entry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entry.group != LITUUS;
    }
    entry.label.to_lowercase().contains(&query) || entry.keywords.split(' ').any(|k| k.starts_with(&query))
}

/// The groups that have a page matching the search, in order.
pub fn groups(entries: &[Entry], query: &str, current: &str) -> Vec<NavGroup> {
    GROUPS
        .iter()
        .filter_map(|&group| {
            let items: Vec<NavItem> = entries
                .iter()
                .filter(|e| e.group == group && matches(e, query))
                .map(|e| NavItem {
                    id: SharedString::from(e.id),
                    label: SharedString::from(e.label),
                    sub: SharedString::from(e.status.as_str()),
                    icon: SharedString::from(e.icon),
                    current: e.id == current,
                })
                .collect();
            (!items.is_empty())
                .then(|| NavGroup { title: SharedString::from(group), items: ModelRc::new(VecModel::from(items)) })
        })
        .collect()
}

/// The page Enter in the search field goes to.
pub fn first_match<'a>(entries: &'a [Entry], query: &str) -> Option<&'a str> {
    GROUPS.iter().find_map(|&group| entries.iter().find(|e| e.group == group && matches(e, query)).map(|e| e.id))
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
                keywords: "wi-fi wifi wired ethernet vpn",
                status: "Home".into(),
            },
            Entry {
                id: "bluetooth",
                label: "Bluetooth",
                icon: "bluetooth",
                group: "CONNECTIONS",
                keywords: "pair headphones",
                status: "2 on".into(),
            },
            Entry {
                id: "settings",
                label: "Settings",
                icon: "settings",
                group: LITUUS,
                keywords: "appearance theme colours dark light",
                status: String::new(),
            },
        ]
    }

    #[test]
    fn groups_keep_their_order_and_drop_when_empty() {
        let g = groups(&entries(), "", "bluetooth");
        let titles: Vec<String> = g.iter().map(|g| g.title.to_string()).collect();
        // Settings is the button below the list, not one of its pages.
        assert_eq!(titles, ["CONNECTIONS"]);
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
        assert_eq!(ids("WIFI"), ["network"]);
        // The app's own look is in Settings.
        assert_eq!(ids("dark"), ["settings"]);
        assert_eq!(ids("Appearance"), ["settings"]);
        assert!(ids("zzz").is_empty());
        assert_eq!(first_match(&entries(), "head"), Some("bluetooth"));
        assert_eq!(first_match(&entries(), "theme"), Some("settings"));
        assert_eq!(first_match(&entries(), "zzz"), None);
    }
}
