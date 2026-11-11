//! Which wire a build speaks, beyond its protocol version.

use std::{fmt, sync::LazyLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The wire schema a build speaks (#1776). While the protocol version stays
/// at 0.1 until the beta, two builds of one version can still speak
/// different wires, and only this tells them apart at the handshake.
///
/// It changes exactly when the wire schema, `schema/protocol.json`, changes.
/// It has no order: two ids say only whether the builds differ, never which
/// one is newer. Opaque to clients: compare it whole.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct BuildId(String);

impl BuildId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id of the wire schema this crate was built with: the 64-bit
    /// FNV-1a hash of the committed `schema/protocol.json`, as 16 lowercase
    /// hex digits.
    pub fn of_this_crate() -> Self {
        static OF_THIS_CRATE: LazyLock<BuildId> = LazyLock::new(|| {
            BuildId(format!(
                "{:016x}",
                fnv1a(include_str!("../schema/protocol.json").as_bytes())
            ))
        });
        OF_THIS_CRATE.clone()
    }
}

impl fmt::Display for BuildId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
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
