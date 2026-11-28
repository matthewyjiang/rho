//! The session catalog over the daemon's socket (`rho/session/list`,
//! `rho/session/list_detailed`, `rho/session/rename`, `rho/session/mark`,
//! `rho/session/delete`), through the real binary: a list holds every
//! workspace's sessions or one workspace's, a detailed list names each
//! session's agent and the session that started it, a rename reaches
//! attached clients and the list, a mark does too and outlives the daemon,
//! and a delete detaches a loaded session's clients and removes it, removes
//! one the daemon has not loaded, but never runs under a running turn
//! (#1670, #1667). Paging itself is the session catalog's, covered in its
//! unit tests.

use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{
        JobList, Method, SessionAttach, SessionCancel, SessionDelete, SessionList,
        SessionListDetailed, SessionMarkMethod, SessionNew, SessionRename,
    },
    Client, ClientError, DaemonNotification, DetachReason, ErrorCode, JobKind, JobListParams,
    NewSessionMark, Notifications, RpcError, SessionAttachParams, SessionCancelParams,
    SessionDeleteParams, SessionDetachedParams, SessionEvent, SessionId, SessionListParams,
    SessionListResponse, SessionMark, SessionMarkParams, SessionMarkResponse, SessionNewParams,
    SessionRenameParams, SessionRenameResponse, SessionSetup, SessionSummary,
};

use super::sessions::{
    blue, first_request, settled_job, Follower, Sandbox, ARCHIVIST_AGENT, DEADLINE,
};

/// Sends `M` with `params`; its result, or the error the daemon answered.
async fn call<M: Method>(client: &Client, params: &M::Params) -> Result<M::Result, RpcError> {
    client
        .request::<M>(params)
        .await
        .map_err(|error| match error {
            ClientError::Rpc { error, .. } => error,
            error => panic!("{} failed outside the protocol: {error}", M::NAME),
        })
}

/// Lists the sessions `params` asks for.
async fn list(client: &Client, params: SessionListParams) -> SessionListResponse {
    call::<SessionList>(client, &params).await.unwrap()
}

/// Opens a session in `cwd`, which saves nothing until its first prompt.
async fn open_session(sandbox: &Sandbox, client: &Client, cwd: PathBuf) -> SessionId {
    call::<SessionNew>(
        client,
        &SessionNewParams {
            cwd,
            environment: sandbox.client_environment(),
            agent: None,
            setup: SessionSetup::default(),
            parent: None,
            grant: None,
            worktree: false,
        },
    )
    .await
    .unwrap()
    .session
}

/// Prompts `session` with "hello" and waits for the turn to end, so the
/// session is saved and listed.
async fn answer(sandbox: &Sandbox, client: &Client, session: &SessionId) {
    sandbox.prompt(client, session).await.unwrap();
    settled_job(sandbox, client, session).await;
}

/// A session opened in `cwd` and answered once.
async fn saved_session(sandbox: &Sandbox, client: &Client, cwd: PathBuf) -> SessionId {
    let session = open_session(sandbox, client, cwd).await;
    answer(sandbox, client, &session).await;
    session
}

/// What the list reports of a session the `fixed` fixture answered once,
/// saved in `cwd`. The save time is the daemon's to report;
/// [`without_time`] clears it.
fn prompted_once(session: &SessionId, cwd: &Path) -> SessionSummary {
    SessionSummary {
        session: session.clone(),
        cwd: cwd.to_path_buf(),
        added_dirs: Vec::new(),
        title: None,
        first_user_message: Some("hello".into()),
        last_user_message: Some("hello".into()),
        updated_at: 0,
    }
}

/// `sessions` with their save times cleared, for whole-object comparison.
fn without_time(sessions: &[SessionSummary]) -> Vec<SessionSummary> {
    sessions
        .iter()
        .cloned()
        .map(|summary| SessionSummary {
            updated_at: 0,
            ..summary
        })
        .collect()
}

/// Attaches `session` on a connection of its own; the connection, kept
/// open so the attachment lasts, and its notifications.
async fn follow(sandbox: &Sandbox, session: &SessionId) -> (Client, Notifications) {
    let (client, notifications) = sandbox.connect_following().await;
    call::<SessionAttach>(
        &client,
        &SessionAttachParams {
            session: session.clone(),
            after: None,
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        },
    )
    .await
    .unwrap();
    (client, notifications)
}

