//! Work the embedding host does for a session's coding tools.

use std::sync::Arc;

use crate::session_environment::{Ambient, SessionEnvironment};

/// What the embedding host does in place of local disk and local processes
/// for one session's tools. Each part is absent unless the host offers it. Delegated Rho
/// agents share their parent's, so both see the same files.
#[derive(Clone, Default)]
pub(crate) struct HostIo {
    /// Text files the host serves; see [`rho_tools::HostTextFiles`].
    pub(crate) files: Option<Arc<dyn rho_tools::HostTextFiles>>,
    /// Terminals the host runs shell commands in; see
    /// [`rho_tools::HostTerminals`].
    pub(crate) terminals: Option<Arc<dyn rho_tools::HostTerminals>>,
    /// The environment the session's children see, in place of this
    /// process's own: a daemon session's is its client's, replaced by each
    /// prompt (#1659).
    pub(crate) environment: Option<SessionEnvironment>,
}

impl HostIo {
    /// The environment the session's children see: the host's, or else this
    /// process's own, which this call makes the host's, so the tools, MCP
    /// servers, hooks, and OpenAPI sources of one session share one handle.
    pub(crate) fn environment(&mut self) -> SessionEnvironment {
        self.environment
            .get_or_insert_with(|| SessionEnvironment::new(Ambient::ThisProcess))
            .clone()
    }

    /// `options` with the host's parts installed.
    pub(crate) fn coding_options(
        &self,
        options: rho_tools::CodingToolOptions,
    ) -> rho_tools::CodingToolOptions {
        match &self.files {
            Some(files) => options.host_files(Arc::clone(files)),
            None => options,
        }
    }

    /// `options` with the host's terminals installed.
    pub(crate) fn shell_options(
        &self,
        options: rho_tools::ShellToolOptions,
    ) -> rho_tools::ShellToolOptions {
        match &self.terminals {
            Some(terminals) => options.host_terminals(Arc::clone(terminals)),
            None => options,
        }
    }
}
