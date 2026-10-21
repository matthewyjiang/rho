//! Work the embedding host does for a session's coding tools.

use std::sync::Arc;

/// What the embedding host does in place of local disk for one session's
/// tools. Each part is absent unless the host offers it. Delegated Rho
/// agents share their parent's, so both see the same files.
#[derive(Clone, Default)]
pub(crate) struct HostIo {
    /// Text files the host serves; see [`rho_tools::HostTextFiles`].
    pub(crate) files: Option<Arc<dyn rho_tools::HostTextFiles>>,
}

impl HostIo {
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
}
