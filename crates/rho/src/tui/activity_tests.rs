use super::*;

// Covers: stacked rail rows share one column layout for identity / activity / elapsed.
// Owner: pure layout
#[test]
fn rail_row_columns_identity_activity_and_trailing() {
    let row_style = Theme::activity_rail();
    let line = RailRow {
        connector: tree_connector(true),
        identity: vec![
            Span::styled("sleep", Theme::text_strong().patch(row_style)),
            Span::styled(" ", row_style),
            Span::styled("aaaaaaaa", Theme::dim().patch(row_style)),
        ],
        activity: "running".into(),
        trailing: "4s".into(),
        row_style,
    }
    .into_line(80);

    let texts: Vec<&str> = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(texts[0], "  └ ");
    assert_eq!(texts[1], "sleep");
    assert_eq!(texts[2], " ");
    assert_eq!(texts[3], "aaaaaaaa");
    assert_eq!(texts[4], "  ·  ");
    assert_eq!(texts[5], "running");
    assert!(texts[6].chars().all(|ch| ch == ' '));
    assert_eq!(texts[7], "4s");
}

// Covers: a too-narrow rail collapses to a single truncated detail span.
// Owner: pure layout
#[test]
fn rail_row_truncates_when_fixed_width_overflows() {
    let row_style = Theme::activity_rail();
    let line = RailRow {
        connector: tree_connector(true),
        identity: vec![Span::styled(
            "very-long-command",
            Theme::text_strong().patch(row_style),
        )],
        activity: "running".into(),
        trailing: "12s".into(),
        row_style,
    }
    .into_line(18);

    assert_eq!(line.spans.len(), 2);
    assert_eq!(line.spans[0].content.as_ref(), "  └ ");
    let detail = line.spans[1].content.as_ref();
    assert!(detail.starts_with("very-long-"));
    assert!(display_width(detail) <= 14);
}

#[test]
fn bottom_follow_activity_inset_only_when_activity_and_pinned() {
    assert_eq!(bottom_follow_activity_inset(false, true), 0);
    assert_eq!(bottom_follow_activity_inset(true, false), 0);
    assert_eq!(
        bottom_follow_activity_inset(true, true),
        ACTIVITY_RAIL_ROWS + ACTIVITY_CONTENT_GAP_ROWS
    );
}
