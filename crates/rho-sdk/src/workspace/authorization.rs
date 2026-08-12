//! Capability authorization: policy, hooks, then host approval.
//!
//! This module is the composition root for one authorize call. Approval
//! handlers, remembered decisions, and audit records live in [`super::approval`];
//! hooks supply only a deny-only gate. Keeping that wiring here stops
//! `approval.rs` from owning the whole pipeline.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::hooks::{
    summarize_capability, BeforeToolUsePayload, HookDecision, HookPayload, HookPolicyOutcome,
    HookTool, HookWiring, PreToolUseRequest,
};

use super::{
    approval::{
        ApprovalAuditDecision, ApprovalAuditLog, ApprovalContext, ApprovalDecision,
        ApprovalHandler, ApprovalRequest, AuthorizationDenialKind, AuthorizationError,
        AuthorizationOutcome, DenyApprovals, SessionApprovals,
    },
    CapabilityRequest, PolicyDecision, WorkspacePolicy,
};

/// Lazily reads the conversation available to approval handlers for this run.
pub(crate) type LiveHistorySource = Arc<dyn Fn() -> Vec<crate::model::Message> + Send + Sync>;

/// Where one authorization happened, for hook envelope identity.
#[derive(Clone, Default)]
pub(crate) struct AuthorizationScope {
    pub(crate) session_id: Option<crate::SessionId>,
    pub(crate) run_id: Option<crate::RunId>,
    pub(crate) workspace_root: Option<PathBuf>,
    pub(crate) live_history: Option<LiveHistorySource>,
}

impl fmt::Debug for AuthorizationScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationScope")
            .field("session_id", &self.session_id)
            .field("run_id", &self.run_id)
            .field("workspace_root", &self.workspace_root)
            .field("live_history", &self.live_history.is_some())
            .finish()
    }
}

impl AuthorizationScope {
    fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }
}

/// Everything one tool invocation needs to authorize a capability.
///
/// Bundled so the authorization call site names one collaborator instead of six
/// positional handles, and so adding the hook gate did not widen every caller.
pub(crate) struct AuthorizationServices {
    policy: Arc<dyn WorkspacePolicy>,
    approvals: Arc<dyn ApprovalHandler>,
    remembered: Arc<SessionApprovals>,
    audit: Arc<ApprovalAuditLog>,
    hooks: HookWiring,
    scope: AuthorizationScope,
}

impl AuthorizationServices {
    pub(crate) fn new(
        policy: Arc<dyn WorkspacePolicy>,
        approvals: Arc<dyn ApprovalHandler>,
        remembered: Arc<SessionApprovals>,
        audit: Arc<ApprovalAuditLog>,
        hooks: HookWiring,
        scope: AuthorizationScope,
    ) -> Self {
        Self {
            policy,
            approvals,
            remembered,
            audit,
            hooks,
            scope,
        }
    }

    /// Denies every capability and consults no hook. Used by hosts that build a
    /// bare [`ToolContext`](crate::tool::ToolContext).
    pub(crate) fn denied() -> Self {
        Self::new(
            Arc::new(super::DenyAllPolicy),
            Arc::new(DenyApprovals),
            Arc::default(),
            Arc::default(),
            HookWiring::default(),
            AuthorizationScope::default(),
        )
    }

    pub(crate) fn audit(&self) -> &Arc<ApprovalAuditLog> {
        &self.audit
    }

    pub(crate) fn approval_session(&self) -> super::ApprovalSession {
        super::ApprovalSession::from_parts(
            Arc::clone(&self.approvals),
            Arc::clone(&self.remembered),
            Arc::clone(&self.audit),
        )
    }

    fn approval_context(&self, cancellation: crate::CancellationToken) -> ApprovalContext {
        let history = self
            .scope
            .live_history
            .as_ref()
            .map_or_else(Vec::new, |source| source());
        ApprovalContext::new(
            self.scope
                .session_id
                .clone()
                .unwrap_or_else(crate::SessionId::new),
            cancellation,
            history,
        )
    }
}

impl fmt::Debug for AuthorizationServices {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthorizationServices")
            .field("policy", &self.policy)
            .field("approvals", &self.approvals)
            .field("hooks", &self.hooks)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) async fn authorize(
    services: &AuthorizationServices,
    request: CapabilityRequest,
) -> Result<AuthorizationOutcome, AuthorizationError> {
    authorize_for_call(services, request, None, crate::CancellationToken::new()).await
}

