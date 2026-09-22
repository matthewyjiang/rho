use std::{path::PathBuf, sync::Arc};

use crate::workflow::{NodeId, ResolvedNode, RunId, StoredRun, WorkspaceAccess};

use super::{
    cancellation::CancellationRequest, RuntimeError, RuntimeEvent, RuntimeSecurity,
    WorkflowNodeExecutor,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryDecision {
    NormalResume,
    ConfirmNoProcess,
}

pub(crate) struct WorkflowRunner {
    pub(super) rho_home: PathBuf,
    pub(super) workspace: PathBuf,
    security: RuntimeSecurity,
    pub(super) agents: Arc<dyn WorkflowNodeExecutor<super::prepared::AgentInvocation>>,
    pub(super) commands: Arc<dyn WorkflowNodeExecutor<super::prepared::CommandInvocation>>,
    pub(super) cancellation: rho_sdk::CancellationToken,
    /// Wakes the drive loop to re-check durable cancellation without waiting for
    /// the cross-process poll interval. Production CLI cancel still relies on the
    /// poll as a fallback when the owner process is separate.
    pub(super) cancel_check: Arc<tokio::sync::Notify>,
    pub(super) hooks: Option<Arc<crate::hooks::HookEngine>>,
    pub(super) custom_providers: Arc<[String]>,
}

impl WorkflowRunner {
    pub(crate) fn new(
        rho_home: PathBuf,
        workspace: PathBuf,
        security: RuntimeSecurity,
        agents: Arc<dyn WorkflowNodeExecutor<super::prepared::AgentInvocation>>,
        commands: Arc<dyn WorkflowNodeExecutor<super::prepared::CommandInvocation>>,
    ) -> Self {
        Self {
            rho_home,
            workspace,
            security,
            agents,
            commands,
            cancellation: rho_sdk::CancellationToken::new(),
            cancel_check: Arc::new(tokio::sync::Notify::new()),
            hooks: None,
            custom_providers: Arc::from([]),
        }
    }

    pub(crate) fn with_hooks(mut self, hooks: Arc<crate::hooks::HookEngine>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub(crate) fn with_custom_providers(mut self, names: Arc<[String]>) -> Self {
        self.custom_providers = names;
        self
    }

    /// Ask the drive loop to re-read the durable cancellation request file now.
    #[cfg(test)]
    pub(crate) fn wake_cancel_check(&self) {
        self.cancel_check.notify_one();
    }

    pub(crate) fn cancellation_request(&self, run_id: RunId) -> CancellationRequest {
        CancellationRequest {
            rho_home: self.rho_home.clone(),
            run_id,
            cancellation: self.cancellation.clone(),
            cancel_check: Arc::clone(&self.cancel_check),
        }
    }

    pub(crate) async fn drive(
        &self,
        run_id: RunId,
        recovery: RecoveryDecision,
        events: Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    ) -> Result<StoredRun, RuntimeError> {
        super::drive_session::drive(self, run_id, recovery, events).await
    }

    pub(super) fn validate_security(&self, run: &StoredRun) -> Result<(), RuntimeError> {
        let current_path = self.workspace.canonicalize()?;
        let current = crate::paths::display(&current_path);
        let identity_matches = {
            #[cfg(windows)]
            {
                current == run.manifest.workspace_identity
                    || crate::workflow::windows_paths_match(
                        std::path::Path::new(&run.manifest.workspace_identity),
                        &current_path,
                    )
            }
            #[cfg(not(windows))]
            {
                current == run.manifest.workspace_identity
            }
        };
        if !identity_matches {
            return Err(RuntimeError::WorkspaceChanged {
                planned: run.manifest.workspace_identity.clone(),
                current,
            });
        }
        for node in run.graph.program.root.nodes.values() {
            let resolved = run.graph.resolved_nodes.get(&node.id).ok_or_else(|| {
                RuntimeError::DefinitionLaunchMetadata {
                    node: node.id.clone(),
                }
            })?;
            match resolved {
                ResolvedNode::Agent(agent) => {
                    if agent.trust_required && !self.security.project_trusted {
                        return Err(RuntimeError::TrustRemoved {
                            node: node.id.clone(),
                        });
                    }
                    validate_agent_access(&node.id, node.access, agent)?;
                }
                ResolvedNode::Command(_) if node.access != WorkspaceAccess::Mutating => {
                    return Err(RuntimeError::ReadOnlyCapability {
                        node: node.id.clone(),
                        capability: "commands are always mutating".into(),
                    });
                }
                ResolvedNode::Command(_) => {}
            }
        }
        Ok(())
    }
}

fn validate_agent_access(
    node: &NodeId,
    access: WorkspaceAccess,
    agent: &crate::workflow::ResolvedAgent,
) -> Result<(), RuntimeError> {
    if access == WorkspaceAccess::Mutating {
        return Ok(());
    }
    if agent.runtime.is_external_cli() {
        return Err(RuntimeError::ReadOnlyCapability {
            node: node.clone(),
            capability: format!(
                "{} does not support read-only workflow execution",
                agent.runtime
            ),
        });
    }
    const MUTATING: &[&str] = &[
        "agent",
        "agents",
        "bash",
        "edit",
        "powershell",
        "process",
        "rho",
        "shell",
        "workflow",
        "write",
    ];
    if let Some(capability) = agent
        .capabilities
        .iter()
        .find(|capability| MUTATING.contains(&capability.as_str()))
    {
        return Err(RuntimeError::ReadOnlyCapability {
            node: node.clone(),
            capability: capability.clone(),
        });
    }
    Ok(())
}

pub(super) fn send_event(
    sender: &Option<tokio::sync::mpsc::UnboundedSender<RuntimeEvent>>,
    event: RuntimeEvent,
) {
    if let Some(sender) = sender {
        let _ = sender.send(event);
    }
}
