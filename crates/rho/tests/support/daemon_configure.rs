//! Changing a daemon session's settings between its turns
//! (`rho/session/configure`), through the real binary and its socket: the
//! change answers with the session's options, reaches every attached client,
//! the next turn runs with it, one sent during a turn applies once the turn
//! ends, and it outlasts the daemon unloading the session.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{
        SessionAttach, SessionCancel, SessionConfigure, SessionDelete, SessionDetach, SessionNew,
    },
    Client, ClientError, CommandKind, DaemonNotification, ErrorCode, Notifications, PermissionMode,
    ReasoningLevel, ReasoningOptions, RpcError, SessionAttachParams, SessionCancelParams,
    SessionChange, SessionConfigureParams, SessionDeleteParams, SessionDetachParams, SessionEvent,
    SessionId, SessionNewParams, SessionOptions, TurnOutcome,
};
use serde_json::Value;

use super::sessions::{settled_job, Sandbox, DEADLINE};

/// Asks the daemon to apply `change` to `session`; the session's options
/// then, or the error that refused the change.
async fn configure(
    client: &Client,
    session: &SessionId,
    change: SessionChange,
) -> Result<SessionOptions, RpcError> {
    client
        .request::<SessionConfigure>(&SessionConfigureParams {
            session: session.clone(),
            change,
        })
        .await
        .map(|response| response.options)
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error,
            error => panic!("configure failed outside the protocol: {error}"),
        })
}

/// Reads `session`'s events until one satisfies `done`, and returns it.
async fn next_event<T>(
    notifications: &mut Notifications,
    session: &SessionId,
    mut done: impl FnMut(SessionEvent) -> Option<T>,
) -> T {
    loop {
        let notification = tokio::time::timeout(DEADLINE, notifications.recv())
            .await
            .expect("the daemon sends the session's next event")
            .expect("the connection stays open");
        let DaemonNotification::SessionEvent(params) = notification else {
            panic!("expected a session event, got {notification:?}");
        };
        assert_eq!(&params.session, session);
        if let Some(found) = done(params.event) {
            return found;
        }
    }
}

/// The options of the next options event.
pub(super) async fn next_options(
    notifications: &mut Notifications,
    session: &SessionId,
) -> SessionOptions {
    next_event(notifications, session, |event| match event {
        SessionEvent::OptionsChanged { options } => Some(options),
        _ => None,
    })
    .await
}

/// The model request the `inspect` fixture answered the next turn with:
/// its answer, read up to the turn's end.
async fn next_request(notifications: &mut Notifications, session: &SessionId) -> Value {
    let mut answer = String::new();
    next_event(notifications, session, |event| match event {
        SessionEvent::AgentMessageChunk { text, .. } => {
            answer.push_str(&text);
            None
        }
        SessionEvent::TurnEnded { .. } => Some(()),
        _ => None,
    })
    .await;
    serde_json::from_str(&answer).unwrap_or_else(|error| panic!("{error}: {answer}"))
}

/// The reasoning level a model request asked for, and whether it offered
/// the model `exit_plan_mode`, which only plan mode does.
fn reasoning_and_plan_exit(request: &Value) -> (String, bool) {
    let offers_plan_exit = request["tools"]
        .as_array()
        .expect("the request lists its tools")
        .iter()
        .any(|tool| tool["name"] == "exit_plan_mode");
    let reasoning = request["reasoning"].as_str().expect("the request's level");
    (reasoning.to_owned(), offers_plan_exit)
}

