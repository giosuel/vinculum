//! Files over Bluetooth (OBEX Object Push), through BlueZ's obexd on the session bus: sending
//! files to a device, and an agent that asks the user about files a device sends.
//!
//! obexd saves received files in its own folder; the app moves them where the user wants them
//! once they're complete (see [`Transfer::filename`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use crate::Error;

/// obexd's well-known name on the session bus.
pub const OBEX_NAME: &str = "org.bluez.obex";
const ROOT: &str = "/org/bluez/obex";
pub const OBEX_AGENT_PATH: &str = "/ch/giosuel/Lituus/ObexAgent";
const AGENT_IFACE: &str = "org.bluez.obex.Agent1";
const TRANSFER_IFACE: &str = "org.bluez.obex.Transfer1";

/// Where a transfer is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transfer {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub transferred: u64,
    /// "queued", "active", "suspended", "complete" or "error".
    pub status: String,
    /// Where the file is (sent from, or received into).
    pub filename: String,
}

impl Transfer {
    pub fn done(&self) -> bool {
        matches!(self.status.as_str(), "complete" | "error")
    }
}

/// A device wants to send a file. Answer with [`Self::accept`] (the file name to save it as in
/// obexd's folder) or [`Self::reject`]; dropping it rejects.
#[derive(Debug)]
pub struct IncomingFile {
    pub transfer: String,
    pub name: String,
    pub size: u64,
    /// A MIME type, when the sender gave one.
    pub kind: String,
    /// The sender's Bluetooth address.
    pub address: String,
    reply: oneshot::Sender<Option<String>>,
}

impl IncomingFile {
    pub fn accept(self, name: &str) {
        let _ = self.reply.send(Some(name.to_owned()));
    }

    pub fn reject(self) {
        let _ = self.reply.send(None);
    }
}

#[derive(Debug)]
pub enum ObexEvent {
    Push(IncomingFile),
    /// obexd gave up waiting: close the dialog.
    Cancel,
}

/// obexd on the session bus.
#[derive(Clone, Debug)]
pub struct Obex {
    conn: Connection,
    /// obexd's folder for received files.
    cache: PathBuf,
}

impl Obex {
    pub async fn session() -> Result<Self, Error> {
        Ok(Self::new(Connection::session().await?))
    }

    pub fn new(conn: Connection) -> Self {
        Self { conn, cache: cache_dir() }
    }

    /// With another folder for received files than obexd's usual one (for tests).
    pub fn with_cache(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache = dir.into();
        self
    }

