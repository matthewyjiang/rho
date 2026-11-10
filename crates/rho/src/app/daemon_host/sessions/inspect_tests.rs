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
            Sessions,
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
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let sessions = Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
    );
    let session = load(&sessions, &root, Load::Saved).await;
    let context = || SessionInspectParams {
        session: session_to_wire(session.id.clone()),
        report: SessionReport::Context,
    };

    let read = sessions.inspect(context()).await.map(|answer| {
        serde_json::from_value::<ContextReport>(answer.report).map_err(|error| error.to_string())
    });
    let expected = session
        .runtime
        .try_lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .context_report();
    let turn = session.runtime.try_lock().unwrap();
    let during_turn = sessions
        .inspect(context())
        .await
        .map(|_| ())
        .map_err(|error| error.code);
    drop(turn);

    assert_eq!(
        (read, during_turn),
        (Ok(Ok(expected)), Err(wire::ErrorCode::SessionBusy.code()))
    );
}
