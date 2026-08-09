use pretty_assertions::assert_eq;
use rho_providers::credentials::MemoryCredentialStore;

use super::*;

fn stored_credentials(client_id: &str) -> StoredCredentials {
    serde_json::from_value(serde_json::json!({
        "client_id": client_id,
        "token_response": {
            "access_token": "access-token-value",
            "token_type": "bearer",
            "expires_in": 3600,
            "refresh_token": "refresh-token-value",
        },
        "granted_scopes": ["mcp"],
        "token_received_at": 1_700_000_000_u64,
        "issuer": "https://auth.example.com",
    }))
    .expect("stored credential fixture must parse")
}

// Failure mode: two MCP servers share one credential entry, so authorizing one
// silently replaces the other's tokens.
// Owner layer: MCP OAuth credential-store keying.
#[test]
fn each_server_identity_owns_its_own_account() {
    assert_eq!(account_name("docs"), "mcp-oauth:docs");
    assert_ne!(account_name("docs"), account_name("docs-staging"));
}

// Failure mode: a registration and its tokens do not survive the session, so
// every start re-registers the client and asks the user to log in again.
// Owner layer: the rmcp credential-store adapter over Rho's store.
#[tokio::test]
async fn credentials_round_trip_through_rhos_store() {
    let backing = Arc::new(MemoryCredentialStore::default());
    let store = McpOAuthCredentialStore::new("docs", backing.clone());

    assert!(store.load().await.unwrap().is_none());
    store.save(stored_credentials("client-1")).await.unwrap();

    let reopened = McpOAuthCredentialStore::new("docs", backing.clone());
    let loaded = reopened.load().await.unwrap().expect("credentials persist");
    assert_eq!(loaded.client_id, "client-1");
    assert_eq!(loaded.granted_scopes, vec!["mcp".to_string()]);

    reopened.clear().await.unwrap();
    assert!(store.load().await.unwrap().is_none());
}

// Failure mode: an entry written by an older layout makes the server
// permanently unusable instead of prompting one fresh login.
// Owner layer: the rmcp credential-store adapter's read path.
#[tokio::test]
async fn an_unreadable_entry_reads_as_absent_rather_than_failing() {
    let backing = Arc::new(MemoryCredentialStore::default());
    backing
        .set_secret(&account_name("docs"), "{ not json")
        .unwrap();

    let store = McpOAuthCredentialStore::new("docs", backing);
    assert!(store.load().await.unwrap().is_none());
}
