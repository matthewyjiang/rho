use crate::model::{AbortedAssistant, Message, ModelUsage, PartialToolCall};

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

    let [Entry::Tool(ToolEntry { display_lines, .. }), Entry::Notice(notice)] = entries.as_slice()
    else {
        panic!("expected an interrupted tool entry followed by a notice");
    };
    assert_eq!(display_lines[0], "read_file");
    assert_eq!(notice, "model interrupted");
}
