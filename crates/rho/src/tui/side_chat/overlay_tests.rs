use ratatui::layout::Rect;

use super::{prepare_side_panel, Entry, SideOverlay};

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
    assert!(matches!(overlay.entries.last(), Some(Entry::Error(_))));

    overlay.fail_run("could not complete side chat: provider error".into());

    pretty_assertions::assert_eq!(overlay.busy, false);
    pretty_assertions::assert_eq!(overlay.streaming_assistant, None);
    let [Entry::Error(rejected), Entry::Assistant(partial), Entry::Error(failed)] =
        overlay.entries.as_slice()
    else {
        panic!("unexpected entries: {:?}", overlay.entries);
    };
    pretty_assertions::assert_eq!(
        (rejected.as_str(), partial.text.as_str(), failed.as_str()),
        (
            "could not start side chat: a turn is already running",
            "partial",
            "could not complete side chat: provider error",
        )
    );
}

// Covers: assistant text preceding a tool must stay before it, and a retry
// reset must discard only the uncommitted continuation, not earlier entries.
// Owner: side-chat overlay state
#[test]
fn tool_boundary_commits_assistant_before_retry_reset() {
    let mut overlay = SideOverlay::new("snap".into());
    overlay.append_assistant_delta("before tool");
    overlay.push_tool("read_file".into());
    overlay.append_assistant_delta("discarded attempt");
    overlay.reset_assistant_stream();
    overlay.append_assistant_delta("after tool");
    overlay.finish_assistant();

    let [Entry::Assistant(before), Entry::Notice(tool), Entry::Assistant(after)] =
        overlay.entries.as_slice()
    else {
        panic!("unexpected entries: {:?}", overlay.entries);
    };
    pretty_assertions::assert_eq!(
        (before.text.as_str(), tool.as_str(), after.text.as_str()),
        ("before tool", "tool read_file", "after tool")
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
        .push(Entry::Assistant(["word"; 80].join(" ").into()));
    let area = Rect::new(0, 0, 40, 20);
    let prepared = prepare_side_panel(&overlay, area).expect("panel fits");
    let metrics = prepared.metrics;
    let body_len = prepared.body.lines.len();

    pretty_assertions::assert_eq!(metrics.body_len, body_len);
    assert!(
        body_len > overlay.entries.len().saturating_add(4),
        "wrapped assistant text must beat an entry-count fudge, body_len={body_len} entries={}",
        overlay.entries.len()
    );
}
