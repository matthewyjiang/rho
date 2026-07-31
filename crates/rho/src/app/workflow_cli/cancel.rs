use std::{path::Path, time::Duration};

use serde::Serialize;

use crate::workflow::{RunLifecycle, WorkflowStore};

use super::super::workflow_runtime::{
    cancellation_request_acknowledged, cross_process_cancel_acknowledged,
    request_cross_process_cancel,
};
use super::{planner_worker, workflow_service, write_json_document};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CancellationState {
    Acknowledged,
    Pending,
    AlreadyCompleted,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CancellationOutcome {
    pub(crate) request_id: Option<String>,
    pub(crate) state: CancellationState,
    pub(crate) lifecycle: RunLifecycle,
}

#[derive(Serialize)]
struct CancelDocument {
    run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    cancellation_state: CancellationState,
    lifecycle: RunLifecycle,
}

pub(super) async fn run_cancel(prefix: &str) -> anyhow::Result<()> {
    let service = workflow_service()?;
    let run_id = service.store().resolve_run(prefix)?;
    let run = service.store().load_run(run_id)?;
    let rho_home = crate::paths::rho_dir()?;
    let outcome = request_cancellation(&rho_home, run_id, run.state.state.lifecycle).await?;
    write_json_document(&CancelDocument {
        run_id: run_id.to_string(),
        request_id: outcome.request_id,
        cancellation_state: outcome.state,
        lifecycle: outcome.lifecycle,
    })
}

pub(in crate::app) async fn request_cancellation(
    rho_home: &Path,
    run_id: crate::workflow::RunId,
    lifecycle: RunLifecycle,
) -> Result<CancellationOutcome, super::super::workflow_runtime::RuntimeError> {
    if lifecycle == RunLifecycle::Completed {
        // Keep journal corruption detection for completed runs, but never
        // expose a prior request as this command's acknowledgement.
        let _historical_acknowledgement = cross_process_cancel_acknowledged(rho_home, run_id)?;
        return Ok(CancellationOutcome {
            request_id: None,
            state: CancellationState::AlreadyCompleted,
            lifecycle,
        });
    }
    let receipt = request_cross_process_cancel(rho_home, run_id)?;
    let acknowledged = wait_for_cancellation_ack(
        || cancellation_request_acknowledged(rho_home, run_id, &receipt),
        Duration::from_millis(planner_worker::cancellation_acknowledgement_limit_millis()),
        Duration::from_millis(planner_worker::cancellation_acknowledgement_poll_millis()),
    )
    .await?;
    let lifecycle = WorkflowStore::new(rho_home)?
        .load_run(run_id)?
        .state
        .state
        .lifecycle;
    Ok(CancellationOutcome {
        request_id: Some(receipt.request_id().to_owned()),
        state: cancellation_state(acknowledged, lifecycle),
        lifecycle,
    })
}

pub(super) fn cancellation_state(acknowledged: bool, lifecycle: RunLifecycle) -> CancellationState {
    if acknowledged {
        CancellationState::Acknowledged
    } else if lifecycle == RunLifecycle::Completed {
        CancellationState::AlreadyCompleted
    } else {
        CancellationState::Pending
    }
}

pub(super) async fn wait_for_cancellation_ack<E>(
    mut acknowledged: impl FnMut() -> Result<bool, E>,
    timeout: Duration,
    poll: Duration,
) -> Result<bool, E> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if acknowledged()? {
            return Ok(true);
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(poll.min(deadline - now)).await;
    }
}
