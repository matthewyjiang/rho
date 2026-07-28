use super::*;

#[test]
fn bottom_follow_activity_inset_only_when_activity_and_pinned() {
    assert_eq!(bottom_follow_activity_inset(false, true), 0);
    assert_eq!(bottom_follow_activity_inset(true, false), 0);
    assert_eq!(
        bottom_follow_activity_inset(true, true),
        ACTIVITY_RAIL_ROWS + ACTIVITY_CONTENT_GAP_ROWS
    );
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn loading_spinner_frames_keep_a_stable_one_cell_width() {
    for frame in LoadingSpinner::FRAMES {
        assert_eq!(display_width(frame), 1, "frame {frame}");
    }
}

#[test]
fn spinner_line_compacts_to_available_width() {
    let spinner = LoadingSpinner::default();
    let rendered = line_text(&spinner.line(
        Instant::now(),
        1,
        ActivityStatus::ParentWithSubagents(ActivityPhase::Thinking, 2),
    ));

    assert_eq!(rendered, "⠙");
    assert_eq!(
        activity_width(
            1,
            ActivityStatus::ParentWithSubagents(ActivityPhase::Thinking, 2),
        ),
        1
    );
    assert_eq!(
        activity_width(
            40,
            ActivityStatus::ParentWithSubagents(ActivityPhase::Thinking, 2),
        ),
        display_width("⠙ thinking  ·  2 agents")
    );
}
