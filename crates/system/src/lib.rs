//! This computer, as far as a user can read it without root: /etc/os-release, /proc and /sys,
//! udev's database (memory modules) and pci.ids (names of graphics cards and adapters). Every
//! path is read below a root folder, `/` but for tests. A value that can't be read, or that is a
//! board maker's placeholder ("To Be Filled By O.E.M."), is left out rather than guessed.

use std::path::{Path, PathBuf};

/// The operating system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Os {
    /// "Arch Linux".
    pub name: String,
    /// "rolling", "13 (trixie)".
    pub version: String,
    /// The logo's icon name ("archlinux-logo"), and its file if one is installed.
    pub logo: String,
    pub logo_file: Option<PathBuf>,
}

/// The device: who made it and what it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub vendor: String,
    pub model: String,
    /// "desktop", "laptop", …: from DMI's chassis type.
    pub kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Firmware {
    pub vendor: String,
    pub version: String,
    /// As the firmware says it, "08/14/2025".
    pub date: String,
    pub uefi: bool,
    /// Known only with UEFI.
    pub secure_boot: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cpu {
    pub model: String,
    pub cores: usize,
    pub threads: usize,
    /// The highest clock, in MHz.
    pub max_mhz: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gpu {
    /// From pci.ids: the card's own name if it has one, else the chip's.
    pub name: String,
    /// "1002:744c".
    pub id: String,
    pub driver: String,
    /// The one the firmware showed the boot screen on.
    pub boot: bool,
    /// Its own memory, in bytes (amdgpu says).
    pub memory: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// What the system can use, in bytes.
    pub usable: u64,
    /// Installed, by udev's reading of the memory modules: bytes, type ("DDR5"), speed in MT/s.
    pub installed: u64,
    pub kind: String,
    pub speed: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Adapter {
    pub interface: String,
    pub name: String,
    pub driver: String,
    pub wireless: bool,
}

/// A drive and its file systems.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drive {
    /// "nvme0n1".
    pub device: String,
    pub model: String,
    pub size: u64,
    pub filesystems: Vec<Filesystem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filesystem {
    pub kind: String,
    /// Where it's mounted; a btrfs with subvolumes lists each.
    pub mounts: Vec<String>,
    pub size: u64,
    pub used: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Battery {
    pub name: String,
    /// Now, in %.
    pub charge: u32,
    /// How much it holds against when it was new, in %.
    pub health: Option<u32>,
    pub cycles: Option<u32>,
    pub status: String,
}

/// Values that Lituus reads but are placeholders, not names.
const PLACEHOLDERS: [&str; 9] = [
    "to be filled by o.e.m.",
    "system product name",
    "system manufacturer",
    "default string",
    "not applicable",
    "not specified",
    "o.e.m.",
    "none",
    "unknown",
];

fn known(value: &str) -> Option<String> {
    let v = value.trim();
    (!v.is_empty() && !PLACEHOLDERS.contains(&v.to_lowercase().as_str())).then(|| v.to_owned())
}

fn read(root: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(root.join(path.trim_start_matches('/'))).ok().map(|s| s.trim().to_owned())
}

/// A `KEY=value` file's value (os-release, uevent), quotes taken off.
fn key_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_owned())
}

pub fn os(root: &Path) -> Os {
    let text = read(root, "etc/os-release").or_else(|| read(root, "usr/lib/os-release")).unwrap_or_default();
    let name = key_value(&text, "NAME").unwrap_or_else(|| "Linux".into());
    let version = key_value(&text, "VERSION").or_else(|| key_value(&text, "BUILD_ID")).unwrap_or_default();
    let logo = key_value(&text, "LOGO").unwrap_or_default();
    let logo_file = (!logo.is_empty())
        .then(|| {
            ["svg", "png"].iter().map(|ext| root.join(format!("usr/share/pixmaps/{logo}.{ext}"))).find(|p| p.is_file())
        })
        .flatten();
    Os { name, version, logo, logo_file }
}

/// How long since the computer started, in seconds.
pub fn uptime(root: &Path) -> Option<u64> {
    read(root, "proc/uptime")?.split_whitespace().next()?.parse::<f64>().ok().map(|s| s as u64)
}

pub fn kernel(root: &Path) -> String {
    read(root, "proc/sys/kernel/osrelease").unwrap_or_default()
}

pub fn device(root: &Path) -> Device {
    let dmi = |f: &str| read(root, &format!("sys/class/dmi/id/{f}")).and_then(|v| known(&v));
    let mut vendor = dmi("sys_vendor");
    let mut model = dmi("product_name");
    // Lenovo names the model in the version ("ThinkPad P1 Gen 7"); the product is a code.
    if vendor.as_deref() == Some("LENOVO") {
        if let Some(v) = dmi("product_version") {
            model = Some(v);
        }
        vendor = Some("Lenovo".into());
    }
    // A home-built PC has placeholders there: its mainboard says more.
    if model.is_none() {
        vendor = dmi("board_vendor");
        model = dmi("board_name");
    }
    let kind = match read(root, "sys/class/dmi/id/chassis_type").and_then(|t| t.parse::<u32>().ok()) {
        Some(3 | 4 | 5 | 6 | 7 | 15 | 16 | 24) => "desktop",
        Some(8 | 9 | 10 | 14) => "laptop",
        Some(30..=32) => "tablet or convertible",
        Some(13) => "all-in-one",
        Some(17 | 23 | 28 | 29) => "server",
        _ => "",
    };
    Device { vendor: vendor.unwrap_or_default(), model: model.unwrap_or_default(), kind: kind.into() }
}

pub fn firmware(root: &Path) -> Firmware {
    let dmi = |f: &str| read(root, &format!("sys/class/dmi/id/{f}")).and_then(|v| known(&v)).unwrap_or_default();
    let uefi = root.join("sys/firmware/efi").is_dir();
    // The SecureBoot variable: four bytes of attributes, then 1 when it's on.
    let secure_boot = uefi
        .then(|| {
            std::fs::read(root.join("sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"))
                .ok()
                .and_then(|b| b.get(4).map(|v| *v == 1))
        })
        .flatten();
    Firmware { vendor: dmi("bios_vendor"), version: dmi("bios_version"), date: dmi("bios_date"), uefi, secure_boot }
}

pub fn cpu(root: &Path) -> Cpu {
    let info = read(root, "proc/cpuinfo").unwrap_or_default();
    let model = info
        .lines()
        .find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_owned()))
        .unwrap_or_default();
    // Threads are the CPUs Linux has; cores are their distinct (package, core) pairs: cpuinfo's
    // "cpu cores" is wrong in virtual machines.
    let mut threads = 0;
    let mut cores = std::collections::BTreeSet::new();
    let mut max_khz = 0u32;
    if let Ok(entries) = std::fs::read_dir(root.join("sys/devices/system/cpu")) {
        for e in entries.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.strip_prefix("cpu").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
                continue;
            }
            threads += 1;
            let topo = |f: &str| {
                std::fs::read_to_string(e.path().join("topology").join(f)).unwrap_or_default().trim().to_owned()
            };
            cores.insert((topo("physical_package_id"), topo("core_id")));
            let khz = std::fs::read_to_string(e.path().join("cpufreq/cpuinfo_max_freq"))
                .ok()
                .and_then(|f| f.trim().parse().ok());
            max_khz = max_khz.max(khz.unwrap_or(0));
        }
    }
    if threads == 0 {
        threads = info.lines().filter(|l| l.starts_with("processor")).count();
    }
    Cpu { model, cores: cores.len().max(usize::from(threads > 0)), threads, max_mhz: max_khz / 1000 }
}

