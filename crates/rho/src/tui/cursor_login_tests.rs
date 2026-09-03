use pretty_assertions::assert_eq;

use super::{resolve_cursor_login_auth_outcome, CursorLoginAuthOutcome};
use crate::cursor_runtime::auth::CursorAuthStatus;

const AUTHENTICATED: &str = r#"{
    "status":"authenticated",
    "isAuthenticated":true,
    "userInfo":{"email":"someone@example.com"}
}"#;

const UNAUTHENTICATED: &str = r#"{
    "status":"unauthenticated",
    "isAuthenticated":false,
    "message":"Not logged in"
}"#;

// Covers: post-login auth JSON must become a signed-in notice or a not-signed-in error
// Owner: tui cursor login
#[tokio::test]
async fn cursor_login_outcome_records_email_or_not_signed_in() {
    let cases = [
        (
            AUTHENTICATED,
            CursorLoginAuthOutcome::Complete {
                notice: "signed in to cursor as someone@example.com".into(),
            },
        ),
        (
            UNAUTHENTICATED,
            CursorLoginAuthOutcome::Incomplete {
                message: "could not complete cursor login: not signed in".into(),
            },
        ),
    ];
    for (body, expected) in cases {
        let status: CursorAuthStatus = serde_json::from_str(body).expect("fixture json");
        let outcome =
            resolve_cursor_login_auth_outcome(Ok(()), || async { Ok(status.clone()) }).await;
        assert_eq!(outcome, expected, "{body}");
    }
}