// Covers: a new session answers with its options, offering only commands
// the daemon runs; a permission-mode and a reasoning change each answer
// with the options as they are now, which an attached client receives as
// `options_changed`, and the next turn's model request runs with both. A
// change the session cannot take, or one this daemon does not know, is
// refused as invalid params and changes nothing (#1667, #1670).
// Owner: daemon session settings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_reaches_attached_clients_and_the_next_turn() {
    let sandbox = Sandbox::new();
    let (_daemon, client) = sandbox.start_daemon("inspect").await;
    let opened = client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment: sandbox.client_environment(),
        })
        .await
        .unwrap();
    let (session, before) = (opened.session, opened.options);
    let slash_commands = before
        .commands
        .iter()
        .filter(|command| command.kind == CommandKind::Slash)
        .map(|command| command.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        slash_commands,
        ["init"],
        "no workflow or agent the daemon refuses"
    );
    let (watcher, mut notifications) = sandbox.connect_following().await;
    let attached = watcher
        .request::<SessionAttach>(&SessionAttachParams {
            session: session.clone(),
            after: None,
            environment: sandbox.client_environment(),
        })
        .await
        .unwrap();
    assert_eq!(attached.options, before);
    sandbox.prompt(&client, &session).await.unwrap();
    assert_eq!(
        reasoning_and_plan_exit(&next_request(&mut notifications, &session).await),
        ("medium".to_owned(), false),
        "the fixture config's settings, before any change"
    );

    let planned = configure(
        &client,
        &session,
        SessionChange::PermissionMode {
            mode: PermissionMode::Plan,
        },
    )
    .await;
    let mut expected = before.clone();
    expected.permission_mode.current = PermissionMode::Plan;
    assert_eq!(planned, Ok(expected.clone()));
    assert_eq!(next_options(&mut notifications, &session).await, expected);
    let reasoned = configure(
        &client,
        &session,
        SessionChange::Reasoning {
            level: ReasoningLevel::High,
        },
    )
    .await;
    expected.reasoning = Some(ReasoningOptions {
        current: ReasoningLevel::High,
        available: before
            .reasoning
            .clone()
            .expect("grok-fixture reasons")
            .available,
    });
    assert_eq!(reasoned, Ok(expected.clone()));
    assert_eq!(next_options(&mut notifications, &session).await, expected);

    let refused = [
        SessionChange::FastMode { enabled: true },
        SessionChange::PermissionMode {
            mode: PermissionMode::Unknown,
        },
    ];
    let mut errors = Vec::new();
    for change in refused {
        errors.push(configure(&client, &session, change).await);
    }
    assert_eq!(
        errors,
        [
            Err(RpcError::new(
                ErrorCode::InvalidParams,
                "fast mode is not available for xai/grok-fixture",
            )),
            Err(RpcError::new(
                ErrorCode::InvalidParams,
                "unknown permission mode"
            )),
        ]
    );

    sandbox.prompt(&client, &session).await.unwrap();
    assert_eq!(
        reasoning_and_plan_exit(&next_request(&mut notifications, &session).await),
        ("high".to_owned(), true),
        "log:\n{}",
        sandbox.daemon_log()
    );
}

