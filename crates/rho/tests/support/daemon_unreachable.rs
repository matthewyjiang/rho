//! A daemon session whose provider cannot be reached: the connection is
//! refused, or its host does not resolve. Nothing streamed, so clients hear
//! no restarted response, and the turn ends with one error that names the
//! provider, where it was sent, and why it failed.

use std::net::{TcpListener, ToSocketAddrs};

use pretty_assertions::assert_eq;
use rho_protocol::{SessionEvent, TurnError, TurnErrorKind, TurnOutcome};
use rho_providers::credentials::{CredentialStore, FileCredentialStore};

use super::sessions::{Follower, Sandbox};

/// A host no resolver answers (RFC 6761).
const UNRESOLVABLE_HOST: &str = "rho-unreachable.invalid";

/// A sandbox whose daemon runs the custom `mock` provider at `base_url`.
async fn daemon_at(base_url: &str) -> Sandbox {
    let sandbox = Sandbox::new();
    std::fs::write(
        &sandbox.isolated.config_path,
        format!(
            r#"[model]
provider = "mock"
model = "mock-model"
auth = "mock-api-key"

[providers.custom.mock]
base_url = "{base_url}"

[web_search]
mode = "off"
"#
        ),
    )
    .unwrap();
    FileCredentialStore::with_rho_home(sandbox.isolated.rho_home())
        .unwrap()
        .set_secret("provider:mock:api-key", "test-key")
        .unwrap();
    let mut command = sandbox.rho("fixed");
    // The session must reach the configured host, not the fixture provider,
    // and directly: through the sandbox's offline proxy, every host refuses.
    command.env_remove("RHO_AUTOMATION_TEST_MODE");
    for proxy in ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"] {
        command.env_remove(proxy);
    }
    sandbox.spawn_daemon(command, &[]).await;
    sandbox
}

/// Prompts a new session on `sandbox`'s daemon and follows it until its
/// turn ends; the events its clients got.
async fn failed_turn(sandbox: &Sandbox) -> Vec<SessionEvent> {
    let client = sandbox.connect().await;
    let session = sandbox.new_session(&client).await;
    let (mut follower, _) = Follower::attach(sandbox, &session, Vec::new(), None).await;
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    follower
        .read_until(|event| matches!(event, SessionEvent::TurnEnded { .. }))
        .await;
    follower
        .events
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}

/// What clients were told: how many responses the provider restarted, and
/// how the turn ended.
fn told(events: &[SessionEvent]) -> (usize, Option<TurnOutcome>) {
    let restarted = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::ResponseDiscarded))
        .count();
    let ended = events.iter().find_map(|event| match event {
        SessionEvent::TurnEnded { outcome, .. } => Some(outcome.clone()),
        _ => None,
    });
    (restarted, ended)
}

fn failed(message: String) -> Option<TurnOutcome> {
    Some(TurnOutcome::Failed {
        error: TurnError {
            kind: TurnErrorKind::Provider,
            message,
        },
    })
}

// Covers: a provider whose connection is refused, or whose host does not
// resolve, ends the turn with one provider error naming the provider, its
// base URL, and the OS cause, after its retries, with no "restarted its
// response" notice for attempts that streamed nothing.
// Owner: daemon session turn failure, over the real socket and HTTP.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_provider_ends_the_turn_with_one_error() {
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    // The OS's own words for each failure, as the HTTP client gets them.
    let refused = std::net::TcpStream::connect(closed)
        .unwrap_err()
        .to_string();
    let unresolved = (UNRESOLVABLE_HOST, 80)
        .to_socket_addrs()
        .unwrap_err()
        .to_string();
    let refused_url = format!("http://{closed}/v1");
    let unresolved_url = format!("http://{UNRESOLVABLE_HOST}/v1");

    let (refusing, unresolving) = tokio::join!(daemon_at(&refused_url), daemon_at(&unresolved_url));
    let (refusing_events, unresolving_events) =
        tokio::join!(failed_turn(&refusing), failed_turn(&unresolving));

    let expected = |url: &str, cause: &str| {
        (
            0,
            failed(format!(
                "mock at {url}: provider failed: provider request failed to connect: {cause}"
            )),
        )
    };
    assert_eq!(
        told(&refusing_events),
        expected(&refused_url, &refused),
        "events: {refusing_events:#?}\nlog:\n{}",
        refusing.isolated.daemon_log()
    );
    assert_eq!(
        told(&unresolving_events),
        expected(&unresolved_url, &unresolved),
        "events: {unresolving_events:#?}\nlog:\n{}",
        unresolving.isolated.daemon_log()
    );
}
