//! Opening a session's runtime: a new session, a fork of a saved one, or a
//! saved one reopened.
//!
//! A new session writes no file until its first save (see `persist.rs`), so
//! a session nobody prompts leaves none. A reopened session runs as the
//! agent it was saved as, and its host may show its saved conversation to
//! its client before the first prompt. A workflow node's session is found
//! by [`NodeSession::find`] instead, which holds it to the node's agent.

use std::path::Path;

use rho_sdk::{
    model::{ContentBlock, Message},
    SessionId, SessionOptions,
};

use super::{
    agent::loaded_agent,
    build::{build_session, session_config},
    credentials::SessionCredentials,
    OpenedSession, SessionBuild, SessionRuntime, SessionStorage, SessionTools,
};
use crate::{
    added_dirs::AddedDirs,
    agent::AgentCatalog,
    app::agent_binding::BoundAgent,
    config::Config,
    credential_store::AppCredentialStore,
    permission::PermissionMode,
    session::{
        tree::ProjectedMessage, Session as StoredSession, SessionOrigin, SessionPersistence,
        SessionProvenance, SessionTarget,
    },
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
}

/// What a workflow node's attempt finds under the session id it recorded.
pub(crate) enum NodeSession {
    /// Nothing is saved under the id: the attempt's agent never prompted.
    Unsaved,
    /// The saved session, whose conversation the attempt continues.
    Saved(StoredSession),
    /// The saved session ends in the agent's final reply, with this text:
    /// the process that ran the attempt died before it reported the reply.
    Answered(String),
}

impl NodeSession {
    /// Finds the session saved under exactly `id` in `cwd`'s workspace for a
    /// node bound to `agent`. The plan froze the node's agent, so the session
    /// must be `agent`'s root session, by id and fingerprint; any other fails
    /// rather than run as an agent the plan did not approve.
    pub(crate) fn find(cwd: &Path, id: &SessionId, agent: &BoundAgent) -> anyhow::Result<Self> {
        let Some((stored, history)) = StoredSession::open_exact_in_workspace(cwd, id.as_str())?
        else {
            return Ok(Self::Unsaved);
        };
        let fingerprint = agent.fingerprint().to_string();
        let bound = match (
            stored.delegated_agent_id()?,
            stored.stored_agent_identity()?,
        ) {
            (None, Some((stored_id, stored_fingerprint))) => {
                stored_id == agent.id().as_str() && stored_fingerprint == fingerprint
            }
            (Some(_), _) | (None, None) => false,
        };
        anyhow::ensure!(
            bound,
            "session '{id}' is not a session of workflow agent '{}' as the plan froze it; refusing to continue the node in it",
            agent.id(),
        );
        Ok(match final_reply(&history) {
            Some(reply) => Self::Answered(reply),
            None => Self::Saved(stored),
        })
    }
}

