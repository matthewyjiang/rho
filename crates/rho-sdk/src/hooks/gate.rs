use std::{future::Future, pin::Pin};

use super::{envelope::HookEnvelope, payload::HookPolicyOutcome};

/// Result of a blocking pre-action hook.
///
/// A hook may let the existing decision stand or make it stricter. There is no
/// variant that grants authority, and none that rewrites tool arguments: a hook
/// cannot widen workspace policy, sandbox policy, permission mode, or a denial
/// the host already made.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookDecision {
    Continue,
    Deny { reason: String },
}

impl HookDecision {
    pub fn deny(reason: impl Into<String>) -> Self {
        Self::Deny {
            reason: reason.into(),
        }
    }

    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }

    /// Returns the denial reason, or `None` when the operation may continue.
    pub fn denial_reason(&self) -> Option<&str> {
        match self {
            Self::Continue => None,
            Self::Deny { reason } => Some(reason),
        }
    }
}

/// One `before_tool_use` question put to the gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreToolUseRequest {
    envelope: HookEnvelope,
    policy: HookPolicyOutcome,
}

impl PreToolUseRequest {
    pub(crate) fn new(envelope: HookEnvelope, policy: HookPolicyOutcome) -> Self {
        Self { envelope, policy }
    }

    pub fn envelope(&self) -> &HookEnvelope {
        &self.envelope
    }

    /// Host policy outcome the hook is being asked to narrow.
    ///
    /// Never `Deny`: a policy denial short-circuits before the gate runs.
    pub fn policy(&self) -> HookPolicyOutcome {
        self.policy
    }
}

/// Future returned by a [`PreToolUseGate`].
pub type HookGateFuture<'a> = Pin<Box<dyn Future<Output = HookDecision> + Send + 'a>>;

/// Deny-only gate consulted before a capability-bearing tool call is authorized.
///
/// The runtime calls this after [`WorkspacePolicy::evaluate`] and before any
/// approval await, so a denial happens before the host is prompted. Implementors
/// must:
///
/// - fail closed: return [`HookDecision::Deny`] when their own machinery fails,
///   because a gate that fails open is decoration;
/// - stay bounded: the runtime does not impose a timeout, so the implementation
///   owns per-handler and aggregate deadlines;
/// - stay reentrancy-free: work started by a gate must not re-enter the agent's
///   tool loop.
///
/// [`WorkspacePolicy::evaluate`]: crate::WorkspacePolicy::evaluate
pub trait PreToolUseGate: Send + Sync {
    /// Cheap check used before the runtime builds a full envelope.
    ///
    /// Return `false` when no configured handler can match `tool_name`, so
    /// observational-only installs and unmatched tools skip payload work.
    /// Defaults to `true`.
    fn applies_to_tool(&self, _tool_name: &str) -> bool {
        true
    }

    fn evaluate(&self, request: PreToolUseRequest) -> HookGateFuture<'_>;
}

/// Gate that lets every request continue. Useful as an explicit host default.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllGate;

impl PreToolUseGate for AllowAllGate {
    fn evaluate(&self, _request: PreToolUseRequest) -> HookGateFuture<'_> {
        Box::pin(std::future::ready(HookDecision::Continue))
    }
}

impl std::fmt::Debug for dyn PreToolUseGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreToolUseGate(..)")
    }
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
