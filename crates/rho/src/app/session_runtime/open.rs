//! Opening a session's runtime: a new session, or a saved one reopened.
//!
//! A new session writes no file until its first save (see `persist.rs`), so
//! a session nobody prompts leaves none. That makes "nothing saved under
//! this id" a session that was opened but never prompted, so reopening it
//! opens it new again under the same id. A reopened session runs as the
//! agent it was saved as, and its host may show the saved conversation to
//! its client before the runtime takes over.

use std::{future::Future, path::Path};

use rho_sdk::SessionOptions;

use super::{
    agent::loaded_agent,
    build::{build_session, session_config},
    Logouts, OpenedSession, SessionBuild, SessionRuntime, SessionStorage,
};
use crate::{
    added_dirs::AddedDirs,
    agent::AgentCatalog,
    app::interactive_runtime::startup::{prompt_cache_key, session_options_for_id},
    config::Config,
    session::{tree::ProjectedMessage, NoSessionFound, Session as StoredSession},
    tools::{mcp::config::McpConfig, sdk_registry::AppToolSet},
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

/// A reopened session's saved conversation, which its host may show its
/// client before the runtime takes over.
pub(crate) struct SavedHistory<'a> {
    stored: &'a StoredSession,
    cwd: &'a Path,
    tools: &'a AppToolSet,
}

impl<'a> SavedHistory<'a> {
    /// The rows a transcript shows, along the session's active branch.
    pub(crate) fn display_rows(&self) -> anyhow::Result<Vec<ProjectedMessage>> {
        self.stored.active_display_history()
    }

    /// The session's workspace, which the rows' paths are relative to.
    pub(crate) fn cwd(&self) -> &'a Path {
        self.cwd
    }

    /// The session's tools, which describe the calls the rows show.
    pub(crate) fn tool_set(&self) -> &'a AppToolSet {
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
    /// agent it was saved as and keeping the origin its file records.
    /// `replay` gets its saved conversation once the session is built; a
    /// replay that fails fails the reopen.
    ///
    /// An `id` nothing was saved under opens new under that id, without a
    /// replay: it names a session opened but never prompted.
    pub(crate) async fn reopen<Replay, Replayed>(
        opening: SessionOpening<'_>,
        id: &str,
        replay: Replay,
    ) -> anyhow::Result<Self>
    where
        Replay: FnOnce(SavedHistory<'_>) -> Replayed,
        Replayed: Future<Output = anyhow::Result<()>>,
    {
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
        let replayed = replay(SavedHistory {
            stored: &stored,
            cwd,
            tools: &built.tools,
        })
        .await;
        if let Err(error) = replayed {
            built.teardown().await;
            return Err(error);
        }
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
}
