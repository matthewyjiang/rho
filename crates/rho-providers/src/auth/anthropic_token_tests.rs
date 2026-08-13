use super::*;
use pretty_assertions::assert_eq;

#[test]
fn refresh_without_expires_in_clears_stale_expiry() {
    let refreshed = merge_refreshed_tokens(
        AnthropicRefreshResponse {
            access_token: Some("new-access".into()),
            refresh_token: Some("new-refresh".into()),
            expires_in: None,
        },
        "refresh",
        Some(10_000),
    )
    .unwrap();

    assert_eq!(
        refreshed,
        AnthropicTokens {
            access_token: "new-access".into(),
            refresh_token: Some("new-refresh".into()),
            expires_at_unix: None,
        }
    );
    assert!(!token_is_expiring(&refreshed));
}

#[test]
fn refresh_with_expires_in_sets_absolute_expiry() {
    let refreshed = merge_refreshed_tokens(
        AnthropicRefreshResponse {
            access_token: Some("new-access".into()),
            refresh_token: None,
            expires_in: Some(3_600),
        },
        "refresh",
        Some(10_000),
    )
    .unwrap();

    assert_eq!(
        refreshed,
        AnthropicTokens {
            access_token: "new-access".into(),
            refresh_token: Some("refresh".into()),
            expires_at_unix: Some(13_600),
        }
    );
}

#[test]
fn auth_material_debug_redacts_access_token() {
    let material = AnthropicAuthMaterial {
        access_token: "anthropic-secret-token".into(),
    };

    let debug = format!("{material:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("anthropic-secret-token"));
}
