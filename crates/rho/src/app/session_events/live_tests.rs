use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{
    CardTone, CompactionStatus, ContentBlock, Location, MessageId, PlanEntry, PlanStatus,
    SessionEvent as WireEvent, ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus,
};
use rho_sdk::{
    tool::{ToolFileChange, ToolMetadata, ToolOutput, ToolProgress},
    ContextEstimate, ProviderStreamResetReason, Revision, RunEvent, ToolCompletion, ToolExecution,
    UserInput,
};

use super::LiveEvents;
use crate::{
    app::{
        session_events::test_support::{
            agent, canonical_messages, closed, opened, origin, shell_call, text, updated,
        },
        session_runtime::{InsertedMessage, RunEnd, SessionEvent, SessionTools},
    },
    display_transcript::{DisplayRow, DisplayTranscript},
    presentation::{
        NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
        NotificationVisibility,
    },
    tools::{
        sdk_registry::AppToolSet,
        todo::{TodoItem, TodoList, TodoStatus},
    },
};

fn live(tool_set: &AppToolSet) -> LiveEvents {
    LiveEvents::new(
        Path::new("/work"),
        SessionTools::of_tool_set_for_tests(tool_set),
        /*configured_window*/ None,
    )
}

fn map_all(live: &mut LiveEvents, events: &[SessionEvent]) -> Vec<WireEvent> {
    events.iter().flat_map(|event| live.map(event)).collect()
}

fn call_id(id: &str) -> rho_sdk::ToolCallId {
    rho_sdk::ToolCallId::from_string(id).unwrap()
}

fn proposed(id: &str, command: &str) -> SessionEvent {
    SessionEvent::Run(RunEvent::ToolProposed {
        call: shell_call(id, command),
    })
}

fn started(id: &str, metadata: ToolMetadata) -> SessionEvent {
    SessionEvent::Run(RunEvent::ToolStarted {
        call_id: call_id(id),
        name: "bash".into(),
        metadata,
        execution: ToolExecution::Attached,
    })
}

fn finished(id: &str, result: ToolCompletion) -> SessionEvent {
    SessionEvent::Run(RunEvent::ToolFinished {
        call_id: call_id(id),
        result,
    })
}

fn step(tokens: u64) -> SessionEvent {
    SessionEvent::Run(RunEvent::StepStarted {
        step: 0,
        context_estimate: ContextEstimate::from_estimated_tokens(tokens),
    })
}

fn delta(text: &str) -> SessionEvent {
    SessionEvent::Run(RunEvent::AssistantTextDelta { text: text.into() })
}

fn usage(tokens: u64) -> WireEvent {
    WireEvent::Usage {
        used_tokens: tokens,
        window_tokens: None,
        cost_usd_micros: None,
    }
}

// Covers: one live turn as a client sees it: the prompt as the saved user
// message, each step's context fill, text deltas sharing a message until the
// next step, a call that opens with its card and arguments, reports
// progress, and finishes with its output, a call the run left open closing
// `failed` when the run's events end, and the settled context fill the run
// ends with.
// Owner: session event live mapping
#[test]
fn a_live_turn_streams_its_prompt_text_and_calls() {
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    live.prompt(&UserInput::text("list files"));
    let events = [
        SessionEvent::MessageInserted(InsertedMessage::Saved(origin(0))),
        SessionEvent::RunStarted,
        step(100),
        delta("Let me "),
        delta("look."),
        proposed("call-1", "ls"),
        started("call-1", ToolMetadata::new()),
        SessionEvent::Run(RunEvent::ToolUpdated {
            call_id: call_id("call-1"),
            progress: ToolProgress::new("running"),
        }),
        finished(
            "call-1",
            ToolCompletion::Completed(ToolOutput::text("a.rs")),
        ),
        step(200),
        delta("Found a.rs."),
        proposed("call-2", "cat a.rs"),
        started("call-2", ToolMetadata::new()),
        SessionEvent::RunClosed {
            ending: RunEnd::Ended,
        },
        SessionEvent::RunFinished {
            context: ContextEstimate::from_estimated_tokens(300),
        },
    ];
    assert_eq!(
        canonical_messages(map_all(&mut live, &events)),
        vec![
            WireEvent::UserMessage {
                message: MessageId::new("m0"),
                content: vec![ContentBlock::Text {
                    text: "list files".into()
                }],
            },
            usage(100),
            agent("m1", "Let me "),
            agent("m1", "look."),
            opened("call-1", "ls"),
            updated("call-1", ToolStatus::InProgress, vec![text("running")]),
            updated("call-1", ToolStatus::Completed, vec![text("a.rs")]),
            usage(200),
            agent("m2", "Found a.rs."),
            opened("call-2", "cat a.rs"),
            closed("call-2"),
            usage(300),
        ]
    );
}

