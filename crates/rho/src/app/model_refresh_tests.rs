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
