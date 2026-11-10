//! What a daemon takes from the client that starts it, through the real
//! binary (#1779): an allowed part of its environment and nothing else, so
//! each session's project trust is its own client's, and a prompt from a
//! client with other trust is refused. A session's credentials are its own
//! client's too: an API key in the starting client's environment signs in
//! nobody else.

use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{SessionNew, SessionObserve, SessionPrompt},
    Client, ClientEnvironment, ClientError, ContentBlock, ErrorCode, JobState,
    ProjectTrustMismatch, SessionEvent, SessionId, SessionNewParams, SessionObserveParams,
    SessionPromptParams, SessionSetup,
};
use serde_json::json;

use super::{
    key_checking_provider::spawn_provider,
    sessions::{is_state, settled_job, Follower, Sandbox},
};

/// Every variable that grants a workspace's project files.
const TRUST: [(&str, &str); 4] = [
    ("RHO_TRUST_PROJECT_HOOKS", "1"),
    ("RHO_TRUST_PROJECT_AGENTS", "1"),
    ("RHO_TRUST_PROJECT_PLUGINS", "1"),
    ("RHO_TRUST_PROJECT_OPENAPI", "1"),
];

/// An agent only the workspace defines, so only a client that trusts the
/// project can open a session as it.
const PROJECT_AGENT: &str = "---
description: Fixture agent the project defines
---

Review.
";

/// Where the workspace defines [`PROJECT_AGENT`], as agent `project-probe`.
const PROJECT_AGENT_FILE: &str = ".agents/agents/project-probe.md";

/// Writes `contents` to `file` in the sandbox's workspace.
fn write_project_file(sandbox: &Sandbox, file: &str, contents: &str) {
    let path = sandbox.workspace.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Has a client whose environment adds `extra` start the sandbox's daemon,
/// as any client does when none runs, and connects to it.
async fn started_by_client(sandbox: &Sandbox, mode: &str, extra: &[(&str, &str)]) -> Client {
    let started = sandbox
        .rho(mode)
        .envs(extra.iter().copied())
        .args(["daemon", "start"])
        .status()
        .unwrap();
    assert!(
        started.success(),
        "daemon start: {started:?}; log:\n{}",
        sandbox.isolated.daemon_log()
    );
    sandbox.connect().await
}

/// The sandbox's client environment with `extra` added.
fn environment_with(sandbox: &Sandbox, extra: &[(&str, &str)]) -> ClientEnvironment {
    sandbox
        .client_environment()
        .into_iter()
        .chain(
            extra
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        )
        .collect()
}

/// Why the daemon refused a prompt.
#[derive(Debug, PartialEq)]
enum Refusal {
    /// The prompting client's project trust is not the session's.
    TrustMismatch(ProjectTrustMismatch),
    Other(Option<ErrorCode>),
}

/// Prompts `session` with `text` from a client whose environment is
/// `environment`; returns once the turn saved the prompt, or why the daemon
/// refused it.
async fn prompt_as(
    client: &Client,
    session: &SessionId,
    text: &str,
    environment: ClientEnvironment,
) -> Result<(), Refusal> {
    let prompted = client
        .request::<SessionPrompt>(&SessionPromptParams {
            session: session.clone(),
            content: vec![ContentBlock::Text { text: text.into() }],
            timeout_ms: None,
            idempotency_key: None,
            environment,
        })
        .await;
    match prompted {
        Ok(_) => Ok(()),
        Err(ClientError::Rpc { error, .. }) => Err(match error.error_code() {
            Some(ErrorCode::ProjectTrustMismatch) => Refusal::TrustMismatch(
                serde_json::from_value(error.data.expect("a trust mismatch says what differs"))
                    .unwrap(),
            ),
            code => Refusal::Other(code),
        }),
        Err(error) => panic!("prompt failed outside the protocol: {error}"),
    }
}

/// Opens a session as `agent` for a client with `environment`; the error
/// code of a refused one.
async fn new_session(
    sandbox: &Sandbox,
    client: &Client,
    agent: Option<&str>,
    environment: ClientEnvironment,
) -> Result<SessionId, Option<ErrorCode>> {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment,
            agent: agent.map(str::to_owned),
            setup: SessionSetup::default(),
            parent: None,
        })
        .await
        .map(|opened| opened.session)
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code(),
            error => panic!("session/new failed outside the protocol: {error}"),
        })
}

