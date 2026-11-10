use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, SessionInspectParams, SessionReport};
use tempfile::TempDir;

use crate::app::{
    context_report::ContextReport,
    daemon_host::{
        jobs::{JobRegistry, JobStore},
        sessions::{
            test_sessions::{load, Load},
            LoadedSession, Sessions,
        },
        wire::session_to_wire,
    },
};

// Covers: a client reads what fills a loaded session's context window as
// the report the TUI's overlay shows; while a turn holds the session the
// daemon refuses rather than making the person wait for it.
// Owner: daemon session reports.
#[tokio::test]
async fn a_context_report_is_read_between_turns() {
    let (root, other_root, home) = (
        TempDir::new().unwrap(),
        TempDir::new().unwrap(),
        TempDir::new().unwrap(),
    );
    let sessions = Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
    );
    let session = load(&sessions, &root, Load::Saved).await;
    let in_turn = load(&sessions, &other_root, Load::Busy).await;
    let context = |session: &LoadedSession| SessionInspectParams {
        session: session_to_wire(session.id.clone()),
        report: SessionReport::Context,
    };

    let read = sessions.inspect(context(&session)).await.map(|answer| {
        serde_json::from_value::<ContextReport>(answer.report).map_err(|error| error.to_string())
    });
    let expected = session
        .runtime
        .try_lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .context_report();
    let during_turn = sessions
        .inspect(context(&in_turn))
        .await
        .map(|_| ())
        .map_err(|error| error.code);

    assert_eq!(
        (read, during_turn),
        (Ok(Ok(expected)), Err(wire::ErrorCode::SessionBusy.code()))
    );
}
