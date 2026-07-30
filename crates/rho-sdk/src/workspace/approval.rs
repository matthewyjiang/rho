use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    num::NonZeroUsize,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};

use tokio::sync::{mpsc, oneshot};

use crate::hooks::{
    capability_label, summarize_capability, BeforeToolUsePayload, HookDecision, HookEventKind,
    HookPayload, HookPolicyOutcome, HookRuntime, HookTool, PreToolUseRequest,
};

use super::{CapabilityKind, CapabilityRequest, PolicyDecision, WorkspacePolicy};

/// Host decision for one approval request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApprovalDecision {
    AllowOnce,
    /// Remember only this exact structured request for the current session.
    AllowForSession,
    Deny {
        reason: String,
    },
}

/// Owned request supplied to an [`ApprovalHandler`].
#[derive(Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    capability: CapabilityRequest,
    reason: String,
    tool_call_id: Option<crate::ToolCallId>,
}

impl ApprovalRequest {
    pub fn new(capability: CapabilityRequest, reason: impl Into<String>) -> Self {
        Self {
            capability,
            reason: reason.into(),
            tool_call_id: None,
        }
    }

    pub fn capability(&self) -> &CapabilityRequest {
        &self.capability
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Identifies the tool call that requested approval during a run.
    pub fn tool_call_id(&self) -> Option<&crate::ToolCallId> {
        self.tool_call_id.as_ref()
    }
}

impl fmt::Debug for ApprovalRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApprovalRequest")
            .field("capability_kind", &self.capability.kind())
            .field("source", self.capability.source())
            .field("correlated_tool_call", &self.tool_call_id.is_some())
            .field("details", &"available through accessors")
            .field("reason", &"[redacted]")
            .finish()
    }
}

/// Future returned by approval handlers.
pub type ApprovalFuture<'a> = Pin<Box<dyn Future<Output = ApprovalDecision> + Send + 'a>>;

/// Host extension point for interactive or remote approval decisions.
pub trait ApprovalHandler: Send + Sync {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a>;
}

/// Approval handler that denies every request.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyApprovals;

impl ApprovalHandler for DenyApprovals {
    fn request<'a>(&'a self, _request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async {
            ApprovalDecision::Deny {
                reason: "no approval handler is configured".into(),
            }
        })
    }
}

/// Cloneable approval handler backed by a bounded host request channel.
#[derive(Clone, Debug)]
pub struct ChannelApprovalHandler {
    sender: mpsc::Sender<PendingApproval>,
}

impl ChannelApprovalHandler {
    #[cfg(test)]
    pub(crate) async fn wait_until_full(&self) {
        while self.sender.capacity() > 0 {
            tokio::task::yield_now().await;
        }
    }
}

impl ApprovalHandler for ChannelApprovalHandler {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            let (response, receiver) = oneshot::channel();
            let pending = PendingApproval {
                request,
                response: Some(response),
            };
            if self.sender.send(pending).await.is_err() {
                return ApprovalDecision::Deny {
                    reason: "approval request receiver was dropped".into(),
                };
            }
            receiver.await.unwrap_or_else(|_| ApprovalDecision::Deny {
                reason: "approval responder was dropped".into(),
            })
        })
    }
}

/// Receiving side of a bounded approval request channel.
#[derive(Debug)]
pub struct ApprovalRequestReceiver {
    receiver: mpsc::Receiver<PendingApproval>,
}

impl ApprovalRequestReceiver {
    pub async fn recv(&mut self) -> Option<PendingApproval> {
        while let Some(pending) = self.receiver.recv().await {
            if pending.is_live() {
                return Some(pending);
            }
        }
        None
    }
}

/// One pending approval with an exactly-once response slot.
#[derive(Debug)]
pub struct PendingApproval {
    request: ApprovalRequest,
    response: Option<oneshot::Sender<ApprovalDecision>>,
}

impl PendingApproval {
    /// Build a pending approval for hosts and tests that drive the prompt directly.
    pub fn new(request: ApprovalRequest) -> (Self, oneshot::Receiver<ApprovalDecision>) {
        let (response, receiver) = oneshot::channel();
        (
            Self {
                request,
                response: Some(response),
            },
            receiver,
        )
    }

    fn is_live(&self) -> bool {
        self.response
            .as_ref()
            .is_some_and(|response| !response.is_closed())
    }

    pub fn request(&self) -> &ApprovalRequest {
        &self.request
    }

