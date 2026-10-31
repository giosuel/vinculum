//! The keyring backends against real services on private buses.
//!
//! - The Secret Service test needs a bus with an unlocked keyring on it, given as
//!   LITUUS_KEYRING_BUS (scripts/test.sh starts gnome-keyring when it's installed).
//! - The KWallet test runs a python-dbusmock stand-in for kwalletd6 (tests/kwalletd6.py), with
//!   LITUUS_DBUSMOCK=1 like the NetworkManager tests.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use lituus_keyring::{Entry, Keyring};
use zbus::Connection;

async fn connect(address: &str) -> Connection {
    zbus::connection::Builder::address(address).unwrap().build().await.unwrap()
}

/// Stores, finds, replaces and deletes, the way the secret agent uses a keyring.
async fn round_trip(keyring: &Keyring) {
    // A fresh UUID each run, so a keyring that outlives the test doesn't matter.
    let uuid = format!("test-{}", std::process::id());
    let psk = Entry { uuid: &uuid, setting: "802-11-wireless-security", key: "psk" };
    let wep = Entry { key: "wep-key0", ..psk };
    let other = Entry { setting: "802-1x", key: "password", ..psk };

    assert!(keyring.find(&uuid, psk.setting).await.unwrap().is_empty());
    keyring.store(psk, "Aster Café", "first").await.unwrap();
    keyring.store(psk, "Aster Café", "café-pass 2").await.unwrap();
    keyring.store(wep, "Aster Café", "wep").await.unwrap();
    keyring.store(other, "Aster Café", "eap").await.unwrap();

    let found = keyring.find(&uuid, psk.setting).await.unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found["psk"], "café-pass 2");
    assert_eq!(found["wep-key0"], "wep");
    assert_eq!(keyring.find(&uuid, "802-1x").await.unwrap()["password"], "eap");
    assert!(keyring.find("another-uuid", psk.setting).await.unwrap().is_empty());

    keyring.delete(&uuid).await.unwrap();
    assert!(keyring.find(&uuid, psk.setting).await.unwrap().is_empty());
    assert!(keyring.find(&uuid, "802-1x").await.unwrap().is_empty());
}

#[tokio::test]
async fn secret_service() {
    let Ok(address) = std::env::var("LITUUS_KEYRING_BUS") else {
        // Our CI installs gnome-keyring, so skipping there would hide a broken set-up. Other CI
        // (the AUR package's check() runs in a release job) has none.
        assert!(
            std::env::var_os("LITUUS_REQUIRE_KEYRING").is_none(),
            "LITUUS_REQUIRE_KEYRING is set, but there's no keyring to test against"
        );
        eprintln!("skipped: set LITUUS_KEYRING_BUS to a bus with an unlocked Secret Service");
        return;
    };
    let keyring = Keyring::open(connect(&address).await).await.expect("a keyring on that bus");
    assert_eq!(keyring.name(), "keyring");
    round_trip(&keyring).await;
}

struct Bus {
    address: String,
    _daemon: Child,
    _mock: Option<Child>,
}

async fn private_bus() -> Option<Bus> {
    if std::env::var("LITUUS_DBUSMOCK").as_deref() != Ok("1") {
        eprintln!("skipped: set LITUUS_DBUSMOCK=1 (scripts/test.sh does) to run against python-dbusmock");
        return None;
    }
    let config = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/bus.conf");
    let mut daemon = Command::new("dbus-daemon")
        .args(["--config-file", config, "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("dbus-daemon runs");
    let mut line = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut line).await.unwrap();
    Some(Bus { address: line.trim().to_owned(), _daemon: daemon, _mock: None })
}

#[tokio::test]
async fn nothing_on_the_bus() {
    let Some(bus) = private_bus().await else { return };
    assert!(Keyring::open(connect(&bus.address).await).await.is_none());
}

#[tokio::test]
async fn kwallet() {
    let Some(mut bus) = private_bus().await else { return };
    let python = std::env::var("LITUUS_PYTHON").unwrap_or_else(|_| "python3".into());
    let template = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/kwalletd6.py");
    bus._mock = Some(
        Command::new(python)
            .args(["-m", "dbusmock", "--session", "--template", template])
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("python-dbusmock runs"),
    );
    let conn = connect(&bus.address).await;
    let dbus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    for _ in 0..100 {
        if dbus.name_has_owner("org.kde.kwalletd6".try_into().unwrap()).await.unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let keyring = Keyring::open(conn).await.expect("kwalletd6 is found");
    assert_eq!(keyring.name(), "KWallet");
    round_trip(&keyring).await;
}
