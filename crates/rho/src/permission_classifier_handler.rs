use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
};

use rho_sdk::{
    model::Message, ApprovalDecision, ApprovalFuture, ApprovalHandler, ApprovalRequest,
    CancellationToken, ProviderRequestUsageRecording, Session, SessionId,
};

use crate::{
    config::Config,
    permission_classifier::{
        classify_capability_request, ClassifierVerdict, CONSECUTIVE_DENY_ESCALATION,
    },
};

type ClassifyFuture = Pin<Box<dyn Future<Output = anyhow::Result<ClassifierVerdict>> + Send>>;
pub(crate) type ClassifyFn =
    Arc<dyn Fn(ClassificationInput) -> ClassifyFuture + Send + Sync + 'static>;

pub(crate) struct ClassificationInput {
    pub(crate) config: Config,
    pub(crate) history: Vec<Message>,
    pub(crate) request: ApprovalRequest,
    pub(crate) session_id: SessionId,
    pub(crate) workspace_path: PathBuf,
    pub(crate) usage_recording: ProviderRequestUsageRecording,
}

pub(crate) struct ClassifierApprovalHandler {
    config: RwLock<Config>,
    session: Mutex<Option<Session>>,
    workspace_path: PathBuf,
    usage_recording: ProviderRequestUsageRecording,
    classifier: ClassifyFn,
    inner: Option<Arc<dyn ApprovalHandler>>,
    cancellation: Mutex<Option<CancellationToken>>,
    consecutive_denials: tokio::sync::Mutex<u32>,
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
            session: Mutex::new(None),
            workspace_path,
            usage_recording,
            classifier: default_classifier(),
            inner,
            cancellation: Mutex::new(None),
            consecutive_denials: tokio::sync::Mutex::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(
        classifier: ClassifyFn,
        inner: Option<Arc<dyn ApprovalHandler>>,
    ) -> Self {
        Self {
            config: RwLock::new(Config::default()),
            session: Mutex::new(None),
            workspace_path: PathBuf::from("/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
            classifier,
            inner,
            cancellation: Mutex::new(None),
            consecutive_denials: tokio::sync::Mutex::new(0),
        }
    }

    pub(crate) fn bind_session(&self, session: Session) {
        *self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session);
    }

    pub(crate) fn bind_cancellation(&self, cancellation: CancellationToken) {
        *self
            .cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(cancellation);
    }

    pub(crate) fn update_config(&self, config: Config) {
        *self
            .config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
    }

    fn input_for(&self, request: ApprovalRequest) -> ClassificationInput {
        let config = self
            .config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let session = self
            .session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let (history, session_id) = session.map_or_else(
            || (Vec::new(), SessionId::new()),
            |session| (session.live_history(), session.id().clone()),
        );
        ClassificationInput {
            config,
            history,
            request,
            session_id,
            workspace_path: self.workspace_path.clone(),
            usage_recording: self.usage_recording.clone(),
        }
    }

    async fn escalate_or_deny_headless(&self, request: ApprovalRequest) -> ApprovalDecision {
        let Some(inner) = &self.inner else {
            if let Some(cancellation) = self
                .cancellation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
            {
                cancellation.cancel();
            }
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
            let mut consecutive_denials = self.consecutive_denials.lock().await;
            if *consecutive_denials >= CONSECUTIVE_DENY_ESCALATION {
                let decision = self.escalate_or_deny_headless(request).await;
                if self.inner.is_some() {
                    *consecutive_denials = 0;
                }
                return decision;
            }

            let verdict = (self.classifier)(self.input_for(request)).await;
            match verdict {
                Ok(ClassifierVerdict::Allow) => {
                    *consecutive_denials = 0;
                    ApprovalDecision::AllowOnce
                }
                Ok(ClassifierVerdict::Deny { reason }) => {
                    *consecutive_denials += 1;
                    ApprovalDecision::Deny {
                        reason: deny_and_continue_reason(reason),
                    }
                }
                Err(error) => {
                    *consecutive_denials += 1;
                    ApprovalDecision::Deny {
                        reason: deny_and_continue_reason(format!(
                            "classifier unavailable: {error}"
                        )),
                    }
                }
            }
        })
    }
}

fn default_classifier() -> ClassifyFn {
    Arc::new(|input: ClassificationInput| {
        Box::pin(async move {
            Ok(classify_capability_request(
                &input.config,
                &input.history,
                &input.request,
                CancellationToken::new(),
                &input.session_id,
                &input.workspace_path,
                input.usage_recording,
            )
            .await)
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
