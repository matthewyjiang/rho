use std::path::Path;

use anyhow::{anyhow, bail, Context};
use rho_providers::{
    model::{ContentBlock, Message},
    reasoning::ReasoningLevel,
};
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

use super::{
    parse_classifier_verdict, parse_screen_verdict, render_classifier_transcript,
    ClassifierVerdict, ScreenVerdict, CLASSIFIER_REVIEW_INSTRUCTION, CLASSIFIER_SCREEN_INSTRUCTION,
};

pub(crate) struct ClassifyRequest<'a> {
    pub history: &'a [Message],
    pub pending: &'a ApprovalRequest,
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a Path,
    pub usage_recording: ProviderRequestUsageRecording,
}

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

/// Runs the two-stage pipeline: a cheap screen, then a reasoned review.
///
/// Stage 1 answers `allow` or `escalate` in one token. Only an escalation (or a
/// stage 1 provider error) pays for stage 2.
///
/// Cache-prefix layout: both stages send the same system prompt, the same
/// rendered transcript as the first user text block, and the same reasoning
/// level. The stage instruction is a second user text block so the last
/// byte-identical block can be the cache breakpoint. Never move a stage
/// instruction into the system prompt, and never change thinking or effort
/// between stages: Anthropic invalidates message-block cache when those change.
async fn try_classify_capability_request_with_provider(
    provider: &dyn ModelProvider,
    reasoning: ReasoningLevel,
    request: ClassifyRequest<'_>,
) -> anyhow::Result<ClassifierVerdict> {
    let transcript = render_classifier_transcript(request.history, request.pending)?;

    let screen = run_stage(
        provider,
        &request,
        StageSpec {
            usage_purpose: "permission-classifier-screen",
            reasoning,
            input: stage_input(&transcript, CLASSIFIER_SCREEN_INSTRUCTION),
        },
    )
    .await;
    match screen.as_deref().map(parse_screen_verdict) {
        Ok(ScreenVerdict::Allow) => return Ok(ClassifierVerdict::Allow),
        Ok(ScreenVerdict::Escalate) => {}
        Err(error) => {
            // A broken screen must not decide anything; stage 2 still runs and
            // fails closed on its own if it also breaks.
            tracing::warn!(error = %error, "permission classifier screen failed; running review");
        }
    }

    let review = run_stage(
        provider,
        &request,
        StageSpec {
            usage_purpose: "permission-classifier-review",
            reasoning,
            input: stage_input(&transcript, CLASSIFIER_REVIEW_INSTRUCTION),
        },
    )
    .await?;
    parse_classifier_verdict(&review).context("permission classifier returned an invalid response")
}

struct StageSpec {
    usage_purpose: &'static str,
    reasoning: ReasoningLevel,
    input: Vec<ContentBlock>,
}

fn stage_input(transcript: &str, instruction: &str) -> Vec<ContentBlock> {
    vec![
        ContentBlock::Text(transcript.to_owned()),
        ContentBlock::Text(instruction.to_owned()),
    ]
}

async fn run_stage(
    provider: &dyn ModelProvider,
    request: &ClassifyRequest<'_>,
    stage: StageSpec,
) -> anyhow::Result<String> {
    let result = run_one_shot_with_provider(
        provider,
        OneShotAgentRequest {
            definition: internal_definition(PERMISSION_CLASSIFIER_AGENT_ID),
            usage_purpose: stage.usage_purpose,
            reasoning: Some(stage.reasoning),
            input: stage.input,
            cancellation: request.cancellation.clone(),
            session_id: request.session_id,
            workspace_path: request.workspace_path,
        },
        request.usage_recording.clone(),
        /*updates*/ None,
    )
    .await?;
    Ok(result.texts.join("\n"))
}

fn classifier_unavailable(error: impl std::fmt::Display) -> ClassifierVerdict {
    // Keep details out of the executor-facing deny reason; credential and
    // provider response bodies can show up in Display output.
    tracing::warn!(error = %error, "permission classifier unavailable");
    ClassifierVerdict::Deny {
        reason: "classifier unavailable".into(),
    }
}
