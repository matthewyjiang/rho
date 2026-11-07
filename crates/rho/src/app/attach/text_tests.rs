use pretty_assertions::assert_eq;
use rho_protocol::{
    AnswerStop, ApprovalAsk, ApprovalSource, Ask, ContentBlock, InputRequest, InputRequestId,
    MessageId, Operation, SessionEvent, ToolCallId, ToolCallUpdate, ToolStatus, TurnOutcome,
};

use super::TextView;

fn tool_call(title: Option<&str>, status: ToolStatus) -> SessionEvent {
    SessionEvent::ToolCall(ToolCallUpdate {
        id: ToolCallId::new("call"),
        title: title.map(Into::into),
        kind: None,
        status: Some(status),
        raw_input: None,
        content: None,
        locations: None,
    })
}

fn chunk(text: &str) -> SessionEvent {
    SessionEvent::AgentMessageChunk {
        message: MessageId::new("reply"),
        text: text.into(),
    }
}

// Covers: the conversation streams to stdout with the prompt quoted, each
// tool call on a line of its own once titled and once more if it fails,
// and the reply ending its line; notices and input requests go to stderr.
// Owner: rho attach text output (unit).
#[test]
fn text_output_streams_the_conversation_and_tells_the_rest_on_stderr() {
    let events = [
        SessionEvent::UserMessage {
            message: MessageId::new("prompt"),
            content: vec![ContentBlock::Text {
                text: "read the docs\nthen answer".into(),
            }],
        },
        chunk("Looking"),
        tool_call(None, ToolStatus::Pending),
        tool_call(Some("Read docs/index.md"), ToolStatus::InProgress),
        tool_call(None, ToolStatus::Failed),
        tool_call(None, ToolStatus::Failed),
        SessionEvent::Notice {
            text: "retrying the request".into(),
        },
        SessionEvent::InputRequested {
            request: Box::new(InputRequest {
                id: InputRequestId(1),
                ask: Ask::Approval(ApprovalAsk {
                    source: ApprovalSource::Rho,
                    operation: Operation::Network { url: None },
                    reason: "fetch the docs".into(),
                }),
                tool_call: None,
                timeout: None,
            }),
        },
        chunk("Done."),
        SessionEvent::TurnEnded {
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
        "> read the docs\n> then answer\nLooking\n• Read docs/index.md\n• Read docs/index.md failed\nDone.\n"
    );
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "rho: retrying the request\nrho: the turn waits for an approval: fetch the docs\n"
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
