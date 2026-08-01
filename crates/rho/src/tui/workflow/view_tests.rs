use pretty_assertions::assert_eq;
use ratatui::{layout::Rect, text::Line};

use super::detail_pane_heights;

// Covers: wrapped metadata cannot hide finished output on a short details pane.
// Owner: workflow details layout.
#[test]
fn short_pane_reserves_output_rows_after_metadata_wraps() {
    let meta = vec![Line::from("long metadata ".repeat(12))];

    assert_eq!(
        detail_pane_heights(&meta, Rect::new(0, 0, 12, 6), /*has_body*/ true),
        (3, 3)
    );
}
