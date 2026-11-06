use std::time::Duration;

use tokio::sync::watch;

use super::node_deadline;
use crate::subagent::{RunState, RunStatus};

fn status(state: RunState) -> RunStatus {
    RunStatus {
        state,
        ..RunStatus::default()
    }
}

// Covers: an agent node's timeout runs from when its run has capacity, so a
// node that waited in the shared budget's queue longer than its timeout
// still gets its whole timeout once it starts.
// Owner: workflow agent node execution.
#[tokio::test(start_paused = true)]
async fn a_node_times_out_only_after_its_run_starts() {
    let timeout = Duration::from_secs(60);
    let (status_tx, status_rx) = watch::channel(status(RunState::Queued));
    let deadline = tokio::spawn(node_deadline(status_rx, timeout));

    tokio::time::sleep(timeout * 3).await;
    assert!(!deadline.is_finished(), "a queued node does not time out");

    status_tx.send_replace(status(RunState::Starting));
    tokio::time::sleep(timeout - Duration::from_secs(1)).await;
    assert!(!deadline.is_finished(), "the timeout runs from the start");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        deadline.is_finished(),
        "the node times out after its timeout"
    );
}
