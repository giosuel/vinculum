//! Against a mock BlueZ (python-dbusmock's template) on a private bus, so the real system bus is
//! never touched.
//!
//! Needs dbus-daemon and python-dbusmock, so it runs only with LITUUS_DBUSMOCK=1
//! (scripts/test.sh and CI set it). LITUUS_PYTHON picks the interpreter that has dbusmock.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use lituus_bt::agent::AGENT_PATH;
use lituus_bt::{AgentEvent, Answer, Ask, Client, DeviceKind, Error, Snapshot};
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};

const MOCK: &str = "org.freedesktop.DBus.Mock";
const BLUEZ_MOCK: &str = "org.bluez.Mock";
const HEADPHONES: &str = "AA:BB:CC:00:00:01";
const PHONE: &str = "AA:BB:CC:00:00:02";

/// A private bus, optionally with the mock BlueZ on it. Stopped when dropped.
struct Bus {
    address: String,
    daemon: Child,
    mock: Option<Child>,
}

impl Bus {
    async fn start(with_bluez: bool) -> Option<Self> {
        if std::env::var("LITUUS_DBUSMOCK").as_deref() != Ok("1") {
            eprintln!("skipped: set LITUUS_DBUSMOCK=1 (scripts/test.sh does) to run against the mock BlueZ");
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
        let mut bus = Self { address: line.trim().to_owned(), daemon, mock: None };
        if with_bluez {
            let python = std::env::var("LITUUS_PYTHON").unwrap_or_else(|_| "python3".into());
            bus.mock = Some(
                Command::new(python)
                    .args(["-m", "dbusmock", "--session", "--template", "bluez5"])
                    .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
                    .stdout(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .expect("python-dbusmock runs"),
            );
            bus.wait_for(lituus_bt::BUS_NAME).await;
        }
        Some(bus)
    }

    async fn connect(&self) -> Connection {
        zbus::connection::Builder::address(self.address.as_str()).unwrap().build().await.unwrap()
    }

    async fn wait_for(&self, name: &str) {
        let conn = self.connect().await;
        let dbus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
        for _ in 0..100 {
            if dbus.name_has_owner(name.try_into().unwrap()).await.unwrap() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("{name} didn't start");
    }

    /// Calls a mock method on one of the mock's objects.
    async fn mock<B, R>(&self, path: &str, interface: &str, method: &str, body: &B) -> R
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let conn = self.connect().await;
        let reply = conn.call_method(Some(lituus_bt::BUS_NAME), path, Some(interface), method, body).await.unwrap();
        reply.body().deserialize().unwrap()
    }

    /// Changes properties of a mock object, as BlueZ would (with PropertiesChanged).
    async fn update(&self, path: &str, interface: &str, props: HashMap<&str, Value<'_>>) {
        self.mock::<_, ()>(path, MOCK, "UpdateProperties", &(interface, props)).await;
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

/// A laptop with paired headphones and a phone nearby. Returns the adapter's and the two
/// devices' paths.
async fn setup(bus: &Bus) -> (String, String, String) {
    let adapter: String = bus.mock("/", BLUEZ_MOCK, "AddAdapter", &("hci0", "thinkpad-x1")).await;
    let headphones: String = bus.mock("/", BLUEZ_MOCK, "AddDevice", &("hci0", HEADPHONES, "WH-1000XM5")).await;
    bus.mock::<_, ()>("/", BLUEZ_MOCK, "PairDevice", &("hci0", HEADPHONES)).await;
    let uuids = vec![lituus_bt::uuid::A2DP_SINK, lituus_bt::uuid::HFP_HANDS_FREE];
    bus.update(
        &headphones,
        "org.bluez.Device1",
        HashMap::from([("Icon", Value::from("audio-headphones")), ("UUIDs", Value::from(uuids))]),
    )
    .await;
    let phone: String = bus.mock("/", BLUEZ_MOCK, "AddDevice", &("hci0", PHONE, "Pixel 8")).await;
    (adapter, headphones, phone)
}

/// Waits (up to 5 s) for a snapshot from `watch` that satisfies `done`.
async fn until(
    stream: &mut (impl futures_util::Stream<Item = Result<Snapshot, Error>> + Unpin),
    done: impl Fn(&Snapshot) -> bool,
) -> Snapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = stream.next().await.expect("the stream goes on").expect("a snapshot");
            if done(&snapshot) {
                return snapshot;
            }
        }
    })
    .await
    .expect("the change arrives")
}

