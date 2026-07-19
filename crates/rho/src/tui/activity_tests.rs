use super::*;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn loading_spinner_advances_frames() {
    let started_at = Instant::now();
    let spinner = LoadingSpinner {
        started_at: Some(started_at),
    };

    assert_eq!(spinner.frame_at(started_at), "⠋");
    assert_eq!(
        spinner.frame_at(started_at + LoadingSpinner::FRAME_INTERVAL),
        "⠙"
    );
}

#[test]
fn loading_spinner_line_separates_frame_from_text() {
    let started_at = Instant::now();
    let spinner = LoadingSpinner {
        started_at: Some(started_at),
    };

    assert_eq!(
        line_text(&spinner.line(
            started_at,
            40,
            ActivityStatus::Parent(ActivityPhase::WaitingForProvider),
        )),
        "⠋ waiting for provider"
    );
}

#[test]
fn activity_line_combines_parent_and_subagent_work() {
    let spinner = LoadingSpinner::default();

    assert_eq!(
        line_text(&spinner.line(
            Instant::now(),
            40,
            ActivityStatus::ParentWithSubagents(ActivityPhase::Thinking, 2),
        )),
        "⠋ thinking  ·  2 agents"
    );
    assert_eq!(
        line_text(&spinner.line(Instant::now(), 40, ActivityStatus::Subagents(2))),
        "⠋ 2 agents working"
    );
}

#[test]
fn spinner_line_compacts_to_available_width() {
    let spinner = LoadingSpinner::default();
    let rendered = line_text(&spinner.line(
        Instant::now(),
        1,
        ActivityStatus::ParentWithSubagents(ActivityPhase::Thinking, 2),
    ));

    assert_eq!(rendered, "⠋");
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
        display_width("⠋ thinking  ·  2 agents")
    );
}
