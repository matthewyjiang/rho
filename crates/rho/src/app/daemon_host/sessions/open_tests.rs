use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_protocol::{ClientEnvironment, ErrorCode};

use super::session_environment;
use crate::session_environment::Ambient;

fn client(variables: &[(&str, &str)]) -> ClientEnvironment {
    variables
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

// Covers: a session takes exactly its client's environment, credentials
// included for the allowlists that name them, and a variable no process can
// be given fails the request (#1659).
// Owner: unit (daemon session environment)
#[test]
fn a_clients_environment_is_taken_whole_unless_no_process_could_take_it() {
    let cases = [
        (
            "valid",
            client(&[("PATH", "/bin"), ("OPENAI_API_KEY", "secret")]),
            Ok(&[("PATH", "/bin"), ("OPENAI_API_KEY", "secret")][..]),
        ),
        (
            "empty name",
            client(&[("", "x")]),
            Err(ErrorCode::InvalidParams),
        ),
        (
            "= in name",
            client(&[("A=B", "x")]),
            Err(ErrorCode::InvalidParams),
        ),
        (
            "NUL in name",
            client(&[("A\0", "x")]),
            Err(ErrorCode::InvalidParams),
        ),
        (
            "NUL in value",
            client(&[("A", "x\0y")]),
            Err(ErrorCode::InvalidParams),
        ),
    ];
    for (name, environment, expected) in cases {
        let actual = session_environment(&environment).map_err(|error| error.error_code());
        let expected = expected.map(|variables| {
            Ambient::Client(Arc::new(
                variables
                    .iter()
                    .map(|(variable, value)| ((*variable).to_owned(), (*value).to_owned()))
                    .collect(),
            ))
        });
        assert_eq!(actual, expected.map_err(Some), "{name}");
    }
}
