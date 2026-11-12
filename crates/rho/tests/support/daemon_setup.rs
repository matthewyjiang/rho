//! What a client chooses for a session the daemon opens for it, through the
//! real binary and its socket: the agent it runs as and its setup, the
//! directories, MCP servers, parts left out, and host child threads that
//! `rho acp` takes per session and `rho` takes as flags.

use std::{collections::BTreeSet, path::PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionAttach, SessionDetach, SessionList, SessionNew},
    Ask, ChildThreads, Client, ClientError, DaemonNotification, ErrorCode, JobKind, JobListParams,
    JobState, Operation, PathScope, SessionAttachParams, SessionDetachParams, SessionEvent,
    SessionEventParams, SessionId, SessionListParams, SessionNewParams, SessionPart, SessionSetup,
    ToolCallId, ToolStatus, TurnOutcome,
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
            parent: None,
            grant: None,
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
    inspected_turn(sandbox, client, session, "hello").await.0
}

/// The model request of `session`'s turn of `text`, as [`inspected_request`]
/// gets it, with the events the turn sent before it ended.
async fn inspected_turn(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
    text: &str,
) -> (Value, Vec<SessionEvent>) {
    match turn(sandbox, client, session, text).await {
        (TurnOutcome::Answered { text, .. }, events) => {
            (serde_json::from_str(&text).unwrap(), events)
        }
        (outcome, _) => panic!("the turn answers: {outcome:?}"),
    }
}

/// How `session`'s turn of `text` ended, with the events it sent before.
/// It attaches first with no setup, so it reopens a session the daemon
/// unloaded as a prompt would.
async fn turn(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
    text: &str,
) -> (TurnOutcome, Vec<SessionEvent>) {
    let (follower, mut notifications) = sandbox.connect_following().await;
    let attached = follower
        .request::<SessionAttach>(&attach(sandbox, session))
        .await
        .unwrap();
    assert_eq!(sandbox.prompt_text(client, session, text).await, Ok(()));
    // A reset replays the last turn's end first; this prompt's comes after.
    let mut replayed = attached.replay_events;
    let mut events = Vec::new();
    loop {
        let notification = tokio::time::timeout(DEADLINE, notifications.recv())
            .await
            .expect("the daemon sends the session's next event")
            .expect("the connection stays open");
        let DaemonNotification::SessionEvent(SessionEventParams { event, .. }) = notification
        else {
            continue;
        };
        if replayed > 0 {
            replayed -= 1;
            continue;
        }
        if let SessionEvent::TurnEnded { outcome, .. } = event {
            return (outcome, events);
        }
        events.push(event);
    }
}

