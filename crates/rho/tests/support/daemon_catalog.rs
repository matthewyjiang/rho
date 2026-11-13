//! The session catalog over the daemon's socket (`rho/session/list`,
//! `rho/session/rename`, `rho/session/delete`), through the real binary: a
//! list holds every workspace's sessions or one workspace's, a rename
//! reaches attached clients and the list, and a delete detaches a loaded
//! session's clients and removes it, removes one the daemon has not loaded,
//! but never runs under a running turn (#1670, #1667). Paging itself is the
//! session catalog's, covered in its unit tests.

use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{
        Method, SessionAttach, SessionCancel, SessionDelete, SessionList, SessionNew, SessionRename,
    },
    Client, ClientError, DaemonNotification, DetachReason, ErrorCode, Notifications, RpcError,
    SessionAttachParams, SessionCancelParams, SessionDeleteParams, SessionDetachedParams,
    SessionEvent, SessionId, SessionListParams, SessionListResponse, SessionNewParams,
    SessionRenameParams, SessionRenameResponse, SessionSetup, SessionSummary,
};

use super::sessions::{settled_job, Sandbox, DEADLINE};

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
