//! `session/prompt`: one prompt of this host's, from the daemon taking it
//! to the end of its turn.
//!
//! The session's pump is armed first ([`Waiter`]) and reads no event until
//! the daemon answered the prompt with its turn's id, so the turn's user
//! message and end, which arrive as events, are matched to the prompt by
//! that id. The daemon answers the prompt once it saved it, or with the
//! turn's outcome when the turn ended first (a cancel that beat it).

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, PoisonError,
};

use agent_client_protocol::{
    v1::schema::{ContentBlock, MessageId, SessionId as AcpSessionId, StopReason},
    Error as AcpError, ErrorCode,
};
use rho_protocol::{
    methods::SessionPrompt, AnswerStop, ClientEnvironment, SessionPromptParams, TurnError,
    TurnErrorKind, TurnOutcome,
};
use tokio::sync::oneshot;

use super::{
    daemon, requests,
    sessions::{Accept, Control, Waiter},
    Adapter,
};

impl Adapter {
    /// Runs `content` as a prompt on session `session_id` until its turn
    /// ends. `accept`, for ACP v2, hears the id of the prompt's user message
    /// once the daemon saved it; a prompt whose turn ended before that never
    /// calls it.
    pub(super) async fn prompt(
        &self,
        session_id: &AcpSessionId,
        content: &[ContentBlock],
        accept: Option<Accept>,
    ) -> Result<StopReason, AcpError> {
        let live = self.live(session_id).await?;
        let _turn = match live.prompt.try_lock() {
            Ok(turn) => turn,
            // A host that cancels and prompts again at once waits for the
            // cancelled turn.
            Err(_) if live.cancelling.load(Ordering::Acquire) => live.prompt.lock().await,
            Err(_) => {
                return Err(AcpError::invalid_request().data(format!(
                    "session '{}' already has an active prompt",
                    session_id.0
                )));
            }
        };
        let _cancelled = ClearOnDrop(&live.cancelling);
        // The host closed the session while this prompt queued behind a
        // cancelled one.
        if live.closing.load(Ordering::Acquire) {
            return Ok(StopReason::Cancelled);
        }
        let link = self.daemon.link().await?;
        let (outcome, ended) = oneshot::channel();
        let accept = Arc::new(Mutex::new(accept));
        let waiter = Waiter {
            outcome,
            accept: Arc::clone(&accept),
        };
        let (began, turn) = oneshot::channel();
        live.deliver(&link, Control::Arm { waiter, turn }).await?;
        let params = SessionPromptParams {
            author: rho_protocol::PromptAuthor::Person,
            session: live.id.clone(),
            content: requests::prompt_content(content),
            timeout_ms: None,
            environment: ClientEnvironment::of_this_process(),
            idempotency_key: None,
            grant: None,
        };
        let outcome = match link.request::<SessionPrompt>(&params).await {
            Ok(response) => {
                // A pump that is gone fails `ended`.
                let _ = began.send(response.turn);
                ended.await.map_err(|_| daemon::lost())??
            }
            Err(error) => {
                // No turn to wait on: the pump reads on.
                drop(began);
                // What the turn showed before it ended, such as a menu its
                // end refreshed, reaches the host before this answer.
                let (flushed, done) = oneshot::channel();
                if live.deliver(&link, Control::Flush(flushed)).await.is_ok() {
                    let _ = done.await;
                }
                return match error.turn_ended() {
                    Some(outcome) => stop_reason(outcome),
                    None => Err(daemon::daemon_error(&error)),
                };
            }
        };
        // A saved prompt was accepted even if its user message never
        // reached this host.
        let unaccepted = accept.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(accept) = unaccepted {
            accept(MessageId::new(format!("rho-user-{}", uuid::Uuid::new_v4())));
        }
        stop_reason(outcome)
    }
}

/// The ACP stop reason of a turn that ended `outcome`, or the error of one
/// that failed.
fn stop_reason(outcome: TurnOutcome) -> Result<StopReason, AcpError> {
    match outcome {
        TurnOutcome::Answered { stop, .. } => Ok(match stop {
            AnswerStop::EndTurn | AnswerStop::Unknown => StopReason::EndTurn,
            AnswerStop::MaxTokens => StopReason::MaxTokens,
            AnswerStop::Refusal => StopReason::Refusal,
            // A reason a newer daemon sends.
            _ => StopReason::EndTurn,
        }),
        TurnOutcome::MaxSteps { .. } => Ok(StopReason::MaxTurnRequests),
        TurnOutcome::Cancelled | TurnOutcome::TimedOut { .. } => Ok(StopReason::Cancelled),
        TurnOutcome::Failed { error } => Err(turn_error(error)),
        // `Unknown`, or an outcome a newer daemon sends.
        _ => Ok(StopReason::EndTurn),
    }
}

/// `auth_required` when signing in can fix the failure, `invalid_params`
/// for a config the session cannot run, else `internal_error`. The daemon
/// writes the message for a person and never quotes credentials in it.
fn turn_error(error: TurnError) -> AcpError {
    match error.kind {
        TurnErrorKind::Authentication => AcpError::new(ErrorCode::AUTH_REQUIRED, error.message),
        TurnErrorKind::Configuration => AcpError::invalid_params().data(error.message),
        TurnErrorKind::Provider
        | TurnErrorKind::ToolHost
        | TurnErrorKind::Other
        | TurnErrorKind::Unknown => AcpError::internal_error().data(error.message),
        // A kind a newer daemon sends.
        _ => AcpError::internal_error().data(error.message),
    }
}

/// Clears a session's cancel flag when its prompt ends.
struct ClearOnDrop<'a>(&'a AtomicBool);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
