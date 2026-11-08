//! Opening ACP sessions: `session/new` opens a new runtime, whose file
//! waits for its first save, and `session/load` and `session/resume` reopen
//! a saved one, `session/load` also replaying its display history to the
//! client. The runtime decides how a session opens; this side turns the
//! requests into its parameters and the results into ACP's. All take the
//! host's `additionalDirectories` as the session's added directories.

use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    LoadSessionRequest, LoadSessionResponse, McpServer, NewSessionRequest, NewSessionResponse,
    ResumeSessionRequest, ResumeSessionResponse, SessionConfigOption, SessionId, SessionModeState,
};

use super::{
    super::{client_io::ClientIo, host_mcp::host_mcp_config, permission},
    convert::{validate_session_cwd, workspace_cwd},
    send_notification, session_build, EventMapper, HostBuild, SessionHost,
};
use crate::{
    added_dirs::session_added_dirs,
    app::{
        acp::{AcpClientPort, AcpStartup},
        session_runtime::{SavedHistory, SessionOpening, SessionRuntime},
    },
    config::Config,
    permission::PermissionMode,
    session::SessionPersistence,
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
        let sdk_id = rho_sdk::SessionId::new();
        let io = ClientIo::new(
            startup,
            SessionId::new(sdk_id.as_str()),
            cwd,
            &request.additional_directories,
        );
        let HostBuild { build, t3_wake } = session_build(startup, &io);
        let runtime = SessionRuntime::open_new(
            SessionOpening {
                build,
                process_config: &startup.config,
                host_mcp: host_mcp_config(&request.mcp_servers),
                cwd,
                added_dirs,
            },
            sdk_id,
            SessionPersistence::Saved,
        )
        .await?;
        let host = Self::from_runtime(io, startup, runtime, t3_wake);
        let response = NewSessionResponse::new(host.acp_session_id.clone())
            .modes(mode_state(host.runtime.permission_mode()))
            .config_options(host.config_options(&startup.config));
        Ok((host, response))
    }

    /// Restores a saved session for `session/load` or `session/resume`,
    /// sending its display history as `history` says.
    ///
    /// A session created by `session/new` has no file until its first
    /// prompt, so an id with no saved session anywhere reopens as that
    /// empty session, as it did when `session/new` wrote the file eagerly.
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
        let io = ClientIo::new(
            startup,
            request.session_id.clone(),
            cwd,
            &request.additional_directories,
        );
        let HostBuild { build, t3_wake } = session_build(startup, &io);
        let opening = SessionOpening {
            build,
            process_config: &startup.config,
            host_mcp: host_mcp_config(&request.mcp_servers),
            cwd,
            added_dirs,
        };
        let runtime = SessionRuntime::reopen(opening, request.session_id.0.as_ref()).await?;
        let replayed = match (history, runtime.saved_history()) {
            (History::Replay(client), Some(saved)) => {
                replay_display_history(&request.session_id, saved, client).await
            }
            // A session reopened new has nothing saved to replay.
            (History::Replay(_), None) | (History::Skip, _) => Ok(()),
        };
        if let Err(error) = replayed {
            runtime.shutdown().await;
            return Err(error);
        }
        Ok(Self::from_runtime(io, startup, runtime, t3_wake))
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
            modes: mode_state(self.runtime.permission_mode()),
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

fn mode_state(mode: PermissionMode) -> SessionModeState {
    SessionModeState::new(permission::mode_id(mode), permission::mode_list())
}

/// Sends `saved`'s display history to `client` as session `session_id`'s
/// replay.
async fn replay_display_history(
    session_id: &SessionId,
    saved: SavedHistory<'_>,
    client: &dyn AcpClientPort,
) -> anyhow::Result<()> {
    let rows = saved.display_rows()?;
    for notification in EventMapper::replay_history(session_id, &rows, saved.cwd(), saved.tools()) {
        send_notification(client, notification).await?;
    }
    Ok(())
}
