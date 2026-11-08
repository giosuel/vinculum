//! The System page (System › System), following the "System" mockups: the computer's names and
//! what it is in the hero (renamed with the hostname dialog), its hardware, drives and battery, the
//! software Lituus's pages use, and the whole page as plain text for a bug report, without the
//! names or serial numbers. What it shows is read once a page visit, on a thread (lituus-system).

use std::path::{Path, PathBuf};

use lituus_system::{Battery, Cpu, Device, Drive, Firmware, Gpu, Memory, Os};
use slint::{Image, ModelRc, SharedString, VecModel};

use crate::{MainWindow, SystemDrive, SystemFact, SystemFs, SystemSoftware, SystemView, update};

/// What the page reads.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pub os: Os,
    pub kernel: String,
    pub uptime: Option<u64>,
    pub device: Device,
    pub firmware: Firmware,
    pub cpu: Cpu,
    pub gpus: Vec<Gpu>,
    pub memory: Memory,
    pub adapters: Vec<lituus_system::Adapter>,
    pub drives: Vec<Drive>,
    pub batteries: Vec<Battery>,
    /// The pretty name (/etc/machine-info), if one is set.
    pub pretty: String,
    /// Versions: Hyprland's (empty: not running), BlueZ's, PipeWire's, and the wallpaper program's.
    pub hyprland: String,
    pub bluez: String,
    pub pipewire: String,
    pub wallpaper: Option<(String, String)>,
    /// The session: "wayland", and the desktop's name when it isn't Hyprland.
    pub session: String,
    pub desktop: String,
    /// The monitors' models, from Hyprland.
    pub monitors: Vec<String>,
}

#[derive(Default)]
pub struct State {
    pub info: Option<Info>,
    /// "Copied" shows for a moment after Copy system info.
    pub copied: bool,
    /// The hostname and NetworkManager's version, from NetworkManager.
    pub hostname: String,
    pub nm_version: String,
}