/// Resolves one capability request through policy, hooks, and host approval.
///
/// Order is fixed and observable: host policy decides first, a trusted deny-only
/// hook may then narrow an `Allow` or `RequireApproval` to a denial, and only
/// after that does the host get prompted. A hook therefore never sees a request
/// policy already denied, and a denial never reaches an approval prompt.
pub(crate) async fn authorize_for_call(
    services: &AuthorizationServices,
    request: CapabilityRequest,
    tool_call_id: Option<&crate::ToolCallId>,
    cancellation: crate::CancellationToken,
) -> Result<AuthorizationOutcome, AuthorizationError> {
    let capability = request.kind();
    let audit = &services.audit;

    let decision = services.policy.evaluate(&request);
    let Some(hook_policy) = HookPolicyOutcome::from_policy(&decision) else {
        let PolicyDecision::Deny { reason } = decision else {
            unreachable!("only denied policy decisions have no hook outcome")
        };
        audit.record(capability, ApprovalAuditDecision::DeniedByPolicy);
        return Err(AuthorizationError::denied(
            AuthorizationDenialKind::Policy,
            capability,
            reason,
        ));
    };
    match decision {
        PolicyDecision::Deny { reason } => {
            unreachable!("denied policy decision handled before hook dispatch: {reason}")
        }
        PolicyDecision::Allow => {
            deny_if_hooked(services, &request, hook_policy, tool_call_id).await?;
            Ok(AuthorizationOutcome::AllowedByPolicy)
        }
        PolicyDecision::RequireApproval { reason } => {
            deny_if_hooked(services, &request, hook_policy, tool_call_id).await?;
            prompt_for_approval(
                services,
                request,
                capability,
                reason,
                tool_call_id,
                cancellation,
            )
            .await
        }
    }
}

async fn deny_if_hooked(
    services: &AuthorizationServices,
    request: &CapabilityRequest,
    policy: HookPolicyOutcome,
    tool_call_id: Option<&crate::ToolCallId>,
) -> Result<(), AuthorizationError> {
    match consult_pre_tool_gate(services, request, policy, tool_call_id).await {
        HookDecision::Continue => Ok(()),
        HookDecision::Deny { reason } => {
            services
                .audit
                .record(request.kind(), ApprovalAuditDecision::DeniedByHook);
            Err(AuthorizationError::denied(
                AuthorizationDenialKind::Hook,
                request.kind(),
                reason,
            ))
        }
    }
}

async fn prompt_for_approval(
    services: &AuthorizationServices,
    request: CapabilityRequest,
    capability: super::CapabilityKind,
    reason: String,
    tool_call_id: Option<&crate::ToolCallId>,
    cancellation: crate::CancellationToken,
) -> Result<AuthorizationOutcome, AuthorizationError> {
    let remembered = &services.remembered;
    let audit = &services.audit;

    if remembered.contains(&request) {
        audit.record(
            capability,
            ApprovalAuditDecision::AllowedByRememberedApproval,
        );
        return Ok(AuthorizationOutcome::AllowedByRememberedApproval);
    }

    // Serialize only the miss path so a concurrent identical waiter
    // observes AllowForSession recorded by the first prompt. Remembered
    // hits above stay concurrent. AllowOnce/Deny still re-prompt after
    // the holder finishes because nothing is remembered for them.
    let _gate = remembered.approval_gate().lock().await;
    if remembered.contains(&request) {
        audit.record(
            capability,
            ApprovalAuditDecision::AllowedByRememberedApproval,
        );
        return Ok(AuthorizationOutcome::AllowedByRememberedApproval);
    }
    let context = services.approval_context(cancellation);
    match services
        .approvals
        .request_with_context(
            ApprovalRequest::new(request.clone(), reason).with_tool_call_id(tool_call_id.cloned()),
            context,
        )
        .await
    {
        ApprovalDecision::AllowOnce => {
            audit.record(capability, ApprovalAuditDecision::AllowedOnce);
            Ok(AuthorizationOutcome::AllowedOnce)
        }
        ApprovalDecision::AllowForSession => {
            remembered.remember(request);
            audit.record(capability, ApprovalAuditDecision::AllowedForSession);
            Ok(AuthorizationOutcome::AllowedForSession)
        }
        ApprovalDecision::Deny { reason } => {
            audit.record(capability, ApprovalAuditDecision::DeniedByHost);
            Err(AuthorizationError::denied(
                AuthorizationDenialKind::Host,
                capability,
                reason,
            ))
        }
    }
}

/// Builds a `before_tool_use` envelope only when the installed gate may care.
async fn consult_pre_tool_gate(
    services: &AuthorizationServices,
    request: &CapabilityRequest,
    policy: HookPolicyOutcome,
    tool_call_id: Option<&crate::ToolCallId>,
) -> HookDecision {
    let hooks = &services.hooks;
    let Some(gate) = hooks.gate() else {
        return HookDecision::Continue;
    };

    let tool_name = HookTool::source_name(request.source());
    if !gate.applies_to_tool(tool_name) {
        return HookDecision::Continue;
    }

    let scope = &services.scope;
    let mut builder = hooks.builder(
        scope.session_id.as_ref(),
        scope.run_id.as_ref(),
        scope.workspace_root(),
    );
    let tool = HookTool::from_source(
        request.source(),
        tool_call_id.map(|id| id.as_str().to_owned()),
        hooks.bounds(),
        builder.truncation(),
    );
    let capability = summarize_capability(request, hooks.bounds(), builder.truncation());
    let payload = HookPayload::BeforeToolUse(BeforeToolUsePayload {
        tool,
        capability,
        policy,
    });
    hooks
        .evaluate_pre_tool_use(PreToolUseRequest::new(builder.finish(payload), policy))
        .await
}
