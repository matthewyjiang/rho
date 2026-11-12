//! A stand-in for the daemon host, so transport tests need no app code.

use std::sync::Arc;

use rho_protocol::{
    AuthChooseStoreParams, AuthChooseStoreResponse, AuthKeyParams, AuthKeyResponse, AuthListParams,
    AuthListResponse, AuthLoginCancelParams, AuthLoginCancelResponse, AuthLoginParams,
    AuthLoginResponse, AuthLogoutParams, AuthLogoutResponse, BuildId, ConfigGetParams,
    ConfigRegisterParams, ConfigRegisterResponse, ConfigSchema, ConfigSetParams, ConfigSetResponse,
    ConfigWatch, CredentialStoreKind, CredentialStoreStatus, DaemonStatus, ErrorCode,
    JobListResponse, LoginStream, RpcError, Service, SessionAnswerParams, SessionAnswerResponse,
    SessionAttachParams, SessionAttachResponse, SessionCancelParams, SessionCancelResponse,
    SessionConfigureParams, SessionConfigureResponse, SessionDeleteParams, SessionDeleteResponse,
    SessionHoldParams, SessionHoldResponse, SessionListParams, SessionListResponse,
    SessionNewParams, SessionNewResponse, SessionPromptParams, SessionPromptResponse,
    SessionRenameParams, SessionRenameResponse, SessionSteerParams, SessionSteerResponse,
    SessionUnsteerParams, SessionUnsteerResponse, SettingsService, StoreSelectedBy, Subscription,
    PROTOCOL_VERSION,
};

use super::{
    paths::DaemonPaths,
    platform::Listener,
    shutdown::{Ending, Shutdown},
};

pub(crate) use super::platform::StopTarget;

/// A stand-in for a daemon that refuses every client: it listens where the
/// daemon of `paths` would, and answers a hello with a version mismatch.
pub(crate) struct RefusingListener(Listener);

impl RefusingListener {
    /// Listens on the endpoint of `paths`, preparing its runtime directory.
    pub(crate) fn bind(paths: &DaemonPaths) -> Self {
        paths.prepare_runtime_dir().unwrap();
        Self(Listener::bind(paths).unwrap())
    }

    /// Answers the next client's hello with `refusal`.
    pub(crate) async fn refuse_next(&mut self, refusal: rho_protocol::VersionMismatch) {
        use rho_protocol::{Message, MessageReader, MessageWriter, Response};

        let (read, write) = tokio::io::split(self.0.accept().await.unwrap());
        let hello = MessageReader::new(read).read_line().await.unwrap();
        let Message::Request(hello) = Message::parse(&hello.expect("a hello")).unwrap() else {
            panic!("expected a request");
        };
        let refused = RpcError::new(ErrorCode::VersionMismatch, refusal.to_string())
            .with_data(serde_json::to_value(refusal).unwrap());
        // The client may leave as soon as it has read the refusal.
        let _ = MessageWriter::new(write)
            .write(&Message::Response(Response {
                id: Some(hello.id),
                outcome: Err(refused),
            }))
            .await;
    }
}

/// Answers `status` with [`test_status`] and, like the real host, stops the
/// daemon on `rho/daemon/stop` by requesting its shutdown. It hosts no
/// sessions.
pub(super) struct StubHost {
    shutdown: Shutdown,
}

impl StubHost {
    pub(super) fn new(shutdown: &Shutdown) -> Arc<Self> {
        Arc::new(Self {
            shutdown: shutdown.clone(),
        })
    }
}

/// What restarting [`StubHost`] costs: nobody uses it.
const UNUSED: rho_protocol::RestartCost = rho_protocol::RestartCost {
    in_progress: 0,
    clients: 0,
};

/// What [`StubHost`] reports, distinct from any real daemon's status.
pub(super) fn test_status() -> DaemonStatus {
    DaemonStatus {
        pid: 0,
        version: "test".to_owned(),
        protocol: PROTOCOL_VERSION,
        started_at: 0,
        uptime_secs: 0,
        jobs: 0,
        socket: "/test/daemon.sock".into(),
        credential_store: CredentialStoreStatus {
            store: CredentialStoreKind::File,
            selected_by: StoreSelectedBy::Environment,
            unreachable: None,
        },
        upgrade: None,
    }
}

impl Service for StubHost {
    fn refused_newer(&self, _: rho_protocol::ProtocolVersion) -> rho_protocol::RestartCost {
        UNUSED
    }

    fn skewed(&self, _: &rho_protocol::ClientInfo, _: &BuildId) -> rho_protocol::RestartCost {
        UNUSED
    }

    async fn restart(&self) {
        self.shutdown.request(Ending::Restart, |_| {});
    }

    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {
        self.shutdown.request(Ending::Stop, |_| {});
    }

    async fn new_session(&self, _: SessionNewParams) -> Result<SessionNewResponse, RpcError> {
        Err(no_sessions())
    }

    async fn prompt_session(
        &self,
        _: SessionPromptParams,
    ) -> Result<SessionPromptResponse, RpcError> {
        Err(no_sessions())
    }

    async fn cancel_session(
        &self,
        _: SessionCancelParams,
    ) -> Result<SessionCancelResponse, RpcError> {
        Err(no_sessions())
    }

