use pretty_assertions::assert_eq;
use rho_protocol::{
    AnswerStop, CardTone, CompactionId, CompactionStatus, ContentBlock, MessageId, SessionEvent,
    ToolCallId, ToolCallUpdate, ToolStatus, TurnId, TurnOutcome,
};

use super::TextView;

fn tool_call(title: Option<&str>, status: ToolStatus) -> SessionEvent {
    SessionEvent::tool_call(ToolCallUpdate {
        id: ToolCallId::new("call"),
        title: title.map(Into::into),
        kind: None,
        status: Some(status),
        raw_input: None,
        content: None,
        locations: None,
        display: None,
    })
}

fn chunk(text: &str) -> SessionEvent {
    SessionEvent::AgentMessageChunk {
        message: MessageId::new("reply"),
        text: text.into(),
    }
}

fn compaction(status: CompactionStatus) -> SessionEvent {
    SessionEvent::Compaction {
        id: CompactionId::new("compaction"),
        status,
    }
}

// Covers: the conversation streams to stdout with the prompt quoted, each
// tool call and compaction on a line of its own once started and once more
// if it fails, a card as plain text, and the reply ending its line;
// notices and discarded responses go to stderr.
// Owner: rho attach text output (unit).
#[test]
fn text_output_streams_the_conversation_and_tells_the_rest_on_stderr() {
    let events = [
        SessionEvent::UserMessage {
            message: MessageId::new("prompt"),
            content: vec![ContentBlock::Text {
                text: "read the docs\nthen answer".into(),
            }],
            steer: None,
        },
        chunk("Looking"),
        tool_call(None, ToolStatus::Pending),
        tool_call(Some("Read docs/index.md"), ToolStatus::InProgress),
        tool_call(None, ToolStatus::Failed),
        tool_call(None, ToolStatus::Failed),
        SessionEvent::Notice {
            text: "retrying the request".into(),
        },
        SessionEvent::ResponseDiscarded,
        compaction(CompactionStatus::Started),
        compaction(CompactionStatus::Failed {
            error: "provider unavailable".into(),
        }),
        SessionEvent::Card {
            title: "Background process".into(),
            markdown: "exited 1\n\n```text\nboom\n```".into(),
            plain: "exited 1\n\nboom".into(),
            tone: CardTone::Error,
            display: None,
        },
        chunk("Done."),
        SessionEvent::TurnEnded {
            turn: TurnId::new("t1"),
            outcome: TurnOutcome::Answered {
                text: "Looking Done.".into(),
                stop: AnswerStop::EndTurn,
            },
        },
    ];
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut view = TextView::new(&mut out, &mut err);
    for event in &events {
        view.show(event).unwrap();
    }

    assert_eq!(
        String::from_utf8(out).unwrap(),
        "> read the docs\n> then answer\nLooking\n• Read docs/index.md\n• Read docs/index.md failed\n\
         • compact context\n• compact context failed\nBackground process\nexited 1\n\nboom\nDone.\n"
    );
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "rho: retrying the request\nrho: provider response discarded; retrying\n"
    );
}

// Covers: an attachment that ends mid-reply, as a detach does, ends the
// reply's line, so the reason it ended starts a line of its own.
// Owner: rho attach text output (unit).
#[test]
fn finishing_mid_reply_ends_the_line() {
    let mut out = Vec::new();
    let mut view = TextView::new(&mut out, Vec::new());
    view.show(&chunk("partial")).unwrap();
    view.finish().unwrap();
    view.finish().unwrap();
    drop(view);
    assert_eq!(String::from_utf8(out).unwrap(), "partial\n");
}

// Covers: a restart, as a reset replay makes, shows the tool calls shown
// before again and starts on a line of its own.
// Owner: rho attach text output (unit).
#[test]
fn a_restart_shows_tool_calls_again() {
    let mut out = Vec::new();
    let mut view = TextView::new(&mut out, Vec::new());
    for event in [
        tool_call(Some("Read docs/index.md"), ToolStatus::InProgress),
        chunk("partial"),
    ] {
        view.show(&event).unwrap();
    }
    view.restart().unwrap();
    view.show(&tool_call(
        Some("Read docs/index.md"),
        ToolStatus::InProgress,
    ))
    .unwrap();
    drop(view);

    assert_eq!(
        String::from_utf8(out).unwrap(),
        "• Read docs/index.md\npartial\n• Read docs/index.md\n"
    );
}
