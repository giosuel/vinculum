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

    /// Calls a mock method on another of the mock's objects.
    async fn mock_at<B, R>(&self, path: &str, method: &str, body: &B) -> R
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let conn = self.connect().await;
        let reply = conn.call_method(Some(vinculum_nm::BUS_NAME), path, Some(MOCK), method, body).await.unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn join_a_secured_network_with_the_password_in_the_keyring() {
    use vinculum_nm::{KeyMgmt, Storage, WifiJoin};
    let Some(bus) = Bus::start(true).await else { return };
    let (wifi, _) = home_setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let snap = client.snapshot().await.unwrap();
    // The mock can't match non-ASCII names, so not "Aster Café".
    let library = snap.networks()[0].1.iter().find(|n| n.name() == "Bibliothek-Free").unwrap().strongest().path.clone();
    let join = WifiJoin {
        ssid: "Bibliothek-Free".into(),
        uuid: "0b8c3d0e-1f2a-4b5c-8d9e-0f1a2b3c4d5e".into(),
        security: Some((KeyMgmt::WpaPsk, "café-pass".into())),
        storage: Storage::Agent,
        owner: Some("giosuel".into()),
        autoconnect: true,
        hidden: false,
    };
    client.join_wifi(&wifi, Some(&library), &join).await.unwrap();
    // The mock makes its own profile from the access point, so check what was sent instead.
    let calls: Vec<(u64, String, Vec<zbus::zvariant::OwnedValue>)> = bus.mock("GetCalls", &()).await;
    let (_, _, args) = calls.iter().find(|(_, m, _)| m == "AddAndActivateConnection").expect("the call");
    let settings =
        std::collections::HashMap::<String, std::collections::HashMap<String, zbus::zvariant::OwnedValue>>::try_from(
            args[0].try_clone().unwrap(),
        )
        .unwrap();
    assert_eq!(<&str>::try_from(&*settings["connection"]["uuid"]).unwrap(), join.uuid);
    assert_eq!(<&str>::try_from(&*settings["802-11-wireless-security"]["psk"]).unwrap(), "café-pass");
    let security = &settings["802-11-wireless-security"];
    assert_eq!(u32::try_from(&security["psk-flags"]).unwrap(), 1, "agent-owned");
    let permissions = <Vec<String>>::try_from(settings["connection"]["permissions"].clone()).unwrap();
    assert_eq!(permissions, ["user:giosuel:"]);
}

