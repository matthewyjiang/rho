use std::{path::Path, sync::Arc};

use anyhow::{anyhow, bail, Context};
use rho_providers::{
    model::{models_dev::cached_model_metadata, ContentBlock, Message},
    reasoning::ReasoningLevel,
};
use rho_sdk::model::context::estimate_text_tokens;
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
    parse_classifier_verdict, parse_screen_verdict, transcript::render_with_pending_call,
    ClassifierVerdict, ScreenVerdict, TranscriptBudget, TranscriptOverBudget, CLASSIFIER_PROMPT,
    CLASSIFIER_REVIEW_INSTRUCTION, CLASSIFIER_SCREEN_INSTRUCTION,
};

/// Context reserved for the classifier's own output when sizing the transcript.
///
/// Usage ledger receipt: 398 review calls peaked at 5,348 output tokens
/// (p99 954); screens peaked at 924. 8,192 covers the observed peak.
const CLASSIFIER_OUTPUT_RESERVE_TOKENS: u64 = 8_192;

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
    let result = match ClassifierModel::resolve(config).await {
        Ok(model) => model.classify(request).await.result,
        Err(error) => Err(error),
    };
    result.unwrap_or_else(classifier_unavailable)
}

/// The configured classifier model, ready to classify requests.
pub(crate) struct ClassifierModel {
    provider: Arc<dyn ModelProvider>,
    reasoning: ReasoningLevel,
    budget: TranscriptBudget,
}

impl ClassifierModel {
    /// Builds the `[internal_agents.permission-classifier]` model.
    pub(crate) async fn resolve(config: &Config) -> anyhow::Result<Self> {
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
        .await
        .map_err(|_| {
            anyhow!(
                "failed to build {PERMISSION_CLASSIFIER_AGENT_ID} provider; check configured credentials"
            )
        })?;
        // Read the window after building the provider: catalog-driven
        // providers hydrate the model catalog during construction.
        let budget = transcript_budget(
            cached_model_metadata(&selection.provider, &selection.model)
                .and_then(|metadata| metadata.display_context_window()),
        );
        Ok(Self {
            provider,
            reasoning,
            budget,
        })
    }

    pub(crate) fn provider(&self) -> &dyn ModelProvider {
        self.provider.as_ref()
    }

    /// Review-stage reasoning; the screen always runs at [`ReasoningLevel::Low`].
    pub(crate) fn reasoning(&self) -> ReasoningLevel {
        self.reasoning
    }

    pub(crate) async fn classify(&self, request: ClassifyRequest<'_>) -> ClassifierTrace {
        let pending_call_id = request.pending.tool_call_id().map(|id| id.as_str());
        run_pipeline(
            self.provider.as_ref(),
            self.reasoning,
            self.budget,
            pending_call_id,
            &request,
        )
        .await
    }

    /// [`Self::classify`] for a request the SDK did not build, such as a
    /// replayed call, whose tool call ID cannot be attached to it.
    pub(crate) async fn classify_with_pending_call(
        &self,
        request: ClassifyRequest<'_>,
        pending_call_id: &str,
    ) -> ClassifierTrace {
        run_pipeline(
            self.provider.as_ref(),
            self.reasoning,
            self.budget,
            Some(pending_call_id),
            &request,
        )
        .await
    }
}

