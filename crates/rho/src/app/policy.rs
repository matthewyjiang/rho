use rho_sdk::{CapabilityRequest, PolicyDecision, WorkspacePolicy};

use crate::permission::{ModePolicy, PermissionMode};

#[derive(Clone, Copy, Debug)]
pub(crate) enum AppPolicy {
    Allow,
    Mode(ModePolicy),
}

impl AppPolicy {
    pub(crate) fn for_mode(mode: PermissionMode) -> Self {
        match mode.workspace_policy() {
            Some(policy) => Self::Mode(policy),
            None => Self::Allow,
        }
    }
}

impl WorkspacePolicy for AppPolicy {
    fn evaluate(&self, request: &CapabilityRequest) -> PolicyDecision {
        match self {
            Self::Allow => PolicyDecision::Allow,
            Self::Mode(policy) => policy.evaluate(request),
        }
    }
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
