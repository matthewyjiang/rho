use pretty_assertions::assert_eq;
use serde_json::json;

use super::transcript_entries_from_messages;
use crate::tui::Entry;
use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};
use rho_tools::tool_card::ToolHeader;

fn call(id: &str, name: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({}),
    })])
}

fn calls(entries: &[(&str, &str)]) -> Message {
    Message::Assistant(
        entries
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolCall(ToolCall {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: json!({}),
                })
            })
            .collect(),
    )
}

fn result(id: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: "ok".into(),
    })
}

fn tool_names(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Tool(tool) => match &tool.card.header {
                ToolHeader::Call { verb, .. } => Some(verb.clone()),
                ToolHeader::StatusFirst { identity, .. } => Some(identity.clone()),
                ToolHeader::Shell { command, .. } => command.clone(),
            },
            _ => None,
        })
        .collect()
}

// Covers: historical tool cards pair by call id, including late and
// out-of-order results.
// Owner: tui message history pairing
#[test]
fn transcript_pairs_tool_results_by_id() {
    struct Case {
        name: &'static str,
        messages: Vec<Message>,
        expected_verbs: Vec<&'static str>,
    }
    let cwd = std::path::Path::new("/tmp");
    let cases = [
        Case {
            name: "late result after user steer",
            messages: vec![call("a", "agent"), Message::user_text("steer"), result("a")],
            expected_verbs: vec!["agent"],
        },
        Case {
            name: "out-of-order results",
            messages: vec![
                calls(&[("a", "reviewer"), ("b", "agent")]),
                result("b"),
                result("a"),
            ],
            expected_verbs: vec!["agent", "reviewer"],
        },
        Case {
            name: "missing result truncates",
            messages: vec![call("a", "agent"), Message::user_text("next")],
            expected_verbs: vec![],
        },
    ];
    for case in cases {
        let entries = transcript_entries_from_messages(&case.messages, cwd);
        assert_eq!(tool_names(&entries), case.expected_verbs, "{}", case.name);
    }
}

// Covers: reopening a session retains message identity and the full body,
// including legacy receipts that predate task metadata.
// Owner: transcript replay, not live rendering.
#[test]
fn transcript_restores_message_receipts() {
    use crate::{
        app::message_card::{MessageCard, MessageDelivery},
        tools::agent::message_receipt::MessageReceipt,
    };

    let body = "Check routing first.\nKeep the full message.";
    let receipt = MessageReceipt {
        run_id: "abc123".into(),
        agent_id: "reviewer".into(),
        task: "Review routing".into(),
    };
    for (content, title, recipient) in [
        (receipt.content(), "Review routing", "reviewer"),
        (
            "queued parent message for delegated run 'abc123'".into(),
            "Delegated task",
            "child",
        ),
    ] {
        let messages = vec![
            Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                id: "message-call".into(),
                name: "agents".into(),
                arguments: json!({"action": "message", "id": "abc123", "message": body}),
            })]),
            Message::ToolResult(ToolResult {
                id: "message-call".into(),
                ok: true,
                content,
            }),
        ];
        let entries = transcript_entries_from_messages(&messages, std::path::Path::new("."));
        let [Entry::Tool(tool)] = entries.as_slice() else {
            panic!("expected one historical tool entry");
        };
        assert_eq!(
            tool.message,
            Some(Box::new(MessageCard {
                title: title.into(),
                sender: "parent".into(),
                recipient: recipient.into(),
                delivery: MessageDelivery::Queued,
                body: body.into(),
                details: vec!["run: abc123".into(), "attach: rho attach abc123".into()],
            }))
        );
    }
}
