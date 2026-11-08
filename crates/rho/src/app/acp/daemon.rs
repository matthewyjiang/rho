//! The ACP errors for what the daemon connection (`app::daemon_client`)
//! reports: a daemon that could not be reached, a refused request, or a
//! connection that closed under a session.

use agent_client_protocol::Error as AcpError;
use rho_protocol::{ClientError, ErrorCode};

use crate::app::{daemon_cli::Unreachable, daemon_client::DaemonError};

impl From<Unreachable> for AcpError {
    fn from(error: Unreachable) -> Self {
        Self::internal_error().data(error.to_string())
    }
}

impl From<DaemonError> for AcpError {
    fn from(error: DaemonError) -> Self {
        match error {
            DaemonError::Unreachable(error) => error.into(),
            DaemonError::Request(error) => daemon_error(&error),
        }
    }
}

/// The ACP error for a daemon refusal.
pub(super) fn daemon_error(error: &ClientError) -> AcpError {
    let message = match error {
        ClientError::Rpc { error, .. } => error.message.clone(),
        other => other.to_string(),
    };
    let acp_error = match error.error_code() {
        Some(ErrorCode::InvalidParams | ErrorCode::SessionOpenFailed) => AcpError::invalid_params(),
        // As a failed turn's: the daemon's own message, which names how to
        // sign in and quotes neither a credential nor the provider, and no
        // data.
        Some(ErrorCode::SignInRequired) => {
            return AcpError::new(
                i32::from(agent_client_protocol::ErrorCode::AuthRequired),
                message,
            );
        }
        Some(ErrorCode::SessionNotFound | ErrorCode::SessionNotLoaded) => {
            AcpError::resource_not_found(None)
        }
        Some(
            ErrorCode::SessionBusy
            | ErrorCode::SessionInUse
            | ErrorCode::LoginBusy
            | ErrorCode::EnvironmentOverride
            | ErrorCode::InvalidRequest,
        ) => AcpError::invalid_request(),
        Some(ErrorCode::DaemonStopping) => {
            return AcpError::internal_error().data("rho daemon is stopping");
        }
        _ => AcpError::internal_error(),
    };
    acp_error.data(message)
}

/// Why a session's prompt failed when its connection closed under it.
pub(super) fn lost() -> AcpError {
    AcpError::internal_error().data("lost the connection to the rho daemon")
}
