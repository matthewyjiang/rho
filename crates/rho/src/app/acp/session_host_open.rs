//! Opening ACP sessions: `session/new` builds a host whose file waits for
//! its first save, and `session/load` restores a saved one and replays its
//! display history to the client.

use std::path::Path;

use agent_client_protocol::schema::v1::{
    LoadSessionRequest, LoadSessionResponse, McpServer, NewSessionRequest, NewSessionResponse,
    SessionId,
};
use rho_sdk::{model::Message, SessionOptions};

use super::{
    agent,
    build::{build_session, mode_state, session_config},
    convert::{validate_session_cwd, workspace_cwd},
    persist::HostStorage,
    send_notification, split_assembly, EventMapper, SessionHost,
};
use crate::{
    app::{
        acp::{AcpClientPort, AcpStartup},
        interactive_runtime::startup::{prompt_cache_key, session_options_for_id},
    },
    config::Config,
    session::Session as StoredSession,
};

impl SessionHost {
    pub(in crate::app::acp) async fn create(
        startup: &AcpStartup,
        request: NewSessionRequest,
    ) -> anyhow::Result<(Self, NewSessionResponse)> {
        let cwd = validate_session_cwd(&request.cwd)?;
        let config = session_config(&startup.config, &request.mcp_servers);
        let host = Self::unsaved(
            startup,
            &config,
            cwd,
            rho_sdk::SessionId::new(),
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
        sdk_id: rho_sdk::SessionId,
        host_mcp_servers: Vec<McpServer>,
    ) -> anyhow::Result<Self> {
        let added_dirs = startup.added_dirs.clone();
        let assembly = build_session(startup, &startup.agent, config, cwd, &added_dirs, |_| {
            Ok(session_options_for_id(sdk_id))
        })
        .await?;
        let (built, permission) = split_assembly(assembly, config.permission_mode);
        let storage = HostStorage::deferred(
            cwd.to_path_buf(),
            built.session.id().to_string(),
            startup.agent.id().to_string(),
            startup.agent.fingerprint().to_string(),
        );
        Ok(Self::from_built(
            SessionId::new(built.session.id().as_str()),
            built,
            storage,
            added_dirs,
            permission,
            startup,
            host_mcp_servers,
        ))
    }

    /// Loads a saved session and replays its display history to `client`.
    ///
    /// A session created by `session/new` has no file until its first
    /// prompt, so an id with no saved session anywhere is loaded as that
    /// empty session, as it was when `session/new` wrote the file eagerly.
    pub(in crate::app::acp) async fn load(
        startup: &AcpStartup,
        request: LoadSessionRequest,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<(Self, LoadSessionResponse)> {
        let cwd = workspace_cwd(&request.cwd, &startup.cwd);
        if !request.cwd.as_os_str().is_empty() {
            validate_session_cwd(cwd)?;
        }
        let config = session_config(&startup.config, &request.mcp_servers);
        let (stored, histories) =
            match StoredSession::open_by_id_with_histories(cwd, request.session_id.0.as_ref()) {
                Ok(opened) => opened,
                Err(error) if error.is::<crate::session::NoSessionFound>() => {
                    let Ok(sdk_id) = rho_sdk::SessionId::from_string(request.session_id.0.as_ref())
                    else {
                        return Err(error);
                    };
                    let host =
                        Self::unsaved(startup, &config, cwd, sdk_id, request.mcp_servers).await?;
                    let response = LoadSessionResponse::new()
                        .modes(mode_state(host.permission.mode))
                        .config_options(host.config_options(&startup.config));
                    return Ok((host, response));
                }
                Err(error) => return Err(error),
            };
        // ACP has no notice channel at load, so a missing saved directory
        // is logged and skipped, matching the interactive notice.
        let root = std::fs::canonicalize(cwd)?;
        let restored = crate::added_dirs::AddedDirs::from_storage(&stored, &root);
        for dir in &restored.missing {
            tracing::warn!(dir = %dir.display(), "could not restore added directory: not found");
        }
        let added_dirs = startup.added_dirs.union(&root, &restored.dirs);
        let (agent, config) = agent::loaded_agent(startup, &stored, config, || {
            Ok(crate::agent::AgentCatalog::discover(cwd)?)
        })?;
        let assembly = build_session(startup, &agent, &config, cwd, &added_dirs, |provider| {
            let snapshot =
                stored.snapshot_for_resume(provider.identity(), prompt_cache_key(stored.id()))?;
            Ok(SessionOptions::from_snapshot(snapshot))
        })
        .await?;
        let (built, permission) = split_assembly(assembly, config.permission_mode);
        let replay_result =
            replay_display_history(&request.session_id, &histories.display, client).await;
        if let Err(error) = replay_result {
            built.teardown().await;
            return Err(error);
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
            request.session_id,
            built,
            HostStorage::saved(stored),
            added_dirs,
            permission,
            startup,
            request.mcp_servers,
        );
        host.load_notice = load_notice;
        host.agent = agent;
        let response = LoadSessionResponse::new()
            .modes(mode_state(host.permission.mode))
            .config_options(host.config_options(&startup.config));
        Ok((host, response))
    }
}

async fn replay_display_history(
    session_id: &SessionId,
    messages: &[Message],
    client: &dyn AcpClientPort,
) -> anyhow::Result<()> {
    for notification in EventMapper::replay_history(session_id, messages) {
        send_notification(client, notification).await?;
    }
    Ok(())
}