// Covers: a change sent while a turn runs waits for the turn's end, then
// applies and answers, rather than being refused; a delete meanwhile sees
// the queued change; and a prompt sent once the turn ended starts a turn
// that clients see after the change (#1685). The order of a prompt and a
// change that both wait on the runtime is `configure_tests`'.
// Owner: daemon session settings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_sent_during_a_turn_applies_at_its_end_before_the_next_turn() {
    #[derive(Debug, PartialEq)]
    enum Seen {
        Prompt,
        Options(SessionOptions),
    }
    let sandbox = Sandbox::new();
    // The fixture holds each turn until it is cancelled.
    let (_daemon, client) = sandbox.start_daemon("delay").await;
    let session = sandbox.new_session(&client).await;
    let (watcher, mut notifications) = sandbox.connect_following().await;
    let before = watcher
        .request::<SessionAttach>(&SessionAttachParams {
            session: session.clone(),
            after: None,
            environment: sandbox.client_environment(),
        })
        .await
        .unwrap()
        .options;
    sandbox.prompt(&client, &session).await.unwrap();

    let change = configure(
        &client,
        &session,
        SessionChange::PermissionMode {
            mode: PermissionMode::Plan,
        },
    );
    let delete = SessionDeleteParams {
        session: session.clone(),
    };
    let turn_runs = RpcError::new(
        ErrorCode::SessionBusy,
        "the session has a turn in progress; delete it once the turn ends",
    );
    let change_queued = RpcError::new(
        ErrorCode::SessionBusy,
        "a change to the session's settings is queued or applying; delete it once the change has applied",
    );
    let race = async {
        // The delete is refused for the turn until the change queues behind
        // it, then for the change: the turn cannot end before the cancel.
        sandbox
            .poll_until("the change to queue behind the turn", async || {
                let error = match client.request::<SessionDelete>(&delete).await {
                    Err(ClientError::Rpc { error, .. }) => error,
                    other => panic!("the delete was not refused: {other:?}"),
                };
                if error == turn_runs {
                    None
                } else {
                    assert_eq!(error, change_queued);
                    Some(())
                }
            })
            .await;
        client
            .request::<SessionCancel>(&SessionCancelParams {
                session: session.clone(),
            })
            .await
            .unwrap();
        // The turn's end is told under the lock a prompt starts its turn
        // under, so the prompt after it finds no turn under way.
        let ended = next_event(&mut notifications, &session, |event| match event {
            SessionEvent::TurnEnded { outcome } => Some(outcome),
            _ => None,
        })
        .await;
        assert_eq!(ended, TurnOutcome::Cancelled);
        sandbox.prompt(&client, &session).await.unwrap();
    };
    let (changed, ()) = tokio::join!(change, race);

    let mut expected = before;
    expected.permission_mode.current = PermissionMode::Plan;
    assert_eq!(changed, Ok(expected.clone()));
    let mut seen = Vec::new();
    next_event(&mut notifications, &session, |event| {
        seen.push(match event {
            SessionEvent::UserMessage { .. } => Seen::Prompt,
            SessionEvent::OptionsChanged { options } => Seen::Options(options),
            _ => return None,
        });
        seen.contains(&Seen::Prompt).then_some(())
    })
    .await;
    assert_eq!(seen, [Seen::Options(expected), Seen::Prompt]);
    client
        .request::<SessionCancel>(&SessionCancelParams { session })
        .await
        .unwrap();
}

// Covers: a change outlasts the daemon unloading the idle session, which
// it does on its own: the session a client reopens reports the change, and
// its next turn's model request runs with it, rather than the session
// silently going back to the config's settings (#1661, #1667, #1670).
// Owner: daemon session settings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_outlasts_the_idle_unload() {
    let sandbox = Sandbox::new();
    let (_daemon, client) = sandbox
        .start_daemon_with("inspect", &["--unload-idle-after", "200ms"])
        .await;
    let session = sandbox.new_session(&client).await;
    // Followed, so the unload keeps the session until the changes applied.
    let (watcher, _) = sandbox.connect_following().await;
    let attach = SessionAttachParams {
        session: session.clone(),
        after: None,
        environment: sandbox.client_environment(),
    };
    watcher.request::<SessionAttach>(&attach).await.unwrap();
    // Saved, so the unload has a file to reopen.
    sandbox.prompt(&client, &session).await.unwrap();
    settled_job(&sandbox, &client, &session).await;
    configure(
        &client,
        &session,
        SessionChange::PermissionMode {
            mode: PermissionMode::Plan,
        },
    )
    .await
    .unwrap();
    let configured = configure(
        &client,
        &session,
        SessionChange::Reasoning {
            level: ReasoningLevel::High,
        },
    )
    .await
    .unwrap();
    watcher
        .request::<SessionDetach>(&SessionDetachParams {
            session: session.clone(),
        })
        .await
        .unwrap();
    sandbox
        .poll_until("the daemon to unload the idle session", async || {
            sandbox
                .daemon_log()
                .lines()
                .any(|line| {
                    line.contains("unloaded idle session") && line.contains(session.as_str())
                })
                .then_some(())
        })
        .await;

    let (follower, mut notifications) = sandbox.connect_following().await;
    let reopened = follower.request::<SessionAttach>(&attach).await.unwrap();
    // The saved turn, replayed before anything new.
    let mut replayed = reopened.replay_events;
    if replayed > 0 {
        next_event(&mut notifications, &session, |_| {
            replayed -= 1;
            (replayed == 0).then_some(())
        })
        .await;
    }
    sandbox.prompt(&client, &session).await.unwrap();

    assert_eq!(reopened.options, configured);
    assert_eq!(
        reasoning_and_plan_exit(&next_request(&mut notifications, &session).await),
        ("high".to_owned(), true),
        "log:\n{}",
        sandbox.daemon_log()
    );
}
