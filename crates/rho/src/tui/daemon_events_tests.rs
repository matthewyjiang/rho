use std::{collections::BTreeMap, path::PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{
    display::{Presentation, ToolCard, ToolCardStatus, ToolFamily, ToolHeader},
    AnswerStop, ApprovalAsk, ApprovalSource, Ask, Choice, ContentBlock, InputAnswer, InputRequest,
    InputRequestId, InputTimeout, JobState, MessageId, Operation, PathScope, Question, QuestionAsk,
    Resolution, Selection, SessionEvent, SteerId, ToolCallId as WireCallId, ToolCallUpdate,
    ToolStatus, TurnError, TurnErrorKind, TurnOutcome,
};
use rho_sdk::{SteeringId, ToolCallId};

use super::{DaemonEvents, DaemonView, Rest, TurnState};
use crate::tui::{daemon_inputs::AnswerRefusal, StreamKind};

// Covers: context tokens and cumulative spend have different meanings; a
// later unknown price must clear spend rather than reusing the prior total.
// Title events must survive mapping, including successive renames; a
// replayed rename sets the title without a notice.
// Owner: pure daemon event mapper
#[test]
fn usage_and_title_updates_keep_their_independent_values() {
    let views = map(vec![
        Step::Attached(Vec::new()),
        Step::Event(SessionEvent::TitleChanged {
            title: "Replayed title".into(),
        }),
        Step::ReplayEnded(JobState::Idle),
        Step::Event(SessionEvent::Usage {
            used_tokens: 1200,
            window_tokens: Some(8000),
            cost_usd_micros: Some(4321),
        }),
        Step::Event(SessionEvent::TitleChanged {
            title: "First title".into(),
        }),
        Step::Event(SessionEvent::Usage {
            used_tokens: 900,
            window_tokens: None,
            cost_usd_micros: None,
        }),
        Step::Event(SessionEvent::TitleChanged {
            title: "Renamed live".into(),
        }),
    ]);
    // Compare the state updates, and count the notices rather than lock
    // their UI copy: only the two live renames say so.
    let notices = views
        .iter()
        .filter(|view| matches!(view, DaemonView::Notice(_)))
        .count();
    assert_eq!(notices, 2);
    let updates: Vec<_> = views
        .into_iter()
        .filter(|view| !matches!(view, DaemonView::Notice(_)))
        .collect();
    assert_eq!(
        updates,
        vec![
            DaemonView::Title("Replayed title".into()),
            resting(Rest::Ready),
            DaemonView::Context {
                used_tokens: 1200,
                window_tokens: Some(8000)
            },
            DaemonView::Spend(Some(4321)),
            DaemonView::Title("First title".into()),
            DaemonView::Context {
                used_tokens: 900,
                window_tokens: None
            },
            DaemonView::Spend(None),
            DaemonView::Title("Renamed live".into()),
        ]
    );
}

/// What the session hands the mapper: an event, a reset replay's start, an
/// attach's open requests and its replay's end, or what came of an answer
/// the TUI sent.
enum Step {
    Event(SessionEvent),
    Reset,
    Attached(Vec<InputRequest>),
    ReplayEnded(JobState),
    Answering(InputRequestId),
    Refused(InputRequestId, AnswerRefusal),
}

/// The views one session's mapper makes of `steps`.
fn map(steps: Vec<Step>) -> Vec<DaemonView> {
    let mut events = DaemonEvents::default();
    let mut views = Vec::new();
    for step in steps {
        match step {
            Step::Event(event) => views.extend(events.map(event)),
            Step::Reset => views.push(events.reset()),
            Step::Attached(inputs) => events.attached(inputs),
            Step::ReplayEnded(state) => views.extend(events.end_replay(state)),
            Step::Answering(id) => events.answering(id),
            Step::Refused(id, refusal) => views.extend(events.answer_refused(id, refusal)),
        }
    }
    views
}

fn user(text: &str) -> Step {
    Step::Event(SessionEvent::UserMessage {
        message: MessageId::new("u1"),
        content: vec![ContentBlock::Text { text: text.into() }],
        steer: None,
    })
}

fn reply(message: &str, text: &str) -> Step {
    Step::Event(SessionEvent::AgentMessageChunk {
        message: MessageId::new(message),
        text: text.into(),
    })
}

fn thought(message: &str, text: &str) -> Step {
    Step::Event(SessionEvent::ThoughtChunk {
        message: MessageId::new(message),
        text: text.into(),
    })
}

fn tool(status: ToolStatus, display: Option<Presentation>) -> Step {
    Step::Event(SessionEvent::tool_call(ToolCallUpdate {
        id: WireCallId::new("call-1"),
        title: None,
        kind: None,
        status: Some(status),
        raw_input: None,
        content: None,
        locations: None,
        display,
    }))
}

fn answered(text: &str) -> Step {
    Step::Event(SessionEvent::TurnEnded {
        outcome: TurnOutcome::Answered {
            text: text.into(),
            stop: AnswerStop::EndTurn,
        },
    })
}

fn state(state: JobState) -> Step {
    Step::Event(SessionEvent::State { state })
}

fn ended(outcome: TurnOutcome) -> Step {
    Step::Event(SessionEvent::TurnEnded { outcome })
}

fn provider_failed(message: &str) -> TurnOutcome {
    TurnOutcome::Failed {
        error: TurnError {
            kind: TurnErrorKind::Provider,
            message: message.into(),
        },
    }
}

fn resting(rest: Rest) -> DaemonView {
    DaemonView::Turn(TurnState::Resting(rest))
}

fn steer() -> SteeringId {
    SteeringId::from_string("s1").unwrap()
}

fn read_card(status: ToolCardStatus) -> ToolCard {
    ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call("read", Some("src/lib.rs".into())),
    )
}

