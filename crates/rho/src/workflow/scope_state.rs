use super::{
    AttemptNumber, CommandExit, DurableReplayState, FrozenWorkflow, NodeCompletion, NodeId,
    NodeState, RunLifecycle, ScopeInstanceId, TaskInstanceId, ValidatedOutputRef, WorkflowError,
    WorkflowOutcome, WorkflowResult, WorkflowValue,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowState {
    pub(crate) revision: u64,
    pub(crate) lifecycle: RunLifecycle,
    pub(crate) cancellation_requested: bool,
    pub(crate) scopes: BTreeMap<ScopeInstanceId, ScopeState>,
}

/// Persisted root execution data. The scope map and definition ID remain part
/// of the on-disk format; execution currently supports only the root instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopeState {
    pub(crate) definition: super::ScopeDefinitionId,
    pub(crate) durable: DurableReplayState,
    pub(crate) nodes: BTreeMap<NodeId, NodeState>,
    pub(crate) command_exits: BTreeMap<NodeId, CommandExit>,
    pub(crate) outputs: BTreeMap<NodeId, WorkflowValue>,
    pub(crate) completions: BTreeMap<NodeId, NodeCompletion>,
    pub(crate) result: Option<ScopeResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScopeResult {
    pub(crate) outcome: WorkflowOutcome,
    pub(crate) outputs: BTreeMap<String, WorkflowValue>,
}

impl WorkflowState {
    pub(crate) fn new(workflow: &FrozenWorkflow) -> Self {
        Self {
            revision: 0,
            lifecycle: RunLifecycle::Planned,
            cancellation_requested: false,
            scopes: BTreeMap::from([(
                ScopeInstanceId::ROOT,
                ScopeState {
                    definition: super::ScopeDefinitionId::Root,
                    durable: Default::default(),
                    nodes: workflow
                        .program
                        .root
                        .nodes
                        .keys()
                        .cloned()
                        .map(|id| (id, NodeState::Pending))
                        .collect(),
                    outputs: BTreeMap::new(),
                    command_exits: BTreeMap::new(),
                    completions: BTreeMap::new(),
                    result: None,
                },
            )]),
        }
    }
    /// Call only on constructed or validated state; untrusted state uses scope().
    pub(crate) fn root_scope(&self) -> &ScopeState {
        &self.scopes[&ScopeInstanceId::ROOT]
    }
    #[cfg(test)]
    pub(crate) fn root_scope_mut(&mut self) -> &mut ScopeState {
        self.scopes
            .get_mut(&ScopeInstanceId::ROOT)
            .expect("validated root scope")
    }
    pub(crate) fn scope(&self, id: ScopeInstanceId) -> Option<&ScopeState> {
        self.scopes.get(&id)
    }
    pub(crate) fn scope_mut(&mut self, id: ScopeInstanceId) -> Option<&mut ScopeState> {
        self.scopes.get_mut(&id)
    }
    pub(crate) fn task(&self, id: &TaskInstanceId) -> Option<&NodeState> {
        let (scope, node) = self.local(id).ok()?;
        scope.nodes.get(node)
    }
    /// Resolve an existing task and its local scope without assuming root identity.
    pub(crate) fn local<'a>(
        &self,
        id: &'a TaskInstanceId,
    ) -> WorkflowResult<(&ScopeState, &'a NodeId)> {
        self.scope(id.scope())
            .filter(|scope| scope.nodes.contains_key(id.definition()))
            .map(|scope| (scope, id.definition()))
            .ok_or_else(|| WorkflowError::Scheduler(format!("unknown task '{id}'")))
    }
    pub(crate) fn local_mut<'a>(
        &mut self,
        id: &'a TaskInstanceId,
    ) -> WorkflowResult<(&mut ScopeState, &'a NodeId)> {
        self.scope_mut(id.scope())
            .filter(|scope| scope.nodes.contains_key(id.definition()))
            .map(|scope| (scope, id.definition()))
            .ok_or_else(|| WorkflowError::Scheduler(format!("unknown task '{id}'")))
    }
    pub(crate) fn run_result(&self) -> Option<&ScopeResult> {
        self.scope(ScopeInstanceId::ROOT)?.result.as_ref()
    }
    /// Enumerate executable tasks after validating the root-only state shape.
    pub(crate) fn tasks(&self) -> impl Iterator<Item = (TaskInstanceId, &NodeState)> {
        self.root_scope()
            .nodes
            .iter()
            .map(|(id, state)| (TaskInstanceId::root(id.clone()), state))
    }
    pub(crate) fn outcome(&self) -> Option<WorkflowOutcome> {
        if self.lifecycle != RunLifecycle::Completed {
            return None;
        }
        self.run_result().map(|result| result.outcome)
    }
    pub(crate) fn completion(&self, id: &TaskInstanceId) -> Option<&NodeCompletion> {
        let (scope, node) = self.local(id).ok()?;
        scope.completions.get(node)
    }
    pub(crate) fn next_attempt(&self, id: &TaskInstanceId) -> WorkflowResult<AttemptNumber> {
        let (scope, node) = self.local(id)?;
        scope.durable.next_attempt(node)
    }
    pub(crate) fn structured_output(
        &self,
        id: &TaskInstanceId,
        attempt: AttemptNumber,
    ) -> Option<&ValidatedOutputRef> {
        let (scope, node) = self.local(id).ok()?;
        scope.durable.structured_output(node, attempt)
    }
}

/// Current execution admits exactly one root invocation, not dynamic scopes.
pub(crate) fn validate_state_shape(
    workflow: &FrozenWorkflow,
    state: &WorkflowState,
) -> WorkflowResult<()> {
    if state.scopes.len() != 1 || !state.scopes.contains_key(&ScopeInstanceId::ROOT) {
        return Err(WorkflowError::Scheduler(
            "program requires exactly the root scope instance".to_owned(),
        ));
    }
    let local = state.root_scope();
    let definition = workflow.program.scope(local.definition);
    local.durable.validate_membership(&local.nodes)?;
    if definition.nodes.keys().ne(local.nodes.keys()) {
        return Err(WorkflowError::Scheduler(
            "node state keys differ from root scope node keys".to_owned(),
        ));
    }
    if local
        .outputs
        .keys()
        .chain(local.command_exits.keys())
        .chain(local.completions.keys())
        .any(|id| !local.nodes.contains_key(id))
    {
        return Err(WorkflowError::Scheduler(
            "scope data references an unknown local task".to_owned(),
        ));
    }
    Ok(())
}