/// Names from pci.ids: a vendor's, a device's, and a card's (subsystem) own.
struct PciIds(String);

impl PciIds {
    fn load(root: &Path) -> PciIds {
        PciIds(
            ["usr/share/hwdata/pci.ids", "usr/share/misc/pci.ids", "usr/share/pci.ids"]
                .iter()
                .find_map(|p| read(root, p))
                .unwrap_or_default(),
        )
    }

    /// (the device's name, the card's own name if pci.ids has it)
    fn names(&self, vendor: &str, device: &str, sub_vendor: &str, sub_device: &str) -> (String, String) {
        let mut in_vendor = false;
        let mut in_device = false;
        let (mut device_name, mut card) = (String::new(), String::new());
        for line in self.0.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if !line.starts_with('\t') {
                if in_vendor {
                    break;
                }
                in_vendor = line.starts_with(vendor);
            } else if in_vendor && !line.starts_with("\t\t") {
                if in_device {
                    break;
                }
                if let Some(rest) = line[1..].strip_prefix(device) {
                    in_device = true;
                    device_name = rest.trim().to_owned();
                }
            } else if in_device && line[2..].starts_with(&format!("{sub_vendor} {sub_device}")) {
                card = line[2 + sub_vendor.len() + 1 + sub_device.len()..].trim().to_owned();
            }
        }
        (device_name, card)
    }
}

