use std::{path::Path, sync::Arc};

use anyhow::bail;
use rho_sdk::{
    model::{ContentBlock, Message, ModelEvent, ModelRequest, ModelResponse, ModelUsage},
    provider::{ModelProvider, ProviderRequestEvent, ProviderStreamEvent},
    CancellationToken, ProviderRequestUsageContext, ProviderRequestUsageRecording, SessionId,
};
use tokio::sync::watch;

use crate::credential_store::build_provider;

use super::{AgentDefinition, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolPolicy};

pub(crate) struct OneShotAgentRequest<'a> {
    pub definition: &'a AgentDefinition,
    pub usage_purpose: &'static str,
    /// When set, overrides the definition's reasoning level.
    pub reasoning: Option<rho_providers::reasoning::ReasoningLevel>,
    pub input: Vec<ContentBlock>,
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a Path,
}

/// Live phase of a one-shot model request.
///
/// Labels mirror the root activity rail so nested cards read the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OneShotPhase {
    WaitingForProvider,
    Thinking,
    Responding,
    RetryingProvider,
}

impl OneShotPhase {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::WaitingForProvider => "waiting for provider",
            Self::Thinking => "thinking",
            Self::Responding => "responding",
            Self::RetryingProvider => "retrying provider",
        }
    }
}

/// Latest display snapshot while a one-shot request is in flight.
///
/// `text` is the canonical assistant output so far. Reasoning never appears
/// here - only the phase moves to [`OneShotPhase::Thinking`]. Published through
/// a watch channel so a slow consumer keeps only the newest snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OneShotUpdate {
    pub phase: OneShotPhase,
    pub text: Arc<str>,
}

impl OneShotUpdate {
    pub(crate) fn new(phase: OneShotPhase, text: impl AsRef<str>) -> Self {
        Self {
            phase,
            text: Arc::from(text.as_ref()),
        }
    }
}

/// Text blocks and usage from a finished one-shot agent request.
#[derive(Debug)]
pub(crate) struct OneShotAgentResult {
    pub texts: Vec<String>,
    pub usage: ModelUsage,
}

/// Builds the provider (awaiting catalog hydrate when needed) then runs the request.
pub(crate) async fn run_one_shot_agent(
    request: OneShotAgentRequest<'_>,
    provider_name: &str,
    model: &str,
    auth: &str,
    usage_recording: ProviderRequestUsageRecording,
) -> anyhow::Result<OneShotAgentResult> {
    let reasoning = resolve_reasoning(request.definition, request.reasoning)?;
    let provider = build_provider(provider_name, model, reasoning, auth).await?;
    run_one_shot_with_provider(
        provider.as_ref(),
        request,
        usage_recording,
        /*updates*/ None,
    )
    .await
}

/// Runs a prepared one-shot request against an already-built provider.
pub(crate) async fn run_one_shot_with_provider(
    provider: &dyn ModelProvider,
    request: OneShotAgentRequest<'_>,
    usage_recording: ProviderRequestUsageRecording,
    updates: Option<watch::Sender<OneShotUpdate>>,
) -> anyhow::Result<OneShotAgentResult> {
    let reasoning = resolve_reasoning(request.definition, request.reasoning)?;
    let PromptPolicy::Replace(prompt) = &request.definition.prompt else {
        unreachable!("definition was validated")
    };
    let messages = vec![
        Message::System(prompt.clone()),
        Message::User(request.input),
    ];
    let usage_context =
        ProviderRequestUsageContext::for_purpose(provider.identity(), request.usage_purpose)
            .with_session_id(request.session_id.clone())
            .with_workspace_path(request.workspace_path);

    let mut stream = OneShotStream::new(updates);
    stream.publish(OneShotPhase::WaitingForProvider, "");

    let model_request = ModelRequest {
        messages: &messages,
        tools: &[],
        cancellation: request.cancellation,
        reasoning_level: reasoning,
        prompt_cache_key: None,
    };
    let (response, usage) = if stream.has_updates() {
        crate::usage::send_recorded_observing(
            provider,
            model_request,
            usage_context,
            usage_recording,
            1,
            |event| stream.observe(event),
        )
        .await
    } else {
        crate::usage::send_recorded(provider, model_request, usage_context, usage_recording).await
    }
    .map_err(|error| anyhow::anyhow!(error))?;
    let ModelResponse::Assistant(blocks) = response;
    // Successful runs only: failed attempts are written to the durable ledger by
    // send_recorded, but their usage is not returned here.
    Ok(OneShotAgentResult {
        texts: blocks
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text),
                ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
            })
            .collect(),
        usage,
    })
}

