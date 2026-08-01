use std::collections::BTreeMap;

use super::{FrozenWorkflow, NodeId, WorkflowError, WorkflowResult};

pub(crate) fn normalize_workflow(mut workflow: FrozenWorkflow) -> WorkflowResult<FrozenWorkflow> {
    let mut normalized = BTreeMap::new();
    for (key, mut node) in workflow.graph.nodes {
        if key != node.id {
            return Err(WorkflowError::Schema {
                path: format!("graph.nodes.{key}"),
                reason: format!("map key does not match node ID '{}'", node.id),
            });
        }
        node.needs.sort();
        node.needs.dedup();
        normalized.insert(key, node);
    }
    workflow.graph.nodes = normalized;
    workflow.resolved_nodes = workflow
        .resolved_nodes
        .into_iter()
        .collect::<BTreeMap<NodeId, _>>();
    workflow.graph_digest = super::graph_digest(&workflow)?;
    Ok(workflow)
}
