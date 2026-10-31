//! Lituus's pairing agent: BlueZ asks it to show or confirm a code, to ask for a PIN, or to
//! allow a service while pairing or when an untrusted device connects. The agent only passes the
//! requests on, as [`AgentEvent`]s; the app asks the user.
//!
//! Method calls are read straight off the connection, as `lituus-nm`'s secret agent does, so
//! a request waiting for the user doesn't hold up `Cancel`.

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot};
use zbus::fdo::DBusProxy;
use zbus::message::{Header, Type as MessageType};
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, MatchRule, Message, MessageStream};

use crate::{BUS_NAME, Error};

/// Where BlueZ finds the agent on the agent's connection.
pub const AGENT_PATH: &str = "/ch/giosuel/Lituus/BluetoothAgent";
const AGENT_IFACE: &str = "org.bluez.Agent1";
const AGENT_MANAGER: &str = "org.bluez.AgentManager1";
/// It can show a code and take one typed in, so BlueZ picks the safest pairing both sides allow.
const CAPABILITY: &str = "KeyboardDisplay";

/// What BlueZ wants from the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// A PIN to type in (older devices); answer [`Answer::Pin`].
    PinCode,
    /// A six-digit passkey shown on the other device; answer [`Answer::Passkey`].
    Passkey,
    /// Whether the other device shows this passkey too; answer [`Answer::Accept`].
    Confirm(u32),
    /// Whether to pair with a device that shows no code at all (most headphones).
    Authorize,
    /// Whether an untrusted device may use a service, by its UUID.
    Service(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Pin(String),
    Passkey(u32),
    Accept,
    Reject,
    /// The user closed the dialog.
    Cancel,
}

/// BlueZ is waiting for the user. Answer with [`Self::reply`]; dropping it rejects.
#[derive(Debug)]
pub struct PairingRequest {
    /// The device's object path.
    pub device: String,
    pub ask: Ask,
    reply: oneshot::Sender<Answer>,
}

impl PairingRequest {
    pub fn reply(self, answer: Answer) {
        let _ = self.reply.send(answer);
    }

    /// A request for tests of what answers it.
    #[doc(hidden)]
    pub fn for_tests(device: &str, ask: Ask, reply: oneshot::Sender<Answer>) -> Self {
        Self { device: device.into(), ask, reply }
    }
}

#[derive(Debug)]
pub enum AgentEvent {
    Ask(PairingRequest),
    /// Show this code until pairing ends: type it on the device (a keyboard). `entered` counts
    /// the keys typed so far, when the device reports them.
    Display {
        device: String,
        code: String,
        entered: Option<u16>,
    },
    /// BlueZ doesn't need the answer any more (the device gave up, or it timed out): close the
    /// dialog.
    Cancel,
}

impl crate::Client {
    /// Registers the agent as the default one and hands every request to the returned receiver.
    /// It registers again whenever BlueZ restarts, and stops when the receiver is dropped.
    pub async fn register_agent(&self) -> Result<mpsc::UnboundedReceiver<AgentEvent>, Error> {
        let conn = self.connection().clone();
        let (events, receiver) = mpsc::unbounded_channel();
        let calls = MessageStream::from(&conn);
        let owner_rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, BUS_NAME)?
            .build();
        let mut owners = MessageStream::for_match_rule(owner_rule, &conn, Some(16)).await?;
        register(&conn).await?;

        let (c, e) = (conn.clone(), events.clone());
        tokio::spawn(async move {
            // BlueZ forgets its agents when it stops.
            while let Some(Ok(message)) = owners.next().await {
                if e.is_closed() {
                    break;
                }
                let new_owner = message.body().deserialize::<(String, String, String)>().map(|(_, _, n)| n);
                if new_owner.is_ok_and(|n| !n.is_empty()) {
                    let _ = register(&c).await;
                }
            }
        });
        tokio::spawn(serve(conn, calls, events));
        Ok(receiver)
    }
}

