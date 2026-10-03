use std::{path::Path, sync::Arc};

use anyhow::{anyhow, bail};
use rho_providers::{
    model::{
        models_dev::{cached_model_metadata, ModelMetadata},
        Message,
    },
    provider::ProviderId,
    reasoning::ReasoningLevel,
};
use rho_sdk::model::context::estimate_text_tokens;
use rho_sdk::{
    provider::ModelProvider, ApprovalRequest, CancellationToken, ProviderRequestUsageRecording,
    SessionId,
};

use crate::{
    agent::{effective_internal_agent_reasoning, PERMISSION_CLASSIFIER_AGENT_ID},
    config::{
        Config, InternalAgentModelConfig, InternalAgentTarget, ModelKind, RhoInternalAgentModel,
    },
    credential_store::build_provider_on,
};

use super::{
    review_verdict, screen_allow_probability, screen_verdict, transcript::render_with_pending_call,
    ClassifierVerdict, ScreenVerdict, TranscriptBudget, TranscriptOverBudget, CLASSIFIER_POLICY,
    DEFAULT_SCREEN_ALLOW_PERCENT, REVIEW_QUESTION, SCREEN_ALLOW_PERCENT_RANGE, SCREEN_QUESTION,
};
use crate::decision::{self, EntryModel, TextModel};
use rho_sdk::decision::{
    text::{questions_block, system_prompt, AnswerStyle},
    Answer, DecisionModel, DecisionRequest, Question,
};

/// Config entry, `[internal_agents.permission-classifier-screen]`, naming a
/// decision model or another chat model that answers the screen in place of
/// the classifier's own model. It is not an agent: it has no prompt or tools.
pub(crate) const DECISION_SCREEN_ID: &str = "permission-classifier-screen";

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
    screen: Screen,
}

/// What answers the screen, from [`DECISION_SCREEN_ID`].
pub(super) enum Screen {
    /// No entry: the classifier's own model, reading the review's transcript.
    Classifier,
    /// Another chat model, reading a transcript fitted to its own window.
    /// `budget` is `None` when the window it serves is unknown, so it could
    /// silently drop part of the transcript; it then escalates every request.
    Text {
        provider: Arc<dyn ModelProvider>,
        budget: Option<TranscriptBudget>,
    },
    /// A decision model, allowing at P(allow) of `allow_percent` or more.
    Decision {
        model: Box<dyn DecisionModel>,
        allow_percent: u8,
    },
}

/// Fails when [`DECISION_SCREEN_ID`] is configured but unusable, so a
/// headless run can refuse to start instead of denying every request.
pub(crate) fn check_screen_config(config: &Config) -> anyhow::Result<()> {
    // A text model is built at classification, so only its kind and provider
    // are checked here.
    match decision::resolve(config, DECISION_SCREEN_ID)? {
        Some(EntryModel::Decision(_)) => {
            decision_allow_percent(config)?;
        }
        Some(EntryModel::Text(_)) | None => {}
    }
    Ok(())
}

/// The allow percent of a screen entry that resolved to a decision model.
fn decision_allow_percent(config: &Config) -> Result<u8, decision::ConfigError> {
    config
        .internal_agent_model(DECISION_SCREEN_ID)
        .and_then(InternalAgentModelConfig::rho)
        .map_or(Ok(DEFAULT_SCREEN_ALLOW_PERCENT), screen_allow_percent)
}

/// The P(allow) percent at which the screen entry `selection` allows: its
/// `allow_threshold_percent`, else [`DEFAULT_SCREEN_ALLOW_PERCENT`]. Only a
/// decision model reports probabilities; a text screen ignores it.
pub(crate) fn screen_allow_percent(
    selection: &RhoInternalAgentModel,
) -> Result<u8, decision::ConfigError> {
    let percent = selection
        .allow_threshold_percent
        .unwrap_or(DEFAULT_SCREEN_ALLOW_PERCENT);
    if SCREEN_ALLOW_PERCENT_RANGE.contains(&percent) {
        Ok(percent)
    } else {
        Err(decision::ConfigError::AllowThresholdOutOfRange {
            entry: DECISION_SCREEN_ID,
            percent,
            min: *SCREEN_ALLOW_PERCENT_RANGE.start(),
            max: *SCREEN_ALLOW_PERCENT_RANGE.end(),
        })
    }
}

