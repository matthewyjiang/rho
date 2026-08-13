use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, RwLock,
    },
};

use rho_sdk::{
    ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    ProviderRequestUsageRecording,
};

use crate::{
    config::Config,
    permission::{SessionWriteLog, WriteAuthority},
    permission_classifier::{classify_capability_request, ClassifierVerdict, ClassifyRequest},
};

/// Consecutive classifier denials before Auto escalates to a human (or cancels
/// headless). Lives next to the streak counter that enforces it.
pub(crate) const CONSECUTIVE_DENY_ESCALATION: u32 = 3;

/// Total classifier denials in one run before Auto escalates to a human (or
/// cancels headless). Like the consecutive limit, it stops a runaway loop: an
/// agent that keeps probing around denials never grinds on forever, even when
/// occasional allows break the streak.
pub(crate) const TOTAL_DENY_ESCALATION: u32 = 20;

type ClassifyFuture = Pin<Box<dyn Future<Output = ClassifierVerdict> + Send>>;
pub(crate) type ClassifyFn =
    Arc<dyn Fn(ClassificationInput) -> ClassifyFuture + Send + Sync + 'static>;

pub(crate) struct ClassificationInput {
    pub(crate) config: Config,
    pub(crate) request: ApprovalRequest,
    pub(crate) workspace_path: PathBuf,
    pub(crate) usage_recording: ProviderRequestUsageRecording,
}

/// Approval handler that classifies Auto-mode capability requests.
///
/// History and cancellation come from [`ApprovalRequest::context`]. The only
/// mutable run state is the deny counters, which [`Self::isolate`] resets so
/// concurrent workflow agents do not share them.
pub(crate) struct ClassifierApprovalHandler {
    config: RwLock<Config>,
    workspace_path: PathBuf,
    usage_recording: ProviderRequestUsageRecording,
    classifier: ClassifyFn,
    inner: Option<Arc<dyn ApprovalHandler>>,
    session_writes: Option<SessionWriteLog>,
    consecutive_denials: AtomicU32,
    total_denials: AtomicU32,
}

impl ClassifierApprovalHandler {
    pub(crate) fn new(
        config: Config,
        workspace_path: PathBuf,
        usage_recording: ProviderRequestUsageRecording,
        inner: Option<Arc<dyn ApprovalHandler>>,
        session_writes: Option<SessionWriteLog>,
    ) -> Self {
        Self {
            config: RwLock::new(config),
            workspace_path,
            usage_recording,
            classifier: default_classifier(),
            inner,
            session_writes,
            consecutive_denials: AtomicU32::new(0),
            total_denials: AtomicU32::new(0),
        }
    }

