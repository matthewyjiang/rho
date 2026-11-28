//! A session's open input requests, put to the host.
//!
//! The daemon asks every attached client (`SessionEvent::InputRequested`);
//! the first answer wins and every client hears `InputResolved`. Each open
//! request is one task here that asks the host and sends its answer back;
//! a resolution, from this host or another client, aborts the task, which
//! drops the host request (the connection then cancels it).
//!
//! - An approval is `session/request_permission` on the call's card, or on
//!   a placeholder card for a call the host never saw.
//! - A plan approval is `session/request_permission` offering the modes.
//! - A question is a form-mode `elicitation/create` on hosts that draw
//!   forms. A host without forms leaves it to the other clients: sessions
//!   it opens ask none (the daemon declines an MCP server's question
//!   itself), but a session another client opened may.
//!
//! Closing a session on ACP v2 withdraws its open requests from the host
//! before it answers ([`Asks::withdrawn`]).

use std::{
    collections::HashMap,
    future::Future,
    sync::{atomic::AtomicU64, Arc},
};

use agent_client_protocol::{v1::schema::SessionId as AcpSessionId, Error as AcpError};
use rho_protocol::{
    methods::{SessionAnswer, SessionHold},
    Ask, ClientError, ErrorCode, InputAnswer, InputRequest, InputRequestId, SessionAnswerParams,
    SessionHoldParams, SessionId,
};
use tokio::{sync::watch, task::JoinHandle};

use super::{
    elicitation,
    host_features::{Forms, HostFeatures},
    permission::{self, Card},
    AcpClientPort,
};
use crate::app::daemon_client::Daemon;

/// What every input task of a session shares.
#[derive(Clone)]
pub(super) struct InputContext {
    pub(super) daemon: Arc<Daemon>,
    pub(super) session: SessionId,
    pub(super) acp_id: AcpSessionId,
    pub(super) port: Arc<dyn AcpClientPort>,
    pub(super) features: HostFeatures,
    /// Numbers the placeholder cards of calls the host never saw, across
    /// this process's sessions.
    pub(super) placeholders: Arc<AtomicU64>,
    /// The session's input tasks, each of which may have a request open on
    /// the host.
    pub(super) asks: Asks,
}

/// Counts a session's input tasks until each is gone. A task that is
/// aborted drops its host request, which withdraws the request from the
/// host with `$/cancel_request` as it drops, so once none is left, none of
/// the session's requests is open on the host.
#[derive(Clone, Default)]
pub(super) struct Asks(Arc<watch::Sender<usize>>);

impl Asks {
    fn spawn(&self, ask: impl Future<Output = ()> + Send + 'static) -> JoinHandle<()> {
        self.0.send_modify(|tasks| *tasks += 1);
        // Moved into the task, so it drops with the task's future, even one
        // aborted before it first ran.
        let counted = Counted(Arc::clone(&self.0));
        tokio::spawn(async move {
            let _counted = counted;
            ask.await;
        })
    }

    /// Waits until every input task is gone: answered, or aborted, as when
    /// the session's pump stops.
    pub(super) async fn withdrawn(&self) {
        // The sender lives as long as `self`.
        let _ = self.0.subscribe().wait_for(|tasks| *tasks == 0).await;
    }
}

/// One input task, counted in its session's [`Asks`] while it lives.
struct Counted(Arc<watch::Sender<usize>>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.send_modify(|tasks| *tasks -= 1);
    }
}

/// The input requests one session has open on the host.
#[derive(Default)]
pub(super) struct Inputs {
    open: HashMap<InputRequestId, JoinHandle<()>>,
}

impl Inputs {
    /// Puts `request` to the host, unless it is already open: an attach
    /// lists the open requests, and its replay may repeat them.
    pub(super) fn open(&mut self, request: InputRequest, context: &InputContext) {
        if self
            .open
            .get(&request.id)
            .is_some_and(|task| !task.is_finished())
        {
            return;
        }
        let id = request.id;
        let task = context.asks.spawn(ask(request, context.clone()));
        self.open.insert(id, task);
    }

    /// Withdraws request `id` from the host: answered, here or elsewhere,
    /// withdrawn, or timed out.
    pub(super) fn resolve(&mut self, id: InputRequestId) {
        if let Some(task) = self.open.remove(&id) {
            task.abort();
        }
    }

