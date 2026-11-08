//! What a client chooses for a session the daemon opens for it, through the
//! real binary and its socket: the agent it runs as and its setup, the
//! directories, MCP servers, and parts left out that `rho acp` takes per
//! session and `rho` takes as flags.

use std::{collections::BTreeSet, path::PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionAttach, SessionDetach, SessionList, SessionNew},
    Ask, Client, ClientError, DaemonNotification, ErrorCode, JobListParams, JobState, McpServer,
    Operation, PathScope, SessionAttachParams, SessionDetachParams, SessionEvent,
    SessionEventParams, SessionId, SessionListParams, SessionNewParams, SessionPart, SessionSetup,
    TurnOutcome,
};
use serde_json::Value;

use super::sessions::{settled_job, Sandbox, ARCHIVIST_AGENT, DEADLINE};

/// A directory outside the sandbox's workspace, canonicalized as the
/// daemon resolves it: `../extra` from the workspace.
pub(super) fn extra_dir(sandbox: &Sandbox) -> PathBuf {
    let extra = sandbox.isolated.path().join("extra");
    std::fs::create_dir(&extra).unwrap();
    extra.canonicalize().unwrap()
}

/// Opens a new session in the workspace as `agent` with `setup`; the error
/// code of a refused one.
async fn new_session(
    sandbox: &Sandbox,
    client: &Client,
    agent: Option<&str>,
    setup: SessionSetup,
) -> Result<SessionId, Option<ErrorCode>> {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment: sandbox.client_environment(),
            agent: agent.map(str::to_owned),
            setup,
        })
        .await
        .map(|opened| opened.session)
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code(),
            error => panic!("session/new failed outside the protocol: {error}"),
        })
}

/// Attach params for `session` that bring nothing for it.
fn attach(sandbox: &Sandbox, session: &SessionId) -> SessionAttachParams {
    SessionAttachParams {
        session: session.clone(),
        after: None,
        environment: sandbox.client_environment(),
        setup: SessionSetup::default(),
    }
}

/// What `session`'s turn asks approval for, once the turn waits on it.
pub(super) async fn asked_operation(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
) -> Operation {
    sandbox
        .poll_until("the turn to ask", async || {
            let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
            jobs.jobs.into_iter().find(|job| {
                job.session.as_ref() == Some(session) && job.state == JobState::WaitingForInput
            })
        })
        .await;
    let (follower, _notifications) = sandbox.connect_following().await;
    let attached = follower
        .request::<SessionAttach>(&attach(sandbox, session))
        .await
        .unwrap();
    match attached
        .inputs
        .into_iter()
        .next()
        .map(|request| request.ask)
    {
        Some(Ask::Approval(approval)) => approval.operation,
        ask => panic!("the turn asks for approval: {ask:?}"),
    }
}

/// The model request of `session`'s turn of "hello", which the `inspect`
/// fixture answers with. It attaches first with no setup, so it reopens a
/// session the daemon unloaded as a prompt would.
async fn inspected_request(sandbox: &Sandbox, client: &Client, session: &SessionId) -> Value {
    let (follower, mut notifications) = sandbox.connect_following().await;
    follower
        .request::<SessionAttach>(&attach(sandbox, session))
        .await
        .unwrap();
    assert_eq!(sandbox.prompt(client, session).await, Ok(()));
    loop {
        let notification = tokio::time::timeout(DEADLINE, notifications.recv())
            .await
            .expect("the daemon sends the session's next event")
            .expect("the connection stays open");
        if let DaemonNotification::SessionEvent(SessionEventParams {
            event: SessionEvent::TurnEnded { outcome },
            ..
        }) = notification
        {
            match outcome {
                TurnOutcome::Answered { text, .. } => return serde_json::from_str(&text).unwrap(),
                outcome => panic!("the turn answers: {outcome:?}"),
            }
        }
    }
}

/// What a model request offered the model.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Offered {
    system_prompt: bool,
    tools: bool,
    questionnaire: bool,
    /// The `agent` tool, which starts subagents.
    agent: bool,
}

impl Offered {
    fn by(request: &Value) -> Self {
        let tools = request["tools"]
            .as_array()
            .expect("the request lists its tools");
        let offers = |name: &str| tools.iter().any(|tool| tool["name"] == name);
        Self {
            system_prompt: request["messages"][0].get("System").is_some(),
            tools: !tools.is_empty(),
            questionnaire: offers("questionnaire"),
            agent: offers("agent"),
        }
    }
}

