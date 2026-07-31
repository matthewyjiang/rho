use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use super::{
    AgentRuntime, Condition, FrozenWorkflow, Node, NodeExecution, NodeId, OutputReference,
    TemplatePart, WorkflowError, WorkflowResult, WorkspaceAccess,
};

pub(crate) fn validate_workflow(workflow: &FrozenWorkflow) -> WorkflowResult<()> {
    let graph = &workflow.graph;
    if graph.nodes.keys().ne(workflow.resolved_nodes.keys()) {
        return Err(WorkflowError::Scheduler(
            "resolved node keys differ from graph node keys".to_owned(),
        ));
    }
    for (key, node) in &graph.nodes {
        if key != &node.id {
            return Err(WorkflowError::Scheduler(format!(
                "node map key '{key}' does not match node ID '{}'",
                node.id
            )));
        }
        validate_node_shape(node, workflow)?;
        for dependency in &node.needs {
            if !graph.nodes.contains_key(dependency) {
                return Err(WorkflowError::MissingDependency {
                    node: node.id.clone(),
                    dependency: dependency.clone(),
                });
            }
        }
    }
    detect_cycles(&graph.nodes)?;
    validate_references(workflow)
}

fn validate_node_shape(node: &Node, workflow: &FrozenWorkflow) -> WorkflowResult<()> {
    if node.display_name.is_empty() {
        return Err(WorkflowError::Scheduler(format!(
            "node '{}' has an empty display name",
            node.id
        )));
    }
    if matches!(node.execution, NodeExecution::Command(_))
        && node.access != WorkspaceAccess::Mutating
    {
        return Err(WorkflowError::InvalidAccess {
            node: node.id.clone(),
            reason: "direct and shell commands must be mutating".to_owned(),
        });
    }
    if let NodeExecution::Command(command) = &node.execution {
        let cwd = match command {
            super::CommandNode::Direct { cwd, .. } | super::CommandNode::Shell { cwd, .. } => cwd,
        };
        let path = Path::new(cwd);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(WorkflowError::InvalidAccess {
                node: node.id.clone(),
                reason: format!("command cwd '{cwd}' must stay under the workspace"),
            });
        }
    }
    if let Some(super::ResolvedNode::Agent(agent)) = workflow.resolved_nodes.get(&node.id) {
        if agent.runtime == AgentRuntime::ClaudeCli && node.access != WorkspaceAccess::Mutating {
            return Err(WorkflowError::InvalidAccess {
                node: node.id.clone(),
                reason: "claude-cli nodes must be mutating in workflow schema v1".to_owned(),
            });
        }
        if node.access == WorkspaceAccess::ReadOnly {
            for capability in &agent.capabilities {
                match crate::tools::canonical_tool_is_mutating(capability) {
                    Some(false) => {}
                    Some(true) => {
                        return Err(WorkflowError::InvalidAccess {
                            node: node.id.clone(),
                            reason: "read-only Rho agent has a mutating or nested capability"
                                .to_owned(),
                        });
                    }
                    None => {
                        return Err(WorkflowError::InvalidAccess {
                            node: node.id.clone(),
                            reason: format!(
                                "read-only Rho agent has unknown capability '{capability}'"
                            ),
                        });
                    }
                }
            }
        }
    }
    if let Some(schema) = node.execution.output_schema() {
        schema.validate_definition()?;
    }
    Ok(())
}