fn text(kind: StreamKind, text: &str, opens: bool) -> DaemonView {
    DaemonView::Text {
        kind,
        text: text.into(),
        opens,
    }
}

fn call() -> ToolCallId {
    ToolCallId::from_string("call-1").unwrap()
}

// Covers: a daemon session's replayed or live events reach the transcript as
// the rows the in-process TUI shows: one row per message, the daemon's tool
// cards running then finished, a call cut off shown interrupted, an answer
// completing missing chunks, and a reset replay starting the session over.
// Owner: tui daemon event mapping
#[test]
fn session_events_map_to_transcript_views() {
    let cases = [
        (
            "replayed turn with a tool card",
            vec![
                Step::Reset,
                user("read lib.rs"),
                thought("m1", "planning"),
                reply("m2", "Let me "),
                reply("m2", "read it."),
                tool(
                    ToolStatus::Pending,
                    Some(Presentation::Card(read_card(ToolCardStatus::Running))),
                ),
                tool(
                    ToolStatus::Completed,
                    Some(Presentation::Card(read_card(ToolCardStatus::Ok))),
                ),
                reply("m3", "Done."),
                answered("Done."),
            ],
            vec![
                DaemonView::Clear,
                DaemonView::User("read lib.rs".into()),
                text(StreamKind::Reasoning, "planning", true),
                text(StreamKind::Assistant, "Let me ", true),
                text(StreamKind::Assistant, "read it.", false),
                DaemonView::ToolRunning {
                    call: call(),
                    card: read_card(ToolCardStatus::Running),
                },
                DaemonView::ToolFinished {
                    call: call(),
                    presentation: Presentation::Card(read_card(ToolCardStatus::Ok)),
                },
                text(StreamKind::Assistant, "Done.", true),
            ],
        ),
        (
            "a call that closes failed without a card was interrupted",
            vec![
                tool(
                    ToolStatus::InProgress,
                    Some(Presentation::Card(read_card(ToolCardStatus::Running))),
                ),
                tool(ToolStatus::Failed, /*display*/ None),
            ],
            vec![
                DaemonView::ToolRunning {
                    call: call(),
                    card: read_card(ToolCardStatus::Running),
                },
                DaemonView::ToolFinished {
                    call: call(),
                    presentation: Presentation::Card(read_card(ToolCardStatus::Interrupted)),
                },
            ],
        ),
        (
            "the answer completes a reply whose last chunks went missing",
            vec![reply("m1", "Hel"), answered("Hello")],
            vec![
                text(StreamKind::Assistant, "Hel", true),
                text(StreamKind::Assistant, "lo", false),
            ],
        ),
        (
            "the answer completes the reply after the last call, not before",
            vec![
                reply("m1", "Let me read it."),
                tool(
                    ToolStatus::Completed,
                    Some(Presentation::Card(read_card(ToolCardStatus::Ok))),
                ),
                answered("Done."),
            ],
            vec![
                text(StreamKind::Assistant, "Let me read it.", true),
                DaemonView::ToolFinished {
                    call: call(),
                    presentation: Presentation::Card(read_card(ToolCardStatus::Ok)),
                },
                text(StreamKind::Assistant, "Done.", true),
            ],
        ),
        (
            "an answer that is the text before the last call shows once",
            vec![
                reply("m1", "Reading it."),
                tool(
                    ToolStatus::Completed,
                    Some(Presentation::Card(read_card(ToolCardStatus::Ok))),
                ),
                Step::Event(SessionEvent::TurnEnded {
                    outcome: TurnOutcome::MaxSteps {
                        text: "Reading it.".into(),
                    },
                }),
            ],
            vec![
                text(StreamKind::Assistant, "Reading it.", true),
                DaemonView::ToolFinished {
                    call: call(),
                    presentation: Presentation::Card(read_card(ToolCardStatus::Ok)),
                },
            ],
        ),
        (
            "an empty answer after calls adds no row",
            vec![
                reply("m1", "Reading it."),
                tool(
                    ToolStatus::Completed,
                    Some(Presentation::Card(read_card(ToolCardStatus::Ok))),
                ),
                Step::Event(SessionEvent::TurnEnded {
                    outcome: TurnOutcome::MaxSteps {
                        text: String::new(),
                    },
                }),
            ],
            vec![
                text(StreamKind::Assistant, "Reading it.", true),
                DaemonView::ToolFinished {
                    call: call(),
                    presentation: Presentation::Card(read_card(ToolCardStatus::Ok)),
                },
            ],
        ),
        (
            "a reset replay forgets the open message and running cards",
            vec![
                reply("m1", "old"),
                tool(
                    ToolStatus::InProgress,
                    Some(Presentation::Card(read_card(ToolCardStatus::Running))),
                ),
                Step::Reset,
                // No card survives the reset to close.
                tool(ToolStatus::Failed, /*display*/ None),
                reply("m1", "new"),
            ],
            vec![
                text(StreamKind::Assistant, "old", true),
                DaemonView::ToolRunning {
                    call: call(),
                    card: read_card(ToolCardStatus::Running),
                },
                DaemonView::Clear,
                text(StreamKind::Assistant, "new", true),
            ],
        ),
    ];

    for (name, steps, expected) in cases {
        assert_eq!(map(steps), expected, "{name}");
    }
}

