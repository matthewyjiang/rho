//! Opening ACP sessions: `session/new` builds a host whose file waits for
//! its first save, and `session/load` and `session/resume` restore a saved
//! one, `session/load` also replaying its display history to the client.
//! All take the host's `additionalDirectories` as the session's added
//! directories.

use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    LoadSessionRequest, LoadSessionResponse, McpServer, NewSessionRequest, NewSessionResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionConfigOption, SessionId, SessionModeState,
};
use rho_sdk::{model::Message, SessionOptions};

use super::{
    super::client_io::ClientIo,
    agent,
    build::{build_session, mode_state, session_config},
    convert::{session_added_dirs, validate_session_cwd, workspace_cwd},
    persist::HostStorage,
    send_notification, split_assembly, EventMapper, SessionHost,
};
use crate::{
    added_dirs::AddedDirs,
    app::{
        acp::{AcpClientPort, AcpStartup},
        interactive_runtime::startup::{prompt_cache_key, session_options_for_id},
    },
    config::Config,
    session::Session as StoredSession,
    tools::mcp::exported_name::McpToolNames,
};

impl SessionHost {
    pub(in crate::app::acp) async fn create(
        startup: &AcpStartup,
        request: NewSessionRequest,
    ) -> anyhow::Result<(Self, NewSessionResponse)> {
        let cwd = validate_session_cwd(&request.cwd)?;
        let root = std::fs::canonicalize(cwd)?;
        let added_dirs =
            session_added_dirs(&startup.added_dirs, &request.additional_directories, &root)?;
        let config = session_config(&startup.config, &request.mcp_servers);
        let host = Self::unsaved(
            startup,
            &config,
            cwd,
            added_dirs,
            rho_sdk::SessionId::new(),
            &request.additional_directories,
            request.mcp_servers,
        )
        .await?;
        let response = NewSessionResponse::new(host.acp_session_id.clone())
            .modes(mode_state(host.permission.mode))
            .config_options(host.config_options(&startup.config));
        Ok((host, response))
    }

    /// A host for session `sdk_id` with no saved turns. Its file is created
    /// by the first save, so a session the host never prompts leaves none.
    async fn unsaved(
        startup: &AcpStartup,
        config: &Config,
        cwd: &Path,
        added_dirs: AddedDirs,
        sdk_id: rho_sdk::SessionId,
        host_dirs: &[PathBuf],
        host_mcp_servers: Vec<McpServer>,
    ) -> anyhow::Result<Self> {
        let io = ClientIo::new(startup, SessionId::new(sdk_id.as_str()), cwd, host_dirs);
        let assembly = build_session(
            startup,
            &io,
            &startup.agent,
            config,
            cwd,
            &added_dirs,
            |_| Ok(session_options_for_id(sdk_id)),
        )
        .await?;
        let (built, permission) = split_assembly(assembly, config.permission_mode);
        let storage = HostStorage::deferred(
            cwd.to_path_buf(),
            built.session.id().to_string(),
            startup.agent.id().to_string(),
            startup.agent.fingerprint().to_string(),
        );
        Ok(Self::from_built(
            io,
            built,
            storage,
            added_dirs,
            permission,
            startup,
            host_mcp_servers,
        ))
    }

