use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::{Arc, Mutex},
};

use tokio::sync::{mpsc, oneshot};

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
    #[cfg(test)]
    pub(crate) fn new(capability: CapabilityRequest, reason: impl Into<String>) -> Self {
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

#[derive(Debug, Default)]
pub(crate) struct SessionApprovals {
    exact_requests: Mutex<Vec<CapabilityRequest>>,
    /// Serializes approval evaluation so an identical concurrent request observes
    /// a session approval the first request recorded instead of prompting again.
    /// ponytail: one gate serializes all approvals, not just identical ones — a
    /// hung host approval blocks unrelated ones. Key the gate by request if a
    /// host needs distinct approvals to prompt concurrently.
    approval_gate: tokio::sync::Mutex<()>,
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

#[cfg(test)]
pub(crate) async fn authorize(
    policy: &Arc<dyn WorkspacePolicy>,
    approvals: &Arc<dyn ApprovalHandler>,
    remembered: &Arc<SessionApprovals>,
    audit: &Arc<ApprovalAuditLog>,
    request: CapabilityRequest,
) -> Result<AuthorizationOutcome, AuthorizationError> {
    authorize_for_call(policy, approvals, remembered, audit, request, None).await
}

pub(crate) async fn authorize_for_call(
    policy: &Arc<dyn WorkspacePolicy>,
    approvals: &Arc<dyn ApprovalHandler>,
    remembered: &Arc<SessionApprovals>,
    audit: &Arc<ApprovalAuditLog>,
    request: CapabilityRequest,
    tool_call_id: Option<&crate::ToolCallId>,
) -> Result<AuthorizationOutcome, AuthorizationError> {
    let capability = request.kind();
    match policy.evaluate(&request) {
        PolicyDecision::Allow => Ok(AuthorizationOutcome::AllowedByPolicy),
        PolicyDecision::Deny { reason } => {
            audit.record(capability, ApprovalAuditDecision::DeniedByPolicy);
            Err(AuthorizationError::denied(
                AuthorizationDenialKind::Policy,
                capability,
                reason,
            ))
        }
        PolicyDecision::RequireApproval { reason } => {
            let _gate = remembered.approval_gate.lock().await;
            if remembered.contains(&request) {
                audit.record(
                    capability,
                    ApprovalAuditDecision::AllowedByRememberedApproval,
                );
                return Ok(AuthorizationOutcome::AllowedByRememberedApproval);
            }
            match approvals
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
    }
}

impl fmt::Debug for dyn ApprovalHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApprovalHandler(..)")
    }
}
