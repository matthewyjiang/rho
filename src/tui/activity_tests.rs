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

    assert_eq!(line_text(&spinner.line(started_at)), "⠋ working");
}

#[test]
fn activity_line_right_aligns_jump_text() {
    let spinner = LoadingSpinner::default();
    let rendered = line_text(&line(
        40,
        Instant::now(),
        Some(&spinner),
        Some("↓ jump to bottom  ctrl+g".into()),
    ));

    assert!(rendered.starts_with("⠋ working"), "{rendered:?}");
    assert!(
        rendered.ends_with("↓ jump to bottom  ctrl+g"),
        "{rendered:?}"
    );
    assert_eq!(display_width(&rendered), 40);
}
