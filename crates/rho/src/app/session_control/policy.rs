//! What a caller of the session-control core may do, and the budgets that
//! bound it. Every refusal names what it enforces: a missing scope names the
//! action, a workspace outside the grant names the allowed list, and a cap
//! names its budget, its limit, and the value asked for.

use std::{
    fmt,
    path::{Path, PathBuf},
};

pub(crate) use crate::child_env::SESSION_DEPTH_ENV;

/// The deepest a session the core starts or prompts may run by default: a
/// session at depth 3 may, and asking for depth 4 is refused.
//
// Tripwire. Depth 1 is a session an outside agent or a person's session
// starts, depth 2 one that session hands work to; no known flow goes
// further. One spare level, then a recursion stops within a few starts.
pub(crate) const DEFAULT_MAX_DEPTH: u32 = 3;

/// How many sessions one caller may have running by default.
//
// Matches `behavior.agent_concurrency`'s default, the agent runs the daemon
// already runs at once (`config::DEFAULT_AGENT_CONCURRENCY`).
pub(crate) const DEFAULT_MAX_SESSIONS: u32 = crate::config::DEFAULT_AGENT_CONCURRENCY as u32;

/// One session-control action. Each maps one to one onto the protocol:
/// `rho/job/list`, an attach's replay, `rho/session/new` and `prompt`,
/// `steer`, `cancel`, and an attach followed until the turn ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Action {
    List,
    Read,
    New,
    Prompt,
    Steer,
    Cancel,
    Wait,
}

impl Action {
    /// Every action, in the order adapters present them.
    pub(crate) const ALL: [Self; 7] = [
        Self::List,
        Self::Read,
        Self::New,
        Self::Prompt,
        Self::Steer,
        Self::Cancel,
        Self::Wait,
    ];

    /// The action's name, as adapters expose it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Read => "read",
            Self::New => "new",
            Self::Prompt => "prompt",
            Self::Steer => "steer",
            Self::Cancel => "cancel",
            Self::Wait => "wait",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == name)
    }

    /// The access the action needs: watching, or acting on a session.
    pub(crate) fn needs(self) -> Access {
        match self {
            Self::List | Self::Read | Self::Wait => Access::Read,
            Self::New | Self::Prompt | Self::Steer | Self::Cancel => Access::Write,
        }
    }
}

/// What a grant lets its holder do to sessions.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Access {
    /// List, read, and wait for sessions.
    Read,
    /// Also start, prompt, steer, and cancel them.
    Write,
}

impl Access {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// The workspaces a grant reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Workspaces {
    Any,
    /// These directories and everything under them, as canonical paths.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "`rho mcp serve --http` tokens hold grants (#1672)"
        )
    )]
    Only(Vec<PathBuf>),
}

impl Workspaces {
    /// Whether `workspace`, a canonical path, is one the grant reaches.
    pub(crate) fn allows(&self, workspace: &Path) -> bool {
        match self {
            Self::Any => true,
            Self::Only(allowed) => allowed.iter().any(|root| workspace.starts_with(root)),
        }
    }
}

/// Who holds a grant, as refusals and the per-holder session count name it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Holder {
    /// The process's own caller, as on stdio: whoever started it.
    Local,
    /// The holder of an MCP server token, by its name.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "`rho mcp serve --http` tokens hold grants (#1672)"
        )
    )]
    Token(String),
}

impl fmt::Display for Holder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => formatter.write_str("this server's caller"),
            Self::Token(name) => write!(formatter, "token {name:?}"),
        }
    }
}

/// What one caller may do: whose it is, its access, and its workspaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Grant {
    pub(crate) holder: Holder,
    pub(crate) access: Access,
    pub(crate) workspaces: Workspaces,
}

impl Grant {
    /// Everything, for the local caller.
    pub(crate) fn local() -> Self {
        Self {
            holder: Holder::Local,
            access: Access::Write,
            workspaces: Workspaces::Any,
        }
    }

    /// Refuses `action` unless the grant's access covers it.
    pub(crate) fn permit(&self, action: Action) -> Result<(), Refusal> {
        if self.access >= action.needs() {
            return Ok(());
        }
        Err(Refusal::Access {
            holder: self.holder.clone(),
            action,
            has: self.access,
        })
    }

    /// Refuses a session in `workspace`, a canonical path, unless the
    /// grant reaches it.
    pub(crate) fn reach(&self, workspace: &Path) -> Result<(), Refusal> {
        match &self.workspaces {
            Workspaces::Only(allowed) if !self.workspaces.allows(workspace) => {
                Err(Refusal::Workspace {
                    holder: self.holder.clone(),
                    workspace: workspace.to_path_buf(),
                    allowed: allowed.clone(),
                })
            }
            _ => Ok(()),
        }
    }
}

/// The caps the core enforces for one process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// The deepest a session the core starts or prompts may run.
    pub(crate) max_depth: u32,
    /// How many sessions each holder may have running at once.
    pub(crate) max_sessions: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_sessions: DEFAULT_MAX_SESSIONS,
        }
    }
}

/// Why the core refused an action. Its `Display` is what the caller sees.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Refusal {
    #[error(
        "{holder} has {} access; `{}` needs {}",
        has.name(),
        action.name(),
        action.needs().name()
    )]
    Access {
        holder: Holder,
        action: Action,
        has: Access,
    },
    #[error(
        "{holder} may not reach workspace {}; it may reach: {}",
        workspace.display(),
        list_paths(allowed)
    )]
    Workspace {
        holder: Holder,
        workspace: PathBuf,
        allowed: Vec<PathBuf>,
    },
    #[error(
        "session depth budget: limit {limit}, asked {asked}; a session at depth {} may not start or prompt another",
        asked - 1
    )]
    Depth { limit: u32, asked: u32 },
    #[error("concurrent sessions budget for {holder}: limit {limit}, asked {asked}")]
    Sessions {
        holder: Holder,
        limit: u32,
        asked: u32,
    },
}

fn list_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