impl ClassifierModel {
    /// Builds the `[internal_agents.permission-classifier]` model.
    pub(crate) async fn resolve(config: &Config) -> anyhow::Result<Self> {
        let screen = resolve_screen(config).await?;
        let model = config
            .internal_agent_model(PERMISSION_CLASSIFIER_AGENT_ID)
            .ok_or_else(|| anyhow!("{PERMISSION_CLASSIFIER_AGENT_ID} model is not configured"))?;
        let reasoning = effective_internal_agent_reasoning(PERMISSION_CLASSIFIER_AGENT_ID, model);
        let InternalAgentTarget::Rho(selection) = &model.target else {
            bail!("{PERMISSION_CLASSIFIER_AGENT_ID} cannot run on Claude Code runtime");
        };
        let provider = build_provider_on(
        config,
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
            screen,
        })
    }

    pub(crate) fn provider(&self) -> &dyn ModelProvider {
        self.provider.as_ref()
    }

    /// Review-stage reasoning; a text-model screen always runs at
    /// [`ReasoningLevel::Low`].
    pub(crate) fn reasoning(&self) -> ReasoningLevel {
        self.reasoning
    }

    pub(crate) async fn classify(&self, request: ClassifyRequest<'_>) -> ClassifierTrace {
        let pending_call_id = request.pending.tool_call_id().map(|id| id.as_str());
        run_pipeline(
            self.provider.as_ref(),
            &self.screen,
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
            &self.screen,
            self.reasoning,
            self.budget,
            Some(pending_call_id),
            &request,
        )
        .await
    }
}

/// Builds the screen [`DECISION_SCREEN_ID`] names. A text model's build
/// failure names the entry, since its credentials may be what is missing.
async fn resolve_screen(config: &Config) -> anyhow::Result<Screen> {
    let selection = match decision::resolve(config, DECISION_SCREEN_ID)? {
        None => return Ok(Screen::Classifier),
        Some(EntryModel::Decision(model)) => {
            return Ok(Screen::Decision {
                model,
                allow_percent: decision_allow_percent(config)?,
            });
        }
        Some(EntryModel::Text(selection)) => selection,
    };
    let provider = build_provider_on(
        config,
        &selection.provider,
        &selection.model,
        ReasoningLevel::Low,
        &selection.auth,
    )
    .await
    .map_err(|_| decision::ConfigError::TextModelUnavailable {
        entry: DECISION_SCREEN_ID,
        configured: rho_providers::provider::model_reference(&selection.provider, &selection.model),
    })?;
    let budget = text_screen_budget(
        &selection.provider,
        cached_model_metadata(&selection.provider, &selection.model),
    );
    Ok(Screen::Text { provider, budget })
}

/// Why the screen entry `selection` likely will not work as configured: its
/// kind does not fit what was discovered, or it is a text model whose served
/// window is unknown, so it escalates every request. For `/config` and
/// `/doctor`.
pub(crate) fn screen_warning(selection: &RhoInternalAgentModel) -> Option<String> {
    decision::kind_mismatch(selection).or_else(|| {
        let unknown_window = decision::entry_kind(selection) == ModelKind::Text
            && text_screen_budget(
                &selection.provider,
                cached_model_metadata(&selection.provider, &selection.model),
            )
            .is_none();
        unknown_window.then(|| {
            format!(
                "{} has no usable_context_window, so the screen escalates every request",
                selection.model
            )
        })
    })
}

/// The transcript budget a text screen on `provider` is fitted to, or `None`
/// when the window it serves is unknown.
///
/// Ollama serves a model at the server's `num_ctx`, often far below the
/// window the model advertises, and drops the front of a longer prompt
/// without an error, which could leave a screen allowing a request it never
/// read whole. So on Ollama only a measured `usable_context_window` counts.
/// Hosted providers reject an oversize prompt, and that error escalates.
pub(super) fn text_screen_budget(
    provider: &str,
    metadata: Option<ModelMetadata>,
) -> Option<TranscriptBudget> {
    let ollama = rho_providers::provider::provider_descriptor(provider)
        .is_some_and(|descriptor| descriptor.id == ProviderId::Ollama);
    if ollama {
        metadata
            .and_then(|metadata| metadata.usable_context_window)
            .map(|window| transcript_budget(Some(window)))
    } else {
        Some(transcript_budget(
            metadata.and_then(|metadata| metadata.display_context_window()),
        ))
    }
}

