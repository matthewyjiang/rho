use super::*;
use crate::provider::{CODEX_TOKENS_ACCOUNT, GITHUB_COPILOT_TOKENS_ACCOUNT, XAI_TOKENS_ACCOUNT};

#[test]
fn token_debug_redacts_every_secret_field() {
    let codex = CodexTokens {
        access_token: "codex-access-secret".into(),
        refresh_token: Some("codex-refresh-secret".into()),
        id_token: Some("codex-id-secret".into()),
        account_id: Some("account".into()),
    };
    let xai = XaiTokens {
        access_token: "xai-access-secret".into(),
        refresh_token: Some("xai-refresh-secret".into()),
        expires_at_unix: Some(123),
        id_token: Some("xai-id-secret".into()),
    };

    let debug = format!("{codex:?} {xai:?}");
    for secret in [
        "codex-access-secret",
        "codex-refresh-secret",
        "codex-id-secret",
        "xai-access-secret",
        "xai-refresh-secret",
        "xai-id-secret",
    ] {
        assert!(!debug.contains(secret));
    }
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn web_search_api_keys_use_dedicated_accounts() {
    let store = MemoryCredentialStore::default();

    for (credential, key) in [
        (WebSearchCredential::OpenAi, "openai-search"),
        (WebSearchCredential::Exa, "exa-search"),
        (WebSearchCredential::Brave, "brave-search"),
    ] {
        save_web_search_api_key(&store, credential, key).unwrap();
        assert_eq!(
            load_web_search_api_key(&store, credential)
                .unwrap()
                .as_deref(),
            Some(key)
        );
        assert!(delete_web_search_api_key(&store, credential).unwrap());
    }
}

#[test]
fn provider_api_keys_round_trip_through_memory_store() {
    let store = MemoryCredentialStore::default();

    for (provider, key) in [
        ("openai", "sk-test"),
        ("anthropic", "sk-ant-test"),
        ("openrouter", "sk-or-test"),
        ("xai", "xai-test"),
    ] {
        assert_eq!(load_provider_api_key(&store, provider).unwrap(), None);
        save_provider_api_key(&store, provider, key).unwrap();
        assert_eq!(
            load_provider_api_key(&store, provider).unwrap().as_deref(),
            Some(key)
        );
        assert!(delete_provider_credentials(&store, provider).unwrap());
        assert_eq!(load_provider_api_key(&store, provider).unwrap(), None);
    }
}

#[test]
fn available_auth_modes_includes_anthropic_api_key() {
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "anthropic", "sk-ant-test").unwrap();

    assert!(available_auth_modes(&store).contains(&"anthropic-api-key".into()));
}

#[test]
fn malformed_oauth_tokens_are_not_available_auth() {
    let store = MemoryCredentialStore::default();
    store.set_secret(CODEX_TOKENS_ACCOUNT, "not-json").unwrap();
    store
        .set_secret(GITHUB_COPILOT_TOKENS_ACCOUNT, "not-json")
        .unwrap();
    store.set_secret(XAI_TOKENS_ACCOUNT, "not-json").unwrap();

    assert!(provider_has_stored_credentials(&store, "openai-codex").unwrap());
    assert!(provider_has_stored_credentials(&store, "github-copilot").unwrap());
    assert!(provider_has_stored_credentials(&store, "xai-oauth").unwrap());
    assert!(provider_has_credentials(&store, "openai-codex").is_err());
    assert!(provider_has_credentials(&store, "github-copilot").is_err());
    assert!(provider_has_credentials(&store, "xai-oauth").is_err());
}

#[test]
fn codex_tokens_round_trip_with_optional_fields() {
    let store = MemoryCredentialStore::default();
    let tokens = CodexTokens {
        access_token: "access".into(),
        refresh_token: Some("refresh".into()),
        id_token: Some("id".into()),
        account_id: Some("account".into()),
    };

    save_codex_tokens(&store, &tokens).unwrap();

    assert_eq!(load_codex_tokens(&store).unwrap(), Some(tokens));
}

#[test]
fn codex_tokens_allow_missing_optional_fields() {
    let store = MemoryCredentialStore::default();
    store
        .set_secret(CODEX_TOKENS_ACCOUNT, r#"{"access_token":"access"}"#)
        .unwrap();

    assert_eq!(
        load_codex_tokens(&store).unwrap(),
        Some(CodexTokens {
            access_token: "access".into(),
            refresh_token: None,
            id_token: None,
            account_id: None,
        })
    );
}

#[test]
fn xai_tokens_round_trip_with_optional_fields() {
    let store = MemoryCredentialStore::default();
    let tokens = XaiTokens {
        access_token: "access".into(),
        refresh_token: Some("refresh".into()),
        expires_at_unix: Some(2_500),
        id_token: Some("id".into()),
    };

    save_xai_tokens(&store, &tokens).unwrap();

    assert_eq!(load_xai_tokens(&store).unwrap(), Some(tokens));
    assert!(provider_has_credentials(&store, "xai-oauth").unwrap());
    assert!(available_auth_modes(&store).contains(&"xai-oauth".into()));
    assert!(delete_provider_credentials(&store, "xai-oauth").unwrap());
    assert_eq!(load_xai_tokens(&store).unwrap(), None);
}

#[test]
fn github_copilot_tokens_round_trip_with_cached_token_fields() {
    let store = MemoryCredentialStore::default();
    let tokens = GitHubCopilotTokens {
        github_access_token: "github-access".into(),
        github_refresh_token: Some("github-refresh".into()),
        github_expires_at_unix: Some(2_500),
        copilot_token: Some("copilot".into()),
        copilot_expires_at_unix: Some(2_000),
        copilot_refresh_after_unix: Some(1_500),
        copilot_token_endpoint: Some("https://api.github.com/copilot_internal/v2/token".into()),
        copilot_chat_endpoint: Some("https://api.githubcopilot.com/chat/completions".into()),
        copilot_models_endpoint: Some("https://api.githubcopilot.com/models".into()),
    };

    save_github_copilot_tokens(&store, &tokens).unwrap();

    assert_eq!(load_github_copilot_tokens(&store).unwrap(), Some(tokens));
    assert!(provider_has_credentials(&store, "github-copilot").unwrap());
    assert!(available_auth_modes(&store).contains(&"github-copilot".into()));
    assert!(delete_provider_credentials(&store, "github-copilot").unwrap());
    assert_eq!(load_github_copilot_tokens(&store).unwrap(), None);
}

#[test]
fn empty_github_copilot_env_override_is_not_active() {
    assert!(!provider_has_env_override_from(
        "github-copilot",
        |env_var| {
            assert_eq!(env_var, "GITHUB_COPILOT_TOKEN");
            Some(" \t\n ".into())
        }
    ));
    assert!(provider_has_env_override_from(
        "github-copilot",
        |env_var| {
            assert_eq!(env_var, "GITHUB_COPILOT_TOKEN");
            Some("copilot-token".into())
        }
    ));
}