// Covers: tool results beyond plain output: a call finished without
// starting still opens a complete card (titled from its proposal, or
// generically), late progress cannot reopen a finished call, an edit shows
// whole-file diffs and points at its first changed line, output the tool
// marked failed closes the call failed with that output, and a failed run
// closes only the calls still open.
// Owner: session event live mapping
#[test]
fn tool_results_open_complete_cards_and_never_reopen() {
    let edit = ToolMetadata::new()
        .command_line("sed -i s/b/B/ a.txt")
        .affected_path("a.txt");
    let edit_output = ToolOutput::text("ok").metadata(
        ToolMetadata::new().file_change(ToolFileChange::modified("a.txt", "a\nb\n", "a\nB\n")),
    );
    let cases = [
        (
            "an unavailable tool the run never started",
            vec![finished("call-1", ToolCompletion::Unavailable)],
            vec![WireEvent::tool_call(ToolCallUpdate {
                title: Some("tool".into()),
                kind: Some(ToolKind::Other),
                ..card(
                    "call-1",
                    ToolStatus::Failed,
                    vec![text("tool is unavailable")],
                )
            })],
        ),
        (
            "a proposed call the run finished without starting",
            vec![
                proposed("call-1", "ls"),
                finished("call-1", ToolCompletion::Unavailable),
            ],
            vec![WireEvent::tool_call(ToolCallUpdate {
                title: Some("ls".into()),
                kind: Some(ToolKind::Other),
                raw_input: Some(serde_json::json!({"command": "ls"})),
                ..card(
                    "call-1",
                    ToolStatus::Failed,
                    vec![text("tool is unavailable")],
                )
            })],
        ),
        (
            "progress after the call finished",
            vec![
                proposed("call-1", "ls"),
                started("call-1", ToolMetadata::new()),
                finished(
                    "call-1",
                    ToolCompletion::Completed(ToolOutput::text("a.rs")),
                ),
                SessionEvent::Run(RunEvent::ToolUpdated {
                    call_id: call_id("call-1"),
                    progress: ToolProgress::new("late"),
                }),
                SessionEvent::RunClosed {
                    ending: RunEnd::Failed("boom".into()),
                },
            ],
            vec![
                opened("call-1", "ls"),
                updated("call-1", ToolStatus::Completed, vec![text("a.rs")]),
            ],
        ),
        (
            "an edit",
            vec![
                started("call-1", edit),
                finished("call-1", ToolCompletion::Completed(edit_output)),
            ],
            vec![
                WireEvent::tool_call(ToolCallUpdate {
                    title: Some("sed -i s/b/B/ a.txt".into()),
                    kind: Some(ToolKind::Other),
                    content: None,
                    locations: Some(vec![location(None)]),
                    ..card("call-1", ToolStatus::InProgress, Vec::new())
                }),
                WireEvent::tool_call(ToolCallUpdate {
                    locations: Some(vec![location(Some(1))]),
                    ..card(
                        "call-1",
                        ToolStatus::Completed,
                        vec![
                            text("ok"),
                            ToolContent::Diff {
                                path: PathBuf::from("/work/a.txt"),
                                old_text: Some("a\nb\n".into()),
                                new_text: "a\nB\n".into(),
                            },
                        ],
                    )
                }),
            ],
        ),
        (
            "output the tool marked failed",
            vec![
                proposed("call-1", "ls"),
                started("call-1", ToolMetadata::new()),
                finished(
                    "call-1",
                    ToolCompletion::Completed(ToolOutput::text("no such file").failed()),
                ),
            ],
            vec![
                opened("call-1", "ls"),
                updated("call-1", ToolStatus::Failed, vec![text("no such file")]),
            ],
        ),
    ];
    for (name, events, expected) in cases {
        let tool_set = AppToolSet::disabled();
        let mut live = live(&tool_set);
        assert_eq!(map_all(&mut live, &events), expected, "{name}");
    }
}

