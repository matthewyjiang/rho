use pretty_assertions::assert_eq;
use rho_protocol::{ErrorCode, RpcError};
use rho_providers::model::ModelError;

use super::open_failed;

// Covers: a session that cannot open for want of credentials must answer
// SignInRequired in Rho's words, never with the provider's response body,
// which can quote the request's key; any other failure keeps its causes.
// Owner: daemon session-open error mapping
#[test]
fn open_failures_say_why_without_quoting_the_provider() {
    let refused = ModelError::HttpStatus {
        status: http::StatusCode::UNAUTHORIZED,
        body: "invalid key: Bearer sk-secret".to_owned(),
        retry_after: None,
    };
    let cases = [
        (
            anyhow::Error::new(ModelError::MissingCredentials("missing mock API key")),
            ErrorCode::SignInRequired,
            "could not open session: missing mock API key",
        ),
        (
            anyhow::Error::new(refused).context("could not build the provider"),
            ErrorCode::SignInRequired,
            "could not open session: could not authenticate with the provider; sign in again or check the account's access",
        ),
        (
            anyhow::anyhow!("agent archivist changed"),
            ErrorCode::SessionOpenFailed,
            "could not open session: agent archivist changed",
        ),
    ];
    for (error, code, message) in cases {
        assert_eq!(open_failed(error), RpcError::new(code, message));
    }
}
