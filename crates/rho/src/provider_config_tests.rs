use super::*;

#[test]
fn config_normalizes_provider_profiles_for_top_level_and_internal_models() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"openrouter-oauth\"\nmodel = \"anthropic/claude-sonnet-4\"\nauth = \"openrouter-api-key\"\n[internal_agents.session-title]\nprovider = \"openrouter\"\nmodel = \"anthropic/claude-sonnet-4\"\nauth = \"openrouter-oauth\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(
        (config.provider.as_str(), config.auth.as_str()),
        ("openrouter", "openrouter-api-key")
    );
    let title = config.internal_agent_model("session-title").unwrap();
    assert_eq!(
        (title.provider.as_str(), title.auth.as_str()),
        ("openrouter-oauth", "openrouter-oauth")
    );
}

#[test]
fn config_canonicalizes_legacy_poolside_wire_model_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "provider = \"poolside\"\nmodel = \"poolside/laguna-m.1\"\nauth = \"poolside-api-key\"\n[internal_agents.session-title]\nprovider = \"poolside\"\nmodel = \"poolside/poolside/laguna-m.1\"\nauth = \"poolside-api-key\"\n",
    )
    .unwrap();

    let config = Config::load_with_store(
        path,
        &rho_providers::credentials::MemoryCredentialStore::default(),
    )
    .unwrap();

    assert_eq!(config.provider, "poolside");
    assert_eq!(config.model, "laguna-m.1");
    let title = config.internal_agent_model("session-title").unwrap();
    assert_eq!(title.provider, "poolside");
    assert_eq!(title.model, "laguna-m.1");
}
