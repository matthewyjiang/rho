use std::path::Path;

use anyhow::{anyhow, bail, Context};
use rho_providers::{model::Message, reasoning::ReasoningLevel};
use rho_sdk::{
    provider::ModelProvider, ApprovalRequest, CancellationToken, ProviderRequestUsageRecording,
    SessionId,
};

use crate::{
    agent::{
        effective_internal_agent_reasoning, internal_definition, run_one_shot_with_provider,
        OneShotAgentRequest, PERMISSION_CLASSIFIER_AGENT_ID,
    },
    config::{Config, InternalAgentTarget},
    credential_store::build_provider,
};

use super::{parse_classifier_verdict, render_classifier_transcript, ClassifierVerdict};

pub(crate) struct ClassifyRequest<'a> {
    pub history: &'a [Message],
    pub pending: &'a ApprovalRequest,
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a Path,
    pub usage_recording: ProviderRequestUsageRecording,
}

#[allow(dead_code)]
pub(crate) async fn classify_capability_request(
    config: &Config,
    request: ClassifyRequest<'_>,
) -> ClassifierVerdict {
    match try_classify_capability_request(config, request).await {
        Ok(verdict) => verdict,
        Err(error) => classifier_unavailable(error),
    }
}

async fn try_classify_capability_request(
    config: &Config,
    request: ClassifyRequest<'_>,
) -> anyhow::Result<ClassifierVerdict> {
    let model = config
        .internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)
        .ok_or_else(|| anyhow!("{PERMISSION_CLASSIFIER_AGENT_ID} model is not configured"))?;
    let reasoning = effective_internal_agent_reasoning(PERMISSION_CLASSIFIER_AGENT_ID, model);
    let InternalAgentTarget::Rho(selection) = &model.target else {
        bail!("{PERMISSION_CLASSIFIER_AGENT_ID} cannot run on Claude Code runtime");
    };
    let provider = build_provider(
        &selection.provider,
        &selection.model,
        reasoning,
        &selection.auth,
    )
    .map_err(|_| {
        anyhow!(
            "failed to build {PERMISSION_CLASSIFIER_AGENT_ID} provider; check configured credentials"
        )
    })?;
    try_classify_capability_request_with_provider(provider.as_ref(), reasoning, request).await
}

#[cfg(test)]
pub(super) async fn classify_capability_request_with_provider(
    provider: &dyn ModelProvider,
    reasoning: ReasoningLevel,
    request: ClassifyRequest<'_>,
) -> ClassifierVerdict {
    match try_classify_capability_request_with_provider(provider, reasoning, request).await {
        Ok(verdict) => verdict,
        Err(error) => classifier_unavailable(error),
    }
}

async fn try_classify_capability_request_with_provider(
    provider: &dyn ModelProvider,
    reasoning: ReasoningLevel,
    request: ClassifyRequest<'_>,
) -> anyhow::Result<ClassifierVerdict> {
    let result = run_one_shot_with_provider(
        provider,
        OneShotAgentRequest {
            definition: internal_definition(PERMISSION_CLASSIFIER_AGENT_ID),
            usage_purpose: "permission-classifier",
            reasoning: Some(reasoning),
            input: render_classifier_transcript(request.history, request.pending)?,
            cancellation: request.cancellation,
            session_id: request.session_id,
            workspace_path: request.workspace_path,
        },
        request.usage_recording,
        /*updates*/ None,
    )
    .await?;
    parse_classifier_verdict(&result.texts.join("\n"))
        .context("permission classifier returned an invalid response")
}

fn classifier_unavailable(error: impl std::fmt::Display) -> ClassifierVerdict {
    ClassifierVerdict::Deny {
        reason: format!("classifier unavailable: {error}"),
    }
}
