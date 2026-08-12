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
    permission_classifier::{classify_capability_request, ClassifierVerdict, ClassifyRequest},
};

/// Consecutive classifier denials before Auto escalates to a human (or cancels
/// headless). Lives next to the streak counter that enforces it.
pub(crate) const CONSECUTIVE_DENY_ESCALATION: u32 = 3;

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
/// mutable run state is the consecutive-deny streak, which [`Self::isolate`]
/// resets so concurrent workflow agents do not share it.
pub(crate) struct ClassifierApprovalHandler {
    config: RwLock<Config>,
    workspace_path: PathBuf,
    usage_recording: ProviderRequestUsageRecording,
    classifier: ClassifyFn,
    inner: Option<Arc<dyn ApprovalHandler>>,
    consecutive_denials: AtomicU32,
}

impl ClassifierApprovalHandler {
    pub(crate) fn new(
        config: Config,
        workspace_path: PathBuf,
        usage_recording: ProviderRequestUsageRecording,
        inner: Option<Arc<dyn ApprovalHandler>>,
    ) -> Self {
        Self {
            config: RwLock::new(config),
            workspace_path,
            usage_recording,
            classifier: default_classifier(),
            inner,
            consecutive_denials: AtomicU32::new(0),
        }
    }

    /// Shared Auto classifier over an optional human escalator.
    pub(crate) fn shared(
        config: Config,
        workspace_path: PathBuf,
        usage_recording: ProviderRequestUsageRecording,
        human: Option<Arc<dyn ApprovalHandler>>,
    ) -> Arc<Self> {
        Arc::new(Self::new(config, workspace_path, usage_recording, human))
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
            consecutive_denials: AtomicU32::new(0),
        }
    }

    pub(crate) fn update_config(&self, config: Config) {
        *self
            .config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }

    /// Clones classifier config for an isolated agent/command run.
    ///
    /// The deny streak resets so concurrent workflow nodes cannot escalate each
    /// other. History and cancellation stay request-scoped via
    /// [`ApprovalRequest::context`].
    pub(crate) fn isolate(self: &Arc<Self>) -> Arc<Self> {
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
            consecutive_denials: AtomicU32::new(0),
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

    async fn escalate_or_deny_headless(&self, request: ApprovalRequest) -> ApprovalDecision {
        let Some(inner) = &self.inner else {
            request.context().cancellation().cancel();
            return ApprovalDecision::Deny {
                reason: format!(
                    "permission classifier denied {CONSECUTIVE_DENY_ESCALATION} consecutive requests and no human approval handler is available"
                ),
            };
        };
        inner.request(request).await
    }
}

impl ApprovalHandler for ClassifierApprovalHandler {
    fn request<'a>(&'a self, request: ApprovalRequest) -> ApprovalFuture<'a> {
        Box::pin(async move {
            let streak = self.consecutive_denials.load(Ordering::Relaxed);
            if streak >= CONSECUTIVE_DENY_ESCALATION {
                let decision = self.escalate_or_deny_headless(request).await;
                if self.inner.is_some() {
                    self.consecutive_denials.store(0, Ordering::Relaxed);
                }
                return decision;
            }

            let verdict = (self.classifier)(self.input_for(request)).await;
            match verdict {
                ClassifierVerdict::Allow => {
                    self.consecutive_denials.store(0, Ordering::Relaxed);
                    ApprovalDecision::AllowOnce
                }
                ClassifierVerdict::Deny { reason } => {
                    self.consecutive_denials.fetch_add(1, Ordering::Relaxed);
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