// Covers: a daemon a client starts gets none of that client's variables but
// the allowed ones: not its project trust, nor one nobody listed, which a
// denylist would have passed on. Read from the daemon process itself.
// Owner: daemon auto-spawn.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_daemon_gets_only_the_allowed_part_of_its_starters_environment() {
    let sandbox = Sandbox::new();
    let unlisted = ("RHO_1779_UNLISTED", "from the starting client");
    let starter: Vec<_> = TRUST.into_iter().chain([unlisted]).collect();
    let _client = started_by_client(&sandbox, "fixed", &starter).await;

    let pid = sandbox.isolated.daemon_pid().expect("the daemon's record");
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    let names: Vec<&str> = environ
        .split(|byte| *byte == 0)
        .filter_map(|variable| std::str::from_utf8(variable).ok()?.split('=').next())
        .collect();
    let has = |name: &str| names.contains(&name);

    assert_eq!(
        (
            // The daemon's own, so this read the right process.
            ["RHO_HOME", "XDG_RUNTIME_DIR", "RHO_AUTOMATION_TEST_MODE"].map(has),
            starter
                .iter()
                .map(|(name, _)| has(name))
                .collect::<Vec<_>>(),
        ),
        ([true; 3], vec![false; starter.len()]),
        "the daemon's environment: {names:?}"
    );
}

