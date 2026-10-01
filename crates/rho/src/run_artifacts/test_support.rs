use std::time::Duration;

use tokio::sync::watch;

use crate::subagent::RunStatus;

/// The test transfers its only sender to the session. A detached artifact
/// writer retains that sender until its final disk write, so channel closure
/// (not a terminal snapshot) is the condition for reading the complete journal.
pub(crate) async fn wait_for_writer(mut status_rx: watch::Receiver<RunStatus>) -> RunStatus {
    // Match the existing process-session failure bound; this is not a delay.
    const WRITER_COMPLETION_BUDGET: Duration = Duration::from_secs(30);
    let completed = tokio::time::timeout(WRITER_COMPLETION_BUDGET, async {
        while status_rx.changed().await.is_ok() {}
    })
    .await;
    let status = status_rx.borrow().clone();
    assert!(
        completed.is_ok(),
        "artifact writer did not close its status channel within {WRITER_COMPLETION_BUDGET:?}; status: {status:?}"
    );
    status
}
