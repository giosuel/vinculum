//! Desktop notifications (`org.freedesktop.Notifications`, which mako, swaync and dunst serve) when
//! a connection comes up, goes down or fails, while the window isn't showing it.

use std::collections::HashMap;

use vinculum_nm::{ActiveState, ConnectionKind, Snapshot};
use zbus::zvariant::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    /// "network-wireless-symbolic" and the like.
    pub icon: &'static str,
}

/// The connections that came up or went down between two snapshots.
pub fn changes(before: &Snapshot, after: &Snapshot) -> Vec<Notice> {
    let up = |snap: &Snapshot| -> HashMap<String, String> {
        snap.active
            .iter()
            .filter(|a| a.state == ActiveState::Activated)
            .map(|a| (a.uuid.clone(), a.id.clone()))
            .collect()
    };
    let (was, is) = (up(before), up(after));
    let mut notices = Vec::new();
    for a in after.active.iter().filter(|a| a.state == ActiveState::Activated && !was.contains_key(&a.uuid)) {
        let (kind, icon) = kind(after, &a.uuid, a.vpn);
        notices.push(Notice { summary: format!("Connected to {}", a.id), body: kind.into(), icon });
    }
    for (uuid, id) in was.iter().filter(|(uuid, _)| !is.contains_key(*uuid)) {
        let vpn = before.active.iter().any(|a| a.uuid == *uuid && a.vpn);
        let (kind, icon) = kind(before, uuid, vpn);
        notices.push(Notice { summary: format!("Disconnected from {id}"), body: kind.into(), icon });
    }
    notices
}

pub fn failure(name: &str, reason: &str) -> Notice {
    Notice {
        summary: if name.is_empty() { "Couldn't connect".into() } else { format!("Couldn't connect to {name}") },
        body: reason.into(),
        icon: "network-error-symbolic",
    }
}

fn kind(snap: &Snapshot, uuid: &str, vpn: bool) -> (&'static str, &'static str) {
    match snap.connections.iter().find(|c| c.uuid == uuid).map(|c| &c.kind) {
        Some(ConnectionKind::Wifi { .. }) => ("Wi-Fi", "network-wireless-symbolic"),
        Some(ConnectionKind::Ethernet) => ("Wired", "network-wired-symbolic"),
        Some(ConnectionKind::WireGuard) => ("WireGuard", "network-vpn-symbolic"),
        _ if vpn => ("VPN", "network-vpn-symbolic"),
        _ => ("Network", "network-transmit-receive-symbolic"),
    }
}

/// Sends notices from a thread of its own. Each one replaces the previous, so a quick
/// disconnect-connect doesn't stack up.
pub struct Notifier {
    sender: tokio::sync::mpsc::UnboundedSender<Notice>,
}

impl Notifier {
    pub fn spawn() -> Self {
        let (sender, mut notices) = tokio::sync::mpsc::unbounded_channel::<Notice>();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
            runtime.block_on(async move {
                let Ok(conn) = zbus::Connection::session().await else { return };
                let mut last = 0u32;
                while let Some(n) = notices.recv().await {
                    let mut hints: HashMap<&str, Value> = HashMap::new();
                    hints.insert("desktop-entry", Value::from(crate::APP_ID));
                    hints.insert("category", Value::from("network"));
                    let reply = conn
                        .call_method(
                            Some("org.freedesktop.Notifications"),
                            "/org/freedesktop/Notifications",
                            Some("org.freedesktop.Notifications"),
                            "Notify",
                            &(
                                "Vinculum",
                                last,
                                n.icon,
                                n.summary.as_str(),
                                n.body.as_str(),
                                Vec::<&str>::new(),
                                hints,
                                -1i32,
                            ),
                        )
                        .await;
                    if let Ok(id) = reply.and_then(|r| r.body().deserialize::<u32>()) {
                        last = id;
                    }
                }
            });
        });
        Self { sender }
    }

    pub fn send(&self, notice: Notice) {
        let _ = self.sender.send(notice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::home;

    #[test]
    fn up_and_down() {
        let connected = home();
        let mut offline = home();
        offline.active.clear();
        assert_eq!(
            changes(&offline, &connected),
            [Notice {
                summary: "Connected to Lumen Home".into(),
                body: "Wi-Fi".into(),
                icon: "network-wireless-symbolic"
            }]
        );
        assert_eq!(changes(&connected, &offline)[0].summary, "Disconnected from Lumen Home");
        assert!(changes(&connected, &connected).is_empty());
        // Still connecting is nothing yet.
        let mut activating = home();
        activating.active[0].state = ActiveState::Activating;
        assert!(changes(&offline, &activating).is_empty());
        assert_eq!(changes(&activating, &connected).len(), 1);
    }
}
