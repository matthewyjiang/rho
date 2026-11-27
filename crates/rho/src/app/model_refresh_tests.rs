// Covers: a failed fetch tells the client nothing the provider answered or
// the credential store holds, only rho's own words.
// Owner: model list refresh
#[test]
fn a_failed_fetch_reports_nothing_the_provider_sent() {
    use rho_providers::model::{ModelError, ProviderReportedErrorKind};

    const SECRET: &str = "sk-echoed-secret";
    for error in [
        ModelError::HttpStatus {
            status: http::StatusCode::UNAUTHORIZED,
            body: format!("bad key {SECRET}"),
            retry_after: None,
        },
        ModelError::InvalidResponse(format!("invalid type: string \"{SECRET}\"")),
        ModelError::RetryableInvalidResponse {
            error_type: "overloaded".into(),
            message: SECRET.into(),
        },
        ModelError::ProviderReported {
            kind: ProviderReportedErrorKind::InvalidResponse,
            error_type: "error".into(),
            message: SECRET.into(),
        },
        ModelError::Credentials(format!("expected value at {SECRET}")),
    ] {
        let told = super::failure(&error);
        assert!(!told.contains(SECRET), "{error:?} told the client {told:?}");
    }
}
