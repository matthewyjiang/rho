//! `session/prompt`: one prompt of this host's, from the daemon taking it
//! to the end of its turn.
//!
//! The session's pump is armed first ([`Waiter`]) and reads no event until
//! the daemon answered the prompt with its turn's id, so the turn's user
//! message and end, which arrive as events, are matched to the prompt by
//! that id. The daemon answers the prompt once it saved it, or with the
//! turn's outcome when the turn ended first (a cancel that beat it).
//!
//! A prompt that finds the session busy is refused on ACP v1 and held on
//! ACP v2 until the session is free ([`BusyPrompt`], `hold`).

mod hold;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use agent_client_protocol::{
    v1::schema::{ContentBlock, SessionId as AcpSessionId, StopReason},
    Error as AcpError, ErrorCode,
};
use rho_protocol::{
    methods::SessionPrompt, AnswerStop, ClientEnvironment, ClientError,
    ErrorCode as DaemonErrorCode, SessionPromptParams, TurnError, TurnErrorKind, TurnOutcome,
};
use tokio::sync::oneshot;

use super::{
    daemon,
    requests::DaemonBuild,
    sessions::{Accept, Busy, Control, Live, Waiter},
    Adapter,
};
pub(super) use hold::{Holds, Place};

/// What a prompt does while the session's foreground work is busy.
pub(super) enum BusyPrompt {
    /// Refuse it with invalid request, as ACP v1 does: a v1 host prompts
    /// once its last prompt was answered. A prompt sent right after a
    /// cancel waits for the cancelled turn instead.
    Reject,
    /// Hold it in its place in the session's line and insert it as the
    /// session's next turn once the session is free, as ACP v2 requires.
    Hold(Place),
}

impl Adapter {
    /// Runs `content` as a prompt on session `session_id` until its turn
    /// ends. `accept`, for ACP v2, hears the id of the prompt's user message
    /// once the daemon saved it, before the turn's end; a prompt whose turn
    /// ended before it began never calls it. The session's pump reports
    /// the turn's end to the host ([`super::AcpClientPort::turn_ended`]).
    /// `busy` says what a prompt does while the session runs another turn.
    pub(super) async fn prompt(
        &self,
        session_id: &AcpSessionId,
        content: &[ContentBlock],
        accept: Option<Accept>,
        busy: BusyPrompt,
    ) -> Result<StopReason, AcpError> {
        let accept = Arc::new(Mutex::new(accept));
        match busy {
            BusyPrompt::Reject => {
                let live = self.live(session_id).await?;
                let _turn = match live.prompt.try_lock() {
                    Ok(turn) => turn,
                    // A host that cancels and prompts again at once waits
                    // for the cancelled turn.
                    Err(_) if live.cancelling.load(Ordering::Acquire) => live.prompt.lock().await,
                    Err(_) => {
                        return Err(AcpError::invalid_request().data(format!(
                            "session '{}' already has an active prompt",
                            session_id.0
                        )));
                    }
                };
                let _cancelled = ClearOnDrop(&live.cancelling);
                // The host closed the session while this prompt queued
                // behind a cancelled one.
                if live.closing.load(Ordering::Acquire) {
                    return Ok(StopReason::Cancelled);
                }
                self.send_prompt(&live, content, &accept, Insert::Now)
                    .await
                    .map_err(Refused::into_error)
            }
            BusyPrompt::Hold(place) => {
                let _turn = place.take_turn().await?;
                let live = place.live();
                // The host closed the session while this prompt was held.
                if live.closing.load(Ordering::Acquire) {
                    return Ok(StopReason::Cancelled);
                }
                loop {
                    // Attaches the session, which may have gone stale while
                    // the prompt was held.
                    self.live(session_id).await?;
                    match self
                        .send_prompt(live, content, &accept, Insert::Held(&place))
                        .await
                    {
                        Ok(stop) => return Ok(stop),
                        // Another client's turn, or one the daemon started
                        // to hand the model finished background work, holds
                        // the session: the prompt stays held until it ends.
                        Err(Refused::Busy { busy, .. }) => place.until_free(busy).await?,
                        Err(Refused::Failed(error)) => return Err(error),
                    }
                }
            }
        }
    }

