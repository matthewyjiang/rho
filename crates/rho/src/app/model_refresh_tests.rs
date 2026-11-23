use pretty_assertions::assert_eq;

// Covers: a refresh keeps the active auth when available, sends a stored key
// instead of probing anonymously, and otherwise falls back to an available
// mode.
// Owner: model list refresh
#[test]
fn model_refresh_selects_an_available_auth_mode() {
    for (case, provider, preferred, available, expected) in [
        (
            "active available auth wins",
            "openrouter",
            "openrouter-oauth",
            &["openrouter-api-key", "openrouter-oauth"][..],
            "openrouter-oauth",
        ),
        (
            "stored key beats keyless",
            "ollama",
            "none",
            &["none", "ollama-api-key"][..],
            "ollama-api-key",
        ),
        (
            "falls back to an available mode",
            "openrouter",
            "openrouter-api-key",
            &["openrouter-oauth"][..],
            "openrouter-oauth",
        ),
    ] {
        let descriptor = rho_providers::provider::provider_descriptor(provider).unwrap();
        let available = available
            .iter()
            .map(|auth| auth.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            super::refresh_auth_for_provider(descriptor, preferred, &available),
            expected,
            "{case}"
        );
    }
}

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