/// The agent registers with the AgentManager and answers NetworkManager (here, the mock calling
/// it the way NetworkManager does), but no one else.
#[tokio::test(flavor = "multi_thread")]
async fn secret_agent_answers_only_network_manager() {
    use vinculum_nm::{AgentEvent, Refusal};
    let Some(bus) = Bus::start(true).await else { return };
    // The mock has no AgentManager: add one, and a test method that calls the agent from the
    // mock's own connection (the one that owns NetworkManager's name), remembering the answer.
    let _: () = bus
        .mock(
            "AddObject",
            &(
                "/org/freedesktop/NetworkManager/AgentManager",
                "org.freedesktop.NetworkManager.AgentManager",
                std::collections::HashMap::<String, zbus::zvariant::Value>::new(),
                vec![("RegisterWithCapabilities", "su", "", "")],
            ),
        )
        .await;
    let call_agent_code = r#"
import dbus
self.answer = 'pending'
def ok(secrets, self=self): self.answer = 'ok ' + str(secrets['802-11-wireless-security']['psk'])
def err(e, self=self): self.answer = 'error ' + e.get_dbus_name()
agent = self.connection.get_object(args[0], '/org/freedesktop/NetworkManager/SecretAgent')
settings = {
    'connection': {'id': 'Aster Café', 'uuid': 'u-cafe', 'type': '802-11-wireless'},
    '802-11-wireless': {'ssid': dbus.ByteArray(b'Aster Caf\xc3\xa9')},
    '802-11-wireless-security': {'key-mgmt': 'wpa-psk', 'psk-flags': dbus.UInt32(1)},
}
agent.GetSecrets(dbus.Dictionary(settings, signature='sa{sv}'), dbus.ObjectPath('/org/freedesktop/NetworkManager/Settings/9'),
    '802-11-wireless-security', dbus.Array([], signature='s'), dbus.UInt32(args[1]),
    dbus_interface='org.freedesktop.NetworkManager.SecretAgent', reply_handler=ok, error_handler=err, timeout=30)
"#;
    let _: () =
        bus.mock("AddMethod", &("org.freedesktop.DBus.Mock.Test", "CallAgent", "su", "", call_agent_code)).await;
    let _: () = bus
        .mock("AddMethod", &("org.freedesktop.DBus.Mock.Test", "Answer", "", "s", "ret = getattr(self, 'answer', '')"))
        .await;

    let agent_conn = bus.connect().await;
    let agent_name = agent_conn.unique_name().unwrap().to_string();
    let client = Client::new(agent_conn);
    let mut events = client.register_agent("ch.giosuel.Vinculum").await.expect("registered");
    let calls: Vec<(u64, String, Vec<zbus::zvariant::OwnedValue>)> =
        bus.mock_at("/org/freedesktop/NetworkManager/AgentManager", "GetCalls", &()).await;
    assert!(calls.iter().any(|(_, m, _)| m == "RegisterWithCapabilities"));

    // Answered with a password.
    call_agent(&bus, &agent_name, 1).await;
    let Some(AgentEvent::GetSecrets(request)) = events.recv().await else { panic!("a request") };
    assert_eq!(request.connection.id, "Aster Café");
    assert_eq!(request.connection.ssid, "Aster Café".as_bytes());
    assert_eq!(request.setting, "802-11-wireless-security");
    assert_eq!(request.connection.secret_flags("802-11-wireless-security", "psk"), 1);
    assert!(request.interactive() && !request.request_new());
    request.reply(Ok([("psk".to_owned(), "café-pass".to_owned())].into()));
    assert_eq!(agent_answer(&bus).await, "ok café-pass");

    // The user cancelled.
    call_agent(&bus, &agent_name, 3).await;
    let Some(AgentEvent::GetSecrets(request)) = events.recv().await else { panic!("a request") };
    assert!(request.request_new());
    request.reply(Err(Refusal::UserCanceled));
    assert_eq!(agent_answer(&bus).await, "error org.freedesktop.NetworkManager.SecretAgent.UserCanceled");

    // Someone other than NetworkManager asks.
    let other = bus.connect().await;
    let settings: std::collections::HashMap<&str, std::collections::HashMap<&str, zbus::zvariant::Value>> =
        [("connection", [("id", "x".into())].into())].into();
    let refused = other
        .call_method(
            Some(agent_name.as_str()),
            "/org/freedesktop/NetworkManager/SecretAgent",
            Some("org.freedesktop.NetworkManager.SecretAgent"),
            "GetSecrets",
            &(
                settings,
                zbus::zvariant::ObjectPath::try_from("/x").unwrap(),
                "802-11-wireless-security",
                Vec::<String>::new(),
                1u32,
            ),
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("NotAuthorized"), "{refused}");
    assert!(events.try_recv().is_err(), "no request reached the app");
}

async fn call_agent(bus: &Bus, agent: &str, flags: u32) {
    let conn = bus.connect().await;
    conn.call_method(
        Some(vinculum_nm::BUS_NAME),
        MANAGER,
        Some("org.freedesktop.DBus.Mock.Test"),
        "CallAgent",
        &(agent, flags),
    )
    .await
    .unwrap();
}

