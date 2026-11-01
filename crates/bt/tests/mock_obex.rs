//! Against a mock obexd (scripts/mock-obex.py) on a private bus: sending files, and the agent
//! that asks about files a device sends. Runs only with LITUUS_DBUSMOCK=1, as the BlueZ tests.

use std::process::Stdio;
use std::time::Duration;

use lituus_bt::ObexEvent;
use lituus_bt::obex::{OBEX_NAME, Obex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[tokio::test(flavor = "multi_thread")]
async fn sends_and_receives_files() {
    if std::env::var("LITUUS_DBUSMOCK").as_deref() != Ok("1") {
        eprintln!("skipped: set LITUUS_DBUSMOCK=1 (scripts/test.sh does) to run against the mock obexd");
        return;
    }
    let mut daemon = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("dbus-daemon runs");
    let mut address = String::new();
    BufReader::new(daemon.stdout.take().unwrap()).read_line(&mut address).await.unwrap();
    let address = address.trim().to_owned();
    let python = std::env::var("LITUUS_PYTHON").unwrap_or_else(|_| "python3".into());
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/mock-obex.py");
    let _mock =
        Command::new(python).arg(script).env("DBUS_SESSION_BUS_ADDRESS", &address).kill_on_drop(true).spawn().unwrap();
    let conn = zbus::connection::Builder::address(address.as_str()).unwrap().build().await.unwrap();
    let dbus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
    for _ in 0..100 {
        if dbus.name_has_owner(OBEX_NAME.try_into().unwrap()).await.unwrap() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let dir = std::env::temp_dir().join(format!("lituus-obex-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let obex = Obex::new(conn.clone()).with_cache(dir.join("obexd"));

    // Sending two files: they go one after the other and finish.
    let (a, b) = (dir.join("a.jpg"), dir.join("b.pdf"));
    std::fs::write(&a, vec![1u8; 4000]).unwrap();
    std::fs::write(&b, vec![2u8; 1000]).unwrap();
    let (session, transfers) = obex.send_files("58:24:29:B1:0E:77", &[a, b]).await.unwrap();
    assert_eq!(transfers.len(), 2);
    let first = obex.transfer(&transfers[0]).await.unwrap();
    assert_eq!((first.name.as_str(), first.size), ("a.jpg", 4000));
    for _ in 0..100 {
        if obex.transfer(&transfers[1]).await.unwrap().done() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(obex.transfer(&transfers[0]).await.unwrap().status, "complete");
    assert_eq!(obex.transfer(&transfers[1]).await.unwrap().status, "complete");
    obex.close(&session).await.unwrap();

    // A phone sends a photo: the agent asks, and the file lands in obexd's folder.
    let mut events = obex.register_agent().await.unwrap();
    let pusher = conn.clone();
    tokio::spawn(async move {
        let _ = pusher
            .call_method(
                Some(OBEX_NAME),
                "/org/bluez/obex",
                Some("org.bluez.obex.Mock"),
                "Push",
                &("58:24:29:B1:0E:77", "../PXL_1.jpg", 3000u64),
            )
            .await;
    });
    let Some(ObexEvent::Push(file)) = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap() else {
        panic!("a push");
    };
    assert_eq!((file.name.as_str(), file.size, file.address.as_str()), ("../PXL_1.jpg", 3000, "58:24:29:B1:0E:77"));
    let transfer = file.transfer.clone();
    file.accept("../PXL_1.jpg");
    let mut received = None;
    for _ in 0..100 {
        let t = obex.transfer(&transfer).await.unwrap();
        if t.done() {
            received = Some(t);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let received = received.expect("the transfer finished");
    assert_eq!(received.status, "complete");
    // The sender's "../" doesn't take it out of the folder.
    assert_eq!(received.filename, dir.join("obexd/PXL_1.jpg").to_string_lossy());
    assert_eq!(std::fs::metadata(&received.filename).unwrap().len(), 3000);
    std::fs::remove_dir_all(&dir).unwrap();
}
