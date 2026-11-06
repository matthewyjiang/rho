use std::time::Duration;

use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::{node_deadline, NodeExpiry};
use crate::subagent::{RunState, RunStatus};

fn status(state: RunState) -> RunStatus {
    RunStatus {
        state,
        ..RunStatus::default()
    }
}

// Covers: an agent node's wait in line for capacity and its run each get
// the node's whole timeout: a node that waited almost its timeout still
// runs its full timeout once it starts, and a node that waits longer than
// its timeout expires queued (giving back its checkout lock) instead of
// waiting forever.
// Owner: workflow agent node execution.
#[tokio::test(start_paused = true)]
async fn a_node_waits_and_runs_up_to_its_timeout_each() {
    let timeout = Duration::from_secs(60);
    let almost = timeout - Duration::from_secs(1);

    let (status_tx, status_rx) = watch::channel(status(RunState::Queued));
    let started = tokio::spawn(node_deadline(status_rx, timeout));
    tokio::time::sleep(almost).await;
    status_tx.send_replace(status(RunState::Starting));
    tokio::time::sleep(almost).await;
    assert!(!started.is_finished(), "the run gets its whole timeout");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(started.await.unwrap(), NodeExpiry::Running);

    let (_status_tx, status_rx) = watch::channel(status(RunState::Queued));
    let queued = tokio::spawn(node_deadline(status_rx, timeout));
    tokio::time::sleep(timeout + Duration::from_secs(1)).await;
    assert_eq!(queued.await.unwrap(), NodeExpiry::Queued);
}