/// The names of the tools a model request offered.
fn offered_tools(request: &Value) -> BTreeSet<String> {
    request["tools"]
        .as_array()
        .expect("the request lists its tools")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
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

// Covers: `/agent:<name> <task>` in a daemon session rebinds the fresh
// session to that agent before its turn: the model gets the agent's
// instructions, which the default agent's lack, and the task alone, not the
// command, and a follower gets the agent's options before the turn's
// output (#1670).
// Owner: daemon session commands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_command_runs_its_task_as_that_agent() {
    let sandbox = Sandbox::new();
    let archivist = sandbox.isolated.home.join(".agents/agents/archivist.md");
    std::fs::create_dir_all(archivist.parent().unwrap()).unwrap();
    // Its own model, so the rebind changes the session's options.
    std::fs::write(
        &archivist,
        "---\ndescription: Fixture agent with its own model\nmodel-policy: select\nprovider: xai\nmodel: grok-child\n---\n\nArchive.\n",
    )
    .unwrap();
    // The fixture answers with the request it got.
    let client = sandbox.start_daemon("inspect").await;
    let session = new_session(&sandbox, &client, None, SessionSetup::default())
        .await
        .unwrap();
    let command = "/agent:archivist summarize the notes";
    let (request, events) = inspected_turn(&sandbox, &client, &session, command).await;

    let system = request["messages"][0]["System"]
        .as_str()
        .expect("the request has a system prompt");
    let prompt = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message.get("User").is_some())
        .cloned();
    // Which of the two comes first, as runs of either.
    let mut order: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::OptionsChanged { .. } => Some("options_changed"),
            SessionEvent::AgentMessageChunk { .. } => Some("agent_message_chunk"),
            _ => None,
        })
        .collect();
    order.dedup();
    order.truncate(2);
    assert_eq!(
        (system.contains("Archive."), prompt, order),
        (
            true,
            Some(serde_json::json!({"User": [{"Text": "summarize the notes"}]})),
            vec!["options_changed", "agent_message_chunk"],
        ),
        "events: {events:?}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session opened as a read-only agent that `/agent:` rebound to
// an agent with every tool keeps the read-only agent's ceiling after the
// daemon unloads it: the reopened turn offers the model the tools the
// rebound turn did, rather than the reopen binding the child under the
// default agent's tools.
// Owner: daemon session commands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebound_session_keeps_its_root_agents_tools_after_a_reopen() {
    let sandbox = Sandbox::new();
    // The fixture answers with the request it got.
    let client = sandbox
        .start_daemon_with("inspect", &["--unload-idle-after", "200ms"])
        .await;
    let explorer = Some("explorer");
    let baseline = new_session(&sandbox, &client, explorer, SessionSetup::default())
        .await
        .unwrap();
    let read_only = offered_tools(&inspected_request(&sandbox, &client, &baseline).await);
    assert!(
        !read_only.contains("write"),
        "explorer is read-only: {read_only:?}"
    );
    // Another session: `/agent:` rebinds only a session with no history.
    let session = new_session(&sandbox, &client, explorer, SessionSetup::default())
        .await
        .unwrap();
    let command = "/agent:worker summarize the notes";
    let rebound = offered_tools(&inspected_turn(&sandbox, &client, &session, command).await.0);
    settled_job(&sandbox, &client, &session).await;
    sandbox.wait_unloaded(&session, /*times*/ 1).await;

    let reopened = offered_tools(&inspected_request(&sandbox, &client, &session).await);

    assert_eq!(
        (
            rebound.difference(&read_only).cloned().collect::<Vec<_>>(),
            reopened,
        ),
        (Vec::new(), rebound.clone()),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: an MCP server a session's setup provides is started with the env
// the setup gives it, in place of the config server of the same name, which
// is never started.
// Owner: daemon session setup.
// Unix: the MCP servers are shell scripts.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_setups_mcp_server_replaces_the_config_server_of_its_name() {
    use std::os::unix::fs::PermissionsExt;

    use rho_protocol::McpServer;

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

// Covers: who runs a session's children follows the client that opened it,
// as its MCP servers do: a setup whose host runs them never falls back to a
// daemon subagent (without the host's `t3-code` task tools the `agent` call
// fails), and once the daemon unloaded it, a reopen that brings no setup
// runs them in the daemon instead of keeping a host it no longer has.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_threads_follow_the_client_that_opened_the_session() {
    let sandbox = Sandbox::new();
    // No approval stands between the call and its result.
    sandbox.set_permission_mode("bypass");
    // The fixture makes the call the prompt spells out as JSON.
    let client = sandbox
        .start_daemon_with("tool-call", &["--unload-idle-after", "200ms"])
        .await;
    let setup = SessionSetup {
        child_threads: ChildThreads::Host,
        ..SessionSetup::default()
    };
    let session = new_session(&sandbox, &client, None, setup).await.unwrap();
    let mut outcomes = Vec::new();
    for (unloads, id) in [(0, "spawn-new"), (1, "spawn-reopened")] {
        if unloads > 0 {
            settled_job(&sandbox, &client, &session).await;
            sandbox.wait_unloaded(&session, unloads).await;
        }
        let call = serde_json::json!({
            "id": id,
            "name": "agent",
            "arguments": {"agent_id": "worker", "prompt": "inspect the workspace"},
        });
        let (_, events) = turn(&sandbox, &client, &session, &call.to_string()).await;
        let id = ToolCallId::new(id);
        let status = events.iter().rev().find_map(|event| match event {
            SessionEvent::ToolCall(update) if update.id == id => update.status,
            _ => None,
        });
        let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
        let subagents = jobs
            .jobs
            .iter()
            .filter(|job| job.kind == JobKind::Subagent)
            .count();
        outcomes.push((status, subagents));
    }

    // The tool's status and the daemon subagent jobs after each turn.
    assert_eq!(
        outcomes,
        [
            (Some(ToolStatus::Failed), 0),
            (Some(ToolStatus::Completed), 1)
        ],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session opened without saving runs its turn and leaves no file,
// where one opened plainly is listed once its turn saved it; the daemon
// discards it once it sits idle, so a prompt finds no session; and a saved
// session cannot be reopened without saving.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_opened_without_saving_leaves_no_file() {
    let sandbox = Sandbox::new();
    let client = sandbox
        .start_daemon_with("fixed", &["--unload-idle-after", "200ms"])
        .await;
    let unsaved = SessionSetup {
        without: BTreeSet::from([SessionPart::Saving]),
        ..SessionSetup::default()
    };
    // Each prompted as it opens: one that sits unprompted is discarded.
    let mut opened = Vec::new();
    let mut answered = Vec::new();
    for setup in [SessionSetup::default(), unsaved.clone()] {
        let session = new_session(&sandbox, &client, None, setup).await.unwrap();
        let (outcome, _events) = turn(&sandbox, &client, &session, "hello").await;
        answered.push(matches!(outcome, TurnOutcome::Answered { .. }));
        opened.push(session);
    }
    let [kept, ephemeral] = <[SessionId; 2]>::try_from(opened).unwrap();
    let listed: Vec<_> = client
        .request::<SessionList>(&SessionListParams::default())
        .await
        .unwrap()
        .sessions
        .into_iter()
        .map(|listed| listed.session)
        .collect();
    // Unloaded, so the attach reopens it, which is when a setup applies.
    sandbox.wait_unloaded(&kept, /*times*/ 1).await;
    let (reopener, _notifications) = sandbox.connect_following().await;
    let reopened = reopener
        .request::<SessionAttach>(&SessionAttachParams {
            setup: unsaved,
            ..attach(&sandbox, &kept)
        })
        .await
        .map(|_| ())
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error.error_code(),
            error => panic!("session/attach failed outside the protocol: {error}"),
        });
    sandbox
        .poll_until("the daemon to discard the idle session", async || {
            sandbox
                .isolated
                .daemon_log()
                .lines()
                .any(|line| {
                    line.contains("discarded unsaved idle session")
                        && line.contains(ephemeral.as_str())
                })
                .then_some(())
        })
        .await;
    let prompted = sandbox.prompt(&client, &ephemeral).await;

    assert_eq!(
        (answered, listed, reopened, prompted),
        (
            vec![true, true],
            vec![kept],
            Err(Some(ErrorCode::InvalidParams)),
            Err(Some(ErrorCode::SessionNotFound)),
        ),
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
