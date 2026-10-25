use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    CompactionStatus, CompactionUpdate, ContentBlock, SessionId, SessionUpdate, StopReason,
    ToolCallStatus, ToolKind,
};
use pretty_assertions::assert_eq;
use rho_sdk::{
    model::ToolCall,
    tool::{OperationKind, ToolMetadata, ToolOutput, ToolProgress},
    ProviderStreamResetReason, Revision, RunEvent, RunId, ToolCallId, ToolCompletion,
};
use serde_json::json;

use super::{
    map_sdk_stop_reason, EventMapper, HostFeatures, RunEnding, PROVIDER_STREAM_RESET_NOTICE,
};

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
    let mut mapper = EventMapper::new(HostFeatures::default(), PathBuf::new(), Default::default());
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

// Covers: streamed text surfaces as agent message chunks; reasoning, reasoning
// summaries, and the stream-reset discard notice (ACP cannot retract streamed
// text) share the thought channel
// Owner: acp event mapper
#[test]
fn stream_events_map_to_message_and_thought_chunks() {
    for (event, thought, text) in [
        (
            RunEvent::AssistantTextDelta { text: "hi".into() },
            false,
            "hi",
        ),
        (
            RunEvent::ReasoningDelta {
                text: "think".into(),
            },
            true,
            "think",
        ),
        (
            RunEvent::ReasoningSummaryDelta {
                text: "think".into(),
            },
            true,
            "think",
        ),
        (
            RunEvent::ProviderStreamReset {
                reason: ProviderStreamResetReason::InvalidResponse,
                detail: "retry".into(),
            },
            true,
            PROVIDER_STREAM_RESET_NOTICE,
        ),
    ] {
        let update = only_update(
            &mut EventMapper::new(HostFeatures::default(), PathBuf::new(), Default::default()),
            event,
        );
        match (thought, update) {
            (false, SessionUpdate::AgentMessageChunk(chunk))
            | (true, SessionUpdate::AgentThoughtChunk(chunk)) => {
                assert_eq!(text_of(&chunk.content), text);
            }
            (_, other) => panic!("unexpected update {other:?} for {text:?}"),
        }
    }
}

