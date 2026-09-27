use pretty_assertions::assert_eq;
use rho_providers::model::{AssistantMessage, ImageContent};
use serde_json::json;

use super::*;

fn history(previous_summary: Option<&str>) -> Vec<Message> {
    let mut history = vec![
        Message::System("system".into()),
        Message::user_text("build the thing"),
    ];
    history.extend(
        previous_summary
            .map(|text| Message::compaction_summary(CompactionTrigger::Automatic, text)),
    );
    history.extend([
        Message::assistant_text("did step one ".repeat(400)),
        Message::user_text("recent"),
    ]);
    history
}

fn partition(history: &[Message]) -> CompactionPartition<'_> {
    super::super::partition_messages_for_compaction(history, &[], 1_000).unwrap()
}

// Covers: the scratchpad never reaches model context, including when the model
// forgets to close it, while summary text around it survives.
// Owner: text-summary compaction
#[test]
fn strip_analysis_removes_scratchpads() {
    let cases = [
        ("## Open tasks\nNone", "## Open tasks\nNone"),
        (
            "<analysis>think</analysis>\n## Open tasks\nNone",
            "## Open tasks\nNone",
        ),
        ("a<analysis>x</analysis>b<analysis>y</analysis>c", "abc"),
        (
            "## Open tasks\nNone\n<analysis>unclosed",
            "## Open tasks\nNone",
        ),
        ("<analysis>only</analysis>", ""),
    ];

    for (input, expected) in cases {
        assert_eq!(strip_analysis(input), expected, "{input:?}");
    }
}

// Covers: the session-history suffix points at the deleted span and the
// verbatim tail, and does not copy the deleted message into the uncached
// suffix. The cached prefix stays the session history.
// Owner: text-summary compaction
#[test]
fn session_summary_request_names_the_deleted_span() {
    let history = history(None);
    let deleted = "did step one ".repeat(400);
    let request = build_session_summary_request(&history, &partition(&history));

    assert_eq!(&request[..history.len()], history.as_slice());
    let Message::User(blocks) = &request[history.len()] else {
        panic!("expected a trailing user instruction");
    };
    let [ContentBlock::Text(instruction)] = blocks.as_slice() else {
        panic!("expected one text block, got {blocks:?}");
    };
    assert!(
        instruction.contains("assistant: did step one"),
        "{instruction}"
    );
    assert!(instruction.contains("user: recent"), "{instruction}");
    assert!(
        !instruction.contains(&deleted),
        "the deleted message was copied into the suffix"
    );
}

// Covers: tool-call and tool-result boundaries are marked by name, call id,
// and status. Scraping the pretty-printed transcript would point at `{`
// instead, so the cached summary suffix could no longer tell the deleted
// span from the verbatim tail.
// Owner: text-summary compaction
#[test]
fn session_summary_markers_name_tool_calls_instead_of_json() {
    let payload = "x".repeat(8_000);
    let history = vec![
        Message::System("system".into()),
        Message::user_text("build the thing"),
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-old".into(),
            name: "read_file".into(),
            arguments: json!({"path": payload}),
        })]),
        Message::ToolResult(ToolResult {
            id: "call-old".into(),
            ok: false,
            content: format!("missing\n{payload}"),
        }),
        Message::assistant(AssistantMessage::from_content(vec![
            ContentBlock::ToolCall(ToolCall {
                id: "call-new".into(),
                name: "bash".into(),
                arguments: json!({"command": "true"}),
            }),
        ])),
    ];
    let request = build_session_summary_request(&history, &partition(&history));
    let Message::User(blocks) = request.last().expect("trailing instruction") else {
        panic!("expected a trailing user instruction");
    };
    let [ContentBlock::Text(instruction)] = blocks.as_slice() else {
        panic!("expected one text block, got {blocks:?}");
    };

    assert!(
        instruction.contains("assistant: tool call read_file (call-old)"),
        "{instruction}"
    );
    assert!(
        instruction.contains("assistant: tool call bash (call-new)"),
        "{instruction}"
    );
    assert!(!instruction.contains(&payload), "{instruction}");
    assert!(!instruction.contains('{'), "{instruction}");

    let result_history = vec![
        Message::System("system".into()),
        Message::user_text("build the thing"),
        Message::ToolResult(ToolResult {
            id: "call-lost".into(),
            ok: false,
            content: format!("missing file\n{payload}"),
        }),
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-new".into(),
            name: "bash".into(),
            arguments: json!({"command": "true"}),
        })]),
    ];
    let result_request =
        build_session_summary_request(&result_history, &partition(&result_history));
    let Message::User(blocks) = result_request.last().expect("trailing instruction") else {
        panic!("expected a trailing user instruction");
    };
    let [ContentBlock::Text(instruction)] = blocks.as_slice() else {
        panic!("expected one text block, got {blocks:?}");
    };
    assert!(
        instruction.contains("tool result: (call-lost) [error] missing file"),
        "{instruction}"
    );
    assert!(
        instruction.contains("assistant: tool call bash (call-new)"),
        "{instruction}"
    );
    assert!(!instruction.contains(&payload), "{instruction}");
}

