//! Binds one selected leaf before handing it to an executor.
use std::{collections::BTreeMap, path::Path};

use rho_sdk::{ProcessInvocation, SessionId};

use crate::workflow::{
    AgentRuntime, CommandNode, FrozenRuntimeLimits, FrozenWorkflow, Leaf, LeafExecution, NodeId,
    OutputSchema, ResolvedAgent, ResolvedCommand, TaskInstanceId, WorkflowState, WorkflowValue,
};

use super::{
    template::{check_runtime_limit, render_template},
    RuntimeError,
};

/// The executor receives only the selected leaf, never the workflow or sibling outputs.
#[derive(Clone)]
pub(crate) struct PreparedInvocation<I = PreparedExecution> {
    pub(crate) execution: I,
    pub(crate) output: Option<OutputSchema>,
    pub(crate) timeout_seconds: u64,
    pub(crate) max_output_bytes: u64,
}

#[derive(Clone)]
pub(crate) enum PreparedExecution {
    Agent(AgentInvocation),
    Command(CommandInvocation),
}

#[derive(Clone)]
pub(crate) struct AgentInvocation {
    pub(crate) agent: Box<ResolvedAgent>,
    pub(crate) prompt: String,
    /// The session a Rho agent runs in; agents on other runtimes have none.
    pub(crate) session: Option<AgentSession>,
}

/// The saved session a Rho agent node's attempt runs in. Its `status.json`
/// records the id before the attempt is dispatched, so a later process can
/// reopen it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentSession {
    /// The attempt's first run, in a new session under this id.
    New(SessionId),
    /// An attempt an earlier process left running, continuing in the session
    /// it saved under this id.
    Reopen(SessionId),
}

#[derive(Clone)]
pub(crate) struct CommandInvocation {
    pub(crate) resolved: Box<ResolvedCommand>,
    pub(crate) invocation: ProcessInvocation,
}

impl<I> PreparedInvocation<I> {
    pub(super) fn map_execution<J>(self, map: impl FnOnce(I) -> J) -> PreparedInvocation<J> {
        PreparedInvocation {
            execution: map(self.execution),
            output: self.output,
            timeout_seconds: self.timeout_seconds,
            max_output_bytes: self.max_output_bytes,
        }
    }
}

impl PreparedInvocation {
    /// Bind using frozen authority. No discovery or permission decisions happen here.
    /// A Rho agent's attempt runs in `session`; other nodes drop it.
    pub(crate) fn prepare(
        workflow: &FrozenWorkflow,
        node_id: &TaskInstanceId,
        state: &WorkflowState,
        session: AgentSession,
    ) -> Result<Self, RuntimeError> {
        let (scope, _) = state
            .local(node_id)
            .map_err(|_| RuntimeError::LaunchMetadata {
                node: node_id.clone(),
            })?;
        let leaf = workflow
            .leaf(node_id)
            .ok_or_else(|| RuntimeError::LaunchMetadata {
                node: node_id.clone(),
            })?;
        Self::prepare_node(leaf, &workflow.runtime_limits, &scope.outputs, session)
    }

    /// Prepare a leaf already selected from a scope by the driver.
    pub(crate) fn prepare_node(
        leaf: Leaf<'_>,
        limits: &FrozenRuntimeLimits,
        outputs: &BTreeMap<NodeId, WorkflowValue>,
        session: AgentSession,
    ) -> Result<Self, RuntimeError> {
        let execution = match leaf.execution {
            LeafExecution::Agent {
                node,
                resolved: agent,
            } => {
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
                // Only a Rho agent runs in a Rho session.
                let session = match agent.runtime {
                    AgentRuntime::Rho => Some(session),
                    AgentRuntime::ClaudeCli | AgentRuntime::Cursor | AgentRuntime::Antigravity => {
                        None
                    }
                };
                PreparedExecution::Agent(AgentInvocation {
                    agent: Box::new(agent.clone()),
                    prompt,
                    session,
                })
            }
            LeafExecution::Command {
                node: command,
                resolved,
            } => {
                let executable = Path::new(&resolved.executable);
                let invocation = invocation(command, executable, outputs, limits)?;
                PreparedExecution::Command(CommandInvocation {
                    resolved: Box::new(resolved.clone()),
                    invocation,
                })
            }
        };
        Ok(Self {
            execution,
            output: leaf.node.output_schema().cloned(),
            timeout_seconds: leaf.node.timeout_seconds,
            max_output_bytes: leaf.node.max_output_bytes,
        })
    }

    /// The id of the session a Rho agent node's attempt runs in, which its
    /// `status.json` records. Other nodes run none.
    pub(super) fn agent_session_id(&self) -> Option<&SessionId> {
        match &self.execution {
            PreparedExecution::Agent(AgentInvocation {
                session: Some(AgentSession::New(id) | AgentSession::Reopen(id)),
                ..
            }) => Some(id),
            PreparedExecution::Agent(AgentInvocation { session: None, .. })
            | PreparedExecution::Command(_) => None,
        }
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

#[cfg(test)]
#[path = "prepared_tests.rs"]
mod tests;
