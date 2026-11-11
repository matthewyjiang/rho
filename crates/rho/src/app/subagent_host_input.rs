//! Routes structured host questionnaires from delegated agents to the parent session.

use std::collections::BTreeMap;

use rho_sdk::{
    CancellationToken, Error, HostInputId, HostInputRefusal, HostInputRequest, HostInputResponse,
    SessionId,
};
use tokio::sync::{mpsc, oneshot};

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

/// The questionnaires a host took from subagents and has not answered,
/// with what it tells the person about each, worded as the in-process TUI
/// words it. Dropping one tells its subagent nobody will answer.
#[derive(Default)]
pub(crate) struct OpenChildQuestions(BTreeMap<HostInputId, OpenChildQuestion>);

struct OpenChildQuestion {
    /// The asking run as notices name it: `<run id> (<agent id>)`.
    asker: String,
    reply: oneshot::Sender<Result<HostInputResponse, Error>>,
}

impl OpenChildQuestions {
    /// Takes `asked`. Returns the notice that says who asks, and the
    /// questionnaire to put to the person.
    pub(crate) fn open(&mut self, asked: SubagentHostInputRequest) -> (String, HostInputRequest) {
        let SubagentHostInputRequest {
            run_id,
            agent_id,
            parent_session_id: _,
            request,
            response,
        } = asked;
        let asker = format!("{run_id} ({agent_id})");
        let notice = format!("agent {asker} asks: {}", request.title());
        self.0.insert(
            request.id().clone(),
            OpenChildQuestion {
                asker,
                reply: response,
            },
        );
        (notice, request)
    }

    /// Gives questionnaire `id`'s subagent the person's `answer`. Returns
    /// the notice that says so, or `None` when no such questionnaire is
    /// open.
    pub(crate) fn answer(
        &mut self,
        id: &HostInputId,
        answer: Result<HostInputResponse, HostInputRefusal>,
    ) -> Option<String> {
        let OpenChildQuestion { asker, reply } = self.0.remove(id)?;
        let verb = match &answer {
            Ok(_) => "answered",
            Err(_) => "cancelled",
        };
        let _ = reply.send(answer.map_err(|refusal| {
            Error::Interrupted {
                message: match refusal {
                    HostInputRefusal::Declined => "delegated questionnaire declined by user",
                    // `Dismissed`, or a refusal added later.
                    _ => "delegated questionnaire cancelled by user",
                }
                .into(),
                source: None,
            }
        }));
        Some(format!("{verb} questionnaire for agent {asker}"))
    }

    /// Takes out the next questionnaire whose subagent no longer waits for
    /// an answer; never resolves while every subagent waits.
    pub(crate) async fn abandoned(&mut self) -> HostInputId {
        let id = std::future::poll_fn(|context| {
            for (id, question) in &mut self.0 {
                if question.reply.poll_closed(context).is_ready() {
                    return std::task::Poll::Ready(id.clone());
                }
            }
            std::task::Poll::Pending
        })
        .await;
        self.0.remove(&id);
        id
    }

    /// Drops every questionnaire, which tells each subagent nobody will
    /// answer. Returns their ids.
    pub(crate) fn drop_all(&mut self) -> impl Iterator<Item = HostInputId> {
        std::mem::take(&mut self.0).into_keys()
    }
}

#[derive(Clone)]
pub(crate) struct SubagentHostInputBridge {
    bridge: ParentBridge<SubagentHostInputRequest>,
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

    /// Installs `sender` as the parent's side (see
    /// [`ParentBridge::bind_parent_to`]).
    pub(crate) fn bind_parent_to(&self, sender: mpsc::Sender<SubagentHostInputRequest>) {
        self.bridge.bind_parent_to(sender);
    }

    /// How many questionnaires a parent's queue holds before a child's
    /// request waits for room.
    pub(crate) const QUEUE_CAPACITY: usize = HOST_INPUT_QUEUE_CAPACITY;

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
