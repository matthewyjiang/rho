use std::path::Path;

use pretty_assertions::assert_eq;
use rho_providers::reasoning::ReasoningLevel;
use rho_sdk::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse, ToolCall},
    provider::{ScriptedProvider, ScriptedTurn},
    ApprovalRequest, CancellationToken, CapabilityRequest, CapabilitySource, ProviderError,
    ProviderErrorKind, ProviderRequestUsageRecording, Retryability, SessionId,
};

use super::{
    classify::{classify_capability_request_with_provider, ClassifyRequest},
    classify_capability_request, render_classifier_transcript, ClassifierVerdict,
    CLASSIFIER_PROMPT, CLASSIFIER_REVIEW_INSTRUCTION, CLASSIFIER_SCREEN_INSTRUCTION,
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

fn text_turn(text: &str) -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        text.into(),
    )]))
}

fn failed_turn() -> ScriptedTurn {
    ScriptedTurn::failed(ProviderError::new(
        ProviderErrorKind::Unavailable,
        "provider down",
        Retryability::Permanent,
    ))
}

fn unavailable() -> ClassifierVerdict {
    ClassifierVerdict::Deny {
        reason: "classifier unavailable".into(),
    }
}

async fn run_pipeline(
    provider: &ScriptedProvider,
    reasoning: ReasoningLevel,
) -> (
    ClassifierVerdict,
    Vec<rho_sdk::provider::RecordedModelRequest>,
) {
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();
    let verdict = classify_capability_request_with_provider(
        provider,
        reasoning,
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
    (verdict, provider.recorded_requests())
}

// Covers: the screen decides whether the reasoned review runs, and only the review can deny
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn screen_result_decides_whether_review_runs() {
    let deny_json = r#"{"decision":"deny","reason":"outside user intent"}"#;
    let cases: Vec<(&str, Vec<ScriptedTurn>, ClassifierVerdict, usize)> = vec![
        (
            "screen allow skips review",
            vec![text_turn("allow")],
            ClassifierVerdict::Allow,
            1,
        ),
        (
            "screen escalate reaches review allow",
            vec![text_turn("escalate"), text_turn(r#"{"decision":"allow"}"#)],
            ClassifierVerdict::Allow,
            2,
        ),
        (
            "screen escalate reaches review deny",
            vec![text_turn("escalate"), text_turn(deny_json)],
            ClassifierVerdict::Deny {
                reason: "outside user intent".into(),
            },
            2,
        ),
        (
            "unreadable screen output escalates",
            vec![text_turn("hmm, maybe?"), text_turn(deny_json)],
            ClassifierVerdict::Deny {
                reason: "outside user intent".into(),
            },
            2,
        ),
        (
            "screen provider error still reaches review",
            vec![failed_turn(), text_turn(r#"{"decision":"allow"}"#)],
            ClassifierVerdict::Allow,
            2,
        ),
        (
            "review reasoning before the verdict still parses",
            vec![
                text_turn("escalate"),
                text_turn(&format!("The write is in scope.\n{deny_json}")),
            ],
            ClassifierVerdict::Deny {
                reason: "outside user intent".into(),
            },
            2,
        ),
        (
            "unparseable review fails closed",
            vec![text_turn("escalate"), text_turn("not json")],
            unavailable(),
            2,
        ),
        (
            "review provider error fails closed",
            vec![text_turn("escalate"), failed_turn()],
            unavailable(),
            2,
        ),
    ];

    for (name, turns, expected, expected_requests) in cases {
        let provider = ScriptedProvider::new(ModelIdentity::new("provider", "api", "model"), turns);
        let (verdict, requests) = run_pipeline(&provider, ReasoningLevel::Medium).await;
        assert_eq!(verdict, expected, "{name}");
        assert_eq!(requests.len(), expected_requests, "{name}");
    }
}

// Covers: the screen stays at Low while the review uses configured reasoning; transcript blocks match
// Owner: permission classifier two-stage pipeline
#[tokio::test]
async fn screen_stays_low_reasoning_while_review_uses_configured_level() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("provider", "api", "model"),
        [text_turn("escalate"), text_turn(r#"{"decision":"allow"}"#)],
    );
    let transcript = render_classifier_transcript(&sample_history(), &pending_write()).unwrap();

    let (verdict, requests) = run_pipeline(&provider, ReasoningLevel::High).await;

    assert_eq!(verdict, ClassifierVerdict::Allow);
    assert_eq!(
        requests[0].messages,
        [
            Message::System(CLASSIFIER_PROMPT.into()),
            Message::User(vec![
                ContentBlock::Text(transcript.clone()),
                ContentBlock::Text(CLASSIFIER_SCREEN_INSTRUCTION.into()),
            ]),
        ]
    );
    assert_eq!(
        requests[1].messages,
        [
            Message::System(CLASSIFIER_PROMPT.into()),
            Message::User(vec![
                ContentBlock::Text(transcript),
                ContentBlock::Text(CLASSIFIER_REVIEW_INSTRUCTION.into()),
            ]),
        ]
    );
    assert_eq!(requests[0].reasoning_level, ReasoningLevel::Low);
    assert_eq!(requests[1].reasoning_level, ReasoningLevel::High);
    assert!(requests[0].tools.is_empty());
    assert!(requests[1].tools.is_empty());
}

// Covers: an unset classifier model cannot fall back to the executor model
// Owner: permission classifier model resolution
#[tokio::test]
async fn missing_classifier_model_fails_closed() {
    assert_eq!(verdict_for_config(Config::default()).await, unavailable());
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

    assert_eq!(verdict_for_config(config).await, unavailable());
}

async fn verdict_for_config(config: Config) -> ClassifierVerdict {
    let history = sample_history();
    let pending = pending_write();
    let session_id = SessionId::new();
    classify_capability_request(
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
    .await
}
