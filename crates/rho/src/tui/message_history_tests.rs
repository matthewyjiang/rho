use rho_providers::model::{AbortedAssistant, Message, ModelUsage, PartialToolCall};
use rho_tools::tool_card::{ToolFact, ToolStatus};

use super::{
    super::{Entry, ToolEntry},
    transcript_entries_from_messages,
};

#[test]
fn interrupted_tool_call_uses_the_tool_name_without_a_preparing_label() {
    let entries = transcript_entries_from_messages(
        &[Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: Vec::new(),
            reasoning: String::new(),
            provenance: None,
            reasoning_summary: None,
            provider_context: Vec::new(),
            tool_calls: vec![PartialToolCall {
                id: Some("call_1".into()),
                name: Some("read_file".into()),
                arguments: r#"{"path":"src/main.rs"}"#.into(),
            }],
            usage: ModelUsage::default(),
        }))],
        std::path::Path::new(""),
    );

    let [Entry::Tool(ToolEntry { card, .. }), Entry::Notice(notice)] = entries.as_slice() else {
        panic!("expected an interrupted tool entry followed by a notice");
    };
    assert_eq!(card.header_text(), "■ read_file(src/main.rs)");
    assert_eq!(notice, "model interrupted");
}

#[test]
fn interrupted_agent_tools_hide_partial_json() {
    for (name, arguments, expected_header, expected_facts) in [
        (
            "agent",
            r#"{"agent_id":"explorer","prompt":"Audit the repository"#,
            "■ explorer  interrupted",
            vec!["Audit the repository".to_string()],
        ),
        (
            "agents",
            r#"{"action":"status","id":"abc123"#,
            "■ abc123  status interrupted",
            Vec::<String>::new(),
        ),
    ] {
        let entries = transcript_entries_from_messages(
            &[Message::AbortedAssistant(Box::new(AbortedAssistant {
                content: Vec::new(),
                reasoning: String::new(),
                provenance: None,
                reasoning_summary: None,
                provider_context: Vec::new(),
                tool_calls: vec![PartialToolCall {
                    id: Some("call_1".into()),
                    name: Some(name.into()),
                    arguments: arguments.into(),
                }],
                usage: ModelUsage::default(),
            }))],
            std::path::Path::new(""),
        );

        let [Entry::Tool(ToolEntry { card, .. }), Entry::Notice(_)] = entries.as_slice() else {
            panic!("expected an interrupted tool entry followed by a notice");
        };
        assert_eq!(card.status, ToolStatus::Interrupted);
        assert_eq!(card.header_text(), expected_header);
        let facts: Vec<String> = card.facts.iter().map(ToolFact::plain_text).collect();
        assert_eq!(facts, expected_facts);
    }
}
