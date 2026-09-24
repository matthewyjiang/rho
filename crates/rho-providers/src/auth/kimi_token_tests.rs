use super::*;

fn tokens(expires_in: Option<u64>, expires_at_unix: Option<i64>) -> KimiTokens {
    KimiTokens {
        access_token: "access".into(),
        refresh_token: Some("refresh".into()),
        expires_at_unix,
        scope: String::new(),
        token_type: "Bearer".into(),
        expires_in,
    }
}

#[test]
fn refresh_threshold_is_half_lifetime_with_five_minute_minimum() {
    let now = now_unix();
    for (case, expires_in, expires_at, expected) in [
        ("half of one hour", 3_600, now + 1_000, true),
        ("minimum keeps fresh", 600, now + 400, false),
        ("minimum marks expiring", 600, now + 200, true),
    ] {
        let tokens = tokens(Some(expires_in), Some(expires_at));
        assert_eq!(token_is_expiring(&tokens), expected, "{case}");
    }
}

#[test]
fn token_without_expiration_does_not_refresh() {
    assert!(!token_is_expiring(&tokens(None, None)));
}