    /// Shared Auto classifier over an optional human escalator.
    pub(crate) fn shared(
        config: Config,
        workspace_path: PathBuf,
        usage_recording: ProviderRequestUsageRecording,
        human: Option<Arc<dyn ApprovalHandler>>,
        session_writes: Option<SessionWriteLog>,
    ) -> Arc<Self> {
        Arc::new(Self::new(
            config,
            workspace_path,
            usage_recording,
            human,
            session_writes,
        ))
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        classifier: ClassifyFn,
        inner: Option<Arc<dyn ApprovalHandler>>,
    ) -> Self {
        Self {
            config: RwLock::new(Config::default()),
            workspace_path: PathBuf::from("/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
            classifier,
            inner,
            session_writes: None,
            consecutive_denials: AtomicU32::new(0),
            total_denials: AtomicU32::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_session_writes(mut self, session_writes: SessionWriteLog) -> Self {
        self.session_writes = Some(session_writes);
        self
    }

    pub(crate) fn update_config(&self, config: Config) {
        *self
            .config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }

    /// Clones classifier config for an isolated agent/command run.
    ///
    /// The deny counters reset so concurrent workflow nodes cannot escalate each
    /// other. History and cancellation stay request-scoped via
    /// [`ApprovalRequest::context`]. Remembered writes stay on this handler's
    /// log; distinct runs should use [`Self::isolate_for_run`] so they record
    /// into the log their workspace policy consults.
    pub(crate) fn isolate(self: &Arc<Self>) -> Arc<Self> {
        self.clone_with_reset_streak(self.session_writes.clone())
    }

    /// Isolates a template onto a distinct run's write log.
    ///
    /// The deny streak still resets. Classifier allows and human escalations
    /// record into `session_writes` instead of the template's log, which is
    /// often absent or owned by a different session.
    pub(crate) fn isolate_for_run(self: &Arc<Self>, session_writes: SessionWriteLog) -> Arc<Self> {
        self.clone_with_reset_streak(Some(session_writes))
    }

    fn clone_with_reset_streak(
        self: &Arc<Self>,
        session_writes: Option<SessionWriteLog>,
    ) -> Arc<Self> {
        let config = self
            .config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        Arc::new(Self {
            config: RwLock::new(config),
            workspace_path: self.workspace_path.clone(),
            usage_recording: self.usage_recording.clone(),
            classifier: Arc::clone(&self.classifier),
            inner: self.inner.clone(),
            session_writes,
            consecutive_denials: AtomicU32::new(0),
            total_denials: AtomicU32::new(0),
        })
    }

    fn input_for(&self, request: ApprovalRequest) -> ClassificationInput {
        let config = self
            .config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        ClassificationInput {
            config,
            request,
            workspace_path: self.workspace_path.clone(),
            usage_recording: self.usage_recording.clone(),
        }
    }

    /// True once either deny budget is spent.
    fn should_escalate(&self) -> bool {
        self.consecutive_denials.load(Ordering::Relaxed) >= CONSECUTIVE_DENY_ESCALATION
            || self.total_denials.load(Ordering::Relaxed) >= TOTAL_DENY_ESCALATION
    }

    async fn escalate_or_deny_headless(&self, request: ApprovalRequest) -> ApprovalDecision {
        let Some(inner) = &self.inner else {
            request.context().cancellation().cancel();
            return ApprovalDecision::Deny {
                reason: format!(
                    "permission classifier denied {CONSECUTIVE_DENY_ESCALATION} consecutive or {TOTAL_DENY_ESCALATION} total requests and no human approval handler is available"
                ),
            };
        };
        let capability = request.capability().clone();
        let decision = inner.request(request).await;
        if matches!(
            decision,
            ApprovalDecision::AllowOnce | ApprovalDecision::AllowForSession
        ) {
            if let Some(writes) = &self.session_writes {
                writes.remember(&capability, WriteAuthority::Human);
            }
        }
        decision
    }
}

impl ApprovalHandler for ClassifierApprovalHandler {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            if self.should_escalate() {
                let decision = self.escalate_or_deny_headless(request).await;
                if self.inner.is_some() {
                    self.consecutive_denials.store(0, Ordering::Relaxed);
                    self.total_denials.store(0, Ordering::Relaxed);
                }
                return decision;
            }

            let capability = request.capability().clone();
            let verdict = (self.classifier)(self.input_for(request)).await;
            match verdict {
                ClassifierVerdict::Allow => {
                    self.consecutive_denials.store(0, Ordering::Relaxed);
                    if let Some(writes) = &self.session_writes {
                        writes.remember(&capability, WriteAuthority::Classifier);
                    }
                    ApprovalDecision::AllowOnce
                }
                ClassifierVerdict::Deny { reason } => {
                    self.consecutive_denials.fetch_add(1, Ordering::Relaxed);
                    self.total_denials.fetch_add(1, Ordering::Relaxed);
                    ApprovalDecision::Deny {
                        reason: deny_and_continue_reason(reason),
                    }
                }
            }
        })
    }

    fn reads_live_history(&self) -> bool {
        true
    }
}

fn default_classifier() -> ClassifyFn {
    Arc::new(|input: ClassificationInput| {
        Box::pin(async move {
            let context = input.request.context();
            classify_capability_request(
                &input.config,
                ClassifyRequest {
                    history: context.history(),
                    pending: &input.request,
                    cancellation: context.cancellation().clone(),
                    session_id: context.session_id(),
                    workspace_path: &input.workspace_path,
                    usage_recording: input.usage_recording,
                },
            )
            .await
        })
    })
}

fn deny_and_continue_reason(reason: impl std::fmt::Display) -> String {
    format!(
        "permission classifier denied this request: {reason}; find a safer path; do not route around this block"
    )
}

#[cfg(test)]
#[path = "permission_classifier_handler_tests.rs"]
mod tests;