// Covers: the job's state drives the TUI's busy state, a failed turn rests
// with its own reason and never the last turn's, and a steer's delivery and
// echo carry the id the pending panel keys it by, ahead of its user row.
// Owner: tui daemon event mapping
#[test]
fn job_states_and_steers_map_to_turn_views() {
    let cases = [
        (
            "a cancelled turn rests interrupted",
            vec![
                state(JobState::Running),
                reply("m1", "partial"),
                ended(TurnOutcome::Cancelled),
                state(JobState::Cancelled),
            ],
            vec![
                DaemonView::Turn(TurnState::Running),
                text(StreamKind::Assistant, "partial", true),
                DaemonView::Status("model interrupted".into()),
                resting(Rest::Interrupted),
            ],
        ),
        (
            "a failed turn rests with its reason",
            vec![
                state(JobState::Running),
                ended(provider_failed("provider refused the request")),
                state(JobState::Failed),
            ],
            vec![
                DaemonView::Turn(TurnState::Running),
                DaemonView::Error("provider refused the request".into()),
                resting(Rest::Failed {
                    error: "provider refused the request".into(),
                }),
            ],
        ),
        (
            "a failure's reason does not outlive its turn",
            vec![
                ended(provider_failed("provider refused the request")),
                state(JobState::Queued),
                state(JobState::Failed),
            ],
            vec![
                DaemonView::Error("provider refused the request".into()),
                DaemonView::Turn(TurnState::Running),
                resting(Rest::Failed {
                    error: "the turn failed".into(),
                }),
            ],
        ),
        (
            "a turn waits for input, then completes",
            vec![
                state(JobState::WaitingForInput),
                state(JobState::Running),
                state(JobState::Completed),
                state(JobState::Idle),
            ],
            vec![
                DaemonView::Turn(TurnState::WaitingForInput),
                DaemonView::Turn(TurnState::Running),
                resting(Rest::Ready),
                resting(Rest::Ready),
            ],
        ),
        (
            "a steer is delivered, then applies as a user row",
            vec![
                Step::Event(SessionEvent::SteeringDelivered {
                    id: SteerId::new("s1"),
                }),
                Step::Event(SessionEvent::UserMessage {
                    message: MessageId::new("u2"),
                    content: vec![ContentBlock::Text {
                        text: "check the tests too".into(),
                    }],
                    steer: Some(SteerId::new("s1")),
                }),
            ],
            vec![
                DaemonView::SteerDelivered(steer()),
                DaemonView::SteerApplied(steer()),
                DaemonView::User("check the tests too".into()),
            ],
        ),
    ];

    for (name, steps, expected) in cases {
        assert_eq!(map(steps), expected, "{name}");
    }
}

/// A write approval, request `id`.
fn approval(id: u64) -> InputRequest {
    InputRequest {
        id: InputRequestId(id),
        ask: Ask::Approval(ApprovalAsk {
            source: ApprovalSource::BuiltInTool {
                name: "write".into(),
            },
            operation: Operation::WritePath {
                path: PathBuf::from("notes.md"),
                scope: PathScope::Workspace,
            },
            reason: String::new(),
        }),
        tool_call: None,
        timeout: None,
    }
}

