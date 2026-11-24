//! The tools `rho mcp serve` answers, parsed once from their names: the
//! session-control core's actions, and over HTTP the webhook watch tools.
//! Routing (`remotes/`) and the watches (`watch/`) both decide by it.

use crate::app::session_control::Action;

const WATCH: &str = "watch";
const UNWATCH: &str = "unwatch";

/// The core actions the server serves, in the core's order.
pub(crate) fn served_actions() -> impl Iterator<Item = Action> {
    Action::ALL.into_iter().filter(|&action| served(action))
}

/// Whether the server serves core action `action`: all but `answer`, whose
/// caller must be the session that started the asking one, which no MCP
/// caller is.
fn served(action: Action) -> bool {
    match action {
        Action::List
        | Action::Read
        | Action::New
        | Action::Prompt
        | Action::Steer
        | Action::Cancel
        | Action::Wait => true,
        Action::Answer => false,
    }
}

/// A tool the server answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServedTool {
    Core(Action),
    /// HTTP only.
    Watch,
    /// HTTP only.
    Unwatch,
}

impl ServedTool {
    /// The tool called `name`, if there is one.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            WATCH => Some(Self::Watch),
            UNWATCH => Some(Self::Unwatch),
            _ => Self::core(name),
        }
    }

    /// The core action called `name`, if the server serves it.
    pub(crate) fn core(name: &str) -> Option<Self> {
        Action::from_name(name)
            .filter(|&action| served(action))
            .map(Self::Core)
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Core(action) => action.name(),
            Self::Watch => WATCH,
            Self::Unwatch => UNWATCH,
        }
    }
}
