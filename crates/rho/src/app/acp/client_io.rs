//! What one ACP session's host does for the session's tools.
//!
//! Built once when the session opens, from the host's own description of the
//! session, and shared by every tool set the session builds, such as the one
//! a delegated agent run rebuilds.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use agent_client_protocol::schema::v1::SessionId;

use super::{client_files::ClientFiles, client_terminals::ClientTerminals, AcpStartup};
use crate::tools::host_io::HostIo;

/// One session's host I/O. Each part is `None` unless the host advertised it.
#[derive(Clone)]
pub(super) struct ClientIo {
    pub(super) session_id: SessionId,
    files: Option<Arc<ClientFiles>>,
    terminals: Option<Arc<ClientTerminals>>,
}

impl ClientIo {
    /// The host I/O of session `session_id`, whose host gave it `cwd` and
    /// `host_dirs` (`additionalDirectories`). Directories added at launch
    /// with `--add-dir` are not part of the host's project, so they stay on
    /// local disk.
    pub(super) fn new(
        startup: &AcpStartup,
        session_id: SessionId,
        cwd: &Path,
        host_dirs: &[PathBuf],
    ) -> Self {
        Self {
            files: ClientFiles::for_session(startup, &session_id, cwd, host_dirs),
            terminals: ClientTerminals::for_session(startup, &session_id),
            session_id,
        }
    }

    /// A session whose tools use no host I/O.
    #[cfg(test)]
    pub(super) fn local(session_id: SessionId) -> Self {
        Self {
            session_id,
            files: None,
            terminals: None,
        }
    }

    /// The host I/O to give a tool set.
    pub(super) fn host_io(&self) -> HostIo {
        HostIo {
            files: self
                .files
                .clone()
                .map(|files| files as Arc<dyn rho_tools::HostTextFiles>),
            terminals: self
                .terminals
                .clone()
                .map(|terminals| terminals as Arc<dyn rho_tools::HostTerminals>),
        }
    }

    /// Releases host terminals whose calls ended. Call it once the last card
    /// update of a prompt was sent; see `client_terminals`.
    pub(super) async fn release_finished_terminals(&self) {
        if let Some(terminals) = &self.terminals {
            terminals.release_finished().await;
        }
    }

    /// Releases every host terminal, stopping commands still running.
    pub(super) async fn release_all_terminals(&self) {
        if let Some(terminals) = &self.terminals {
            terminals.release_all().await;
        }
    }
}