    /// Restores a saved session for `session/load` or `session/resume`,
    /// sending its display history as `history` says.
    ///
    /// A session created by `session/new` has no file until its first
    /// prompt, so an id with no saved session anywhere is restored as that
    /// empty session, as it was when `session/new` wrote the file eagerly.
    pub(in crate::app::acp) async fn reopen(
        startup: &AcpStartup,
        request: ReopenRequest,
        history: History<'_>,
    ) -> anyhow::Result<Self> {
        let cwd = workspace_cwd(&request.cwd, &startup.cwd);
        if !request.cwd.as_os_str().is_empty() {
            validate_session_cwd(cwd)?;
        }
        let root = std::fs::canonicalize(cwd)?;
        let added_dirs =
            session_added_dirs(&startup.added_dirs, &request.additional_directories, &root)?;
        let config = session_config(&startup.config, &request.mcp_servers);
        let (stored, histories) =
            match StoredSession::open_by_id_with_histories(cwd, request.session_id.0.as_ref()) {
                Ok(opened) => opened,
                Err(error) if error.is::<crate::session::NoSessionFound>() => {
                    let Ok(sdk_id) = rho_sdk::SessionId::from_string(request.session_id.0.as_ref())
                    else {
                        return Err(error);
                    };
                    return Self::unsaved(
                        startup,
                        &config,
                        cwd,
                        added_dirs,
                        sdk_id,
                        &request.additional_directories,
                        request.mcp_servers,
                    )
                    .await;
                }
                Err(error) => return Err(error),
            };
        let (agent, config) = agent::loaded_agent(startup, &stored, config, || {
            Ok(crate::agent::AgentCatalog::discover(cwd)?)
        })?;
        let io = ClientIo::new(
            startup,
            request.session_id.clone(),
            cwd,
            &request.additional_directories,
        );
        let assembly = build_session(
            startup,
            &io,
            &agent,
            &config,
            cwd,
            &added_dirs,
            |provider| {
                let snapshot = stored
                    .snapshot_for_resume(provider.identity(), prompt_cache_key(stored.id()))?;
                Ok(SessionOptions::from_snapshot(snapshot))
            },
        )
        .await?;
        let (built, permission) = split_assembly(assembly, config.permission_mode);
        if let History::Replay(client) = history {
            let replay_result = replay_display_history(
                &request.session_id,
                &histories.display,
                cwd,
                &built.tools.mcp_report().tool_names(),
                client,
            )
            .await;
            if let Err(error) = replay_result {
                built.teardown().await;
                return Err(error);
            }
        }
        let load_notice = built
            .prompt_template
            .is_some()
            .then(|| {
                crate::app::model_prompt_metadata::change_notice(
                    &built.session.snapshot(),
                    built.prompt.loaded.as_ref(),
                )
            })
            .flatten();
        let mut host = Self::from_built(
            io,
            built,
            HostStorage::saved(stored),
            added_dirs,
            permission,
            startup,
            request.mcp_servers,
        );
        host.load_notice = load_notice;
        // The restored child may use a different auth family than the process.
        host.auth = config.auth;
        host.agent = agent;
        Ok(host)
    }
}

/// The parameters `session/load` and `session/resume` share.
pub(in crate::app::acp) struct ReopenRequest {
    pub(in crate::app::acp) session_id: SessionId,
    cwd: PathBuf,
    additional_directories: Vec<PathBuf>,
    mcp_servers: Vec<McpServer>,
}

impl From<LoadSessionRequest> for ReopenRequest {
    fn from(request: LoadSessionRequest) -> Self {
        Self {
            session_id: request.session_id,
            cwd: request.cwd,
            additional_directories: request.additional_directories,
            mcp_servers: request.mcp_servers,
        }
    }
}

impl From<ResumeSessionRequest> for ReopenRequest {
    fn from(request: ResumeSessionRequest) -> Self {
        Self {
            session_id: request.session_id,
            cwd: request.cwd,
            additional_directories: request.additional_directories,
            mcp_servers: request.mcp_servers,
        }
    }
}

/// Whether reopening sends the saved conversation to the client.
#[derive(Clone, Copy)]
pub(in crate::app::acp) enum History<'a> {
    /// `session/load`: replay it before the response.
    Replay(&'a dyn AcpClientPort),
    /// `session/resume`: the client already has it.
    Skip,
}

/// The state a reopened session answers `session/load` or `session/resume`
/// with.
pub(in crate::app::acp) struct Reopened {
    modes: SessionModeState,
    config_options: Vec<SessionConfigOption>,
}

impl SessionHost {
    /// What a reopened session answers `session/load` or `session/resume`
    /// with.
    pub(in crate::app::acp) fn reopened(&self, process_config: &Config) -> Reopened {
        Reopened {
            modes: mode_state(self.permission.mode),
            config_options: self.config_options(process_config),
        }
    }
}

impl From<Reopened> for LoadSessionResponse {
    fn from(reopened: Reopened) -> Self {
        Self::new()
            .modes(reopened.modes)
            .config_options(reopened.config_options)
    }
}

impl From<Reopened> for ResumeSessionResponse {
    fn from(reopened: Reopened) -> Self {
        Self::new()
            .modes(reopened.modes)
            .config_options(reopened.config_options)
    }
}

async fn replay_display_history(
    session_id: &SessionId,
    messages: &[Message],
    cwd: &Path,
    mcp_tools: &McpToolNames,
    client: &dyn AcpClientPort,
) -> anyhow::Result<()> {
    for notification in EventMapper::replay_history(session_id, messages, cwd, mcp_tools) {
        send_notification(client, notification).await?;
    }
    Ok(())
}
