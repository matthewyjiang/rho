use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{WorkflowError, WorkflowResult};

pub(crate) const PORTABLE_NAME_GRAMMAR: &str =
    "1 to 63 ASCII bytes: a-z first, then a-z, 0-9, '_' or '-'";
const MAX_PORTABLE_NAME_BYTES: usize = 63;

fn validate_portable(kind: &'static str, value: &str) -> WorkflowResult<()> {
    let valid = !value.is_empty()
        && value.len() <= MAX_PORTABLE_NAME_BYTES
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
        });
    if valid {
        Ok(())
    } else {
        Err(WorkflowError::InvalidId {
            kind,
            value: value.to_owned(),
            grammar: PORTABLE_NAME_GRAMMAR,
        })
    }
}

macro_rules! portable_id {
    ($name:ident, $kind:literal) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub(crate) struct $name(String);

        impl $name {
            pub(crate) fn new(value: impl Into<String>) -> WorkflowResult<Self> {
                let value = value.into();
                validate_portable($kind, &value)?;
                Ok(Self(value))
            }

            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = WorkflowError;
            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

portable_id!(WorkflowName, "workflow name");
portable_id!(NodeId, "node ID");
portable_id!(InputName, "input name");

macro_rules! uuid_id {
    ($name:ident, $kind:literal) => {
        #[derive(
            Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub(crate) struct $name(Uuid);

        impl $name {
            pub(crate) fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl FromStr for $name {
            type Err = WorkflowError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value)
                    .map(Self)
                    .map_err(|_| WorkflowError::InvalidId {
                        kind: $kind,
                        value: value.to_owned(),
                        grammar: "a canonical UUID",
                    })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id!(PlanId, "plan ID");
uuid_id!(RunId, "run ID");

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub(crate) struct AttemptNumber(u32);

impl TryFrom<u32> for AttemptNumber {
    type Error = WorkflowError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AttemptNumber> for u32 {
    fn from(value: AttemptNumber) -> Self {
        value.0
    }
}

impl AttemptNumber {
    pub(crate) fn new(value: u32) -> WorkflowResult<Self> {
        if value == 0 {
            return Err(WorkflowError::InvalidId {
                kind: "attempt number",
                value: value.to_string(),
                grammar: "a non-zero u32",
            });
        }
        Ok(Self(value))
    }

    pub(crate) fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for AttemptNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
