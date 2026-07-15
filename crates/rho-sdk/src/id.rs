use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Error returned when an opaque SDK identifier is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidId;

impl fmt::Display for InvalidId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("identifier must not be empty")
    }
}

impl std::error::Error for InvalidId {}

macro_rules! opaque_id {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a random identifier suitable for a new SDK object.
            pub fn new() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            /// Creates an identifier from a persisted or host-provided value.
            pub fn from_string(value: impl Into<String>) -> Result<Self, InvalidId> {
                let value = value.into();
                if value.is_empty() {
                    Err(InvalidId)
                } else {
                    Ok(Self(value))
                }
            }

            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consumes the identifier and returns its string representation.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = InvalidId;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::from_string(value)
            }
        }
    };
}

opaque_id!(SessionId, "Stable identity for an SDK session.");
opaque_id!(RunId, "Stable identity for one run within a session.");
opaque_id!(
    ToolCallId,
    "Stable identity for a provider-requested tool call."
);

/// Monotonically increasing revision of a session snapshot.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// The initial session revision.
    pub const INITIAL: Self = Self(0);

    /// Creates a revision from its stored numeric value.
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric revision.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the next revision, or `None` if the revision space is exhausted.
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
#[path = "id_tests.rs"]
mod tests;
