use super::*;
use pretty_assertions::assert_eq;

fn previous_tokens() -> XaiTokens {
    XaiTokens {
        access_token: "expired-access".into(),
        refresh_token: Some("refresh".into()),
        expires_at_unix: Some(1_000),
        id_token: Some("old-id".into()),
    }
}

#[test]
fn refresh_without_expires_in_clears_stale_expiry() {
    let refreshed = merge_refreshed_tokens(
        XaiRefreshResponse {
            access_token: Some("new-access".into()),
            refresh_token: Some("new-refresh".into()),
            id_token: Some("new-id".into()),
            expires_in: None,
        },
        "refresh",
        &previous_tokens(),
        Some(10_000),
    )
    .unwrap();

    assert_eq!(
        refreshed,
        XaiTokens {
            access_token: "new-access".into(),
            refresh_token: Some("new-refresh".into()),
            expires_at_unix: None,
            id_token: Some("new-id".into()),
        }
    );
    assert!(!token_is_expiring(&refreshed));
}

#[test]
fn refresh_with_expires_in_sets_absolute_expiry() {
    let refreshed = merge_refreshed_tokens(
        XaiRefreshResponse {
            access_token: Some("new-access".into()),
            refresh_token: None,
            id_token: None,
            expires_in: Some(3_600),
        },
        "refresh",
        &previous_tokens(),
        Some(10_000),
    )
    .unwrap();

    assert_eq!(
        refreshed,
        XaiTokens {
            access_token: "new-access".into(),
            refresh_token: Some("refresh".into()),
            expires_at_unix: Some(13_600),
            id_token: Some("old-id".into()),
        }
    );
}
