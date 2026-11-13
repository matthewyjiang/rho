//! The ACP v2 prompt lifecycle around Rho's v1 prompt.
//!
//! In v1 the `session/prompt` response is the turn: it carries the stop
//! reason. In v2 it only acknowledges that the user message was inserted, and
//! the turn reports itself through `state_update`s:
//!
//! 1. The daemon saves the user message, under the ID its replay reuses
//!    (the prompt's accept callback, `sessions::Accept`): Rho sends the
//!    `user_message`, reports `running`, and answers with the message's ID.
//! 2. The turn streams its updates as in v1.
//! 3. The turn ends: `idle` with the stop reason, or with `error` when it
//!    failed after acceptance.
//!
//! Both state updates list the session's background work as the host was
//! last told it, holding it while they send ([`super::background`]).
//!
//! A prompt that stops before its message is inserted (busy or unknown
//! session, a rejected command, a cancel or close first) gets an error
//! response and no updates, which v2 reserves for prompts never inserted.

use std::sync::{Arc, Mutex, PoisonError};

use agent_client_protocol::{v1::schema as v1, v2::schema as v2};
use agent_client_protocol::{
    v2::{ConnectionTo as V2ConnectionTo, Responder},
    Client, Error as AcpError,
};

use super::{background, port::V2Port, requests};
use crate::app::acp::{sessions::Accept, Adapter, Foreground, TurnEnd};

pub(super) fn spawn(
    adapter: Arc<Adapter>,
    request: v2::PromptRequest,
    responder: Responder<v2::PromptResponse>,
    cx: V2ConnectionTo<Client>,
) -> Result<(), AcpError> {
    let port = Arc::new(V2Port::new(cx.clone()));
    cx.spawn(async move {
        let v1_request = match requests::prompt_request(&request) {
            Ok(v1_request) => v1_request,
            Err(error) => return responder.respond_with_error(error),
        };
        let session_id = request.session_id.clone();
        let unanswered = Arc::new(Mutex::new(Some(responder)));
        let accept: Accept = Box::new({
            let port = Arc::clone(&port);
            let unanswered = Arc::clone(&unanswered);
            let session_id = session_id.clone();
            let prompt = request.prompt.clone();
            let adapter = Arc::clone(&adapter);
            let v1_session_id = v1_request.session_id.clone();
            move |message_id| {
                adapter.with_background(&v1_session_id, |work| {
                    let running = background::state(Foreground::Running, work);
                    acknowledge(&port, &session_id, prompt, message_id, running, &unanswered);
                });
            }
        });
        let result = adapter
            .prompt(&v1_request.session_id, &v1_request.prompt, Some(accept))
            .await;
        if let Some(responder) = take(&unanswered) {
            let error = match result {
                Err(error) => error,
                // The turn ended before the daemon saved the prompt, as a
                // cancel that beat it does.
                Ok(_) => {
                    AcpError::request_cancelled().data("the prompt was cancelled before it started")
                }
            };
            return responder.respond_with_error(error);
        }
        let end = TurnEnd::Ended(result);
        adapter.with_background(&v1_request.session_id, |work| {
            port.send_update(&session_id, requests::idle(&end, work))
        })
    })
}

/// Reports the inserted user message, `running`, and the prompt's answer.
fn acknowledge(
    port: &V2Port,
    session_id: &v2::SessionId,
    prompt: Vec<v2::ContentBlock>,
    message_id: v1::MessageId,
    running: v2::SessionUpdate,
    unanswered: &Mutex<Option<Responder<v2::PromptResponse>>>,
) {
    let message_id = v2::MessageId::new(message_id.0);
    let user_message =
        v2::SessionUpdate::UserMessage(v2::UserMessage::new(message_id.clone()).content(prompt));
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
