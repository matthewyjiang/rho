use std::{collections::BTreeMap, path::Path};

use super::corrupt;
use crate::workflow::{
    check_schema_version, derive_workflow_outcome, secure_fs::SecureDirectory,
    store_replay::derive_snapshot, ArtifactObservation, FrozenWorkflow, NodeCompletion,
    NodeExecution, NodeId, NodeTerminalState, RunLifecycle, RunStateRecord, WorkflowError,
    WorkflowEventRecord, WorkflowResult, RUN_STATE_VERSION,
};

#[derive(Clone, Copy)]
pub(super) enum CompletionFileValidation<'a> {
    All,
    ChangedSince(&'a BTreeMap<NodeId, NodeCompletion>),
}

#[derive(Clone, Copy)]
enum ArtifactFileValidation {
    Full,
    Skip,
}

pub(super) fn validate_state(
    graph: &FrozenWorkflow,
    record: &RunStateRecord,
    events: &[WorkflowEventRecord],
    path: &Path,
    root: &SecureDirectory,
    run_relative: &Path,
) -> WorkflowResult<()> {
    check_schema_version("run state", record.schema_version, RUN_STATE_VERSION)?;
    let state = &record.state;
    if graph.graph.nodes.keys().ne(state.nodes.keys()) {
        return corrupt(path, "node state keys differ from frozen graph keys");
    }
    let tail = events.last().map_or(0, |event| event.sequence);
    if record.last_event_sequence > tail {
        return corrupt(path, "snapshot sequence is ahead of the journal tail");
    }
    let derived =
        derive_snapshot(graph, events, record.last_event_sequence, path).map_err(|error| {
            match error {
                WorkflowError::Corrupt { .. } => error,
                error => WorkflowError::Corrupt {
                    path: path.to_path_buf(),
                    reason: format!("journal prefix does not derive valid state: {error}"),
                },
            }
        })?;
    if record.state != derived {
        return corrupt(path, "snapshot state differs from its journal prefix");
    }
    validate_state_contents(
        graph,
        record,
        path,
        root,
        run_relative,
        CompletionFileValidation::All,
    )
}

