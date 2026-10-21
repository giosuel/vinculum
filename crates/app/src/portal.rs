//! The desktop's own file dialogs, through the XDG desktop portal (`org.freedesktop.portal
//! .FileChooser`), so choosing a certificate or a VPN file looks like every other app. On
//! Hyprland the dialog comes from xdg-desktop-portal-gtk (or -kde, -gnome).

use std::collections::HashMap;
use std::path::PathBuf;

use futures_util::StreamExt;
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.FileChooser";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no file dialog: xdg-desktop-portal (with -gtk, -kde or -gnome) isn't running")]
    NoPortal,
    #[error(transparent)]
    DBus(#[from] zbus::Error),
}

/// A filter: its name, and glob patterns such as `*.conf`.
pub type Filter<'a> = (&'a str, &'a [&'a str]);

/// Asks for a file to open. `None` when the user cancelled.
pub async fn open_file(title: &str, filters: &[Filter<'_>]) -> Result<Option<PathBuf>, Error> {
    let filters: Vec<(String, Vec<(u32, String)>)> = filters
        .iter()
        .map(|(name, globs)| (name.to_string(), globs.iter().map(|g| (0u32, g.to_string())).collect()))
        .collect();
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("filters", Value::from(filters));
    options.insert("modal", Value::from(true));
    request("OpenFile", title, options).await
}

/// Asks where to save a file, suggesting `name`.
pub async fn save_file(title: &str, name: &str) -> Result<Option<PathBuf>, Error> {
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("current_name", Value::from(name));
    options.insert("modal", Value::from(true));
    request("SaveFile", title, options).await
}

async fn request(method: &str, title: &str, mut options: HashMap<&str, Value<'_>>) -> Result<Option<PathBuf>, Error> {
    let conn = Connection::session().await.map_err(|_| Error::NoPortal)?;
    // The request object's path is known before the call, so the answer can't be missed.
    let token = format!(
        "vinculum{}",
        std::process::id() ^ (std::time::SystemTime::now().elapsed().map_or(0, |d| d.subsec_nanos()))
    );
    let sender = conn.unique_name().map(|n| n.trim_start_matches(':').replace('.', "_")).unwrap_or_default();
    let handle = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    options.insert("handle_token", Value::from(token));
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.freedesktop.portal.Request")?
        .member("Response")?
        .path(handle)?
        .build();
    let mut responses = MessageStream::for_match_rule(rule, &conn, Some(4)).await?;
    let reply = conn.call_method(Some(DESTINATION), PATH, Some(IFACE), method, &("", title, options)).await;
    match reply {
        Ok(reply) => {
            let _: OwnedObjectPath = reply.body().deserialize()?;
        }
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown"
                || name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod"
                || name.as_str() == "org.freedesktop.DBus.Error.UnknownInterface" =>
        {
            return Err(Error::NoPortal);
        }
        Err(e) => return Err(e.into()),
    }
    let Some(message) = responses.next().await else { return Ok(None) };
    let (response, results): (u32, HashMap<String, OwnedValue>) = message?.body().deserialize()?;
    if response != 0 {
        return Ok(None);
    }
    let uris = results.get("uris").and_then(|v| <Vec<String>>::try_from(v.clone()).ok()).unwrap_or_default();
    Ok(uris.first().and_then(|uri| uri.strip_prefix("file://")).map(|p| PathBuf::from(unescape(p))))
}

/// `file://` URIs escape spaces and the like as `%20`.
fn unescape(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn uris_become_paths() {
        assert_eq!(super::unescape("/home/g/My%20VPN/uni%C3%A9.ovpn"), "/home/g/My VPN/unié.ovpn");
        assert_eq!(super::unescape("/plain/path.conf"), "/plain/path.conf");
    }
}
