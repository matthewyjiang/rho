use std::{future::Future, path::Path};

use anyhow::bail;
use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    provider::ModelProvider,
    CancellationToken, ProviderRequestUsageContext, ProviderRequestUsageRecording, SessionId,
};

use crate::credential_store::build_provider;

use super::{AgentDefinition, ModelPolicy, PromptPolicy, ToolPolicy};

pub(crate) struct OneShotAgentRequest<'a> {
    pub definition: &'a AgentDefinition,
    pub usage_purpose: &'static str,
    pub provider_name: &'a str,
    pub model: &'a str,
    pub input: String,
    pub cancellation: CancellationToken,
    pub session_id: &'a SessionId,
    pub workspace_path: &'a Path,
}

/// Builds the provider before returning so callers can time only the model request.
pub(crate) fn run_one_shot_agent(
    request: OneShotAgentRequest<'_>,
    usage_recording: ProviderRequestUsageRecording,
) -> anyhow::Result<impl Future<Output = anyhow::Result<Vec<String>>> + '_> {
    let reasoning = validate_definition(request.definition)?;
    let provider = build_provider(request.provider_name, request.model, reasoning)?;
    Ok(async move { run_one_shot_with_provider(provider.as_ref(), request, usage_recording).await })
}

async fn run_one_shot_with_provider(
    provider: &dyn ModelProvider,
    request: OneShotAgentRequest<'_>,
    usage_recording: ProviderRequestUsageRecording,
) -> anyhow::Result<Vec<String>> {
    let reasoning = validate_definition(request.definition)?;
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
    let (response, _) = crate::usage::send_recorded(
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
    Ok(blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect())
}

fn validate_definition(
    definition: &AgentDefinition,
) -> anyhow::Result<rho_providers::reasoning::ReasoningLevel> {
    if !matches!(definition.prompt, PromptPolicy::Replace(_)) {
        bail!(
            "one-shot agent definition '{}' must replace the system prompt",
            definition.id
        );
    }
    if definition.model != ModelPolicy::Inherit {
        bail!(
            "one-shot agent definition '{}' must inherit its model",
            definition.id
        );
    }
    if !matches!(&definition.tools, ToolPolicy::Allow(tools) if tools.is_empty()) {
        bail!(
            "one-shot agent definition '{}' must allow no tools",
            definition.id
        );
    }
    definition.reasoning.ok_or_else(|| {
        anyhow::anyhow!(
            "one-shot agent definition '{}' must set a reasoning level",
            definition.id
        )
    })
}

#[cfg(test)]
#[path = "one_shot_tests.rs"]
mod tests;
