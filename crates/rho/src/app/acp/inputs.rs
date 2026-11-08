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
//!   forms. Hosts without forms are declined at once: the daemon then
//!   answers the tool or MCP server that asked as declined. (Sessions such
//!   hosts open have no questionnaire tool.)

use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc},
};

use agent_client_protocol::{schema::v1::SessionId as AcpSessionId, Error as AcpError};
use rho_protocol::{
    methods::{SessionAnswer, SessionHold},
    Ask, ErrorCode, InputAnswer, InputRequest, InputRequestId, SessionAnswerParams,
    SessionHoldParams, SessionId,
};
use tokio::task::JoinHandle;

use super::{
    daemon::Daemon,
    elicitation,
    host_features::{Forms, HostFeatures},
    permission, AcpClientPort,
};

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
        let task = tokio::spawn(ask(request, context.clone()));
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
        // The host cancelled: the turn's cancel withdraws the request.
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
        Err(Some(ErrorCode::InvalidParams)) if retry => {
            if let Err(code) = send(&context, request.id, InputAnswer::Decline).await {
                tracing::warn!(?code, "could not decline an input request");
            }
        }
        Ok(()) => {}
        Err(code) => tracing::warn!(?code, "could not answer an input request"),
    }
}

/// What the host answered, or `None` when it cancelled the prompt.
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
            let placeholder = permission::next_placeholder_tool_call_id(placeholders);
            let prompt = permission::permission_request(
                acp_id,
                ask,
                request.tool_call.as_ref(),
                &placeholder,
            );
            let response = port.request_permission(prompt).await?;
            Ok(permission::approval_answer(&response.outcome))
        }
        Ask::PlanApproval(ask) => {
            let tool_call_id = match &request.tool_call {
                Some(id) => id.as_str().to_owned(),
                None => permission::next_placeholder_tool_call_id(placeholders),
            };
            let prompt = permission::plan_approval_request(acp_id, ask, &tool_call_id);
            let response = port.request_permission(prompt).await?;
            Ok(permission::plan_answer(&response.outcome))
        }
        Ask::Question(ask) => {
            let Forms::Available(dialect) = features.forms else {
                return Ok(Some(InputAnswer::Decline));
            };
            // A person may take longer to fill a form than the request's
            // timeout allows; holding it waits for them.
            if request.timeout.is_some() {
                if let Err(code) = hold(context, request.id).await {
                    tracing::debug!(?code, "could not hold an input request");
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

/// Answers request `id`. A request already resolved elsewhere is not an
/// error: the first answer wins.
async fn send(
    context: &InputContext,
    id: InputRequestId,
    answer: InputAnswer,
) -> Result<(), Option<ErrorCode>> {
    let params = SessionAnswerParams {
        session: context.session.clone(),
        request: id,
        answer,
    };
    let link = context.daemon.link().await.map_err(|_| None)?;
    match link.request::<SessionAnswer>(&params).await {
        Ok(_) => Ok(()),
        Err(error) => match error.error_code() {
            Some(ErrorCode::InputNotOpen) => Ok(()),
            code => Err(code),
        },
    }
}

async fn hold(context: &InputContext, id: InputRequestId) -> Result<(), Option<ErrorCode>> {
    let params = SessionHoldParams {
        session: context.session.clone(),
        request: id,
    };
    let link = context.daemon.link().await.map_err(|_| None)?;
    link.request::<SessionHold>(&params)
        .await
        .map(drop)
        .map_err(|error| error.error_code())
}
