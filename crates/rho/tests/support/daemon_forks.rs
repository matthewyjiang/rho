//! Forks of a saved session (`rho/session/fork`), through the real binary
//! and its socket: a side conversation that starts from the session's
//! conversation with the same tools, opens supervised, and belongs to the
//! session it forks.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{SessionAttach, SessionConfigure, SessionDelete, SessionFork, SessionList},
    Client, ClientError, DaemonNotification, DetachReason, ErrorCode, Notifications, Operation,
    PermissionMode, SessionAttachParams, SessionAttachResponse, SessionChange,
    SessionConfigureParams, SessionDeleteParams, SessionDetachedParams, SessionForkParams,
    SessionId, SessionListParams, SessionSetup, TurnOutcome,
};
use serde_json::{json, Value};

use super::{
    sessions::{settled_job, Sandbox, DEADLINE},
    setup::{asked_operation, inspected_turn, turn},
};

/// Forks `session`; the fork's id and permission mode, or the error code of
/// a refused fork.
async fn fork(
    sandbox: &Sandbox,
    client: &Client,
    session: &SessionId,
) -> Result<(SessionId, PermissionMode), Option<ErrorCode>> {
    client
        .request::<SessionFork>(&SessionForkParams {
            session: session.clone(),
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        })
        .await
        .map(|forked| (forked.session, forked.options.permission_mode.current))
        .map_err(rpc_code)
}

fn rpc_code(error: ClientError) -> Option<ErrorCode> {
    match error {
        ClientError::Rpc { error, .. } => error.error_code(),
        error => panic!("the request failed outside the protocol: {error}"),
    }
}

/// Attaches to `session` from a new connection, which then follows it.
async fn follow(
    sandbox: &Sandbox,
    session: &SessionId,
) -> Result<(Client, Notifications, SessionAttachResponse), Option<ErrorCode>> {
    let (client, notifications) = sandbox.connect_following().await;
    let attached = client
        .request::<SessionAttach>(&SessionAttachParams {
            session: session.clone(),
            after: None,
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        })
        .await
        .map_err(rpc_code)?;
    Ok((client, notifications, attached))
}

async fn delete(
    client: &Client,
    session: &SessionId,
    unless_attached: bool,
) -> Result<(), Option<ErrorCode>> {
    client
        .request::<SessionDelete>(&SessionDeleteParams {
            session: session.clone(),
            unless_attached,
            work: rho_protocol::DeleteWork::Refuse,
        })
        .await
        .map(|_| ())
        .map_err(rpc_code)
}

/// The messages of an `inspect` fixture's model request.
fn messages(request: &Value) -> &[Value] {
    request["messages"]
        .as_array()
        .expect("a request has messages")
}

// Covers: a fork's model requests start with its session's tools,
// conversation, and prompt cache key, which a provider's cache can serve,
// then its word to the model and the person's message; it opens supervised,
// or in plan mode beside a session in plan mode, whose tools supervised mode
// would change; and session/list leaves it out.
// Owner: daemon session opening (fork).
#[tokio::test]
async fn fork_starts_from_the_saved_conversation_with_the_same_tools() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("inspect").await;
    let mut sessions = Vec::new();
    for (parent_mode, fork_mode) in [
        (None, PermissionMode::Supervised),
        (Some(PermissionMode::Plan), PermissionMode::Plan),
    ] {
        let session = sandbox.new_session(&client).await;
        if let Some(mode) = parent_mode {
            client
                .request::<SessionConfigure>(&SessionConfigureParams {
                    session: session.clone(),
                    change: SessionChange::PermissionMode { mode },
                })
                .await
                .unwrap();
        }
        let (main, _) = inspected_turn(&sandbox, &client, &session, "main question").await;

        let (forked, mode) = fork(&sandbox, &client, &session).await.unwrap();
        assert_eq!(mode, fork_mode, "beside {parent_mode:?}");
        let (side, _) = inspected_turn(&sandbox, &client, &forked, "side question").await;
        assert_eq!(side["tools"], main["tools"], "beside {parent_mode:?}");
        assert!(main["prompt_cache_key"].is_string());
        assert_eq!(side["prompt_cache_key"], main["prompt_cache_key"]);
        let (main_messages, side_messages) = (messages(&main), messages(&side));
        assert_eq!(
            &side_messages[..main_messages.len()],
            main_messages,
            "the fork's request starts with its session's"
        );
        let asked = side_messages.last().expect("the fork's turn asks");
        let blocks = asked["User"].as_array().expect("the turn asks as the user");
        assert_eq!(
            (blocks.len(), blocks.last()),
            (2, Some(&json!({"Text": "side question"}))),
            "the fork's word to the model goes ahead of the person's message: {asked}"
        );
        sessions.push(session);
    }

    let mut listed: Vec<SessionId> = client
        .request::<SessionList>(&SessionListParams::default())
        .await
        .unwrap()
        .sessions
        .into_iter()
        .map(|summary| summary.session)
        .collect();
    listed.sort();
    sessions.sort();
    assert_eq!(listed, sessions);
}

