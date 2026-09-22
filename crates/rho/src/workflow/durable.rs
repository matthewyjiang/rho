use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use super::{
    validate_lifecycle_transition, validate_reset_transition, validate_transition, AttemptNumber,
    CancellationResumeState, FrozenWorkflow, LifecycleTransition, NodeCompletion, NodeExecution,
    NodeId, NodeState, NodeTerminalState, RunLifecycle, TaskInstanceId, ValidatedOutputRef,
    WorkflowError, WorkflowEvent, WorkflowEventRecord, WorkflowResult, WorkflowState,
};

/// Journal-owned state that must survive a snapshot between paired events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DurableReplayState {
    last_attempts: BTreeMap<NodeId, AttemptNumber>,
    launch_intents: BTreeMap<NodeId, AttemptNumber>,
    structured_outputs: BTreeMap<NodeId, (AttemptNumber, ValidatedOutputRef)>,
}

impl DurableReplayState {
    pub(crate) fn validate_membership(
        &self,
        nodes: &BTreeMap<NodeId, NodeState>,
    ) -> WorkflowResult<()> {
        if self
            .last_attempts
            .keys()
            .chain(self.launch_intents.keys())
            .chain(self.structured_outputs.keys())
            .any(|node| !nodes.contains_key(node))
        {
            return Err(WorkflowError::Scheduler(
                "durable scope data references an unknown local task".to_owned(),
            ));
        }
        Ok(())
    }
    pub(crate) fn structured_output(
        &self,
        node: &NodeId,
        attempt: AttemptNumber,
    ) -> Option<&ValidatedOutputRef> {
        self.structured_outputs
            .get(node)
            .filter(|(recorded_attempt, _)| *recorded_attempt == attempt)
            .map(|(_, output)| output)
    }

    /// Allocation is committed by LaunchIntended, never inferred from artifact directories.
    pub(crate) fn next_attempt(&self, node: &NodeId) -> WorkflowResult<AttemptNumber> {
        let previous = self
            .last_attempts
            .get(node)
            .map_or(0, |number| number.get());
        AttemptNumber::new(previous.checked_add(1).ok_or_else(|| {
            WorkflowError::Scheduler(format!(
                "node '{node}' attempt number budget is {} but requested {}",
                u32::MAX,
                u64::from(previous) + 1
            ))
        })?)
    }
}

