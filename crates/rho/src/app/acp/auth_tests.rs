use super::*;
use pretty_assertions::assert_eq;

// Covers: which typed failures ask the host to sign in, wherever they sit in
// the cause chain, before (startup) or after (prompt) the provider layer
// converts them for the SDK, and that the reply carries no credential or
// provider diagnostic. Other provider errors stay internal.
// Owner: ACP auth classification; hosts render only code and message.
#[test]
fn credential_failures_map_to_auth_required_without_diagnostics() {
    const SECRET: &str = "Authorization: Bearer sk-leaked";
    let refused = |status: u16| ModelError::HttpStatus {
        status: http::StatusCode::from_u16(status).unwrap(),
        body: SECRET.into(),
        retry_after: None,
    };
    let provider = |kind| {
        rho_sdk::Error::Provider(
            rho_sdk::ProviderError::new(kind, "HTTP 401", rho_sdk::Retryability::Permanent)
                .with_diagnostic(SECRET),
        )
    };
    let converted = |error| {
        rho_sdk::Error::Provider(
            rho_providers::providers::sdk_contract::provider_error_from_model_error(error),
        )
    };
    let missing = "missing OpenAI API key; run /login openai";
    let cases: Vec<(anyhow::Error, Option<CredentialFailure>)> = vec![
        (
            anyhow::Error::from(ModelError::MissingCredentials(missing))
                .context("could not start session"),
            Some(CredentialFailure::Missing(missing)),
        ),
        (refused(401).into(), Some(CredentialFailure::Refused)),
        (refused(403).into(), Some(CredentialFailure::Refused)),
        (refused(500).into(), None),
        (
            provider(rho_sdk::ProviderErrorKind::Authentication).into(),
            Some(CredentialFailure::Refused),
        ),
        (
            rho_sdk::Error::Authentication {
                message: SECRET.into(),
            }
            .into(),
            Some(CredentialFailure::Refused),
        ),
        (provider(rho_sdk::ProviderErrorKind::RateLimit).into(), None),
        (
            ModelError::credentials("keyring locked").into(),
            Some(CredentialFailure::Unreadable),
        ),
        // Prompt-time failures arrive already converted.
        (
            converted(ModelError::credentials("keyring locked")).into(),
            Some(CredentialFailure::Refused),
        ),
        (
            converted(refused(401)).into(),
            Some(CredentialFailure::Refused),
        ),
        (converted(refused(429)).into(), None),
        // Wording alone never classifies: only typed causes count.
        (anyhow::anyhow!("missing OpenAI API key; HTTP 401"), None),
    ];
    for (error, expected) in cases {
        let found = CredentialFailure::find(&error);
        assert_eq!(found, expected, "{error:#}");
        if let Some(failure) = found {
            let reply = serde_json::to_value(failure.into_acp()).unwrap();
            assert_eq!(reply["code"], -32000);
            assert_eq!(reply.get("data"), None);
            assert!(!reply.to_string().contains("sk-leaked"), "{reply}");
        }
    }
}
