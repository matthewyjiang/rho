//! Host-served file text over `fs/read_text_file` and `fs/write_text_file`.
//!
//! An editor host that advertises both methods, such as Zed, holds unsaved
//! buffers and records the agent's edits in its own undo and review. Rho's
//! file tools then read and write the text of paths under the directories
//! the host gave the session through the host. Rho still resolves and
//! authorizes every path and computes every edit; this module only carries
//! text. Other paths, which the host's project does not hold, stay on local
//! disk.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use agent_client_protocol::schema::v1::{ReadTextFileRequest, SessionId, WriteTextFileRequest};
use rho_tools::{HostError, HostFuture, HostTextFiles};

use super::{AcpClientPort, AcpStartup};

/// One session's host-served files.
pub(super) struct ClientFiles {
    port: Arc<dyn AcpClientPort>,
    session_id: SessionId,
    roots: Vec<Root>,
}

/// A directory the host gave the session.
struct Root {
    /// Where Rho's tools see it: tool paths are canonical.
    canonical: PathBuf,
    /// How the host spelled it, which may go through a symlink. Requests use
    /// this spelling, since the host's project knows the path by it.
    host: PathBuf,
}

impl ClientFiles {
    /// The files `startup`'s host serves for session `session_id`: those
    /// under `cwd` and `host_dirs`, as the host spelled them. `None` unless
    /// the host advertised both `fs` methods at `initialize`.
    ///
    /// A host directory that is also a launch `--add-dir` stays on disk:
    /// `session/list` reports launch directories, so a host may echo them
    /// back on `session/load` though its project does not hold them.
    pub(super) fn for_session(
        startup: &AcpStartup,
        session_id: &SessionId,
        cwd: &Path,
        host_dirs: &[PathBuf],
    ) -> Option<Arc<Self>> {
        let port = startup
            .client
            .as_ref()
            .filter(|_| startup.host_features.files)?;
        let launch = startup
            .added_dirs
            .as_slice()
            .iter()
            .filter_map(|dir| std::fs::canonicalize(dir).ok())
            .collect::<Vec<_>>();
        let root = |host: &Path| {
            Some(Root {
                canonical: std::fs::canonicalize(host).ok()?,
                host: host.to_path_buf(),
            })
        };
        let roots = root(cwd)
            .into_iter()
            .chain(
                host_dirs
                    .iter()
                    .filter_map(|dir| root(dir))
                    .filter(|dir| !launch.contains(&dir.canonical)),
            )
            .collect();
        Some(Arc::new(Self {
            port: Arc::clone(port),
            session_id: session_id.clone(),
            roots,
        }))
    }

    /// `path` as the host spells it, under the most specific root holding it.
    fn host_path(&self, path: &Path) -> PathBuf {
        self.roots
            .iter()
            .filter_map(|root| Some((root, path.strip_prefix(&root.canonical).ok()?)))
            .max_by_key(|(root, _)| root.canonical.components().count())
            .map_or_else(|| path.to_path_buf(), |(root, rest)| root.host.join(rest))
    }
}

impl HostTextFiles for ClientFiles {
    fn serves(&self, path: &Path) -> bool {
        self.roots
            .iter()
            .any(|root| path.starts_with(&root.canonical))
    }

    fn read_text<'a>(&'a self, path: &'a Path) -> HostFuture<'a, String> {
        Box::pin(async move {
            let path = self.host_path(path);
            self.port
                .read_text_file(ReadTextFileRequest::new(self.session_id.clone(), path))
                .await
                .map(|response| response.content)
                .map_err(|error| HostError::new(error.to_string()))
        })
    }

    fn write_text<'a>(&'a self, path: &'a Path, text: &'a str) -> HostFuture<'a, ()> {
        Box::pin(async move {
            let path = self.host_path(path);
            self.port
                .write_text_file(WriteTextFileRequest::new(
                    self.session_id.clone(),
                    path,
                    text,
                ))
                .await
                .map(drop)
                .map_err(|error| HostError::new(error.to_string()))
        })
    }
}
