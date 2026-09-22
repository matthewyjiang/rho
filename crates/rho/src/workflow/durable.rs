use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use super::{
    apply_event, validate_lifecycle_transition, AttemptNumber, CancellationResumeState,
    FrozenWorkflow, NodeCompletion, NodeId, NodeState, NodeTerminalState, RunLifecycle,
    SchedulerEvent, TaskInstanceId, ValidatedOutputRef, WorkflowError, WorkflowEvent,
    WorkflowResult, WorkflowState,
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
/// Errors leave the supplied state unchanged.
pub(crate) fn apply_durable_event(
    graph: &FrozenWorkflow,
    state: &WorkflowState,
    event: &WorkflowEvent,
    path: &Path,
) -> WorkflowResult<WorkflowState> {
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
    // Every task-bearing record resolves its scope before local journal access.
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
        if state.task(task).is_none() {
            return corrupt(path, "event targets an unknown task instance");
        }
        if !matches!(event, WorkflowEvent::HookObserved { .. })
            && state
                .scope(task.scope())
                .is_some_and(|scope| scope.result.is_some())
        {
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
            next.scope_mut(*scope).expect("scope checked").result = Some(result.clone());
            bump_revision(&mut next, path)?;
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
            next.scope_mut(*scope).expect("scope checked").result = None;
            // This explicit recovery transition atomically reopens the scope and
            // leaves Completed; no snapshot may be completed with an open root.
            next.lifecycle = RunLifecycle::Cancelling;
            bump_revision(&mut next, path)?;
        }
        WorkflowEvent::NodeReady { node } => {
            next = apply_event(
                graph,
                state,
                SchedulerEvent::MarkReady { node: node.clone() },
            )?;
        }
        WorkflowEvent::LaunchIntended { node, attempt } => {
            if state.task(node) != Some(&NodeState::Ready)
                || *attempt != state.next_attempt(node)?
            {
                return corrupt(
                    path,
                    "launch intention does not allocate the next ready-node attempt",
                );
            }
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .last_attempts
                .insert(node.definition().clone(), *attempt);
            // Execution starts only after AttemptStarted is durable. A ready-node
            // reservation left by a crash can therefore be replaced, never reused.
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .launch_intents
                .insert(node.definition().clone(), *attempt);
            bump_revision(&mut next, path)?;
        }
        WorkflowEvent::AttemptStarted { node, attempt, .. } => {
            if state
                .scope(node.scope())
                .expect("task scope checked")
                .durable
                .launch_intents
                .get(node.definition())
                != Some(attempt)
            {
                return corrupt(path, "attempt start does not match its launch intention");
            }
            next = apply_event(
                graph,
                state,
                SchedulerEvent::Launched {
                    node: node.clone(),
                    attempt: *attempt,
                },
            )?;
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .launch_intents
                .remove(node.definition());
        }
        WorkflowEvent::StructuredOutput {
            node,
            attempt,
            output,
        } => {
            if state.task(node) != Some(&NodeState::Running { attempt: *attempt })
                || state
                    .scope(node.scope())
                    .expect("task scope checked")
                    .durable
                    .structured_outputs
                    .contains_key(node.definition())
            {
                return corrupt(path, "structured output event has no unique active attempt");
            }
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .structured_outputs
                .insert(node.definition().clone(), (*attempt, output.clone()));
            bump_revision(&mut next, path)?;
        }
        WorkflowEvent::NodeFinished { node, completion } => {
            let completion = event_completion(state, node, completion, path)?;
            if let Some(attempt) = completion.attempt {
                let recorded = state
                    .scope(node.scope())
                    .expect("task scope checked")
                    .durable
                    .structured_outputs
                    .get(node.definition());
                let expected = completion
                    .structured_output
                    .as_ref()
                    .map(|output| (attempt, output));
                if recorded.map(|(attempt, output)| (*attempt, output)) != expected {
                    return corrupt(path, "structured output event differs from node completion");
                }
            }
            next = apply_event(
                graph,
                state,
                SchedulerEvent::Finished {
                    node: node.clone(),
                    completion: Box::new(completion),
                },
            )?;
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .structured_outputs
                .remove(node.definition());
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .launch_intents
                .remove(node.definition());
        }
        WorkflowEvent::CancellationRequested { .. } => {
            next = apply_event(graph, state, SchedulerEvent::CancellationRequested)?;
        }
        WorkflowEvent::NodeReset { node, reason } => {
            next = apply_event(
                graph,
                state,
                SchedulerEvent::ResetNode {
                    node: node.clone(),
                    reason: *reason,
                },
            )?;
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .structured_outputs
                .remove(node.definition());
            next.scope_mut(node.scope())
                .expect("task scope checked")
                .durable
                .launch_intents
                .remove(node.definition());
        }
        WorkflowEvent::CancellationCleared => {
            if !state.cancellation_requested {
                return corrupt(path, "cancellation clear event has no cancellation");
            }
            next.cancellation_requested = false;
            bump_revision(&mut next, path)?;
        }
        WorkflowEvent::RunLifecycle { lifecycle } => {
            validate_lifecycle_transition(state, *lifecycle)?;
            next.lifecycle = *lifecycle;
            bump_revision(&mut next, path)?;
        }
        WorkflowEvent::CancellationAcknowledged { .. } | WorkflowEvent::HookObserved { .. } => {}
    }
    Ok(next)
}

fn event_completion(
    state: &WorkflowState,
    node: &TaskInstanceId,
    completion: &NodeCompletion,
    path: &Path,
) -> WorkflowResult<NodeCompletion> {
    match completion.attempt {
        Some(attempt) if state.task(node) == Some(&NodeState::Running { attempt }) => {
            Ok(completion.clone())
        }
        Some(_) => corrupt(path, "node completion does not match its active attempt"),
        None if completion.outcome == NodeTerminalState::Cancellation => {
            let resume = match state.task(node) {
                Some(NodeState::Pending) => CancellationResumeState::Pending,
                Some(NodeState::Ready) => CancellationResumeState::Ready,
                _ => return corrupt(path, "synthetic cancellation targets a non-waiting node"),
            };
            if completion.command_exit.is_some()
                || completion.structured_output.is_some()
                || completion.artifacts.iter().next().is_some()
            {
                return corrupt(path, "synthetic cancellation contains attempt-owned data");
            }
            Ok(NodeCompletion::cancelled(resume))
        }
        None if matches!(state.task(node), Some(NodeState::Running { .. })) => {
            corrupt(path, "running node completion has no attempt")
        }
        None => Ok(completion.clone()),
    }
}

fn bump_revision(state: &mut WorkflowState, path: &Path) -> WorkflowResult<()> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or_else(|| WorkflowError::Corrupt {
            path: path.to_path_buf(),
            reason: "workflow state revision overflowed".to_owned(),
        })?;
    Ok(())
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
