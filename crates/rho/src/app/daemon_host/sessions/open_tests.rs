use pretty_assertions::assert_eq;
use rho_protocol::{ClientEnvironment, ErrorCode};
use rho_sdk::ProcessEnvironment;

use super::session_environment;

fn client(variables: &[(&str, &str)]) -> ClientEnvironment {
    variables
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

// Covers: a client's environment reaches a session's processes without its
// provider credentials, and a variable no process can be given fails the
// request (#1659).
// Owner: unit (daemon session environment)
#[test]
fn a_clients_environment_loses_credentials_and_refuses_what_no_process_takes() {
    let cases = [
        (
            "credentials",
            client(&[
                ("PATH", "/bin"),
                ("OPENAI_API_KEY", "secret"),
                ("RHO_MYHOST_API_KEY", "secret"),
            ]),
            Ok(client(&[("PATH", "/bin")])),
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
        let expected = expected.map(|variables| ProcessEnvironment::Explicit {
            variables: variables.into_iter().collect(),
        });
        assert_eq!(actual, expected.map_err(Some), "{name}");
    }
}
