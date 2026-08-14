use agent_client_protocol::schema::v1::{
    ContentBlock, SessionId, SessionUpdate, StopReason, ToolCallStatus, ToolKind,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock as SdkContentBlock, Message, ToolCall},
    tool::{OperationKind, ToolMetadata, ToolOutput, ToolProgress},
    ProviderStreamResetReason, Revision, RunEvent, RunId, ToolCallId, ToolCompletion,
};
use serde_json::json;

use super::{map_sdk_stop_reason, EventMapper, PROVIDER_STREAM_RESET_NOTICE};

fn session_id() -> SessionId {
    SessionId::new("session-1")
}

fn only_update(mapper: &mut EventMapper, event: RunEvent) -> SessionUpdate {
    mapper
        .map_event(&session_id(), &event)
        .expect("one notification")
        .update
}

fn text_of(block: &ContentBlock) -> &str {
    match block {
        ContentBlock::Text(text) => text.text.as_str(),
        other => panic!("expected text content, got {other:?}"),
    }
}

// Covers: streamed assistant tokens must surface as ACP agent message chunks
// Owner: acp event mapper
#[test]
fn assistant_delta_maps_to_agent_message_chunk() {
    let update = only_update(
        &mut EventMapper::new(),
        RunEvent::AssistantTextDelta { text: "hi".into() },
    );
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => assert_eq!(text_of(&chunk.content), "hi"),
        other => panic!("expected AgentMessageChunk, got {other:?}"),
    }
}

// Covers: reasoning and reasoning-summary deltas share the thought channel
// Owner: acp event mapper
#[test]
fn reasoning_deltas_map_to_thought_chunks() {
    for event in [
        RunEvent::ReasoningDelta {
            text: "think".into(),
        },
        RunEvent::ReasoningSummaryDelta {
            text: "think".into(),
        },
    ] {
        let update = only_update(&mut EventMapper::new(), event);
        match update {
            SessionUpdate::AgentThoughtChunk(chunk) => assert_eq!(text_of(&chunk.content), "think"),
            other => panic!("expected AgentThoughtChunk, got {other:?}"),
        }
    }
}