/// Accumulates canonical one-shot display state from provider stream events.
struct OneShotStream {
    phase: OneShotPhase,
    text: String,
    updates: Option<watch::Sender<OneShotUpdate>>,
}

impl OneShotStream {
    fn new(updates: Option<watch::Sender<OneShotUpdate>>) -> Self {
        Self {
            phase: OneShotPhase::WaitingForProvider,
            text: String::new(),
            updates,
        }
    }

    fn observe(&mut self, event: &ProviderStreamEvent) {
        match event {
            ProviderStreamEvent::Model(ModelEvent::OutputDelta(delta)) => {
                self.phase = OneShotPhase::Responding;
                self.text.push_str(delta);
                self.try_publish();
            }
            ProviderStreamEvent::Model(
                ModelEvent::ReasoningDelta(_) | ModelEvent::ReasoningSummaryDelta(_),
            ) => {
                // Never surface reasoning content. Once output has started, keep
                // responding so a mid-turn thinking blip does not hide text.
                if self.phase != OneShotPhase::Responding && self.phase != OneShotPhase::Thinking {
                    self.phase = OneShotPhase::Thinking;
                    self.try_publish();
                }
            }
            ProviderStreamEvent::Request(ProviderRequestEvent::RequestAttemptFailed { .. }) => {
                // A failed physical attempt abandons its partial output. Stay on
                // retrying until the next stream event so latest-wins consumers
                // can observe the phase.
                self.text.clear();
                self.phase = OneShotPhase::RetryingProvider;
                self.try_publish();
            }
            ProviderStreamEvent::Model(
                ModelEvent::Usage(_)
                | ModelEvent::WebSearch(_)
                | ModelEvent::ToolCallDelta { .. }
                | ModelEvent::ProviderContext { .. },
            ) => {}
        }
    }

    fn has_updates(&self) -> bool {
        self.updates.is_some()
    }

    fn try_publish(&self) {
        let Some(updates) = &self.updates else {
            return;
        };
        // Latest-wins: a slow tool-card consumer never queues every token copy.
        let _ = updates.send(OneShotUpdate::new(self.phase, &self.text));
    }

    fn publish(&mut self, phase: OneShotPhase, text: &str) {
        self.phase = phase;
        self.text = text.to_owned();
        self.try_publish();
    }
}

fn resolve_reasoning(
    definition: &AgentDefinition,
    override_level: Option<rho_providers::reasoning::ReasoningLevel>,
) -> anyhow::Result<rho_providers::reasoning::ReasoningLevel> {
    validate_definition(definition)?;
    override_level
        .or_else(|| definition.reasoning())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "one-shot agent definition '{}' must set a reasoning level",
                definition.id
            )
        })
}

fn validate_definition(definition: &AgentDefinition) -> anyhow::Result<()> {
    if !matches!(definition.prompt, PromptPolicy::Replace(_)) {
        bail!(
            "one-shot agent definition '{}' must replace the system prompt",
            definition.id
        );
    }
    if *definition.model_policy() != ModelPolicy::Inherit {
        bail!(
            "one-shot agent definition '{}' must inherit its model",
            definition.id
        );
    }
    match &definition.runtime {
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(tools),
            ..
        } if tools.is_empty() => {}
        AgentRuntimeSpec::Rho { .. } => {
            bail!(
                "one-shot agent definition '{}' must allow no tools",
                definition.id
            );
        }
        AgentRuntimeSpec::ClaudeCli(_) => {
            bail!(
                "one-shot agent definition '{}' must use the rho runtime",
                definition.id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "one_shot_tests.rs"]
mod tests;
