//! YAML parsing with an expansion budget.
//!
//! YAML aliases replay whole subtrees and scalars, so a few kilobytes can
//! expand into gigabytes before serde_yaml_ng's own repetition check trips.
//! Each node costs one unit and each string or key also costs its length, and
//! the budget is one unit per input byte. Real specs need far less: GitHub's
//! 10.3 MB YAML spec needs 0.61 units per byte and Stripe's 6.6 MB spec 0.54,
//! so the budget only trips when aliases replay content.

use std::{cell::Cell, fmt};

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::LoadError;

/// Parse YAML into JSON, failing once it expands past one unit per byte.
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
                LoadError::YamlAliasExpansion { limit: bytes.len() }
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
        self.charge_units(1)
    }

    /// A string costs its node plus its length: an alias replays the whole
    /// scalar.
    fn charge_string<E: de::Error>(self, value: &str) -> Result<(), E> {
        self.charge_units(1 + value.len())
    }

    fn charge_units<E: de::Error>(self, units: usize) -> Result<(), E> {
        match self.budget.remaining.get().checked_sub(units) {
            Some(remaining) => {
                self.budget.remaining.set(remaining);
                Ok(())
            }
            None => {
                self.budget.exceeded.set(true);
                Err(E::custom("YAML expansion budget exceeded"))
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

    // serde_yaml_ng hands integers past 64 bits to these; arbitrary_precision
    // keeps them exact, as the JSON path does.
    fn visit_i128<E: de::Error>(self, value: i128) -> Result<Value, E> {
        self.charge()?;
        big_integer(value.to_string())
    }

    fn visit_u128<E: de::Error>(self, value: u128) -> Result<Value, E> {
        self.charge()?;
        big_integer(value.to_string())
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        self.charge()?;
        Ok(Value::from(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.charge_string(value)?;
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        self.charge_string(&value)?;
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
            self.charge_string(&key)?;
            entries.insert(key, map.next_value_seed(self)?);
        }
        Ok(Value::Object(entries))
    }
}

fn big_integer<E: de::Error>(digits: String) -> Result<Value, E> {
    serde_json::from_str(&digits).map_err(E::custom)
}
