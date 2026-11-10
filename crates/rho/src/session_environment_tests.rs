use pretty_assertions::assert_eq;
use rho_sdk::ProcessEnvironment;

use super::{Ambient, SessionEnvironment, TrustMismatch};
use crate::workspace::{
    ProjectTrust, ProjectTrusts, TRUST_PROJECT_AGENTS_ENV, TRUST_PROJECT_HOOKS_ENV,
    TRUST_PROJECT_OPENAPI_ENV, TRUST_PROJECT_PLUGINS_ENV,
};

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

// Covers: a session keeps the project trust of the environment it opened
// with when a prompt brings another, in both directions, and a workflow
// run's snapshot takes that trust with the environment current then, not
// the trust of the client that prompted for the run (#1779).
// Owner: unit (session environment)
#[test]
fn a_session_keeps_its_opening_trust_through_prompts_and_snapshots() {
    let trusting = [(TRUST_PROJECT_AGENTS_ENV, "1"), ("PATH", "/trusting")];
    let untrusting = [("PATH", "/untrusting")];
    let trusted = ProjectTrusts {
        agents: ProjectTrust::Trusted,
        ..ProjectTrusts::UNTRUSTED
    };
    let cases = [
        ("trusting opener", &trusting[..], &untrusting[..], trusted),
        (
            "untrusting opener",
            &untrusting[..],
            &trusting[..],
            ProjectTrusts::UNTRUSTED,
        ),
    ];
    for (name, opening, prompting, expected) in cases {
        let session = SessionEnvironment::new(Ambient::client(opening));
        session.replace(Ambient::client(prompting));
        let run = session.snapshot();

        assert_eq!(
            (
                session.project_trust(),
                run.project_trust(),
                run.current().path(),
            ),
            (
                expected,
                expected,
                Some(prompting[prompting.len() - 1].1.into())
            ),
            "{name}"
        );
    }
}

// Covers: an environment's project trust is compared with the session's by
// what each grants, not by the variables' text, and a difference names the
// variables on each side, so a host can refuse a prompt it would otherwise
// run with trust its client did not grant, or without trust it did (#1779).
// Owner: unit (session environment)
#[test]
fn a_trust_mismatch_names_the_variables_only_one_side_grants() {
    const HOOKS: &str = TRUST_PROJECT_HOOKS_ENV;
    const AGENTS: &str = TRUST_PROJECT_AGENTS_ENV;
    const PLUGINS: &str = TRUST_PROJECT_PLUGINS_ENV;
    const OPENAPI: &str = TRUST_PROJECT_OPENAPI_ENV;
    let mismatch = |session_only: &[&'static str], other_only: &[&'static str]| {
        Some(TrustMismatch {
            session_only: session_only.to_vec(),
            other_only: other_only.to_vec(),
        })
    };
    let cases = [
        ("neither trusts", &[][..], &[("PATH", "/bin")][..], None),
        (
            "the same trust, other variables",
            &[(HOOKS, "1"), ("PATH", "/a")][..],
            &[(HOOKS, "1"), ("PATH", "/b")][..],
            None,
        ),
        (
            "a value that grants nothing is no trust",
            &[][..],
            &[(AGENTS, "true"), (PLUGINS, "0")][..],
            None,
        ),
        (
            "the other trusts less",
            &[(HOOKS, "1"), (AGENTS, "1"), (OPENAPI, "1")][..],
            &[(AGENTS, "1")][..],
            mismatch(&[HOOKS, OPENAPI], &[]),
        ),
        (
            "the other trusts more",
            &[][..],
            &[(PLUGINS, "1")][..],
            mismatch(&[], &[PLUGINS]),
        ),
        (
            "each trusts what the other does not",
            &[(HOOKS, "1"), (PLUGINS, "1")][..],
            &[(PLUGINS, "1"), (AGENTS, "1")][..],
            mismatch(&[HOOKS], &[AGENTS]),
        ),
    ];
    for (name, opening, prompting, expected) in cases {
        let session = SessionEnvironment::new(Ambient::client(opening));

        assert_eq!(
            session.trust_mismatch(&Ambient::client(prompting)),
            expected,
            "{name}"
        );
    }
}