/// What the agent answered the mock's last `CallAgent`.
async fn agent_answer(bus: &Bus) -> String {
    let conn = bus.connect().await;
    for _ in 0..100 {
        let reply = conn
            .call_method(Some(vinculum_nm::BUS_NAME), MANAGER, Some("org.freedesktop.DBus.Mock.Test"), "Answer", &())
            .await
            .unwrap();
        let text: String = reply.body().deserialize().unwrap();
        if text != "pending" && !text.is_empty() {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the agent didn't answer");
}

/// A wrong password: the device fails with "supplicant disconnected", then the attempt ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_attempt_says_why() {
    use vinculum_nm::Outcome;
    let Some(bus) = Bus::start(true).await else { return };
    let (wifi, _) = home_setup(&bus).await;
    let cafe_conn: String = bus.mock("AddWiFiConnection", &(&wifi, "cafeconn", "Bibliothek-Free", "wpa-psk")).await;
    // State 1: activating.
    let active: String = bus.mock("AddActiveConnection", &(vec![&wifi], &cafe_conn, "/", "cafeactive", 1u32)).await;
    let client = Client::new(bus.connect().await);
    let waiting = tokio::spawn({
        let (client, active, wifi) = (client.clone(), active.clone(), wifi.clone());
        async move { client.outcome(&active, Some(&wifi)).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let emit =
        |path: String, interface: &'static str, signature: &'static str, args: Vec<zbus::zvariant::Value<'static>>| {
            let bus = &bus;
            async move {
                let conn = bus.connect().await;
                conn.call_method(
                    Some(vinculum_nm::BUS_NAME),
                    path.as_str(),
                    Some(MOCK),
                    "EmitSignal",
                    &(interface, "StateChanged", signature, args),
                )
                .await
                .unwrap();
            }
        };
    // Device: FAILED (120) from CONFIG (50), reason SUPPLICANT_DISCONNECT (8).
    emit(wifi.clone(), "org.freedesktop.NetworkManager.Device", "uuu", vec![120u32.into(), 50u32.into(), 8u32.into()])
        .await;
    // Active connection: DEACTIVATED (4), reason DEVICE_DISCONNECTED (4).
    emit(active.clone(), "org.freedesktop.NetworkManager.Connection.Active", "uu", vec![4u32.into(), 4u32.into()])
        .await;
    let outcome = tokio::time::timeout(Duration::from_secs(5), waiting).await.unwrap().unwrap().unwrap();
    assert_eq!(outcome, Outcome::Failed("The network rejected the password.".into()));
}

/// The editor's round trip: read the settings, write them back changed, add a new connection,
/// flip one key, set the hostname. The mock has only the older Update and AddConnection, so this
/// also covers the fallbacks.
#[tokio::test(flavor = "multi_thread")]
async fn settings_round_trip() {
    use zbus::zvariant::{OwnedValue, Value};
    let Some(bus) = Bus::start(true).await else { return };
    let (_wifi, _) = home_setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let snap = client.snapshot().await.unwrap();
    let home = snap.connections[0].path.clone();

    let mut settings = client.settings(&home).await.unwrap();
    assert_eq!(<&str>::try_from(&*settings["connection"]["id"]).unwrap(), "Lumen Home");
    settings
        .get_mut("connection")
        .unwrap()
        .insert("autoconnect-priority".into(), OwnedValue::try_from(Value::from(20i32)).unwrap());
    client.update(&home, &settings).await.unwrap();
    let saved = client.settings(&home).await.unwrap();
    assert_eq!(i32::try_from(&*saved["connection"]["autoconnect-priority"]).unwrap(), 20);
    // What the editor didn't touch is still there.
    assert_eq!(saved["802-11-wireless"]["seen-bssids"], settings["802-11-wireless"]["seen-bssids"]);

    client.set_setting(&home, "connection", "autoconnect", Value::from(false)).await.unwrap();
    assert!(!client.snapshot().await.unwrap().connections[0].autoconnect);

    let mut new: vinculum_nm::Settings = std::collections::HashMap::new();
    for (setting, key, value) in [
        ("connection", "id", Value::from("Office dock")),
        ("connection", "type", Value::from("802-3-ethernet")),
        ("connection", "uuid", Value::from("3e7f1c2a-0b4d-4e5f-8a9b-0c1d2e3f4a5b")),
        ("ipv4", "method", Value::from("auto")),
    ] {
        new.entry(setting.into()).or_default().insert(key.into(), OwnedValue::try_from(value).unwrap());
    }
    let path = client.add(&new).await.unwrap();
    let snap = client.snapshot().await.unwrap();
    assert!(snap.connections.iter().any(|c| c.path == path && c.id == "Office dock"));

    // The mock has no SaveHostname; give it one that sets the property.
    let _: () = bus
        .mock_at(
            "/org/freedesktop/NetworkManager/Settings",
            "AddMethod",
            &(
                "org.freedesktop.NetworkManager.Settings",
                "SaveHostname",
                "s",
                "",
                "self.Set('org.freedesktop.NetworkManager.Settings', 'Hostname', args[0])",
            ),
        )
        .await;
    client.save_hostname("thinkpad-x1").await.unwrap();
    assert_eq!(client.snapshot().await.unwrap().hostname, "thinkpad-x1");
}
