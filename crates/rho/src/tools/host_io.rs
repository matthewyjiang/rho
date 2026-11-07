//! Work the embedding host does for a session's coding tools.

use std::sync::Arc;

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
    /// The environment the session's processes start with, in place of this
    /// process's own minus provider credentials: a daemon session's is its
    /// client's, replaced by each prompt (#1659).
    pub(crate) environment: Option<rho_tools::SharedProcessEnvironment>,
}

impl HostIo {
    /// The environment the session's processes start with: the host's, or
    /// this process's own without the provider credentials it sets now, so
    /// tools cannot read host API keys.
    pub(crate) fn process_environment(&self) -> rho_tools::SharedProcessEnvironment {
        self.environment.clone().unwrap_or_else(|| {
            rho_sdk::ProcessEnvironment::inherit_except(rho_providers::credential_env_vars()).into()
        })
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
