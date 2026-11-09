//! A stand-in for the daemon host, so transport tests need no app code.

use std::sync::Arc;

use rho_protocol::{
    AuthChooseStoreParams, AuthChooseStoreResponse, AuthKeyParams, AuthKeyResponse,
    AuthListResponse, AuthLoginCancelParams, AuthLoginCancelResponse, AuthLoginParams,
    AuthLoginResponse, AuthLogoutParams, AuthLogoutResponse, CredentialStoreKind,
    CredentialStoreStatus, DaemonStatus, ErrorCode, JobListResponse, LoginStream, RpcError,
    Service, SessionAnswerParams, SessionAnswerResponse, SessionAttachParams,
    SessionAttachResponse, SessionCancelParams, SessionCancelResponse, SessionConfigureParams,
    SessionConfigureResponse, SessionDeleteParams, SessionDeleteResponse, SessionHoldParams,
    SessionHoldResponse, SessionListParams, SessionListResponse, SessionNewParams,
    SessionNewResponse, SessionPromptParams, SessionPromptResponse, SessionRenameParams,
    SessionRenameResponse, SessionSteerParams, SessionSteerResponse, SessionUnsteerParams,
    SessionUnsteerResponse, StoreSelectedBy, Subscription, PROTOCOL_VERSION,
};

use super::shutdown::{Ending, Shutdown};

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

    async fn list_auths(&self) -> Result<AuthListResponse, RpcError> {
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