/// The text of the agent's final reply when `history` ends in one: a
/// completed assistant message that calls no tools.
fn final_reply(history: &[Message]) -> Option<String> {
    let mut reply = String::new();
    for block in history.last()?.completed_assistant_content()? {
        match block {
            ContentBlock::Text(text) => reply.push_str(text),
            ContentBlock::Image(_) => {}
            ContentBlock::ToolCall(_) => return None,
        }
    }
    Some(reply)
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
    /// Opens a new session `id` as the build's launch agent, which session
    /// `started_by` started through session control, if one did. Its file
    /// is created by its first save, unless `persistence` says it is never
    /// written, and records `started_by` (see
    /// `SessionProvenance::started_by`).
    pub(crate) async fn open_new(
        opening: SessionOpening<'_>,
        id: SessionId,
        persistence: SessionPersistence,
        started_by: Option<SessionId>,
    ) -> anyhow::Result<Self> {
        let provenance = SessionProvenance {
            started_by: started_by.map(|session| session.to_string()),
            ..opening.build.run.role.provenance()
        };
        Self::open_fresh(opening, id.clone(), persistence, provenance, |_| {
            Ok(session_options_for_id(id))
        })
        .await
    }

    /// Opens new session `id` as a fork of `parent`, as the build's launch
    /// agent, which the host bound as the agent `parent` was saved as: with
    /// `parent`'s conversation as last saved, its prompt cache key, and the
    /// settings its file keeps, what the sessions it starts open with
    /// included. `parent` may still be working in the same
    /// workspace, so the fork then opens supervised, or stays in plan mode,
    /// which edits nothing either and keeps `parent`'s tools. It keeps its
    /// settings with its own file, so a reopen keeps that mode. That file is
    /// created by its first save, unless `persistence` says it is never
    /// written, and records `parent` as the session that started it.
    pub(crate) async fn open_fork(
        opening: SessionOpening<'_>,
        id: SessionId,
        parent: &StoredSession,
        persistence: SessionPersistence,
    ) -> anyhow::Result<Self> {
        let process_config = opening.process_config;
        let provenance = SessionProvenance {
            origin: SessionOrigin::Fork,
            parent: Some(SessionTarget::new(parent.id(), parent.cwd())),
            started_by: None,
        };
        let mut runtime =
            Self::open_fresh(opening, id.clone(), persistence, provenance, |provider| {
                let key = prompt_cache_key(parent.id());
                let conversation = parent.snapshot_for_fork(provider.identity(), key, id)?;
                Ok(SessionOptions::from_snapshot(conversation))
            })
            .await?;
        Box::pin(runtime.restore_settings_of(parent, process_config)).await;
        if runtime.permission_mode() != PermissionMode::Plan {
            runtime
                .set_permission_mode(PermissionMode::Supervised, process_config)
                .await
                .map_err(|error| anyhow::anyhow!("could not open the fork supervised: {error}"))?;
        }
        runtime.keep_settings()?;
        Ok(runtime)
    }

    /// Opens new session `id` as the build's launch agent, recording
    /// `provenance`, with the options `session_options` gives for its
    /// provider: the body of [`Self::open_new`] and [`Self::open_fork`].
    async fn open_fresh(
        opening: SessionOpening<'_>,
        id: SessionId,
        persistence: SessionPersistence,
        provenance: SessionProvenance,
        session_options: impl FnOnce(
            std::sync::Arc<dyn rho_sdk::provider::ModelProvider>,
        ) -> anyhow::Result<SessionOptions>,
    ) -> anyhow::Result<Self> {
        let SessionOpening {
            build,
            process_config,
            host_mcp,
            cwd,
            added_dirs,
        } = opening;
        let config = session_config(process_config, &host_mcp);
        let agent = build.launch.agent.clone();
        let read = AppCredentialStore::generation();
        let (built, permission) = build_session(
            &build,
            &agent,
            &config,
            cwd,
            &added_dirs,
            build.host.tool_sources,
            session_options,
        )
        .await?;
        let credentials = SessionCredentials::opened(read, built.provider_credentials);
        let storage = match persistence {
            SessionPersistence::Saved => SessionStorage::deferred(
                cwd.to_path_buf(),
                id.to_string(),
                agent.id().to_string(),
                agent.fingerprint().to_string(),
                provenance,
            ),
            SessionPersistence::Ephemeral => SessionStorage::ephemeral(cwd.to_path_buf()),
        };
        Ok(Self::new(OpenedSession {
            build,
            built,
            storage,
            agent,
            auth: config.auth,
            auth_origin: config.auth_origin,
            host_mcp,
            added_dirs,
            permission,
            reopen_notice: None,
            credentials,
            providers: config.providers,
        }))
    }

    /// Opens `stored` as the agent it was saved as and keeping the origin its
    /// file records, with the settings its file keeps (see
    /// [`Self::restore_settings`]). Its host may then show its
    /// [`Self::saved_history`].
    pub(crate) async fn open_saved(
        opening: SessionOpening<'_>,
        stored: StoredSession,
    ) -> anyhow::Result<Self> {
        let SessionOpening {
            build,
            process_config,
            host_mcp,
            cwd,
            added_dirs,
        } = opening;
        let (agent, config) = loaded_agent(
            &build.launch.agent,
            &stored,
            session_config(process_config, &host_mcp),
            || {
                Ok(AgentCatalog::discover(
                    cwd,
                    build.host.environment.project_trust(),
                )?)
            },
            build.host.environment.credentials().as_ref(),
        )?;
        let read = AppCredentialStore::generation();
        let (built, permission) = build_session(
            &build,
            &agent,
            &config,
            cwd,
            &added_dirs,
            build.host.tool_sources,
            |provider| {
                let snapshot = stored
                    .snapshot_for_resume(provider.identity(), prompt_cache_key(stored.id()))?;
                Ok(SessionOptions::from_snapshot(snapshot))
            },
        )
        .await?;
        let credentials = SessionCredentials::opened(read, built.provider_credentials);
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
        let mut runtime = Self::new(OpenedSession {
            build,
            built,
            storage: SessionStorage::saved(stored),
            agent,
            // The restored child may use a different auth family than
            // the process.
            auth: config.auth,
            auth_origin: config.auth_origin,
            host_mcp,
            added_dirs,
            permission,
            reopen_notice,
            credentials,
            providers: config.providers,
        });
        // In every host, since every host's saves keep a record once the
        // file has one: a host that skipped this would overwrite it with
        // the config's settings.
        Box::pin(runtime.restore_settings(process_config)).await;
        Ok(runtime)
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

    /// Whether the session's history holds a conversation. The SDK stores
    /// its initial system prompt and context in history; those alone are
    /// not one. A session opened new, or reopened from a file saved before
    /// its first prompt reached the model, holds none.
    pub(crate) fn has_conversation(&self) -> bool {
        self.built
            .session
            .history()
            .iter()
            .any(|message| match message {
                Message::System(_) | Message::ModelContext(_) | Message::ReasoningShift(_) => false,
                Message::User(_)
                | Message::Assistant(_)
                | Message::EnrichedAssistant(_)
                | Message::AbortedAssistant(_)
                | Message::ToolResult(_)
                | Message::CompactionSummary(_)
                | Message::BoundaryInput(_) => true,
            })
    }
}

pub(super) fn prompt_cache_key(id: &str) -> String {
    rho_providers::providers::openai::prompt_cache_key_from_session_id(id)
        .unwrap_or_else(|| format!("rho:{id}"))
}

/// New session options with a stable prompt-cache key derived from the id.
pub(super) fn session_options_for_id(id: SessionId) -> SessionOptions {
    let key = prompt_cache_key(id.as_str());
    SessionOptions::new().id(id).prompt_cache_key(key)
}
