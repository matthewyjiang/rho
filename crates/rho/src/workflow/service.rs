use std::collections::BTreeMap;

use super::{
    normalize_workflow, validate_workflow, Digest, FrozenSchedulerSettings, FrozenWorkflow,
    InputName, PlanConsent, PlannerIdentity, ResolvedNode, RunLifecycle, RunStateRecord,
    SourceManifest, StoredPlan, StoredRun, WorkflowResult, WorkflowState, WorkflowStore,
    WorkflowValue, FROZEN_WORKFLOW_SCHEMA_VERSION, RUN_STATE_VERSION,
};

pub(crate) struct WorkflowService {
    store: WorkflowStore,
}

pub(crate) struct FreezePlan<'a> {
    pub(crate) planner: PlannerIdentity,
    pub(crate) sources: SourceManifest,
    pub(crate) source_bytes: &'a BTreeMap<String, String>,
    pub(crate) inputs: BTreeMap<InputName, WorkflowValue>,
    pub(crate) graph: super::WorkflowGraph,
    pub(crate) resolved_nodes: BTreeMap<super::NodeId, ResolvedNode>,
    pub(crate) scheduler: FrozenSchedulerSettings,
    pub(crate) workspace_identity: String,
}

impl WorkflowService {
    pub(crate) fn new(store: WorkflowStore) -> Self {
        Self { store }
    }

    pub(crate) fn freeze_and_store(&self, plan: FreezePlan<'_>) -> WorkflowResult<StoredPlan> {
        let workflow = FrozenWorkflow {
            schema_version: FROZEN_WORKFLOW_SCHEMA_VERSION,
            planner: plan.planner,
            graph_digest: Digest(String::new()),
            sources: plan.sources,
            inputs: plan.inputs,
            graph: plan.graph,
            resolved_nodes: plan.resolved_nodes,
            scheduler: plan.scheduler,
        };
        let workflow = normalize_workflow(workflow)?;
        validate_workflow(&workflow)?;
        self.store
            .create_plan(&workflow, plan.workspace_identity, plan.source_bytes)
    }

    pub(crate) fn create_run(
        &self,
        plan: &StoredPlan,
        consent: PlanConsent,
    ) -> WorkflowResult<StoredRun> {
        let state = WorkflowState {
            revision: 0,
            lifecycle: RunLifecycle::Planned,
            cancellation_requested: false,
            nodes: plan
                .graph
                .graph
                .nodes
                .keys()
                .cloned()
                .map(|id| (id, super::NodeState::Pending))
                .collect(),
            command_exits: BTreeMap::new(),
            outputs: BTreeMap::new(),
        };
        self.store.create_run(
            plan,
            consent,
            RunStateRecord {
                schema_version: RUN_STATE_VERSION,
                last_event_sequence: 0,
                state,
            },
        )
    }

    pub(crate) fn store(&self) -> &WorkflowStore {
        &self.store
    }
}