fn detect_cycles(nodes: &BTreeMap<NodeId, Node>) -> WorkflowResult<()> {
    enum Visit {
        Enter(NodeId),
        Leave(NodeId),
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in nodes.keys() {
        let mut pending = vec![Visit::Enter(id.clone())];
        while let Some(visit) = pending.pop() {
            match visit {
                Visit::Enter(id) => {
                    if visited.contains(&id) {
                        continue;
                    }
                    if !visiting.insert(id.clone()) {
                        return Err(WorkflowError::Cycle { node: id });
                    }
                    pending.push(Visit::Leave(id.clone()));
                    pending.extend(nodes[&id].needs.iter().rev().cloned().map(Visit::Enter));
                }
                Visit::Leave(id) => {
                    visiting.remove(&id);
                    visited.insert(id);
                }
            }
        }
    }
    Ok(())
}

fn validate_references(workflow: &FrozenWorkflow) -> WorkflowResult<()> {
    let nodes = &workflow.graph.nodes;
    for node in nodes.values() {
        let ancestors = ancestors(node, nodes);
        let mut referenced = BTreeSet::new();
        if let Some(condition) = &node.condition {
            condition.referenced_nodes(&mut referenced);
        }
        for reference in template_references(node) {
            referenced.insert(reference.node.clone());
        }
        for target in referenced {
            if !ancestors.contains(&target) {
                return Err(WorkflowError::NonAncestorReference {
                    node: node.id.clone(),
                    referenced: target,
                });
            }
        }
        if let Some(condition) = &node.condition {
            validate_condition_schemas(condition, nodes)?;
        }
        for reference in template_references(node) {
            validate_output_reference(reference, nodes)?;
        }
    }
    Ok(())
}

fn ancestors(node: &Node, nodes: &BTreeMap<NodeId, Node>) -> BTreeSet<NodeId> {
    let mut found = BTreeSet::new();
    let mut pending = node.needs.clone();
    while let Some(id) = pending.pop() {
        if found.insert(id.clone()) {
            pending.extend(
                nodes
                    .get(&id)
                    .into_iter()
                    .flat_map(|node| node.needs.iter().cloned()),
            );
        }
    }
    found
}

fn template_references(node: &Node) -> Vec<&OutputReference> {
    let templates: Vec<_> = match &node.execution {
        NodeExecution::Agent(agent) => vec![&agent.prompt],
        NodeExecution::Command(super::CommandNode::Direct { arguments, .. }) => {
            arguments.iter().collect()
        }
        NodeExecution::Command(super::CommandNode::Shell { .. }) => Vec::new(),
    };
    templates
        .into_iter()
        .flat_map(|template| template.0.iter())
        .filter_map(|part| match part {
            TemplatePart::Output { reference } => Some(reference),
            TemplatePart::Literal { .. } => None,
        })
        .collect()
}

fn validate_condition_schemas(
    condition: &Condition,
    nodes: &BTreeMap<NodeId, Node>,
) -> WorkflowResult<()> {
    match condition {
        Condition::Output {
            node,
            path,
            predicate,
        } => {
            let reference = OutputReference {
                node: node.clone(),
                path: path.clone(),
            };
            validate_output_reference(&reference, nodes)?;
            let schema = nodes[node]
                .execution
                .output_schema()
                .and_then(|schema| schema.schema_at_path(&path.0))
                .expect("output reference was validated");
            match predicate {
                super::ValuePredicate::Equals(value) | super::ValuePredicate::NotEquals(value) => {
                    schema.validate_value(value)
                }
                super::ValuePredicate::IsOneOf(values) => values
                    .iter()
                    .try_for_each(|value| schema.validate_value(value)),
            }
        }
        Condition::CommandExit { node, .. } => {
            if matches!(
                nodes.get(node).map(|node| &node.execution),
                Some(NodeExecution::Command(_))
            ) {
                Ok(())
            } else {
                Err(WorkflowError::Condition(format!(
                    "command exit reference targets non-command node '{node}'"
                )))
            }
        }
        Condition::All { conditions } | Condition::Any { conditions } => conditions
            .iter()
            .try_for_each(|condition| validate_condition_schemas(condition, nodes)),
        Condition::Not { condition } => validate_condition_schemas(condition, nodes),
        Condition::NodeStatus { .. } => Ok(()),
    }
}

fn validate_output_reference(
    reference: &OutputReference,
    nodes: &BTreeMap<NodeId, Node>,
) -> WorkflowResult<()> {
    let schema = nodes
        .get(&reference.node)
        .and_then(|node| node.execution.output_schema())
        .ok_or_else(|| {
            WorkflowError::Condition(format!(
                "output reference targets node '{}' without a schema",
                reference.node
            ))
        })?;
    if schema.schema_at_path(&reference.path.0).is_none() {
        return Err(WorkflowError::Condition(format!(
            "output path {:?} does not exist in node '{}' schema",
            reference.path.0, reference.node
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
