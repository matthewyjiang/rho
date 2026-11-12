//! The requests a service answers once, with nothing sent after: which
//! method a request names, the session it is about, and the [`Service`]
//! handler that answers it. [`crate::server`] decodes each and runs it in
//! its own task.

use serde_json::Value;

use crate::{
    methods::{
        AuthChooseStore, AuthChooseStoreParams, AuthKey, AuthKeyParams, AuthList, AuthListParams,
        AuthLoginCancel, AuthLoginCancelParams, AuthLogout, AuthLogoutParams, ConfigRegister,
        ConfigRegisterParams, ConfigSchemaMethod, ConfigSchemaParams, ConfigSet, ConfigSetParams,
        JobList, JobListParams, Restart, RestartParams, RestartResponse, SessionAnswer,
        SessionAnswerParams, SessionCancel, SessionCancelParams, SessionConfigure,
        SessionConfigureParams, SessionDelete, SessionDeleteParams, SessionHold, SessionHoldParams,
        SessionId, SessionInspect, SessionInspectParams, SessionList, SessionListParams,
        SessionMcp, SessionMcpCatalogParams, SessionMcpCatalogRead, SessionMcpComplete,
        SessionMcpCompleteParams, SessionMcpParams, SessionMcpResourceParams,
        SessionMcpResourceRead, SessionNew, SessionNewParams, SessionProcesses,
        SessionProcessesParams, SessionPrompt, SessionPromptParams, SessionRename,
        SessionRenameParams, SessionSteer, SessionSteerParams, SessionUnsteer,
        SessionUnsteerParams, Status, Stop, WorktreePrune, WorktreePruneParams, AUTH_CHOOSE_STORE,
        AUTH_KEY, AUTH_LIST, AUTH_LOGIN_CANCEL, AUTH_LOGOUT, CONFIG_REGISTER, CONFIG_SCHEMA,
        CONFIG_SET, DAEMON_RESTART, DAEMON_STATUS, DAEMON_STOP, JOB_LIST, SESSION_ANSWER,
        SESSION_CANCEL, SESSION_CONFIGURE, SESSION_DELETE, SESSION_HOLD, SESSION_INSPECT,
        SESSION_LIST, SESSION_MCP, SESSION_MCP_CATALOG, SESSION_MCP_COMPLETE, SESSION_MCP_RESOURCE,
        SESSION_NEW, SESSION_PROCESSES, SESSION_PROMPT, SESSION_RENAME, SESSION_STEER,
        SESSION_UNSTEER, WORKTREE_PRUNE,
    },
    server::{decode_params, encode_result, Service},
    ErrorCode, RpcError, StatusParams, StopParams, StopResponse,
};

/// A post-handshake request whose params decoded, waiting for the service.
pub(crate) enum ServiceCall {
    Status,
    Stop,
    Restart,
    NewSession(SessionNewParams),
    PromptSession(SessionPromptParams),
    CancelSession(SessionCancelParams),
    SteerSession(SessionSteerParams),
    UnsteerSession(SessionUnsteerParams),
    AnswerSession(SessionAnswerParams),
    HoldSession(SessionHoldParams),
    ConfigureSession(SessionConfigureParams),
    SessionMcp(SessionMcpParams),
    SessionMcpCatalog(SessionMcpCatalogParams),
    SessionMcpResource(SessionMcpResourceParams),
    SessionMcpComplete(SessionMcpCompleteParams),
    SessionProcesses(SessionProcessesParams),
    InspectSession(SessionInspectParams),
    ListSessions(SessionListParams),
    RenameSession(SessionRenameParams),
    DeleteSession(SessionDeleteParams),
    ListJobs,
    PruneWorktrees(WorktreePruneParams),
    SaveAuthKey(AuthKeyParams),
    LogoutAuth(AuthLogoutParams),
    ListAuths(AuthListParams),
    CancelLogin(AuthLoginCancelParams),
    ChooseStore(AuthChooseStoreParams),
    ConfigSchema,
    RegisterConfig(ConfigRegisterParams),
    SetConfig(ConfigSetParams),
}

