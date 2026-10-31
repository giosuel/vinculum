//! The tray icon: a StatusNotifierItem (what Waybar's `tray` module, KDE and most bars show),
//! with the menu as DBusMenu items the bar draws itself. Left-click opens the quick popup.
//!
//! It runs on a thread of its own; the window's event loop sends it what to show, and gets back
//! what was picked.

use std::sync::Arc;

use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};

use crate::quick::{NETWORKS, Quick};

/// What the user did in the tray.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Left-click on the icon.
    Activate,
    /// A network, by its row ID: connect to it (nothing, if it's the connected one).
    Connect(String),
    /// A VPN, by its row ID: connect, or disconnect a connected one.
    Toggle(String),
    SetWifi(bool),
    SetAirplane(bool),
    Open,
    Settings,
    Quit,
}

type Sink = Arc<dyn Fn(Event) + Send + Sync>;

struct Tray {
    quick: Quick,
    sink: Sink,
    pixmap: Vec<ksni::Icon>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        crate::APP_ID.into()
    }

    fn title(&self) -> String {
        "Lituus".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn icon_name(&self) -> String {
        self.quick.icon.name().into()
    }

    /// For icon themes without the network icons: the logo's tray outline.
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.pixmap.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.quick.status.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.sink)(Event::Activate);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        menu(&self.quick)
    }
}

fn item(label: &str, event: Event) -> MenuItem<Tray> {
    StandardItem {
        label: label.into(),
        activate: Box::new(move |t: &mut Tray| (t.sink)(event.clone())),
        ..Default::default()
    }
    .into()
}

fn check(label: &str, checked: bool, enabled: bool, event: Event) -> MenuItem<Tray> {
    CheckmarkItem {
        label: label.into(),
        checked,
        enabled,
        activate: Box::new(move |t: &mut Tray| (t.sink)(event.clone())),
        ..Default::default()
    }
    .into()
}

fn info(label: &str) -> MenuItem<Tray> {
    StandardItem { label: label.into(), enabled: false, ..Default::default() }.into()
}

/// The menu as the mockup has it: status, Wi-Fi switch, networks (connected one selected), VPNs,
/// airplane mode, then the window.
fn menu(q: &Quick) -> Vec<MenuItem<Tray>> {
    let mut items = vec![info(&q.status), check("Wi-Fi", q.wifi_on, q.wifi_available, Event::SetWifi(!q.wifi_on))];
    if !q.networks.is_empty() {
        items.push(MenuItem::Separator);
        let (first, rest) = q.networks.split_at(q.networks.len().min(NETWORKS - 2));
        items.push(networks(first));
        if !rest.is_empty() {
            items.push(
                SubMenu { label: "More networks".into(), submenu: vec![networks(rest)], ..Default::default() }.into(),
            );
        }
    }
    if !q.vpns.is_empty() {
        items.push(MenuItem::Separator);
        for v in &q.vpns {
            items.push(check(&v.name, v.on, true, Event::Toggle(v.id.clone())));
        }
    }
    items.push(MenuItem::Separator);
    items.push(check("Airplane mode", q.airplane, true, Event::SetAirplane(!q.airplane)));
    items.push(MenuItem::Separator);
    items.push(item("Open Lituus", Event::Open));
    items.push(item("Settings", Event::Settings));
    items.push(item("Quit", Event::Quit));
    items
}

/// Networks as radio items, the connected one selected.
fn networks(list: &[crate::quick::QuickNetwork]) -> MenuItem<Tray> {
    let ids: Vec<String> = list.iter().map(|n| n.id.clone()).collect();
    RadioGroup {
        selected: list.iter().position(|n| n.active).unwrap_or(usize::MAX),
        select: Box::new(move |t: &mut Tray, i| {
            if let Some(id) = ids.get(i) {
                (t.sink)(Event::Connect(id.clone()));
            }
        }),
        options: list
            .iter()
            .map(|n| RadioItem {
                label: format!("{}  {}", n.name, "▂▄▆█".chars().take(n.bars as usize).collect::<String>()),
                ..Default::default()
            })
            .collect(),
    }
    .into()
}

/// The tray's handle: [`Handle::show`] updates it, and dropping it removes the icon.
pub struct Handle {
    updates: std::sync::mpsc::Sender<Quick>,
}

impl Handle {
    pub fn show(&self, quick: Quick) {
        let _ = self.updates.send(quick);
    }
}