// Covers: a second compaction hands the earlier summary to the model as a
// summary to update, never as a rendered user turn, and a first compaction
// sends no previous-summary section.
// Owner: text-summary compaction
#[test]
fn summary_request_updates_previous_summary_instead_of_resummarizing() {
    for previous in [None, Some("## Open tasks\nship it")] {
        let history = history(previous);
        let request = build_summary_request_messages(&partition(&history));

        let [Message::System(_), Message::User(blocks)] = request.as_slice() else {
            panic!("unexpected request shape: {request:?}");
        };
        let [ContentBlock::Text(body)] = blocks.as_slice() else {
            panic!("unexpected body: {blocks:?}");
        };
        let expected =
            previous.map(|summary| format!("<previous-summary>\n{summary}\n</previous-summary>"));
        assert_eq!(
            body.contains("<previous-summary>"),
            expected.is_some(),
            "{previous:?}"
        );
        if let Some(expected) = expected {
            assert!(body.contains(&expected), "{body}");
        }
        assert!(
            body.contains("<original-request>\nuser:\nbuild the thing"),
            "{body}"
        );
        assert!(!body.contains("summary:\n## Open tasks"), "{body}");
    }
}

// Covers: the response becomes one summary labeled by trigger, with the
// scratchpad removed and the first turn kept ahead of it; a response with no
// text outside the scratchpad is an error, not an empty summary.
// Owner: text-summary compaction
#[test]
fn summary_replacement_labels_summary_and_rejects_empty_text() {
    let history = history(None);
    let partition = partition(&history);
    let response = |text: &str| vec![ContentBlock::Text(text.into())];

    for trigger in [CompactionTrigger::Manual, CompactionTrigger::Automatic] {
        let replacement = summary_replacement(
            &partition,
            trigger,
            &response("<analysis>scratch</analysis>\n  summary  "),
        )
        .unwrap();
        assert_eq!(
            replacement,
            vec![
                Message::System("system".into()),
                Message::user_text("build the thing"),
                Message::compaction_summary(trigger, "summary"),
                Message::user_text("recent"),
            ],
            "{trigger:?}"
        );
    }
    assert!(matches!(
        summary_replacement(
            &partition,
            CompactionTrigger::Manual,
            &response("<analysis>only</analysis>")
        ),
        Err(Error::InvalidHostResponse { .. })
    ));
}

// Covers: fallback summary transcripts keep tool arguments readable and tool
// result bodies raw, including enriched-assistant calls, so file text and
// elided stubs are not JSON-escaped. Image bytes stay out of the transcript.
// A later call that reuses an id does not relabel the earlier result.
// Owner: text-summary compaction
#[test]
fn summary_transcript_renders_tool_calls_and_results_as_text() {
    let image_bytes = "BASE64DATA";
    let stub = "[elided tool result: read_file path=src/a.rs · ok · 12 bytes · recall_id=abc; fetch the text with the sessions tool, action=recall]";
    let body = "fn main() {\n    let quoted = \"hi\";\n}\n";
    let messages = vec![
        Message::assistant(AssistantMessage::from_content(vec![
            ContentBlock::Text("reading".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "src/a.rs", "offset": 1}),
            }),
            ContentBlock::Image(ImageContent {
                mime_type: "image/png".into(),
                data: image_bytes.into(),
            }),
        ])),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: body.into(),
        }),
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-elided".into(),
            name: "read_file".into(),
            arguments: json!({"path": "src/a.rs"}),
        })]),
        Message::ToolResult(ToolResult {
            id: "call-elided".into(),
            ok: true,
            content: stub.into(),
        }),
        Message::ToolResult(ToolResult {
            id: "call-missing".into(),
            ok: false,
            content: "boom\n".into(),
        }),
        Message::assistant(AssistantMessage::from_content(vec![
            ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: json!({"command": "true"}),
            }),
        ])),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "ran\n".into(),
        }),
    ];

    let rendered = render_messages_for_summary(&messages);
    assert_eq!(
        rendered,
        "\
assistant:
reading
tool call read_file (call-1):
{
  \"path\": \"src/a.rs\",
  \"offset\": 1
}
[image: image/png]

tool result read_file (call-1) [ok]:
fn main() {
    let quoted = \"hi\";
}


assistant:
tool call read_file (call-elided):
{
  \"path\": \"src/a.rs\"
}

tool result read_file (call-elided) [ok]:
[elided tool result: read_file path=src/a.rs · ok · 12 bytes · recall_id=abc; fetch the text with the sessions tool, action=recall]

tool result unknown (call-missing) [error]:
boom


assistant:
tool call bash (call-1):
{
  \"command\": \"true\"
}

tool result bash (call-1) [ok]:
ran
"
    );
}