// Covers: proposed args must attach to the first ToolCall, not a premature update
// Owner: acp event mapper
#[test]
fn proposed_started_finished_map_kind_and_status() {
    let mut mapper = EventMapper::new(HostFeatures::default(), PathBuf::new(), Default::default());
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
            // A unified diff is patch text, never ACP Diff.new_text (file contents).
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

// Covers: hosts such as Zed list each call by its title, so a started call
// must say what it does, as the interactive card header does, with paths
// relative to the session directory, rather than only naming the tool. Zed
// shows an execute title as the command, so a shell call's title is bare,
// while execute tools whose summary is display copy keep their tool name.
// A title is sent once, so it never carries a live phase such as "starting".
// Owner: acp event mapper
#[test]
fn started_call_titles_describe_the_call() {
    let cases = [
        (
            "read_file",
            Some(json!({"path": "/work/src/a.rs", "offset": 10, "limit": 5})),
            ToolMetadata::new(),
            "read_file(src/a.rs:10-14)",
        ),
        (
            "grep",
            Some(json!({"pattern": "auth", "path": "crates"})),
            ToolMetadata::new(),
            "grep(auth, crates)",
        ),
        (
            "bash",
            Some(json!({"command": "cargo test -p rho"})),
            ToolMetadata::new(),
            "cargo test -p rho",
        ),
        (
            "bash",
            None,
            ToolMetadata::new()
                .operation(OperationKind::Execute)
                .command_line("cargo test"),
            "cargo test",
        ),
        (
            "computer",
            Some(json!({})),
            ToolMetadata::new()
                .operation(OperationKind::Execute)
                .command_summary("click · x 4"),
            "computer(click · x 4)",
        ),
        (
            "custom",
            Some(json!({})),
            ToolMetadata::new().command_summary("sync docs"),
            "custom(sync docs)",
        ),
        ("read_file", None, ToolMetadata::new(), "read_file"),
        ("bash", None, ToolMetadata::new(), "bash"),
        // Names the agent, not the "starting" phase the title would keep.
        (
            "agent",
            Some(json!({"agent_id": "explorer", "prompt": "look"})),
            ToolMetadata::new(),
            "explorer",
        ),
        // Keeps the server, so it cannot pass for a built-in tool.
        (
            "mcp__fs__search",
            Some(json!({"query": "auth"})),
            ToolMetadata::new().command_summary("npx (1 arguments)"),
            "fs/search(auth)",
        ),
    ];
    for (name, arguments, metadata, expected) in cases {
        let mut mapper = EventMapper::new(
            HostFeatures::default(),
            PathBuf::from("/work"),
            Default::default(),
        );
        if let Some(arguments) = arguments {
            mapper.map_event(
                &session_id(),
                &RunEvent::ToolProposed {
                    call: ToolCall {
                        id: "call-1".into(),
                        name: name.into(),
                        arguments,
                    },
                },
            );
        }
        let started = only_update(
            &mut mapper,
            RunEvent::ToolStarted {
                call_id: ToolCallId::from_string("call-1").unwrap(),
                name: name.into(),
                metadata,
            },
        );
        match started {
            SessionUpdate::ToolCall(tool) => assert_eq!(tool.title, expected, "{name}"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }
}

// Covers: run events ACP has no session update for must not become session
// updates, including compaction for a host that did not advertise it
// Owner: acp event mapper
#[test]
fn ignored_events_emit_nothing() {
    let mut mapper = EventMapper::new(HostFeatures::default(), PathBuf::new(), Default::default());
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
        RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            message_count: 2,
        },
    ];
    for event in ignored {
        assert_eq!(mapper.map_event(&session_id(), &event), Vec::new());
    }
}

// Covers: a compaction card still open when the run ends must close with
// the run's ending and error, never stay in progress on the host
// Owner: acp event mapper
#[test]
fn compaction_open_at_run_end_closes_with_the_ending() {
    let compaction_host = HostFeatures {
        compaction: true,
        ..HostFeatures::default()
    };
    for (label, ending, status, error) in [
        (
            "cancelled",
            Some(RunEvent::Cancelled {
                revision: Revision::INITIAL,
            }),
            CompactionStatus::Cancelled,
            None,
        ),
        (
            "failed",
            Some(RunEvent::Failed {
                message: "boom".into(),
                retryability: rho_sdk::Retryability::Permanent,
                revision: Revision::INITIAL,
            }),
            CompactionStatus::Failed,
            Some("boom"),
        ),
        (
            "stream ended",
            None,
            CompactionStatus::Failed,
            Some("compaction ended without a result"),
        ),
    ] {
        let mut mapper = EventMapper::new(compaction_host, PathBuf::new(), Default::default());
        let SessionUpdate::CompactionUpdate(opened) = only_update(
            &mut mapper,
            RunEvent::CompactionStarted {
                trigger: rho_sdk::CompactionTrigger::Automatic,
                message_count: 2,
            },
        ) else {
            panic!("{label}: expected a compaction card");
        };
        let closing = match ending {
            Some(event) => mapper.map_event(&session_id(), &event),
            None => mapper.close_unfinished(&session_id(), RunEnding::WithoutResult),
        };
        let mut expected = CompactionUpdate::new(opened.compaction_id, status);
        if let Some(error) = error {
            expected = expected.error(error.to_owned());
        }
        let closed = closing
            .into_iter()
            .map(|notification| notification.update)
            .collect::<Vec<_>>();
        assert_eq!(
            closed,
            vec![SessionUpdate::CompactionUpdate(expected)],
            "{label}"
        );
        assert_eq!(
            mapper.close_unfinished(&session_id(), RunEnding::WithoutResult),
            Vec::new(),
            "{label}: closes once"
        );
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
        (rho_sdk::StopReason::MaxTokens, StopReason::MaxTokens),
        (rho_sdk::StopReason::Refusal, StopReason::Refusal),
    ];
    for (sdk, expected) in cases {
        assert_eq!(map_sdk_stop_reason(sdk), expected);
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
    let cards = EventMapper::replay_history(
        &session_id(),
        &[crate::session::tree::ProjectedMessage {
            origin: Some(crate::session::tree::DisplayRowOrigin {
                node: crate::session::tree::NodeId::from_string("node").unwrap(),
                row: 0,
            }),
            message: transcript.display_message(),
        }],
        Path::new(""),
        &Default::default(),
    )
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

// Covers: live chunks carry a message ID that continues across chunks of one
// kind and changes at each boundary: thought to text, a tool call between
// texts, a new model step, and the stream-reset notice and the retry after
// it. Updates that send nothing, such as usage, do not split a message.
// Owner: acp event mapper
#[test]
fn chunks_share_a_message_id_until_a_boundary() {
    let text = |text: &str| RunEvent::AssistantTextDelta { text: text.into() };
    let step = |step| RunEvent::StepStarted {
        step,
        estimated_context_tokens: 0,
    };
    let events = [
        RunEvent::ReasoningDelta {
            text: "plan".into(),
        },
        RunEvent::ReasoningSummaryDelta {
            text: " more".into(),
        },
        text("look"),
        text(" now"),
        RunEvent::ToolStarted {
            call_id: ToolCallId::from_string("call-1").unwrap(),
            name: "read_file".into(),
            metadata: ToolMetadata::new(),
        },
        text("found"),
        step(2),
        text("again"),
        RunEvent::UsageUpdated {
            usage: rho_sdk::model::ModelUsage::default(),
        },
        text(" still"),
        RunEvent::ProviderStreamReset {
            reason: ProviderStreamResetReason::InvalidResponse,
            detail: "retry".into(),
        },
        text("retried"),
    ];
    let mut mapper = EventMapper::new(HostFeatures::default(), PathBuf::new(), Default::default());
    let mut labels = std::collections::HashMap::new();
    let chunks = events
        .iter()
        .flat_map(|event| mapper.map_event(&session_id(), event))
        .filter_map(|notification| match notification.update {
            SessionUpdate::AgentMessageChunk(chunk) | SessionUpdate::AgentThoughtChunk(chunk) => {
                let id = chunk.message_id.expect("live chunk has a message ID");
                let next = labels.len();
                let label = *labels.entry(id).or_insert(next);
                Some((text_of(&chunk.content).to_owned(), label))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        chunks,
        [
            ("plan", 0),
            (" more", 0),
            ("look", 1),
            (" now", 1),
            ("found", 2),
            ("again", 3),
            (" still", 3),
            (PROVIDER_STREAM_RESET_NOTICE, 4),
            ("retried", 5),
        ]
        .map(|(text, label)| (text.to_owned(), label))
    );
}