// Covers: a session's project trust (`RHO_TRUST_PROJECT_*`) is that of the
// client that opened it, not of the client that started the daemon, and a
// prompt from a client with other trust is refused, naming the variables
// that differ, instead of run under the session's. On a daemon a trusting
// client started, a client without trust opens no session as a project
// agent and its session runs no project hook; a client with trust does
// both; and neither's session takes the other's prompt, so no project hook
// runs for a client that did not trust it.
// Owner: daemon session open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_project_trust_is_its_opening_clients() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    write_project_file(&sandbox, PROJECT_AGENT_FILE, PROJECT_AGENT);
    let ran = sandbox.workspace.parent().unwrap().join("project-hook.txt");
    write_project_file(
        &sandbox,
        ".rho/hooks.toml",
        &format!(
            r#"version = 1

[[hook]]
id = "probe"
on = "before_tool_use"
tools = ["bash"]
command = ["/bin/sh", "-c", {hook:?}]
env = ["PROBE"]
timeout = "10s"
"#,
            hook = format!(
                "printf '%s\\n' \"$PROBE\" >> {} && printf '{{\"version\": 1, \"decision\": \"continue\"}}'",
                ran.display()
            ),
        ),
    );
    // The first client, which starts the daemon, trusts the project.
    let client = started_by_client(&sandbox, "tool-call", &TRUST).await;
    let clients = [("untrusting", &[][..]), ("trusting", &TRUST[..])];
    let call = json!({"id": "bash", "name": "bash", "arguments": {"command": "true"}}).to_string();

    let mut opened_as_project_agent = Vec::new();
    let mut prompted = Vec::new();
    for (opener, trust) in clients {
        let as_project_agent = new_session(
            &sandbox,
            &client,
            Some("project-probe"),
            environment_with(&sandbox, trust),
        )
        .await;
        // The project agent's session where the client may open one, else
        // the default agent's.
        let session = match &as_project_agent {
            Ok(session) => session.clone(),
            Err(_) => new_session(&sandbox, &client, None, environment_with(&sandbox, trust))
                .await
                .unwrap(),
        };
        for (prompter, trust) in clients {
            let probe = format!("{opener} session, {prompter} prompt");
            let environment =
                environment_with(&sandbox, &[trust, &[("PROBE", probe.as_str())]].concat());
            let answer = prompt_as(&client, &session, &call, environment).await;
            settled_job(&sandbox, &client, &session).await;
            prompted.push((probe, answer));
        }
        opened_as_project_agent.push((opener, as_project_agent.map(|_| ())));
    }

    let every_variable = || TRUST.map(|(variable, _)| variable.to_owned()).to_vec();
    assert_eq!(
        (
            opened_as_project_agent,
            prompted,
            std::fs::read_to_string(&ran).unwrap_or_default(),
        ),
        (
            vec![
                ("untrusting", Err(Some(ErrorCode::SessionOpenFailed))),
                ("trusting", Ok(())),
            ],
            vec![
                ("untrusting session, untrusting prompt".to_owned(), Ok(())),
                (
                    "untrusting session, trusting prompt".to_owned(),
                    Err(Refusal::TrustMismatch(ProjectTrustMismatch {
                        session_only: Vec::new(),
                        client_only: every_variable(),
                    })),
                ),
                (
                    "trusting session, untrusting prompt".to_owned(),
                    Err(Refusal::TrustMismatch(ProjectTrustMismatch {
                        session_only: every_variable(),
                        client_only: Vec::new(),
                    })),
                ),
                ("trusting session, trusting prompt".to_owned(), Ok(())),
            ],
            "trusting session, trusting prompt\n".to_owned(),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a prompt refused for its client's project trust changes nothing
// of the session. A client that trusts less than the session may still
// attach to it and observe it; its prompt starts no turn, tells the
// attached clients nothing, and detaches none of them; and the session's
// own client prompts it next as if nothing was asked.
// Owner: daemon session prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_refused_for_its_trust_leaves_the_session_and_its_viewers_alone() {
    let sandbox = Sandbox::new();
    let client = started_by_client(&sandbox, "fixed", &[]).await;
    let trusting = environment_with(&sandbox, &TRUST);
    let session = new_session(&sandbox, &client, None, trusting.clone())
        .await
        .unwrap();
    // Attached from an environment without the session's trust.
    let (mut viewer, _) = Follower::attach(&sandbox, &session, Vec::new(), None).await;

    let refused = prompt_as(&client, &session, "hello", sandbox.client_environment()).await;
    let observed = client
        .request::<SessionObserve>(&SessionObserveParams {
            session: session.clone(),
            after: None,
        })
        .await
        .map(|observed| observed.state)
        .map_err(|error| error.to_string());
    let accepted = prompt_as(&client, &session, "hello", trusting).await;
    // Any word of the refused prompt, or a detach, would arrive first.
    viewer.read_until(is_state(JobState::Completed)).await;
    let (states, prompts): (Vec<_>, Vec<_>) = viewer
        .events
        .iter()
        .filter_map(|(_, event)| match event {
            SessionEvent::State { state, .. } => Some((Some(*state), None)),
            SessionEvent::UserMessage { .. } => Some((None, Some(()))),
            _ => None,
        })
        .unzip();

    assert_eq!(
        (
            refused.map_err(|refusal| matches!(refusal, Refusal::TrustMismatch(_))),
            observed,
            accepted,
            states.into_iter().flatten().collect::<Vec<_>>(),
            prompts.into_iter().flatten().count(),
        ),
        (
            Err(true),
            Ok(JobState::Idle),
            Ok(()),
            vec![JobState::Running, JobState::Completed],
            1,
        ),
        "events: {:#?}\nlog:\n{}",
        viewer.events,
        sandbox.isolated.daemon_log()
    );
}

// Covers: an unloaded session replays as the agent it was saved as. Observing
// carries no client environment, so no trust to find a project's agent
// with: a session saved as one still replays, and one saved as a built-in
// agent is not handed a project file of the same name that appeared since.
// Owner: daemon session observe.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unloaded_session_replays_as_the_agent_it_was_saved_as() {
    let sandbox = Sandbox::new();
    write_project_file(&sandbox, PROJECT_AGENT_FILE, PROJECT_AGENT);
    let client = started_by_client(&sandbox, "fixed", &[]).await;
    let mut saved = Vec::new();
    for (label, agent, trust) in [
        ("a project's agent", Some("project-probe"), &TRUST[..]),
        ("a built-in agent the project shadows since", None, &[][..]),
    ] {
        let session = new_session(&sandbox, &client, agent, environment_with(&sandbox, trust))
            .await
            .unwrap_or_else(|code| panic!("{label} opens: {code:?}"));
        // Its first prompt saves it.
        prompt_as(
            &client,
            &session,
            "hello",
            environment_with(&sandbox, trust),
        )
        .await
        .unwrap();
        settled_job(&sandbox, &client, &session).await;
        saved.push((label, session));
    }
    write_project_file(&sandbox, ".agents/agents/default.md", PROJECT_AGENT);
    // A new daemon has loaded nothing.
    let stopped = sandbox
        .rho("fixed")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stopped.success(), "daemon stop: {stopped:?}");
    let client = started_by_client(&sandbox, "fixed", &[]).await;

    let mut replayed = Vec::new();
    for (label, session) in saved {
        let observed = client
            .request::<SessionObserve>(&SessionObserveParams {
                session,
                after: None,
            })
            .await;
        replayed.push((
            label,
            observed.map(|_| ()).map_err(|error| error.to_string()),
        ));
    }

    assert_eq!(
        replayed,
        [
            ("a project's agent", Ok(())),
            ("a built-in agent the project shadows since", Ok(())),
        ],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// The variable that holds the key of the custom `mock` provider.
const KEY_VARIABLE: &str = "RHO_MOCK_API_KEY";

/// Points `sandbox`'s config at a provider that checks every key over real
/// HTTP and accepts only `key`, for the custom `mock` provider whose key
/// [`KEY_VARIABLE`] holds.
fn reach_provider_accepting(sandbox: &Sandbox, key: &str) {
    let address = spawn_provider(Arc::new(Mutex::new(key.to_owned())));
    std::fs::write(
        &sandbox.isolated.config_path,
        format!(
            r#"web_search.mode = "off"

[model]
provider = "mock"
model = "mock-model"
auth = "mock-api-key"

[providers.custom.mock]
base_url = "http://{address}/v1"

[behavior]
credential_store = "file"
"#
        ),
    )
    .unwrap();
}

/// Has a client whose environment adds `variables` start `sandbox`'s
/// daemon, outside fixture mode: its sessions reach the provider itself.
fn start_daemon_with(sandbox: &Sandbox, variables: &[(&str, &str)]) {
    let started = sandbox
        .rho("fixed")
        .env_remove("RHO_AUTOMATION_TEST_MODE")
        .envs(variables.iter().copied())
        .args(["daemon", "start"])
        .status()
        .unwrap();
    assert!(
        started.success(),
        "daemon start: {started:?}; log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session signs in with the credentials of the client that opens
// it, not of the client that started the daemon. On a daemon started by a
// client whose environment holds a key the provider refuses, that key
// reaches only that client's own sessions, whose turn the provider fails: a
// client with the key the provider accepts completes a turn, and a client
// with no key is told to sign in instead of borrowing the starter's (#1779).
// Owner: daemon session credentials.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_signs_in_with_its_opening_clients_key() {
    const STARTERS_KEY: &str = "test-starters-key";
    const OTHER_KEY: &str = "test-other-clients-key";
    let sandbox = Sandbox::new();
    reach_provider_accepting(&sandbox, OTHER_KEY);
    start_daemon_with(&sandbox, &[(KEY_VARIABLE, STARTERS_KEY)]);
    let client = sandbox.connect().await;

    let mut signed_in = Vec::new();
    for (opener, key) in [
        ("the starting client", Some(STARTERS_KEY)),
        ("a client with another key", Some(OTHER_KEY)),
        ("a client with no key", None),
    ] {
        let variables: Vec<_> = key.map(|key| (KEY_VARIABLE, key)).into_iter().collect();
        let environment = environment_with(&sandbox, &variables);
        let turn = match new_session(&sandbox, &client, None, environment.clone()).await {
            Ok(session) => match prompt_as(&client, &session, "hello", environment).await {
                Ok(()) => Ok(settled_job(&sandbox, &client, &session).await.state),
                Err(refusal) => Err(format!("prompt: {refusal:?}")),
            },
            Err(code) => Err(format!("open: {code:?}")),
        };
        signed_in.push((opener, turn));
    }

    assert_eq!(
        signed_in,
        [
            // The provider refused the starter's key, so it was the one sent.
            ("the starting client", Ok(JobState::Failed)),
            ("a client with another key", Ok(JobState::Completed)),
            (
                "a client with no key",
                Err(format!("open: {:?}", Some(ErrorCode::SignInRequired))),
            ),
        ],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: reading a saved session needs no sign-in. A session whose only
// credential was a key in its client's environment is replayed by a daemon
// that never saw that key, to an observer that sends no environment: it
// runs no turn, so it asks for no credential.
// Owner: daemon session observe.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unloaded_session_replays_without_its_clients_key() {
    const KEY: &str = "test-clients-key";
    let sandbox = Sandbox::new();
    reach_provider_accepting(&sandbox, KEY);
    start_daemon_with(&sandbox, &[]);
    let client = sandbox.connect().await;
    let environment = environment_with(&sandbox, &[(KEY_VARIABLE, KEY)]);
    let session = new_session(&sandbox, &client, None, environment.clone())
        .await
        .unwrap();
    // Its first prompt saves it.
    prompt_as(&client, &session, "hello", environment)
        .await
        .unwrap();
    let saved = settled_job(&sandbox, &client, &session).await.state;
    // A new daemon has loaded nothing.
    let stopped = sandbox
        .rho("fixed")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stopped.success(), "daemon stop: {stopped:?}");
    start_daemon_with(&sandbox, &[]);
    let client = sandbox.connect().await;

    let observed = client
        .request::<SessionObserve>(&SessionObserveParams {
            session,
            after: None,
        })
        .await;

    assert_eq!(
        (
            saved,
            observed.map(|_| ()).map_err(|error| error.to_string())
        ),
        (JobState::Completed, Ok(())),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
