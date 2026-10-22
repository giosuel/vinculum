//! One Vinculum per user: the first one owns `ch.giosuel.Vinculum` on the session bus, and a
//! second start passes its request on instead (`vinculum` shows the window, `vinculum --quick`
//! the popup, `vinculum --toggle-wifi` switches Wi-Fi; from a keybind or Waybar). Only one can be
//! NetworkManager's secret agent anyway.

use std::sync::mpsc;
use std::time::Duration;

use futures_util::StreamExt;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type as MessageType;
use zbus::{Connection, MessageStream};

const PATH: &str = "/ch/giosuel/Vinculum";
const IFACE: &str = "ch.giosuel.Vinculum1";

/// What a start asks the running Vinculum for: the methods of `ch.giosuel.Vinculum1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Show,
    Quick,
    ToggleWifi,
}

impl Request {
    fn method(self) -> &'static str {
        match self {
            Self::Show => "Show",
            Self::Quick => "Quick",
            Self::ToggleWifi => "ToggleWifi",
        }
    }

    fn from_method(name: &str) -> Option<Self> {
        [Self::Show, Self::Quick, Self::ToggleWifi].into_iter().find(|r| r.method() == name)
    }
}

/// Whether this process is the one to run.
pub enum Claim {
    /// It is: `handle` is called with what later starts ask for.
    First,
    /// Another one is running, and took the request.
    Passed,
}

/// Claims the name on a thread of its own, which then keeps answering requests. If another
/// process has it, that one gets `request` (with none, a second `vinculum --background` just
/// leaves). Without a session bus there's nothing to share, so this process runs.
pub fn claim(app_id: &'static str, request: Option<Request>, handle: impl Fn(Request) + Send + 'static) -> Claim {
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
                    serve(&conn, handle).await;
                }
                _ => {
                    let passed = match request {
                        None => true,
                        Some(r) => conn.call_method(Some(app_id), PATH, Some(IFACE), r.method(), &()).await.is_ok(),
                    };
                    let _ = result.send(if passed { Claim::Passed } else { Claim::First });
                }
            }
        });
    });
    claimed.recv_timeout(Duration::from_secs(5)).unwrap_or(Claim::First)
}

async fn serve(conn: &Connection, handle: impl Fn(Request)) {
    let mut calls = MessageStream::from(conn);
    while let Some(Ok(message)) = calls.next().await {
        let header = message.header();
        if header.message_type() != MessageType::MethodCall || header.path().map(|p| p.as_str()) != Some(PATH) {
            continue;
        }
        let request = header.member().and_then(|m| Request::from_method(m.as_str()));
        if let Some(request) = request.filter(|_| header.interface().map(|i| i.as_str()) == Some(IFACE)) {
            handle(request);
            let _ = conn.reply(&header, &()).await;
        } else {
            let _ = conn.reply_error(&header, "org.freedesktop.DBus.Error.UnknownMethod", &("Unknown method",)).await;
        }
    }
}
