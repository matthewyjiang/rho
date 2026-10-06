use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

// Covers: malformed replacement lists must fail instead of showing misleading
// progress; valid lists report all status counts, including an empty reset.
// Owner: todo argument parser and summary.
#[test]
fn todo_lists_validate_and_summarize() {
    let pending = json!({"content": "next step", "status": "pending"});
    let invalid = |message: &str| Err((ToolErrorKind::InvalidArguments, message.to_owned()));
    let cases = [
        (
            json!({"todos": [{"content": "step", "status": "unknown"}]}),
            invalid("invalid todo arguments: unknown variant `unknown`, expected one of `pending`, `in_progress`, `completed`"),
        ),
        (
            json!({"todos": [{"content": "", "status": "pending"}]}),
            invalid("todo item 1 content must not be empty"),
        ),
        (
            json!({"todos": [{"content": " \n\t", "status": "pending"}]}),
            invalid("todo item 1 content must not be empty"),
        ),
        (
            json!({"todos": [
                {"content": "one", "status": "in_progress"},
                {"content": "two", "status": "in_progress"}
            ]}),
            invalid("todo list has 2 in_progress items, limit 1"),
        ),
        (
            json!({"todos": vec![pending.clone(); 51]}),
            invalid("todo list has 51 items, limit 50"),
        ),
        (
            json!({"todos": vec![pending; 50]}),
            Ok("50 todos: 0 completed, 0 in progress, 50 pending".into()),
        ),
        (
            json!({"todos": []}),
            Ok("0 todos: 0 completed, 0 in progress, 0 pending".into()),
        ),
        (
            json!({"todos": [
                {"content": "done", "status": "completed"},
                {"content": "working", "status": "in_progress"},
                {"content": "later", "status": "pending"}
            ]}),
            Ok("3 todos: 1 completed, 1 in progress, 1 pending".into()),
        ),
        (
            json!({}),
            invalid("invalid todo arguments: missing field `todos`"),
        ),
    ];
    for (arguments, expected) in cases {
        let actual = TodoList::parse(arguments.clone())
            .map(|list| list.summary())
            .map_err(|error| (error.kind(), error.message().to_owned()));
        assert_eq!(actual, expected, "arguments: {arguments}");
    }
}
