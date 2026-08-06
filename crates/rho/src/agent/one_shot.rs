use std::{future::Future, path::Path};

use anyhow::bail;
use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse, ModelUsage},
    provider::ModelProvider,
    CancellationToken, ProviderRequestUsageContext, ProviderRequestUsageRecording, SessionId,
};

use crate::credential_store::build_provider;

use super::{AgentDefinition, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolPolicy};

pub(crate) struct OneShotAgentRequest<'a> {
    pub definition: &'a AgentDefinition,
    pub usage_purpose: &'static str,
    pub provider_name: &'a str,
    pub model: &'a str,
    pub auth: &'a str,
    /// When set, overrides the definition's reasoning level.
    pub reasoning: Option<rho_providers::reasoning::ReasoningLevel>,
    pub input: String,
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a Path,
}

/// Text blocks and usage from a finished one-shot agent request.
#[derive(Debug)]
pub(crate) struct OneShotAgentResult {
    pub texts: Vec<String>,
    pub usage: ModelUsage,
}

/// Builds the provider before returning so callers can time only the model request.
pub(crate) fn run_one_shot_agent(
    request: OneShotAgentRequest<'_>,
    usage_recording: ProviderRequestUsageRecording,
) -> anyhow::Result<impl Future<Output = anyhow::Result<OneShotAgentResult>> + '_> {
    let reasoning = resolve_reasoning(request.definition, request.reasoning)?;
    let provider = build_provider(
        request.provider_name,
        request.model,
        reasoning,
        request.auth,
    )?;
    Ok(async move { run_one_shot_with_provider(provider.as_ref(), request, usage_recording).await })
}

async fn run_one_shot_with_provider(
    provider: &dyn ModelProvider,
    request: OneShotAgentRequest<'_>,
    usage_recording: ProviderRequestUsageRecording,
) -> anyhow::Result<OneShotAgentResult> {
    let reasoning = resolve_reasoning(request.definition, request.reasoning)?;
    let PromptPolicy::Replace(prompt) = &request.definition.prompt else {
        unreachable!("definition was validated")
    };
    let messages = vec![
        Message::System(prompt.clone()),
        Message::user_text(request.input),
    ];
    let usage_context =
        ProviderRequestUsageContext::for_purpose(provider.identity(), request.usage_purpose)
            .with_session_id(request.session_id.clone())
            .with_workspace_path(request.workspace_path);
    let (response, usage) = crate::usage::send_recorded(
        provider,
        ModelRequest {
            messages: &messages,
            tools: &[],
            cancellation: request.cancellation,
            reasoning_level: reasoning,
            prompt_cache_key: None,
        },
        usage_context,
        usage_recording,
    )
    .await
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