impl ServiceCall {
    /// Decodes a post-handshake request for the service, or refuses a method
    /// the protocol does not define.
    pub(crate) fn decode(method: &str, params: Option<Value>) -> Result<Self, RpcError> {
        match method {
            DAEMON_STATUS => decode_params::<Status>(params).map(|StatusParams {}| Self::Status),
            DAEMON_STOP => decode_params::<Stop>(params).map(|StopParams {}| Self::Stop),
            DAEMON_RESTART => {
                decode_params::<Restart>(params).map(|RestartParams {}| Self::Restart)
            }
            SESSION_NEW => decode_params::<SessionNew>(params).map(Self::NewSession),
            SESSION_PROMPT => decode_params::<SessionPrompt>(params).map(Self::PromptSession),
            SESSION_CANCEL => decode_params::<SessionCancel>(params).map(Self::CancelSession),
            SESSION_STEER => decode_params::<SessionSteer>(params).map(Self::SteerSession),
            SESSION_UNSTEER => decode_params::<SessionUnsteer>(params).map(Self::UnsteerSession),
            SESSION_ANSWER => decode_params::<SessionAnswer>(params).map(Self::AnswerSession),
            SESSION_HOLD => decode_params::<SessionHold>(params).map(Self::HoldSession),
            SESSION_CONFIGURE => {
                decode_params::<SessionConfigure>(params).map(Self::ConfigureSession)
            }
            SESSION_MCP => decode_params::<SessionMcp>(params).map(Self::SessionMcp),
            SESSION_MCP_CATALOG => {
                decode_params::<SessionMcpCatalogRead>(params).map(Self::SessionMcpCatalog)
            }
            SESSION_MCP_RESOURCE => {
                decode_params::<SessionMcpResourceRead>(params).map(Self::SessionMcpResource)
            }
            SESSION_MCP_COMPLETE => {
                decode_params::<SessionMcpComplete>(params).map(Self::SessionMcpComplete)
            }
            SESSION_PROCESSES => {
                decode_params::<SessionProcesses>(params).map(Self::SessionProcesses)
            }
            SESSION_INSPECT => decode_params::<SessionInspect>(params).map(Self::InspectSession),
            SESSION_LIST => decode_params::<SessionList>(params).map(Self::ListSessions),
            SESSION_RENAME => decode_params::<SessionRename>(params).map(Self::RenameSession),
            SESSION_DELETE => decode_params::<SessionDelete>(params).map(Self::DeleteSession),
            JOB_LIST => decode_params::<JobList>(params).map(|JobListParams {}| Self::ListJobs),
            WORKTREE_PRUNE => decode_params::<WorktreePrune>(params).map(Self::PruneWorktrees),
            AUTH_KEY => decode_params::<AuthKey>(params).map(Self::SaveAuthKey),
            AUTH_LOGOUT => decode_params::<AuthLogout>(params).map(Self::LogoutAuth),
            AUTH_LIST => decode_params::<AuthList>(params).map(Self::ListAuths),
            AUTH_LOGIN_CANCEL => decode_params::<AuthLoginCancel>(params).map(Self::CancelLogin),
            AUTH_CHOOSE_STORE => decode_params::<AuthChooseStore>(params).map(Self::ChooseStore),
            CONFIG_SCHEMA => decode_params::<ConfigSchemaMethod>(params)
                .map(|ConfigSchemaParams {}| Self::ConfigSchema),
            CONFIG_REGISTER => decode_params::<ConfigRegister>(params).map(Self::RegisterConfig),
            CONFIG_SET => decode_params::<ConfigSet>(params).map(Self::SetConfig),
            method => Err(RpcError::new(
                ErrorCode::MethodNotFound,
                format!("unknown method {method}"),
            )),
        }
    }

    /// The session the call is about, whose events published before its
    /// answer the connection sends first.
    pub(crate) fn session(&self) -> Option<&SessionId> {
        match self {
            Self::PromptSession(SessionPromptParams { session, .. })
            | Self::CancelSession(SessionCancelParams { session, .. })
            | Self::SteerSession(SessionSteerParams { session, .. })
            | Self::UnsteerSession(SessionUnsteerParams { session, .. })
            | Self::AnswerSession(SessionAnswerParams { session, .. })
            | Self::HoldSession(SessionHoldParams { session, .. })
            | Self::SessionMcp(SessionMcpParams { session, .. })
            | Self::SessionMcpCatalog(SessionMcpCatalogParams { session, .. })
            | Self::SessionMcpResource(SessionMcpResourceParams { session, .. })
            | Self::SessionMcpComplete(SessionMcpCompleteParams { session, .. })
            | Self::SessionProcesses(SessionProcessesParams { session, .. })
            | Self::InspectSession(SessionInspectParams { session, .. })
            | Self::ConfigureSession(SessionConfigureParams { session, .. })
            | Self::RenameSession(SessionRenameParams { session, .. }) => Some(session),
            Self::Status
            | Self::Stop
            | Self::Restart
            | Self::NewSession(_)
            | Self::ListSessions(_)
            | Self::DeleteSession(_)
            | Self::ListJobs
            | Self::PruneWorktrees(_)
            | Self::SaveAuthKey(_)
            | Self::LogoutAuth(_)
            | Self::ListAuths(_)
            | Self::CancelLogin(_)
            | Self::ChooseStore(_)
            | Self::ConfigSchema
            | Self::RegisterConfig(_)
            | Self::SetConfig(_) => None,
        }
    }

