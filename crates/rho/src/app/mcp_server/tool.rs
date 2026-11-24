//! The tools `rho mcp serve` answers, parsed once from their names: the
//! session-control core's actions, and over HTTP the webhook watch tools.
//! Routing (`remotes/`) and the watches (`watch/`) both decide by it.

use crate::app::session_control::Action;

const WATCH: &str = "watch";
const UNWATCH: &str = "unwatch";

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
            _ => Action::from_mcp_name(name).map(Self::Core),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Core(action) => action.name(),
            Self::Watch => WATCH,
            Self::Unwatch => UNWATCH,
        }
    }
}
