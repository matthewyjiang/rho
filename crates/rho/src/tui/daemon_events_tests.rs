use pretty_assertions::assert_eq;
use rho_protocol::{
    display::{Presentation, ToolCard, ToolCardStatus, ToolFamily, ToolHeader},
    AnswerStop, ContentBlock, MessageId, SessionEvent, ToolCallId as WireCallId, ToolCallUpdate,
    ToolStatus, TurnOutcome,
};
use rho_sdk::ToolCallId;

use super::{DaemonEvents, DaemonView};
use crate::tui::StreamKind;

/// What the session hands the mapper: an event, or a reset replay's start.
enum Step {
    Event(SessionEvent),
    Reset,
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
        let mut events = DaemonEvents::default();
        let mut views = Vec::new();
        for step in steps {
            match step {
                Step::Event(event) => views.extend(events.map(event)),
                Step::Reset => views.push(events.reset()),
            }
        }
        assert_eq!(views, expected, "{name}");
    }
}