    /// Completes the request. A second call returns the decision unchanged.
    pub fn respond(&mut self, decision: ApprovalDecision) -> Result<(), ApprovalDecision> {
        let Some(response) = self.response.take() else {
            return Err(decision);
        };
        response.send(decision)
    }
}

impl Drop for PendingApproval {
    fn drop(&mut self) {
        if let Some(response) = self.response.take() {
            let _ = response.send(ApprovalDecision::Deny {
                reason: "approval responder was dropped".into(),
            });
        }
    }
}

pub fn approval_channel(
    capacity: NonZeroUsize,
) -> (ChannelApprovalHandler, ApprovalRequestReceiver) {
    let (sender, receiver) = mpsc::channel(capacity.get());
    (
        ChannelApprovalHandler { sender },
        ApprovalRequestReceiver { receiver },
    )
}

/// Successful source of authorization returned to a tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthorizationOutcome {
    AllowedByPolicy,
    AllowedOnce,
    AllowedForSession,
    AllowedByRememberedApproval,
}

/// Typed source of an authorization denial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthorizationDenialKind {
    Policy,
    Host,
    Cancelled,
    /// A trusted pre-action hook narrowed the decision to a denial.
    Hook,
}

/// Typed authorization failure available to tool implementations and hosts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationError {
    kind: AuthorizationDenialKind,
    capability: CapabilityKind,
    message: String,
}

impl AuthorizationError {
    pub(crate) fn denied(
        kind: AuthorizationDenialKind,
        capability: CapabilityKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            capability,
            message: message.into(),
        }
    }

    pub(crate) fn cancelled(capability: CapabilityKind) -> Self {
        Self::denied(
            AuthorizationDenialKind::Cancelled,
            capability,
            "authorization cancelled",
        )
    }

    pub fn kind(&self) -> AuthorizationDenialKind {
        self.kind
    }

    pub fn capability(&self) -> CapabilityKind {
        self.capability
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AuthorizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "authorization denied: {}", self.message)
    }
}

impl std::error::Error for AuthorizationError {}

/// Secret-free approval decision retained in runtime diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApprovalAuditRecord {
    sequence: u64,
    capability: CapabilityKind,
    decision: ApprovalAuditDecision,
}

impl ApprovalAuditRecord {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn capability(&self) -> CapabilityKind {
        self.capability
    }

    pub fn decision(&self) -> ApprovalAuditDecision {
        self.decision
    }
}

/// Sanitized approval result. Reasons, paths, commands, URLs, and environment
/// values are intentionally not retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ApprovalAuditDecision {
    AllowedOnce,
    AllowedForSession,
    AllowedByRememberedApproval,
    DeniedByPolicy,
    DeniedByHost,
    Cancelled,
    /// A trusted pre-action hook denied the request before any prompt.
    DeniedByHook,
}

const MAX_AUDIT_RECORDS: usize = 1024;

#[derive(Debug, Default)]
pub(crate) struct ApprovalAuditLog {
    records: Mutex<VecDeque<ApprovalAuditRecord>>,
}

impl ApprovalAuditLog {
    pub(crate) fn record(&self, capability: CapabilityKind, decision: ApprovalAuditDecision) {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let sequence = records.back().map_or(1, |record| record.sequence + 1);
        if records.len() == MAX_AUDIT_RECORDS {
            records.pop_front();
        }
        records.push_back(ApprovalAuditRecord {
            sequence,
            capability,
            decision,
        });
    }

    pub(crate) fn snapshot(&self) -> Vec<ApprovalAuditRecord> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .copied()
            .collect()
    }
}

#[derive(Debug)]
pub(crate) struct SessionApprovals {
    exact_requests: Mutex<Vec<CapabilityRequest>>,
    /// Serializes the miss path of approval evaluation: check remembered, prompt
    /// the host if needed, then record `AllowForSession`. Waiters re-check under
    /// the gate so an identical concurrent request observes the first decision
    /// instead of prompting again.
    ///
    /// This is a session-wide prompt gate, not identical-only singleflight. A
    /// slow host approval orders unrelated `RequireApproval` misses behind it.
    /// Remembered hits stay outside the gate. Key by request if a host needs
    /// distinct misses to prompt concurrently.
    ///
    /// Wrapped in `AssertUnwindSafe` so embedding `tokio::sync::Mutex` does not
    /// strip `UnwindSafe` / `RefUnwindSafe` from public types that hold session
    /// state (`Session`). The gate only orders cooperative approval work.
    approval_gate: AssertUnwindSafe<tokio::sync::Mutex<()>>,
}