/// The sole durable-event reducer, used for both complete and incremental replay.
/// Errors leave the supplied state unchanged and use the journal's corruption context.
pub(crate) fn apply_durable_event(
    graph: &FrozenWorkflow,
    state: &WorkflowState,
    event: &WorkflowEvent,
    path: &Path,
) -> WorkflowResult<WorkflowState> {
    let reduce = || -> WorkflowResult<WorkflowState> {
        super::validate_state_shape(graph, state)?;
        if matches!(
            event,
            WorkflowEvent::NodeReady { .. }
                | WorkflowEvent::LaunchIntended { .. }
                | WorkflowEvent::AttemptStarted { .. }
        ) && (state.lifecycle != RunLifecycle::Running || state.cancellation_requested)
        {
            return corrupt(
                path,
                "work admission requires a running, uncancelled workflow",
            );
        }
        let mut next = state.clone();
        let task = match event {
            WorkflowEvent::NodeReady { node }
            | WorkflowEvent::LaunchIntended { node, .. }
            | WorkflowEvent::AttemptStarted { node, .. }
            | WorkflowEvent::StructuredOutput { node, .. }
            | WorkflowEvent::NodeFinished { node, .. }
            | WorkflowEvent::NodeReset { node, .. } => Some(node),
            WorkflowEvent::HookObserved { node, .. } => node.as_ref(),
            WorkflowEvent::ScopeFinished { .. }
            | WorkflowEvent::ScopeReopened { .. }
            | WorkflowEvent::RunLifecycle { .. }
            | WorkflowEvent::CancellationRequested { .. }
            | WorkflowEvent::CancellationCleared
            | WorkflowEvent::CancellationAcknowledged { .. } => None,
        };
        if let Some(task) = task {
            let (local, _) = state.local(task)?;
            if !matches!(event, WorkflowEvent::HookObserved { .. }) && local.result.is_some() {
                return corrupt(path, "task event targets a closed scope");
            }
        }
        match event {
            WorkflowEvent::ScopeFinished { scope, result } => {
                if !state.lifecycle.is_live()
                    || state
                        .scope(*scope)
                        .is_none_or(|local| local.result.is_some())
                {
                    return corrupt(path, "scope finish requires an open scope in a live run");
                }
                if super::scope_result(graph, state, *scope)?.as_ref() != Some(result) {
                    return corrupt(
                        path,
                        "scope result differs from terminal children and declared exports",
                    );
                }
                next.scope_mut(*scope)
                    .ok_or_else(|| WorkflowError::Scheduler(format!("unknown scope '{scope}'")))?
                    .result = Some(result.clone());
            }
            WorkflowEvent::ScopeReopened { scope } => {
                if !matches!(
                    state.lifecycle,
                    RunLifecycle::Running | RunLifecycle::Cancelling | RunLifecycle::Completed
                ) {
                    return corrupt(path, "scope reopen requires a live or completed run");
                }
                if state
                    .scope(*scope)
                    .and_then(|local| local.result.as_ref())
                    .map(|result| result.outcome)
                    != Some(super::WorkflowOutcome::Cancellation)
                {
                    return corrupt(path, "only a cancelled closed scope can reopen");
                }
                validate_lifecycle_transition(state, LifecycleTransition::ReopenCancelledScope)?;
                next.scope_mut(*scope)
                    .ok_or_else(|| WorkflowError::Scheduler(format!("unknown scope '{scope}'")))?
                    .result = None;
                next.lifecycle = RunLifecycle::Cancelling;
            }
            WorkflowEvent::NodeReady { node } => replace_node(&mut next, node, NodeState::Ready)?,
            WorkflowEvent::LaunchIntended { node, attempt } => {
                if state.task(node) != Some(&NodeState::Ready)
                    || *attempt != state.next_attempt(node)?
                {
                    return corrupt(
                        path,
                        "launch intention does not allocate the next ready-node attempt",
                    );
                }
                let (local, id) = next.local_mut(node)?;
                local.durable.last_attempts.insert(id.clone(), *attempt);
                // A crash before AttemptStarted leaves a replaceable reservation, never a reusable attempt.
                local.durable.launch_intents.insert(id.clone(), *attempt);
            }
            WorkflowEvent::AttemptStarted { node, attempt, .. } => {
                let (local, id) = state.local(node)?;
                if local.durable.launch_intents.get(id) != Some(attempt) {
                    return corrupt(path, "attempt start does not match its launch intention");
                }
                replace_node(&mut next, node, NodeState::Running { attempt: *attempt })?;
                let (local, id) = next.local_mut(node)?;
                local.durable.launch_intents.remove(id);
            }
            WorkflowEvent::StructuredOutput {
                node,
                attempt,
                output,
            } => {
                let (local, id) = next.local_mut(node)?;
                if local.nodes.get(id) != Some(&NodeState::Running { attempt: *attempt })
                    || local.durable.structured_outputs.contains_key(id)
                {
                    return corrupt(path, "structured output event has no unique active attempt");
                }
                local
                    .durable
                    .structured_outputs
                    .insert(id.clone(), (*attempt, output.clone()));
            }
            WorkflowEvent::NodeFinished { node, completion } => {
                validate_completion(state, node, completion, path)?;
                if let Some(attempt) = completion.attempt {
                    let (local, id) = state.local(node)?;
                    let recorded = local.durable.structured_outputs.get(id);
                    let expected = completion
                        .structured_output
                        .as_ref()
                        .map(|output| (attempt, output));
                    if recorded.map(|(attempt, output)| (*attempt, output)) != expected {
                        return corrupt(
                            path,
                            "structured output event differs from node completion",
                        );
                    }
                }
                finish_node(graph, &mut next, node, completion)?;
            }
            WorkflowEvent::CancellationRequested { .. } => {
                validate_lifecycle_transition(
                    state,
                    LifecycleTransition::Advance(RunLifecycle::Cancelling),
                )?;
                next.cancellation_requested = true;
                next.lifecycle = RunLifecycle::Cancelling;
            }
            WorkflowEvent::NodeReset { node, reason } => reset_node(&mut next, node, *reason)?,
            WorkflowEvent::CancellationCleared => {
                if !state.cancellation_requested {
                    return corrupt(path, "cancellation clear event has no cancellation");
                }
                next.cancellation_requested = false;
            }
            WorkflowEvent::RunLifecycle { lifecycle } => {
                validate_lifecycle_transition(state, LifecycleTransition::Advance(*lifecycle))?;
                next.lifecycle = *lifecycle;
            }
            WorkflowEvent::CancellationAcknowledged { .. } | WorkflowEvent::HookObserved { .. } => {
                return Ok(next)
            }
        }
        next.revision = next.revision.checked_add(1).ok_or_else(|| {
            WorkflowError::Scheduler("workflow state revision overflowed".to_owned())
        })?;
        Ok(next)
    };
    reduce().map_err(|error| match error {
        WorkflowError::Corrupt { .. } => error,
        error => WorkflowError::Corrupt {
            path: path.to_path_buf(),
            reason: error.to_string(),
        },
    })
}

fn replace_node(
    state: &mut WorkflowState,
    node: &TaskInstanceId,
    target: NodeState,
) -> WorkflowResult<()> {
    let (local, id) = state.local_mut(node)?;
    validate_transition(node, &local.nodes[id], &target)?;
    local.nodes.insert(id.clone(), target);
    Ok(())
}

