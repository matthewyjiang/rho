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

/// Local execution data for one invocation of a scope definition.
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

    pub(crate) fn scope_definition<'a>(
        &self,
        workflow: &'a FrozenWorkflow,
        id: ScopeInstanceId,
    ) -> WorkflowResult<&'a super::ScopeDefinition> {
        let scope = self
            .scope(id)
            .ok_or_else(|| WorkflowError::Scheduler(format!("unknown scope '{id}'")))?;
        Ok(workflow.program.scope_definition(scope.definition))
    }
    pub(crate) fn scope_mut(&mut self, id: ScopeInstanceId) -> Option<&mut ScopeState> {
        self.scopes.get_mut(&id)
    }
    pub(crate) fn task(&self, id: &TaskInstanceId) -> Option<&NodeState> {
        self.scope(id.scope())?.nodes.get(id.definition())
    }
    pub(crate) fn tasks(&self) -> impl Iterator<Item = (TaskInstanceId, &NodeState)> {
        self.scopes.iter().flat_map(|(scope, state)| {
            state
                .nodes
                .iter()
                .map(move |(id, state)| (TaskInstanceId::new(*scope, id.clone()), state))
        })
    }
    pub(crate) fn outcome(&self) -> Option<WorkflowOutcome> {
        if self.lifecycle != RunLifecycle::Completed {
            return None;
        }
        self.scope(ScopeInstanceId::ROOT)?
            .result
            .as_ref()
            .map(|result| result.outcome)
    }
    pub(crate) fn completion(&self, id: &TaskInstanceId) -> Option<&NodeCompletion> {
        self.scope(id.scope())?.completions.get(id.definition())
    }
    pub(crate) fn next_attempt(&self, id: &TaskInstanceId) -> WorkflowResult<AttemptNumber> {
        self.task(id)
            .ok_or_else(|| WorkflowError::Scheduler(format!("unknown task '{id}'")))?;
        self.scopes[&id.scope()]
            .durable
            .next_attempt(id.definition())
    }
    pub(crate) fn structured_output(
        &self,
        id: &TaskInstanceId,
        attempt: AttemptNumber,
    ) -> Option<&ValidatedOutputRef> {
        self.scope(id.scope())?
            .durable
            .structured_output(id.definition(), attempt)
    }
}
