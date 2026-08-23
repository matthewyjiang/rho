use ratatui::text::Line;

use super::{
    markdown::push_wrapped_markdown_without_copy_button_from_fence_state,
    render::{pad_display_line, padded_content_width},
    theme::Theme,
    StreamKind, StreamPreviewRenderCache, StreamUi,
};

impl StreamUi {
    /// Cached live-preview paint. Hits skip markdown and highlighter clone.
    ///
    /// Dropped when preview identity or committed fence state changes. Remaining
    /// key is width + theme generation.
    pub(super) fn cached_preview_lines(&mut self, width: usize) -> &[Line<'static>] {
        let Some(preview) = self.live_stream_preview.as_ref() else {
            self.preview_render_cache = None;
            return &[];
        };
        let theme_generation = Theme::generation();
        if self
            .preview_render_cache
            .as_ref()
            .is_some_and(|cache| cache.width == width && cache.theme_generation == theme_generation)
        {
            return &self
                .preview_render_cache
                .as_ref()
                .expect("hit checked above")
                .lines;
        }

        let kind = preview.kind;
        let include_leading_blank = preview.include_leading_blank;
        let text = preview.text.clone();
        let mut lines = Vec::new();
        if include_leading_blank {
            lines.push(Line::raw(""));
        }
        let mut text_lines = Vec::new();
        let mut code_fence = match kind {
            StreamKind::Assistant => self.assistant_stream_code_fence.clone(),
            StreamKind::Reasoning => self.reasoning_stream_code_fence.clone(),
        };
        push_wrapped_markdown_without_copy_button_from_fence_state(
            &mut text_lines,
            &text,
            padded_content_width(width),
            &mut code_fence,
        );
        if matches!(kind, StreamKind::Reasoning) {
            Theme::reasoning_output(&mut text_lines);
        }
        lines.extend(text_lines.into_iter().map(pad_display_line));
        #[cfg(test)]
        {
            self.preview_paints = self.preview_paints.saturating_add(1);
        }
        self.preview_render_cache = Some(StreamPreviewRenderCache {
            width,
            theme_generation,
            lines,
        });
        &self
            .preview_render_cache
            .as_ref()
            .expect("render cache populated above")
            .lines
    }

    #[cfg(test)]
    pub(super) fn preview_cache_paints(&self) -> u32 {
        self.preview_paints
    }

    #[cfg(test)]
    pub(super) fn preview_cache_theme_generation(&self) -> Option<u64> {
        self.preview_render_cache
            .as_ref()
            .map(|cache| cache.theme_generation)
    }
}

#[cfg(test)]
#[path = "stream_preview_tests.rs"]
mod tests;