// Covers: a session opened with an additional directory treats a path there
// as under a root its client granted, while a session opened without it
// sees the same path outside every root; the approval a write asks says
// which.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_takes_its_additional_directories() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("supervised");
    let extra = extra_dir(&sandbox);
    let note = extra.join("note.txt");
    // The fixture makes the call the prompt spells out as JSON.
    let client = sandbox.start_daemon("tool-call").await;
    let call = serde_json::json!({
        "id": "edit",
        "name": "write",
        "arguments": {"path": note, "content": "granted"},
    });
    let granted = SessionSetup {
        additional_directories: vec![extra.clone()],
        ..SessionSetup::default()
    };
    let mut asked = Vec::new();
    for setup in [granted, SessionSetup::default()] {
        let session = new_session(&sandbox, &client, None, setup).await.unwrap();
        sandbox.prompt_tool_call(&client, &session, &call).await;
        asked.push(asked_operation(&sandbox, &client, &session).await);
    }

    assert_eq!(
        asked,
        vec![
            Operation::WritePath {
                path: note.clone(),
                scope: PathScope::GrantedRoot {
                    root: extra.clone()
                },
            },
            Operation::WritePath {
                path: note,
                scope: PathScope::Unrestricted,
            },
        ],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session whose setup leaves a part out offers the model only
// what it keeps: without tools, none; without questions, every tool but the
// questionnaire; without subagents, every tool but `agent`; without the
// system prompt, no system prompt; a session opened without that setup
// offers them all.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_offers_the_model_only_the_parts_its_setup_keeps() {
    let sandbox = Sandbox::new();
    // The fixture answers with the request it got.
    let client = sandbox.start_daemon("inspect").await;
    let all = Offered {
        system_prompt: true,
        tools: true,
        questionnaire: true,
        agent: true,
    };
    let rows = [
        (None, all),
        (
            Some(SessionPart::Tools),
            Offered {
                tools: false,
                questionnaire: false,
                agent: false,
                ..all
            },
        ),
        (
            Some(SessionPart::Questions),
            Offered {
                questionnaire: false,
                ..all
            },
        ),
        (
            Some(SessionPart::Subagents),
            Offered {
                agent: false,
                ..all
            },
        ),
        (
            Some(SessionPart::SystemPrompt),
            Offered {
                system_prompt: false,
                ..all
            },
        ),
    ];
    let mut offered = Vec::new();
    for (without, _) in &rows {
        let setup = SessionSetup {
            without: without.iter().copied().collect(),
            ..SessionSetup::default()
        };
        let session = new_session(&sandbox, &client, None, setup).await.unwrap();
        offered.push((
            *without,
            Offered::by(&inspected_request(&sandbox, &client, &session).await),
        ));
    }

    assert_eq!(offered, rows, "log:\n{}", sandbox.isolated.daemon_log());
}

