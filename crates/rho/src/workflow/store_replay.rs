use std::path::Path;

use super::{
    apply_durable_event, FrozenWorkflow, WorkflowEventRecord, WorkflowResult, WorkflowState,
};

pub(super) fn derive_snapshot(
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
