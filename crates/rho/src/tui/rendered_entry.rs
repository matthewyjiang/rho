use ratatui::text::Line;

use super::{feed_image::RenderedImagePlacement, markdown::MarkdownCodeBlock};

pub(super) struct RenderedEntry {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) code_blocks: Vec<MarkdownCodeBlock>,
    pub(super) image_placement: Option<RenderedImagePlacement>,
}