// Covers: proposed args must attach to the first ToolCall, not a premature update
// Owner: acp event mapper
#[test]
fn proposed_started_finished_map_kind_and_status() {
    let mut mapper = EventMapper::new();
    let call_id = ToolCallId::from_string("call-1").unwrap();
    let proposed = mapper.map_event(
        &session_id(),
        &RunEvent::ToolProposed {
            call: ToolCall {
                id: "call-1".into(),
                name: "write".into(),
                arguments: json!({"path": "a.rs"}),
            },
        },
    );
    assert_eq!(proposed, None);

    let started = only_update(
        &mut mapper,
        RunEvent::ToolStarted {
            call_id: call_id.clone(),
            name: "write".into(),
            metadata: ToolMetadata::new()
                .operation(OperationKind::Write)
                .affected_path("a.rs")
                .command_summary("patch a.rs"),
        },
    );
    match started {
        SessionUpdate::ToolCall(tool) => {
            assert_eq!(tool.tool_call_id.0.as_ref(), "call-1");
            assert_eq!(tool.kind, ToolKind::Edit);
            assert_eq!(tool.status, ToolCallStatus::InProgress);
            assert_eq!(tool.raw_input, Some(json!({"path": "a.rs"})));
            assert_eq!(
                tool.locations
                    .iter()
                    .map(|location| location.path.clone())
                    .collect::<Vec<_>>(),
                vec![std::path::PathBuf::from("a.rs")]
            );
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }

    let finished = only_update(
        &mut mapper,
        RunEvent::ToolFinished {
            call_id,
            result: ToolCompletion::Success(
                ToolOutput::text("ok").metadata(ToolMetadata::new().diff("diff --git a/a.rs")),
            ),
        },
    );
    match finished {
        SessionUpdate::ToolCallUpdate(update) => {
            assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
            assert_eq!(update.fields.content.as_ref().map(Vec::len), Some(2));
        }
        other => panic!("expected ToolCallUpdate, got {other:?}"),
    }
}

// Covers: ACP cannot retract streamed text, so a reset must emit the discard notice
// Owner: acp event mapper
#[test]
fn provider_stream_reset_emits_discard_thought() {
    let update = only_update(
        &mut EventMapper::new(),
        RunEvent::ProviderStreamReset {
            reason: ProviderStreamResetReason::InvalidResponse,
            detail: "retry".into(),
        },
    );
    match update {
        SessionUpdate::AgentThoughtChunk(chunk) => {
            assert_eq!(text_of(&chunk.content), PROVIDER_STREAM_RESET_NOTICE);
        }
        other => panic!("expected AgentThoughtChunk, got {other:?}"),
    }
}

// Covers: host-only or deprecated run events must not become session updates
// Owner: acp event mapper
#[test]
fn ignored_events_emit_nothing() {
    let mut mapper = EventMapper::new();
    let ignored = [
        RunEvent::Started {
            run_id: RunId::from_string("run-1").unwrap(),
            revision: Revision::INITIAL,
        },
        RunEvent::StepStarted { step: 1 },
        RunEvent::ToolCallUpdated {
            index: 0,
            id: Some("call-1".into()),
            name: Some("write".into()),
            arguments_delta: "{".into(),
        },
        RunEvent::UsageUpdated {
            usage: rho_sdk::model::ModelUsage {
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
                total_tokens: None,
                context_window: None,
                cost_usd_micros: None,
            },
        },
        RunEvent::Cancelled {
            revision: Revision::INITIAL,
        },
        RunEvent::Failed {
            message: "boom".into(),
            retryability: rho_sdk::Retryability::Permanent,
        },
        RunEvent::SteeringApplied { ids: Vec::new() },
        RunEvent::ProviderRequestRetry,
        RunEvent::WebSearch {
            detail: "search".into(),
        },
        RunEvent::ContextEstimated { tokens: 12 },
        RunEvent::HostedToolActivity {
            name: "x_search".into(),
            detail: "hit".into(),
        },
    ];
    for event in ignored {
        assert_eq!(mapper.map_event(&session_id(), &event), None);
    }
}

// Covers: successful SDK stop reasons must land on the matching ACP prompt stop
// Owner: acp event mapper
#[test]
fn map_stop_maps_sdk_stop_reasons() {
    // RunOutcome::new is crate-private; the public mapper only reads stop_reason().
    let cases = [
        (rho_sdk::StopReason::EndTurn, StopReason::EndTurn),
        (rho_sdk::StopReason::MaxSteps, StopReason::MaxTurnRequests),
    ];
    for (sdk, expected) in cases {
        assert_eq!(map_sdk_stop_reason(sdk), expected);
    }
}

// Covers: load_session replay must emit user/assistant text and skip tool-only rows
// Owner: acp event mapper
#[test]
fn replay_history_emits_user_and_assistant_text() {
    let notifications = EventMapper::replay_history(
        &session_id(),
        &[
            Message::user_text("hello"),
            Message::assistant_text("world"),
            Message::Assistant(vec![SdkContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read".into(),
                arguments: json!({}),
            })]),
        ],
    );
    assert_eq!(notifications.len(), 2);
    match &notifications[0].update {
        SessionUpdate::UserMessageChunk(chunk) => assert_eq!(text_of(&chunk.content), "hello"),
        other => panic!("expected UserMessageChunk, got {other:?}"),
    }
    match &notifications[1].update {
        SessionUpdate::AgentMessageChunk(chunk) => assert_eq!(text_of(&chunk.content), "world"),
        other => panic!("expected AgentMessageChunk, got {other:?}"),
    }
}

// Covers: in-flight progress must stay in_progress and carry the progress text
// Owner: acp event mapper
#[test]
fn tool_updated_is_in_progress() {
    let update = only_update(
        &mut EventMapper::new(),
        RunEvent::ToolUpdated {
            call_id: ToolCallId::from_string("call-1").unwrap(),
            progress: ToolProgress::message("working"),
        },
    );
    match update {
        SessionUpdate::ToolCallUpdate(update) => {
            assert_eq!(update.fields.status, Some(ToolCallStatus::InProgress));
            let content = update.fields.content.expect("progress content");
            assert_eq!(content.len(), 1);
            match &content[0] {
                agent_client_protocol::schema::v1::ToolCallContent::Content(block) => {
                    assert_eq!(text_of(&block.content), "working");
                }
                other => panic!("expected progress text, got {other:?}"),
            }
        }
        other => panic!("expected ToolCallUpdate, got {other:?}"),
    }
}