/// A call update with `status` and `content`, and nothing else.
fn card(id: &str, status: ToolStatus, content: Vec<ToolContent>) -> ToolCallUpdate {
    ToolCallUpdate {
        id: ToolCallId::new(id),
        title: None,
        kind: None,
        status: Some(status),
        raw_input: None,
        content: Some(content),
        locations: None,
    }
}

fn location(line: Option<u32>) -> Location {
    Location {
        path: PathBuf::from("/work/a.txt"),
        line,
    }
}

// Covers: the checklist reaches clients as a plan only when a finished tool
// changed it; a checklist the session had when the mapper was built counts
// as sent, since replay ends with it.
// Owner: session event live mapping
#[test]
fn finished_tools_send_the_checklist_only_when_it_changed() {
    let list = |items: &[(&str, TodoStatus)]| {
        Some(TodoList {
            todos: items
                .iter()
                .map(|(content, status)| TodoItem {
                    content: (*content).into(),
                    status: *status,
                })
                .collect(),
        })
    };
    let tool_set = AppToolSet::disabled();
    let todo = tool_set.todo_state();
    todo.replace(list(&[("read", TodoStatus::Completed)]));
    let mut live = live(&tool_set);
    let steps = [
        ("checklist the session had", None, Vec::new()),
        (
            "replaced checklist",
            list(&[
                ("read", TodoStatus::Completed),
                ("edit", TodoStatus::InProgress),
            ]),
            vec![WireEvent::Plan {
                entries: vec![
                    PlanEntry {
                        content: "read".into(),
                        status: PlanStatus::Completed,
                    },
                    PlanEntry {
                        content: "edit".into(),
                        status: PlanStatus::InProgress,
                    },
                ],
            }],
        ),
        ("unchanged checklist", None, Vec::new()),
        (
            "cleared checklist",
            Some(TodoList { todos: Vec::new() }),
            vec![WireEvent::Plan {
                entries: Vec::new(),
            }],
        ),
    ];
    for (index, (name, replacement, expected)) in steps.into_iter().enumerate() {
        if replacement.is_some() {
            todo.replace(replacement);
        }
        let id = format!("call-{index}");
        let plans: Vec<_> = map_all(
            &mut live,
            &[finished(
                &id,
                ToolCompletion::Completed(ToolOutput::text("")),
            )],
        )
        .into_iter()
        .filter(|event| matches!(event, WireEvent::Plan { .. }))
        .collect();
        assert_eq!(plans, expected, "{name}");
    }
}

