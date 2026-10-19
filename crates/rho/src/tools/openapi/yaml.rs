//! YAML parsing with a node budget.
//!
//! YAML aliases replay whole subtrees, so a few kilobytes can expand into
//! billions of nodes before serde_yaml_ng's own repetition check trips. The
//! budget allows one node per input byte: an alias-free document needs at
//! least two bytes per node (`0,`), and GitHub's spec averages 28. Aliases can
//! therefore at most double the node count a spec of the same size could
//! reach without them.

use std::{cell::Cell, fmt};

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::LoadError;

/// Parse YAML into JSON, failing once it expands past one node per byte.
pub(super) fn parse(bytes: &[u8]) -> Result<Value, LoadError> {
    let budget = Budget {
        remaining: Cell::new(bytes.len()),
        exceeded: Cell::new(false),
    };
    let deserializer = serde_yaml_ng::Deserializer::from_slice(bytes);
    Node { budget: &budget }
        .deserialize(deserializer)
        .map_err(|error| {
            if budget.exceeded.get() {
                LoadError::YamlTooManyNodes { limit: bytes.len() }
            } else {
                LoadError::Yaml(error)
            }
        })
}

struct Budget {
    remaining: Cell<usize>,
    exceeded: Cell<bool>,
}

/// Builds one JSON value, charging each node to the shared budget.
#[derive(Clone, Copy)]
struct Node<'a> {
    budget: &'a Budget,
}

impl Node<'_> {
    fn charge<E: de::Error>(self) -> Result<(), E> {
        match self.budget.remaining.get().checked_sub(1) {
            Some(remaining) => {
                self.budget.remaining.set(remaining);
                Ok(())
            }
            None => {
                self.budget.exceeded.set(true);
                Err(E::custom("YAML node budget exceeded"))
            }
        }
    }
}

impl<'de> DeserializeSeed<'de> for Node<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Node<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a YAML value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::from(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::from(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::from(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        self.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        self.charge()?;
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(self)? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        self.charge()?;
        let mut entries = Map::new();
        // YAML keys such as `200` arrive as strings, as with serde_json::Value.
        while let Some(key) = map.next_key::<String>()? {
            self.charge()?;
            entries.insert(key, map.next_value_seed(self)?);
        }
        Ok(Value::Object(entries))
    }
}
