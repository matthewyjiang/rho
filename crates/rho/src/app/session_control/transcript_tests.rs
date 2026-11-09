use pretty_assertions::assert_eq;
use rho_protocol::{ToolCallUpdate, TurnError, TurnErrorKind};

use super::*;

// Covers: a turn's events fold into the text `read` pages through: chunks
// of one reply join, a tool call's updates land on its line, and a turn
// that failed says so.
// Owner: session-control read.
#[test]
fn a_turn_reads_as_who_said_what() {
    let tool = |title: Option<&str>, status| {
        SessionEvent::ToolCall(Box::new(ToolCallUpdate {
            id: ToolCallId::new("call"),
            title: title.map(str::to_owned),
            kind: None,
            status,
            raw_input: None,
            content: None,
            locations: None,
            display: None,
        }))
    };
    let mut transcript = Transcript::default();
    for event in [
        SessionEvent::UserMessage {
            message: MessageId::new("u"),
            content: vec![ContentBlock::Text {
                text: "list files".into(),
            }],
            steer: None,
        },
        SessionEvent::AgentMessageChunk {
            message: MessageId::new("a"),
            text: "Look".into(),
        },
        SessionEvent::AgentMessageChunk {
            message: MessageId::new("a"),
            text: "ing.".into(),
        },
        tool(Some("ls"), Some(ToolStatus::InProgress)),
        tool(None, Some(ToolStatus::Completed)),
        SessionEvent::TurnEnded {
            outcome: TurnOutcome::Failed {
                error: TurnError {
                    kind: TurnErrorKind::Provider,
                    message: "the provider failed".into(),
                },
            },
        },
    ] {
        transcript.push(&event);
    }

    assert_eq!(
        transcript.text(),
        "[user]\nlist files\n\n[assistant]\nLooking.\n\n[tool] ls (completed)\n\n[the turn failed: the provider failed]"
    );
}
