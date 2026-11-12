use std::{path::Path, sync::Arc};

use pretty_assertions::assert_eq;
use rho_protocol::{
    CardTone, ContentBlock, MessageId, PlanEntry, PlanStatus, SessionEvent as WireEvent,
    ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
use rho_sdk::{
    model::{self, AbortedAssistant, AssistantMessage, Message, ToolResult},
    tool::{ToolMetadata, ToolOutput},
    ContextEstimate, RunEvent, ToolCompletion, ToolExecution, UserInput,
};
use serde_json::json;

use super::{SavedTranscript, SavedTurn};
use crate::{
    agent::{AgentCapabilities, ToolCapability},
    app::{
        session_events::{
            test_support::{
                canonical_messages, closed, opened, origin, shell_call, text, updated,
                without_display,
            },
            CallIds, LiveEvents,
        },
        session_runtime::{InsertedMessage, SessionEvent, SessionTools},
    },
    config::Config,
    diagnostics::RuntimeDiagnostics,
    display_transcript::{DisplayRow, DisplayTranscript},
    session::tree::ProjectedMessage,
    tools::{
        sdk_registry::{AppToolSet, ToolSetOptions},
        todo::{TodoItem, TodoList, TodoStatus},
    },
};

/// `messages` as display rows, each saved as its own row.
fn saved_rows(messages: &[Message]) -> Vec<ProjectedMessage> {
    messages
        .iter()
        .enumerate()
        .map(|(row, message)| ProjectedMessage {
            origin: Some(origin(row)),
            message: message.clone(),
        })
        .collect()
}

/// The events a client that resets replays once `messages` saved as one
/// turn with the tools of `tool_set`.
fn replay_with(tool_set: &AppToolSet, messages: &[Message]) -> Vec<WireEvent> {
    let mut transcript = SavedTranscript::default();
    transcript.push(SavedTurn::capture(
        &saved_rows(messages),
        Path::new("/work"),
        SessionTools::of_tool_set_for_tests(tool_set),
        &mut CallIds::default(),
    ));
    transcript.events().collect()
}

fn replay_messages(messages: &[Message]) -> Vec<WireEvent> {
    replay_with(&AppToolSet::disabled(), messages)
}

fn call_block(id: &str) -> model::ContentBlock {
    model::ContentBlock::ToolCall(shell_call(id, "ls"))
}

fn saved_id(row: usize, part: u32) -> MessageId {
    MessageId::new(format!("saved-node-{row}-{part}"))
}

fn user(message: MessageId, text: &str) -> WireEvent {
    WireEvent::UserMessage {
        message,
        content: vec![ContentBlock::Text { text: text.into() }],
        steer: None,
    }
}

fn saved_agent(message: MessageId, text: &str) -> WireEvent {
    WireEvent::AgentMessageChunk {
        message,
        text: text.into(),
    }
}

fn image() -> model::ImageContent {
    model::ImageContent {
        data: "YWJj".into(),
        mime_type: "image/png".into(),
    }
}

fn image_content() -> ToolContent {
    ToolContent::Image {
        mime_type: "image/png".into(),
        data: "YWJj".into(),
    }
}

fn background_transcript() -> DisplayTranscript {
    DisplayTranscript(vec![DisplayRow::Notice("Background done\nall good".into())])
}

/// The conversation both the replay and the agreement tests use, as saved.
fn saved_conversation() -> Vec<Message> {
    vec![
        Message::user_text("list files"),
        Message::Assistant(
            vec![
                model::ContentBlock::Text("Let me look.".into()),
                call_block("call-1"),
            ]
            .into(),
        ),
        Message::ToolResult(ToolResult::succeeded("call-1", "a.rs").into()),
        background_transcript().display_message(),
        Message::assistant_text("Found a.rs."),
    ]
}

// Covers: a saved conversation replays whole messages with ids from the
// rows they were saved in, a call with its provider id that opens with its
// card and finishes with its result, and a background result's rows as
// cards, in saved order.
// Owner: session event replay
#[test]
fn a_saved_conversation_replays_messages_calls_and_cards() {
    assert_eq!(
        without_display(replay_messages(&saved_conversation())),
        vec![
            user(saved_id(0, 0), "list files"),
            saved_agent(saved_id(1, 0), "Let me look."),
            opened("call-1", "ls"),
            updated("call-1", ToolStatus::Completed, vec![text("a.rs")]),
            WireEvent::Card {
                title: "Background done".into(),
                markdown: "all good".into(),
                plain: "all good".into(),
                tone: CardTone::Neutral,
                display: None,
            },
            saved_agent(saved_id(4, 0), "Found a.rs."),
        ]
    );
}

// Covers: a client sees the same conversation live and replayed: the same
// messages (the user message under the same id), the same cards and the
// same calls, drawn with the same TUI cards, apart from how live text was
// chunked and the usage only live steps report.
// Owner: session event live and replay mapping
#[test]
fn live_and_replayed_turns_agree() {
    let tool_set = AppToolSet::disabled();
    let mut live = LiveEvents::new(
        Path::new("/work"),
        SessionTools::of_tool_set_for_tests(&tool_set),
        /*configured_window*/ None,
    );
    live.prompt(&UserInput::text("list files"));
    let step = || {
        SessionEvent::Run(RunEvent::StepStarted {
            step: 0,
            context_estimate: ContextEstimate::from_estimated_tokens(10),
        })
    };
    let delta = |text: &str| SessionEvent::Run(RunEvent::AssistantTextDelta { text: text.into() });
    let call_id = rho_sdk::ToolCallId::from_string("call-1").unwrap();
    let events = [
        SessionEvent::MessageInserted(InsertedMessage::Saved(origin(0))),
        SessionEvent::RunStarted,
        step(),
        delta("Let me "),
        delta("look."),
        SessionEvent::Run(RunEvent::ToolProposed {
            call: shell_call("call-1", "ls"),
        }),
        SessionEvent::Run(RunEvent::ToolStarted {
            call_id: call_id.clone(),
            name: "bash".into(),
            metadata: ToolMetadata::new(),
            execution: ToolExecution::Attached,
        }),
        SessionEvent::Run(RunEvent::ToolFinished {
            call_id,
            result: ToolCompletion::Completed(ToolOutput::text("a.rs")),
        }),
        SessionEvent::BoundaryApplied {
            transcript: background_transcript(),
        },
        step(),
        delta("Found "),
        delta("a.rs."),
        SessionEvent::RunClosed,
        SessionEvent::RunFinished {
            context: ContextEstimate::from_estimated_tokens(10),
        },
    ];
    let live_events: Vec<_> = events.iter().flat_map(|event| live.map(event)).collect();
    let replayed = replay_messages(&saved_conversation());

    let users = |events: &[WireEvent]| -> Vec<WireEvent> {
        events
            .iter()
            .filter(|event| matches!(event, WireEvent::UserMessage { .. }))
            .cloned()
            .collect()
    };
    assert_eq!(users(&live_events), users(&replayed));
    let live_conversation = live_events
        .into_iter()
        .filter(|event| !matches!(event, WireEvent::Usage { .. } | WireEvent::ProviderRequest))
        .collect();
    assert_eq!(
        canonical_messages(merged_chunks(live_conversation)),
        canonical_messages(replayed)
    );
}

/// `events` with consecutive chunks of one message joined into one.
fn merged_chunks(events: Vec<WireEvent>) -> Vec<WireEvent> {
    let mut merged: Vec<WireEvent> = Vec::new();
    for event in events {
        let extends_last = match (merged.last_mut(), &event) {
            (
                Some(WireEvent::AgentMessageChunk { message, text }),
                WireEvent::AgentMessageChunk {
                    message: next,
                    text: more,
                },
            )
            | (
                Some(WireEvent::ThoughtChunk { message, text }),
                WireEvent::ThoughtChunk {
                    message: next,
                    text: more,
                },
            ) if message == next => {
                text.push_str(more);
                true
            }
            _ => false,
        };
        if !extends_last {
            merged.push(event);
        }
    }
    merged
}

// Covers: parallel calls keep their ids and order, a result lands on its own
// call whatever order results arrive in, and its images follow its output.
// Owner: session event replay
#[test]
fn replay_pairs_out_of_order_results_with_their_images() {
    for (result, status) in [
        (ToolResult::succeeded("b", "output"), ToolStatus::Completed),
        (ToolResult::failed("b", "output"), ToolStatus::Failed),
    ] {
        let messages = vec![
            Message::user_text("read images"),
            Message::Assistant(
                vec![
                    call_block("a"),
                    model::ContentBlock::Text("between".into()),
                    call_block("b"),
                ]
                .into(),
            ),
            Message::ToolResult(result.with_images(vec![image()]).into()),
            Message::ToolResult(ToolResult::succeeded("a", String::new()).into()),
        ];
        assert_eq!(
            without_display(replay_messages(&messages)),
            vec![
                user(saved_id(0, 0), "read images"),
                opened("a", "ls"),
                saved_agent(saved_id(1, 0), "between"),
                opened("b", "ls"),
                updated("b", status, vec![text("output"), image_content()]),
                updated("a", ToolStatus::Completed, Vec::new()),
            ]
        );
    }
}

// Covers: history a killed process left without results does not leave
// calls running, whichever kind of assistant message holds them, and a
// result without its call still opens a complete card.
// Owner: session event replay
#[test]
fn replay_closes_incomplete_calls_and_opens_orphan_results() {
    for message in [
        Message::Assistant(vec![call_block("a")].into()),
        Message::EnrichedAssistant(Arc::new(AssistantMessage::from_content(vec![call_block(
            "a",
        )]))),
        Message::AbortedAssistant(Arc::new(AbortedAssistant {
            content: vec![call_block("a")],
            ..Default::default()
        })),
    ] {
        assert_eq!(
            without_display(replay_messages(&[message])),
            vec![opened("a", "ls"), closed("a")]
        );
    }
    for (result, status) in [
        (
            ToolResult::succeeded("orphan", "output"),
            ToolStatus::Completed,
        ),
        (ToolResult::failed("orphan", "output"), ToolStatus::Failed),
    ] {
        assert_eq!(
            without_display(replay_messages(&[Message::ToolResult(result.into())])),
            vec![WireEvent::tool_call(ToolCallUpdate {
                id: ToolCallId::new("orphan"),
                title: Some("tool".into()),
                kind: Some(ToolKind::Other),
                status: Some(status),
                raw_input: None,
                content: Some(vec![text("output")]),
                locations: None,
                display: None,
            })]
        );
    }
}

// Covers: a provider id reused across turns opens a new call rather than
// updating the earlier one, an unfinished occurrence it supersedes closes,
// and later results and end-of-history closing reach the newest call.
// Owner: session event replay
#[test]
fn replay_gives_reused_provider_ids_their_own_calls() {
    let messages = vec![
        Message::Assistant(vec![call_block("reused")].into()),
        Message::Assistant(vec![call_block("reused")].into()),
        Message::ToolResult(
            ToolResult::failed("reused", "second output")
                .with_images(vec![image()])
                .into(),
        ),
        Message::Assistant(vec![call_block("reused")].into()),
    ];
    assert_eq!(
        without_display(replay_messages(&messages)),
        vec![
            opened("reused", "ls"),
            closed("reused"),
            opened("reused#2", "ls"),
            updated(
                "reused#2",
                ToolStatus::Failed,
                vec![text("second output"), image_content()]
            ),
            opened("reused#3", "ls"),
            closed("reused#3"),
        ]
    );
}

// Covers: replayed messages split where live ones do (text after a tool
// call, a reasoning summary), come in live order (the summary before the
// text it led to), and take ids from their saved row, so equal text saved
// twice is two messages.
// Owner: session event replay
#[test]
fn replayed_messages_split_where_live_ones_do() {
    let assistant = AssistantMessage {
        reasoning_summary: Some("why".into()),
        ..AssistantMessage::from_content(vec![
            model::ContentBlock::Text("before".into()),
            call_block("a"),
            model::ContentBlock::Text("after".into()),
            model::ContentBlock::Text(" more".into()),
        ])
    };
    let messages = vec![
        Message::user_text("continue"),
        Message::assistant(assistant),
        Message::ToolResult(ToolResult::succeeded("a", String::new()).into()),
        Message::user_text("continue"),
    ];
    assert_eq!(
        without_display(replay_messages(&messages)),
        vec![
            user(saved_id(0, 0), "continue"),
            WireEvent::ThoughtChunk {
                message: saved_id(1, 0),
                text: "why".into(),
            },
            saved_agent(saved_id(1, 1), "before"),
            opened("a", "ls"),
            saved_agent(saved_id(1, 2), "after more"),
            updated("a", ToolStatus::Completed, Vec::new()),
            user(saved_id(3, 0), "continue"),
        ]
    );
}

// Covers: a replayed call of a registered tool gets the kind its tool
// reports at start, as a live call does, including a tool that builds its
// metadata while preparing (`sessions`); an unknown tool stays `other`.
// Owner: session event replay; saved history keeps no tool metadata.
#[test]
fn replayed_calls_take_their_kind_from_the_session_tools() {
    let config = Config::default();
    let tool_set = AppToolSet::new(
        &config,
        RuntimeDiagnostics::new(&config),
        ToolSetOptions::new(AgentCapabilities::new(
            [
                ToolCapability::ReadFile,
                ToolCapability::Grep,
                ToolCapability::Sessions,
            ]
            .into(),
        )),
    );
    let call = |id: &str, name: &str, arguments| {
        model::ContentBlock::ToolCall(model::ToolCall::new(id.into(), name.into(), arguments))
    };
    let messages = [Message::assistant(AssistantMessage::from_content(vec![
        call("a", "read_file", json!({"path": "src/lib.rs"})),
        call("b", "grep", json!({"pattern": "fn main"})),
        call("s", "sessions", json!({"action": "search", "query": "x"})),
        call("c", "not_a_tool", json!({})),
    ]))];
    let kinds: Vec<_> = replay_with(&tool_set, &messages)
        .into_iter()
        .filter_map(|event| match event {
            WireEvent::ToolCall(update) => update.kind,
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ToolKind::Read,
            ToolKind::Search,
            ToolKind::Read,
            ToolKind::Other
        ]
    );
}

// Covers: a replay ends with the session's checklist, so a client that
// attaches sees the plan the live mapper counts as sent.
// Owner: session event replay
#[test]
fn replay_ends_with_the_current_plan() {
    let tool_set = AppToolSet::disabled();
    tool_set.todo_state().replace(Some(TodoList {
        todos: vec![TodoItem {
            content: "read".into(),
            status: TodoStatus::InProgress,
        }],
    }));
    assert_eq!(
        replay_with(&tool_set, &[Message::user_text("go")]),
        vec![
            user(saved_id(0, 0), "go"),
            WireEvent::Plan {
                entries: vec![PlanEntry {
                    content: "read".into(),
                    status: PlanStatus::InProgress,
                }],
            },
        ]
    );
}

// Covers: replaying the saved rows again after a turn that saved nothing
// yields nothing, so the daemon keeps that turn's live events in resets
// instead of cutting them for an empty save.
// Owner: session event live and replay mapping
#[test]
fn a_replay_with_no_new_rows_reports_nothing_saved() {
    let tool_set = AppToolSet::disabled();
    let tools = || SessionTools::of_tool_set_for_tests(&tool_set);
    let rows = saved_rows(&[Message::Assistant(vec![call_block("call")].into())]);
    let mut live = LiveEvents::new(Path::new("/work"), tools(), /*configured_window*/ None);
    assert!(live.replay(&rows, Path::new("/work"), tools()).is_some());
    assert!(live.replay(&rows, Path::new("/work"), tools()).is_none());
}

// Covers: a provider id the saved transcript already used is numbered live
// as replay numbers it, so the live call opens its own card rather than
// updating the saved one, and a client reset after the turn saves sees the
// same id and the same TUI cards.
// Owner: session event live and replay mapping
#[test]
fn live_calls_continue_the_numbering_of_the_saved_transcript() {
    let tool_set = AppToolSet::disabled();
    let tools = || SessionTools::of_tool_set_for_tests(&tool_set);
    let saved = vec![
        Message::Assistant(vec![call_block("reused")].into()),
        Message::ToolResult(ToolResult::succeeded("reused", "a.rs").into()),
    ];
    let mut live = LiveEvents::new(Path::new("/work"), tools(), /*configured_window*/ None);
    live.replay(&saved_rows(&saved), Path::new("/work"), tools());
    live.prompt(&UserInput::text("again"));
    let call_id = rho_sdk::ToolCallId::from_string("reused").unwrap();
    let live_calls: Vec<_> = [
        SessionEvent::Run(RunEvent::ToolProposed {
            call: shell_call("reused", "ls"),
        }),
        SessionEvent::Run(RunEvent::ToolStarted {
            call_id: call_id.clone(),
            name: "bash".into(),
            metadata: ToolMetadata::new(),
            execution: ToolExecution::Attached,
        }),
        SessionEvent::Run(RunEvent::ToolFinished {
            call_id,
            result: ToolCompletion::Completed(ToolOutput::text("b.rs")),
        }),
    ]
    .iter()
    .flat_map(|event| live.map(event))
    .collect();

    let mut whole = saved;
    whole.extend([
        Message::Assistant(vec![call_block("reused")].into()),
        Message::ToolResult(ToolResult::succeeded("reused", "b.rs").into()),
    ]);
    let replayed_tail = replay_messages(&whole).split_off(2);
    assert_eq!(
        without_display(live_calls.clone()),
        vec![
            opened("reused#2", "ls"),
            updated("reused#2", ToolStatus::Completed, vec![text("b.rs")]),
        ]
    );
    assert_eq!(live_calls, replayed_tail);
}

// Covers: a saved transcript built a save at a time numbers a call id the
// provider reused in a later save as its own call, as a replay of every row
// at once does, and replays the same events each time a client resets. The
// daemon renders a reset from the saved rows again for each client, so a
// numbering that restarted per save would update the earlier call's card.
// Owner: session event replay
#[test]
fn a_transcript_saved_turn_by_turn_replays_as_one_and_the_same_each_time() {
    let tool_set = AppToolSet::disabled();
    let tools = || SessionTools::of_tool_set_for_tests(&tool_set);
    let first = vec![
        Message::Assistant(vec![call_block("reused")].into()),
        Message::ToolResult(ToolResult::succeeded("reused", "a.rs").into()),
    ];
    let mut whole = first.clone();
    whole.extend([
        Message::Assistant(vec![call_block("reused")].into()),
        Message::ToolResult(ToolResult::succeeded("reused", "b.rs").into()),
    ]);
    let mut live = LiveEvents::new(Path::new("/work"), tools(), /*configured_window*/ None);
    let mut transcript = SavedTranscript::default();
    for saved in [&first, &whole] {
        let turn = live.replay(&saved_rows(saved), Path::new("/work"), tools());
        transcript.push(turn.expect("each save adds rows"));
    }

    let rendered: Vec<_> = transcript.events().collect();
    let ids: Vec<_> = rendered
        .iter()
        .filter_map(|event| match event {
            WireEvent::ToolCall(update) => Some(update.id.as_str().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, ["reused", "reused", "reused#2", "reused#2"]);
    assert_eq!(rendered, replay_messages(&whole));
    assert_eq!(rendered, transcript.events().collect::<Vec<_>>());
    assert_eq!(rendered.len(), transcript.len());
}
