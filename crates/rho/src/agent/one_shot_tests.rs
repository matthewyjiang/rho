use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    ProviderRequestUsageEvent, ProviderRequestUsageRecorder, ProviderRequestUsageRecorderFuture,
};
use serde_json::json;

use super::*;
use crate::agent::{AgentId, ModelPolicy, PromptPolicy, ToolCapability, ToolPolicy};

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
        model: ModelPolicy::Inherit,
        tools: ToolPolicy::Allow(BTreeSet::new()),
        reasoning: Some(rho_providers::reasoning::ReasoningLevel::Low),
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
        input: "user input".into(),
        cancellation: CancellationToken::new(),
        session_id,
        workspace_path,
    }
}

#[test]
fn rejects_invalid_definition_before_returning_request_future() {
    let mut definition = definition();
    definition.prompt = PromptPolicy::Extend("extension".into());
    let session_id = SessionId::new();

    let result = run_one_shot_agent(
        request(&definition, &session_id, Path::new("/test/workspace")),
        ProviderRequestUsageRecording::default(),
    );

    let Err(error) = result else {
        panic!("invalid definition returned a request future");
    };
    assert!(error.to_string().contains("replace the system prompt"));
}

#[test]
fn builds_the_provider_before_returning_request_future() {
    let definition = definition();
    let session_id = SessionId::new();

    let result = run_one_shot_agent(
        request(&definition, &session_id, Path::new("/test/workspace")),
        ProviderRequestUsageRecording::default(),
    );

    let Err(error) = result else {
        panic!("unknown provider returned a request future");
    };
    assert!(error.to_string().contains("test-provider"));
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
    definition.tools = ToolPolicy::Allow(BTreeSet::from([ToolCapability::ReadFile]));
    assert!(validate_definition(&definition)
        .unwrap_err()
        .to_string()
        .contains("allow no tools"));
}

#[test]
fn rejects_definitions_that_select_a_model() {
    let mut definition = definition();
    definition.model = ModelPolicy::Select(crate::agent::ModelSelection {
        provider: None,
        model: "other-model".into(),
    });
    assert!(validate_definition(&definition)
        .unwrap_err()
        .to_string()
        .contains("inherit its model"));
}

#[test]
fn rejects_definitions_without_reasoning() {
    let mut definition = definition();
    definition.reasoning = None;
    assert!(validate_definition(&definition)
        .unwrap_err()
        .to_string()
        .contains("set a reasoning level"));
}

#[tokio::test]
async fn assembles_messages_extracts_text_and_records_usage_purpose() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("first".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "call".into(),
                name: "ignored".into(),
                arguments: json!({}),
            }),
            ContentBlock::Text("second".into()),
        ]))],
    );
    let recorder = CapturingRecorder::default();
    let definition = definition();
    let session_id = SessionId::new();

    let blocks = run_one_shot_with_provider(
        &provider,
        request(&definition, &session_id, Path::new("/test/workspace")),
        ProviderRequestUsageRecording::new(recorder.clone()),
    )
    .await
    .unwrap();

    assert_eq!(blocks, ["first", "second"]);
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

    let error =
        run_one_shot_with_provider(&provider, request, ProviderRequestUsageRecording::default())
            .await
            .unwrap_err();

    assert!(error.to_string().contains("cancel"));
    assert!(provider.recorded_requests().is_empty());
}
