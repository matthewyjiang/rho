use super::*;
use crate::provider::{
    ANTHROPIC_API_KEY_ACCOUNT, CODEX_TOKENS_ACCOUNT, GITHUB_COPILOT_TOKENS_ACCOUNT,
    OPENAI_API_KEY_ACCOUNT,
};

#[test]
fn credential_account_names_are_stable() {
    assert_eq!(SERVICE, "rho");
    assert_eq!(OPENAI_API_KEY_ACCOUNT, "provider:openai:api-key");
    assert_eq!(ANTHROPIC_API_KEY_ACCOUNT, "provider:anthropic:api-key");
    assert_eq!(CODEX_TOKENS_ACCOUNT, "provider:openai-codex:tokens");
    assert_eq!(
        GITHUB_COPILOT_TOKENS_ACCOUNT,
        "provider:github-copilot:tokens"
    );
}

#[test]
fn chunk_secret_preserves_unicode_boundaries() {
    let chunks = chunk_secret_with_max("ab🙂cd", 2);

    assert_eq!(chunks, vec!["ab", "🙂", "cd"]);
    assert_eq!(chunks.concat(), "ab🙂cd");
}

#[test]
fn chunk_account_names_are_derived_from_stable_base_account() {
    assert_eq!(
        OsCredentialStore::chunk_manifest_account(CODEX_TOKENS_ACCOUNT),
        "provider:openai-codex:tokens:chunks"
    );
    assert_eq!(
        OsCredentialStore::chunk_account(CODEX_TOKENS_ACCOUNT, 3),
        "provider:openai-codex:tokens:chunk:3"
    );
    assert_eq!(
        OsCredentialStore::chunk_batch_account(CODEX_TOKENS_ACCOUNT, "abc", 3),
        "provider:openai-codex:tokens:chunk:abc:3"
    );
}

#[test]
fn chunk_manifest_supports_legacy_and_current_chunks() {
    assert_eq!(ChunkSet::parse("2").unwrap(), ChunkSet::Legacy { count: 2 });
    assert_eq!(
        ChunkSet::parse("v2:batch:3").unwrap(),
        ChunkSet::Current {
            batch_id: "batch".to_string(),
            count: 3,
        }
    );
}
