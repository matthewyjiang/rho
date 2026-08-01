//! Text and JSONL presentation for non-TUI workflow runs.

use std::{io, sync::Arc};

use crate::{
    app::workflow_runtime::{RecoveryDecision, RuntimeEvent, WorkflowRunner},
    workflow::{RunId, StoredRun},
};

use super::super::{write_json_document, WORKFLOW_WIRE_VERSION};

#[derive(Clone, Copy)]
pub(super) enum RuntimePresentation {
    Text,
    Jsonl,
}

pub(super) async fn drive_with_stream(
    runner: Arc<WorkflowRunner>,
    run: &StoredRun,
    recovery: RecoveryDecision,
    presentation: RuntimePresentation,
) -> anyhow::Result<bool> {
    let cancellation = runner.cancellation_request(run.manifest.run_id);
    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
    let presenter = tokio::spawn(present_runtime_events(
        event_rx,
        presentation,
        run.manifest.run_id,
    ));
    let interrupted = {
        let drive = runner.drive(run.manifest.run_id, recovery, Some(event_tx));
        tokio::pin!(drive);
        tokio::select! {
            result = drive.as_mut() => {
                result?;
                false
            }
            result = workflow_shutdown_signal() => {
                result?;
                cancellation.request()?;
                drive.as_mut().await?;
                true
            }
        }
    };
    presenter
        .await
        .map_err(|error| anyhow::anyhow!("workflow event presenter failed: {error}"))??;
    Ok(interrupted)
}

async fn present_runtime_events(
    mut events: tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    presentation: RuntimePresentation,
    run_id: RunId,
) -> anyhow::Result<()> {
    let mut sequence = 0_u64;
    while let Some(event) = events.recv().await {
        sequence = sequence.saturating_add(1);
        match presentation {
            RuntimePresentation::Text => println!("{}", event.message()),
            RuntimePresentation::Jsonl => {
                let value = runtime_event_json(sequence, run_id, &event);
                write_json_document(&value)?;
            }
        }
    }
    Ok(())
}

pub(super) fn runtime_event_json(
    sequence: u64,
    run_id: RunId,
    event: &RuntimeEvent,
) -> serde_json::Value {
    let mut value = serde_json::to_value(event).expect("RuntimeEvent serializes");
    let object = value
        .as_object_mut()
        .expect("RuntimeEvent serializes to an object");
    object.insert("version".into(), WORKFLOW_WIRE_VERSION.into());
    object.insert("sequence".into(), sequence.into());
    object.insert("run_id".into(), run_id.to_string().into());
    value
}

#[cfg(unix)]
async fn workflow_shutdown_signal() -> io::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => Ok(()),
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn workflow_shutdown_signal() -> io::Result<()> {
    tokio::signal::ctrl_c().await
}
