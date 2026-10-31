//! Connection passwords in the user's keyring, for the "for me only" storage: NetworkManager
//! keeps such secrets out of its own files and asks Lituus's secret agent for them instead.
//!
//! Two backends, tried in this order:
//! - the **Secret Service** (`org.freedesktop.secrets`): GNOME Keyring, KeePassXC, and KWallet 6
//!   when its Secret Service support is on. Items use nm-applet's attributes, so passwords saved
//!   by nm-applet or GNOME Settings are found, and the other way round.
//! - **KWallet**'s own D-Bus API (`kwalletd6`, then `kwalletd5`), for KDE setups without the
//!   Secret Service. Entries go in a "Lituus" folder of the network wallet.

use std::collections::HashMap;

use zbus::Connection;
use zbus::fdo::DBusProxy;

mod kwallet;
mod secret_service;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The user dismissed the unlock prompt, or the wallet wouldn't open.
    #[error("the keyring is locked")]
    Locked,
    #[error(transparent)]
    DBus(#[from] zbus::Error),
}

impl From<zbus::fdo::Error> for Error {
    fn from(error: zbus::fdo::Error) -> Self {
        Self::DBus(error.into())
    }
}

/// Which secret of which connection: NetworkManager's connection UUID, the setting the secret
/// belongs to (`802-11-wireless-security`) and its key (`psk`).
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub uuid: &'a str,
    pub setting: &'a str,
    pub key: &'a str,
}

#[derive(Debug)]
pub struct Keyring {
    backend: Backend,
}

#[derive(Debug)]
enum Backend {
    SecretService(secret_service::SecretService),
    KWallet(kwallet::KWallet),
}

impl Keyring {
    /// The keyring on the user's session bus, if there is one.
    pub async fn session() -> Option<Self> {
        Self::open(Connection::session().await.ok()?).await
    }

    /// The first keyring that runs, or can be started, on a given bus (tests use a private one).
    pub async fn open(conn: Connection) -> Option<Self> {
        let dbus = DBusProxy::new(&conn).await.ok()?;
        let mut names: Vec<String> = dbus.list_names().await.ok()?.into_iter().map(|n| n.to_string()).collect();
        names.extend(dbus.list_activatable_names().await.unwrap_or_default().into_iter().map(|n| n.to_string()));
        let has = |name: &str| names.iter().any(|n| n == name);
        if has(secret_service::BUS_NAME)
            && let Ok(service) = secret_service::SecretService::connect(conn.clone()).await
        {
            return Some(Self { backend: Backend::SecretService(service) });
        }
        for wallet in &kwallet::SERVICES {
            if has(wallet.bus_name) {
                return Some(Self { backend: Backend::KWallet(kwallet::KWallet::new(conn.clone(), wallet)) });
            }
        }
        None
    }

    /// What the settings page and the password dialog call it.
    pub fn name(&self) -> &'static str {
        match self.backend {
            Backend::SecretService(_) => "keyring",
            Backend::KWallet(_) => "KWallet",
        }
    }

    /// Saves a secret, replacing an older one for the same entry. `connection` is the
    /// connection's name, for the label the keyring app shows.
    pub async fn store(&self, entry: Entry<'_>, connection: &str, secret: &str) -> Result<(), Error> {
        match &self.backend {
            Backend::SecretService(s) => s.store(entry, connection, secret).await,
            Backend::KWallet(w) => w.store(entry, secret).await,
        }
    }

    /// Every saved secret of one setting of a connection, by key.
    pub async fn find(&self, uuid: &str, setting: &str) -> Result<HashMap<String, String>, Error> {
        match &self.backend {
            Backend::SecretService(s) => s.find(uuid, Some(setting)).await,
            Backend::KWallet(w) => w.find(uuid, Some(setting)).await,
        }
    }

    /// Deletes every secret of a connection (when it's forgotten).
    pub async fn delete(&self, uuid: &str) -> Result<(), Error> {
        match &self.backend {
            Backend::SecretService(s) => s.delete(uuid).await,
            Backend::KWallet(w) => w.delete(uuid).await,
        }
    }
}