fn read_root(root: &Path) -> Info {
    let wallpaper = {
        let env = lituus_wallpaper::Env::from_env();
        let s = lituus_wallpaper::status(&env);
        s.running.map(|p| (p.name().to_owned(), s.version))
    };
    let hypr = lituus_hypr::Hyprland::from_env().ok();
    let hyprland = hypr
        .as_ref()
        .and_then(|h| h.version().ok())
        .map(|v| v.version.trim_start_matches('v').to_owned())
        .unwrap_or_default();
    let monitors = hypr
        .and_then(|h| h.monitors().ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|m| !m.disabled)
        .map(|m| if m.model.is_empty() { m.name } else { m.model })
        .collect();
    let pretty = std::fs::read_to_string(root.join("etc/machine-info"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_HOSTNAME=").map(|v| v.trim().trim_matches('"').to_owned()))
        .unwrap_or_default();
    Info {
        os: lituus_system::os(root),
        kernel: lituus_system::kernel(root),
        uptime: lituus_system::uptime(root),
        device: lituus_system::device(root),
        firmware: lituus_system::firmware(root),
        cpu: lituus_system::cpu(root),
        gpus: lituus_system::gpus(root),
        memory: lituus_system::memory(root),
        adapters: lituus_system::adapters(root),
        drives: lituus_system::drives(root, &lituus_system::df()),
        batteries: lituus_system::batteries(root),
        pretty,
        hyprland,
        bluez: lituus_system::version_of("bluetoothctl", &["--version"]).unwrap_or_default(),
        pipewire: lituus_system::version_of("pipewire", &["--version"]).unwrap_or_default(),
        wallpaper,
        session: std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
        desktop: std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        monitors,
    }
}

/// Reads the computer, on a thread.
pub fn refresh(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    std::thread::spawn(move || {
        let info = read_root(Path::new("/"));
        let _ = weak.clone().upgrade_in_event_loop(move |_| update(&weak, move |app| app.system.info = Some(info)));
    });
}

// ---- in words ----------------------------------------------------------------------------------

/// "3 days, 4 hours", "2 hours, 5 minutes", "12 minutes".
fn uptime_text(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    let unit = |n: u64, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    match (days, hours) {
        (0, 0) => unit(minutes, "minute"),
        (0, h) => format!("{}, {}", unit(h, "hour"), unit(minutes, "minute")),
        (d, h) => format!("{}, {}", unit(d, "day"), unit(h, "hour")),
    }
}

/// Bytes as people say them: "64 GB", "1.59 TB", "312 MB" (powers of 1000, as drives are sold).
pub fn size_text(bytes: u64) -> String {
    let units = ["B", "kB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if v >= 100.0 || u == 0 {
        return format!("{v:.0} {}", units[u]);
    }
    // Two decimals, without the zeros at the end: "1.59", "1.5", "4".
    let text = format!("{v:.2}");
    format!("{} {}", text.trim_end_matches('0').trim_end_matches('.'), units[u])
}

/// Memory in GB as it's sold: powers of 1024, round.
fn memory_text(bytes: u64) -> String {
    let gib = bytes as f64 / (1u64 << 30) as f64;
    if gib >= 10.0 { format!("{gib:.0} GB") } else { format!("{gib:.1} GB").replace(".0 ", " ") }
}

/// "AMD Ryzen 7 7800X3D 8-Core Processor" → "AMD Ryzen 7 7800X3D": the trade name.
fn cpu_name(model: &str) -> String {
    // Up to the clock speed ("@ 3.00GHz"), without "8-Core Processor".
    let mut words: Vec<&str> = model.split_whitespace().take_while(|w| *w != "@").collect();
    words.retain(|w| !w.ends_with("-Core") && *w != "Processor" && *w != "CPU");
    words.join(" ").replace("(R)", "").replace("(TM)", "")
}

fn firmware_date(date: &str) -> String {
    // DMI's "MM/DD/YYYY" as "14 Aug 2025".
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let parts: Vec<&str> = date.split('/').collect();
    match parts[..] {
        [m, d, y] => match m.parse::<usize>() {
            Ok(m) if (1..=12).contains(&m) => format!("{} {} {y}", d.trim_start_matches('0'), MONTHS[m - 1]),
            _ => date.to_owned(),
        },
        _ => date.to_owned(),
    }
}

fn join(parts: impl IntoIterator<Item = String>) -> String {
    parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// "Arch Linux on an ASUS ROG STRIX X670E-E GAMING WIFI desktop".
fn device_line(info: &Info) -> String {
    let device = [info.device.vendor.as_str(), info.device.model.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let device = device.replace("ASUSTeK COMPUTER INC.", "ASUS");
    match (device.is_empty(), info.device.kind.is_empty()) {
        (true, _) => info.os.name.clone(),
        (false, true) => format!("{} on {device}", info.os.name),
        (false, false) => {
            let article = if device.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" };
            format!("{} on {article} {device} {}", info.os.name, info.device.kind)
        }
    }
}

/// The hardware: a fact per kind, and what couldn't be read.
fn facts(info: &Info) -> (Vec<SystemFact>, Vec<String>) {
    let mut facts = Vec::new();
    let mut unknown = Vec::new();
    let fact = |icon: &str, label: &str, title: String, detail: String| SystemFact {
        icon: icon.into(),
        label: label.into(),
        title: title.into(),
        detail: detail.into(),
        link: false,
    };
    let c = &info.cpu;
    if c.model.is_empty() {
        unknown.push("the processor's name".to_owned());
    } else {
        facts.push(fact(
            "chip",
            "Processor",
            cpu_name(&c.model),
            join([
                format!("{} cores", c.cores),
                format!("{} threads", c.threads),
                if c.max_mhz > 0 { format!("up to {:.2} GHz", f64::from(c.max_mhz) / 1000.0) } else { String::new() },
            ]),
        ));
    }
    match info.gpus.as_slice() {
        [] => unknown.push("the graphics".to_owned()),
        gpus => {
            let main = gpus.iter().find(|g| g.boot).unwrap_or(&gpus[0]);
            let title = if main.name.is_empty() { format!("Graphics [{}]", main.id) } else { main.name.clone() };
            let others: Vec<String> = gpus.iter().filter(|g| g.id != main.id).map(|g| g.name.clone()).collect();
            facts.push(fact(
                "monitor",
                "Graphics",
                title,
                join([
                    main.driver.clone(),
                    if main.memory > 0 { memory_text(main.memory) } else { String::new() },
                    if others.is_empty() { String::new() } else { format!("also {}", others.join(", ")) },
                ]),
            ));
        }
    }
    let m = &info.memory;
    if m.installed > 0 {
        facts.push(fact(
            "memory",
            "Memory",
            memory_text(m.installed),
            join([m.kind.clone(), if m.speed > 0 { format!("{} MT/s", m.speed) } else { String::new() }]),
        ));
    } else if m.usable > 0 {
        facts.push(fact("memory", "Memory", format!("{} usable", memory_text(m.usable)), String::new()));
    }
    if !info.adapters.is_empty() {
        let wireless = info.adapters.iter().any(|a| a.wireless);
        let wired = info.adapters.iter().any(|a| !a.wireless);
        let title = match (wireless, wired) {
            (true, true) => "Wi-Fi and Ethernet",
            (true, false) => "Wi-Fi",
            _ => "Ethernet",
        };
        let names = info.adapters.iter().map(|a| if a.name.is_empty() { a.interface.clone() } else { a.name.clone() });
        facts.push(fact("wifi", "Network adapters", title.into(), names.collect::<Vec<_>>().join(" · ")));
    }
    let f = &info.firmware;
    if f.vendor.is_empty() && f.version.is_empty() {
        unknown.push("the firmware".to_owned());
    } else {
        facts.push(fact(
            "settings",
            "Firmware",
            if f.version.is_empty() { f.vendor.clone() } else { format!("BIOS {}", f.version) },
            join([
                if f.version.is_empty() { String::new() } else { f.vendor.clone() },
                firmware_date(&f.date),
                if f.uefi {
                    match f.secure_boot {
                        Some(true) => "UEFI, Secure Boot on".to_owned(),
                        Some(false) => "UEFI, Secure Boot off".to_owned(),
                        None => "UEFI".to_owned(),
                    }
                } else {
                    "Legacy BIOS".to_owned()
                },
            ]),
        ));
    }
    if !info.monitors.is_empty() {
        let mut f = fact("monitor", "Displays", format!("{} connected", info.monitors.len()), info.monitors.join(", "));
        f.link = true;
        facts.push(f);
    }
    (facts, unknown)
}

/// The whole page as plain text, for an issue or a forum post: no names, no serial numbers.
pub fn report(state: &State, info: &Info) -> String {
    let mut lines = vec![format!("Lituus {} system info", env!("APP_VERSION"))];
    let mut add = |k: &str, v: String| {
        if !v.is_empty() {
            lines.push(format!("{:<10} {v}", format!("{k}:")));
        }
    };
    add(
        "OS",
        join([format!("{} {}", info.os.name, info.os.version).trim().to_owned(), format!("kernel {}", info.kernel)])
            .replace(" · ", ", "),
    );
    let desktop = if info.hyprland.is_empty() { info.desktop.clone() } else { format!("Hyprland {}", info.hyprland) };
    add("Desktop", join([desktop, capital(&info.session)]).replace(" · ", ", "));
    add(
        "Device",
        format!("{} {}", info.device.vendor, info.device.model).trim().to_owned() + &paren(&info.device.kind),
    );
    let f = &info.firmware;
    add(
        "Firmware",
        join([f.vendor.clone(), f.version.clone(), f.date.clone(), if f.uefi { "UEFI".into() } else { "BIOS".into() }])
            .replace(" · ", ", "),
    );
    add(
        "CPU",
        join([cpu_name(&info.cpu.model), format!("{} cores, {} threads", info.cpu.cores, info.cpu.threads)])
            .replace(" · ", ", "),
    );
    add(
        "GPU",
        info.gpus
            .iter()
            .map(|g| {
                format!(
                    "{} [{}]{}",
                    g.name,
                    g.id,
                    if g.driver.is_empty() { String::new() } else { format!(", {}", g.driver) }
                )
            })
            .collect::<Vec<_>>()
            .join("; "),
    );
    let m = &info.memory;
    add(
        "Memory",
        join([memory_text(if m.installed > 0 { m.installed } else { m.usable }), m.kind.clone()]).replace(" · ", " "),
    );
    add(
        "Network",
        info.adapters
            .iter()
            .map(|a| format!("{} ({})", if a.name.is_empty() { &a.interface } else { &a.name }, a.driver))
            .collect::<Vec<_>>()
            .join(", "),
    );
    if !info.monitors.is_empty() {
        add("Displays", format!("{} ({})", info.monitors.len(), info.monitors.join(", ")));
    }
    add(
        "Storage",
        info.drives
            .iter()
            .map(|d| {
                let fs = d
                    .filesystems
                    .iter()
                    .map(|f| format!("{} {} {}%", f.kind, f.mounts.join(", "), percent(f.used, f.size)))
                    .collect::<Vec<_>>();
                format!("{} ({})", if d.model.is_empty() { &d.device } else { &d.model }, fs.join("; "))
            })
            .collect::<Vec<_>>()
            .join(", "),
    );
    let mut stack = vec![
        format!("NetworkManager {}", state.nm_version),
        format!("BlueZ {}", info.bluez),
        format!("PipeWire {}", info.pipewire),
    ];
    if let Some((name, v)) = &info.wallpaper {
        stack.push(format!("{name} {v}"));
    }
    add("Stack", stack.into_iter().filter(|s| !s.ends_with(' ')).collect::<Vec<_>>().join(", "));
    lines.join("\n")
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn paren(s: &str) -> String {
    if s.is_empty() { String::new() } else { format!(" ({s})") }
}

fn percent(used: u64, size: u64) -> u64 {
    (used * 100).checked_div(size).unwrap_or(0)
}

// ---- the page ----------------------------------------------------------------------------------

fn s(text: impl AsRef<str>) -> SharedString {
    SharedString::from(text.as_ref())
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

pub fn status(state: &State) -> String {
    state.hostname.clone()
}

/// The OS's logo, as an image (os-release names it; the file is in /usr/share/pixmaps).
fn logo(path: Option<&PathBuf>) -> Image {
    path.and_then(|p| Image::load_from_path(p).ok()).unwrap_or_default()
}

pub fn view(state: &State) -> SystemView {
    let mut v = SystemView::default();
    let Some(info) = &state.info else {
        v.state = s("loading");
        return v;
    };
    v.state = s("list");
    v.sub = s(info.uptime.map(|u| format!("Up {}", uptime_text(u))).unwrap_or_default());
    v.name = s(if info.pretty.is_empty() { &state.hostname } else { &info.pretty });
    v.hostname = s(&state.hostname);
    v.pretty = !info.pretty.is_empty();
    v.line = s(device_line(info));
    v.logo = logo(info.os.logo_file.as_ref());
    v.monogram = s(info.os.name.chars().next().map(|c| c.to_string()).unwrap_or_default());
    let (facts, unknown) = facts(info);
    v.facts = model(facts);
    v.unknown =
        s(if unknown.is_empty() { String::new() } else { format!("Lituus couldn't read {}.", unknown.join(" or ")) });
    v.drives = model(
        info.drives
            .iter()
            .map(|d| SystemDrive {
                name: s(if d.model.is_empty() { &d.device } else { &d.model }),
                meta: s(join([
                    if d.device.starts_with("nvme") { "NVMe".to_owned() } else { String::new() },
                    if d.size > 0 { size_text(d.size) } else { String::new() },
                ])),
                filesystems: model(
                    d.filesystems
                        .iter()
                        .map(|f| SystemFs {
                            mounts: s(f.mounts.join(" and ")),
                            kind: s(&f.kind),
                            used: if f.size == 0 { 0.0 } else { f.used as f32 / f.size as f32 },
                            text: s(format!(
                                "{} used · {} free",
                                size_text(f.used),
                                size_text(f.size.saturating_sub(f.used))
                            )),
                            warn: percent(f.used, f.size) >= 90,
                        })
                        .collect(),
                ),
            })
            .collect(),
    );
    v.drive_count = s(match info.drives.len() {
        1 => "1 drive".to_owned(),
        n => format!("{n} drives"),
    });
    let row =
        |name: &str, version: &str, note: &str| SystemSoftware { name: s(name), version: s(version), note: s(note) };
    let desktop = if info.hyprland.is_empty() {
        row(
            "Desktop",
            if info.desktop.is_empty() { "Unknown" } else { &info.desktop },
            &capital(&format!("{} session", info.session)),
        )
    } else {
        row("Desktop", &format!("Hyprland {}", info.hyprland), &capital(&format!("{} session", info.session)))
    };
    v.software = model(vec![row("Kernel", &info.kernel, ""), desktop, row("Lituus", env!("APP_VERSION"), "")]);
    let or = |v: &str, missing: &str| if v.is_empty() { missing.to_owned() } else { v.to_owned() };
    let mut uses = vec![
        row("NetworkManager", &or(&state.nm_version, "Not running"), "Network"),
        row("BlueZ", &or(&info.bluez, "Not installed"), "Bluetooth"),
        row("PipeWire", &or(&info.pipewire, "Not installed"), "Sound"),
    ];
    if let Some((name, version)) = &info.wallpaper {
        uses.push(row(name, version, "Wallpaper"));
    }
    v.uses = model(uses);
    if let Some(b) = info.batteries.first() {
        v.battery = true;
        v.battery_title = s(format!(
            "{} %{}",
            b.charge,
            if b.status.is_empty() { String::new() } else { format!(" · {}", b.status.to_lowercase()) }
        ));
        v.battery_detail = s(join([
            b.health.map(|h| format!("{h} % of its new capacity")).unwrap_or_default(),
            b.cycles.map(|c| format!("{c} charge cycles")).unwrap_or_default(),
            b.name.clone(),
        ]));
    }
    v.report = s(report(state, info));
    v.copied = state.copied;
    v
}

pub fn render(window: &MainWindow, state: &State) {
    if window.get_page() == "system" {
        window.set_system(view(state));
    }
}

/// Copy system info: "Copied" for two seconds.
pub fn copied(weak: &slint::Weak<MainWindow>) {
    let weak = weak.clone();
    update(&weak, |app| app.system.copied = true);
    slint::Timer::single_shot(std::time::Duration::from_secs(2), move || {
        update(&weak, |app| app.system.copied = false)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> Info {
        Info {
            os: Os { name: "Arch Linux".into(), version: "rolling".into(), ..Default::default() },
            kernel: "7.0.4-arch1-1".into(),
            uptime: Some(3 * 86400 + 4 * 3600 + 120),
            device: Device {
                vendor: "ASUSTeK COMPUTER INC.".into(),
                model: "ROG STRIX X670E-E GAMING WIFI".into(),
                kind: "desktop".into(),
            },
            firmware: Firmware {
                vendor: "American Megatrends".into(),
                version: "3205".into(),
                date: "08/14/2025".into(),
                uefi: true,
                secure_boot: Some(false),
            },
            cpu: Cpu { model: "AMD Ryzen 7 7800X3D 8-Core Processor".into(), cores: 8, threads: 16, max_mhz: 5050 },
            gpus: vec![Gpu {
                name: "TUF Gaming Radeon RX 7900 XTX OC".into(),
                id: "1002:744c".into(),
                driver: "amdgpu".into(),
                boot: true,
                memory: 24 << 30,
            }],
            memory: Memory { usable: 62 << 30, installed: 64 << 30, kind: "DDR5".into(), speed: 6000 },
            drives: vec![Drive {
                device: "nvme0n1".into(),
                model: "Samsung SSD 990 PRO 2TB".into(),
                size: 2_000_398_934_016,
                filesystems: vec![lituus_system::Filesystem {
                    kind: "btrfs".into(),
                    mounts: vec!["/".into(), "/home".into()],
                    size: 2_000_000_000_000,
                    used: 412_000_000_000,
                }],
            }],
            pretty: "Giosuel's desk".into(),
            hyprland: "0.57.0".into(),
            bluez: "5.84".into(),
            pipewire: "1.4.9".into(),
            wallpaper: Some(("awww".into(), "0.12.1".into())),
            session: "wayland".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_page_in_words() {
        use slint::Model;
        let state = State {
            info: Some(Info { monitors: vec!["Odyssey G61SH".into()], ..info() }),
            hostname: "orbis".into(),
            nm_version: "1.54.2".into(),
            ..Default::default()
        };
        let v = view(&state);
        assert_eq!(
            (v.sub.as_str(), v.name.as_str(), v.hostname.as_str()),
            ("Up 3 days, 4 hours", "Giosuel's desk", "orbis")
        );
        assert_eq!(v.line, "Arch Linux on an ASUS ROG STRIX X670E-E GAMING WIFI desktop");
        let facts: Vec<(String, String, String)> =
            v.facts.iter().map(|f| (f.label.to_string(), f.title.to_string(), f.detail.to_string())).collect();
        assert_eq!(
            facts[0],
            ("Processor".into(), "AMD Ryzen 7 7800X3D".into(), "8 cores · 16 threads · up to 5.05 GHz".into())
        );
        assert_eq!(facts[1], ("Graphics".into(), "TUF Gaming Radeon RX 7900 XTX OC".into(), "amdgpu · 24 GB".into()));
        assert_eq!(facts[2], ("Memory".into(), "64 GB".into(), "DDR5 · 6000 MT/s".into()));
        assert_eq!(
            facts[3],
            ("Firmware".into(), "BIOS 3205".into(), "American Megatrends · 14 Aug 2025 · UEFI, Secure Boot off".into())
        );
        assert!(v.facts.row_data(4).unwrap().link);
        let fs = v.drives.row_data(0).unwrap().filesystems.row_data(0).unwrap();
        assert_eq!((fs.mounts.as_str(), fs.text.as_str()), ("/ and /home", "412 GB used · 1.59 TB free"));
        assert_eq!(v.uses.row_data(3).unwrap().name, "awww");
        // The report: no names.
        assert!(!v.report.contains("orbis") && !v.report.contains("Giosuel"), "{}", v.report);
        assert!(v.report.contains("CPU:       AMD Ryzen 7 7800X3D, 8 cores, 16 threads"), "{}", v.report);
    }

    #[test]
    fn words() {
        assert_eq!(uptime_text(125), "2 minutes");
        assert_eq!(uptime_text(3600 + 60), "1 hour, 1 minute");
        assert_eq!(size_text(312_000_000), "312 MB");
        assert_eq!(size_text(1_590_000_000_000), "1.59 TB");
        assert_eq!(size_text(4_000_000_000_000), "4 TB");
        assert_eq!(size_text(10_000_000_000), "10 GB");
        assert_eq!(size_text(20_500_000_000), "20.5 GB");
        assert_eq!(memory_text(64 << 30), "64 GB");
        assert_eq!(cpu_name("Intel(R) Core(TM) i7-1185G7 @ 3.00GHz"), "Intel Core i7-1185G7");
        assert_eq!(firmware_date("04/01/2014"), "1 Apr 2014");
    }
}