/// What one classification did at each stage. Production acts only on
/// `result`; the classifier eval also reports the screen outcome.
pub(crate) struct ClassifierTrace {
    pub screen: ScreenOutcome,
    /// A decision-model screen's P(allow), when it answered.
    pub screen_allow_probability: Option<f64>,
    /// `Err` fails closed in production, as "classifier unavailable" or as
    /// the over-budget reason.
    pub result: anyhow::Result<ClassifierVerdict>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScreenOutcome {
    /// The transcript could not be rendered, so no stage ran.
    Skipped,
    Allowed,
    Escalated,
    /// The screen call or its parse failed with this error; the review
    /// decided.
    Failed(String),
}

#[cfg(test)]
pub(super) async fn classify_capability_request_with_provider(
    provider: &dyn ModelProvider,
    screen: &Screen,
    reasoning: ReasoningLevel,
    budget: TranscriptBudget,
    request: ClassifyRequest<'_>,
) -> ClassifierVerdict {
    let pending_call_id = request.pending.tool_call_id().map(|id| id.as_str());
    run_pipeline(
        provider,
        screen,
        reasoning,
        budget,
        pending_call_id,
        &request,
    )
    .await
    .result
    .unwrap_or_else(classifier_unavailable)
}

/// Transcript budget for a classifier model with `context_window` tokens.
///
/// Subtracts the shared system prompt, the longer stage questions block, and
/// [`CLASSIFIER_OUTPUT_RESERVE_TOKENS`]. An unknown window leaves the
/// transcript unbounded; the provider still rejects oversize requests.
pub(super) fn transcript_budget(context_window: Option<u64>) -> TranscriptBudget {
    let Some(window) = context_window else {
        return TranscriptBudget::Unbounded;
    };
    let questions_tokens = [SCREEN_STAGE, REVIEW_STAGE]
        .iter()
        .map(|stage| estimate_text_tokens(&questions_block(stage.questions, stage.style)))
        .max()
        .unwrap_or_default();
    let overhead = estimate_text_tokens(&system_prompt(CLASSIFIER_POLICY))
        .saturating_add(questions_tokens)
        .saturating_add(CLASSIFIER_OUTPUT_RESERVE_TOKENS);
    TranscriptBudget::Tokens(window.saturating_sub(overhead))
}

/// Runs the two-stage pipeline: a cheap screen, then a reasoned review.
///
/// Both stages are decision requests over the rendered transcript. Stage 1
/// answers `allow` or `escalate` from the [`Screen`]: a decision model, or a
/// text model at [`ReasoningLevel::Low`]. Only an escalation (or a stage 1 failure) pays
/// for stage 2, which the text model reasons through at the configured level.
///
/// Cache-prefix layout: both stages send the same system prompt and the same
/// rendered transcript as the first user text block. The stage's questions are
/// a second user text block so the last byte-identical block can be the cache
/// breakpoint. Never move a stage's questions into the system prompt.
///
/// That layout can reuse stage 1's message-cache prefix only when thinking and
/// effort stay the same, which is the default Low classifier reasoning. Raising
/// review reasoning keeps the common-path screen cheap and forgoes that cache
/// hit: Anthropic invalidates message-block cache when thinking or effort change.
async fn run_pipeline(
    provider: &dyn ModelProvider,
    screen: &Screen,
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
                    screen_allow_probability: None,
                    result: Err(error),
                }
            }
        };

    let (screen, screen_allow_probability) = match run_screen(
        provider,
        screen,
        request,
        pending_call_id,
        &transcript,
    )
    .await
    {
        Ok((ScreenVerdict::Allow, allow_probability)) => {
            return ClassifierTrace {
                screen: ScreenOutcome::Allowed,
                screen_allow_probability: allow_probability,
                result: Ok(ClassifierVerdict::Allow),
            }
        }
        Ok((ScreenVerdict::Escalate, allow_probability)) => {
            (ScreenOutcome::Escalated, allow_probability)
        }
        Err(error) => {
            // A broken screen must not decide anything; stage 2 still runs
            // and fails closed on its own if it also breaks.
            tracing::warn!(error = %error, "permission classifier screen failed; running review");
            (ScreenOutcome::Failed(format!("{error:#}")), None)
        }
    };

    let review = text_model(provider, request, &REVIEW_STAGE, reasoning);
    let result = ask(&review, request, &transcript, &REVIEW_STAGE)
        .await
        .and_then(|answers| review_verdict(&answers));
    ClassifierTrace {
        screen,
        screen_allow_probability,
        result,
    }
}

