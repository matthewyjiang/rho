use ratatui::layout::Rect;

use super::{
    overlay_panel_inner_width, side_overlay_panel_body, side_scroll_metrics, SideEntry, SideOverlay,
};

// Covers: a rejected concurrent submit must not idle the overlay or drop the
// in-flight assistant stream; only a terminal fail ends the run.
// Owner: side-chat overlay state
#[test]
fn rejected_submit_keeps_busy_and_stream() {
    let mut overlay = SideOverlay::new("snap".into());
    overlay.busy = true;
    overlay.append_assistant_delta("partial");

    overlay.push_notice("could not start side chat: a turn is already running".into());

    pretty_assertions::assert_eq!(overlay.busy, true);
    pretty_assertions::assert_eq!(overlay.streaming_assistant.as_deref(), Some("partial"));
    pretty_assertions::assert_eq!(
        overlay.entries.last(),
        Some(&SideEntry::Error(
            "could not start side chat: a turn is already running".into()
        ))
    );

    overlay.fail_run("could not complete side chat: provider error".into());

    pretty_assertions::assert_eq!(overlay.busy, false);
    pretty_assertions::assert_eq!(overlay.streaming_assistant, None);
    pretty_assertions::assert_eq!(
        overlay.entries,
        vec![
            SideEntry::Error("could not start side chat: a turn is already running".into()),
            SideEntry::Assistant("partial".into()),
            SideEntry::Error("could not complete side chat: provider error".into()),
        ]
    );
}

// Covers: scroll range must use the wrapped overlay body, not entry count.
// Owner: side-chat overlay layout
#[test]
fn side_scroll_metrics_follow_wrapped_body() {
    let mut overlay = SideOverlay::new("snap".into());
    overlay.push_user("short".into());
    overlay
        .entries
        .push(SideEntry::Assistant(["word"; 80].join(" ")));
    let area = Rect::new(0, 0, 40, 20);
    let metrics = side_scroll_metrics(&overlay, area).expect("panel fits");
    let inner_width = overlay_panel_inner_width(area).max(1);
    let body_len = side_overlay_panel_body(&overlay, inner_width).len();

    pretty_assertions::assert_eq!(metrics.body_len, body_len);
    assert!(
        body_len > overlay.entries.len().saturating_add(4),
        "wrapped assistant text must beat an entry-count fudge, body_len={body_len} entries={}",
        overlay.entries.len()
    );
}