/// A question, request `id`, that the daemon answers at a timeout.
fn timed_question(id: u64) -> InputRequest {
    InputRequest {
        id: InputRequestId(id),
        ask: Ask::Question(QuestionAsk {
            title: "Colour".into(),
            questions: vec![Question {
                id: "colour".into(),
                prompt: "Pick a colour".into(),
                header: None,
                choices: vec![Choice {
                    value: "red".into(),
                    label: "Red".into(),
                    description: None,
                }],
                selection: Selection::One,
                allow_other: false,
                help: None,
                default: None,
                required: true,
            }],
        }),
        tool_call: None,
        timeout: Some(InputTimeout {
            deadline_ms: 1,
            answer: InputAnswer::Answers {
                answers: BTreeMap::from([("colour".to_owned(), vec!["red".to_owned()])]),
            },
            reason: "red keeps the build going".into(),
        }),
    }
}

fn requested(request: InputRequest) -> Step {
    Step::Event(SessionEvent::InputRequested {
        request: Box::new(request),
    })
}

fn resolved(id: u64, resolution: Resolution) -> Step {
    Step::Event(SessionEvent::InputResolved {
        id: InputRequestId(id),
        resolution,
    })
}

fn opened(request: InputRequest) -> DaemonView {
    DaemonView::InputOpened(Box::new(request))
}

// Covers: an input request shows once, whether it opened live or the replay
// left it open (listed by the attach, repeated by the replay, or neither),
// and never when the replay opened and closed it; one another client
// answered, the turn withdrew, or the daemon answered at its timeout closes
// with a notice, one the TUI answered closes silently, and a refused answer
// shows the request again only while it is still open.
// Owner: tui daemon event mapping
#[test]
fn input_requests_show_once_and_close_as_resolved() {
    let held = InputRequest {
        timeout: None,
        ..timed_question(3)
    };
    let cases = [
        (
            "the replay leaves a request open: it shows once the replay ends",
            vec![
                Step::Attached(vec![approval(1)]),
                requested(approval(1)),
                requested(approval(2)),
                Step::ReplayEnded(JobState::WaitingForInput),
            ],
            vec![
                DaemonView::Turn(TurnState::WaitingForInput),
                opened(approval(1)),
                opened(approval(2)),
            ],
        ),
        (
            "the replay opens and closes a request: it never shows",
            vec![
                Step::Attached(Vec::new()),
                requested(approval(1)),
                resolved(1, Resolution::Answered),
                Step::ReplayEnded(JobState::Running),
            ],
            vec![DaemonView::Turn(TurnState::Running)],
        ),
        (
            "a request held during the replay shows without its timeout",
            vec![
                Step::Attached(vec![timed_question(3)]),
                Step::Event(SessionEvent::InputHeld {
                    id: InputRequestId(3),
                }),
                Step::ReplayEnded(JobState::WaitingForInput),
            ],
            vec![DaemonView::Turn(TurnState::WaitingForInput), opened(held)],
        ),
        (
            "live requests close as resolved elsewhere",
            vec![
                requested(approval(1)),
                requested(approval(1)),
                resolved(1, Resolution::Answered),
                requested(approval(2)),
                resolved(2, Resolution::Withdrawn),
                requested(timed_question(3)),
                resolved(3, Resolution::TimedOut),
            ],
            vec![
                opened(approval(1)),
                DaemonView::InputClosed {
                    id: InputRequestId(1),
                    notice: "another client answered the approval".into(),
                },
                opened(approval(2)),
                DaemonView::InputClosed {
                    id: InputRequestId(2),
                    notice: "the turn withdrew the approval".into(),
                },
                opened(timed_question(3)),
                DaemonView::InputClosed {
                    id: InputRequestId(3),
                    notice: "timeout fallback: red keeps the build going".into(),
                },
            ],
        ),
        (
            "a request the TUI answered closes silently",
            vec![
                requested(approval(1)),
                Step::Answering(InputRequestId(1)),
                resolved(1, Resolution::Answered),
            ],
            vec![opened(approval(1))],
        ),
        (
            "a refused answer asks again; one no longer open does not",
            vec![
                requested(approval(1)),
                Step::Answering(InputRequestId(1)),
                Step::Refused(
                    InputRequestId(1),
                    AnswerRefusal::Failed("the daemon is busy".into()),
                ),
                requested(approval(2)),
                Step::Answering(InputRequestId(2)),
                Step::Refused(InputRequestId(2), AnswerRefusal::NotOpen),
                resolved(2, Resolution::Answered),
            ],
            vec![
                opened(approval(1)),
                DaemonView::Error("could not answer the request: the daemon is busy".into()),
                opened(approval(1)),
                opened(approval(2)),
                DaemonView::Error("could not answer the request: it is no longer open".into()),
            ],
        ),
    ];

    for (name, steps, expected) in cases {
        assert_eq!(map(steps), expected, "{name}");
    }
}
