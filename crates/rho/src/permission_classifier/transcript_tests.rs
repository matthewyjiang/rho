use pretty_assertions::assert_eq;
use rho_providers::model::{
    AssistantMessage, ContentBlock, ImageContent, Message, ToolCall, ToolResult,
};
use rho_sdk::{ApprovalRequest, CapabilityRequest, CapabilitySource, PathScope};

use super::render_classifier_transcript;
use super::CONSECUTIVE_DENY_ESCALATION;

fn source(name: &str) -> CapabilitySource {
    CapabilitySource::built_in_tool(name)
}

fn sample_history() -> Vec<Message> {
    vec![
        Message::User(vec![ContentBlock::Text("please update the config".into())]),
        Message::Assistant(vec![
            ContentBlock::Text("I'll inspect the file first.".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "config.toml"}),
            }),
        ]),
        Message::assistant(AssistantMessage {
            content: vec![
                ContentBlock::Text("Checking write access.".into()),
                ContentBlock::ToolCall(ToolCall {
                    id: "call-2".into(),
                    name: "write_file".into(),
                    arguments: serde_json::json!({"path": "config.toml", "content": "x=1"}),
                }),
            ],
            reasoning_summary: Some("planning a safe edit".into()),
            ..AssistantMessage::default()
        }),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "file contents with secrets".into(),
        }),
        Message::User(vec![ContentBlock::Image(ImageContent {
            data: "base64-data".into(),
            mime_type: "image/png".into(),
        })]),
    ]
}

#[test]
fn transcript_keeps_user_text_and_tool_calls_only() {
    let pending = ApprovalRequest::new(
        CapabilityRequest::write_path(
            "config.toml",
            PathScope::PrimaryWorkspace,
            source("write_file"),
        ),
        "agent requested a write after reading config",
    );
    let transcript = render_classifier_transcript(&sample_history(), &pending);

    assert!(transcript.contains("please update the config"));
    assert!(transcript.contains("read_file"));
    assert!(transcript.contains(r#""path":"config.toml""#));
    assert!(transcript.contains("write_file"));
    assert!(transcript.contains(r#""content":"x=1""#));
    assert!(transcript.contains("[image omitted]"));

    assert!(!transcript.contains("I'll inspect the file first."));
    assert!(!transcript.contains("Checking write access."));
    assert!(!transcript.contains("planning a safe edit"));
    assert!(!transcript.contains("file contents with secrets"));
}

#[test]
fn transcript_appends_pending_capability_details_at_end() {
    let pending = ApprovalRequest::new(
        CapabilityRequest::write_path(
            "config.toml",
            PathScope::PrimaryWorkspace,
            source("write_file"),
        ),
        "agent requested a write after reading config",
    );
    let transcript = render_classifier_transcript(&sample_history(), &pending);

    let pending_section = transcript
        .split("pending_capability:")
        .nth(1)
        .expect("pending capability section");
    assert!(pending_section.contains("write"));
    assert!(pending_section.contains("config.toml"));
    assert!(pending_section.contains("write_file"));
    assert!(pending_section.contains("agent requested a write after reading config"));
    assert_eq!(
        transcript.rfind("pending_capability:"),
        transcript.find("pending_capability:")
    );
}

#[test]
fn consecutive_deny_escalation_is_three() {
    assert_eq!(CONSECUTIVE_DENY_ESCALATION, 3);
}
