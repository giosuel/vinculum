//! Reading the properties of one interface leniently: typed lookups that give `None` for a
//! property that's missing or has an unexpected type.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

#[derive(Default)]
pub(crate) struct Props(HashMap<String, OwnedValue>);

impl From<HashMap<String, OwnedValue>> for Props {
    fn from(map: HashMap<String, OwnedValue>) -> Self {
        Self(map)
    }
}

impl Props {
    fn value(&self, name: &str) -> Option<&Value<'static>> {
        self.0.get(name).map(|v| &**v)
    }

    pub(crate) fn u32(&self, name: &str) -> Option<u32> {
        self.value(name).and_then(|v| u32::try_from(v).ok())
    }

    pub(crate) fn i16(&self, name: &str) -> Option<i16> {
        self.value(name).and_then(|v| i16::try_from(v).ok())
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

    pub(crate) fn path(&self, name: &str) -> Option<String> {
        match self.value(name)? {
            Value::ObjectPath(path) => Some(path.as_str().to_owned()),
            _ => None,
        }
    }

    pub(crate) fn strings(&self, name: &str) -> Vec<String> {
        match self.value(name) {
            Some(Value::Array(array)) => {
                array.iter().filter_map(|v| <&str>::try_from(v).ok()).map(str::to_owned).collect()
            }
            _ => Vec::new(),
        }
    }
}