    /// Runs the call, returning its answer.
    pub(crate) async fn run<Svc: Service>(self, service: &Svc) -> Result<Value, RpcError> {
        match self {
            Self::Status => encode_result::<Status>(&service.status().await),
            Self::Stop => {
                service.stop().await;
                encode_result::<Stop>(&StopResponse {})
            }
            Self::Restart => {
                service.restart().await;
                encode_result::<Restart>(&RestartResponse {})
            }
            Self::NewSession(params) => {
                encode_result::<SessionNew>(&service.new_session(params).await?)
            }
            Self::PromptSession(params) => {
                encode_result::<SessionPrompt>(&service.prompt_session(params).await?)
            }
            Self::CancelSession(params) => {
                encode_result::<SessionCancel>(&service.cancel_session(params).await?)
            }
            Self::SteerSession(params) => {
                encode_result::<SessionSteer>(&service.steer_session(params).await?)
            }
            Self::UnsteerSession(params) => {
                encode_result::<SessionUnsteer>(&service.unsteer_session(params).await?)
            }
            Self::AnswerSession(params) => {
                encode_result::<SessionAnswer>(&service.answer_session(params).await?)
            }
            Self::HoldSession(params) => {
                encode_result::<SessionHold>(&service.hold_session(params).await?)
            }
            Self::ConfigureSession(params) => {
                encode_result::<SessionConfigure>(&service.configure_session(params).await?)
            }
            Self::SessionMcp(params) => {
                encode_result::<SessionMcp>(&service.session_mcp(params).await?)
            }
            Self::SessionMcpCatalog(params) => {
                encode_result::<SessionMcpCatalogRead>(&service.session_mcp_catalog(params).await?)
            }
            Self::SessionMcpResource(params) => encode_result::<SessionMcpResourceRead>(
                &service.session_mcp_resource(params).await?,
            ),
            Self::SessionMcpComplete(params) => {
                encode_result::<SessionMcpComplete>(&service.session_mcp_complete(params).await?)
            }
            Self::SessionProcesses(params) => {
                encode_result::<SessionProcesses>(&service.session_processes(params).await?)
            }
            Self::InspectSession(params) => {
                encode_result::<SessionInspect>(&service.inspect_session(params).await?)
            }
            Self::ListSessions(params) => {
                encode_result::<SessionList>(&service.list_sessions(params).await?)
            }
            Self::RenameSession(params) => {
                encode_result::<SessionRename>(&service.rename_session(params).await?)
            }
            Self::DeleteSession(params) => {
                encode_result::<SessionDelete>(&service.delete_session(params).await?)
            }
            Self::ListJobs => encode_result::<JobList>(&service.list_jobs().await),
            Self::PruneWorktrees(params) => {
                encode_result::<WorktreePrune>(&service.prune_worktrees(params).await?)
            }
            Self::SaveAuthKey(params) => {
                encode_result::<AuthKey>(&service.save_auth_key(params).await?)
            }
            Self::LogoutAuth(params) => {
                encode_result::<AuthLogout>(&service.logout_auth(params).await?)
            }
            Self::ListAuths(params) => {
                encode_result::<AuthList>(&service.list_auths(params).await?)
            }
            Self::CancelLogin(params) => {
                encode_result::<AuthLoginCancel>(&service.cancel_login(params).await?)
            }
            Self::ChooseStore(params) => {
                encode_result::<AuthChooseStore>(&service.choose_store(params).await?)
            }
            Self::ConfigSchema => {
                encode_result::<ConfigSchemaMethod>(&service.config_schema().await?)
            }
            Self::RegisterConfig(params) => {
                encode_result::<ConfigRegister>(&service.register_config(params).await?)
            }
            Self::SetConfig(params) => {
                encode_result::<ConfigSet>(&service.set_config(params).await?)
            }
        }
    }
}
