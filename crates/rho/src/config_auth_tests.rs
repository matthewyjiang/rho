use pretty_assertions::assert_eq;

// Covers: the auth picked among those with credentials (for a session
// whose config names none, and for a model list refresh) keeps the
// preferred auth when available, takes a stored key over keyless access,
// and otherwise an available mode.
// Owner: config auth origin.
#[test]
fn the_auth_with_credentials_follows_the_selection_rules() {
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
            super::provider_auth_with_credentials(descriptor, preferred, &available),
            expected,
            "{case}"
        );
    }
}
