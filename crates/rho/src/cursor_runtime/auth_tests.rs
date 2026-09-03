use pretty_assertions::assert_eq;

use super::*;

const AUTHENTICATED: &str = r#"{
    "status":"authenticated",
    "isAuthenticated":true,
    "hasAccessToken":true,
    "hasRefreshToken":true,
    "userInfo":{"email":"someone@example.com"}
}"#;

const UNAUTHENTICATED: &str = r#"{
    "status":"unauthenticated",
    "isAuthenticated":false,
    "hasAccessToken":false,
    "hasRefreshToken":false,
    "message":"Not logged in"
}"#;

// Covers: Cursor status JSON (signed in, signed out, garbage) must parse the
// load-bearing signed-in bit and email without requiring extra keys.
// Owner: cursor auth parse
#[test]
fn parses_status_json_bodies() {
    for (body, expected) in [
        (
            AUTHENTICATED,
            Some(CursorAuthStatus {
                status: "authenticated".into(),
                is_authenticated: true,
                message: None,
                user_info: Some(CursorUserInfo {
                    email: Some("someone@example.com".into()),
                }),
            }),
        ),
        (
            UNAUTHENTICATED,
            Some(CursorAuthStatus {
                status: "unauthenticated".into(),
                is_authenticated: false,
                message: Some("Not logged in".into()),
                user_info: None,
            }),
        ),
        ("not-json {", None),
    ] {
        let parsed = serde_json::from_str::<CursorAuthStatus>(body);
        match expected {
            Some(status) => assert_eq!(parsed.expect("valid status json"), status),
            None => assert!(parsed.is_err(), "garbage body must not parse: {body}"),
        }
    }
}
