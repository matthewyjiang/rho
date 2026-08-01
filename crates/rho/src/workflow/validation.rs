use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

use super::{
    AgentRuntime, Condition, FrozenWorkflow, Node, NodeExecution, NodeId, OutputReference,
    PlanningLimits, Template, TemplatePart, WorkflowError, WorkflowResult, WorkspaceAccess,
};

pub(crate) fn validate_workflow(workflow: &FrozenWorkflow) -> WorkflowResult<()> {
    workflow.runtime_limits.validate()?;
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

pub(crate) fn validate_runtime_budgets(
    workflow: &FrozenWorkflow,
    limits: &PlanningLimits,
) -> WorkflowResult<()> {
    let mut retained_total = 0_u64;
    for node in workflow.graph.nodes.values() {
        limits
            .node_timeout_seconds
            .check_nonzero(node.timeout_seconds)?;
        limits
            .retained_output_per_stream_bytes
            .check_nonzero(node.max_output_bytes)?;
        let stream_count = if matches!(node.execution, NodeExecution::Command(_)) {
            2
        } else {
            1
        };
        retained_total = checked_add(
            &limits.retained_output_total_bytes,
            retained_total,
            node.max_output_bytes.saturating_mul(stream_count),
        )?;

        match &node.execution {
            NodeExecution::Agent(agent) => {
                let mut prompt = template_expansion_bound(&agent.prompt, workflow, limits)?;
                if let Some(schema) = &agent.output {
                    prompt = checked_add(
                        &limits.prompt_expansion_bytes,
                        prompt,
                        serde_json::to_vec(schema)?.len() as u64,
                    )?;
                }
                limits.prompt_expansion_bytes.check(prompt)?;
            }
            NodeExecution::Command(super::CommandNode::Direct {
                executable,
                arguments,
                ..
            }) => {
                let mut argv_bytes = executable.len() as u64;
                for argument in arguments {
                    argv_bytes = checked_add(
                        &limits.argv_expansion_bytes,
                        argv_bytes,
                        template_expansion_bound(argument, workflow, limits)?,
                    )?;
                }
                limits.argv_expansion_bytes.check(argv_bytes)?;
            }
            NodeExecution::Command(super::CommandNode::Shell {
                executable,
                arguments,
                command,
                ..
            }) => {
                let argv_bytes = arguments.iter().try_fold(
                    executable.len() as u64 + command.len() as u64,
                    |total, argument| {
                        checked_add(&limits.argv_expansion_bytes, total, argument.len() as u64)
                    },
                )?;
                limits.argv_expansion_bytes.check(argv_bytes)?;
            }
        }
    }
    limits.retained_output_total_bytes.check(retained_total)?;
    // Workflow schema v1 has no source-controlled environment entries.
    limits.environment_expansion_bytes.check(0)
}

fn template_expansion_bound(
    template: &Template,
    workflow: &FrozenWorkflow,
    limits: &PlanningLimits,
) -> WorkflowResult<u64> {
    let mut bytes = 0_u64;
    for part in &template.0 {
        let part_bytes = match part {
            TemplatePart::Literal { value } => value.len() as u64,
            TemplatePart::Output { reference } => workflow
                .graph
                .nodes
                .get(&reference.node)
                .map_or(u64::MAX, |node| node.max_output_bytes),
        };
        bytes = checked_add(&limits.rendered_template_bytes, bytes, part_bytes)?;
    }
    limits.rendered_template_bytes.check(bytes)?;
    Ok(bytes)
}

fn checked_add(budget: &super::Budget, left: u64, right: u64) -> WorkflowResult<u64> {
    let actual = left.saturating_add(right);
    budget.check(actual)?;
    Ok(actual)
}

fn validate_node_shape(node: &Node, workflow: &FrozenWorkflow) -> WorkflowResult<()> {
    if node.timeout_seconds == 0
        || node.timeout_seconds > workflow.runtime_limits.node_timeout_seconds
    {
        return Err(WorkflowError::BudgetExceeded {
            budget: "node timeout seconds",
            limit: workflow.runtime_limits.node_timeout_seconds,
            actual: node.timeout_seconds,
        });
    }
    if node.max_output_bytes == 0
        || node.max_output_bytes > workflow.runtime_limits.retained_output_per_stream_bytes
        || node.max_output_bytes > workflow.runtime_limits.retained_output_total_bytes
    {
        return Err(WorkflowError::BudgetExceeded {
            budget: "node retained output bytes",
            limit: workflow
                .runtime_limits
                .retained_output_per_stream_bytes
                .min(workflow.runtime_limits.retained_output_total_bytes),
            actual: node.max_output_bytes,
        });
    }
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
    if let Some(super::ResolvedNode::Command(command)) = workflow.resolved_nodes.get(&node.id) {
        if command.executable != command.executable_identity.file.canonical_path {
            return Err(WorkflowError::InvalidAccess {
                node: node.id.clone(),
                reason: "resolved command path differs from its frozen executable identity"
                    .to_owned(),
            });
        }
        if command.cwd != command.cwd_identity.canonical_path {
            return Err(WorkflowError::InvalidAccess {
                node: node.id.clone(),
                reason: "resolved command cwd differs from its frozen directory identity"
                    .to_owned(),
            });
        }
    }
    if let Some(super::ResolvedNode::Agent(agent)) = workflow.resolved_nodes.get(&node.id) {
        match (&agent.executable, &agent.executable_identity) {
            (Some(executable), Some(identity)) if executable != &identity.file.canonical_path => {
                return Err(WorkflowError::InvalidAccess {
                    node: node.id.clone(),
                    reason: "resolved agent path differs from its frozen executable identity"
                        .to_owned(),
                });
            }
            (Some(_), None) | (None, Some(_)) => {
                return Err(WorkflowError::InvalidAccess {
                    node: node.id.clone(),
                    reason: "resolved agent executable and identity must both be present"
                        .to_owned(),
                });
            }
            (Some(_), Some(_)) | (None, None) => {}
        }
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
