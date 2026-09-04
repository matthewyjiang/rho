use pretty_assertions::assert_eq;

use super::MessageReceipt;

// Covers: verbatim tool-result consumers receive readable task identity, while
// replay retains embedded delimiters, Unicode, and multiline task titles.
// Owner: agent receipt wire format.
#[test]
fn receipt_wire_format_is_readable_and_round_trips_task_text() {
    for task in [
        "Review routing",
        "Review ' to worker\nTask: 第二行",
        "Inspect {json} and \"quotes\"",
    ] {
        let receipt = MessageReceipt {
            run_id: "abc123".into(),
            agent_id: "reviewer".into(),
            task: task.into(),
        };
        let content = receipt.content();
        assert_eq!(
            content,
            format!("queued parent message for delegated run 'abc123' to reviewer\nTask: {task}")
        );
        assert_eq!(MessageReceipt::parse(&content), Some(receipt));
    }
}

// Covers: saved JSON receipts remain readable, but mismatched identities and
// unstructured legacy acknowledgements cannot invent task metadata.
// Owner: agent receipt parser.
#[test]
fn receipt_parser_handles_legacy_and_invalid_identities() {
    for (content, expected) in [
        (
            "queued parent message for delegated run 'abc123'\n{\"run_id\":\"abc123\",\"agent_id\":\"reviewer\",\"task\":\"Review routing\"}",
            Some(MessageReceipt { run_id: "abc123".into(), agent_id: "reviewer".into(), task: "Review routing".into() }),
        ),
        ("queued parent message for delegated run 'abc123'", None),
        ("queued parent message for delegated run 'abc123' to reviewer\nnot a task", None),
        ("queued parent message for delegated run 'abc123'\n{\"run_id\":\"def456\",\"agent_id\":\"reviewer\",\"task\":\"Review routing\"}", None),
    ] {
        assert_eq!(MessageReceipt::parse(content), expected);
    }
}