#[tokio::test(flavor = "multi_thread")]
async fn adapters_and_devices_are_read() {
    let Some(bus) = Bus::start(true).await else { return };
    let (adapter, headphones, phone) = setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let snapshot = client.snapshot().await.unwrap();

    let a = snapshot.adapter().expect("an adapter");
    assert_eq!((a.path.as_str(), a.name.as_str(), a.alias.as_str()), (adapter.as_str(), "hci0", "thinkpad-x1"));
    assert!(a.powered && !a.blocked() && !a.discovering);
    let h = snapshot.device(&headphones).unwrap();
    assert_eq!((h.alias.as_str(), h.address.as_str()), ("WH-1000XM5", HEADPHONES));
    assert!(h.paired && h.audio() && !h.connected);
    assert_eq!(h.kind(), DeviceKind::Headphones);
    let p = snapshot.device(&phone).unwrap();
    assert!(!p.paired);
    assert_eq!(p.kind(), DeviceKind::Phone);
    assert_eq!(snapshot.devices_of(&adapter).count(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn power_and_discovery_show_up_in_the_watch() {
    let Some(bus) = Bus::start(true).await else { return };
    let (adapter, ..) = setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let mut stream = Box::pin(client.watch().await.unwrap());
    until(&mut stream, |s| s.adapter().is_some_and(|a| a.powered)).await;

    // The mock's StartDiscovery needs a filter set first; BlueZ's doesn't.
    let filter: HashMap<&str, Value> = HashMap::new();
    bus.mock::<_, ()>(&adapter, "org.bluez.Adapter1", "SetDiscoveryFilter", &(filter,)).await;
    client.start_discovery(&adapter).await.unwrap();
    until(&mut stream, |s| s.adapter().is_some_and(|a| a.discovering)).await;
    // Twice is fine.
    client.start_discovery(&adapter).await.unwrap();
    client.stop_discovery(&adapter).await.unwrap();
    until(&mut stream, |s| s.adapter().is_some_and(|a| !a.discovering)).await;

    client.set_powered(&adapter, false).await.unwrap();
    until(&mut stream, |s| s.adapter().is_some_and(|a| !a.powered)).await;
    client.set_discoverable(&adapter, true, 120).await.unwrap();
    let s = until(&mut stream, |s| s.adapter().is_some_and(|a| a.discoverable)).await;
    assert_eq!(s.adapter().unwrap().discoverable_timeout, 120);
    client.set_pairable(&adapter, false).await.unwrap();
    until(&mut stream, |s| s.adapter().is_some_and(|a| !a.pairable)).await;
    client.set_adapter_alias(&adapter, "work laptop").await.unwrap();
    until(&mut stream, |s| s.adapter().is_some_and(|a| a.alias == "work laptop")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_is_paired_connected_trusted_renamed_and_forgotten() {
    let Some(bus) = Bus::start(true).await else { return };
    let (adapter, headphones, phone) = setup(&bus).await;
    let client = Client::new(bus.connect().await);
    let mut stream = Box::pin(client.watch().await.unwrap());

    client.pair(&phone).await.unwrap();
    until(&mut stream, |s| s.device(&phone).is_some_and(|d| d.paired)).await;
    match client.pair(&phone).await {
        Err(e @ Error::Refused { .. }) => assert_eq!(e.describe(), "The device is already paired."),
        other => panic!("pairing twice: {other:?}"),
    }

    // The mock's Connect and Disconnect don't change the Connected property, so only their
    // answers are checked: connecting what's connected is fine, and so is disconnecting twice.
    client.connect(&headphones).await.unwrap();
    client.connect(&headphones).await.unwrap();
    client.disconnect(&headphones).await.unwrap();
    client.disconnect(&headphones).await.unwrap();

    client.set_trusted(&phone, true).await.unwrap();
    client.set_alias(&phone, "Work phone").await.unwrap();
    until(&mut stream, |s| s.device(&phone).is_some_and(|d| d.trusted && d.alias == "Work phone")).await;
    client.set_blocked(&phone, true).await.unwrap();
    until(&mut stream, |s| s.device(&phone).is_some_and(|d| d.blocked)).await;

    client.remove(&adapter, &phone).await.unwrap();
    let s = until(&mut stream, |s| s.device(&phone).is_none()).await;
    assert!(s.device(&headphones).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_bluez_it_says_so() {
    let Some(bus) = Bus::start(false).await else { return };
    let client = Client::new(bus.connect().await);
    assert!(matches!(client.snapshot().await, Err(Error::NotRunning)));
}

/// Plays BlueZ's agent manager, so the test can call the agent as BlueZ would.
#[derive(Clone, Default)]
struct AgentManager {
    registered: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.bluez.AgentManager1")]
impl AgentManager {
    fn register_agent(&self, agent: OwnedObjectPath, capability: String) {
        self.registered.lock().unwrap().push(format!("{agent} {capability}"));
    }

    fn request_default_agent(&self, agent: OwnedObjectPath) {
        self.registered.lock().unwrap().push(format!("{agent} default"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_passes_bluez_requests_on_and_answers_them() {
    let Some(bus) = Bus::start(false).await else { return };
    let manager = AgentManager::default();
    let bluez = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(lituus_bt::BUS_NAME)
        .unwrap()
        .serve_at("/org/bluez", manager.clone())
        .unwrap()
        .build()
        .await
        .unwrap();
    let app = bus.connect().await;
    let app_name = app.unique_name().unwrap().to_string();
    let mut events = Client::new(app).register_agent().await.unwrap();
    assert_eq!(
        *manager.registered.lock().unwrap(),
        [format!("{AGENT_PATH} KeyboardDisplay"), format!("{AGENT_PATH} default")]
    );

    let device = OwnedObjectPath::try_from("/org/bluez/hci0/dev_AA_BB_CC_00_00_02").unwrap();
    let call = |bluez: Connection, method: &'static str, body: (OwnedObjectPath, u32)| {
        let app_name = app_name.clone();
        tokio::spawn(async move {
            bluez.call_method(Some(app_name.as_str()), AGENT_PATH, Some("org.bluez.Agent1"), method, &body).await
        })
    };

    // The passkey matches: the user confirms.
    let confirm = call(bluez.clone(), "RequestConfirmation", (device.clone(), 123_456));
    let Some(AgentEvent::Ask(request)) = events.recv().await else { panic!("a request") };
    assert_eq!((request.device.as_str(), &request.ask), (device.as_str(), &Ask::Confirm(123_456)));
    request.reply(Answer::Accept);
    confirm.await.unwrap().expect("accepted");

    // It doesn't: the user says no.
    let confirm = call(bluez.clone(), "RequestConfirmation", (device.clone(), 654_321));
    let Some(AgentEvent::Ask(request)) = events.recv().await else { panic!("a request") };
    request.reply(Answer::Reject);
    let error = confirm.await.unwrap().expect_err("rejected");
    assert!(matches!(&error, zbus::Error::MethodError(name, ..) if name.as_str() == "org.bluez.Error.Rejected"));

    // A keyboard: the code to type is shown, padded to six digits.
    let body = (device.clone(), 42u32, 2u16);
    bluez
        .call_method(Some(app_name.as_str()), AGENT_PATH, Some("org.bluez.Agent1"), "DisplayPasskey", &body)
        .await
        .unwrap();
    let Some(AgentEvent::Display { code, entered, .. }) = events.recv().await else { panic!("a display") };
    assert_eq!((code.as_str(), entered), ("000042", Some(2)));

    // Only BlueZ may ask.
    let stranger = bus.connect().await;
    let refused = stranger
        .call_method(
            Some(app_name.as_str()),
            AGENT_PATH,
            Some("org.bluez.Agent1"),
            "RequestConfirmation",
            &(device.clone(), 1u32),
        )
        .await;
    assert!(refused.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(200), events.recv()).await.is_err(),
        "no request reached the app"
    );
}