    /// Hands the daemon the prompt once and waits for its turn to end.
    async fn send_prompt(
        &self,
        live: &Live,
        content: &[ContentBlock],
        accept: &Arc<Mutex<Option<Accept>>>,
        insert: Insert<'_>,
    ) -> Result<StopReason, Refused> {
        let link = self.daemon.link().await.map_err(AcpError::from)?;
        let (outcome, ended) = oneshot::channel();
        let waiter = Waiter {
            outcome,
            accept: Arc::clone(accept),
        };
        let (began, turn) = oneshot::channel();
        live.deliver(&link, Control::Arm { waiter, turn }).await?;
        let params = SessionPromptParams {
            author: rho_protocol::PromptAuthor::Person,
            session: live.id.clone(),
            content: self.launch.prompt_content(
                content,
                match link.skew() {
                    None => DaemonBuild::Same,
                    Some(_) => DaemonBuild::Other,
                },
            ),
            timeout_ms: None,
            environment: ClientEnvironment::of_this_process(),
            idempotency_key: None,
            grant: None,
        };
        let busy = live.watch_busy();
        // Past this point the daemon may take the prompt.
        if let Insert::Held(place) = insert {
            place.check()?;
        }
        let error = match link.request::<SessionPrompt>(&params).await {
            Ok(response) => {
                // A pump that is gone fails `ended`.
                let _ = began.send(response.turn);
                // A cancel read while the request was on its way may have
                // reached the daemon before the prompt and stopped nothing;
                // the prompt's turn, now under way, is the one it meant.
                if matches!(insert, Insert::Held(place) if place.session_cancelled()) {
                    self.stop_turn(live).await;
                }
                return Ok(stop_reason(ended.await.map_err(|_| daemon::lost())??)?);
            }
            Err(error) => error,
        };
        // No turn to wait on: the pump reads on.
        drop(began);
        // The events the connection read before the refusal reach the host
        // before this answer, such as a menu the turn's end refreshed, and
        // reach `busy`; [`Busy::ended`] covers a state still in flight.
        let (flushed, done) = oneshot::channel();
        if live.deliver(&link, Control::Flush(flushed)).await.is_ok() {
            let _ = done.await;
        }
        if let Some(outcome) = error.turn_ended() {
            return Ok(stop_reason(outcome)?);
        }
        Err(match error.error_code() {
            Some(DaemonErrorCode::SessionBusy) => Refused::Busy { error, busy },
            _ => Refused::Failed(daemon::daemon_error(&error)),
        })
    }
}

/// How a prompt reaches the daemon.
#[derive(Clone, Copy)]
enum Insert<'a> {
    /// As it arrived (ACP v1).
    Now,
    /// Once its place in line came up, unless a cancel covered it first.
    Held(&'a Place),
}

/// Why handing the daemon a prompt did not end in a turn.
enum Refused {
    /// A turn holds the session, as `busy` saw it.
    Busy {
        error: ClientError,
        busy: Busy,
    },
    Failed(AcpError),
}

impl Refused {
    fn into_error(self) -> AcpError {
        match self {
            Self::Busy { error, .. } => daemon::daemon_error(&error),
            Self::Failed(error) => error,
        }
    }
}

impl From<AcpError> for Refused {
    fn from(error: AcpError) -> Self {
        Self::Failed(error)
    }
}

/// The ACP stop reason of a turn that ended `outcome`, or the error of one
/// that failed.
pub(super) fn stop_reason(outcome: TurnOutcome) -> Result<StopReason, AcpError> {
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

/// Clears a session's ACP v1 cancel flag when its prompt ends.
struct ClearOnDrop<'a>(&'a AtomicBool);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
