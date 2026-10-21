use pretty_assertions::assert_eq;

use super::super::{
    markdown::StreamLineStart, theme::Theme, LiveStreamPreview, StreamKind, StreamUi,
};

fn preview(text: &str) -> LiveStreamPreview {
    LiveStreamPreview {
        kind: StreamKind::Assistant,
        text: text.into(),
        line_start: StreamLineStart::Fresh,
        include_leading_blank: false,
    }
}

// Covers: new preview text, width, fence advance, and theme must miss.
// Owner: tui stream preview render cache
#[test]
fn stream_preview_cache_misses_on_content_width_fence_and_theme() {
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
