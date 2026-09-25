use pretty_assertions::assert_eq;

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
