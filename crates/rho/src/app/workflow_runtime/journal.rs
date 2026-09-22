use std::path::PathBuf;

use crate::workflow::{
    apply_durable_event, attempt_directory, AttemptNumber, AttemptRecord, AttemptState,
    NodeCompletion, RunMutationGuard, StoredRun, TaskInstanceId, WorkflowEvent,
    WorkflowEventRecord, WorkflowStore, ATTEMPT_VERSION, EVENT_VERSION,
};

use super::{runner::send_event, RuntimeError, RuntimeEvent};

/// The single writer for a run. The journal is authoritative; snapshots may lag
/// append-only events or any append interrupted before its snapshot write.
pub(super) struct RunJournal {
    pub(super) store: WorkflowStore,
    pub(super) guard: RunMutationGuard,
    pub(super) directory: PathBuf,
    pub(super) run: StoredRun,
}

impl RunJournal {
    pub(super) fn commit(&mut self, event: WorkflowEvent) -> Result<(), RuntimeError> {
        let record = &mut self.run.state;
        let next = apply_durable_event(&self.run.graph, &record.state, &event, &self.directory)?;
        let sequence = record
            .last_event_sequence
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Data("workflow event sequence overflow".into()))?;
        // These events are consumed by the next saving commit. In particular an
        // intent must precede attempt files, and output must precede completion.
        let append_only = match &event {
            WorkflowEvent::LaunchIntended { .. } | WorkflowEvent::StructuredOutput { .. } => true,
            WorkflowEvent::ScopeFinished { .. }
            | WorkflowEvent::ScopeReopened { .. }
            | WorkflowEvent::RunLifecycle { .. }
            | WorkflowEvent::CancellationRequested { .. }
            | WorkflowEvent::NodeReady { .. }
            | WorkflowEvent::AttemptStarted { .. }
            | WorkflowEvent::NodeFinished { .. }
            | WorkflowEvent::NodeReset { .. }
            | WorkflowEvent::CancellationCleared
            | WorkflowEvent::CancellationAcknowledged { .. }
            | WorkflowEvent::HookObserved { .. } => false,
        };
        self.store.append_event(
            &mut self.guard,
            &WorkflowEventRecord {
                schema_version: EVENT_VERSION,
                sequence,
                event,
            },
        )?;
        record.last_event_sequence = sequence;
        record.state = next;
        if !append_only {
            self.store.save_state(&mut self.guard, record)?;
        }
        Ok(())
    }

    pub(super) fn commit_and_notify(
        &mut self,
        event: WorkflowEvent,
        sender: &Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> Result<(), RuntimeError> {
        self.commit(event)?;
        self.notify(sender);
        Ok(())
    }

    pub(super) fn notify(&self, sender: &Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>) {
        send_event(
            sender,
            RuntimeEvent::StateChanged {
                revision: self.run.state.state.revision,
            },
        );
    }
}

pub(super) fn completed_attempt(
    run_directory: &std::path::Path,
    node: &TaskInstanceId,
    attempt: AttemptNumber,
) -> Result<Option<NodeCompletion>, RuntimeError> {
    let record = read_attempt_record(run_directory, node, attempt)?;
    Ok(match record.state {
        AttemptState::Completed { completion } => Some(*completion),
        AttemptState::LaunchIntended
        | AttemptState::Started { .. }
        | AttemptState::CleanlyCancelled
        | AttemptState::InterruptedUncertain { .. } => None,
    })
}

pub(super) fn read_attempt_record(
    run_directory: &std::path::Path,
    node: &TaskInstanceId,
    attempt: AttemptNumber,
) -> Result<AttemptRecord, RuntimeError> {
    let path = attempt_directory(run_directory, node, attempt).join("status.json");
    let relative = path
        .strip_prefix(run_directory)
        .map_err(|_| RuntimeError::UnsafeArtifact(path.clone()))?;
    let mut file = crate::workflow::open_private_file_beneath(run_directory, relative, false)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)?;
    let record: AttemptRecord = serde_json::from_slice(&bytes)?;
    crate::workflow::check_schema_version(
        "workflow attempt",
        record.schema_version,
        ATTEMPT_VERSION,
    )?;
    if record.attempt != attempt {
        return Err(RuntimeError::Data(format!(
            "attempt record for node '{node}' has the wrong attempt number"
        )));
    }
    Ok(record)
}
