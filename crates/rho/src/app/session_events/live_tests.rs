use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    CardTone, CompactionStatus, CompactionTrigger, ContentBlock, Location, MessageId, PlanEntry,
    PlanStatus, RetryReason, SessionEvent as WireEvent, ToolCallId, ToolCallUpdate, ToolContent,
    ToolKind, ToolStatus,
};
use rho_sdk::{
    tool::{ToolFileChange, ToolMetadata, ToolOutput, ToolProgress},
    ContextEstimate, ProviderErrorKind, ProviderStreamResetReason, Revision, RunEvent,
    ToolCompletion, ToolExecution, UserInput,
};

use super::{CompactionEnd, LiveEvents};
use crate::{
    app::{
        interactive_presenter::InteractiveToolPresenter,
        session_events::{
            steer_to_wire,
            test_support::{
                agent, canonical_messages, closed, opened, origin, shell_call, text, updated,
                without_display,
            },
        },
        session_runtime::{InsertedMessage, SessionEvent, SessionTools},
    },
    display_transcript::{DisplayRow, DisplayTranscript},
    tools::{
        mcp::exported_name::McpToolNames,
        sdk_registry::AppToolSet,
        todo::{TodoItem, TodoList, TodoStatus},
    },
};
use rho_protocol::display::{
    NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
    NotificationVisibility, Presentation,
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
        SessionEvent::RunClosed,
        SessionEvent::RunFinished {
            context: ContextEstimate::from_estimated_tokens(300),
        },
    ];
    assert_eq!(
        canonical_messages(without_display(map_all(&mut live, &events))),
        vec![
            WireEvent::UserMessage {
                message: MessageId::new("m0"),
                content: vec![ContentBlock::Text {
                    text: "list files".into()
                }],
                steer: None,
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
                SessionEvent::RunClosed,
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
                                deleted: false,
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
        assert_eq!(
            without_display(map_all(&mut live, &events)),
            expected,
            "{name}"
        );
    }
}

// Covers: each start, progress update and result of a call carries the
// card the TUI's presenter draws at that event, a call the run finished
// without starting carries its result's, and a call the run cut off closes
// without one, so a client rebuilds the TUI's cards and marks the cut-off
// call interrupted.
// Owner: session event live mapping
#[test]
fn tool_calls_carry_the_tui_cards() {
    let ran = ToolCompletion::Completed(ToolOutput::text("a.rs"));
    let progress = ToolProgress::new("running");
    // The TUI's presenter, fed each call as the TUI's event adapter feeds it.
    let mut tui = InteractiveToolPresenter::new(PathBuf::from("/work"), McpToolNames::default());
    let mut start = |id: &str, command: &str| {
        tui.proposed(shell_call(id, command));
        Presentation::Card(
            tui.started(call_id(id), "bash".into(), ToolMetadata::new())
                .card,
        )
    };
    let ran_started = start("call-1", "ls");
    let cut_off_started = start("call-3", "sleep 9");
    let ran_progress = Presentation::Card(tui.updated(&call_id("call-1"), &progress).card);
    let ran_finished = tui.finished(&call_id("call-1"), &ran).1.presentation;
    tui.proposed(shell_call("call-2", "cat b.rs"));
    let unstarted_finished = tui
        .finished(&call_id("call-2"), &ToolCompletion::Unavailable)
        .1
        .presentation;

    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let events = [
        proposed("call-1", "ls"),
        started("call-1", ToolMetadata::new()),
        SessionEvent::Run(RunEvent::ToolUpdated {
            call_id: call_id("call-1"),
            progress,
        }),
        finished("call-1", ran),
        proposed("call-2", "cat b.rs"),
        finished("call-2", ToolCompletion::Unavailable),
        proposed("call-3", "sleep 9"),
        started("call-3", ToolMetadata::new()),
        SessionEvent::RunClosed,
    ];
    let displays: Vec<_> = map_all(&mut live, &events)
        .into_iter()
        .filter_map(|event| match event {
            WireEvent::ToolCall(update) => Some(update.display),
            _ => None,
        })
        .collect();
    assert_eq!(
        displays,
        vec![
            Some(ran_started),
            Some(ran_progress),
            Some(ran_finished),
            Some(unstarted_finished),
            Some(cut_off_started),
            None,
        ]
    );
}

// Covers: a steer the daemon recorded shows as a user message carrying its
// steer id once the run applies it, even when the apply reaches the mapper
// before the daemon recorded the steer, then ahead of the next event so it
// keeps the mapper's order; a withdrawn steer, or one the run's
// end dropped, never shows, nor does an apply the run's end forgot; the
// provider taking a steer mid-request reports its id.
// Owner: session event live mapping
#[test]
fn steers_show_as_user_messages_once_applied() {
    /// What a case does next: the daemon records or withdraws the steer, or
    /// the runtime emits an event.
    enum Step {
        Steered,
        Withdrawn,
        Runtime(Box<SessionEvent>),
    }
    let id = rho_sdk::SteeringId::new();
    let applied = || {
        Step::Runtime(Box::new(SessionEvent::Run(RunEvent::SteeringApplied {
            ids: vec![id.clone()],
        })))
    };
    let run_closed = || Step::Runtime(Box::new(SessionEvent::RunClosed));
    let next = || {
        Step::Runtime(Box::new(SessionEvent::Notice {
            text: "next".into(),
        }))
    };
    let next_notice = || WireEvent::Notice {
        text: "next".into(),
    };
    let steer_message = vec![WireEvent::UserMessage {
        message: MessageId::new("m0"),
        content: vec![ContentBlock::Text {
            text: "use tabs".into(),
        }],
        steer: Some(steer_to_wire(&id)),
    }];
    let cases = [
        (
            "recorded, then applied",
            vec![(Step::Steered, vec![]), (applied(), steer_message.clone())],
        ),
        (
            "applied before it was recorded",
            vec![
                (applied(), vec![]),
                (Step::Steered, vec![]),
                (
                    next(),
                    [steer_message.clone(), vec![next_notice()]].concat(),
                ),
            ],
        ),
        (
            "withdrawn",
            vec![
                (Step::Steered, vec![]),
                (Step::Withdrawn, vec![]),
                (applied(), vec![]),
            ],
        ),
        (
            "delivered",
            vec![
                (Step::Steered, vec![]),
                (
                    Step::Runtime(Box::new(SessionEvent::Run(RunEvent::SteeringDelivered {
                        id: id.clone(),
                    }))),
                    vec![WireEvent::SteeringDelivered {
                        id: steer_to_wire(&id),
                    }],
                ),
            ],
        ),
        (
            "dropped by the run's end",
            vec![
                (Step::Steered, vec![]),
                (run_closed(), vec![]),
                (applied(), vec![]),
            ],
        ),
        (
            "applied, then forgotten at the run's end",
            vec![
                (applied(), vec![]),
                (run_closed(), vec![]),
                (Step::Steered, vec![]),
                (next(), vec![next_notice()]),
            ],
        ),
    ];
    for (name, steps) in cases {
        let tool_set = AppToolSet::disabled();
        let mut live = live(&tool_set);
        for (index, (step, expected)) in steps.into_iter().enumerate() {
            let shown = match step {
                Step::Steered => {
                    live.steered(id.clone(), &UserInput::text("use tabs"));
                    Vec::new()
                }
                Step::Withdrawn => {
                    live.withdrawn(&id);
                    Vec::new()
                }
                Step::Runtime(event) => live.map(&event),
            };
            assert_eq!(canonical_messages(shown), expected, "{name}, step {index}");
        }
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
        display: None,
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
// with why, under one id; a host's own compaction that fails ends `failed`
// with the error the host gives.
// Owner: session event live mapping
#[test]
fn compactions_always_end() {
    /// How a case drives the mapper: with a run's events, or as a host
    /// whose own compaction, such as `/compact`'s, fails with this error.
    enum Drive {
        Run(Vec<SessionEvent>),
        HostFailed(&'static str),
    }
    let started = || {
        SessionEvent::Run(RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Manual,
            message_count: 4,
        })
    };
    let cancelled = SessionEvent::Run(RunEvent::Cancelled {
        revision: Revision::INITIAL,
    });
    let failed = |error: &str| CompactionStatus::Failed {
        error: error.into(),
    };
    let cases = [
        (
            "the run ended first",
            Drive::Run(vec![started(), SessionEvent::RunClosed]),
            failed("compaction ended without a result"),
        ),
        (
            "the run was cancelled",
            Drive::Run(vec![started(), cancelled, SessionEvent::RunClosed]),
            CompactionStatus::Cancelled,
        ),
        (
            "a host's compaction failed",
            Drive::HostFailed("could not compact context"),
            failed("could not compact context"),
        ),
    ];
    for (name, drive, ended) in cases {
        let tool_set = AppToolSet::disabled();
        let mut live = live(&tool_set);
        let (trigger, mapped) = match drive {
            Drive::Run(events) => (CompactionTrigger::Automatic, map_all(&mut live, &events)),
            Drive::HostFailed(error) => {
                let mut mapped = live.compaction_started(CompactionTrigger::Command);
                mapped.extend(live.compaction_ended(CompactionEnd::Failed(error.into())));
                (CompactionTrigger::Command, mapped)
            }
        };
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
            vec![
                compaction(CompactionStatus::Started {
                    trigger: Some(trigger),
                }),
                compaction(ended)
            ],
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
        without_display(map_all(&mut live, &events)),
        vec![
            WireEvent::ResponseDiscarded,
            WireEvent::ProviderRetry {
                reason: RetryReason::InvalidResponse,
                retry_after_ms: None,
            },
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

// Covers: each time the run asks the provider again, clients hear why and
// how long the provider said to wait, after the discarded response when a
// stream reset discarded one.
// Owner: session event live mapping
#[test]
fn a_retry_says_why() {
    let reset = |reason| {
        SessionEvent::Run(RunEvent::ProviderStreamReset {
            reason,
            detail: "retry".into(),
        })
    };
    let retry = |reason, retry_after_ms| WireEvent::ProviderRetry {
        reason,
        retry_after_ms,
    };
    let cases = [
        (
            "request failed",
            SessionEvent::Run(RunEvent::ProviderRequestRetry),
            vec![retry(RetryReason::RequestFailed, None)],
        ),
        (
            "rate limited",
            reset(ProviderStreamResetReason::RetryableFailure {
                kind: ProviderErrorKind::RateLimit,
                retry_after: Some(Duration::from_secs(5)),
            }),
            vec![
                WireEvent::ResponseDiscarded,
                retry(RetryReason::RateLimit, Some(5_000)),
            ],
        ),
        (
            "other failure",
            reset(ProviderStreamResetReason::RetryableFailure {
                kind: ProviderErrorKind::Unavailable,
                retry_after: None,
            }),
            vec![
                WireEvent::ResponseDiscarded,
                retry(RetryReason::ProviderError, None),
            ],
        ),
        (
            "context overflow",
            reset(ProviderStreamResetReason::ContextOverflow),
            vec![
                WireEvent::ResponseDiscarded,
                retry(RetryReason::ContextOverflow, None),
            ],
        ),
    ];
    for (case, event, expected) in cases {
        let tool_set = AppToolSet::disabled();
        assert_eq!(live(&tool_set).map(&event), expected, "{case}");
    }
}

// Covers: a tool the provider ran itself shows as a call that opens
// finished with its detail, each under an id of its own.
// Owner: session event live mapping
#[test]
fn hosted_tool_activity_opens_finished_calls() {
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let events = [
        SessionEvent::Run(RunEvent::WebSearch {
            detail: " rho docs ".into(),
        }),
        SessionEvent::Run(RunEvent::HostedToolActivity {
            name: "code_interpreter".into(),
            detail: String::new(),
        }),
    ];
    let mut ids = Vec::new();
    let mut shown = Vec::new();
    for event in map_all(&mut live, &events) {
        let WireEvent::ToolCall(mut update) = event else {
            panic!("hosted activity maps only to tool calls");
        };
        ids.push(std::mem::replace(&mut update.id, ToolCallId::new("hosted")));
        assert!(update.display.take().is_some(), "{update:?} has no card");
        shown.push(*update);
    }
    let finished = |title: &str, kind, content| ToolCallUpdate {
        title: Some(title.into()),
        kind: Some(kind),
        content,
        ..card("hosted", ToolStatus::Completed, Vec::new())
    };
    assert_eq!(
        shown,
        vec![
            finished("web_search", ToolKind::Search, Some(vec![text("rho docs")])),
            finished("code_interpreter", ToolKind::Other, None),
        ]
    );
    assert_ne!(ids[0], ids[1]);
}

// Covers: a call opens while the provider still streams its arguments, once
// it has an id, a name, and arguments to show; starting it keeps the call's
// id rather than closing the streamed card as failed; and a discarded
// response closes the calls its stream opened.
// Owner: session event live mapping
#[test]
fn a_streamed_call_opens_pending_and_keeps_its_id_when_it_starts() {
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let streamed = |id: Option<&str>, arguments: &str| {
        SessionEvent::Run(RunEvent::ToolCallUpdated {
            index: 0,
            id: id.map(str::to_owned),
            name: id.map(|_| "bash".to_owned()),
            arguments_delta: arguments.to_owned(),
        })
    };
    let events = [
        // Named before it has arguments: nothing to show yet.
        streamed(Some("call-1"), ""),
        streamed(Some("call-1"), r#"{"command":"l"#),
        // Its card follows its arguments.
        streamed(Some("call-1"), "s"),
        // Sent once the arguments grew by a quarter.
        streamed(Some("call-1"), " -la /tmp"),
        proposed("call-1", "ls"),
        started("call-1", ToolMetadata::default()),
        step(100),
        streamed(Some("call-2"), r#"{"command":"pwd"#),
        SessionEvent::Run(RunEvent::ProviderStreamReset {
            reason: ProviderStreamResetReason::InvalidResponse,
            detail: "retry".into(),
        }),
    ];
    let seen: Vec<_> = map_all(&mut live, &events)
        .into_iter()
        .filter_map(|event| match event {
            WireEvent::ToolCall(update) => Some((
                update.id.as_str().to_owned(),
                update.status,
                update.display.is_some(),
            )),
            WireEvent::ResponseDiscarded => Some(("discarded".to_owned(), None, false)),
            _ => None,
        })
        .collect();
    let (first, second) = (seen[0].0.clone(), seen[3].0.clone());

    assert_eq!(
        seen,
        [
            (first.clone(), Some(ToolStatus::Pending), true),
            (first.clone(), None, true),
            (first.clone(), Some(ToolStatus::InProgress), true),
            (second.clone(), Some(ToolStatus::Pending), true),
            (second.clone(), Some(ToolStatus::Failed), false),
            ("discarded".to_owned(), None, false),
        ]
    );
    assert_ne!(first, second);
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

// Covers: a streamed card held back for its arguments to grow is the
// host's to send when the stream pauses, once, under the call's id.
// Owner: session event live mapping
#[test]
fn a_held_streamed_card_goes_out_when_the_host_asks() {
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let streamed = |arguments: &str| {
        SessionEvent::Run(RunEvent::ToolCallUpdated {
            index: 0,
            id: Some("call-1".to_owned()),
            name: Some("bash".to_owned()),
            arguments_delta: arguments.to_owned(),
        })
    };
    let opened = map_all(&mut live, &[streamed(r#"{"command":"l"#)]);
    let [WireEvent::ToolCall(opened)] = opened.as_slice() else {
        panic!("expected the call to open: {opened:?}");
    };
    // Too little more to send by itself.
    assert_eq!(map_all(&mut live, &[streamed("s")]), Vec::new());
    assert!(live.holds_previews());

    let held = live.held_previews();
    let [WireEvent::ToolCall(update)] = held.as_slice() else {
        panic!("expected the held card: {held:?}");
    };
    assert_eq!(
        (&update.id, update.status, update.display.is_some()),
        (&opened.id, None, true)
    );
    assert_eq!(
        (live.holds_previews(), live.held_previews()),
        (false, Vec::new())
    );

    // A card still held when its call has run is dropped, not opened as a
    // second call.
    map_all(&mut live, &[streamed("x")]);
    map_all(
        &mut live,
        &[
            started("call-1", ToolMetadata::default()),
            finished("call-1", ToolCompletion::Unavailable),
        ],
    );
    assert_eq!(live.held_previews(), Vec::new());
}

// Covers: a card held back for more arguments goes out before the text
// that follows its call, so a pause partway through that text finds
// nothing to send between two chunks of one message, which would split
// the row a client shows it in (#1818).
// Owner: session event live mapping
#[test]
fn a_held_card_goes_out_before_the_text_that_follows_its_call() {
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let streamed = |arguments: &str| {
        SessionEvent::Run(RunEvent::ToolCallUpdated {
            index: 0,
            id: Some("call-1".to_owned()),
            name: Some("bash".to_owned()),
            arguments_delta: arguments.to_owned(),
        })
    };
    map_all(&mut live, &[streamed(r#"{"command":"l"#), streamed("s")]);

    let first = map_all(&mut live, &[delta("Hel")]);
    let paused = live.held_previews();
    let second = map_all(&mut live, &[delta("lo")]);

    let shape: Vec<_> = first
        .iter()
        .chain(&second)
        .map(|event| match event {
            WireEvent::ToolCall(_) => "card",
            WireEvent::AgentMessageChunk { .. } => "chunk",
            _ => "other",
        })
        .collect();
    let messages: Vec<_> = first
        .iter()
        .chain(&second)
        .filter_map(|event| match event {
            WireEvent::AgentMessageChunk { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        (shape, paused, messages[0] == messages[1]),
        (vec!["card", "chunk", "chunk"], Vec::new(), true)
    );
}

// Covers: a host that prices a streaming response gets the step's context
// fill again, and none before any step reported one.
// Owner: session event live mapping
#[test]
fn streamed_usage_repeats_the_steps_fill() {
    let tool_set = AppToolSet::disabled();
    let mut live = live(&tool_set);
    let before = live.streamed_usage();
    map_all(&mut live, &[step(100)]);
    assert_eq!((before, live.streamed_usage()), (None, Some(usage(100))));

    // It arrives between a reply's chunks, which stay one message.
    let mut reply = map_all(&mut live, &[delta("Hel")]);
    live.streamed_usage();
    reply.extend(map_all(&mut live, &[delta("lo")]));
    let messages: Vec<_> = reply
        .iter()
        .filter_map(|event| match event {
            WireEvent::AgentMessageChunk { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], messages[1]);
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
        transcript: DisplayTranscript(vec![DisplayRow::Notification(Box::new(card.clone()))]),
    };
    assert_eq!(
        live.map(&applied),
        vec![WireEvent::Card {
            title: "Background process".into(),
            markdown: "exited 1\nfailed\n\n```text\nboom\n```".into(),
            plain: "exited 1\nfailed\n\nboom".into(),
            tone: CardTone::Error,
            display: Some(Box::new(card)),
        }]
    );
}
