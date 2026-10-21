//! The freedesktop Secret Service, with a "plain" session: secrets travel unencrypted, but only
//! over the user's own session bus, which every process of the user can read from anyway.

use std::collections::HashMap;

use futures_util::StreamExt;
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use crate::{Entry, Error};

pub(crate) const BUS_NAME: &str = "org.freedesktop.secrets";
const SERVICE: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
/// nm-applet's schema; its attributes are `connection-uuid`, `setting-name` and `setting-key`.
const SCHEMA: &str = "org.freedesktop.NetworkManager.Connection";

/// (session, parameters, value, content type)
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[derive(Debug)]
pub(crate) struct SecretService {
    conn: Connection,
    session: OwnedObjectPath,
}

impl SecretService {
    pub(crate) async fn connect(conn: Connection) -> Result<Self, Error> {
        let (_, session): (OwnedValue, OwnedObjectPath) =
            call(&conn, SERVICE, SERVICE_IFACE, "OpenSession", &("plain", Value::from(""))).await?;
        Ok(Self { conn, session })
    }

    pub(crate) async fn store(&self, entry: Entry<'_>, connection: &str, secret: &str) -> Result<(), Error> {
        let collection = self.default_collection().await?;
        self.unlock(vec![collection.clone()]).await?;
        let attributes = HashMap::from([
            ("xdg:schema", SCHEMA),
            ("connection-uuid", entry.uuid),
            ("setting-name", entry.setting),
            ("setting-key", entry.key),
        ]);
        let label = format!("Network secret for {connection}/{}/{}", entry.setting, entry.key);
        let properties: HashMap<&str, Value> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from(label)),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes)),
        ]);
        let secret: Secret = (self.session.clone(), Vec::new(), secret.as_bytes().to_vec(), "text/plain".into());
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = call(
            &self.conn,
            collection.as_str(),
            "org.freedesktop.Secret.Collection",
            "CreateItem",
            &(properties, secret, true),
        )
        .await?;
        self.prompt(&prompt).await.map(drop)
    }

    /// Secrets by `setting-key`, for one setting or (with `None`) all of them.
    pub(crate) async fn find(&self, uuid: &str, setting: Option<&str>) -> Result<HashMap<String, String>, Error> {
        let items = self.search(uuid, setting).await?;
        if items.is_empty() {
            return Ok(HashMap::new());
        }
        let secrets: HashMap<OwnedObjectPath, Secret> =
            call(&self.conn, SERVICE, SERVICE_IFACE, "GetSecrets", &(&items, &self.session)).await?;
        let mut found = HashMap::new();
        for (item, (_, _, value, _)) in secrets {
            let attributes: HashMap<String, String> = self.property(&item, "Attributes").await?;
            if let (Some(key), Ok(value)) = (attributes.get("setting-key"), String::from_utf8(value)) {
                found.insert(key.clone(), value);
            }
        }
        Ok(found)
    }

    pub(crate) async fn delete(&self, uuid: &str) -> Result<(), Error> {
        for item in self.search(uuid, None).await? {
            let prompt: OwnedObjectPath =
                call(&self.conn, item.as_str(), "org.freedesktop.Secret.Item", "Delete", &()).await?;
            self.prompt(&prompt).await?;
        }
        Ok(())
    }

    /// The matching items, unlocking the locked ones first.
    async fn search(&self, uuid: &str, setting: Option<&str>) -> Result<Vec<OwnedObjectPath>, Error> {
        let mut attributes = HashMap::from([("connection-uuid", uuid)]);
        if let Some(setting) = setting {
            attributes.insert("setting-name", setting);
        }
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
            call(&self.conn, SERVICE, SERVICE_IFACE, "SearchItems", &(attributes,)).await?;
        if !locked.is_empty() {
            unlocked.extend(self.unlock(locked).await?);
        }
        Ok(unlocked)
    }

    /// The collection new items go in: the "default" alias, made if there's none yet.
    async fn default_collection(&self) -> Result<OwnedObjectPath, Error> {
        let path: OwnedObjectPath = call(&self.conn, SERVICE, SERVICE_IFACE, "ReadAlias", &("default",)).await?;
        if path.as_str() != "/" {
            return Ok(path);
        }
        let properties: HashMap<&str, Value> =
            HashMap::from([("org.freedesktop.Secret.Collection.Label", "Login".into())]);
        let (path, prompt): (OwnedObjectPath, OwnedObjectPath) =
            call(&self.conn, SERVICE, SERVICE_IFACE, "CreateCollection", &(properties, "default")).await?;
        if path.as_str() != "/" {
            return Ok(path);
        }
        let result = self.prompt(&prompt).await?.ok_or(Error::Locked)?;
        OwnedObjectPath::try_from(result).map_err(|_| Error::Locked)
    }

    async fn unlock(&self, objects: Vec<OwnedObjectPath>) -> Result<Vec<OwnedObjectPath>, Error> {
        let (mut unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
            call(&self.conn, SERVICE, SERVICE_IFACE, "Unlock", &(&objects,)).await?;
        if let Some(result) = self.prompt(&prompt).await? {
            unlocked.extend(<Vec<OwnedObjectPath>>::try_from(result).unwrap_or_default());
        }
        if unlocked.is_empty() && !objects.is_empty() {
            return Err(Error::Locked);
        }
        Ok(unlocked)
    }

    /// Shows a prompt (the keyring's unlock dialog) and waits for it. `/` means there is none.
    /// Gives the prompt's result, or [`Error::Locked`] if the user dismissed it.
    async fn prompt(&self, prompt: &ObjectPath<'_>) -> Result<Option<OwnedValue>, Error> {
        if prompt.as_str() == "/" {
            return Ok(None);
        }
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface("org.freedesktop.Secret.Prompt")?
            .member("Completed")?
            .path(prompt.clone())?
            .build();
        let mut completed = MessageStream::for_match_rule(rule, &self.conn, Some(4)).await?;
        call::<_, ()>(&self.conn, prompt.as_str(), "org.freedesktop.Secret.Prompt", "Prompt", &("",)).await?;
        let Some(message) = completed.next().await else { return Err(Error::Locked) };
        let (dismissed, result): (bool, OwnedValue) = message?.body().deserialize()?;
        if dismissed { Err(Error::Locked) } else { Ok(Some(result)) }
    }

    async fn property<T>(&self, path: &str, name: &str) -> Result<T, Error>
    where
        T: TryFrom<OwnedValue>,
    {
        let value: OwnedValue =
            call(&self.conn, path, "org.freedesktop.DBus.Properties", "Get", &("org.freedesktop.Secret.Item", name))
                .await?;
        T::try_from(value).map_err(|_| Error::DBus(zbus::Error::InvalidReply))
    }
}

async fn call<B, R>(conn: &Connection, path: &str, interface: &str, method: &str, body: &B) -> zbus::Result<R>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
{
    let reply = conn.call_method(Some(BUS_NAME), path, Some(interface), method, body).await?;
    reply.body().deserialize()
}
