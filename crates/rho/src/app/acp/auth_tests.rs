use agent_client_protocol::schema::v1::{AuthMethod, AuthMethodAgent, AuthMethodTerminal};
use pretty_assertions::assert_eq;
use rho_protocol::{AuthKind, AuthStatus, AuthSummary};

use super::methods;

fn summary(auth: &str, label: &str, kind: AuthKind) -> AuthSummary {
    AuthSummary {
        auth: auth.into(),
        provider: "p".into(),
        label: label.into(),
        kind,
        status: AuthStatus::SignedOut,
    }
}

fn terminal(auth: &str, label: &str) -> AuthMethod {
    AuthMethod::Terminal(
        AuthMethodTerminal::new(auth, label).args(vec!["--login".into(), auth.into()]),
    )
}

// Covers: an interactive login runs in the host only where it opens URLs,
// an API key never does (secrets must not travel in ACP messages), the
// terminal login is offered only to hosts with a terminal, and a kind this
// rho does not know is not offered at all
// Owner: ACP auth advertisement
#[test]
fn auth_methods_follow_what_the_host_can_do() {
    let auths = [
        summary("openai-codex", "ChatGPT", AuthKind::Login),
        summary("openai", "OpenAI API key", AuthKind::ApiKey),
        summary("future", "Future", AuthKind::Unknown),
    ];
    let cases = [
        (
            "urls and terminal",
            /*urls*/ true,
            /*terminal*/ true,
            vec![
                AuthMethod::Agent(AuthMethodAgent::new("openai-codex", "ChatGPT")),
                terminal("openai", "OpenAI API key"),
            ],
        ),
        (
            "terminal only",
            false,
            true,
            vec![
                terminal("openai-codex", "ChatGPT"),
                terminal("openai", "OpenAI API key"),
            ],
        ),
        (
            "urls only",
            true,
            false,
            vec![AuthMethod::Agent(AuthMethodAgent::new(
                "openai-codex",
                "ChatGPT",
            ))],
        ),
        ("neither", false, false, vec![]),
    ];
    for (label, urls, has_terminal, expected) in cases {
        assert_eq!(methods(&auths, urls, has_terminal), expected, "{label}");
    }
}