fn finish_node(
    graph: &FrozenWorkflow,
    next: &mut WorkflowState,
    node: &TaskInstanceId,
    completion: &NodeCompletion,
) -> WorkflowResult<()> {
    let (local, id) = next.local_mut(node)?;
    let definition = &graph.program.scope(local.definition).nodes[id];
    if let Some(output) = &completion.structured_output {
        let value = &output.value;
        let output_bytes = serde_json::to_vec(value)?.len() as u64;
        if output_bytes > definition.max_output_bytes {
            return Err(WorkflowError::Scheduler(format!(
                "node '{node}' output budget is {} bytes but observed {output_bytes} bytes",
                definition.max_output_bytes
            )));
        }
        definition
            .output_schema()
            .ok_or_else(|| {
                WorkflowError::Scheduler(format!(
                    "node '{node}' produced structured output without a schema"
                ))
            })?
            .validate_value(value)?;
        local.outputs.insert(id.clone(), value.clone());
    }
    if let Some(exit) = &completion.command_exit {
        if !matches!(definition.execution, NodeExecution::Command(_)) {
            return Err(WorkflowError::Scheduler(format!(
                "agent node '{node}' reported a command exit"
            )));
        }
        local.command_exits.insert(id.clone(), exit.clone());
    }
    let target = NodeState::Terminal {
        outcome: completion.outcome,
    };
    validate_transition(node, &local.nodes[id], &target)?;
    local.nodes.insert(id.clone(), target);
    local.completions.insert(id.clone(), completion.clone());
    local.durable.structured_outputs.remove(id);
    local.durable.launch_intents.remove(id);
    Ok(())
}

fn reset_node(
    next: &mut WorkflowState,
    node: &TaskInstanceId,
    reason: super::NodeResetReason,
) -> WorkflowResult<()> {
    let (local, id) = next.local_mut(node)?;
    let target = match reason {
        super::NodeResetReason::InterruptedRecovery => NodeState::Ready,
        super::NodeResetReason::CleanCancellation => match local
            .completions
            .get(id)
            .and_then(|completion| completion.cancellation_resume)
        {
            Some(CancellationResumeState::Pending) => NodeState::Pending,
            Some(CancellationResumeState::Ready) => NodeState::Ready,
            None => {
                return Err(WorkflowError::Scheduler(format!(
                    "cancelled node '{node}' has no resume state"
                )))
            }
        },
    };
    validate_reset_transition(node, &local.nodes[id], reason, &target)?;
    local.nodes.insert(id.clone(), target);
    local.command_exits.remove(id);
    local.outputs.remove(id);
    local.completions.remove(id);
    local.durable.structured_outputs.remove(id);
    local.durable.launch_intents.remove(id);
    Ok(())
}

fn validate_completion(
    state: &WorkflowState,
    node: &TaskInstanceId,
    completion: &NodeCompletion,
    path: &Path,
) -> WorkflowResult<()> {
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
    match completion.attempt {
        Some(attempt) if state.task(node) == Some(&NodeState::Running { attempt }) => {
            if completion
                .cancellation_resume
                .is_some_and(|resume| resume != CancellationResumeState::Ready)
            {
                return corrupt(path, "attempt cancellation must resume ready");
            }
            Ok(())
        }
        Some(_) => corrupt(path, "node completion does not match its active attempt"),
        None if completion.outcome == NodeTerminalState::Cancellation => {
            let resume = match state.task(node) {
                Some(NodeState::Pending) => CancellationResumeState::Pending,
                Some(NodeState::Ready) => CancellationResumeState::Ready,
                _ => return corrupt(path, "synthetic cancellation targets a non-waiting node"),
            };
            if completion.cancellation_resume != Some(resume) {
                return corrupt(
                    path,
                    "synthetic cancellation resume state differs from waiting node",
                );
            }
            Ok(())
        }
        None if matches!(state.task(node), Some(NodeState::Running { .. })) => {
            corrupt(path, "running node completion has no attempt")
        }
        None => Ok(()),
    }
}

pub(crate) fn derive_snapshot(
    graph: &FrozenWorkflow,
    events: &[WorkflowEventRecord],
    through: u64,
    path: &Path,
) -> WorkflowResult<WorkflowState> {
    let mut state = WorkflowState::new(graph);
    for record in events
        .iter()
        .take_while(|record| record.sequence <= through)
    {
        state = apply_durable_event(graph, &state, &record.event, path)?;
    }
    Ok(state)
}

fn corrupt<T>(path: &Path, reason: &str) -> WorkflowResult<T> {
    Err(WorkflowError::Corrupt {
        path: path.to_path_buf(),
        reason: reason.to_owned(),
    })
}

#[cfg(test)]
#[path = "durable_tests.rs"]
mod tests;
