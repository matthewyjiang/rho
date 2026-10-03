use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, RwLock},
};

use rho_sdk::{
    ApprovalConcurrency, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
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
    /// Shared so the handler gets the request back to escalate it when the
    /// verdict lands after the deny budget is spent.
    pub(crate) request: Arc<ApprovalRequest>,
    pub(crate) workspace_path: PathBuf,
    pub(crate) usage_recording: ProviderRequestUsageRecording,
}

/// Approval handler that classifies Auto-mode capability requests.
///
/// History and cancellation come from [`ApprovalRequest::context`]. The only
/// mutable run state is the deny budget, which [`Self::isolate`] resets so
/// concurrent workflow agents do not share it.
///
/// Requests arrive concurrently ([`ApprovalConcurrency::Concurrent`]), so
/// parallel tool calls are classified at once instead of queueing behind each
/// other's review. Verdicts settle into the budget in completion order, as if
/// the requests had run one after another in that order, and escalations to
/// the human go one at a time.
pub(crate) struct ClassifierApprovalHandler {
    config: RwLock<Config>,
    workspace_path: PathBuf,
    usage_recording: ProviderRequestUsageRecording,
    classifier: ClassifyFn,
    inner: Option<Arc<dyn ApprovalHandler>>,
    session_writes: Option<SessionWriteLog>,
    budget: Mutex<DenyBudget>,
    /// Held while a request is escalated, so the human sees one prompt at a
    /// time and a waiter can see the budget that answer reset.
    escalation: tokio::sync::Mutex<()>,
}

/// Classifier denials counted toward escalating to a human (or cancelling
/// headless).
#[derive(Default)]
struct DenyBudget {
    consecutive: u32,
    total: u32,
}

impl DenyBudget {
    /// True once either limit is reached. Stays true until a human answers,
    /// because only a recorded allow resets the streak and [`Self::settle`]
    /// records nothing once spent.
    fn is_spent(&self) -> bool {
        self.consecutive >= CONSECUTIVE_DENY_ESCALATION || self.total >= TOTAL_DENY_ESCALATION
    }

    /// Records `verdict`, or returns false when the budget is already spent
    /// and the request that produced it must escalate instead.
    fn settle(&mut self, verdict: &ClassifierVerdict) -> bool {
        if self.is_spent() {
            return false;
        }
        match verdict {
            ClassifierVerdict::Allow => self.consecutive = 0,
            ClassifierVerdict::Deny { .. } => {
                self.consecutive += 1;
                self.total += 1;
            }
        }
        true
    }
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
            budget: Mutex::default(),
            escalation: tokio::sync::Mutex::default(),
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
            budget: Mutex::default(),
            escalation: tokio::sync::Mutex::default(),
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
            budget: Mutex::default(),
            escalation: tokio::sync::Mutex::default(),
        })
    }

    fn input_for(&self, request: Arc<ApprovalRequest>) -> ClassificationInput {
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

    fn budget(&self) -> MutexGuard<'_, DenyBudget> {
        self.budget
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Escalates `request` if the budget is still spent once it is this
    /// request's turn to escalate. Gives the request back when a human answer
    /// reset the budget while it waited.
    async fn escalate_while_spent(
        &self,
        request: ApprovalRequest,
    ) -> Result<ApprovalDecision, ApprovalRequest> {
        let _escalation = self.escalation.lock().await;
        if !self.budget().is_spent() {
            return Err(request);
        }
        let decision = self.escalate_or_deny_headless(request).await;
        if self.inner.is_some() {
            *self.budget() = DenyBudget::default();
        }
        Ok(decision)
    }

    /// The decision for a verdict the budget recorded.
    fn decide(&self, verdict: ClassifierVerdict, request: &ApprovalRequest) -> ApprovalDecision {
        match verdict {
            ClassifierVerdict::Allow => {
                if let Some(writes) = &self.session_writes {
                    writes.remember(request.capability(), WriteAuthority::Classifier);
                }
                ApprovalDecision::AllowOnce
            }
            ClassifierVerdict::Deny { reason } => ApprovalDecision::Deny {
                reason: deny_and_continue_reason(reason),
            },
        }
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
            let mut request = request;
            // A request that arrives after the budget is spent skips the
            // classifier.
            if self.budget().is_spent() {
                match self.escalate_while_spent(request).await {
                    Ok(decision) => return decision,
                    Err(returned) => request = returned,
                }
            }

            let shared = Arc::new(request);
            let verdict = (self.classifier)(self.input_for(Arc::clone(&shared))).await;
            let mut request = Arc::try_unwrap(shared).unwrap_or_else(|shared| (*shared).clone());
            // A verdict that lands after concurrent denials spent the budget
            // escalates like a request that arrives after them, so a late
            // allow cannot reset a streak that was already due.
            loop {
                if self.budget().settle(&verdict) {
                    return self.decide(verdict, &request);
                }
                match self.escalate_while_spent(request).await {
                    Ok(decision) => return decision,
                    Err(returned) => request = returned,
                }
            }
        })
    }

    fn reads_live_history(&self) -> bool {
        true
    }

    fn concurrency(&self) -> ApprovalConcurrency {
        ApprovalConcurrency::Concurrent
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