async fn register(conn: &Connection) -> Result<(), Error> {
    let path = OwnedObjectPath::try_from(AGENT_PATH).expect("a valid path");
    for (method, body) in [("RegisterAgent", Some(CAPABILITY)), ("RequestDefaultAgent", None)] {
        let result = match body {
            Some(capability) => {
                conn.call_method(Some(BUS_NAME), "/org/bluez", Some(AGENT_MANAGER), method, &(&path, capability)).await
            }
            None => conn.call_method(Some(BUS_NAME), "/org/bluez", Some(AGENT_MANAGER), method, &(&path,)).await,
        };
        match result.map_err(Error::from_dbus) {
            // Registered already (BlueZ kept it).
            Err(Error::Refused { kind, .. }) if kind == "AlreadyExists" => {}
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    Ok(())
}

/// For tools that introspect before calling (BlueZ doesn't).
const INTROSPECTION: &str = r#"<node>
  <interface name="org.bluez.Agent1">
    <method name="Release"/>
    <method name="RequestPinCode">
      <arg name="device" type="o" direction="in"/>
      <arg name="pincode" type="s" direction="out"/>
    </method>
    <method name="DisplayPinCode">
      <arg name="device" type="o" direction="in"/>
      <arg name="pincode" type="s" direction="in"/>
    </method>
    <method name="RequestPasskey">
      <arg name="device" type="o" direction="in"/>
      <arg name="passkey" type="u" direction="out"/>
    </method>
    <method name="DisplayPasskey">
      <arg name="device" type="o" direction="in"/>
      <arg name="passkey" type="u" direction="in"/>
      <arg name="entered" type="q" direction="in"/>
    </method>
    <method name="RequestConfirmation">
      <arg name="device" type="o" direction="in"/>
      <arg name="passkey" type="u" direction="in"/>
    </method>
    <method name="RequestAuthorization">
      <arg name="device" type="o" direction="in"/>
    </method>
    <method name="AuthorizeService">
      <arg name="device" type="o" direction="in"/>
      <arg name="uuid" type="s" direction="in"/>
    </method>
    <method name="Cancel"/>
  </interface>
</node>"#;

async fn serve(conn: Connection, mut calls: MessageStream, events: mpsc::UnboundedSender<AgentEvent>) {
    let dbus = DBusProxy::new(&conn).await.ok();
    while let Some(Ok(message)) = calls.next().await {
        if events.is_closed() {
            break;
        }
        let header = message.header();
        if header.message_type() != MessageType::MethodCall || header.path().map(|p| p.as_str()) != Some(AGENT_PATH) {
            continue;
        }
        if header.interface().map(|i| i.as_str()) == Some("org.freedesktop.DBus.Introspectable") {
            let _ = conn.reply(&header, &(INTROSPECTION,)).await;
            continue;
        }
        if header.interface().map(|i| i.as_str()) != Some(AGENT_IFACE) {
            let _ = conn.reply_error(&header, "org.freedesktop.DBus.Error.UnknownMethod", &("Unknown method",)).await;
            continue;
        }
        let (conn, events) = (conn.clone(), events.clone());
        // Only BlueZ may ask: anyone else could show a pairing dialog of their own making.
        let from_bluez = match &dbus {
            Some(dbus) => match dbus.get_name_owner(BUS_NAME.try_into().expect("a valid name")).await {
                Ok(owner) => header.sender().is_some_and(|s| s.as_str() == owner.as_str()),
                Err(_) => false,
            },
            None => false,
        };
        tokio::spawn(async move {
            let header = message.header();
            if !from_bluez {
                let _ = conn.reply_error(&header, "org.bluez.Error.Rejected", &("Only BlueZ may ask",)).await;
                return;
            }
            let _ = match handle(&message, &header, &events).await {
                Ok(Reply::Nothing) => conn.reply(&header, &()).await,
                Ok(Reply::Pin(pin)) => conn.reply(&header, &(pin,)).await,
                Ok(Reply::Passkey(passkey)) => conn.reply(&header, &(passkey,)).await,
                Err(name) => conn.reply_error(&header, format!("org.bluez.Error.{name}"), &(name,)).await,
            };
        });
    }
}

enum Reply {
    Nothing,
    Pin(String),
    Passkey(u32),
}

/// The reply, or the name of the `org.bluez.Error` to answer with.
async fn handle(
    message: &Message,
    header: &Header<'_>,
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Result<Reply, &'static str> {
    let body = message.body();
    let ask = |device: OwnedObjectPath, ask: Ask| async move {
        let (reply, answer) = oneshot::channel();
        let request = PairingRequest { device: device.to_string(), ask, reply };
        events.send(AgentEvent::Ask(request)).map_err(|_| "Rejected")?;
        Ok::<_, &'static str>(answer.await.unwrap_or(Answer::Reject))
    };
    match header.member().map(|m| m.as_str()) {
        Some("RequestPinCode") => {
            let (device,): (OwnedObjectPath,) = body.deserialize().map_err(|_| "Rejected")?;
            match ask(device, Ask::PinCode).await? {
                // BlueZ takes 1 to 16 characters.
                Answer::Pin(pin) if (1..=16).contains(&pin.len()) => Ok(Reply::Pin(pin)),
                answer => Err(refusal(&answer)),
            }
        }
        Some("RequestPasskey") => {
            let (device,): (OwnedObjectPath,) = body.deserialize().map_err(|_| "Rejected")?;
            match ask(device, Ask::Passkey).await? {
                Answer::Passkey(passkey) if passkey <= 999_999 => Ok(Reply::Passkey(passkey)),
                answer => Err(refusal(&answer)),
            }
        }
        Some("RequestConfirmation") => {
            let (device, passkey): (OwnedObjectPath, u32) = body.deserialize().map_err(|_| "Rejected")?;
            accepted(ask(device, Ask::Confirm(passkey)).await?)
        }
        Some("RequestAuthorization") => {
            let (device,): (OwnedObjectPath,) = body.deserialize().map_err(|_| "Rejected")?;
            accepted(ask(device, Ask::Authorize).await?)
        }
        Some("AuthorizeService") => {
            let (device, uuid): (OwnedObjectPath, String) = body.deserialize().map_err(|_| "Rejected")?;
            accepted(ask(device, Ask::Service(uuid.to_ascii_lowercase())).await?)
        }
        Some("DisplayPinCode") => {
            let (device, code): (OwnedObjectPath, String) = body.deserialize().map_err(|_| "Rejected")?;
            let _ = events.send(AgentEvent::Display { device: device.to_string(), code, entered: None });
            Ok(Reply::Nothing)
        }
        Some("DisplayPasskey") => {
            let (device, passkey, entered): (OwnedObjectPath, u32, u16) = body.deserialize().map_err(|_| "Rejected")?;
            let code = format!("{passkey:06}");
            let _ = events.send(AgentEvent::Display { device: device.to_string(), code, entered: Some(entered) });
            Ok(Reply::Nothing)
        }
        Some("Cancel") => {
            let _ = events.send(AgentEvent::Cancel);
            Ok(Reply::Nothing)
        }
        Some("Release") => Ok(Reply::Nothing),
        _ => Err("Rejected"),
    }
}

fn accepted(answer: Answer) -> Result<Reply, &'static str> {
    match answer {
        Answer::Accept => Ok(Reply::Nothing),
        answer => Err(refusal(&answer)),
    }
}

fn refusal(answer: &Answer) -> &'static str {
    match answer {
        Answer::Cancel => "Canceled",
        _ => "Rejected",
    }
}