/// Reads notifications until one satisfies `done`, and returns what it
/// found.
async fn next_notification<T>(
    notifications: &mut Notifications,
    mut done: impl FnMut(DaemonNotification) -> Option<T>,
) -> T {
    loop {
        let notification = tokio::time::timeout(DEADLINE, notifications.recv())
            .await
            .expect("the daemon sends the next notification")
            .expect("the connection stays open");
        if let Some(found) = done(notification) {
            return found;
        }
    }
}

// Covers: the list holds every workspace's saved sessions, or one
// workspace's when it names one; a `limit` cuts the page and its
// `next_cursor` says more follow; a page over the daemon's limit is refused
// naming that limit (#1670, #1667).
// Owner: daemon session catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_holds_every_workspace_or_one() {
    let sandbox = Sandbox::new();
    let other = sandbox.workspace.with_file_name("other-workspace");
    std::fs::create_dir_all(&other).unwrap();
    let client = sandbox.start_daemon("fixed").await;
    let first = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    let elsewhere = saved_session(&sandbox, &client, other.clone()).await;
    let last = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;

    let all = list(&client, SessionListParams::default()).await;
    let mut listed = without_time(&all.sessions);
    listed.sort_by(|left, right| left.session.cmp(&right.session));
    let mut expected = vec![
        prompted_once(&first, &sandbox.workspace),
        prompted_once(&elsewhere, &other),
        prompted_once(&last, &sandbox.workspace),
    ];
    expected.sort_by(|left, right| left.session.cmp(&right.session));
    assert_eq!((listed, all.next_cursor.clone()), (expected, None));

    let page = list(
        &client,
        SessionListParams {
            limit: NonZeroU32::new(2),
            ..SessionListParams::default()
        },
    )
    .await;
    assert_eq!(
        (page.sessions, page.next_cursor.is_some()),
        (all.sessions[..2].to_vec(), true)
    );

    let one_workspace = list(
        &client,
        SessionListParams {
            cwd: Some(other.clone()),
            ..SessionListParams::default()
        },
    )
    .await;
    assert_eq!(
        (
            without_time(&one_workspace.sessions),
            one_workspace.next_cursor
        ),
        (vec![prompted_once(&elsewhere, &other)], None)
    );

    let too_many = call::<SessionList>(
        &client,
        &SessionListParams {
            limit: NonZeroU32::new(1001),
            ..SessionListParams::default()
        },
    )
    .await;
    assert_eq!(
        too_many,
        Err(RpcError::new(
            ErrorCode::InvalidParams,
            "limit 1001 is over the session list's page limit (MAX_PAGE_SIZE = 1000)",
        ))
    );
}

/// Each session `rho/session/list_detailed` lists, in every workspace: its
/// id, agent, and parent, in id order.
async fn agents_and_parents(
    client: &Client,
) -> Vec<(SessionId, Option<String>, Option<SessionId>)> {
    let mut rows: Vec<_> = call::<SessionListDetailed>(client, &SessionListParams::default())
        .await
        .unwrap()
        .sessions
        .into_iter()
        .map(|row| (row.summary.session, row.agent, row.parent))
        .collect();
    rows.sort();
    rows
}

