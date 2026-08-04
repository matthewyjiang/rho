use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    event::RunOutcome,
    model::Message,
    session::{SessionCore, SessionState},
    Error, Retryability, RunEvent,
};

use super::stream_capture::StreamCapture;

pub(super) async fn commit_cancellation(
    core: Arc<SessionCore>,
    mut history: Vec<Message>,
    capture: StreamCapture,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    if let Some(aborted) = capture.into_aborted_assistant() {
        history.push(Message::AbortedAssistant(Box::new(aborted)));
    }
    commit_cancelled_history(core, history, events).await
}

pub(super) async fn commit_cancelled_history(
    core: Arc<SessionCore>,
    history: Vec<Message>,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    let revision = core.commit(history)?;
    core.set_state(SessionState::Cancelling);
    send_terminal(events, RunEvent::Cancelled { revision }).await;
    Err(Error::Cancelled)
}

/// Keep in-flight turn progress after a terminal provider/run failure.
///
/// Cancellation already commits partial history. Provider failures used to drop
/// the user turn and any completed steps, so the next run could not resume.
/// Commit the same progress, keep any streamed partial assistant output, and
/// append a short failure notice the model can read on the next turn.
pub(super) async fn commit_failure(
    core: Arc<SessionCore>,
    mut history: Vec<Message>,
    capture: StreamCapture,
    error: Error,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    if let Some(aborted) = capture.into_aborted_assistant() {
        history.push(Message::AbortedAssistant(Box::new(aborted)));
    }
    history.push(failure_context_message(&error));
    core.commit(history)?;
    core.set_state(SessionState::Failed);
    emit_failure(events, &error).await;
    Err(error)
}

fn failure_context_message(error: &Error) -> Message {
    Message::user_text(format!("[{error}]"))
}

pub(super) async fn send_terminal(events: &mpsc::Sender<RunEvent>, event: RunEvent) {
    let _ = events.send(event).await;
}

async fn emit_failure(events: &mpsc::Sender<RunEvent>, error: &Error) {
    let diagnostic = match error {
        Error::Provider(error) => error.diagnostic(),
        _ => None,
    };
    if let Some(detail) = diagnostic {
        send_terminal(
            events,
            RunEvent::ProviderDiagnostic {
                detail: crate::ProviderDiagnostic::new(detail),
            },
        )
        .await;
    }
    send_terminal(
        events,
        RunEvent::Failed {
            message: error.to_string(),
            retryability: if error.is_retryable() {
                Retryability::Retryable
            } else {
                Retryability::Permanent
            },
        },
    )
    .await;
}
