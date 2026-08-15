//! Shared title generation for sessions and delegated runs.

use std::time::Duration;

use rho_sdk::{CancellationToken, ProviderRequestUsageRecording, SessionId};

use crate::agent::{
    effective_internal_agent_reasoning, internal_definition, run_one_shot_agent,
    OneShotAgentRequest, SESSION_TITLE_AGENT_ID,
};
use crate::config::{Config, InternalAgentModelConfig, InternalAgentTarget};

pub(crate) const SESSION_TITLE_PROMPT: &str =
    "Generate a concise title for this chat session. Return only the title, no quotes, no punctuation at the end. Use 3 to 7 words.";

const TITLE_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) struct TitleModel {
    pub provider: String,
    pub model: String,
    pub auth: String,
    pub reasoning: rho_providers::reasoning::ReasoningLevel,
}

/// Resolve the session-title internal agent onto Rho's provider stack.
///
/// The reserved title agent does not accept Claude Code, so a delegating
/// selection is ignored and the conversation model is used instead.
pub(crate) fn title_model_from_config(config: &Config) -> TitleModel {
    let configured = config
        .internal_agents
        .get(SESSION_TITLE_AGENT_ID)
        .cloned()
        .unwrap_or_else(|| {
            InternalAgentModelConfig::new(
                config.provider.clone(),
                config.model.clone(),
                config.auth.clone(),
            )
        });
    let reasoning = effective_internal_agent_reasoning(SESSION_TITLE_AGENT_ID, &configured);
    let rho = match configured.target {
        InternalAgentTarget::Rho(model) => model,
        InternalAgentTarget::ClaudeCli { .. } => {
            return TitleModel {
                provider: config.provider.clone(),
                model: config.model.clone(),
                auth: config.auth.clone(),
                reasoning,
            };
        }
    };
    TitleModel {
        provider: rho.provider,
        model: rho.model,
        auth: rho.auth,
        reasoning,
    }
}

pub(crate) async fn generate_title(
    model: TitleModel,
    input: String,
    session_id: SessionId,
    workspace_path: std::path::PathBuf,
    usage_recording: ProviderRequestUsageRecording,
    cancellation: CancellationToken,
) -> anyhow::Result<String> {
    let request = run_one_shot_agent(
        OneShotAgentRequest {
            definition: internal_definition(SESSION_TITLE_AGENT_ID),
            usage_purpose: "title",
            reasoning: Some(model.reasoning),
            input: vec![rho_sdk::model::ContentBlock::Text(input)],
            cancellation: cancellation.clone(),
            session_id: &session_id,
            workspace_path: &workspace_path,
        },
        &model.provider,
        &model.model,
        &model.auth,
        usage_recording,
    );
    tokio::pin!(request);
    let (result, timed_out) = tokio::select! {
        result = &mut request => (result, false),
        () = tokio::time::sleep(TITLE_TIMEOUT) => {
            cancellation.cancel();
            (request.await, true)
        }
    };
    let result = match result {
        Err(_) if timed_out => return Err(anyhow::anyhow!("title generation timed out")),
        result => result?,
    };
    sanitize_title(&result.texts.join(" "))
        .ok_or_else(|| anyhow::anyhow!("title model returned an empty title"))
}

pub(crate) fn sanitize_title(title: &str) -> Option<String> {
    let mut title = title
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim()
        .to_owned();
    loop {
        let next = title
            .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '*' | '#'))
            .trim()
            .trim_end_matches(['.', ':', ';'])
            .trim();
        if next == title {
            break;
        }
        title = next.to_owned();
    }
    if title.is_empty() {
        return None;
    }
    let mut title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > 80 {
        title = title.chars().take(79).collect();
        title.push('…');
    }
    Some(title)
}

/// Short activity label shared by the activity rail and attach picker.
pub(crate) fn activity_label(activity: Option<&str>) -> &str {
    match activity {
        Some("assistant text") => "responding",
        Some(activity) => activity.strip_prefix("tool: ").unwrap_or(activity),
        None => "working",
    }
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