// Covers: the detailed list names the agent each session was saved as,
// `default` for the default agent, and the session its `parent` resolves
// to: the one named, or, for a subagent run's session, which no list holds,
// the session whose turn runs the subagent; a `parent` no job of the daemon
// writes records none.
// Owner: daemon session catalog and `rho/session/new`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_detailed_list_names_agents_and_the_sessions_that_started_them() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let archivist = sandbox.isolated.home.join(".agents/agents/archivist.md");
    std::fs::create_dir_all(archivist.parent().unwrap()).unwrap();
    std::fs::write(&archivist, ARCHIVIST_AGENT).unwrap();
    // The fixture makes the call a prompt spells out as JSON, and answers
    // any other prompt.
    let client = sandbox.start_daemon("tool-call").await;
    let lead = sandbox.new_session(&client).await;
    let (mut following, _) = Follower::attach(&sandbox, &lead, Vec::new(), None).await;
    // The run's prompt is a call too: it asks its parent and waits, so the
    // run is under way when a session starts from its session.
    let ask = serde_json::json!({
        "id": "ask",
        "name": "questionnaire",
        "arguments": {
            "questions": [{
                "id": "color",
                "question": "Which color?",
                "choices": ["Red", "Blue"],
            }],
        },
    });
    let delegate = serde_json::json!({
        "id": "delegate",
        "name": "agent",
        "arguments": {"agent_id": "worker", "prompt": ask.to_string()},
    });
    sandbox.prompt_tool_call(&client, &lead, &delegate).await;
    following
        .read_until(|event| matches!(event, SessionEvent::InputRequested { .. }))
        .await;
    let request = first_request(&following.events).id;
    let run_session = sandbox
        .poll_until("the run's session", async || {
            let jobs = call::<JobList>(&client, &JobListParams {}).await.unwrap();
            jobs.jobs
                .into_iter()
                .find(|job| job.kind == JobKind::Subagent && job.state.in_progress())?
                .session
        })
        .await;
    let start = async |agent: Option<&'static str>, parent: SessionId| {
        let session = call::<SessionNew>(
            &client,
            &SessionNewParams {
                cwd: sandbox.workspace.clone(),
                environment: sandbox.client_environment(),
                agent: agent.map(str::to_owned),
                setup: SessionSetup::default(),
                parent: Some(parent),
                grant: None,
                worktree: false,
            },
        )
        .await
        .unwrap()
        .session;
        answer(&sandbox, &client, &session).await;
        session
    };
    let archived = start(Some("archivist"), lead.clone()).await;
    let from_run = start(None, run_session).await;
    let unknown = start(None, SessionId::new("no-session-of-this-daemon")).await;
    super::sessions::answer(&client, &lead, request, blue())
        .await
        .unwrap();
    settled_job(&sandbox, &client, &lead).await;

    let default = Some("default".to_owned());
    let mut expected = vec![
        (lead.clone(), default.clone(), None),
        (archived, Some("archivist".to_owned()), Some(lead.clone())),
        (from_run, default.clone(), Some(lead)),
        (unknown, default, None),
    ];
    expected.sort();
    assert_eq!(
        agents_and_parents(&client).await,
        expected,
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a rename saves the trimmed title, which an attached client
// receives as `title_changed` and the list reports; a blank title, one
// with a control character, and one over the store's budget are refused,
// naming why (#1667).
// Owner: daemon session catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_reaches_attached_clients_and_the_list() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let session = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    let (_watcher, mut notifications) = follow(&sandbox, &session).await;
    let rename = |title: String| {
        let params = SessionRenameParams {
            session: session.clone(),
            title,
        };
        let client = &client;
        async move { call::<SessionRename>(client, &params).await }
    };

    let refused = vec![
        rename(" \n ".into()).await,
        rename("Release\u{1b}[2J notes".into()).await,
        rename("t".repeat(501)).await,
    ];
    rename("  Release notes \n".into()).await.unwrap();

    let told = next_notification(&mut notifications, |notification| match notification {
        DaemonNotification::SessionEvent(params) => match params.event {
            SessionEvent::TitleChanged { title } => Some((params.session, title)),
            _ => None,
        },
        notification => panic!("expected the session's events, got {notification:?}"),
    })
    .await;
    let listed = list(&client, SessionListParams::default()).await;
    let invalid = |message: &str| -> Result<SessionRenameResponse, RpcError> {
        Err(RpcError::new(ErrorCode::InvalidParams, message))
    };
    assert_eq!(
        (refused, told, without_time(&listed.sessions)),
        (
            vec![
                invalid("title must not be empty"),
                invalid("title must not contain control characters, but has '\\u{1b}'"),
                invalid(
                    "title is 501 characters, over the session title budget (MAX_TITLE_CHARS = 500)"
                ),
            ],
            (session.clone(), "Release notes".to_owned()),
            vec![SessionSummary {
                title: Some("Release notes".into()),
                ..prompted_once(&session, &sandbox.workspace)
            }],
        )
    );
}

/// Each session `rho/session/list_detailed` lists, in every workspace, as
/// `rho/session/list` lists it, with its mark.
async fn marks(client: &Client) -> Vec<(SessionSummary, Option<SessionMark>)> {
    call::<SessionListDetailed>(client, &SessionListParams::default())
        .await
        .unwrap()
        .sessions
        .into_iter()
        .map(|row| (row.summary, row.mark))
        .collect()
}

