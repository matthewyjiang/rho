use pretty_assertions::assert_eq;

use super::super::{theme::Theme, LiveStreamPreview, StreamKind, StreamUi};

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn preview(text: &str) -> LiveStreamPreview {
    LiveStreamPreview {
        kind: StreamKind::Assistant,
        text: text.into(),
        include_leading_blank: false,
    }
}

// Covers: unchanged live preview must not re-markdown or clone the fence highlighter.
// Owner: tui stream preview render cache
#[test]
fn stream_preview_cache_hits_when_preview_and_fence_are_unchanged() {
    let _guard = crate::tui::theme::theme_test_lock();
    let mut streams = StreamUi::default();
    streams.set_live_preview(Some(preview("hello **world**")));
    let first = streams
        .cached_preview_lines(40)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    assert_eq!(streams.preview_cache_paints(), 1);
    let second = streams
        .cached_preview_lines(40)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    assert_eq!(streams.preview_cache_paints(), 1);
    assert_eq!(first, second);
}

// Covers: new preview text, width, fence advance, and theme must miss.
// Owner: tui stream preview render cache
#[test]
fn stream_preview_cache_misses_on_content_width_fence_and_theme() {
    let _guard = crate::tui::theme::theme_test_lock();
    Theme::apply_committed("terminal");
    let mut streams = StreamUi::default();
    streams.set_live_preview(Some(preview("hello")));
    streams.cached_preview_lines(40);
    assert_eq!(streams.preview_cache_paints(), 1);

    streams.set_live_preview(Some(preview("hello there")));
    streams.cached_preview_lines(40);
    assert_eq!(streams.preview_cache_paints(), 2);

    streams.cached_preview_lines(20);
    assert_eq!(streams.preview_cache_paints(), 3);

    streams.advance_code_fence(StreamKind::Assistant, "```rust\n");
    streams.cached_preview_lines(20);
    assert_eq!(streams.preview_cache_paints(), 4);

    Theme::apply_committed("one-half-light");
    let generation = Theme::generation();
    streams.cached_preview_lines(20);
    let cached_generation = streams.preview_cache_theme_generation();
    Theme::apply_committed("terminal");
    assert_eq!(streams.preview_cache_paints(), 5);
    assert_eq!(cached_generation, Some(generation));
}
