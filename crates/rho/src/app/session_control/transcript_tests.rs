use pretty_assertions::assert_eq;
use rho_protocol::{
    CompactionCounts, CompactionId, ToolCallUpdate, TurnError, TurnErrorKind, TurnId,
};

use super::*;

// Covers: a turn's events fold into the text `read` pages through: chunks
// of one reply join, a tool call's updates land on its line, a completed
// compaction, as a replay sends it alone, notes its counts, and a turn that
// failed says so.
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
        SessionEvent::Compaction {
            id: CompactionId::new("saved-compaction"),
            status: CompactionStatus::Completed {
                counts: CompactionCounts {
                    previous_messages: 263,
                    current_messages: 65,
                    previous_tokens: 247_276,
                    current_tokens: 54_798,
                    cost_usd_micros: None,
                },
            },
        },
        SessionEvent::TurnEnded {
            turn: TurnId::new("t1"),
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
        "[user]\nlist files\n\n[assistant]\nLooking.\n\n[tool] ls (completed)\n\n\
         [context compacted: 263 → 65 messages, 247276 → 54798 tokens]\n\n\
         [the turn failed: the provider failed]"
    );
}
