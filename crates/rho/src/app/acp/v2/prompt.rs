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
//!    failed after acceptance. The session's pump sends it as it reads the
//!    end (`V2Port::turn_ended`), so no later update it sends, a delivery
//!    turn's `running` or a background change, can come first.
//!
//! Both state updates list the session's background work as the host was
//! last told it, holding it while they send ([`super::background`]).
//!
//! A prompt that arrives while the session is busy, with this host's
//! prompt, another client's, or a turn the daemon started, is held, its
//! response pending, and inserted as the session's next turn
//! (`prompt::hold`). A prompt that stops before its message is inserted
//! (unknown session, a rejected command, a `session/cancel` or
//! `$/cancel_request` while it is held, a close) gets an error response
//! and no updates, which v2 reserves for prompts never inserted.
//!
//! A prompt's `idempotencyKey` makes a retry safe. A retry of a prompt this
//! connection still holds, or already inserted, answers as that prompt does
//! (`prompt::hold`). Any other goes to the daemon, which knows the key while
//! the session keeps the prompt's user message: a retry it recognizes is
//! answered with the original `messageId` and sends no `user_message` and
//! no state, as no message was inserted and no work started.

use std::sync::{Arc, Mutex, PoisonError};

use agent_client_protocol::{v1::schema as v1, v2::schema as v2};
use agent_client_protocol::{
    v2::{ConnectionTo as V2ConnectionTo, Responder},
    Client, Error as AcpError,
};

use super::{background, port::V2Port, requests};
use crate::app::acp::{
    prompt::{BusyPrompt, Entry, Prompted, Retries},
    sessions::Accept,
    Adapter, Foreground,
};

pub(super) fn spawn(
    adapter: Arc<Adapter>,
    request: v2::PromptRequest,
    responder: Responder<v2::PromptResponse>,
    cx: V2ConnectionTo<Client>,
) -> Result<(), AcpError> {
    let v1_request = match requests::prompt_request(&request) {
        Ok(v1_request) => v1_request,
        Err(error) => return responder.respond_with_error(error),
    };
    // Before the connection reads on, so the prompt is in line ahead of
    // every prompt and cancel the host sent after it.
    let place = match adapter.hold(&v1_request.session_id, responder.cancellation()) {
        Ok(place) => place,
        Err(error) => return responder.respond_with_error(error),
    };
    let (place, retries) = match place.keyed(request.idempotency_key.clone()) {
        Entry::Held { place, retries } => (place, retries),
        Entry::Retry(retry) => {
            return cx.spawn(async move {
                match retry.outcome().await {
                    Ok(message_id) => responder.respond(v2::PromptResponse::new(message_id)),
                    Err(error) => responder.respond_with_error(error),
                }
            });
        }
    };
    let port = Arc::new(V2Port::new(cx.clone()));
    cx.spawn(async move {
        let session_id = request.session_id.clone();
        let unanswered = Arc::new(Mutex::new(Some(Answer { responder, retries })));
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
            .prompt(
                &v1_request.session_id,
                &v1_request.prompt,
                Some(accept),
                BusyPrompt::Hold(place),
            )
            .await;
        let Some(answer) = take(&unanswered) else {
            return Ok(());
        };
        match result {
            Ok(Prompted::SentAgain { turn, message }) => {
                // The daemon names the first prompt's message unless that
                // prompt inserted none it knew, which a turn `rho acp`
                // starts always does: the retry gets one named after the
                // turn.
                let message_id = message.map_or_else(
                    || format!("rho-user-{turn}"),
                    |message| message.as_str().to_owned(),
                );
                answer.inserted(v2::MessageId::new(message_id))
            }
            // The turn ended before the daemon saved the prompt, as a
            // cancel that beat it does.
            Ok(Prompted::Ended(_)) => answer.failed(
                AcpError::request_cancelled().data("the prompt was cancelled before it started"),
            ),
            Err(error) => answer.failed(error),
        }
    })
}

/// A prompt's response until it is sent, and the retries on this
/// connection that hear its outcome then.
struct Answer {
    responder: Responder<v2::PromptResponse>,
    retries: Option<Retries>,
}

impl Answer {
    fn inserted(self, message_id: v2::MessageId) -> Result<(), AcpError> {
        let answered = self
            .responder
            .respond(v2::PromptResponse::new(message_id.clone()));
        if let Some(retries) = self.retries {
            retries.tell(Ok(message_id));
        }
        answered
    }

    fn failed(self, error: AcpError) -> Result<(), AcpError> {
        if let Some(retries) = self.retries {
            retries.tell(Err(error.clone()));
        }
        self.responder.respond_with_error(error)
    }
}

/// Reports the inserted user message, `running`, and the prompt's answer.
fn acknowledge(
    port: &V2Port,
    session_id: &v2::SessionId,
    prompt: Vec<v2::ContentBlock>,
    message_id: v1::MessageId,
    running: v2::SessionUpdate,
    unanswered: &Mutex<Option<Answer>>,
) {
    let message_id = v2::MessageId::new(message_id.0);
    let user_message =
        v2::SessionUpdate::UserMessage(v2::UserMessage::new(message_id.clone()).content(prompt));
    let sent = port
        .send_update(session_id, user_message)
        .and_then(|()| port.send_update(session_id, running));
    let answered = take(unanswered).map_or(Ok(()), |answer| answer.inserted(message_id));
    if let Err(error) = sent.and(answered) {
        tracing::warn!(%error, "could not acknowledge ACP v2 prompt");
    }
}

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}