/// Starts the tray on its own thread. `None` when no tray host (StatusNotifierWatcher) runs:
/// then there's nothing to close the window to.
pub fn spawn(quick: Quick, sink: impl Fn(Event) + Send + Sync + 'static) -> Option<Handle> {
    let (started, ready) = std::sync::mpsc::channel();
    let (updates, received) = std::sync::mpsc::channel::<Quick>();
    std::thread::spawn(move || {
        let tray = Tray { quick, sink: Arc::new(sink), pixmap: pixmap() };
        // Registered by its unique bus name rather than org.kde.StatusNotifierItem-<pid>-<n>:
        // a Flatpak sandbox doesn't let it own that name, and hosts take either.
        let handle = match tray.disable_dbus_name(true).spawn() {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("lituus: no tray icon: {error}");
                let _ = started.send(false);
                return;
            }
        };
        let _ = started.send(true);
        while let Ok(mut quick) = received.recv() {
            // Only the latest matters.
            while let Ok(newer) = received.try_recv() {
                quick = newer;
            }
            if handle.update(|t| t.quick = quick).is_none() {
                return;
            }
        }
        // The handle was dropped: the tray was turned off.
        handle.shutdown().wait();
    });
    ready.recv_timeout(std::time::Duration::from_secs(5)).unwrap_or(false).then_some(Handle { updates })
}

/// The hexagon outline with its hub, in the accent colour, at the sizes bars ask for.
fn pixmap() -> Vec<ksni::Icon> {
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><path d="M32 6 L55 19 V45 L32 58 L9 45 V19 Z M9 19 L32 32 L55 19 M32 32 V58" fill="none" stroke="#5cc8b0" stroke-width="5" stroke-linejoin="round"/><circle cx="32" cy="32" r="5" fill="#5cc8b0"/></svg>"##;
    let Ok(tree) = resvg::usvg::Tree::from_str(SVG, &resvg::usvg::Options::default()) else { return Vec::new() };
    [22u32, 32, 48]
        .into_iter()
        .filter_map(|size| {
            let mut canvas = resvg::tiny_skia::Pixmap::new(size, size)?;
            let scale = size as f32 / 64.0;
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut canvas.as_mut());
            // tiny-skia is premultiplied RGBA; SNI wants ARGB32 in network byte order.
            let data = canvas
                .pixels()
                .iter()
                .flat_map(|p| {
                    let c = p.demultiply();
                    [c.alpha(), c.red(), c.green(), c.blue()]
                })
                .collect();
            Some(ksni::Icon { width: size as i32, height: size as i32, data })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::home;

    fn labels(items: &[MenuItem<Tray>]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                MenuItem::Standard(s) => s.label.clone(),
                MenuItem::Checkmark(c) => format!("[{}] {}", if c.checked { "x" } else { " " }, c.label),
                MenuItem::RadioGroup(g) => g
                    .options
                    .iter()
                    .enumerate()
                    .map(|(n, o)| format!("({}) {}", if n == g.selected { "*" } else { " " }, o.label))
                    .collect::<Vec<_>>()
                    .join(" | "),
                MenuItem::SubMenu(s) => format!("{} >", s.label),
                MenuItem::Separator => "--".into(),
            })
            .collect()
    }

    #[test]
    fn menu_follows_the_mockup() {
        let q = crate::quick::build(Some(&home()));
        assert_eq!(
            labels(&menu(&q)),
            [
                "Wi-Fi: Lumen Home",
                "[x] Wi-Fi",
                "--",
                "(*) Lumen Home  ▂▄▆█ | ( ) Aster Café  ▂▄▆ | ( ) Bibliothek-Free  ▂▄",
                "--",
                "[ ] Airplane mode",
                "--",
                "Open Lituus",
                "Settings",
                "Quit",
            ]
        );
    }

    #[test]
    fn the_logo_renders_as_argb() {
        let icons = pixmap();
        assert_eq!(icons.iter().map(|i| i.width).collect::<Vec<_>>(), [22, 32, 48]);
        let i = &icons[2];
        assert_eq!(i.data.len(), 48 * 48 * 4);
        // The hub in the middle is opaque accent: A, R, G, B.
        let at = (24 * 48 + 24) * 4;
        assert_eq!(&i.data[at..at + 4], &[255, 0x5c, 0xc8, 0xb0]);
    }
}