// Covers: a fork's edit waits for the person's approval even when its
// session ran without asking, since the session may still be working in
// the same workspace.
// Owner: daemon session opening (fork permission mode).
#[tokio::test]
async fn fork_asks_before_it_edits() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    assert!(matches!(
        turn(&sandbox, &client, &session, "hello").await.0,
        TurnOutcome::Answered { .. }
    ));
    let (forked, _) = fork(&sandbox, &client, &session).await.unwrap();
    // The first turn's message leads with the fork's word to the model,
    // which the fixture answers as text.
    assert!(matches!(
        turn(&sandbox, &client, &forked, "hello").await.0,
        TurnOutcome::Answered { .. }
    ));

    let path = sandbox.workspace.join("edited.txt");
    let call = json!({
        "id": "fork-write",
        "name": "write",
        "arguments": {"path": path, "content": "from the fork"},
    });
    sandbox.prompt_tool_call(&client, &forked, &call).await;
    assert!(
        matches!(
            asked_operation(&sandbox, &client, &forked).await,
            Operation::WritePath { .. }
        ),
        "the fork asks before it writes"
    );
    assert!(!path.exists(), "nothing is written before the approval");
}

// Covers: a delete of a session that refuses leaves its loaded forks as
// they were, even one that saved nothing yet; one that passes deletes them
// and detaches their clients as deleted.
// Owner: daemon session delete (forks).
#[tokio::test]
async fn deleting_a_session_deletes_its_forks_only_when_it_deletes() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let session = sandbox.new_session(&client).await;
    sandbox.prompt(&client, &session).await.unwrap();
    settled_job(&sandbox, &client, &session).await;
    let (forked, _) = fork(&sandbox, &client, &session).await.unwrap();

    let (watcher, _, _) = follow(&sandbox, &session).await.unwrap();
    assert_eq!(
        delete(&client, &session, /*unless_attached*/ true).await,
        Err(Some(ErrorCode::SessionInUse))
    );
    drop(watcher);
    assert_eq!(
        sandbox.prompt(&client, &forked).await,
        Ok(()),
        "the fork, unsaved, is still loaded"
    );
    settled_job(&sandbox, &client, &forked).await;

    let (_follower, mut notifications, _) = follow(&sandbox, &forked).await.unwrap();
    assert_eq!(
        delete(&client, &session, /*unless_attached*/ false).await,
        Ok(())
    );
    let reason = tokio::time::timeout(DEADLINE, async {
        loop {
            match notifications
                .recv()
                .await
                .expect("the connection stays open")
            {
                DaemonNotification::SessionDetached(SessionDetachedParams { reason, .. }) => {
                    return reason;
                }
                _ => continue,
            }
        }
    })
    .await
    .expect("the fork's client is detached");
    assert_eq!(reason, DetachReason::Deleted);
    assert_eq!(
        follow(&sandbox, &forked).await.map(|_| ()),
        Err(Some(ErrorCode::SessionNotFound))
    );
}

// Covers: a fork the daemon unloaded reopens supervised and still as its
// session's fork, which that session's delete then unloads and deletes.
// Owner: daemon session reopening (fork).
#[tokio::test]
async fn a_reopened_fork_stays_supervised_and_goes_with_its_session() {
    let sandbox = Sandbox::new();
    let client = sandbox
        .start_daemon_with("fixed", &["--unload-idle-after", "200ms"])
        .await;
    let session = sandbox.new_session(&client).await;
    sandbox.prompt(&client, &session).await.unwrap();
    settled_job(&sandbox, &client, &session).await;
    let (forked, _) = fork(&sandbox, &client, &session).await.unwrap();
    sandbox.prompt(&client, &forked).await.unwrap();
    settled_job(&sandbox, &client, &forked).await;
    sandbox.wait_unloaded(&forked, /*times*/ 1).await;

    let (_follower, _, reopened) = follow(&sandbox, &forked).await.unwrap();
    assert_eq!(
        reopened.options.permission_mode.current,
        PermissionMode::Supervised
    );
    assert_eq!(
        delete(&client, &session, /*unless_attached*/ false).await,
        Ok(())
    );
    assert_eq!(
        follow(&sandbox, &forked).await.map(|_| ()),
        Err(Some(ErrorCode::SessionNotFound))
    );
}

// Covers: what cannot be forked is refused before anything opens: a fork,
// and a session that saved nothing.
// Owner: daemon session opening (fork).
#[tokio::test]
async fn fork_refuses_what_it_cannot_run_as() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let session = sandbox.new_session(&client).await;
    let unsaved = sandbox.new_session(&client).await;
    sandbox.prompt(&client, &session).await.unwrap();
    settled_job(&sandbox, &client, &session).await;
    let (forked, _) = fork(&sandbox, &client, &session).await.unwrap();
    sandbox.prompt(&client, &forked).await.unwrap();
    settled_job(&sandbox, &client, &forked).await;

    let mut codes = Vec::new();
    for session in [&forked, &unsaved] {
        codes.push(fork(&sandbox, &client, session).await.map(|_| ()));
    }
    assert_eq!(
        codes,
        [
            Err(Some(ErrorCode::SessionOpenFailed)),
            Err(Some(ErrorCode::SessionNotFound)),
        ]
    );
}
