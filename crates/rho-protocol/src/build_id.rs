//! Which wire a build speaks, beyond its protocol version.

use std::{fmt, sync::LazyLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The wire schema a build speaks (#1776). While the protocol version stays
/// at 0.1 until the beta, two builds of one version can still speak
/// different wires, and only this tells them apart at the handshake.
///
/// It changes when the wire schema, `schema/protocol.json`, changes in
/// anything but its documentation and layout. It has no order: two ids say
/// only whether the builds differ, never which one is newer. Opaque to
/// clients: compare it whole.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct BuildId(String);

impl BuildId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id of the wire schema this crate was built with, the committed
    /// `schema/protocol.json`.
    pub fn of_this_crate() -> Self {
        static OF_THIS_CRATE: LazyLock<BuildId> = LazyLock::new(|| {
            let schema = serde_json::from_str(include_str!("../schema/protocol.json"))
                .expect("schema/protocol.json is JSON");
            BuildId::of_schema(schema)
        });
        OF_THIS_CRATE.clone()
    }

    /// The id of `schema`: the 64-bit FNV-1a hash of its canonical form
    /// without documentation, as 16 lowercase hex digits. A checkout's line
    /// endings and a reworded doc comment change no wire, so they change no
    /// id.
    fn of_schema(schema: Value) -> Self {
        let mut wire = String::new();
        write_wire(&schema, &mut wire);
        Self(format!("{:016x}", fnv1a(wire.as_bytes())))
    }
}

impl fmt::Display for BuildId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Appends `schema` to `wire` in one canonical form: compact, each
/// object's members in name order, whatever order `serde_json` keeps them
/// in for this build. It leaves out every `description` that is text, which
/// is a type's or field's rustdoc; a wire field named `description` has a
/// schema for its value, not text, and stays.
fn write_wire(schema: &Value, wire: &mut String) {
    match schema {
        Value::Object(members) => {
            let mut members: Vec<_> = members
                .iter()
                .filter(|(name, value)| !(*name == "description" && value.is_string()))
                .collect();
            members.sort_by_key(|(name, _)| *name);
            wire.push('{');
            for (name, value) in members {
                wire.push_str(&Value::from(name.as_str()).to_string());
                wire.push(':');
                write_wire(value, wire);
                wire.push(',');
            }
            wire.push('}');
        }
        Value::Array(items) => {
            wire.push('[');
            for item in items {
                write_wire(item, wire);
                wire.push(',');
            }
            wire.push(']');
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            wire.push_str(&schema.to_string());
        }
    }
}

/// 64-bit FNV-1a. Written out rather than `std::hash::DefaultHasher`, whose
/// output may change between compilers: two builds must agree on it.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
#[path = "build_id_tests.rs"]
mod tests;
