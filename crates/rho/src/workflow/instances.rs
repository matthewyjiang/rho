use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use super::{NodeId, WorkflowError, WorkflowResult};

const SCOPE_SEPARATOR: char = '.';

/// Runtime scope identity. Ordinal zero is reserved for the root invocation.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct ScopeInstanceId(u64);

impl ScopeInstanceId {
    pub(crate) const ROOT: Self = Self::new(0);
    pub(crate) const fn new(ordinal: u64) -> Self {
        Self(ordinal)
    }
}

impl fmt::Display for ScopeInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}", self.0)
    }
}

impl FromStr for ScopeInstanceId {
    type Err = WorkflowError;
    fn from_str(value: &str) -> WorkflowResult<Self> {
        let invalid = || WorkflowError::InvalidId {
            kind: "scope instance ID",
            value: value.to_owned(),
            grammar: "s followed by a canonical unsigned decimal u64",
        };
        let ordinal = value
            .strip_prefix('s')
            .ok_or_else(invalid)?
            .parse::<u64>()
            .map_err(|_| invalid())?;
        let id = Self(ordinal);
        if id.to_string() != value {
            return Err(invalid());
        }
        Ok(id)
    }
}

/// A task invocation, distinct from its scope-local definition identity.
///
/// Root-scope instances use the bare definition name (`review`) as their string
/// form, which keeps CLI JSONL, hook labels, status, and artifact paths identical
/// to single-graph releases. Other scopes use `s<ordinal>.<definition>`; node IDs
/// cannot contain `.`, so the two forms never collide.
// NEXT_MAJOR(rho-coding-agent): qualify root task instances as `s0.<definition>` like other scopes and bump the workflow JSONL wire version.
#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct TaskInstanceId {
    scope: ScopeInstanceId,
    definition: NodeId,
}

impl TaskInstanceId {
    pub(crate) fn new(scope: ScopeInstanceId, definition: NodeId) -> Self {
        Self { scope, definition }
    }
    pub(crate) fn root(definition: NodeId) -> Self {
        Self::new(ScopeInstanceId::ROOT, definition)
    }
    pub(crate) fn scope(&self) -> ScopeInstanceId {
        self.scope
    }
    pub(crate) fn definition(&self) -> &NodeId {
        &self.definition
    }
}

impl fmt::Display for TaskInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.scope == ScopeInstanceId::ROOT {
            f.write_str(self.definition.as_str())
        } else {
            write!(
                f,
                "{}{SCOPE_SEPARATOR}{}",
                self.scope,
                self.definition.as_str()
            )
        }
    }
}

impl FromStr for TaskInstanceId {
    type Err = WorkflowError;
    fn from_str(value: &str) -> WorkflowResult<Self> {
        let Some((scope, definition)) = value.split_once(SCOPE_SEPARATOR) else {
            return Ok(Self::root(NodeId::new(value)?));
        };
        let scope: ScopeInstanceId = scope.parse()?;
        if scope == ScopeInstanceId::ROOT {
            return Err(WorkflowError::InvalidId {
                kind: "task instance ID",
                value: value.to_owned(),
                grammar: "a bare node ID for root tasks, or s<ordinal>.<node ID> for other scopes",
            });
        }
        Ok(Self::new(scope, NodeId::new(definition)?))
    }
}

macro_rules! string_identity {
    ($id:ty) => {
        impl TryFrom<String> for $id {
            type Error = WorkflowError;
            fn try_from(value: String) -> WorkflowResult<Self> {
                value.parse()
            }
        }
        impl From<$id> for String {
            fn from(value: $id) -> Self {
                value.to_string()
            }
        }
    };
}
string_identity!(ScopeInstanceId);
string_identity!(TaskInstanceId);
