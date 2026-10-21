//! Against a mock NetworkManager (python-dbusmock's template) on a private bus, so the real
//! system bus is never touched.
//!
//! Needs dbus-daemon and python-dbusmock, so it runs only with VINCULUM_DBUSMOCK=1
//! (scripts/test.sh and CI set it). VINCULUM_PYTHON picks the interpreter that has dbusmock.

use std::process::Stdio;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use vinculum_nm::{ActiveState, Band, Client, ConnectionKind, DeviceKind, DeviceState, Error, Security};
use zbus::Connection;

const MOCK: &str = "org.freedesktop.DBus.Mock";
const MANAGER: &str = "/org/freedesktop/NetworkManager";

// NM80211ApSecurityFlags as the mock takes them.
const SEC_NONE: u32 = 0;
// The mock sets both WpaFlags and RsnFlags to this, so it reads as WPA2; it only knows bare PSK.
const SEC_WPA2_PSK: u32 = 0x100;

/// A private bus, optionally with the mock NetworkManager on it. Stopped when dropped.
struct Bus {
    address: String,
    daemon: Child,
    mock: Option<Child>,
}

impl Bus {
    async fn start(with_nm: bool) -> Option<Self> {
        if std::env::var("VINCULUM_DBUSMOCK").as_deref() != Ok("1") {
            eprintln!("skipped: set VINCULUM_DBUSMOCK=1 (scripts/test.sh does) to run against the mock NetworkManager");
            return None;
        }
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("dbus-daemon runs");
        let mut line = String::new();
        BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut line).await.unwrap();
        let address = line.trim().to_owned();
        let mut bus = Self { address, daemon, mock: None };
        if with_nm {
            let python = std::env::var("VINCULUM_PYTHON").unwrap_or_else(|_| "python3".into());
            bus.mock = Some(
                Command::new(python)
                    .args(["-m", "dbusmock", "--session", "--template", "networkmanager"])
                    .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
                    .stdout(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .expect("python-dbusmock runs"),
            );
            bus.wait_for_nm().await;
        }
        Some(bus)
    }

    async fn connect(&self) -> Connection {
        zbus::connection::Builder::address(self.address.as_str()).unwrap().build().await.unwrap()
    }

    async fn wait_for_nm(&self) {
        let conn = self.connect().await;
        let dbus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
        for _ in 0..100 {
            if dbus.name_has_owner(vinculum_nm::BUS_NAME.try_into().unwrap()).await.unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the mock NetworkManager didn't start");
    }

    /// Calls one of the mock's set-up methods on the manager object.
    async fn mock<B, R>(&self, method: &str, body: &B) -> R
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let conn = self.connect().await;
        let reply = conn.call_method(Some(vinculum_nm::BUS_NAME), MANAGER, Some(MOCK), method, body).await.unwrap();
        reply.body().deserialize().unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        if let Some(mock) = &mut self.mock {
            let _ = mock.start_kill();
        }
        let _ = self.daemon.start_kill();
    }
}