// Covers: a mark set over the socket is saved with the session: the
// daemon stamps its time and answers with it, an attached client hears it
// as `mark_changed`, and the detailed list reports it beside an
// `updated_at` the mark did not move; a daemon started later still lists
// it, and clears it on a session it has not loaded; a deleted session has
// no mark to set. Owner: daemon session catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mark_reaches_attached_clients_and_the_list_and_outlives_the_daemon() {
    let sandbox = Sandbox::new();
    let (mut daemon, client) = sandbox.spawn_daemon(sandbox.rho("fixed"), &[]).await;
    let session = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    let deleted = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    let mark = async |client: &Client, session: &SessionId, mark: Option<NewSessionMark>| {
        let params = SessionMarkParams::new(session.clone(), mark);
        call::<SessionMarkMethod>(client, &params).await
    };
    // Far past any run of this test.
    let snooze = NewSessionMark::Snoozed {
        until: 4_102_444_800,
    };
    let snoozed = Some(SessionMark::Snoozed {
        until: 4_102_444_800,
    });

    let delete = SessionDeleteParams {
        session: deleted.clone(),
        unless_attached: false,
        background: rho_protocol::DeleteBackground::Refuse,
    };
    call::<SessionDelete>(&client, &delete).await.unwrap();
    let after_delete = mark(&client, &deleted, Some(NewSessionMark::Done)).await;
    let unmarked = marks(&client).await;
    let (watcher, mut notifications) = follow(&sandbox, &session).await;
    let pinned = mark(&client, &session, Some(NewSessionMark::Pinned))
        .await
        .unwrap();
    let told = next_notification(&mut notifications, |notification| match notification {
        DaemonNotification::SessionEvent(params) => match params.event {
            SessionEvent::MarkChanged { mark } => Some((params.session, mark)),
            _ => None,
        },
        notification => panic!("expected the session's events, got {notification:?}"),
    })
    .await;
    let replaced = mark(&client, &session, Some(snooze)).await;
    let marked = marks(&client).await;

    drop((watcher, client));
    let stop = sandbox
        .rho("fixed")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stop.success(), "log:\n{}", sandbox.isolated.daemon_log());
    daemon.wait().unwrap();
    let client = sandbox.start_daemon("fixed").await;
    let after_restart = marks(&client).await;
    let cleared = mark(&client, &session, None).await;
    let after_clear = marks(&client).await;

    let Some(SessionMark::Pinned { at }) = pinned.mark else {
        panic!("the session is pinned, not {:?}", pinned.mark);
    };
    let [(saved, None)] = unmarked.as_slice() else {
        panic!("one session is listed, unmarked, not {unmarked:?}");
    };
    let snoozed_session = vec![(saved.clone(), snoozed.clone())];
    assert_eq!(
        (
            without_time(std::slice::from_ref(saved)),
            after_delete,
            (at >= saved.updated_at, told),
            replaced,
            marked,
            after_restart,
            (cleared, after_clear),
        ),
        (
            vec![prompted_once(&session, &sandbox.workspace)],
            Err(RpcError::new(
                ErrorCode::SessionNotFound,
                format!("no session is saved under {deleted}"),
            )),
            (true, (session.clone(), pinned.mark)),
            Ok(SessionMarkResponse::new(snoozed)),
            snoozed_session.clone(),
            snoozed_session,
            (Ok(SessionMarkResponse::new(None)), unmarked.clone()),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a delete that asks to spare attached sessions refuses one a
// client follows, as in use, and deletes it once that client has left: a
// client deleting sessions it is not showing never takes another's (#1667).
// Owner: daemon session catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_can_spare_a_session_a_client_is_attached_to() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let session = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    let params = SessionDeleteParams {
        session: session.clone(),
        unless_attached: true,
        background: rho_protocol::DeleteBackground::Refuse,
    };

    let (watcher, _notifications) = follow(&sandbox, &session).await;
    let while_attached = call::<SessionDelete>(&client, &params)
        .await
        .map_err(|error| error.error_code());
    drop(watcher);
    // The daemon reads the watcher's close on its own task.
    let after_it_left = tokio::time::timeout(DEADLINE, async {
        loop {
            match call::<SessionDelete>(&client, &params).await {
                Err(error) if error.error_code() == Some(ErrorCode::SessionInUse) => {
                    tokio::task::yield_now().await;
                }
                answer => break answer.map_err(|error| error.error_code()),
            }
        }
    })
    .await
    .expect("the daemon noticed the watcher leave");

    assert_eq!(
        (while_attached, after_it_left),
        (
            Err(Some(ErrorCode::SessionInUse)),
            Ok(rho_protocol::SessionDeleteResponse {})
        )
    );
}

