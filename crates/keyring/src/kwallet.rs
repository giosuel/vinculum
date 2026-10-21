//! KWallet's own D-Bus API (`org.kde.KWallet`), for KDE setups where the Secret Service isn't
//! switched on. Entries are passwords named `{uuid};{setting};{key}` in a "Vinculum" folder of
//! the network wallet. Opening the wallet may show KWallet's unlock dialog.

use std::collections::HashMap;

use zbus::Connection;

use crate::{Entry, Error};

pub(crate) struct Service {
    pub(crate) bus_name: &'static str,
    path: &'static str,
}

/// kwalletd6 (Plasma 6), then kwalletd5.
pub(crate) const SERVICES: [Service; 2] = [
    Service { bus_name: "org.kde.kwalletd6", path: "/modules/kwalletd6" },
    Service { bus_name: "org.kde.kwalletd5", path: "/modules/kwalletd5" },
];

const IFACE: &str = "org.kde.KWallet";
const FOLDER: &str = "Vinculum";
const APP: &str = "Vinculum";

#[derive(Debug)]
pub(crate) struct KWallet {
    conn: Connection,
    bus_name: &'static str,
    path: &'static str,
}

impl KWallet {
    pub(crate) fn new(conn: Connection, service: &Service) -> Self {
        Self { conn, bus_name: service.bus_name, path: service.path }
    }

    pub(crate) async fn store(&self, entry: Entry<'_>, secret: &str) -> Result<(), Error> {
        let handle = self.open().await?;
        let has: bool = self.call("hasFolder", &(handle, FOLDER, APP)).await?;
        if !has {
            let _: bool = self.call("createFolder", &(handle, FOLDER, APP)).await?;
        }
        let key = format!("{};{};{}", entry.uuid, entry.setting, entry.key);
        let status: i32 = self.call("writePassword", &(handle, FOLDER, key, secret, APP)).await?;
        if status == 0 { Ok(()) } else { Err(Error::Locked) }
    }

    pub(crate) async fn find(&self, uuid: &str, setting: Option<&str>) -> Result<HashMap<String, String>, Error> {
        let handle = self.open().await?;
        let prefix = match setting {
            Some(setting) => format!("{uuid};{setting};"),
            None => format!("{uuid};"),
        };
        let mut found = HashMap::new();
        for name in self.entries(handle, &prefix).await? {
            let value: String = self.call("readPassword", &(handle, FOLDER, &name, APP)).await?;
            let key = name.rsplit(';').next().unwrap_or_default().to_owned();
            found.insert(key, value);
        }
        Ok(found)
    }

    pub(crate) async fn delete(&self, uuid: &str) -> Result<(), Error> {
        let handle = self.open().await?;
        for name in self.entries(handle, &format!("{uuid};")).await? {
            let _: i32 = self.call("removeEntry", &(handle, FOLDER, &name, APP)).await?;
        }
        Ok(())
    }

    async fn entries(&self, handle: i32, prefix: &str) -> Result<Vec<String>, Error> {
        let has: bool = self.call("hasFolder", &(handle, FOLDER, APP)).await?;
        if !has {
            return Ok(Vec::new());
        }
        let names: Vec<String> = self.call("entryList", &(handle, FOLDER, APP)).await?;
        Ok(names.into_iter().filter(|n| n.starts_with(prefix)).collect())
    }

    /// A handle to the network wallet; KWallet gives the same one while the wallet stays open.
    async fn open(&self) -> Result<i32, Error> {
        let wallet: String = self.call("networkWallet", &()).await?;
        let handle: i32 = self.call("open", &(wallet, 0i64, APP)).await?;
        if handle < 0 { Err(Error::Locked) } else { Ok(handle) }
    }

    async fn call<B, R>(&self, method: &str, body: &B) -> zbus::Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let reply = self.conn.call_method(Some(self.bus_name), self.path, Some(IFACE), method, body).await?;
        reply.body().deserialize()
    }
}
