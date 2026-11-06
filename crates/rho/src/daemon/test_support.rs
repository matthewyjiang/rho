//! A stand-in for the daemon host, so transport tests need no app code.

use std::sync::Arc;

use rho_protocol::{
    DaemonStatus, ErrorCode, JobsListResponse, RpcError, Service, ThreadCancelParams,
    ThreadCancelResponse, ThreadCreateParams, ThreadCreateResponse, ThreadPromptParams,
    ThreadPromptResponse, ThreadSteerParams, ThreadSteerResponse, PROTOCOL_VERSION,
};
use rho_sdk::CancellationToken;

/// Answers `status` with [`test_status`] and, like the real host, stops the
/// daemon on `rho/daemon/stop` by cancelling its shutdown token. It hosts no
/// threads.
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

    async fn create_thread(&self, _: ThreadCreateParams) -> Result<ThreadCreateResponse, RpcError> {
        Err(no_threads())
    }

    async fn prompt_thread(&self, _: ThreadPromptParams) -> Result<ThreadPromptResponse, RpcError> {
        Err(no_threads())
    }

    async fn cancel_thread(&self, _: ThreadCancelParams) -> Result<ThreadCancelResponse, RpcError> {
        Err(no_threads())
    }

    async fn steer_thread(&self, _: ThreadSteerParams) -> Result<ThreadSteerResponse, RpcError> {
        Err(no_threads())
    }

    async fn list_jobs(&self) -> JobsListResponse {
        JobsListResponse { jobs: Vec::new() }
    }
}

fn no_threads() -> RpcError {
    RpcError::new(ErrorCode::ThreadNotFound, "the stub host hosts no threads")
}