/// A laptop at home: Wi-Fi connected to "Lumen Home" (two access points), a café network and an
/// open one in range, and an unplugged Ethernet port.
async fn home_setup(bus: &Bus) -> (String, String) {
    let wifi: String = bus.mock("AddWiFiDevice", &("wlan0", "wlp2s0", 100u32 as i32)).await;
    let home_5: String = bus
        .mock(
            "AddAccessPoint",
            &(&wifi, "home5", "Lumen Home", "3C:A6:2F:11:8E:20", 2u32, 5180u32, 866_000u32, 78u8, SEC_WPA2_PSK),
        )
        .await;
    let _: String = bus
        .mock(
            "AddAccessPoint",
            &(&wifi, "home24", "Lumen Home", "3C:A6:2F:11:8E:21", 2u32, 2437u32, 144_000u32, 64u8, SEC_WPA2_PSK),
        )
        .await;
    let _: String = bus
        .mock(
            "AddAccessPoint",
            &(&wifi, "cafe", "Aster Café", "08:96:AD:3E:71:C2", 2u32, 2412u32, 54_000u32, 61u8, SEC_WPA2_PSK),
        )
        .await;
    let _: String = bus
        .mock(
            "AddAccessPoint",
            &(&wifi, "lib", "Bibliothek-Free", "0A:11:22:33:44:55", 2u32, 5200u32, 300_000u32, 40u8, SEC_NONE),
        )
        .await;
    let conn: String = bus.mock("AddWiFiConnection", &(&wifi, "home", "Lumen Home", "wpa-psk")).await;
    let _: String = bus.mock("AddActiveConnection", &(vec![&wifi], &conn, &home_5, "home", 2u32)).await;
    let _: String = bus.mock("AddEthernetDevice", &("eth0", "enp3s0", 20u32 as i32)).await;
    (wifi, home_5)
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_devices_networks_and_connections() {
    let Some(bus) = Bus::start(true).await else { return };
    let (wifi_path, home_5) = home_setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let snapshot = client.snapshot().await.expect("a snapshot");

    assert!(snapshot.networking_enabled && snapshot.wireless_enabled);
    assert_eq!(snapshot.devices.len(), 2);
    assert_eq!(snapshot.hostname, "hostname"); // the mock's

    let wifi = snapshot.device(&wifi_path).expect("the Wi-Fi device");
    assert_eq!(wifi.interface, "wlp2s0");
    assert_eq!(wifi.hw_address, "11:22:33:44:55:66");
    assert_eq!(wifi.kind, DeviceKind::Wifi);
    assert_eq!(wifi.state, DeviceState::Activated);
    assert_eq!(wifi.wifi.as_ref().unwrap().access_points.len(), 4);

    let eth = snapshot.devices.iter().find(|d| d.kind == DeviceKind::Ethernet).expect("the Ethernet device");
    assert_eq!(eth.state, DeviceState::Unavailable);
    assert_eq!(eth.wired.as_ref().map(|w| w.carrier), Some(false));

    // Two access points of "Lumen Home" make one network, connected and saved, listed first.
    let networks = &snapshot.networks()[0].1;
    let names: Vec<String> = networks.iter().map(|n| n.name()).collect();
    assert_eq!(names, ["Lumen Home", "Aster Café", "Bibliothek-Free"]);
    let home = &networks[0];
    assert!(home.active);
    assert!(home.saved.is_some());
    assert_eq!(home.access_points.len(), 2);
    assert_eq!(home.strongest().path, home_5);
    assert_eq!(home.bands, [Band::Ghz2_4, Band::Ghz5]);
    assert_eq!(home.security, Security::Wpa2);
    assert_eq!(networks[2].security, Security::Open);

    let saved = &snapshot.connections[0];
    assert_eq!(saved.id, "Lumen Home"); // the mock names a profile after its SSID
    assert_eq!(saved.kind, ConnectionKind::Wifi { ssid: b"Lumen Home".to_vec() });

    let active = &snapshot.active[0];
    assert_eq!(active.state, ActiveState::Activated);
    assert_eq!(active.devices, [wifi_path]);
    assert_eq!(active.specific_object.as_deref(), Some(home_5.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_reports_changes() {
    let Some(bus) = Bus::start(true).await else { return };
    let (wifi_path, _) = home_setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let mut snapshots = Box::pin(client.watch().await.unwrap());
    let first = snapshots.next().await.unwrap().unwrap();
    assert!(first.wireless_enabled);

    // Wi-Fi switched off elsewhere (nmcli, a key): the next snapshot shows it.
    let () = bus
        .mock(
            "SetProperty",
            &(MANAGER, "org.freedesktop.NetworkManager", "WirelessEnabled", zbus::zvariant::Value::from(false)),
        )
        .await;
    let next =
        tokio::time::timeout(Duration::from_secs(5), snapshots.next()).await.expect("a new snapshot").unwrap().unwrap();
    assert!(!next.wireless_enabled);

    // A new access point in range.
    let _: String = bus
        .mock(
            "AddAccessPoint",
            &(&wifi_path, "new", "Nachbar_5G", "AA:BB:CC:DD:EE:FF", 2u32, 5745u32, 0u32, 20u8, SEC_WPA2_PSK),
        )
        .await;
    let after = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let s = snapshots.next().await.unwrap().unwrap();
            if s.device(&wifi_path).unwrap().wifi.as_ref().unwrap().access_points.len() == 5 {
                return s;
            }
        }
    })
    .await
    .expect("the new access point shows up");
    assert!(after.networks()[0].1.iter().any(|n| n.name() == "Nachbar_5G"));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_network_manager_is_reported() {
    let Some(bus) = Bus::start(false).await else { return };
    let client = Client::new(bus.connect().await);
    assert!(matches!(client.snapshot().await, Err(Error::NotRunning)));
}

#[tokio::test(flavor = "multi_thread")]
async fn wifi_switch_and_scan() {
    let Some(bus) = Bus::start(true).await else { return };
    let (wifi, _) = home_setup(&bus).await;
    let client = Client::new(bus.connect().await);
    client.set_wireless_enabled(false).await.unwrap();
    assert!(!client.snapshot().await.unwrap().wireless_enabled);
    client.set_wireless_enabled(true).await.unwrap();
    assert!(client.snapshot().await.unwrap().wireless_enabled);
    client.request_scan(&wifi).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_reconnect_and_forget() {
    let Some(bus) = Bus::start(true).await else { return };
    let (wifi, home_5) = home_setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let snap = client.snapshot().await.unwrap();
    let active = snap.device(&wifi).unwrap().active_connection.clone().expect("connected");
    let saved = snap.connections[0].path.clone();

    client.deactivate(&active).await.unwrap();
    let snap = client.snapshot().await.unwrap();
    assert!(snap.active.is_empty());
    assert!(!snap.networks()[0].1.iter().any(|n| n.active));

    client.activate(&saved, Some(&wifi), Some(&home_5)).await.unwrap();
    let snap = client.snapshot().await.unwrap();
    assert_eq!(snap.active.len(), 1);
    assert!(snap.networks()[0].1[0].active);

    client.forget(&saved).await.unwrap();
    let snap = client.snapshot().await.unwrap();
    assert!(snap.connections.is_empty());
    assert!(snap.active.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn join_an_open_network() {
    let Some(bus) = Bus::start(true).await else { return };
    let (wifi, _) = home_setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let snap = client.snapshot().await.unwrap();
    let library = snap.networks()[0].1.iter().find(|n| n.name() == "Bibliothek-Free").unwrap().strongest().path.clone();
    let (connection, _active) = client.connect_open(&wifi, &library, b"Bibliothek-Free").await.unwrap();
    let snap = client.snapshot().await.unwrap();
    let saved = snap.connection(&connection).expect("a new profile");
    assert_eq!(saved.wifi_ssid(), Some(&b"Bibliothek-Free"[..]));
    assert!(snap.networks()[0].1.iter().any(|n| n.name() == "Bibliothek-Free" && n.saved.is_some()));
}
