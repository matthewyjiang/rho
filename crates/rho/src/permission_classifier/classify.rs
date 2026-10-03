use std::{path::Path, sync::Arc};

use anyhow::{anyhow, bail};
use rho_providers::{
    model::{models_dev::cached_model_metadata, Message},
    reasoning::ReasoningLevel,
};
use rho_sdk::model::context::estimate_text_tokens;
use rho_sdk::{
    provider::ModelProvider, ApprovalRequest, CancellationToken, ProviderRequestUsageRecording,
    SessionId,
};

use crate::{
    agent::{effective_internal_agent_reasoning, PERMISSION_CLASSIFIER_AGENT_ID},
    config::{Config, InternalAgentTarget},
    credential_store::build_provider_on,
};

use super::{
    review_verdict, screen_allow_probability, screen_verdict, transcript::render_with_pending_call,
    ClassifierVerdict, ScreenVerdict, TranscriptBudget, TranscriptOverBudget, CLASSIFIER_POLICY,
    REVIEW_QUESTION, SCREEN_QUESTION,
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
    Text {
        provider: Arc<dyn ModelProvider>,
        budget: TranscriptBudget,
    },
    Decision(Box<dyn DecisionModel>),
}

/// Fails when [`DECISION_SCREEN_ID`] is configured but unusable, so a
/// headless run can refuse to start instead of denying every request.
pub(crate) fn check_screen_config(config: &Config) -> anyhow::Result<()> {
    // A text model is built at classification, so only its kind and provider
    // are checked here.
    decision::resolve(config, DECISION_SCREEN_ID).map(drop)
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
        Some(EntryModel::Decision(model)) => return Ok(Screen::Decision(model)),
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
    let budget = transcript_budget(
        cached_model_metadata(&selection.provider, &selection.model)
            .and_then(|metadata| metadata.display_context_window()),
    );
    Ok(Screen::Text { provider, budget })
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
    let (model, budget): (&dyn DecisionModel, _) = match screen {
        Screen::Classifier => {
            text_screen = text_model(provider, request, &SCREEN_STAGE, ReasoningLevel::Low);
            (&text_screen, None)
        }
        Screen::Text { provider, budget } => {
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
            (&text_screen, own)
        }
        Screen::Decision(model) => (
            model.as_ref(),
            model.state_budget().map(TranscriptBudget::Tokens),
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
    Ok((screen_verdict(&answers), screen_allow_probability(&answers)))
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
