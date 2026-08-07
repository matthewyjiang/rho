use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ModelUsage, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    ProviderRequestUsageEvent, ProviderRequestUsageRecorder, ProviderRequestUsageRecorderFuture,
};
use serde_json::json;

use super::*;
use crate::agent::{
    AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolCapability, ToolPolicy,
};

#[derive(Clone, Default)]
struct CapturingRecorder(Arc<Mutex<Vec<ProviderRequestUsageEvent>>>);

impl ProviderRequestUsageRecorder for CapturingRecorder {
    fn record(&self, event: ProviderRequestUsageEvent) -> ProviderRequestUsageRecorderFuture<'_> {
        self.0.lock().unwrap().push(event);
        Box::pin(async { Ok(()) })
    }
}

fn definition() -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new("test-agent").unwrap(),
        description: "test".into(),
        prompt: PromptPolicy::Replace("system prompt".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(BTreeSet::new()),
            model: ModelPolicy::Inherit,
            reasoning: Some(rho_providers::reasoning::ReasoningLevel::Low),
        },
    }
}

fn request<'a>(
    definition: &'a AgentDefinition,
    session_id: &'a SessionId,
    workspace_path: &'a Path,
) -> OneShotAgentRequest<'a> {
    OneShotAgentRequest {
        definition,
        usage_purpose: "test-purpose",
        provider_name: "test-provider",
        model: "test-model",
        auth: "api-key",
        reasoning: None,
        input: "user input".into(),
        cancellation: CancellationToken::new(),
        session_id,
        workspace_path,
    }
}

#[test]
fn rejects_definitions_that_do_not_replace_the_prompt() {
    let mut definition = definition();
    definition.prompt = PromptPolicy::Extend("extension".into());
    assert!(validate_definition(&definition)
        .unwrap_err()
        .to_string()
        .contains("replace the system prompt"));
}

#[test]
fn rejects_definitions_with_tools() {
    let mut definition = definition();
    definition.runtime = AgentRuntimeSpec::Rho {
        tools: ToolPolicy::Allow(BTreeSet::from([ToolCapability::ReadFile])),
        model: ModelPolicy::Inherit,
        reasoning: Some(rho_providers::reasoning::ReasoningLevel::Low),
    };
    assert!(validate_definition(&definition)
        .unwrap_err()
        .to_string()
        .contains("allow no tools"));
}

#[test]
fn rejects_definitions_that_select_a_model() {
    let mut definition = definition();
    if let AgentRuntimeSpec::Rho { model, .. } = &mut definition.runtime {
        *model = ModelPolicy::Select(crate::agent::ModelSelection {
            provider: None,
            model: "other-model".into(),
            auth: None,
        });
    }
    assert!(validate_definition(&definition)
        .unwrap_err()
        .to_string()
        .contains("inherit its model"));
}

#[test]
fn rejects_definitions_without_reasoning() {
    let mut definition = definition();
    if let AgentRuntimeSpec::Rho { reasoning, .. } = &mut definition.runtime {
        *reasoning = None;
    }
    assert!(resolve_reasoning(&definition, None)
        .unwrap_err()
        .to_string()
        .contains("set a reasoning level"));
}

#[tokio::test]
async fn assembles_messages_extracts_text_and_records_usage_purpose() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [ScriptedTurn::streaming(
            vec![rho_sdk::model::ModelEvent::Usage(ModelUsage {
                input_tokens: Some(11),
                output_tokens: Some(7),
                cost_usd_micros: Some(42),
                ..ModelUsage::default()
            })],
            ModelResponse::Assistant(vec![
                ContentBlock::Text("first".into()),
                ContentBlock::ToolCall(ToolCall {
                    id: "call".into(),
                    name: "ignored".into(),
                    arguments: json!({}),
                }),
                ContentBlock::Text("second".into()),
            ]),
        )],
    );
    let recorder = CapturingRecorder::default();
    let definition = definition();
    let session_id = SessionId::new();

    let result = run_one_shot_with_provider(
        &provider,
        request(&definition, &session_id, Path::new("/test/workspace")),
        ProviderRequestUsageRecording::new(recorder.clone()),
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.texts, ["first", "second"]);
    assert_eq!(
        result.usage,
        ModelUsage {
            input_tokens: Some(11),
            output_tokens: Some(7),
            cost_usd_micros: Some(42),
            ..ModelUsage::default()
        }
    );
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages,
        [
            Message::System("system prompt".into()),
            Message::user_text("user input")
        ]
    );
    assert!(requests[0].tools.is_empty());
    assert_eq!(
        requests[0].reasoning_level,
        rho_providers::reasoning::ReasoningLevel::Low
    );
    let events = recorder.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].context().purpose(), "test-purpose");
    assert_eq!(
        events[0].context().workspace_path(),
        Some(Path::new("/test/workspace"))
    );
}

