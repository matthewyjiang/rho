//! Who answers a session's approval requests: its host, or nobody.
//!
//! A host that can ask a person (ACP) answers them as the turn's
//! [`super::SessionEvent::ApprovalRequested`] and, leaving plan mode,
//! [`super::SessionEvent::PlanApprovalRequested`]. A headless host (`rho
//! run`, Rho subagents, workflow nodes) has nobody to ask, so its session
//! decides alone, as automation always has: Auto's classifier decides with
//! no human to escalate to, other modes answer through what a workflow or
//! parent passed down or deny, and the session gets no plan exit. Neither
//! request then ever reaches the host.

use std::sync::Arc;

use rho_sdk::ApprovalSession;

use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    app::{
        interactive_runtime::startup::{
            approval_channel_for, ApprovalChannel, ApprovalChannelOptions,
        },
        session_assembly::{ApprovalInputs, SessionApproval},
    },
    config::Config,
    permission::PermissionMode,
    permission_classifier_handler::ClassifierApprovalHandler,
};

/// Who answers a session's approval requests. Kept with the session's
/// build, so a permission mode change wires the new mode the same way.
#[derive(Clone)]
pub(crate) enum ApprovalPolicy {
    /// The host answers every request a person would. Auto's classifier
    /// escalates to it.
    Host,
    /// Nobody answers: no request reaches the host.
    Unattended(UnattendedApprovals),
}

/// What answers an unattended session's requests besides itself.
#[derive(Clone)]
pub(crate) enum UnattendedApprovals {
    /// Nothing: Auto runs its own classifier, and other modes deny every
    /// request.
    Alone,
    /// A workflow run's approval session answers outside Auto. Auto still
    /// runs its own classifier.
    Inherited(ApprovalSession),
    /// A parent's classifier template decides in Auto, isolated onto this
    /// session's write grants so its deny streak stays this session's.
    /// Other modes deny every request.
    ClassifierTemplate(Arc<ClassifierApprovalHandler>),
}

impl ApprovalPolicy {
    /// The approval wiring of one runtime build in `inputs.config`'s mode.
    pub(super) fn wire(&self, inputs: ApprovalInputs) -> SessionApproval {
        match self {
            Self::Host => {
                let ApprovalChannel {
                    handler, receiver, ..
                } = approval_channel_for(
                    inputs.config.permission_mode,
                    ApprovalChannelOptions {
                        config: inputs.config,
                        workspace_path: inputs.workspace_root,
                        usage_recording: inputs.usage_recording,
                        session_writes: inputs.session_writes,
                        credentials: inputs.credentials,
                    },
                );
                SessionApproval {
                    session: handler.map(ApprovalSession::from_shared),
                    receiver,
                }
            }
            // Without a receiver the turn has no approval to show; without
            // a session the SDK denies every request.
            Self::Unattended(approvals) => SessionApproval {
                session: approvals.session(inputs),
                receiver: None,
            },
        }
    }

    /// Whether the session may offer `exit_plan_mode`, whose approval only
    /// a host can answer.
    pub(super) fn offers_plan_exit(&self) -> bool {
        match self {
            Self::Host => true,
            Self::Unattended(_) => false,
        }
    }
}

impl UnattendedApprovals {
    fn session(&self, inputs: ApprovalInputs) -> Option<ApprovalSession> {
        match inputs.config.permission_mode {
            PermissionMode::Auto => Some(ApprovalSession::from_shared(self.classifier(inputs))),
            PermissionMode::Supervised
            | PermissionMode::AllowEdits
            | PermissionMode::Bypass
            | PermissionMode::Plan => match self {
                Self::Inherited(session) => Some(session.clone()),
                Self::Alone | Self::ClassifierTemplate(_) => None,
            },
        }
    }

    /// Auto's classifier for one build: the parent's template isolated
    /// onto this session's write grants, else a fresh one with no human to
    /// escalate to.
    fn classifier(&self, inputs: ApprovalInputs) -> Arc<ClassifierApprovalHandler> {
        match self {
            Self::ClassifierTemplate(template) => template.isolate_for_run(inputs.session_writes),
            Self::Alone | Self::Inherited(_) => ClassifierApprovalHandler::shared(
                inputs.config,
                inputs.workspace_root,
                inputs.usage_recording,
                inputs.credentials,
                /*human*/ None,
                Some(inputs.session_writes),
            ),
        }
    }
}

/// Fails when `config`'s mode is Auto and the classifier it needs cannot
/// run: no permission-classifier model, or an invalid screen. Every
/// session build checks it, since an unattended Auto session has nobody to
/// fall back to, and an attended one should not start half-wired.
pub(crate) fn ensure_auto_classifier_model(
    config: &Config,
    credentials: &dyn rho_providers::credentials::CredentialStore,
) -> anyhow::Result<()> {
    if config.permission_mode != PermissionMode::Auto {
        return Ok(());
    }
    if config
        .internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)
        .is_none()
    {
        anyhow::bail!(
            "permission mode auto requires a configured permission-classifier model (set via /config or config.toml [internal_agents.permission-classifier])"
        );
    }
    crate::permission_classifier::check_screen_config(config, credentials)
}

#[cfg(test)]
#[path = "approval_policy_tests.rs"]
mod tests;
