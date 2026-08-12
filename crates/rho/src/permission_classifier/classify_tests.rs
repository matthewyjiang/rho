use std::path::Path;

use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource,
    ProviderRequestUsageRecording, SessionId,
};

use super::{
    classify::{classify_capability_request_with_provider, ClassifyRequest},
    classify_capability_request, render_classifier_transcript, ClassifierVerdict,
    CLASSIFIER_PROMPT,
};
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    config::{Config, InternalAgentModelConfig},
};

fn source(name: &str) -> CapabilitySource {
    CapabilitySource::built_in_tool(name)
}

fn sample_history() -> Vec<Message> {
    vec![
        Message::User(vec![ContentBlock::Text("please update config.toml".into())]),
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "write_file".into(),
            arguments: serde_json::json!({"path": "config.toml", "content": "x=1"}),
        })]),
    ]
}

fn pending_write() -> ApprovalRequest {
    ApprovalRequest::new(
        CapabilityRequest::write_path(
            "config.toml",
            rho_sdk::PathScope::PrimaryWorkspace,
            source("write_file"),
        ),
        "agent requested write access",
    )
}

// Covers: provider output is parsed after the classifier prompt/transcript request is assembled
// Owner: permission classifier one-shot wiring
#[tokio::test]
async fn sends_classifier_prompt_transcript_and_parses_verdict() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text(r#"{"decision":"deny","reason":"outside user intent"}"#.into()),
        ]))],
    );
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();

    let verdict = classify_capability_request_with_provider(
        &provider,
        ReasoningLevel::Low,
        ClassifyRequest {
            history: &history,
            pending: &pending,
            cancellation: CancellationToken::new(),
            session_id: &session_id,
            workspace_path: Path::new("/test/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
        },
    )
    .await;

    assert_eq!(
        verdict,
        ClassifierVerdict::Deny {
            reason: "outside user intent".into()
        }
    );
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages,
        [
            Message::System(CLASSIFIER_PROMPT.into()),
            Message::user_text(render_classifier_transcript(&history, &pending))
        ]
    );
    assert_eq!(requests[0].reasoning_level, ReasoningLevel::Low);
    assert!(requests[0].tools.is_empty());
}

// Covers: malformed classifier output cannot grant a pending capability
// Owner: permission classifier one-shot wiring
#[tokio::test]
async fn invalid_classifier_response_fails_closed() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("not json".into()),
        ]))],
    );
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();

    let verdict = classify_capability_request_with_provider(
        &provider,
        ReasoningLevel::Low,
        ClassifyRequest {
            history: &history,
            pending: &pending,
            cancellation: CancellationToken::new(),
            session_id: &session_id,
            workspace_path: Path::new("/test/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
        },
    )
    .await;

    assert_classifier_unavailable(verdict);
}

// Covers: an unset classifier model cannot fall back to the executor model
// Owner: permission classifier model resolution
#[tokio::test]
async fn missing_classifier_model_fails_closed() {
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();

    let verdict = classify_capability_request(
        &Config::default(),
        ClassifyRequest {
            history: &history,
            pending: &pending,
            cancellation: CancellationToken::new(),
            session_id: &session_id,
            workspace_path: Path::new("/test/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
        },
    )
    .await;

    assert_classifier_unavailable(verdict);
}

// Covers: the classifier never delegates to Claude runtime
// Owner: permission classifier model resolution
#[tokio::test]
async fn claude_runtime_selection_fails_closed() {
    let mut config = Config::default();
    config.set_internal_agent_model_config(
        PERMISSION_CLASSIFIER_AGENT_ID,
        InternalAgentModelConfig::claude_cli(None),
    );
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();

    let verdict = classify_capability_request(
        &config,
        ClassifyRequest {
            history: &history,
            pending: &pending,
            cancellation: CancellationToken::new(),
            session_id: &session_id,
            workspace_path: Path::new("/test/workspace"),
            usage_recording: ProviderRequestUsageRecording::default(),
        },
    )
    .await;

    assert_classifier_unavailable(verdict);
}

fn assert_classifier_unavailable(verdict: ClassifierVerdict) {
    let ClassifierVerdict::Deny { reason } = verdict else {
        panic!("expected deny verdict");
    };
    assert!(
        reason.starts_with("classifier unavailable: "),
        "unexpected reason: {reason}"
    );
}
