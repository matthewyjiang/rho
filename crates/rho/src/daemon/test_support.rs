//! A stand-in for the daemon host, so transport tests need no app code.

use std::sync::Arc;

use rho_protocol::{
    AuthKeyParams, AuthKeyResponse, AuthLogoutParams, AuthLogoutResponse, DaemonStatus, ErrorCode,
    JobListResponse, RpcError, Service, SessionAnswerParams, SessionAnswerResponse,
    SessionAttachParams, SessionAttachResponse, SessionCancelParams, SessionCancelResponse,
    SessionHoldParams, SessionHoldResponse, SessionNewParams, SessionNewResponse,
    SessionPromptParams, SessionPromptResponse, SessionSteerParams, SessionSteerResponse,
    Subscription, PROTOCOL_VERSION,
};
use rho_sdk::CancellationToken;

/// Answers `status` with [`test_status`] and, like the real host, stops the
/// daemon on `rho/daemon/stop` by cancelling its shutdown token. It hosts no
/// sessions.
pub(super) struct StubHost {
    shutdown: CancellationToken,
}

impl StubHost {
    pub(super) fn new(shutdown: &CancellationToken) -> Arc<Self> {
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
    }
}

impl Service for StubHost {
    async fn status(&self) -> DaemonStatus {
        test_status()
    }

    async fn stop(&self) {
        self.shutdown.cancel();
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

    async fn answer_session(
        &self,
        _: SessionAnswerParams,
    ) -> Result<SessionAnswerResponse, RpcError> {
        Err(no_sessions())
    }

    async fn hold_session(&self, _: SessionHoldParams) -> Result<SessionHoldResponse, RpcError> {
        Err(no_sessions())
    }

    async fn attach_session(
        &self,
        _: SessionAttachParams,
    ) -> Result<(SessionAttachResponse, Subscription), RpcError> {
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
