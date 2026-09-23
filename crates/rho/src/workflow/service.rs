use super::{
    PlanConsent, RunStateRecord, StoredPlan, StoredRun, WorkflowResult, WorkflowState,
    WorkflowStore, RUN_STATE_VERSION,
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
        let state = WorkflowState::new(&plan.graph);
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
