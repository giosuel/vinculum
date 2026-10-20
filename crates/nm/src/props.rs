//! Reading D-Bus properties leniently: `GetAll` once per object, then typed lookups that give
//! `None` (or a default) for a property that's missing or has an unexpected type.

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{Connection, names::InterfaceName};

pub(crate) struct Props(HashMap<String, OwnedValue>);

impl Props {
    pub(crate) async fn get_all(conn: &Connection, path: &str, interface: &str) -> zbus::Result<Self> {
        let interface = InterfaceName::try_from(interface)?;
        let reply = conn
            .call_method(Some(crate::BUS_NAME), path, Some("org.freedesktop.DBus.Properties"), "GetAll", &interface)
            .await?;
        Ok(Self(reply.body().deserialize()?))
    }

    fn value(&self, name: &str) -> Option<&Value<'static>> {
        self.0.get(name).map(|v| &**v)
    }

    pub(crate) fn u32(&self, name: &str) -> Option<u32> {
        self.value(name).and_then(|v| u32::try_from(v).ok())
    }

    pub(crate) fn i64(&self, name: &str) -> Option<i64> {
        self.value(name).and_then(|v| i64::try_from(v).ok())
    }

    pub(crate) fn u8(&self, name: &str) -> Option<u8> {
        self.value(name).and_then(|v| u8::try_from(v).ok())
    }

    pub(crate) fn bool(&self, name: &str) -> Option<bool> {
        self.value(name).and_then(|v| bool::try_from(v).ok())
    }

    pub(crate) fn string(&self, name: &str) -> Option<String> {
        self.value(name).and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned)
    }

    /// An object path, or `None` for NetworkManager's "no object" path `/`.
    pub(crate) fn path(&self, name: &str) -> Option<String> {
        self.value(name).and_then(object_path)
    }

    pub(crate) fn paths(&self, name: &str) -> Vec<String> {
        match self.value(name) {
            Some(Value::Array(array)) => array.iter().filter_map(object_path).collect(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn bytes(&self, name: &str) -> Vec<u8> {
        self.value(name).map(bytes).unwrap_or_default()
    }

    /// An `aa{sv}` property, such as an IP config's `AddressData`.
    pub(crate) fn dicts(&self, name: &str) -> Vec<HashMap<String, Value<'static>>> {
        match self.value(name) {
            Some(Value::Array(array)) => array.iter().filter_map(dict).collect(),
            _ => Vec::new(),
        }
    }
}

fn object_path(value: &Value<'_>) -> Option<String> {
    let path = match value {
        Value::ObjectPath(path) => path.as_str().to_owned(),
        Value::Str(s) => ObjectPath::try_from(s.as_str()).ok()?.as_str().to_owned(),
        _ => return None,
    };
    (path != "/").then_some(path)
}

/// An `ay` value.
pub(crate) fn bytes(value: &Value<'_>) -> Vec<u8> {
    match value {
        Value::Array(array) => array.iter().filter_map(|b| u8::try_from(b).ok()).collect(),
        _ => Vec::new(),
    }
}

fn dict(value: &Value<'_>) -> Option<HashMap<String, Value<'static>>> {
    let Value::Dict(dict) = value else { return None };
    let mut out = HashMap::new();
    for (k, v) in dict.iter() {
        let key = <&str>::try_from(k).ok()?.to_owned();
        let v = match v {
            Value::Value(inner) => inner.try_to_owned().ok()?.into(),
            other => other.try_to_owned().ok()?.into(),
        };
        out.insert(key, v);
    }
    Some(out)
}