    async fn call<B, R>(&self, path: &str, iface: &str, method: &str, body: &B) -> Result<R, Error>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let reply =
            self.conn.call_method(Some(OBEX_NAME), path, Some(iface), method, body).await.map_err(Error::from_dbus)?;
        Ok(reply.body().deserialize()?)
    }

    /// Starts sending files to a device; they go one after another. Returns the session (to
    /// close with [`Self::close`]) and the transfers, in order.
    pub async fn send_files(&self, address: &str, files: &[PathBuf]) -> Result<(String, Vec<String>), Error> {
        let mut args: HashMap<&str, Value> = HashMap::new();
        args.insert("Target", Value::from("opp"));
        let session: OwnedObjectPath =
            self.call(ROOT, "org.bluez.obex.Client1", "CreateSession", &(address, args)).await?;
        let mut transfers = Vec::new();
        for file in files {
            let file = file.to_string_lossy().into_owned();
            let (transfer, _): (OwnedObjectPath, HashMap<String, OwnedValue>) =
                self.call(session.as_str(), "org.bluez.obex.ObjectPush1", "SendFile", &(file,)).await?;
            transfers.push(transfer.to_string());
        }
        Ok((session.to_string(), transfers))
    }

    pub async fn close(&self, session: &str) -> Result<(), Error> {
        let path = OwnedObjectPath::try_from(session).map_err(zbus::Error::from)?;
        self.call::<_, ()>(ROOT, "org.bluez.obex.Client1", "RemoveSession", &(path,)).await
    }

    /// A transfer as it is now; a finished one obexd has dropped reads as complete.
    pub async fn transfer(&self, path: &str) -> Result<Transfer, Error> {
        let props: Result<HashMap<String, OwnedValue>, Error> =
            self.call(path, "org.freedesktop.DBus.Properties", "GetAll", &(TRANSFER_IFACE,)).await;
        let props = match props {
            Ok(p) => p,
            // obexd removes a transfer when it's done.
            Err(Error::DBus(e)) if e.to_string().contains("Unknown") => {
                return Ok(Transfer { path: path.into(), status: "complete".into(), ..Transfer::default() });
            }
            Err(e) => return Err(e),
        };
        let text = |k: &str| props.get(k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
        let number = |k: &str| props.get(k).and_then(|v| u64::try_from(v.clone()).ok()).unwrap_or(0);
        Ok(Transfer {
            path: path.into(),
            name: text("Name"),
            size: number("Size"),
            transferred: number("Transferred"),
            status: text("Status"),
            filename: text("Filename"),
        })
    }

    pub async fn cancel(&self, transfer: &str) -> Result<(), Error> {
        self.call::<_, ()>(transfer, TRANSFER_IFACE, "Cancel", &()).await
    }

    /// Registers the agent for files devices send, and hands each request to the receiver.
    pub async fn register_agent(&self) -> Result<mpsc::UnboundedReceiver<ObexEvent>, Error> {
        let (events, receiver) = mpsc::unbounded_channel();
        let calls = MessageStream::from(&self.conn);
        let path = OwnedObjectPath::try_from(OBEX_AGENT_PATH).expect("a valid path");
        self.call::<_, ()>(ROOT, "org.bluez.obex.AgentManager1", "RegisterAgent", &(path,)).await?;
        // obexd forgets its agent when it restarts.
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, OBEX_NAME)?
            .build();
        let mut owners = MessageStream::for_match_rule(rule, &self.conn, Some(16)).await?;
        let (me, e) = (self.clone(), events.clone());
        tokio::spawn(async move {
            while let Some(Ok(m)) = owners.next().await {
                if e.is_closed() {
                    break;
                }
                if m.body().deserialize::<(String, String, String)>().is_ok_and(|(_, _, n)| !n.is_empty()) {
                    let path = OwnedObjectPath::try_from(OBEX_AGENT_PATH).expect("a valid path");
                    let _ = me.call::<_, ()>(ROOT, "org.bluez.obex.AgentManager1", "RegisterAgent", &(path,)).await;
                }
            }
        });
        tokio::spawn(serve(self.clone(), calls, events));
        Ok(receiver)
    }

    async fn sender_address(&self, transfer: &Transfer, session: &str) -> String {
        let _ = transfer;
        let props: Result<HashMap<String, OwnedValue>, Error> =
            self.call(session, "org.freedesktop.DBus.Properties", "GetAll", &("org.bluez.obex.Session1",)).await;
        props.ok().and_then(|p| p.get("Destination").and_then(|v| String::try_from(v.clone()).ok())).unwrap_or_default()
    }
}

