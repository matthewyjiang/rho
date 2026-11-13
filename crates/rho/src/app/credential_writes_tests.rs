use http::StatusCode;
use pretty_assertions::assert_eq;

use super::{status_check, KeyCheck};

// Covers: what a failing models-endpoint status does to a key: 400 and 401
// refuse it, saying what failed; 403 saves it unchecked, since a key that
// may not list models may still run them; any other status saves it
// unchecked as unreachable (#1662).
// Owner: credential writes (API-key check).
#[test]
fn only_a_refusing_status_refuses_a_key() {
    let statuses = [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
    ];
    assert_eq!(
        statuses.map(|status| status_check(status, "OpenAI").map_err(|error| error.to_string())),
        [
            Err(
                "OpenAI refused the key: its models endpoint answered HTTP 400 Bad Request"
                    .to_owned()
            ),
            Err(
                "OpenAI refused the key: its models endpoint answered HTTP 401 Unauthorized"
                    .to_owned()
            ),
            Ok(KeyCheck::Forbidden),
            Ok(KeyCheck::Unreachable("HTTP 404 Not Found".into())),
        ]
    );
}