fn pci_field(dir: &Path, f: &str) -> String {
    std::fs::read_to_string(dir.join(f)).unwrap_or_default().trim().trim_start_matches("0x").to_owned()
}

fn driver(dir: &Path) -> String {
    std::fs::read_link(dir.join("driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// The graphics cards: PCI display controllers (VGA, 3D, other).
pub fn gpus(root: &Path) -> Vec<Gpu> {
    let ids = PciIds::load(root);
    let Ok(entries) = std::fs::read_dir(root.join("sys/bus/pci/devices")) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter()
        .filter(|d| pci_field(d, "class").starts_with("03"))
        .map(|d| {
            let (v, dev) = (pci_field(&d, "vendor"), pci_field(&d, "device"));
            let (chip, card) =
                ids.names(&v, &dev, &pci_field(&d, "subsystem_vendor"), &pci_field(&d, "subsystem_device"));
            Gpu {
                name: if !card.is_empty() { card } else { chip },
                id: format!("{v}:{dev}"),
                driver: driver(&d),
                boot: pci_field(&d, "boot_vga") == "1",
                memory: pci_field(&d, "mem_info_vram_total").parse().unwrap_or(0),
            }
        })
        .collect()
}

pub fn memory(root: &Path) -> Memory {
    let usable = read(root, "proc/meminfo")
        .and_then(|t| key_value(&t.replace(':', "="), "MemTotal"))
        .and_then(|v| v.trim_end_matches("kB").trim().parse::<u64>().ok())
        .map_or(0, |kb| kb * 1024);
    // udev's dmi_memory_id: one MEMORY_DEVICE_<n>_… set per slot.
    let udev = read(root, "run/udev/data/+dmi:id").unwrap_or_default();
    let props: Vec<(&str, &str)> = udev.lines().filter_map(|l| l.strip_prefix("E:")?.split_once('=')).collect();
    let mut installed = 0;
    let mut kind = String::new();
    let mut speed = 0;
    for (k, v) in &props {
        let Some(rest) = k.strip_prefix("MEMORY_DEVICE_") else { continue };
        let field = rest.split_once('_').map_or("", |(_, f)| f);
        match field {
            "SIZE" => installed += v.parse::<u64>().unwrap_or(0),
            "TYPE" if kind.is_empty() => kind = known(v).filter(|t| t != "RAM" && t != "Other").unwrap_or_default(),
            "SPEED_MTS" | "CONFIGURED_SPEED_MTS" => speed = speed.max(v.parse().unwrap_or(0)),
            _ => {}
        }
    }
    Memory { usable, installed, kind, speed }
}

/// The physical network adapters (not loopback, bridges, VPNs).
pub fn adapters(root: &Path) -> Vec<Adapter> {
    let ids = PciIds::load(root);
    let Ok(entries) = std::fs::read_dir(root.join("sys/class/net")) else { return Vec::new() };
    let mut out: Vec<Adapter> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join("device").exists())
        .map(|e| {
            let dev = e.path().join("device");
            let (v, d) = (pci_field(&dev, "vendor"), pci_field(&dev, "device"));
            let name = if v.is_empty() { String::new() } else { ids.names(&v, &d, "", "").0 };
            Adapter {
                interface: e.file_name().to_string_lossy().into_owned(),
                name,
                driver: driver(&dev),
                wireless: e.path().join("wireless").exists() || e.path().join("phy80211").exists(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.interface.cmp(&b.interface));
    out
}

/// The drive a block device (a partition, or the drive itself) is on: "nvme0n1p2" → "nvme0n1". A
/// partition's folder in /sys is in its drive's and has a `partition` file.
fn drive_of(root: &Path, dev: &str) -> String {
    let dir = root.join("sys/class/block").join(dev);
    if !dir.join("partition").exists() {
        return dev.to_owned();
    }
    std::fs::canonicalize(&dir)
        .ok()
        .and_then(|p| p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| dev.to_owned())
}

/// The drives with their file systems, from `df`'s lines (source, type, size, used, mount), as
/// `df -B1 --output=source,fstype,size,used,target` prints them.
pub fn drives(root: &Path, df: &str) -> Vec<Drive> {
    let mut drives: Vec<Drive> = Vec::new();
    for line in df.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [source, kind, size, used, mount] = f[..] else { continue };
        let Some(dev) = source.strip_prefix("/dev/") else { continue };
        // /dev/mapper/… (LUKS, LVM): its own name.
        let dev = dev.rsplit('/').next().unwrap_or(dev);
        let drive = drive_of(root, dev);
        let entry = match drives.iter_mut().position(|d| d.device == drive) {
            Some(i) => &mut drives[i],
            None => {
                let block = format!("sys/class/block/{drive}");
                drives.push(Drive {
                    model: read(root, &format!("{block}/device/model")).and_then(|m| known(&m)).unwrap_or_default(),
                    size: read(root, &format!("{block}/size"))
                        .and_then(|s| s.parse::<u64>().ok())
                        .map_or(0, |s| s * 512),
                    device: drive.clone(),
                    filesystems: Vec::new(),
                });
                drives.last_mut().expect("just pushed")
            }
        };
        let (size, used) = (size.parse().unwrap_or(0), used.parse().unwrap_or(0));
        // One file system mounted in several places (btrfs subvolumes) is listed once.
        match entry.filesystems.iter_mut().find(|fs| fs.kind == kind && fs.size == size && fs.used == used) {
            Some(fs) => fs.mounts.push(mount.to_owned()),
            None => entry.filesystems.push(Filesystem { kind: kind.into(), mounts: vec![mount.into()], size, used }),
        }
    }
    drives
}

/// The file systems `drives` reads, from `df`.
pub fn df() -> String {
    std::process::Command::new("df")
        .args([
            "-B1",
            "--output=source,fstype,size,used,target",
            "-x",
            "tmpfs",
            "-x",
            "devtmpfs",
            "-x",
            "squashfs",
            "-x",
            "overlay",
            "-x",
            "efivarfs",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

pub fn batteries(root: &Path) -> Vec<Battery> {
    let Ok(entries) = std::fs::read_dir(root.join("sys/class/power_supply")) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        let field = |f: &str| std::fs::read_to_string(p.join(f)).map(|s| s.trim().to_owned()).ok();
        if field("type").as_deref() != Some("Battery") || field("scope").as_deref() == Some("Device") {
            continue;
        }
        let num = |f: &str| field(f).and_then(|v| v.parse::<u64>().ok());
        let full = num("energy_full").or_else(|| num("charge_full"));
        let design = num("energy_full_design").or_else(|| num("charge_full_design"));
        let health = match (full, design) {
            (Some(f), Some(d)) if d > 0 => Some(((f * 100) / d).min(100) as u32),
            _ => None,
        };
        let name = [field("manufacturer"), field("model_name")]
            .into_iter()
            .flatten()
            .filter_map(|v| known(&v))
            .collect::<Vec<_>>()
            .join(" ");
        out.push(Battery {
            name,
            charge: num("capacity").unwrap_or(0) as u32,
            health,
            cycles: num("cycle_count").filter(|c| *c > 0).map(|c| c as u32),
            status: field("status").unwrap_or_default(),
        });
    }
    out
}

/// A program's version, from its `--version` (the last word that starts with a digit).
pub fn version_of(command: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(command).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    text.split_whitespace()
        .map(|w| w.trim_start_matches('v'))
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lituus-system-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (path, text) in files {
            let p = d.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        d
    }

    #[test]
    fn a_home_built_desktop() {
        let r = root(
            "desktop",
            &[
                (
                    "etc/os-release",
                    "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nBUILD_ID=rolling\nLOGO=archlinux-logo\n",
                ),
                ("usr/share/pixmaps/archlinux-logo.svg", "<svg/>"),
                ("proc/uptime", "93784.21 512.00\n"),
                ("sys/class/dmi/id/sys_vendor", "To Be Filled By O.E.M.\n"),
                ("sys/class/dmi/id/product_name", "System Product Name\n"),
                ("sys/class/dmi/id/board_vendor", "ASUSTeK COMPUTER INC.\n"),
                ("sys/class/dmi/id/board_name", "ROG STRIX X670E-E GAMING WIFI\n"),
                ("sys/class/dmi/id/chassis_type", "3\n"),
                ("sys/class/dmi/id/bios_vendor", "American Megatrends Inc.\n"),
                ("sys/class/dmi/id/bios_version", "3205\n"),
                ("sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c", "\x06\0\0\0\0"),
                (
                    "proc/cpuinfo",
                    "processor\t: 0\nmodel name\t: AMD Ryzen 7 7800X3D 8-Core Processor\ncpu cores\t: 1\n",
                ),
                ("sys/devices/system/cpu/cpu0/topology/core_id", "0"),
                ("sys/devices/system/cpu/cpu0/topology/physical_package_id", "0"),
                ("sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq", "5050000"),
                ("sys/devices/system/cpu/cpu1/topology/core_id", "0"),
                ("sys/devices/system/cpu/cpu1/topology/physical_package_id", "0"),
                ("sys/devices/system/cpu/cpu2/topology/core_id", "1"),
                ("sys/devices/system/cpu/cpu2/topology/physical_package_id", "0"),
                ("proc/meminfo", "MemTotal:       65536000 kB\nMemFree: 1 kB\n"),
                (
                    "run/udev/data/+dmi:id",
                    "E:MEMORY_DEVICE_0_SIZE=34359738368\nE:MEMORY_DEVICE_0_TYPE=DDR5\nE:MEMORY_DEVICE_0_SPEED_MTS=6000\nE:MEMORY_DEVICE_1_SIZE=34359738368\n",
                ),
                (
                    "usr/share/hwdata/pci.ids",
                    "# comment\n1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]\n\t\t1043 0510  TUF Gaming Radeon RX 7900 XTX OC\n\t7480  Navi 33\n1043  ASUSTeK Computer Inc.\n",
                ),
                ("sys/bus/pci/devices/0000:03:00.0/class", "0x030000\n"),
                ("sys/bus/pci/devices/0000:03:00.0/vendor", "0x1002\n"),
                ("sys/bus/pci/devices/0000:03:00.0/device", "0x744c\n"),
                ("sys/bus/pci/devices/0000:03:00.0/subsystem_vendor", "0x1043\n"),
                ("sys/bus/pci/devices/0000:03:00.0/subsystem_device", "0x0510\n"),
                ("sys/bus/pci/devices/0000:03:00.0/boot_vga", "1\n"),
                ("sys/bus/pci/devices/0000:03:00.0/mem_info_vram_total", "25753026560\n"),
                ("sys/bus/pci/devices/0000:00:14.0/class", "0x0c0330\n"),
            ],
        );
        std::fs::create_dir_all(r.join("sys/firmware/efi")).unwrap();
        let os = os(&r);
        assert_eq!((os.name.as_str(), os.version.as_str()), ("Arch Linux", "rolling"));
        assert_eq!(os.logo_file, Some(r.join("usr/share/pixmaps/archlinux-logo.svg")));
        assert_eq!(uptime(&r), Some(93784));
        assert_eq!(
            device(&r),
            Device {
                vendor: "ASUSTeK COMPUTER INC.".into(),
                model: "ROG STRIX X670E-E GAMING WIFI".into(),
                kind: "desktop".into()
            }
        );
        let fw = firmware(&r);
        assert_eq!(
            (fw.vendor.as_str(), fw.uefi, fw.secure_boot, fw.date.as_str()),
            ("American Megatrends Inc.", true, Some(false), "")
        );
        assert_eq!(
            cpu(&r),
            Cpu { model: "AMD Ryzen 7 7800X3D 8-Core Processor".into(), cores: 2, threads: 3, max_mhz: 5050 }
        );
        let m = memory(&r);
        assert_eq!((m.usable, m.installed, m.kind.as_str(), m.speed), (65536000 * 1024, 68719476736, "DDR5", 6000));
        assert_eq!(
            gpus(&r),
            [Gpu {
                name: "TUF Gaming Radeon RX 7900 XTX OC".into(),
                id: "1002:744c".into(),
                driver: String::new(),
                boot: true,
                memory: 25753026560
            }]
        );
        std::fs::remove_dir_all(&r).unwrap();
    }

    #[test]
    fn drives_and_a_battery() {
        let r = root(
            "laptop",
            &[
                ("sys/class/power_supply/BAT0/type", "Battery\n"),
                ("sys/class/power_supply/BAT0/manufacturer", "SMP\n"),
                ("sys/class/power_supply/BAT0/model_name", "5B11M90061\n"),
                ("sys/class/power_supply/BAT0/capacity", "64\n"),
                ("sys/class/power_supply/BAT0/energy_full", "78300000\n"),
                ("sys/class/power_supply/BAT0/energy_full_design", "90000000\n"),
                ("sys/class/power_supply/BAT0/cycle_count", "0\n"),
                ("sys/class/power_supply/BAT0/status", "Discharging\n"),
                ("sys/class/power_supply/AC/type", "Mains\n"),
                ("sys/class/power_supply/hidpp_battery_0/type", "Battery\n"),
                ("sys/class/power_supply/hidpp_battery_0/scope", "Device\n"),
            ],
        );
        // As /sys has them: the drive's folder, its partitions in it, links in /sys/class/block.
        let disk = r.join("sys/devices/pci0000:00/nvme/nvme0/nvme0n1");
        for part in ["nvme0n1p1", "nvme0n1p2"] {
            std::fs::create_dir_all(disk.join(part)).unwrap();
            std::fs::write(disk.join(part).join("partition"), "1\n").unwrap();
        }
        std::fs::create_dir_all(disk.join("device")).unwrap();
        std::fs::write(disk.join("device/model"), "Samsung SSD 990 PRO 2TB\n").unwrap();
        std::fs::write(disk.join("size"), "3907029168\n").unwrap();
        std::fs::create_dir_all(r.join("sys/class/block")).unwrap();
        for (name, target) in [
            ("nvme0n1", "../../devices/pci0000:00/nvme/nvme0/nvme0n1"),
            ("nvme0n1p1", "../../devices/pci0000:00/nvme/nvme0/nvme0n1/nvme0n1p1"),
            ("nvme0n1p2", "../../devices/pci0000:00/nvme/nvme0/nvme0n1/nvme0n1p2"),
        ] {
            std::os::unix::fs::symlink(target, r.join("sys/class/block").join(name)).unwrap();
        }
        let df = "Filesystem Type 1B-blocks Used Mounted on\n\
                  /dev/nvme0n1p2 btrfs 1999000000000 420000000000 /\n\
                  /dev/nvme0n1p2 btrfs 1999000000000 420000000000 /home\n\
                  /dev/nvme0n1p1 vfat 1071624192 300000000 /boot\n\
                  tmpfs tmpfs 1 0 /tmp\n";
        let d = drives(&r, df);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(
            (d[0].device.as_str(), d[0].model.as_str(), d[0].size),
            ("nvme0n1", "Samsung SSD 990 PRO 2TB", 3907029168 * 512)
        );
        assert_eq!(d[0].filesystems[0].mounts, ["/", "/home"]);
        assert_eq!(d[0].filesystems.len(), 2);
        assert_eq!(
            batteries(&r),
            [Battery {
                name: "SMP 5B11M90061".into(),
                charge: 64,
                health: Some(87),
                cycles: None,
                status: "Discharging".into()
            }]
        );
        std::fs::remove_dir_all(&r).unwrap();
    }

    /// This machine, whatever it has: nothing panics, and it has a kernel and a processor.
    #[test]
    fn reads_this_machine() {
        let r = Path::new("/");
        assert!(!kernel(r).is_empty());
        assert!(cpu(r).threads > 0);
        let _ = (os(r), device(r), firmware(r), gpus(r), memory(r), adapters(r), drives(r, &df()), batteries(r));
    }
}
