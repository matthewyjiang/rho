//! Binds one selected leaf before handing it to an executor.
use std::{collections::BTreeMap, path::Path};

use rho_sdk::ProcessInvocation;

use crate::workflow::{
    CommandNode, FrozenRuntimeLimits, FrozenWorkflow, Node, NodeExecution, NodeId, OutputSchema,
    ResolvedAgent, ResolvedCommand, ResolvedNode, TaskInstanceId, WorkflowState, WorkflowValue,
};

use super::{
    template::{check_runtime_limit, render_template},
    RuntimeError,
};

/// The executor receives only the selected leaf, never the workflow or sibling outputs.
#[derive(Clone)]
pub(crate) struct PreparedInvocation {
    pub(crate) execution: PreparedExecution,
    pub(crate) output: Option<OutputSchema>,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_output_bytes: u64,
}

#[derive(Clone)]
pub(crate) enum PreparedExecution {
    Agent {
        agent: Box<ResolvedAgent>,
        prompt: String,
    },
    Command {
        resolved: Box<ResolvedCommand>,
        invocation: ProcessInvocation,
        progress_message: String,
    },
}

impl PreparedInvocation {
    /// Bind using frozen authority. No discovery or permission decisions happen here.
    pub(crate) fn prepare(
        workflow: &FrozenWorkflow,
        node_id: &TaskInstanceId,
        state: &WorkflowState,
    ) -> Result<Self, RuntimeError> {
        let scope = state
            .scope(node_id.scope())
            .ok_or_else(|| RuntimeError::LaunchMetadata {
                node: node_id.clone(),
            })?;
        let node = workflow
            .program
            .root
            .nodes
            .get(node_id.definition())
            .ok_or_else(|| RuntimeError::LaunchMetadata {
                node: node_id.clone(),
            })?;
        let resolved = workflow
            .resolved_nodes
            .get(node_id.definition())
            .ok_or_else(|| RuntimeError::LaunchMetadata {
                node: node_id.clone(),
            })?;
        Self::prepare_node(
            node_id,
            node,
            resolved,
            &workflow.runtime_limits,
            &scope.outputs,
        )
    }

    /// Prepare a leaf already selected from a scope by the driver.
    pub(crate) fn prepare_node(
        task: &TaskInstanceId,
        node: &Node,
        resolved: &ResolvedNode,
        limits: &FrozenRuntimeLimits,
        outputs: &BTreeMap<NodeId, WorkflowValue>,
    ) -> Result<Self, RuntimeError> {
        let execution = match (&node.execution, resolved) {
            (NodeExecution::Agent(node), ResolvedNode::Agent(agent)) => {
                let mut prompt = render_template(&node.prompt, outputs, limits)?;
                if let Some(schema) = &node.output {
                    prompt.push_str("\n\nReturn exactly one JSON value as the final answer. Do not use a code fence. Schema: ");
                    prompt.push_str(&serde_json::to_string(schema)?);
                }
                check_runtime_limit(
                    "prompt expansion bytes",
                    limits.prompt_expansion_bytes,
                    prompt.len() as u64,
                )?;
                PreparedExecution::Agent {
                    agent: agent.clone(),
                    prompt,
                }
            }
            (NodeExecution::Command(command), ResolvedNode::Command(resolved)) => {
                let executable = Path::new(&resolved.executable);
                let invocation = invocation(command, executable, outputs, limits)?;
                let progress_message = command_progress_message(command, executable, &invocation);
                PreparedExecution::Command {
                    resolved: resolved.clone(),
                    invocation,
                    progress_message,
                }
            }
            (NodeExecution::Agent(_) | NodeExecution::Command(_), _) => {
                return Err(RuntimeError::LaunchMetadata { node: task.clone() });
            }
        };
        Ok(Self {
            execution,
            output: node.output_schema().cloned(),
            timeout_seconds: node.timeout_seconds,
            max_output_bytes: node.max_output_bytes,
        })
    }
}

fn invocation(
    command: &CommandNode,
    executable: &Path,
    outputs: &BTreeMap<NodeId, WorkflowValue>,
    limits: &crate::workflow::FrozenRuntimeLimits,
) -> Result<ProcessInvocation, RuntimeError> {
    let invocation = match command {
        CommandNode::Direct { arguments, .. } => {
            let arguments = arguments
                .iter()
                .map(|argument| render_template(argument, outputs, limits))
                .collect::<Result<Vec<_>, _>>()?;
            let argv_bytes = arguments.iter().try_fold(
                executable.as_os_str().as_encoded_bytes().len() as u64,
                |total, argument| {
                    total.checked_add(argument.len() as u64).ok_or({
                        RuntimeError::Workflow(crate::workflow::WorkflowError::BudgetExceeded {
                            budget: "argv expansion bytes",
                            limit: limits.argv_expansion_bytes,
                            actual: u64::MAX,
                        })
                    })
                },
            )?;
            check_runtime_limit(
                "argv expansion bytes",
                limits.argv_expansion_bytes,
                argv_bytes,
            )?;
            ProcessInvocation::executable(executable, arguments)
        }
        CommandNode::Shell {
            arguments, command, ..
        } => ProcessInvocation::shell(executable, arguments.clone(), command),
    };
    Ok(invocation)
}

fn command_progress_message(
    command: &CommandNode,
    executable: &Path,
    invocation: &ProcessInvocation,
) -> String {
    match command {
        CommandNode::Shell { command, .. } => {
            let shell = executable
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("shell");
            format!("running {shell}: {command}")
        }
        CommandNode::Direct { .. } => {
            let exe = executable
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("command");
            let args = invocation.arguments();
            if args.is_empty() {
                format!("running {exe}")
            } else {
                let joined = args.join(" ");
                let summary = if joined.chars().count() > 140 {
                    let mut out = joined.chars().take(139).collect::<String>();
                    out.push('…');
                    out
                } else {
                    joined
                };
                format!("running {exe} {summary}")
            }
        }
    }
}

#[cfg(test)]
#[path = "prepared_tests.rs"]
mod tests;
