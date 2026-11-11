use std::{sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use rho_protocol::{ErrorCode, SessionDeleteParams};
use tempfile::TempDir;
use tokio::time::timeout;

use crate::app::daemon_host::{
    jobs::{JobRegistry, JobStore},
    sessions::{
        test_sessions::{load, Load},
        Sessions,
    },
    wire::session_to_wire,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

// Covers: a delete that spares sessions a client has open refuses one a
// client attached to while the delete waited for the session's runtime,
// after its first look found no client, and leaves it loaded with its
// client still following, rather than detaching that client as deleted
// (#1818).
// Owner: daemon session delete.
#[tokio::test]
async fn a_client_that_attaches_while_a_sparing_delete_waits_keeps_its_session() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let sessions = Sessions::new(jobs, home.path());
    let session = load(&sessions, &root, Load::Saved).await;
    let ended_turn = session.runtime.try_lock().unwrap();

    let delete = sessions.delete(SessionDeleteParams {
        session: session_to_wire(session.id.clone()),
        unless_attached: true,
    });
    tokio::pin!(delete);
    // Past its first look for clients, waiting for the runtime.
    assert!(futures_util::poll!(delete.as_mut()).is_pending());
    let _client = session.feed.attach(/*after*/ None);
    drop(ended_turn);
    let refused = timeout(TEST_BUDGET, delete)
        .await
        .expect("the delete answers once the runtime is free")
        .map(|_| ())
        .map_err(|error| error.error_code());

    assert_eq!(
        (
            refused,
            session.feed.followed(),
            sessions.loaded().contains_key(&session.id),
            session.runtime.lock().await.is_some(),
        ),
        (Err(Some(ErrorCode::SessionInUse)), true, true, true)
    );
}
