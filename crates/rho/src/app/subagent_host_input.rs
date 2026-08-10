//! Routes structured host questionnaires from delegated agents to the parent session.

use rho_sdk::{CancellationToken, Error, HostInputRequest, HostInputResponse, SessionId};
use tokio::sync::{mpsc, oneshot};

use super::{
    headless_run::{HostInputRespondFuture, HostInputResponder},
    parent_bridge::ParentBridge,
};

/// Questionnaires block the child until the parent answers, so this only needs
/// to absorb a burst of parallel children before back-pressuring.
const HOST_INPUT_QUEUE_CAPACITY: usize = 32;

/// One questionnaire raised by a delegated run and awaiting a parent answer.
pub(crate) struct SubagentHostInputRequest {
    pub(crate) run_id: String,
    pub(crate) agent_id: String,
    pub(crate) parent_session_id: SessionId,
    pub(crate) request: HostInputRequest,
    pub(crate) response: oneshot::Sender<Result<HostInputResponse, Error>>,
}

#[derive(Clone)]
pub(crate) struct SubagentHostInputBridge {
    bridge: ParentBridge<SubagentHostInputRequest>,
}

/// Headless responder that forwards child questionnaires through a parent bridge.
pub(crate) struct SubagentHostInputResponder {
    run_id: String,
    agent_id: String,
    parent_session_id: SessionId,
    bridge: SubagentHostInputBridge,
}

impl SubagentHostInputResponder {
    pub(crate) fn new(
        run_id: impl Into<String>,
        agent_id: impl Into<String>,
        parent_session_id: SessionId,
        bridge: SubagentHostInputBridge,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            agent_id: agent_id.into(),
            parent_session_id,
            bridge,
        }
    }
}

impl HostInputResponder for SubagentHostInputResponder {
    fn respond<'a>(
        &'a self,
        request: HostInputRequest,
        cancellation: &'a CancellationToken,
    ) -> HostInputRespondFuture<'a> {
        Box::pin(self.bridge.request(
            self.run_id.clone(),
            self.agent_id.clone(),
            self.parent_session_id.clone(),
            request,
            cancellation,
        ))
    }
}

impl Default for SubagentHostInputBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentHostInputBridge {
    pub(crate) fn new() -> Self {
        Self {
            bridge: ParentBridge::new(HOST_INPUT_QUEUE_CAPACITY),
        }
    }

    /// Installs the parent receiver. Replaces any previous binding.
    pub(crate) fn bind_parent(&self) -> mpsc::Receiver<SubagentHostInputRequest> {
        self.bridge.bind_parent()
    }

    /// Drops the parent binding so later child requests fail closed.
    pub(crate) fn unbind_parent(&self) {
        self.bridge.unbind_parent();
    }

    /// True while an interactive parent is listening.
    pub(crate) fn is_bound(&self) -> bool {
        self.bridge.is_bound()
    }

    /// Forwards a child questionnaire to the parent and waits for its answer.
    pub(crate) async fn request(
        &self,
        run_id: impl Into<String>,
        agent_id: impl Into<String>,
        parent_session_id: SessionId,
        request: HostInputRequest,
        cancellation: &CancellationToken,
    ) -> Result<HostInputResponse, Error> {
        let sender = self
            .bridge
            .sender()
            .ok_or_else(|| Error::InvalidConfiguration {
                message: "delegated agent questionnaires require an interactive parent session"
                    .into(),
            })?;
        let (response_tx, response_rx) = oneshot::channel();
        let pending = SubagentHostInputRequest {
            run_id: run_id.into(),
            agent_id: agent_id.into(),
            parent_session_id,
            request,
            response: response_tx,
        };
        tokio::select! {
            result = sender.send(pending) => result.map_err(|_| Error::Interrupted {
                message: "parent session stopped accepting delegated questionnaires".into(),
            })?,
            () = cancellation.cancelled() => return Err(Error::Cancelled),
        }
        tokio::select! {
            result = response_rx => result.map_err(|_| Error::Interrupted {
                message: "delegated questionnaire was dropped without a response".into(),
            })?,
            () = cancellation.cancelled() => Err(Error::Cancelled),
        }
    }
}

#[cfg(test)]
#[path = "subagent_host_input_tests.rs"]
mod tests;
