//! `vinculum-cli`: prints what Vinculum reads from NetworkManager, to check the `vinculum-nm`
//! crate against a real system. Not shipped in the packages.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use vinculum_nm::{Client, ConnectionKind, Device, DeviceKind, Snapshot, bars, channel};

#[derive(Parser)]
#[command(name = "vinculum-cli", version = env!("CARGO_PKG_VERSION"), about = "What Vinculum sees from NetworkManager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Devices, Wi-Fi networks, saved and active connections.
    List,
    /// The same, again after every change NetworkManager reports.
    Watch,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vinculum-cli: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> Result<(), vinculum_nm::Error> {
    let client = Client::system().await?;
    match command {
        Command::List => print!("{}", render(&client.snapshot().await?)),
        Command::Watch => {
            let mut snapshots = Box::pin(client.watch().await?);
            while let Some(snapshot) = snapshots.next().await {
                match snapshot {
                    Ok(snapshot) => print!("\x1b[2J\x1b[H{}", render(&snapshot)),
                    Err(error) => println!("\x1b[2J\x1b[Hvinculum-cli: {error}"),
                }
            }
        }
    }
    Ok(())
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn render(s: &Snapshot) -> String {
    let mut out = String::new();
    let wifi = match (s.wireless_enabled, s.wireless_hardware_enabled) {
        (_, false) => "blocked",
        (on, true) => on_off(on),
    };
    out += &format!("NetworkManager {} · networking {} · Wi-Fi {wifi}\n", s.version, on_off(s.networking_enabled));

    out += "\nDevices\n";
    for d in &s.devices {
        out += &format!(
            "  {:<14} {:<10} {:<13} {:<18} {}\n",
            d.interface,
            kind(d),
            format!("{:?}", d.state).to_lowercase(),
            d.hw_address,
            addresses(d)
        );
    }

    for (path, networks) in s.networks() {
        let interface = s.device(&path).map_or("?", |d| d.interface.as_str());
        out += &format!("\nWi-Fi networks ({interface})\n");
        for n in networks {
            let ap = n.strongest();
            let bands: Vec<&str> = n.bands.iter().map(|b| b.label()).collect();
            let channel = channel(ap.frequency).map_or(String::new(), |c| format!("ch {c}"));
            let marks = [n.active.then_some("connected"), n.saved.as_ref().map(|_| "saved")];
            out += &format!(
                "  {} {:<24} {:<16} {:>4} % {:<16} {:<6} {}\n",
                signal(ap.strength),
                n.name(),
                n.security.label(),
                ap.strength,
                bands.join(", "),
                channel,
                marks.into_iter().flatten().collect::<Vec<_>>().join(", "),
            );
        }
    }

    out += "\nSaved connections\n";
    for c in &s.connections {
        let kind = match &c.kind {
            ConnectionKind::Wifi { .. } => "wifi".to_owned(),
            ConnectionKind::Ethernet => "ethernet".to_owned(),
            ConnectionKind::WireGuard => "wireguard".to_owned(),
            ConnectionKind::Vpn { service } => format!("vpn ({})", service.rsplit('.').next().unwrap_or(service)),
            ConnectionKind::Other(t) => t.clone(),
        };
        let auto = if c.autoconnect { format!("autoconnect {}", c.autoconnect_priority) } else { "manual".into() };
        out += &format!("  {:<24} {:<16} {:<16} {}\n", c.id, kind, auto, c.uuid);
    }

    out += "\nActive connections\n";
    for a in &s.active {
        let devices: Vec<&str> = a.devices.iter().filter_map(|p| s.device(p)).map(|d| d.interface.as_str()).collect();
        let primary = if s.primary_connection.as_deref() == Some(a.path.as_str()) { " · default route" } else { "" };
        out += &format!(
            "  {:<24} {:<12} {}{primary}\n",
            a.id,
            format!("{:?}", a.state).to_lowercase(),
            devices.join(", ")
        );
    }
    out
}

fn kind(d: &Device) -> String {
    match d.kind {
        DeviceKind::Other(n) => format!("type {n}"),
        k => format!("{k:?}").to_lowercase(),
    }
}

fn addresses(d: &Device) -> String {
    let mut parts = Vec::new();
    for ip in [&d.ip4, &d.ip6].into_iter().flatten() {
        parts.extend(ip.addresses.iter().map(|(a, p)| format!("{a}/{p}")));
    }
    if d.wired.as_ref().is_some_and(|w| !w.carrier) {
        parts.push("no cable".into());
    }
    parts.join(" ")
}

fn signal(strength: u8) -> String {
    let n = usize::from(bars(strength));
    let full = ['▂', '▄', '▆', '█'];
    full.iter().enumerate().map(|(i, c)| if i < n { *c } else { '·' }).collect()
}
