use pretty_assertions::assert_eq;

use super::*;

fn partition(previous_summary: Option<&str>) -> CompactionPartition {
    CompactionPartition {
        leading_messages: vec![Message::System("system".into())],
        anchor_messages: vec![Message::user_text("build the thing")],
        previous_summary: previous_summary.map(str::to_owned),
        compacted_messages: vec![Message::assistant_text("did step one")],
        recent_messages: vec![Message::user_text("recent")],
    }
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
        let request = build_summary_request_messages(&partition(previous));

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
        assert!(!body.contains("user:\n## Open tasks"), "{body}");
    }
}

// Covers: replacement history keeps the first turn and the active goal verbatim
// ahead of a typed summary labeled by trigger, and drops the goal once cleared.
// Owner: text-summary compaction
#[test]
fn replacement_keeps_anchors_and_labels_summary_by_trigger() {
    let goal = ActiveGoal::default();
    for (trigger, condition) in [
        (CompactionTrigger::Manual, Some("tests pass")),
        (CompactionTrigger::Automatic, None),
    ] {
        goal.set(condition);

        let replacement =
            replacement_history_from_summary(partition(None), trigger, &goal, "  summary  ");

        let expected = [
            vec![
                Message::System("system".into()),
                Message::user_text("build the thing"),
            ],
            condition
                .map(|text| goal_anchor(text.into()))
                .into_iter()
                .collect(),
            vec![
                Message::compaction_summary(trigger, "summary"),
                Message::user_text("recent"),
            ],
        ]
        .concat();
        assert_eq!(replacement, expected, "{trigger:?}");
    }
}