// Covers: deleting an idle session the daemon has loaded detaches its
// attached client as `deleted`, drops it from the list, and leaves nothing
// to reattach, while the other session stays listed (#1670, #1667).
// Owner: daemon session catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_detaches_a_loaded_sessions_clients_and_removes_it() {
    let sandbox = Sandbox::new();
    let client = sandbox.start_daemon("fixed").await;
    let deleted = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    let kept = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    let (_watcher, mut notifications) = follow(&sandbox, &deleted).await;

    call::<SessionDelete>(
        &client,
        &SessionDeleteParams {
            session: deleted.clone(),
            unless_attached: false,
            background: rho_protocol::DeleteBackground::Refuse,
        },
    )
    .await
    .unwrap();

    let detached = next_notification(&mut notifications, |notification| match notification {
        DaemonNotification::SessionEvent(_) => None,
        DaemonNotification::SessionDetached(params) => Some(params),
        notification => panic!("expected the session's notifications, got {notification:?}"),
    })
    .await;
    let listed = list(&client, SessionListParams::default()).await;
    let reattached = call::<SessionAttach>(
        &client,
        &SessionAttachParams {
            session: deleted.clone(),
            after: None,
            environment: sandbox.client_environment(),
            setup: SessionSetup::default(),
        },
    )
    .await
    .map(|_| ());
    assert_eq!(
        (detached, without_time(&listed.sessions), reattached),
        (
            SessionDetachedParams {
                session: deleted.clone(),
                reason: DetachReason::Deleted,
            },
            vec![prompted_once(&kept, &sandbox.workspace)],
            Err(RpcError::new(
                ErrorCode::SessionNotFound,
                format!("no session is saved under {deleted}"),
            )),
        )
    );
}

// Covers: a session the daemon has not loaded, here one it unloaded while
// idle, is deleted from its saved files and leaves the list; deleting it
// again finds nothing saved (#1670, #1667).
// Owner: daemon session catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_removes_a_session_the_daemon_has_not_loaded() {
    let sandbox = Sandbox::new();
    let client = sandbox
        .start_daemon_with("fixed", &["--unload-idle-after", "200ms"])
        .await;
    let session = saved_session(&sandbox, &client, sandbox.workspace.clone()).await;
    sandbox.wait_unloaded(&session, /*times*/ 1).await;
    let delete = || {
        let params = SessionDeleteParams {
            session: session.clone(),
            unless_attached: false,
            background: rho_protocol::DeleteBackground::Refuse,
        };
        let client = &client;
        async move { call::<SessionDelete>(client, &params).await }
    };

    let deleted = delete().await.map(|_| ());
    let listed = list(&client, SessionListParams::default()).await;
    let again = delete().await.map(|_| ());

    assert_eq!(
        (deleted, listed.sessions, again),
        (
            Ok(()),
            Vec::new(),
            Err(RpcError::new(
                ErrorCode::SessionNotFound,
                format!("no session is saved under {session}"),
            )),
        )
    );
}

// Covers: a session whose turn is under way refuses a delete, so a client
// learns to cancel first rather than the turn losing its file mid-write
// (#1667).
// Owner: daemon session catalog.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_is_refused_while_the_turn_runs() {
    let sandbox = Sandbox::new();
    // The fixture holds the turn until it is cancelled.
    let client = sandbox.start_daemon("delay").await;
    let session = sandbox.new_session(&client).await;
    sandbox.prompt(&client, &session).await.unwrap();

    let refused = call::<SessionDelete>(
        &client,
        &SessionDeleteParams {
            session: session.clone(),
            unless_attached: false,
            background: rho_protocol::DeleteBackground::Refuse,
        },
    )
    .await;

    assert_eq!(
        refused,
        Err(RpcError::new(
            ErrorCode::SessionBusy,
            "the session has a turn in progress; delete it once the turn ends",
        ))
    );
    call::<SessionCancel>(&client, &SessionCancelParams { session })
        .await
        .unwrap();
}
