//! One Vinculum per user: the first one owns `ch.giosuel.Vinculum` on the session bus, and a
//! second start asks it to show its window instead (`vinculum` while `vinculum --background`
//! runs, from a keybind or Waybar). Only one can be NetworkManager's secret agent anyway.

use std::sync::mpsc;
use std::time::Duration;

use futures_util::StreamExt;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type as MessageType;
use zbus::{Connection, MessageStream};

const PATH: &str = "/ch/giosuel/Vinculum";
const IFACE: &str = "ch.giosuel.Vinculum1";

/// Whether this process is the one to run.
pub enum Claim {
    /// It is: `show` is called when another start asks for the window.
    First,
    /// Another one is running (and was asked to show its window).
    Shown,
}

/// Claims the name on a thread of its own, which then keeps answering `Show` calls. If another
/// process has it, that one is asked to show its window (unless `ask_to_show` is false: a second
/// `vinculum --background` just leaves). Without a session bus there's nothing to share, so this
/// process runs.
pub fn claim(app_id: &'static str, ask_to_show: bool, show: impl Fn() + Send + 'static) -> Claim {
    let (result, claimed) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
        runtime.block_on(async move {
            let Ok(conn) = Connection::session().await else {
                let _ = result.send(Claim::First);
                return;
            };
            match conn.request_name_with_flags(app_id, RequestNameFlags::DoNotQueue.into()).await {
                Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                    let _ = result.send(Claim::First);
                    serve(&conn, show).await;
                }
                _ => {
                    let shown =
                        !ask_to_show || conn.call_method(Some(app_id), PATH, Some(IFACE), "Show", &()).await.is_ok();
                    let _ = result.send(if shown { Claim::Shown } else { Claim::First });
                }
            }
        });
    });
    claimed.recv_timeout(Duration::from_secs(5)).unwrap_or(Claim::First)
}

async fn serve(conn: &Connection, show: impl Fn()) {
    let mut calls = MessageStream::from(conn);
    while let Some(Ok(message)) = calls.next().await {
        let header = message.header();
        if header.message_type() != MessageType::MethodCall || header.path().map(|p| p.as_str()) != Some(PATH) {
            continue;
        }
        if header.interface().map(|i| i.as_str()) == Some(IFACE) && header.member().map(|m| m.as_str()) == Some("Show")
        {
            show();
            let _ = conn.reply(&header, &()).await;
        } else {
            let _ = conn.reply_error(&header, "org.freedesktop.DBus.Error.UnknownMethod", &("Unknown method",)).await;
        }
    }
}