#[tokio::test]
async fn forwards_cancellation_to_the_provider_request() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("must not complete".into()),
        ]))],
    );
    let definition = definition();
    let session_id = SessionId::new();
    let request = request(&definition, &session_id, Path::new("/test/workspace"));
    request.cancellation.cancel();

    let error = run_one_shot_with_provider(
        &provider,
        request,
        ProviderRequestUsageRecording::default(),
        None,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("cancel"));
    assert!(provider.recorded_requests().is_empty());
}

// Covers: live updates track phase and canonical text without exposing reasoning
// Owner: one-shot agent stream
#[tokio::test]
async fn streams_phase_and_text_without_reasoning_content() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [ScriptedTurn::streaming(
            vec![
                rho_sdk::model::ModelEvent::ReasoningDelta("hidden chain".into()),
                rho_sdk::model::ModelEvent::OutputDelta("plan ".into()),
                rho_sdk::model::ModelEvent::OutputDelta("next".into()),
                rho_sdk::model::ModelEvent::Usage(ModelUsage {
                    output_tokens: Some(2),
                    ..ModelUsage::default()
                }),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("plan next".into())]),
        )],
    );
    let definition = definition();
    let session_id = SessionId::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let result = run_one_shot_with_provider(
        &provider,
        request(&definition, &session_id, Path::new("/test/workspace")),
        ProviderRequestUsageRecording::default(),
        Some(tx),
    )
    .await
    .unwrap();

    assert_eq!(result.texts, ["plan next"]);
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    assert_eq!(
        updates,
        [
            OneShotUpdate {
                phase: OneShotPhase::WaitingForProvider,
                text: String::new(),
            },
            OneShotUpdate {
                phase: OneShotPhase::Thinking,
                text: String::new(),
            },
            OneShotUpdate {
                phase: OneShotPhase::Responding,
                text: "plan ".into(),
            },
            OneShotUpdate {
                phase: OneShotPhase::Responding,
                text: "plan next".into(),
            },
        ]
    );
    assert!(updates.iter().all(|update| !update.text.contains("hidden")));
}

// Covers: a failed physical attempt clears partial text before the next try
// Owner: one-shot agent stream
#[tokio::test]
async fn retry_clears_partial_text_before_the_next_attempt() {
    use rho_sdk::{
        provider::{ProviderRequestEvent, ProviderStreamEvent},
        ProviderErrorKind,
    };

    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [ScriptedTurn::streaming_with_request_events(
            vec![
                ProviderStreamEvent::Model(rho_sdk::model::ModelEvent::OutputDelta("stale".into())),
                ProviderStreamEvent::Request(ProviderRequestEvent::RequestAttemptFailed {
                    kind: ProviderErrorKind::Timeout,
                    usage: ModelUsage::default(),
                }),
                ProviderStreamEvent::Model(rho_sdk::model::ModelEvent::OutputDelta(
                    "recovered".into(),
                )),
            ],
            ModelResponse::Assistant(vec![ContentBlock::Text("recovered".into())]),
        )],
    );
    let definition = definition();
    let session_id = SessionId::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let result = run_one_shot_with_provider(
        &provider,
        request(&definition, &session_id, Path::new("/test/workspace")),
        ProviderRequestUsageRecording::default(),
        Some(tx),
    )
    .await
    .unwrap();

    assert_eq!(result.texts, ["recovered"]);
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    assert!(updates
        .iter()
        .any(|update| { update.phase == OneShotPhase::Responding && update.text == "stale" }));
    assert!(updates.iter().any(|update| {
        update.phase == OneShotPhase::RetryingProvider && update.text.is_empty()
    }));
    assert_eq!(
        updates.last(),
        Some(&OneShotUpdate {
            phase: OneShotPhase::Responding,
            text: "recovered".into(),
        })
    );
}