/// Validates state shape on each write and checks artifact files only for
/// completions that changed since the prior persisted snapshot. Full journal
/// replay and full artifact hashing remain part of `load_run`.
pub(super) fn validate_state_contents(
    graph: &FrozenWorkflow,
    record: &RunStateRecord,
    path: &Path,
    root: &SecureDirectory,
    run_relative: &Path,
    file_validation: CompletionFileValidation<'_>,
) -> WorkflowResult<()> {
    check_schema_version("run state", record.schema_version, RUN_STATE_VERSION)?;
    let state = &record.state;
    if graph.graph.nodes.keys().ne(state.nodes.keys()) {
        return corrupt(path, "node state keys differ from frozen graph keys");
    }
    let mut retained_workflow_output = 0_u64;
    let terminal = state
        .nodes
        .iter()
        .filter_map(|(node, state)| state.terminal().map(|outcome| (node, outcome)))
        .collect::<BTreeMap<_, _>>();
    if terminal.keys().copied().ne(state.completions.keys()) {
        return corrupt(path, "completion keys differ from terminal node keys");
    }
    for (node, completion) in &state.completions {
        if terminal.get(node).copied() != Some(completion.outcome) {
            return corrupt(path, "completion outcome differs from terminal node state");
        }
        if (completion.outcome == NodeTerminalState::Cancellation)
            != completion.cancellation_resume.is_some()
        {
            return corrupt(path, "completion has an invalid cancellation resume state");
        }
        if completion.attempt.is_none()
            && (completion.command_exit.is_some()
                || completion.structured_output.is_some()
                || completion.artifacts.iter().next().is_some())
        {
            return corrupt(path, "synthetic completion contains attempt-owned data");
        }
        let definition = &graph.graph.nodes[node];
        match &definition.execution {
            NodeExecution::Agent(_)
                if completion.command_exit.is_some()
                    || completion.artifacts.stdout.is_some()
                    || completion.artifacts.stderr.is_some()
                    || completion.artifacts.command_outcome.is_some() =>
            {
                return corrupt(path, "agent completion contains command artifacts")
            }
            NodeExecution::Command(_) if completion.artifacts.answer.is_some() => {
                return corrupt(path, "command completion contains an agent answer")
            }
            NodeExecution::Command(_)
                if completion.command_exit.is_some()
                    && (completion.artifacts.stdout.is_none()
                        || completion.artifacts.stderr.is_none()
                        || completion.artifacts.command_outcome.is_none()) =>
            {
                return corrupt(path, "command exit is missing durable command artifacts")
            }
            NodeExecution::Agent(_) | NodeExecution::Command(_) => {}
        }
        if let Some(output) = &completion.structured_output {
            let schema = definition
                .output_schema()
                .ok_or_else(|| WorkflowError::Corrupt {
                    path: path.to_path_buf(),
                    reason: format!("node '{node}' has output without a frozen schema"),
                })?;
            schema
                .validate_value(&output.value)
                .map_err(|error| WorkflowError::Corrupt {
                    path: path.to_path_buf(),
                    reason: format!("node '{node}' has invalid durable output: {error}"),
                })?;
        }
        for artifact in [
            completion.artifacts.stdout.as_ref(),
            completion.artifacts.stderr.as_ref(),
            completion.artifacts.answer.as_ref(),
            completion.artifacts.structured_output.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if artifact.retained_bytes > definition.max_output_bytes {
                return corrupt(
                    path,
                    "completion artifact exceeds its frozen node output limit",
                );
            }
        }
        if completion
            .artifacts
            .stdout
            .iter()
            .chain(completion.artifacts.stderr.iter())
            .try_fold(0_u64, |total, artifact| {
                total.checked_add(artifact.retained_bytes)
            })
            .is_none_or(|total| total > definition.max_output_bytes.saturating_mul(2))
        {
            return corrupt(
                path,
                "command stream artifacts exceed twice the per-stream output limit",
            );
        }
        retained_workflow_output = [
            completion.artifacts.stdout.as_ref(),
            completion.artifacts.stderr.as_ref(),
            completion.artifacts.answer.as_ref(),
        ]
        .into_iter()
        .flatten()
        .try_fold(retained_workflow_output, |total, artifact| {
            total.checked_add(artifact.retained_bytes)
        })
        .ok_or_else(|| WorkflowError::Corrupt {
            path: path.to_path_buf(),
            reason: "retained workflow output size overflowed".to_owned(),
        })?;
        let artifact_validation = match file_validation {
            CompletionFileValidation::All => ArtifactFileValidation::Full,
            CompletionFileValidation::ChangedSince(previous)
                if previous.get(node) != Some(completion) =>
            {
                ArtifactFileValidation::Full
            }
            CompletionFileValidation::ChangedSince(_) => ArtifactFileValidation::Skip,
        };
        validate_completion_artifacts(completion, path, root, run_relative, artifact_validation)?;
    }
    if retained_workflow_output > graph.runtime_limits.retained_output_total_bytes {
        return corrupt(
            path,
            "retained output exceeds the frozen workflow-wide limit",
        );
    }
    let expected_exits = state
        .completions
        .iter()
        .filter_map(|(node, completion)| {
            completion
                .command_exit
                .clone()
                .map(|exit| (node.clone(), exit))
        })
        .collect::<BTreeMap<_, _>>();
    let expected_outputs = state
        .completions
        .iter()
        .filter_map(|(node, completion)| {
            completion
                .structured_output
                .as_ref()
                .map(|output| (node.clone(), output.value.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    if state.command_exits != expected_exits || state.outputs != expected_outputs {
        return corrupt(
            path,
            "output or command-exit keys differ from durable completions",
        );
    }
    match state.lifecycle {
        RunLifecycle::Completed => {
            if terminal.len() != graph.graph.nodes.len() {
                return corrupt(path, "completed run contains non-terminal nodes");
            }
            if state.outcome != derive_workflow_outcome(graph, state) {
                return corrupt(path, "completed run outcome is absent or not derived");
            }
        }
        _ if state.outcome.is_some() => {
            return corrupt(path, "non-completed run contains a workflow outcome")
        }
        _ => {}
    }
    Ok(())
}

fn validate_completion_artifacts(
    completion: &NodeCompletion,
    path: &Path,
    root: &SecureDirectory,
    run_relative: &Path,
    file_validation: ArtifactFileValidation,
) -> WorkflowResult<()> {
    if completion
        .structured_output
        .as_ref()
        .map(|output| &output.artifact)
        != completion.artifacts.structured_output.as_ref()
    {
        return corrupt(
            path,
            "structured output and completion artifact references differ",
        );
    }
    let artifacts = [
        completion.artifacts.stdout.as_ref(),
        completion.artifacts.stderr.as_ref(),
        completion.artifacts.answer.as_ref(),
        completion.artifacts.structured_output.as_ref(),
        completion.artifacts.command_outcome.as_ref(),
    ];
    for artifact in artifacts.into_iter().flatten() {
        let observation_valid = match artifact.observed {
            ArtifactObservation::Complete { observed_bytes } => {
                observed_bytes == artifact.retained_bytes
            }
            ArtifactObservation::Truncated {
                observed_bytes_at_least,
            } => observed_bytes_at_least > artifact.retained_bytes,
            ArtifactObservation::Incomplete { observed_bytes } => {
                observed_bytes >= artifact.retained_bytes
            }
        };
        if artifact.relative_path.is_empty() || !observation_valid {
            return corrupt(path, "durable artifact reference has invalid size metadata");
        }
        let relative = Path::new(&artifact.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return corrupt(path, "durable artifact reference leaves the run directory");
        }
        if matches!(file_validation, ArtifactFileValidation::Skip) {
            continue;
        }
        let Some(run_directory) = path.parent() else {
            continue;
        };
        let artifact_path = run_directory.join(relative);
        let mut file = root.open_file(&run_relative.join(relative))?;
        let metadata = file.metadata().map_err(WorkflowError::Io)?;
        if metadata.len() != artifact.retained_bytes {
            return corrupt(
                &artifact_path,
                "durable artifact size does not match its file",
            );
        }
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(WorkflowError::Io)?;
        let digest = format!("sha256:{:x}", hasher.finalize());
        if artifact.digest.0 != digest {
            return corrupt(
                &artifact_path,
                "durable artifact reference does not match its file",
            );
        }
    }
    Ok(())
}
