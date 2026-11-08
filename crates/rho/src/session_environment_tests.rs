use pretty_assertions::assert_eq;
use rho_sdk::ProcessEnvironment;

use super::{Ambient, SessionEnvironment};

fn explicit(variables: &[(&str, &str)]) -> ProcessEnvironment {
    ProcessEnvironment::Explicit {
        variables: variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

// Covers: a client's environment reaches a session's processes without its
// provider credentials, and with every other variable (#1659).
// Owner: unit (session environment)
#[test]
fn a_clients_process_environment_loses_only_provider_credentials() {
    let cases = [
        (
            "credentials only",
            Ambient::client(&[
                ("OPENAI_API_KEY", "secret"),
                ("RHO_MYHOST_API_KEY", "secret"),
            ]),
            explicit(&[]),
        ),
        (
            "mixed",
            Ambient::client(&[
                ("PATH", "/bin"),
                ("OPENAI_API_KEY", "secret"),
                ("SSH_AUTH_SOCK", "/tmp/agent"),
                ("RHO_MYHOST_API_KEY", "secret"),
            ]),
            explicit(&[("PATH", "/bin"), ("SSH_AUTH_SOCK", "/tmp/agent")]),
        ),
    ];
    for (name, ambient, expected) in cases {
        assert_eq!(ambient.process_environment(), expected, "{name}");
    }
}

// Covers: a prompt's replacement reaches every clone of a session's
// environment, both what MCP servers and hooks read and what process tools
// start with, so the two never disagree (#1659).
// Owner: unit (session environment)
#[test]
fn a_replacement_reaches_every_clone_and_the_process_handle() {
    let environment = SessionEnvironment::new(Ambient::client(&[("PATH", "/old")]));
    let clone = environment.clone();
    let process = environment.process();

    environment.replace(Ambient::client(&[
        ("PATH", "/new"),
        ("OPENAI_API_KEY", "secret"),
    ]));

    assert_eq!(
        (clone.current(), process.current()),
        (
            Ambient::client(&[("PATH", "/new"), ("OPENAI_API_KEY", "secret")]),
            explicit(&[("PATH", "/new")]),
        )
    );
}
