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
fn refresh_merges_tokens_and_derives_expiry_from_expires_in() {
    for (case, response, expected) in [
        (
            "no expires_in clears stale expiry",
            XaiRefreshResponse {
                access_token: Some("new-access".into()),
                refresh_token: Some("new-refresh".into()),
                id_token: Some("new-id".into()),
                expires_in: None,
            },
            XaiTokens {
                access_token: "new-access".into(),
                refresh_token: Some("new-refresh".into()),
                expires_at_unix: None,
                id_token: Some("new-id".into()),
            },
        ),
        (
            "expires_in sets absolute expiry and keeps prior tokens",
            XaiRefreshResponse {
                access_token: Some("new-access".into()),
                refresh_token: None,
                id_token: None,
                expires_in: Some(3_600),
            },
            XaiTokens {
                access_token: "new-access".into(),
                refresh_token: Some("refresh".into()),
                expires_at_unix: Some(13_600),
                id_token: Some("old-id".into()),
            },
        ),
    ] {
        let refreshed =
            merge_refreshed_tokens(response, "refresh", &previous_tokens(), Some(10_000)).unwrap();

        assert_eq!(refreshed, expected, "{case}");
        if expected.expires_at_unix.is_none() {
            assert!(!token_is_expiring(&refreshed), "{case}");
        }
    }
}

#[test]
fn auth_material_debug_redacts_access_token() {
    let material = XaiAuthMaterial {
        access_token: "xai-secret-token".into(),
    };

    let debug = format!("{material:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("xai-secret-token"));
}
