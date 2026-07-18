use ratatui::text::Line;

use super::{
    markdown::push_wrapped_markdown_without_copy_button_from_fence_state, pad_display_line,
    padded_content_width, theme::Theme, App, StreamKind,
};

impl App {
    pub(super) fn render_stream_preview_lines(
        &self,
        preview: &super::LiveStreamPreview,
        width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if preview.include_leading_blank {
            lines.push(Line::raw(""));
        }
        let mut text_lines = Vec::new();
        let mut code_fence = match preview.kind {
            StreamKind::Assistant => self.assistant_stream_code_fence,
            StreamKind::Reasoning => self.reasoning_stream_code_fence,
        };
        push_wrapped_markdown_without_copy_button_from_fence_state(
            &mut text_lines,
            &preview.text,
            padded_content_width(width),
            &mut code_fence,
        );
        if matches!(preview.kind, StreamKind::Reasoning) {
            Theme::reasoning_output(&mut text_lines);
        }
        lines.extend(text_lines.into_iter().map(pad_display_line));
        lines
    }
}
