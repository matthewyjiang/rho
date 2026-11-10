//! What a daemon takes from the client that starts it, through the real
//! binary (#1779): an allowed part of its environment and nothing else, so
//! each session's project trust is its own client's.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{SessionNew, SessionObserve},
    Client, ClientEnvironment, ClientError, ErrorCode, SessionId, SessionNewParams,
    SessionObserveParams, SessionSetup,
};
use serde_json::json;

use super::sessions::{settled_job, Sandbox};

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
// client that opened it, not of the client that started the daemon. On a
// daemon a trusting client started, a client without trust opens no session
// as a project agent and its session runs no project hook; a client with
// trust does both, and its session keeps its hooks when a client without
// trust prompts it, as the other stays without when a trusting one does.
// The trusted session's replay is still readable once no client's
// environment is there to trust the project.
// Owner: daemon session open.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_project_trust_is_its_opening_clients() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let project = |file: &str| {
        let path = sandbox.workspace.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        path
    };
    std::fs::write(project(".agents/agents/project-probe.md"), PROJECT_AGENT).unwrap();
    let ran = sandbox.workspace.parent().unwrap().join("project-hook.txt");
    std::fs::write(
        project(".rho/hooks.toml"),
        format!(
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
    )
    .unwrap();
    // The first client, which starts the daemon, trusts the project.
    let client = started_by_client(&sandbox, "tool-call", &TRUST).await;
    let clients = [("untrusting", &[][..]), ("trusting", &TRUST[..])];
    let call = json!({"id": "bash", "name": "bash", "arguments": {"command": "true"}});

    let mut opened_as_project_agent = Vec::new();
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
            sandbox
                .prompt_tool_call_with_environment(&client, &session, &call, environment)
                .await;
            settled_job(&sandbox, &client, &session).await;
        }
        opened_as_project_agent.push((opener, as_project_agent));
    }
    let saved = opened_as_project_agent[1].1.clone();
    // A daemon nobody's trust reached reads the saved session unloaded.
    let stopped = sandbox
        .rho("tool-call")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stopped.success(), "daemon stop: {stopped:?}");
    let client = started_by_client(&sandbox, "tool-call", &[]).await;
    let replayed = match &saved {
        Ok(session) => client
            .request::<SessionObserve>(&SessionObserveParams {
                session: session.clone(),
                after: None,
            })
            .await
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Err(code) => Err(format!("no session to replay: {code:?}")),
    };

    assert_eq!(
        (
            opened_as_project_agent
                .into_iter()
                .map(|(opener, opened)| (opener, opened.map(|_| ())))
                .collect::<Vec<_>>(),
            std::fs::read_to_string(&ran).unwrap_or_default(),
            replayed,
        ),
        (
            vec![
                ("untrusting", Err(Some(ErrorCode::SessionOpenFailed))),
                ("trusting", Ok(())),
            ],
            "trusting session, untrusting prompt\ntrusting session, trusting prompt\n".to_owned(),
            Ok(()),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
