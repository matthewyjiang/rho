use pretty_assertions::assert_eq;
use rho_protocol::{
    display::{Presentation, ToolCard, ToolCardStatus, ToolFamily, ToolHeader},
    AnswerStop, ContentBlock, JobState, MessageId, SessionEvent, SteerId, ToolCallId as WireCallId,
    ToolCallUpdate, ToolStatus, TurnError, TurnErrorKind, TurnOutcome,
};
use rho_sdk::{SteeringId, ToolCallId};

use super::{DaemonEvents, DaemonView, Rest, TurnState};
use crate::tui::StreamKind;

/// What the session hands the mapper: an event, or a reset replay's start.
enum Step {
    Event(SessionEvent),
    Reset,
}

/// The views one session's mapper makes of `steps`.
fn map(steps: Vec<Step>) -> Vec<DaemonView> {
    let mut events = DaemonEvents::default();
    let mut views = Vec::new();
    for step in steps {
        match step {
            Step::Event(event) => views.extend(events.map(event)),
            Step::Reset => views.push(events.reset()),
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
