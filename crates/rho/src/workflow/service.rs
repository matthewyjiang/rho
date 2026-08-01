use super::{
    PlanConsent, RunLifecycle, RunStateRecord, StoredPlan, StoredRun, WorkflowResult,
    WorkflowState, WorkflowStore, RUN_STATE_VERSION,
};
use std::collections::BTreeMap;

pub(crate) struct WorkflowService {
    store: WorkflowStore,
}

impl WorkflowService {
    pub(crate) fn new(store: WorkflowStore) -> Self {
        Self { store }
    }

    /// Persist an already-normalized and validated frozen workflow.
    ///
    /// Callers must run the freeze pipeline exactly once before this method.
    /// The store still validates durable integrity on write.
    pub(crate) fn store_frozen(
        &self,
        workflow: &super::FrozenWorkflow,
        workspace_identity: String,
        source_bytes: &BTreeMap<String, String>,
    ) -> WorkflowResult<StoredPlan> {
        self.store
            .create_plan(workflow, workspace_identity, source_bytes)
    }

    pub(crate) fn create_run(
        &self,
        plan: &StoredPlan,
        consent: PlanConsent,
    ) -> WorkflowResult<StoredRun> {
        let state = WorkflowState {
            revision: 0,
            lifecycle: RunLifecycle::Planned,
            outcome: None,
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
            completions: BTreeMap::new(),
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