impl Default for SessionApprovals {
    fn default() -> Self {
        Self {
            exact_requests: Mutex::new(Vec::new()),
            approval_gate: AssertUnwindSafe(tokio::sync::Mutex::new(())),
        }
    }
}

impl SessionApprovals {
    fn contains(&self, request: &CapabilityRequest) -> bool {
        self.exact_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(request)
    }

    fn remember(&self, request: CapabilityRequest) {
        let mut requests = self
            .exact_requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !requests.contains(&request) {
            requests.push(request);
        }
    }
}

/// Where one authorization happened, for hook envelope identity.
#[derive(Clone, Debug, Default)]
pub(crate) struct AuthorizationScope {
    pub(crate) session_id: Option<crate::SessionId>,
    pub(crate) run_id: Option<crate::RunId>,
    pub(crate) workspace_root: Option<PathBuf>,
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
    hooks: HookRuntime,
    scope: AuthorizationScope,
}

impl AuthorizationServices {
    pub(crate) fn new(
        policy: Arc<dyn WorkspacePolicy>,
        approvals: Arc<dyn ApprovalHandler>,
        remembered: Arc<SessionApprovals>,
        audit: Arc<ApprovalAuditLog>,
        hooks: HookRuntime,
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
            HookRuntime::default(),
            AuthorizationScope::default(),
        )
    }

    pub(crate) fn audit(&self) -> &Arc<ApprovalAuditLog> {
        &self.audit
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
    authorize_for_call(services, request, None).await
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
) -> Result<AuthorizationOutcome, AuthorizationError> {
    let capability = request.kind();
    let audit = &services.audit;
    let decision = services.policy.evaluate(&request);
    let Some(policy_outcome) = HookPolicyOutcome::from_policy(&decision) else {
        let PolicyDecision::Deny { reason } = decision else {
            unreachable!("only a policy denial has no hook outcome")
        };
        audit.record(capability, ApprovalAuditDecision::DeniedByPolicy);
        return Err(AuthorizationError::denied(
            AuthorizationDenialKind::Policy,
            capability,
            reason,
        ));
    };

    if let HookDecision::Deny { reason } =
        consult_pre_tool_gate(services, &request, policy_outcome, tool_call_id).await
    {
        audit.record(capability, ApprovalAuditDecision::DeniedByHook);
        return Err(AuthorizationError::denied(
            AuthorizationDenialKind::Hook,
            capability,
            reason,
        ));
    }

    let reason = match decision {
        PolicyDecision::Allow => return Ok(AuthorizationOutcome::AllowedByPolicy),
        PolicyDecision::RequireApproval { reason } => reason,
        PolicyDecision::Deny { .. } => unreachable!("policy denial returned above"),
    };

    let remembered = &services.remembered;
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
    let _gate = remembered.approval_gate.lock().await;
    if remembered.contains(&request) {
        audit.record(
            capability,
            ApprovalAuditDecision::AllowedByRememberedApproval,
        );
        return Ok(AuthorizationOutcome::AllowedByRememberedApproval);
    }
    match services
        .approvals
        .request(ApprovalRequest {
            capability: request.clone(),
            reason,
            tool_call_id: tool_call_id.cloned(),
        })
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

async fn consult_pre_tool_gate(
    services: &AuthorizationServices,
    request: &CapabilityRequest,
    policy: HookPolicyOutcome,
    tool_call_id: Option<&crate::ToolCallId>,
) -> HookDecision {
    let hooks = &services.hooks;
    if hooks.gate().is_none() {
        return HookDecision::Continue;
    }
    let scope = &services.scope;
    let mut builder = hooks.builder(
        HookEventKind::BeforeToolUse,
        scope.session_id.as_ref(),
        scope.run_id.as_ref(),
        scope.workspace_root(),
    );
    let bounds = hooks.bounds();
    let capability = summarize_capability(request, bounds, builder.truncation());
    let payload = HookPayload::BeforeToolUse(BeforeToolUsePayload {
        tool: HookTool::from_source(
            request.source(),
            tool_call_id.map(|id| id.as_str().to_owned()),
        ),
        capability_kind: capability_label(request.kind()).to_owned(),
        capability,
        policy,
    });
    hooks
        .evaluate_pre_tool_use(PreToolUseRequest::new(builder.finish(payload), policy))
        .await
}

impl fmt::Debug for dyn ApprovalHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApprovalHandler(..)")
    }
}
