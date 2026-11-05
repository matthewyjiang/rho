use rho_sdk::{
    model::{ContentBlock, Message},
    CompactionTrigger,
};

use super::HistoryMessage;

// Covers: summaries and host-restated checklists never classify as user input,
// while ordinary user text keeps its semantic meaning.
// Owner: model-history classification
#[test]
fn compaction_context_is_not_user_input() {
    let todo = crate::tools::todo::TodoState::default();
    todo.replace(Some(
        crate::tools::todo::TodoList::parse(serde_json::json!({
            "todos": [{"content": "ship the fix", "status": "pending"}]
        }))
        .unwrap(),
    ));
    let restatement = todo.checkpoint().restatement().unwrap();
    let cases = [
        (restatement, "checklist"),
        (
            Message::compaction_summary(CompactionTrigger::Manual, "s"),
            "summary",
        ),
        (
            Message::user_text(
                "Automatic compaction summary of earlier conversation for model context only:\n\nold",
            ),
            "summary",
        ),
        (Message::user_text("fix the test"), "user"),
        (
            Message::User(vec![
                ContentBlock::Text("please compact".into()),
                ContentBlock::Text("summary".into()),
            ]),
            "user",
        ),
    ];

    for (message, expected) in cases {
        let kind = match HistoryMessage::of(&message) {
            HistoryMessage::CompactionSummary(_) => "summary",
            HistoryMessage::ChecklistRestatement(_) => "checklist",
            HistoryMessage::User(_) => "user",
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(kind, expected, "{message:?}");
    }
}
