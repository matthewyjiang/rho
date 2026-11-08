//! What a client chooses for a session the daemon opens for it, through the
//! real binary and its socket: the agent it runs as and its setup, the
//! directories, MCP servers, and parts left out that `rho acp` takes per
//! session and `rho` takes as flags.

use std::{collections::BTreeSet, path::PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionAttach, SessionNew},
    Ask, Client, ClientError, DaemonNotification, ErrorCode, JobListParams, JobState, Operation,
    PathScope, SessionAttachParams, SessionEvent, SessionEventParams, SessionId, SessionNewParams,
    SessionPart, SessionSetup, TurnOutcome,
};

use super::sessions::{Sandbox, DEADLINE};

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
/// fixture answers with.
async fn inspected_request(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
) -> serde_json::Value {
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

// Covers: a session whose setup leaves parts out offers the model only what
// it keeps: without tools, none; without questions, every tool but the
// questionnaire; a session opened without that setup offers them all.
// Owner: daemon session setup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_offers_the_model_only_the_parts_its_setup_keeps() {
    let sandbox = Sandbox::new();
    // The fixture answers with the request it got.
    let client = sandbox.start_daemon("inspect").await;
    let mut offers = Vec::new();
    for without in [
        BTreeSet::new(),
        BTreeSet::from([SessionPart::Tools]),
        BTreeSet::from([SessionPart::Questions]),
    ] {
        let setup = SessionSetup {
            without,
            ..SessionSetup::default()
        };
        let session = new_session(&sandbox, &client, None, setup).await.unwrap();
        let request = inspected_request(&sandbox, &client, &session).await;
        let tools = request["tools"]
            .as_array()
            .expect("the request lists its tools");
        let asks = tools.iter().any(|tool| tool["name"] == "questionnaire");
        offers.push((!tools.is_empty(), asks));
    }

    // (offers tools, offers the questionnaire)
    assert_eq!(
        offers,
        vec![(true, true), (false, false), (true, false)],
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
