use rho_sdk::model::{ContentBlock, Message, SemanticMessage};

use super::HistoryMessage;

// Covers: host-restated checklists never classify as user input, while
// ordinary user text keeps its semantic meaning.
// Owner: model-history classification
#[test]
fn checklist_restatement_is_not_user_input() {
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
        (Message::user_text("fix the test"), "user"),
        (
            Message::User(
                vec![
                    ContentBlock::Text("please compact".into()),
                    ContentBlock::Text("summary".into()),
                ]
                .into(),
            ),
            "user",
        ),
    ];

    for (message, expected) in cases {
        let kind = match HistoryMessage::of(&message) {
            HistoryMessage::ChecklistRestatement(_) => "checklist",
            HistoryMessage::Semantic(SemanticMessage::User(_)) => "user",
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(kind, expected, "{message:?}");
    }
}
