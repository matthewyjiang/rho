//! The ACP v2 prompt lifecycle around Rho's v1 prompt.
//!
//! In v1 the `session/prompt` response is the turn: it carries the stop
//! reason. In v2 it only acknowledges that the user message was inserted, and
//! the turn reports itself through `state_update`s:
//!
//! 1. The session inserts the user message (a model turn saves it first, so
//!    replay reuses its ID; see `prompt_acceptance`): Rho sends the
//!    `user_message`, reports `running`, and answers with the message's ID.
//! 2. The turn streams its updates as in v1.
//! 3. The turn ends: `idle` with the stop reason, or with `error` when it
//!    failed after acceptance.
//!
//! A prompt that stops before its message is inserted (busy or unknown
//! session, a rejected command, a cancel or close first) gets an error
//! response and no updates, which v2 reserves for prompts never inserted.

use std::sync::{Arc, Mutex, PoisonError};

use agent_client_protocol::schema::{v1, v2};
use agent_client_protocol::{Client, Error as AcpError, Responder, V2ConnectionTo};

use super::{port::V2Port, requests};
use crate::app::acp::{prompt_acceptance::PromptAcceptance, startup::LazyAgent, AcpClientPort};

pub(super) fn spawn(
    agent: Arc<LazyAgent>,
    request: v2::PromptRequest,
    responder: Responder<v2::PromptResponse>,
    cx: V2ConnectionTo<Client>,
) -> Result<(), AcpError> {
    let port = Arc::new(V2Port::new(cx.clone()));
    cx.spawn(async move {
        let started = async {
            let v1_request = requests::prompt_request(&request)?;
            Ok::<_, AcpError>((agent.ready().await?, v1_request))
        };
        let (agent, v1_request) = match started.await {
            Ok(started) => started,
            Err(error) => return responder.respond_with_error(error),
        };
        let session_id = request.session_id.clone();
        let unanswered = Arc::new(Mutex::new(Some(responder)));
        let acceptance = PromptAcceptance::Report(Box::new({
            let port = Arc::clone(&port);
            let unanswered = Arc::clone(&unanswered);
            let session_id = session_id.clone();
            let prompt = request.prompt.clone();
            move |message_id| acknowledge(&port, &session_id, prompt, message_id, &unanswered)
        }));
        let result = agent
            .prompt(
                v1_request,
                Arc::clone(&port) as Arc<dyn AcpClientPort>,
                acceptance,
            )
            .await;
        if let Some(responder) = take(&unanswered) {
            let error = match result {
                Err(error) => error,
                // The prompt stopped before its message was inserted: a
                // cancel during command expansion, or a closing session.
                Ok(_) => {
                    AcpError::request_cancelled().data("the prompt was cancelled before it started")
                }
            };
            return responder.respond_with_error(error);
        }
        let stop_reason = match &result {
            Ok(response) => requests::stop_reason(response.stop_reason),
            Err(error) => requests::failed(error),
        };
        port.send_update(
            &session_id,
            v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(
                v2::IdleStateUpdate::new().stop_reason(stop_reason),
            )),
        )
    })
}

/// Reports the inserted user message, `running`, and the prompt's answer.
fn acknowledge(
    port: &V2Port,
    session_id: &v2::SessionId,
    prompt: Vec<v2::ContentBlock>,
    message_id: v1::MessageId,
    unanswered: &Mutex<Option<Responder<v2::PromptResponse>>>,
) {
    let message_id = v2::MessageId::new(message_id.0);
    let user_message =
        v2::SessionUpdate::UserMessage(v2::UserMessage::new(message_id.clone()).content(prompt));
    let running =
        v2::SessionUpdate::StateUpdate(v2::StateUpdate::Running(v2::RunningStateUpdate::new()));
    let sent = port
        .send_update(session_id, user_message)
        .and_then(|()| port.send_update(session_id, running));
    let answered = take(unanswered).map_or(Ok(()), |responder| {
        responder.respond(v2::PromptResponse::new(message_id))
    });
    if let Err(error) = sent.and(answered) {
        tracing::warn!(%error, "could not acknowledge ACP v2 prompt");
    }
}

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}
