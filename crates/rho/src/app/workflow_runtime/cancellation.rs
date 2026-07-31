use std::path::{Path, PathBuf};

use crate::workflow::{
    NodeState, NodeTerminalState, RunId, RunMutationGuard, RunStateRecord, WorkflowEvent,
    WorkflowEventRecord, WorkflowStore,
};

use super::{runner::persist_state_event, RuntimeError};

// Receipt: the cross-process cancellation command measures owner response with
// this poll interval and checks the accepted acknowledgement limit.
pub(super) const CROSS_PROCESS_CANCEL_POLL: std::time::Duration =
    std::time::Duration::from_millis(100);

// Receipt: limit_receipt.json cancellation.accepted_host_cancellation_completion_millis.
pub(super) const AGENT_CANCELLATION_CLEANUP_MILLIS: u64 = 2_500;

#[derive(Clone)]
pub(crate) struct CancellationRequest {
    pub(super) rho_home: PathBuf,
    pub(super) run_id: RunId,
    pub(super) cancellation: rho_sdk::CancellationToken,
}

impl CancellationRequest {
    pub(crate) fn request(&self) -> Result<CancellationRequestReceipt, RuntimeError> {
        let store = WorkflowStore::new(&self.rho_home)?;
        let receipt = create_or_read_cancellation_request(&store, self.run_id)?;
        self.cancellation.cancel();
        Ok(receipt)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CancellationRequestReceipt {
    pub(super) request_id: String,
}

impl CancellationRequestReceipt {
    pub(crate) fn request_id(&self) -> &str {
        &self.request_id
    }
}

pub(super) fn read_cancellation_request(
    store: &WorkflowStore,
    run_id: RunId,
) -> Result<Option<String>, RuntimeError> {
    let Some(bytes) = store.read_cancellation_request(run_id)? else {
        return Ok(None);
    };
    let token = String::from_utf8(bytes)
        .map_err(|_| RuntimeError::Data("cancel request has an invalid identifier".into()))?;
    let parsed = uuid::Uuid::parse_str(&token)
        .map_err(|_| RuntimeError::Data("cancel request has an invalid identifier".into()))?;
    if parsed.to_string() != token {
        return Err(RuntimeError::Data(
            "cancel request identifier is not canonical".into(),
        ));
    }
    Ok(Some(token))
}

pub(super) fn create_or_read_cancellation_request(
    store: &WorkflowStore,
    run_id: RunId,
) -> Result<CancellationRequestReceipt, RuntimeError> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let installed = store.install_cancellation_request(run_id, request_id.as_bytes())?;
    if installed {
        Ok(CancellationRequestReceipt { request_id })
    } else {
        let request_id = read_cancellation_request(store, run_id)?
            .ok_or_else(|| RuntimeError::Data("active cancellation request disappeared".into()))?;
        Ok(CancellationRequestReceipt { request_id })
    }
}

pub(super) fn run_directory(rho_home: &std::path::Path, run_id: RunId) -> PathBuf {
    rho_home
        .join("workflows")
        .join("runs")
        .join(run_id.to_string())
}

pub(super) fn latest_cancellation_request(events: &[WorkflowEventRecord]) -> Option<String> {
    events.iter().rev().find_map(|record| match &record.event {
        WorkflowEvent::CancellationRequested { request_id } => Some(request_id.clone()),
        _ => None,
    })
}

pub(super) fn latest_pending_cancellation_request(
    events: &[WorkflowEventRecord],
) -> Option<String> {
    let request_id = latest_cancellation_request(events)?;
    (!events.iter().any(|record| {
        matches!(
            &record.event,
            WorkflowEvent::CancellationAcknowledged { request_id: acknowledged }
                if acknowledged == &request_id
        )
    }))
    .then_some(request_id)
}

pub(super) fn cancel_waiting_nodes(
    store: &WorkflowStore,
    guard: &mut RunMutationGuard,
    run_directory: &Path,
    graph: &crate::workflow::FrozenWorkflow,
    state: &mut RunStateRecord,
) -> Result<(), RuntimeError> {
    let waiting = state
        .state
        .nodes
        .iter()
        .filter_map(|(node, state)| {
            matches!(state, NodeState::Pending | NodeState::Ready).then_some(node.clone())
        })
        .collect::<Vec<_>>();
    for node in waiting {
        persist_state_event(
            store,
            guard,
            run_directory,
            graph,
            state,
            WorkflowEvent::NodeFinished {
                node,
                completion: Box::new(crate::workflow::NodeCompletion::terminal(
                    NodeTerminalState::Cancellation,
                )),
            },
        )?;
    }
    Ok(())
}
