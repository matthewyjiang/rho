use super::*;
use crate::credentials::CursorTokens;

fn tokens(expires_at_unix: Option<i64>) -> CursorTokens {
    CursorTokens {
        access_token: "access".into(),
        refresh_token: Some("refresh".into()),
        expires_at_unix,
    }
}

// Covers: stored Cursor tokens must refresh at or before the recorded expiry
// Owner: cursor oauth
#[test]
fn stored_tokens_expire_at_recorded_unix_time() {
    assert!(token_is_expiring(&tokens(Some(0))));
    assert!(!token_is_expiring(&tokens(Some(now_unix() + 3_600))));
    assert!(!token_is_expiring(&tokens(None)));
}
