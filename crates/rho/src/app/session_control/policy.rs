//! What a caller of the session-control core may do, and the budgets that
//! bound it. Every refusal names what it enforces: a missing scope names the
//! action, a workspace outside the grant names the allowed list, and a cap
//! names its budget, its limit, and the value asked for.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use rho_protocol::ClientEnvironment;

use crate::caller_context::{Access, CallerContext, Scope, Workspaces};

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

/// Who holds a grant, as refusals and the per-holder session count name it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Holder {
    /// The process's own caller, as on stdio: whoever started it.
    Local,
    /// The holder of an MCP server token, by its name.
    Token(String),
    /// A session's agent, through the native `sessions` tool.
    Session(rho_protocol::SessionId),
}

impl fmt::Display for Holder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => formatter.write_str("this server's caller"),
            Self::Token(name) => write!(formatter, "token {name:?}"),
            Self::Session(session) => write!(formatter, "session {session}"),
        }
    }
}

/// What one caller may do: whose it is, and its scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Grant {
    pub(crate) holder: Holder,
    pub(crate) scope: Scope,
}

impl Grant {
    /// Every workspace with `access`, for the local caller.
    pub(crate) fn local(access: Access) -> Self {
        Self {
            holder: Holder::Local,
            scope: Scope {
                access,
                workspaces: Workspaces::Any,
            },
        }
    }

    /// Refuses `action` unless the grant's access covers it.
    pub(crate) fn permit(&self, action: Action) -> Result<(), Refusal> {
        if self.scope.access >= action.needs() {
            return Ok(());
        }
        Err(Refusal::Access {
            holder: self.holder.clone(),
            action,
            has: self.scope.access,
        })
    }

    /// Refuses a session in `workspace`, a canonical path, unless the
    /// grant reaches it.
    pub(crate) fn reach(&self, workspace: &Path) -> Result<(), Refusal> {
        match &self.scope.workspaces {
            Workspaces::Only(allowed) if !self.scope.workspaces.allows(workspace) => {
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

/// One call's caller (#1765): the grant its transport gave, narrowed by the
/// scope of the session that calls, and that session's nesting depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Caller {
    pub(crate) grant: Grant,
    /// The calling session's depth: 0 for a person, or for an outside agent.
    pub(crate) depth: u32,
    /// The calling session, when the front end knows it, as the native
    /// `sessions` tool does: a session it starts records the job writing it
    /// as its parent and shares the daemon-wide agent budget (#1714).
    pub(crate) session: Option<rho_protocol::SessionId>,
    /// The environment of the process or session that calls, which the
    /// sessions it starts or prompts run with; the daemon sets the grant
    /// the core names for them in it.
    pub(crate) environment: ClientEnvironment,
}

impl Caller {
    /// The caller `grant`'s holder is from `context`, in `environment`: a
    /// scope the calling session carries can only narrow the grant, never
    /// widen it.
    pub(crate) fn new(
        grant: Grant,
        context: &CallerContext,
        environment: ClientEnvironment,
    ) -> Self {
        Self {
            grant: Grant {
                scope: grant.scope.intersect(&context.scope),
                ..grant
            },
            depth: context.depth,
            session: None,
            environment,
        }
    }

    /// The agent of `session`, at the depth and scope of its `context`,
    /// in the session's `environment`: it holds a full grant of its own,
    /// which only its context narrows.
    pub(crate) fn session(
        session: rho_protocol::SessionId,
        context: &CallerContext,
        environment: ClientEnvironment,
    ) -> Self {
        let grant = Grant {
            holder: Holder::Session(session.clone()),
            scope: Scope::FULL,
        };
        Self {
            session: Some(session),
            ..Self::new(grant, context, environment)
        }
    }

    /// The context a session this caller starts or prompts runs with: one
    /// level deeper, and no more than the caller may do.
    pub(crate) fn child_context(&self) -> CallerContext {
        CallerContext {
            depth: self.depth.saturating_add(1),
            scope: self.grant.scope.clone(),
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

/// `paths` for a refusal; "no workspace" when a grant narrowed by a calling
/// session's scope shares none with it.
fn list_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "no workspace".to_owned();
    }
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