// Covers: a session's setup survives the daemon unloading it: a reopen
// that brings no setup still offers the model no tools, and its file still
// saves the additional directory, rather than the reopen widening the
// session or saving it without the directory (#1661).
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_keeps_its_setup_when_a_reopen_brings_none() {
    let sandbox = Sandbox::new();
    let extra = extra_dir(&sandbox);
    // The fixture answers with the request it got.
    let client = sandbox
        .start_daemon_with("inspect", &["--unload-idle-after", "200ms"])
        .await;
    let setup = SessionSetup {
        additional_directories: vec![extra.clone()],
        without: BTreeSet::from([SessionPart::Tools]),
        ..SessionSetup::default()
    };
    let session = new_session(&sandbox, &client, None, setup).await.unwrap();
    // Saved, so the unload has a file to reopen.
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    settled_job(&sandbox, &client, &session).await;
    sandbox.wait_unloaded(&session, /*times*/ 1).await;

    let reopened = Offered::by(&inspected_request(&sandbox, &client, &session).await);
    let saved_dirs = client
        .request::<SessionList>(&SessionListParams::default())
        .await
        .unwrap()
        .sessions
        .into_iter()
        .find(|listed| listed.session == session)
        .map(|listed| listed.added_dirs);

    assert_eq!(
        (reopened.tools, saved_dirs),
        (false, Some(vec![extra])),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a reopen whose setup leaves a part out narrows the session for
// good: it saves the session at once, so after the daemon unloads it again
// a reopen that brings no setup still offers the model no tools, rather
// than the narrowing living only until the unload (#1661).
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reopen_that_narrows_a_session_saves_it_narrowed() {
    let sandbox = Sandbox::new();
    // The fixture answers with the request it got.
    let client = sandbox
        .start_daemon_with("inspect", &["--unload-idle-after", "200ms"])
        .await;
    let session = new_session(&sandbox, &client, None, SessionSetup::default())
        .await
        .unwrap();
    // Saved, so the unload has a file to reopen.
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    settled_job(&sandbox, &client, &session).await;
    sandbox.wait_unloaded(&session, /*times*/ 1).await;
    let narrowing = SessionAttachParams {
        setup: SessionSetup {
            without: BTreeSet::from([SessionPart::Tools]),
            ..SessionSetup::default()
        },
        ..attach(&sandbox, &session)
    };
    let (follower, _notifications) = sandbox.connect_following().await;
    follower.request::<SessionAttach>(&narrowing).await.unwrap();
    follower
        .request::<SessionDetach>(&SessionDetachParams {
            session: session.clone(),
        })
        .await
        .unwrap();
    sandbox.wait_unloaded(&session, /*times*/ 2).await;

    assert_eq!(
        Offered::by(&inspected_request(&sandbox, &client, &session).await),
        Offered {
            system_prompt: true,
            tools: false,
            questionnaire: false,
            agent: false,
        },
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session opened as an agent other than the default runs as that
// agent: the model's system prompt carries the agent's instructions, which
// the default agent's lacks.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_runs_as_the_agent_its_client_names() {
    let sandbox = Sandbox::new();
    let archivist = sandbox.isolated.home.join(".agents/agents/archivist.md");
    std::fs::create_dir_all(archivist.parent().unwrap()).unwrap();
    std::fs::write(&archivist, ARCHIVIST_AGENT).unwrap();
    // The fixture answers with the request it got.
    let client = sandbox.start_daemon("inspect").await;
    let mut instructed = Vec::new();
    for agent in [None, Some("archivist")] {
        let session = new_session(&sandbox, &client, agent, SessionSetup::default())
            .await
            .unwrap();
        let request = inspected_request(&sandbox, &client, &session).await;
        let system = request["messages"][0]["System"]
            .as_str()
            .expect("the request has a system prompt");
        instructed.push((agent, system.contains("Archive.")));
    }

    assert_eq!(
        instructed,
        [(None, false), (Some("archivist"), true)],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: an MCP server a session's setup provides is started with the env
// the setup gives it, in place of the config server of the same name, which
// is never started.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_setups_mcp_server_replaces_the_config_server_of_its_name() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let root = sandbox.workspace.parent().unwrap().to_path_buf();
    let record = root.join("mcp.txt");
    // A server that records its name and `TOKEN`, then exits: the session
    // goes on without its tools.
    let install = |name: &str| {
        let path = root.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nprintf '%s|%s\\n' {name} \"${{TOKEN-unset}}\" >> {}\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let configured = install("config-probe").display().to_string();
    let provided = install("setup-probe");
    sandbox.set_config(
        "supervised",
        &format!("[mcp.servers.probe]\ntransport = \"stdio\"\ncommand = {configured:?}\n"),
    );
    let client = sandbox.start_daemon("fixed").await;
    let setup = SessionSetup {
        mcp_servers: vec![McpServer::Stdio {
            name: "probe".into(),
            command: provided,
            args: Vec::new(),
            env: [("TOKEN".to_owned(), "token-setup".to_owned())]
                .into_iter()
                .collect(),
        }],
        ..SessionSetup::default()
    };
    let session = new_session(&sandbox, &client, None, setup).await.unwrap();
    assert_eq!(sandbox.prompt(&client, &session).await, Ok(()));
    settled_job(&sandbox, &client, &session).await;

    assert_eq!(
        std::fs::read_to_string(&record).unwrap_or_default(),
        "setup-probe|token-setup\n",
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: `rho/session/new` fails, registering no job, when its client asks
// for what the session cannot have: an additional directory that is not an
// absolute path is invalid params, and an agent the workspace does not
// define fails to open.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_the_daemon_cannot_set_up_does_not_open() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let relative = SessionSetup {
        additional_directories: vec!["shared-lib".into()],
        ..SessionSetup::default()
    };
    let refused = [
        new_session(&sandbox, &client, None, relative).await,
        new_session(
            &sandbox,
            &client,
            Some("no-such-agent"),
            SessionSetup::default(),
        )
        .await,
    ];
    let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();

    assert_eq!(
        (refused, jobs.jobs),
        (
            [
                Err(Some(ErrorCode::InvalidParams)),
                Err(Some(ErrorCode::SessionOpenFailed)),
            ],
            Vec::new(),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
