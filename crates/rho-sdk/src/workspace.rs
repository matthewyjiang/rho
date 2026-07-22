use std::{collections::BTreeSet, fmt};

mod approval;
mod capability;
mod path;

#[cfg(test)]
pub(crate) use approval::authorize;
pub use approval::{
    approval_channel, ApprovalAuditDecision, ApprovalAuditRecord, ApprovalDecision, ApprovalFuture,
    ApprovalHandler, ApprovalRequest, ApprovalRequestReceiver, AuthorizationDenialKind,
    AuthorizationError, AuthorizationOutcome, ChannelApprovalHandler, DenyApprovals,
    PendingApproval,
};
pub(crate) use approval::{authorize_for_call, ApprovalAuditLog, SessionApprovals};
pub use capability::{
    managed_credential_env_vars, set_managed_credential_env_vars, CapabilityKind,
    CapabilityOperation, CapabilityRequest, CapabilitySource, ExecutableSelection, NetworkTarget,
    PathScope, ProcessEnvironment, ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};
pub use path::{
    ResolvedWorkspacePath, Workspace, WorkspacePathError, WorkspacePathErrorKind,
    WorkspacePathState,
};

/// Decision returned by a [`WorkspacePolicy`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    RequireApproval { reason: String },
}

/// Host policy for filesystem, process, network, skill, and instruction capabilities.
pub trait WorkspacePolicy: Send + Sync {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision;
}

/// Policy that denies every security-sensitive capability.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllPolicy;

impl WorkspacePolicy for DenyAllPolicy {
    fn evaluate(&self, _request: &CapabilityRequest) -> PolicyDecision {
        PolicyDecision::Deny {
            reason: "capability is denied by default".into(),
        }
    }
}

/// Explicit opt-in policy for independently scoped workspace capabilities.
#[derive(Clone, Debug, Default)]
pub struct ScopedWorkspacePolicy {
    allowed: BTreeSet<CapabilityKind>,
    network_hosts: BTreeSet<String>,
    network_tools: BTreeSet<String>,
    require_approval: BTreeSet<CapabilityKind>,
    outside_workspace_paths: bool,
}

impl ScopedWorkspacePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow_read_paths(mut self) -> Self {
        self.allowed.insert(CapabilityKind::Read);
        self
    }

    pub fn allow_write_paths(mut self) -> Self {
        self.allowed.insert(CapabilityKind::Write);
        self
    }

    pub fn allow_processes(mut self) -> Self {
        self.allowed.insert(CapabilityKind::Process);
        self
    }

    pub fn allow_skills(mut self) -> Self {
        self.allowed.insert(CapabilityKind::Skill);
        self
    }

    pub fn allow_instruction_discovery(mut self) -> Self {
        self.allowed.insert(CapabilityKind::InstructionDiscovery);
        self
    }

    /// Allows capability checks for paths under roots deliberately attached to
    /// [`Workspace::with_granted_root`]. This does not grant read or write by
    /// itself.
    pub fn allow_outside_workspace_paths(mut self) -> Self {
        self.outside_workspace_paths = true;
        self
    }

    pub fn allow_network_host(mut self, host: impl Into<String>) -> Self {
        self.allowed.insert(CapabilityKind::Network);
        self.network_hosts
            .insert(host.into().trim_end_matches('.').to_ascii_lowercase());
        self
    }

    /// Allows a built-in whose destination is selected internally. URL-taking
    /// built-ins should use [`Self::allow_network_host`] instead.
    pub fn allow_network_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.allowed.insert(CapabilityKind::Network);
        self.network_tools.insert(tool_name.into());
        self
    }

    pub fn require_read_approval(self) -> Self {
        self.require_approval_for(CapabilityKind::Read)
    }

    pub fn require_write_approval(self) -> Self {
        self.require_approval_for(CapabilityKind::Write)
    }

    pub fn require_process_approval(self) -> Self {
        self.require_approval_for(CapabilityKind::Process)
    }

    pub fn require_network_approval(self) -> Self {
        self.require_approval_for(CapabilityKind::Network)
    }

    pub fn require_skill_approval(self) -> Self {
        self.require_approval_for(CapabilityKind::Skill)
    }

    pub fn require_instruction_approval(self) -> Self {
        self.require_approval_for(CapabilityKind::InstructionDiscovery)
    }

    fn require_approval_for(mut self, capability: CapabilityKind) -> Self {
        self.require_approval.insert(capability);
        self
    }
}

impl WorkspacePolicy for ScopedWorkspacePolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        let kind = request.kind();
        if !self.allowed.contains(&kind) {
            return denied();
        }
        if request.is_outside_primary_root() && !self.outside_workspace_paths {
            return PolicyDecision::Deny {
                reason: "access to a granted root requires an explicit outside-workspace grant"
                    .into(),
            };
        }
        if let CapabilityOperation::NetworkAccess(target) = request.operation() {
            let allowed = match target {
                NetworkTarget::Url(url) => allowed_url_host(url, &self.network_hosts),
                NetworkTarget::ToolManaged => match request.source() {
                    CapabilitySource::BuiltInTool { name } => self.network_tools.contains(name),
                    CapabilitySource::HostProvidedTool { .. }
                    | CapabilitySource::PromptConstruction => false,
                },
            };
            if !allowed {
                return PolicyDecision::Deny {
                    reason: "network destination is outside the configured policy".into(),
                };
            }
        }
        if self.require_approval.contains(&kind) {
            PolicyDecision::RequireApproval {
                reason: "host approval is required".into(),
            }
        } else {
            PolicyDecision::Allow
        }
    }
}

fn allowed_url_host(url: &str, hosts: &BTreeSet<String>) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    url.host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
        .is_some_and(|host| hosts.contains(&host))
}

fn denied() -> PolicyDecision {
    PolicyDecision::Deny {
        reason: "capability is outside the configured policy".into(),
    }
}

impl fmt::Debug for dyn WorkspacePolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WorkspacePolicy(..)")
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