    async fn steer_session(&self, _: SessionSteerParams) -> Result<SessionSteerResponse, RpcError> {
        Err(no_sessions())
    }

    async fn unsteer_session(
        &self,
        _: SessionUnsteerParams,
    ) -> Result<SessionUnsteerResponse, RpcError> {
        Err(no_sessions())
    }

    async fn answer_session(
        &self,
        _: SessionAnswerParams,
    ) -> Result<SessionAnswerResponse, RpcError> {
        Err(no_sessions())
    }

    async fn hold_session(&self, _: SessionHoldParams) -> Result<SessionHoldResponse, RpcError> {
        Err(no_sessions())
    }

    async fn session_mcp(
        &self,
        _: rho_protocol::SessionMcpParams,
    ) -> Result<rho_protocol::SessionMcpInventory, RpcError> {
        Err(no_sessions())
    }

    async fn session_mcp_catalog(
        &self,
        _: rho_protocol::SessionMcpCatalogParams,
    ) -> Result<rho_protocol::SessionMcpCatalog, RpcError> {
        Err(no_sessions())
    }

    async fn session_mcp_resource(
        &self,
        _: rho_protocol::SessionMcpResourceParams,
    ) -> Result<rho_protocol::SessionMcpResourceResponse, RpcError> {
        Err(no_sessions())
    }

    async fn session_mcp_complete(
        &self,
        _: rho_protocol::SessionMcpCompleteParams,
    ) -> Result<rho_protocol::SessionMcpCompleteResponse, RpcError> {
        Err(no_sessions())
    }

    async fn observe_session(
        &self,
        _: rho_protocol::SessionObserveParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        Err(no_sessions())
    }

    async fn session_processes(
        &self,
        _: rho_protocol::SessionProcessesParams,
    ) -> Result<rho_protocol::SessionProcessesResponse, RpcError> {
        Err(no_sessions())
    }

    async fn inspect_session(
        &self,
        _: rho_protocol::SessionInspectParams,
    ) -> Result<rho_protocol::SessionInspectResponse, RpcError> {
        Err(no_sessions())
    }

    async fn configure_session(
        &self,
        _: SessionConfigureParams,
    ) -> Result<SessionConfigureResponse, RpcError> {
        Err(no_sessions())
    }

    async fn attach_session(
        &self,
        _: SessionAttachParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
        Err(no_sessions())
    }

    async fn list_sessions(&self, _: SessionListParams) -> Result<SessionListResponse, RpcError> {
        Ok(SessionListResponse {
            sessions: Vec::new(),
            next_cursor: None,
        })
    }

    async fn rename_session(
        &self,
        _: SessionRenameParams,
    ) -> Result<SessionRenameResponse, RpcError> {
        Err(no_sessions())
    }

    async fn delete_session(
        &self,
        _: SessionDeleteParams,
    ) -> Result<SessionDeleteResponse, RpcError> {
        Err(no_sessions())
    }

    async fn list_jobs(&self) -> JobListResponse {
        JobListResponse { jobs: Vec::new() }
    }

    async fn save_auth_key(&self, _: AuthKeyParams) -> Result<AuthKeyResponse, RpcError> {
        Err(no_credential_store())
    }

    async fn logout_auth(&self, _: AuthLogoutParams) -> Result<AuthLogoutResponse, RpcError> {
        Err(no_credential_store())
    }

    async fn list_auths(&self, _params: AuthListParams) -> Result<AuthListResponse, RpcError> {
        Err(no_credential_store())
    }

    async fn login(
        &self,
        _: AuthLoginParams,
    ) -> Result<(AuthLoginResponse, LoginStream), RpcError> {
        Err(no_credential_store())
    }

    async fn cancel_login(
        &self,
        _: AuthLoginCancelParams,
    ) -> Result<AuthLoginCancelResponse, RpcError> {
        Err(no_credential_store())
    }

    async fn choose_store(
        &self,
        _: AuthChooseStoreParams,
    ) -> Result<AuthChooseStoreResponse, RpcError> {
        Err(no_credential_store())
    }
}

impl SettingsService for StubHost {
    async fn config_schema(&self) -> Result<ConfigSchema, RpcError> {
        Err(no_settings())
    }

    async fn register_config(
        &self,
        _: ConfigRegisterParams,
    ) -> Result<ConfigRegisterResponse, RpcError> {
        Err(no_settings())
    }

    async fn get_config(&self, _: ConfigGetParams) -> Result<ConfigWatch, RpcError> {
        Err(no_settings())
    }

    async fn set_config(&self, _: ConfigSetParams) -> Result<ConfigSetResponse, RpcError> {
        Err(no_settings())
    }
}

fn no_settings() -> RpcError {
    RpcError::new(ErrorCode::MethodNotFound, "the stub keeps no settings")
}

fn no_sessions() -> RpcError {
    RpcError::new(
        ErrorCode::SessionNotFound,
        "the stub host hosts no sessions",
    )
}

fn no_credential_store() -> RpcError {
    RpcError::new(
        ErrorCode::CredentialStoreUnavailable,
        "the stub host has no credential store",
    )
}