async fn serve(obex: Obex, mut calls: MessageStream, events: mpsc::UnboundedSender<ObexEvent>) {
    let conn = obex.conn.clone();
    let dbus = zbus::fdo::DBusProxy::new(&conn).await.ok();
    while let Some(Ok(message)) = calls.next().await {
        if events.is_closed() {
            break;
        }
        let header = message.header();
        if header.message_type() != MessageType::MethodCall
            || header.path().map(|p| p.as_str()) != Some(OBEX_AGENT_PATH)
        {
            continue;
        }
        if header.interface().map(|i| i.as_str()) != Some(AGENT_IFACE) {
            let _ = conn.reply_error(&header, "org.freedesktop.DBus.Error.UnknownMethod", &("Unknown method",)).await;
            continue;
        }
        // Only obexd may ask.
        let from_obex = match &dbus {
            Some(d) => match d.get_name_owner(OBEX_NAME.try_into().expect("a valid name")).await {
                Ok(owner) => header.sender().is_some_and(|s| s.as_str() == owner.as_str()),
                Err(_) => false,
            },
            None => false,
        };
        if !from_obex {
            let _ = conn.reply_error(&header, "org.bluez.obex.Error.Rejected", &("Not obexd",)).await;
            continue;
        }
        match header.member().map(|m| m.as_str()) {
            Some("AuthorizePush") => {
                let Ok((transfer,)) = message.body().deserialize::<(OwnedObjectPath,)>() else { continue };
                let (obex, conn, events, message) = (obex.clone(), conn.clone(), events.clone(), message.clone());
                tokio::spawn(async move {
                    let header = message.header();
                    let props: HashMap<String, OwnedValue> = obex
                        .call(transfer.as_str(), "org.freedesktop.DBus.Properties", "GetAll", &(TRANSFER_IFACE,))
                        .await
                        .unwrap_or_default();
                    let text =
                        |k: &str| props.get(k).and_then(|v| String::try_from(v.clone()).ok()).unwrap_or_default();
                    let session = props
                        .get("Session")
                        .and_then(|v| OwnedObjectPath::try_from(v.clone()).ok())
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                    let t = Transfer {
                        path: transfer.to_string(),
                        name: text("Name"),
                        size: props.get("Size").and_then(|v| u64::try_from(v.clone()).ok()).unwrap_or(0),
                        ..Transfer::default()
                    };
                    let address = obex.sender_address(&t, &session).await;
                    let (reply, answer) = oneshot::channel();
                    let file = IncomingFile {
                        transfer: t.path.clone(),
                        name: t.name.clone(),
                        size: t.size,
                        kind: text("Type"),
                        address,
                        reply,
                    };
                    if events.send(ObexEvent::Push(file)).is_err() {
                        let _ = conn.reply_error(&header, "org.bluez.obex.Error.Rejected", &("Not now",)).await;
                        return;
                    }
                    match answer.await {
                        Ok(Some(name)) => {
                            let path = obex.cache.join(safe_name(&name));
                            let _ = conn.reply(&header, &(path.to_string_lossy().into_owned(),)).await;
                        }
                        _ => {
                            let _ = conn.reply_error(&header, "org.bluez.obex.Error.Rejected", &("Declined",)).await;
                        }
                    }
                });
            }
            Some("Cancel") => {
                let _ = events.send(ObexEvent::Cancel);
                let _ = conn.reply(&header, &()).await;
            }
            _ => {
                let _ = conn.reply(&header, &()).await;
            }
        }
    }
}

/// obexd's folder for files it receives: `$XDG_CACHE_HOME/obexd`.
pub fn cache_dir() -> PathBuf {
    match std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        Some(c) => PathBuf::from(c).join("obexd"),
        None => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".cache/obexd"),
    }
}

/// A sender's file name, without folders or a leading dot.
pub fn safe_name(name: &str) -> String {
    let base = Path::new(name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let base = base.trim_start_matches('.').to_owned();
    if base.is_empty() { "file".to_owned() } else { base }
}

/// `dir/name`, or `dir/name (2).ext` and so on if that's taken.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).expect("a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_in_their_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(".bashrc"), "bashrc");
        assert_eq!(safe_name("PXL_1.jpg"), "PXL_1.jpg");
        assert_eq!(safe_name(""), "file");
    }

    #[test]
    fn taken_names_get_a_number() {
        let dir = std::env::temp_dir().join(format!("lituus-obex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_path(&dir, "a.jpg"), dir.join("a.jpg"));
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        assert_eq!(free_path(&dir, "a.jpg"), dir.join("a (2).jpg"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