/// What one classification did at each stage. Production acts only on
/// `result`; the classifier eval also reports the screen outcome.
pub(crate) struct ClassifierTrace {
    pub screen: ScreenOutcome,
    /// `Err` fails closed in production, as "classifier unavailable" or as
    /// the over-budget reason.
    pub result: anyhow::Result<ClassifierVerdict>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScreenOutcome {
    /// The transcript could not be rendered, so no stage ran.
    Skipped,
    Allowed,
    Escalated,
    /// The screen call or its parse failed; the review decided.
    Failed,
}

#[cfg(test)]
pub(super) async fn classify_capability_request_with_provider(
    provider: &dyn ModelProvider,
    reasoning: ReasoningLevel,
    budget: TranscriptBudget,
    request: ClassifyRequest<'_>,
) -> ClassifierVerdict {
    let pending_call_id = request.pending.tool_call_id().map(|id| id.as_str());
    run_pipeline(provider, reasoning, budget, pending_call_id, &request)
        .await
        .result
        .unwrap_or_else(classifier_unavailable)
}

/// Transcript budget for a classifier model with `context_window` tokens.
///
/// Subtracts the shared system prompt, the longer stage instruction, and
/// [`CLASSIFIER_OUTPUT_RESERVE_TOKENS`]. An unknown window leaves the
/// transcript unbounded; the provider still rejects oversize requests.
pub(super) fn transcript_budget(context_window: Option<u64>) -> TranscriptBudget {
    let Some(window) = context_window else {
        return TranscriptBudget::Unbounded;
    };
    let instruction_tokens = estimate_text_tokens(CLASSIFIER_SCREEN_INSTRUCTION)
        .max(estimate_text_tokens(CLASSIFIER_REVIEW_INSTRUCTION));
    let overhead = estimate_text_tokens(CLASSIFIER_PROMPT)
        .saturating_add(instruction_tokens)
        .saturating_add(CLASSIFIER_OUTPUT_RESERVE_TOKENS);
    TranscriptBudget::Tokens(window.saturating_sub(overhead))
}

/// Runs the two-stage pipeline: a cheap screen, then a reasoned review.
///
/// Stage 1 answers `allow` or `escalate` in one token at [`ReasoningLevel::Low`].
/// Only an escalation (or a stage 1 provider error) pays for stage 2, which uses
/// the configured reasoning level.
///
/// Cache-prefix layout: both stages send the same system prompt and the same
/// rendered transcript as the first user text block. The stage instruction is a
/// second user text block so the last byte-identical block can be the cache
/// breakpoint. Never move a stage instruction into the system prompt.
///
/// That layout can reuse stage 1's message-cache prefix only when thinking and
/// effort stay the same, which is the default Low classifier reasoning. Raising
/// review reasoning keeps the common-path screen cheap and forgoes that cache
/// hit: Anthropic invalidates message-block cache when thinking or effort change.
async fn run_pipeline(
    provider: &dyn ModelProvider,
    reasoning: ReasoningLevel,
    budget: TranscriptBudget,
    pending_call_id: Option<&str>,
    request: &ClassifyRequest<'_>,
) -> ClassifierTrace {
    let transcript =
        match render_with_pending_call(request.history, request.pending, pending_call_id, budget) {
            Ok(transcript) => transcript,
            Err(error) => {
                return ClassifierTrace {
                    screen: ScreenOutcome::Skipped,
                    result: Err(error),
                }
            }
        };

    let screen = run_stage(
        provider,
        request,
        StageSpec {
            usage_purpose: "permission-classifier-screen",
            reasoning: ReasoningLevel::Low,
            input: stage_input(&transcript, CLASSIFIER_SCREEN_INSTRUCTION),
        },
    )
    .await;
    let screen = match screen.as_deref().map(parse_screen_verdict) {
        Ok(ScreenVerdict::Allow) => {
            return ClassifierTrace {
                screen: ScreenOutcome::Allowed,
                result: Ok(ClassifierVerdict::Allow),
            }
        }
        Ok(ScreenVerdict::Escalate) => ScreenOutcome::Escalated,
        Err(error) => {
            // A broken screen must not decide anything; stage 2 still runs and
            // fails closed on its own if it also breaks.
            tracing::warn!(error = %error, "permission classifier screen failed; running review");
            ScreenOutcome::Failed
        }
    };

    let review = run_stage(
        provider,
        request,
        StageSpec {
            usage_purpose: "permission-classifier-review",
            reasoning,
            input: stage_input(&transcript, CLASSIFIER_REVIEW_INSTRUCTION),
        },
    )
    .await;
    let result = review.and_then(|review| {
        parse_classifier_verdict(&review)
            .context("permission classifier returned an invalid response")
    });
    ClassifierTrace { screen, result }
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

fn classifier_unavailable(error: anyhow::Error) -> ClassifierVerdict {
    // An over-budget transcript carries only token counts, so the limit and
    // the asked size can be shown instead of hidden behind "unavailable".
    if let Some(over_budget) = error.downcast_ref::<TranscriptOverBudget>() {
        tracing::warn!(error = %over_budget, "permission classifier transcript over budget");
        return ClassifierVerdict::Deny {
            reason: over_budget.to_string(),
        };
    }
    // Keep other details out of the executor-facing deny reason; credential
    // and provider response bodies can show up in Display output.
    tracing::warn!(error = %error, "permission classifier unavailable");
    ClassifierVerdict::Deny {
        reason: "classifier unavailable".into(),
    }
}