// Covers: a compaction reports as events of its own: it starts, and ends
// cancelled with a cancelled run, or, when the run ends first, `failed`
// with why, under one id.
// Owner: session event live mapping
#[test]
fn compactions_always_end() {
    let started = || {
        SessionEvent::Run(RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Manual,
            message_count: 4,
        })
    };
    let closed = |ending| SessionEvent::RunClosed { ending };
    let cancelled = SessionEvent::Run(RunEvent::Cancelled {
        revision: Revision::INITIAL,
    });
    let failed = |error: &str| CompactionStatus::Failed {
        error: error.into(),
    };
    let cases = [
        (
            "the run ended first",
            vec![started(), closed(RunEnd::Ended)],
            failed("compaction ended without a result"),
        ),
        (
            "the turn failed, without echoing its unredacted error",
            vec![
                started(),
                closed(RunEnd::Failed("provider unavailable".into())),
            ],
            failed("compaction ended without a result"),
        ),
        (
            "the run was cancelled",
            vec![started(), cancelled, closed(RunEnd::Ended)],
            CompactionStatus::Cancelled,
        ),
    ];
    for (name, events, ended) in cases {
        let tool_set = AppToolSet::disabled();
        let mut live = live(&tool_set);
        let mapped = map_all(&mut live, &events);
        let id = match mapped.first() {
            Some(WireEvent::Compaction { id, .. }) => id.clone(),
            other => panic!("expected the compaction to start, got {other:?}"),
        };
        let compaction = |status| WireEvent::Compaction {
            id: id.clone(),
            status,
        };
        assert_eq!(
            mapped,
            vec![compaction(CompactionStatus::Started), compaction(ended)],
            "{name}"
        );
    }
}

// Covers: a response the provider discarded mid-stream is reported as such,
// and the calls it proposed are dropped, so a call of the retried response
// is not titled from the discarded one.
// Owner: session event live mapping
#[test]
fn a_discarded_response_drops_what_it_proposed() {
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let events = [
        proposed("call-1", "rm -rf build"),
        SessionEvent::Run(RunEvent::ProviderStreamReset {
            reason: ProviderStreamResetReason::InvalidResponse,
            detail: "retry".into(),
        }),
        finished("call-1", ToolCompletion::Unavailable),
    ];
    assert_eq!(
        map_all(&mut live, &events),
        vec![
            WireEvent::ResponseDiscarded,
            WireEvent::tool_call(ToolCallUpdate {
                title: Some("tool".into()),
                kind: Some(ToolKind::Other),
                ..card(
                    "call-1",
                    ToolStatus::Failed,
                    vec![text("tool is unavailable")],
                )
            }),
        ]
    );
}

// Covers: usage reports the context window the session's model is
// configured with, when the mapper was given one.
// Owner: session event live mapping
#[test]
fn usage_reports_the_configured_window() {
    let tool_set = AppToolSet::disabled();
    let mut live = LiveEvents::new(
        Path::new("/work"),
        SessionTools::of_tool_set_for_tests(&tool_set),
        /*configured_window*/ Some(1_000),
    );
    assert_eq!(
        map_all(&mut live, &[step(100)]),
        vec![WireEvent::Usage {
            used_tokens: 100,
            window_tokens: Some(1_000),
            cost_usd_micros: None,
        }]
    );
}

// Covers: a background result's card carries its body as markdown, with
// output fenced, and as plain text, unfenced, and reads as its card's
// tone, so a client can show an error card as failed.
// Owner: session event live mapping
#[test]
fn background_cards_carry_plain_text_and_tone() {
    let card = NotificationCard {
        title: "Background process".into(),
        sender: "process".into(),
        recipient: "agent".into(),
        delivery: NotificationDelivery::Received,
        tone: NotificationTone::Error,
        preview: NotificationPreview::Full,
        visibility: NotificationVisibility::Conversation,
        reference: None,
        subtitle: Some("exited 1".into()),
        body: "failed".into(),
        output: Some("boom".into()),
        details: Vec::new(),
    };
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let applied = SessionEvent::BoundaryApplied {
        transcript: DisplayTranscript(vec![DisplayRow::Notification(Box::new(card))]),
    };
    assert_eq!(
        live.map(&applied),
        vec![WireEvent::Card {
            title: "Background process".into(),
            markdown: "exited 1\nfailed\n\n```text\nboom\n```".into(),
            plain: "exited 1\nfailed\n\nboom".into(),
            tone: CardTone::Error,
        }]
    );
}
