//! `session/prompt`: one prompt of this host's, from the daemon taking it
//! to the end of its turn.
//!
//! The session's pump is armed first ([`Waiter`]), so the turn's user
//! message and end, which arrive as events, are matched to the prompt. The
//! daemon answers the prompt once it saved it, or with the turn's outcome
//! when the turn ended first (a cancel that beat it).

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, PoisonError,
};

use agent_client_protocol::{
    schema::v1::{ContentBlock, MessageId, SessionId as AcpSessionId, StopReason},
    Error as AcpError, ErrorCode,
};
use rho_protocol::{
    methods::SessionPrompt, AnswerStop, ClientEnvironment, SessionPromptParams, TurnError,
    TurnErrorKind, TurnOutcome,
};
use tokio::sync::oneshot;

use super::{
    daemon::{self, Inbound},
    requests,
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
        let link = self.daemon.link().await?;
        // Earlier events, such as the end of a turn this host cancelled,
        // reach the pump before it is armed.
        link.fence().await;
        let (outcome, ended) = oneshot::channel();
        let accept = Arc::new(Mutex::new(accept));
        let waiter = Waiter {
            outcome,
            accept: Arc::clone(&accept),
        };
        live.inbound
            .send(Inbound::Session(Control::Arm(waiter)))
            .map_err(|_| AcpError::resource_not_found(Some(session_id.0.to_string())))?;
        let params = SessionPromptParams {
            session: live.id.clone(),
            content: requests::prompt_content(content),
            timeout_ms: None,
            environment: ClientEnvironment::of_this_process(),
        };
        let outcome = match link.request::<SessionPrompt>(&params).await {
            Ok(_) => ended.await.map_err(|_| daemon::lost())??,
            Err(error) => {
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
        TurnErrorKind::Authentication => {
            AcpError::new(i32::from(ErrorCode::AuthRequired), error.message)
        }
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