/// Stage 1: the screen's verdict, with the model's P(allow) when it reports
/// probabilities.
///
/// The classifier's own model reads the review's `transcript`. Another
/// model with a known window, or a decision model with a state budget, gets
/// its own transcript fitted to it, with the oldest tool calls left out
/// first; the review keeps its own budget, so a screen that cannot fit only
/// escalates.
async fn run_screen(
    provider: &dyn ModelProvider,
    screen: &Screen,
    request: &ClassifyRequest<'_>,
    pending_call_id: Option<&str>,
    transcript: &str,
) -> anyhow::Result<(ScreenVerdict, Option<f64>)> {
    let text_screen;
    // A text model reports no probabilities, so its allow percent is moot.
    let (model, budget, allow_percent): (&dyn DecisionModel, _, _) = match screen {
        Screen::Classifier => {
            text_screen = text_model(provider, request, &SCREEN_STAGE, ReasoningLevel::Low);
            (&text_screen, None, DEFAULT_SCREEN_ALLOW_PERCENT)
        }
        Screen::Text { provider, budget } => {
            let Some(budget) = budget else {
                let identity = provider.identity();
                bail!(
                    "text screen {} has no known served context window, so it may truncate the transcript; set usable_context_window for it in ~/.rho/models.toml",
                    rho_providers::provider::model_reference(&identity.provider, &identity.model)
                );
            };
            text_screen = text_model(
                provider.as_ref(),
                request,
                &SCREEN_STAGE,
                ReasoningLevel::Low,
            );
            let own = match budget {
                TranscriptBudget::Unbounded => None,
                TranscriptBudget::Tokens(_) => Some(*budget),
            };
            (&text_screen, own, DEFAULT_SCREEN_ALLOW_PERCENT)
        }
        Screen::Decision {
            model,
            allow_percent,
        } => (
            model.as_ref(),
            model.state_budget().map(TranscriptBudget::Tokens),
            *allow_percent,
        ),
    };
    let fitted;
    let state = match budget {
        None => transcript,
        Some(budget) => {
            fitted = render_with_pending_call(
                request.history,
                request.pending,
                pending_call_id,
                budget,
            )?;
            &fitted
        }
    };
    let answers = ask(model, request, state, &SCREEN_STAGE).await?;
    Ok((
        screen_verdict(&answers, allow_percent),
        screen_allow_probability(&answers),
    ))
}

/// One classifier stage: the questions it asks and how the model answers.
struct Stage {
    usage_purpose: &'static str,
    questions: &'static [Question<'static>],
    style: AnswerStyle,
}

const SCREEN_STAGE: Stage = Stage {
    usage_purpose: "permission-classifier-screen",
    questions: std::slice::from_ref(&SCREEN_QUESTION),
    style: AnswerStyle::Direct,
};

const REVIEW_STAGE: Stage = Stage {
    usage_purpose: "permission-classifier-review",
    questions: std::slice::from_ref(&REVIEW_QUESTION),
    style: AnswerStyle::Reasoned,
};

/// The classifier's text model answering `stage`'s questions.
fn text_model<'a>(
    provider: &'a dyn ModelProvider,
    request: &'a ClassifyRequest<'_>,
    stage: &Stage,
    reasoning: ReasoningLevel,
) -> TextModel<'a> {
    TextModel {
        provider,
        agent_id: PERMISSION_CLASSIFIER_AGENT_ID,
        usage_purpose: stage.usage_purpose,
        reasoning,
        style: stage.style,
        session_id: request.session_id,
        workspace_path: request.workspace_path,
        usage_recording: request.usage_recording.clone(),
    }
}

/// Asks `stage`'s questions about `state` under the classifier policy.
async fn ask(
    model: &dyn DecisionModel,
    request: &ClassifyRequest<'_>,
    state: &str,
    stage: &Stage,
) -> anyhow::Result<Vec<Answer>> {
    let decision = DecisionRequest::new(CLASSIFIER_POLICY, state, stage.questions);
    let answers = model.decide(decision, &request.cancellation).await?;
    decision.check_answers(&answers)?;
    Ok(answers)
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
    // A misconfigured screen names only configured values, so the fix can
    // be shown too.
    if let Some(screen) = error.downcast_ref::<decision::ConfigError>() {
        tracing::warn!(error = %screen, "permission classifier screen misconfigured");
        return ClassifierVerdict::Deny {
            reason: screen.to_string(),
        };
    }
    // Keep other details out of the executor-facing deny reason; credential
    // and provider response bodies can show up in Display output.
    tracing::warn!(error = %error, "permission classifier unavailable");
    ClassifierVerdict::Deny {
        reason: "classifier unavailable".into(),
    }
}