    /// Withdraws every request, as when the attachment ends.
    pub(super) fn abort_all(&mut self) {
        for (_, task) in self.open.drain() {
            task.abort();
        }
    }
}

impl Drop for Inputs {
    fn drop(&mut self) {
        self.abort_all();
    }
}

async fn ask(request: InputRequest, context: InputContext) {
    let answer = match host_answer(&request, &context).await {
        Ok(Some(answer)) => answer,
        // The host cannot answer, or cancelled: another client may answer,
        // or the turn's cancel withdraws the request.
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, "could not ask the ACP host for input");
            return;
        }
    };
    let retry = matches!(answer, InputAnswer::Answers { .. });
    match send(&context, request.id, answer).await {
        // An answer the daemon finds invalid, such as a choice the question
        // does not offer, declines instead so the tool is not left waiting.
        Err(Unsent::Failed(error))
            if retry && error.error_code() == Some(ErrorCode::InvalidParams) =>
        {
            if let Err(unsent) = send(&context, request.id, InputAnswer::Decline).await {
                tracing::warn!(?unsent, "could not decline an input request");
            }
        }
        Ok(()) => {}
        Err(unsent) => tracing::warn!(?unsent, "could not answer an input request"),
    }
}

/// What the host answered, or `None` when it cannot answer or cancelled
/// the prompt.
async fn host_answer(
    request: &InputRequest,
    context: &InputContext,
) -> Result<Option<InputAnswer>, AcpError> {
    let InputContext {
        acp_id,
        port,
        features,
        placeholders,
        ..
    } = context;
    match &request.ask {
        Ask::Approval(ask) => {
            let card = Card::of(request.tool_call.as_ref(), placeholders);
            let prompt = permission::permission_request(acp_id, ask, &card);
            let response = port.request_permission(prompt).await?;
            Ok(permission::approval_answer(&response.outcome))
        }
        Ask::PlanApproval(ask) => {
            let card = Card::of(request.tool_call.as_ref(), placeholders);
            let prompt = permission::plan_approval_request(acp_id, ask, &card);
            let response = port.request_permission(prompt).await?;
            Ok(permission::plan_answer(&response.outcome))
        }
        Ask::Question(ask) => {
            let Forms::Available(dialect) = features.forms else {
                return Ok(None);
            };
            // A person may take longer to fill a form than the request's
            // timeout allows; holding it waits for them.
            if request.timeout.is_some() {
                if let Err(unsent) = hold(context, request.id).await {
                    tracing::debug!(?unsent, "could not hold an input request");
                }
            }
            let form =
                elicitation::elicitation_request(ask, acp_id, request.tool_call.as_ref(), dialect);
            let response = port.create_elicitation(form).await?;
            Ok(Some(elicitation::host_answer(ask, response, dialect)))
        }
        // `Unknown`, or an ask a newer daemon sends: the host cannot answer
        // it, and another client may.
        _ => Ok(None),
    }
}

/// Why an answer or hold did not reach the daemon.
#[derive(Debug)]
enum Unsent {
    /// The connection the request came over is gone; a new daemon would
    /// not have it.
    Disconnected,
    /// The daemon refused it, or the connection failed under it.
    Failed(ClientError),
}

/// Answers request `id`. A request already resolved elsewhere is not an
/// error: the first answer wins.
async fn send(
    context: &InputContext,
    id: InputRequestId,
    answer: InputAnswer,
) -> Result<(), Unsent> {
    let params = SessionAnswerParams {
        session: context.session.clone(),
        request: id,
        answer,
    };
    let link = context.daemon.current().await.ok_or(Unsent::Disconnected)?;
    match link.request::<SessionAnswer>(&params).await {
        Ok(_) => Ok(()),
        Err(error) if error.error_code() == Some(ErrorCode::InputNotOpen) => Ok(()),
        Err(error) => Err(Unsent::Failed(error)),
    }
}

async fn hold(context: &InputContext, id: InputRequestId) -> Result<(), Unsent> {
    let params = SessionHoldParams {
        session: context.session.clone(),
        request: id,
    };
    let link = context.daemon.current().await.ok_or(Unsent::Disconnected)?;
    link.request::<SessionHold>(&params)
        .await
        .map(drop)
        .map_err(Unsent::Failed)
}
