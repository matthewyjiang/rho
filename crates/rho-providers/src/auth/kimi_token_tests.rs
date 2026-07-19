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
fn refresh_threshold_uses_half_the_original_lifetime() {
    let tokens = tokens(Some(3_600), Some(now_unix() + 1_000));

    assert!(token_is_expiring(&tokens));
}

#[test]
fn refresh_threshold_has_a_five_minute_minimum() {
    let fresh = tokens(Some(600), Some(now_unix() + 400));
    let expiring = tokens(Some(600), Some(now_unix() + 200));

    assert!(!token_is_expiring(&fresh));
    assert!(token_is_expiring(&expiring));
}

#[test]
fn token_without_expiration_does_not_refresh() {
    assert!(!token_is_expiring(&tokens(None, None)));
}
