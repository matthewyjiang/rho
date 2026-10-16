use agent_client_protocol::schema::v1::{
    ContentBlock, SessionId, SessionUpdate, StopReason, ToolCallStatus, ToolKind,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolCall,
    tool::{OperationKind, ToolMetadata, ToolOutput, ToolProgress},
    ProviderStreamResetReason, Revision, RunEvent, RunId, ToolCallId, ToolCompletion,
};
use serde_json::json;

use super::{map_sdk_stop_reason, EventMapper, PROVIDER_STREAM_RESET_NOTICE};

fn session_id() -> SessionId {
    SessionId::new("session-1")
}

fn only_update(mapper: &mut EventMapper, event: RunEvent) -> SessionUpdate {
    let mut notifications = mapper.map_event(&session_id(), &event);
    assert_eq!(notifications.len(), 1, "one notification for {event:?}");
    notifications.remove(0).update
}

/// A mapper that has already sent the host a `tool_call` for `call-1`.
fn mapper_with_started_call() -> EventMapper {
    let mut mapper = EventMapper::new();
    only_update(
        &mut mapper,
        RunEvent::ToolStarted {
            call_id: ToolCallId::from_string("call-1").unwrap(),
            name: "write".into(),
            metadata: ToolMetadata::new(),
        },
    );
    mapper
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
    assert_eq!(proposed, Vec::new());

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
            assert_eq!(tool.name.as_deref(), Some("write"));
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

// Covers: run events ACP has no session update for must not become session updates
// Owner: acp event mapper
#[test]
fn ignored_events_emit_nothing() {
    let mut mapper = EventMapper::new();
    let ignored = [
        RunEvent::Started {
            run_id: RunId::from_string("run-1").unwrap(),
            revision: Revision::INITIAL,
        },
        RunEvent::StepStarted {
            step: 1,
            estimated_context_tokens: 0,
        },
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
            revision: Revision::INITIAL,
        },
        RunEvent::SteeringApplied { ids: Vec::new() },
        RunEvent::ProviderRequestRetry,
        RunEvent::WebSearch {
            detail: "search".into(),
        },
        RunEvent::HostedToolActivity {
            name: "x_search".into(),
            detail: "hit".into(),
        },
    ];
    for event in ignored {
        assert_eq!(mapper.map_event(&session_id(), &event), Vec::new());
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

// Covers: a unified diff must not be sent as ACP Diff.new_text (file contents)
// Owner: acp event mapper
#[test]
fn finished_unified_diff_is_patch_text() {
    let finished = only_update(
        &mut mapper_with_started_call(),
        RunEvent::ToolFinished {
            call_id: ToolCallId::from_string("call-1").unwrap(),
            result: ToolCompletion::Success(
                ToolOutput::text("ok").metadata(
                    ToolMetadata::new()
                        .affected_path("a.rs")
                        .diff("diff --git a/a.rs"),
                ),
            ),
        },
    );
    match finished {
        SessionUpdate::ToolCallUpdate(update) => {
            let content = update.fields.content.expect("finished content");
            assert_eq!(content.len(), 2);
            match &content[1] {
                agent_client_protocol::schema::v1::ToolCallContent::Content(block) => {
                    assert_eq!(text_of(&block.content), "diff --git a/a.rs");
                }
                other => panic!("expected patch text, got {other:?}"),
            }
        }
        other => panic!("expected ToolCallUpdate, got {other:?}"),
    }
}

// Covers: in-flight progress must stay in_progress and carry the progress text,
// and progress after the result must not reopen the finished card
// Owner: acp event mapper
#[test]
fn tool_updated_is_in_progress() {
    let mut mapper = mapper_with_started_call();
    let call_id = ToolCallId::from_string("call-1").unwrap();
    let update = only_update(
        &mut mapper,
        RunEvent::ToolUpdated {
            call_id: call_id.clone(),
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

    // Progress that arrives after the result must not reopen the host's card.
    only_update(
        &mut mapper,
        RunEvent::ToolFinished {
            call_id: call_id.clone(),
            result: ToolCompletion::Success(ToolOutput::text("ok")),
        },
    );
    let late = RunEvent::ToolUpdated {
        call_id,
        progress: ToolProgress::message("late"),
    };
    assert_eq!(mapper.map_event(&session_id(), &late), Vec::new());
}

// Covers: background results the model was given must replay as finished tool
// cards, failed when the result failed, not as agent text merged into replies.
// Process output is fenced in the markdown content, with a fence longer than
// any inside it, and left unfenced in the plain `rawOutput` that hosts like
// T3 Code show as is.
// Owner: acp event mapper
#[test]
fn replayed_display_rows_become_finished_tool_cards() {
    use crate::{
        display_transcript::{DisplayRow, DisplayTranscript},
        presentation::{
            NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
            NotificationVisibility,
        },
    };

    let failed = NotificationCard {
        title: "Failed (exit 1) · cargo test".into(),
        sender: "process".into(),
        recipient: "parent".into(),
        delivery: NotificationDelivery::Received,
        tone: NotificationTone::Error,
        preview: NotificationPreview::Truncated,
        visibility: NotificationVisibility::Conversation,
        reference: Some("p1".into()),
        subtitle: Some("ran 3s".into()),
        body: "1 test failed".into(),
        output: Some("```rust\npanic!()\n```".into()),
        details: Vec::new(),
    };
    let transcript = DisplayTranscript(vec![
        DisplayRow::Notification(Box::new(failed)),
        DisplayRow::Notice("workflow wf (fixture) finished - success".into()),
    ]);
    let cards = EventMapper::replay_history(&session_id(), &[transcript.display_message()])
        .into_iter()
        .map(|notification| match notification.update {
            SessionUpdate::ToolCall(call) => (
                call.title,
                call.kind,
                call.status,
                call.content
                    .iter()
                    .map(|content| match content {
                        agent_client_protocol::schema::v1::ToolCallContent::Content(block) => {
                            text_of(&block.content).to_owned()
                        }
                        other => panic!("expected text content, got {other:?}"),
                    })
                    .collect::<Vec<_>>(),
                call.raw_output,
                notification.meta.is_some(),
            ),
            other => panic!("expected ToolCall, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        cards,
        vec![
            (
                "Failed (exit 1) · cargo test".to_owned(),
                ToolKind::Other,
                ToolCallStatus::Failed,
                vec!["ran 3s\n1 test failed\n\n````text\n```rust\npanic!()\n```\n````".to_owned()],
                Some(json!("ran 3s\n1 test failed\n\n```rust\npanic!()\n```")),
                true,
            ),
            (
                "workflow wf (fixture) finished - success".to_owned(),
                ToolKind::Other,
                ToolCallStatus::Completed,
                Vec::new(),
                None,
                true,
            ),
        ]
    );
}
