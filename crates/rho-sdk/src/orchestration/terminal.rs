use std::sync::Arc;

use tokio::sync::mpsc;

use crate::{
    event::RunOutcome,
    model::Message,
    session::{SessionCore, SessionState},
    Error, Retryability, RunEvent,
};

use super::stream_capture::StreamCapture;

/// Cooperative terminal outcome that commits recoverable candidate history.
pub(super) enum TerminalKind {
    Cancelled,
    Failed(Error),
}

/// Commit candidate history after a cooperative terminal outcome.
///
/// Cancel and failure share one path: keep the in-flight user turn and any
/// completed steps, retain partial provider output as `AbortedAssistant` when
/// present, bump the revision, emit the matching terminal event, and return the
/// terminal error. Event-consumer interrupts are not routed here and still leave
/// uncommitted candidate history uninstalled.
///
/// `Cancelled` and `Failed` both carry the new revision on the event.
pub(super) async fn commit_terminal(
    core: Arc<SessionCore>,
    mut history: Vec<Message>,
    capture: StreamCapture,
    kind: TerminalKind,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    if let Some(aborted) = capture.into_aborted_assistant() {
        history.push(Message::AbortedAssistant(Box::new(aborted)));
    }
    commit_terminal_history(core, history, kind, events).await
}

pub(super) async fn commit_terminal_history(
    core: Arc<SessionCore>,
    history: Vec<Message>,
    kind: TerminalKind,
    events: &mpsc::Sender<RunEvent>,
) -> Result<RunOutcome, Error> {
    let revision = core.commit(history)?;
    match kind {
        TerminalKind::Cancelled => {
            core.set_state(SessionState::Cancelling);
            send_terminal(events, RunEvent::Cancelled { revision }).await;
            Err(Error::Cancelled)
        }
        TerminalKind::Failed(error) => {
            core.set_state(SessionState::Failed);
            emit_failure(events, &error, revision).await;
            Err(error)
        }
    }
}

pub(super) async fn send_terminal(events: &mpsc::Sender<RunEvent>, event: RunEvent) {
    let _ = events.send(event).await;
}

async fn emit_failure(events: &mpsc::Sender<RunEvent>, error: &Error, revision: crate::Revision) {
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
            revision,
        },
    )
    .await;
}
