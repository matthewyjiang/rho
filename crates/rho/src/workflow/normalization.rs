use std::collections::BTreeMap;

use super::{FrozenWorkflow, NodeId, WorkflowError, WorkflowResult};

pub(crate) fn normalize_workflow(mut workflow: FrozenWorkflow) -> WorkflowResult<FrozenWorkflow> {
    let mut normalized = BTreeMap::new();
    for (key, mut node) in workflow.program.root.nodes {
        if key != node.id {
            return Err(WorkflowError::Schema {
                path: format!("program.root.nodes.{key}"),
                reason: format!("map key does not match node ID '{}'", node.id),
            });
        }
        node.needs.sort();
        node.needs.dedup();
        normalized.insert(key, node);
    }
    workflow.program.root.nodes = normalized;
    workflow.resolved_nodes = workflow
        .resolved_nodes
        .into_iter()
        .collect::<BTreeMap<NodeId, _>>();
    workflow.program_digest = super::program_digest(&workflow)?;
    Ok(workflow)
}
