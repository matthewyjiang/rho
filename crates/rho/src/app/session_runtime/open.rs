//! Opening a session's runtime: a new session, or a saved one reopened.
//!
//! A new session writes no file until its first save (see `persist.rs`), so
//! a session nobody prompts leaves none. That makes "nothing saved under
//! this id" a session that was opened but never prompted, so reopening it
//! opens it new again under the same id. A reopened session runs as the
//! agent it was saved as, and its host may show its saved conversation to
//! its client before the first prompt.

use std::path::Path;

use rho_sdk::SessionOptions;

use super::{
    agent::loaded_agent,
    build::{build_session, session_config},
    Logouts, OpenedSession, SessionBuild, SessionRuntime, SessionStorage, SessionTools,
};
use crate::{
    added_dirs::AddedDirs,
    agent::AgentCatalog,
    app::interactive_runtime::startup::{prompt_cache_key, session_options_for_id},
    config::Config,
    session::{tree::ProjectedMessage, NoSessionFound, Session as StoredSession},
    tools::mcp::config::McpConfig,
};

/// Where and how a host opens one session.
pub(crate) struct SessionOpening<'a> {
    /// How every build of the session is made.
    pub(crate) build: SessionBuild,
    /// The process config, before the host's MCP servers.
    pub(crate) process_config: &'a Config,
    /// The host's own MCP servers, overlaid on the process config whenever
    /// the session is built.
    pub(crate) host_mcp: McpConfig,
    /// The session's workspace.
    pub(crate) cwd: &'a Path,
    /// Launch `--add-dir` values plus the host's own roots.
    pub(crate) added_dirs: AddedDirs,
    /// The host's logout count; the session's provider reads credentials
    /// as it opens.
    pub(crate) logouts: &'a Logouts,
}

/// A saved session's conversation, which its host may show its client,
/// such as when it reopens the session.
pub(crate) struct SavedHistory<'a> {
    stored: &'a StoredSession,
    cwd: &'a Path,
    tools: SessionTools<'a>,
}

impl<'a> SavedHistory<'a> {
    /// The rows a transcript shows, along the session's active branch.
    pub(crate) fn display_rows(&self) -> anyhow::Result<Vec<ProjectedMessage>> {
        self.stored.active_display_history()
    }

    /// The workspace the session was opened in, which the rows' paths are
    /// shown relative to.
    pub(crate) fn cwd(&self) -> &'a Path {
        self.cwd
    }

    /// The session's tools, which describe the calls the rows show.
    pub(crate) fn tools(&self) -> SessionTools<'a> {
        self.tools
    }
}

impl SessionRuntime {
    /// Opens a new session `id` as the build's launch agent. Its file is
    /// created by its first save.
    pub(crate) async fn open_new(
        opening: SessionOpening<'_>,
        id: rho_sdk::SessionId,
    ) -> anyhow::Result<Self> {
        let SessionOpening {
            build,
            process_config,
            host_mcp,
            cwd,
            added_dirs,
            logouts,
        } = opening;
        let config = session_config(process_config, &host_mcp);
        let agent = build.launch_agent.clone();
        let (built, permission) = build_session(&build, &agent, &config, cwd, &added_dirs, |_| {
            Ok(session_options_for_id(id))
        })
        .await?;
        let storage = SessionStorage::deferred(
            cwd.to_path_buf(),
            built.session.id().to_string(),
            agent.id().to_string(),
            agent.fingerprint().to_string(),
            build.run.role.origin(),
        );
        Ok(Self::new(
            OpenedSession {
                build,
                built,
                storage,
                agent,
                auth: config.auth,
                host_mcp,
                added_dirs,
                permission,
                reopen_notice: None,
            },
            logouts,
        ))
    }

    /// Reopens the session saved as `id` in the opening's workspace, as the
    /// agent it was saved as and keeping the origin its file records. Its
    /// host may then show its [`Self::saved_history`].
    ///
    /// An `id` nothing was saved under opens new under that id, with no
    /// saved history: it names a session opened but never prompted.
    pub(crate) async fn reopen(opening: SessionOpening<'_>, id: &str) -> anyhow::Result<Self> {
        let stored = match StoredSession::open_by_id_with_histories(opening.cwd, id) {
            Ok((stored, _)) => stored,
            Err(error) if error.is::<NoSessionFound>() => {
                let Ok(id) = rho_sdk::SessionId::from_string(id) else {
                    return Err(error);
                };
                return Self::open_new(opening, id).await;
            }
            Err(error) => return Err(error),
        };
        let SessionOpening {
            build,
            process_config,
            host_mcp,
            cwd,
            added_dirs,
            logouts,
        } = opening;
        let (agent, config) = loaded_agent(
            &build.launch_agent,
            &stored,
            session_config(process_config, &host_mcp),
            || Ok(AgentCatalog::discover(cwd)?),
        )?;
        let (built, permission) =
            build_session(&build, &agent, &config, cwd, &added_dirs, |provider| {
                let snapshot = stored
                    .snapshot_for_resume(provider.identity(), prompt_cache_key(stored.id()))?;
                Ok(SessionOptions::from_snapshot(snapshot))
            })
            .await?;
        // Reported with the first turn, because hosts such as T3 Code
        // discard live updates while a load is in flight.
        let reopen_notice = built
            .prompt_template
            .is_some()
            .then(|| {
                crate::app::model_prompt_metadata::change_notice(
                    &built.session.snapshot(),
                    built.prompt.loaded.as_ref(),
                )
            })
            .flatten();
        Ok(Self::new(
            OpenedSession {
                build,
                built,
                storage: SessionStorage::saved(stored),
                agent,
                // The restored child may use a different auth family than
                // the process.
                auth: config.auth,
                host_mcp,
                added_dirs,
                permission,
                reopen_notice,
            },
            logouts,
        ))
    }

    /// The session's saved conversation, or `None` while nothing is saved:
    /// a new session before its first save.
    pub(crate) fn saved_history(&self) -> Option<SavedHistory<'_>> {
        Some(SavedHistory {
            stored: self.storage.file()?,
            cwd: &self.permission.workspace_root,
            tools: self.tools(),
        })
    }
}
