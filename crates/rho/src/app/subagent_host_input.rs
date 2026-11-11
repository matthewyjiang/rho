//! Routes structured host questionnaires from delegated agents to the parent session.

use rho_sdk::{CancellationToken, Error, HostInputRequest, HostInputResponse, SessionId};
use tokio::sync::{mpsc, oneshot, watch};

use super::parent_bridge::ParentBridge;

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
    /// How many questionnaires the parent took and has not answered.
    open: watch::Sender<usize>,
}

/// Counts one questionnaire as open with the parent until dropped.
struct OpenQuestion<'a>(&'a watch::Sender<usize>);

impl<'a> OpenQuestion<'a> {
    fn new(open: &'a watch::Sender<usize>) -> Self {
        open.send_modify(|open| *open += 1);
        Self(open)
    }
}

impl Drop for OpenQuestion<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|open| *open -= 1);
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
            open: watch::Sender::new(0),
        }
    }

    /// How many questionnaires the parent took and has not answered, and
    /// each change of that.
    pub(crate) fn open(&self) -> watch::Receiver<usize> {
        self.open.subscribe()
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
                source: None,
            })?;
        // Counted before the parent can take it, so a turn that sees the
        // questionnaire arrive also sees it open.
        let _open = OpenQuestion::new(&self.open);
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
                source: None,
            })?,
            () = cancellation.cancelled() => return Err(Error::Cancelled),
        }
        tokio::select! {
            result = response_rx => result.map_err(|_| Error::Interrupted {
                message: "delegated questionnaire was dropped without a response".into(),
                source: None,
            })?,
            () = cancellation.cancelled() => Err(Error::Cancelled),
        }
    }
}

#[cfg(test)]
#[path = "subagent_host_input_tests.rs"]
mod tests;
