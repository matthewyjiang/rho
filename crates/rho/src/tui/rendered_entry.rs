use ratatui::text::Line;

use super::{
    feed_image::RenderedImagePlacements, markdown::MarkdownCodeBlock,
    markdown_image::MarkdownImageSource,
};

pub(super) struct RenderedEntry {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) code_blocks: Vec<MarkdownCodeBlock>,
    pub(super) image_placement: Option<RenderedImagePlacements>,
    /// Standalone `![alt](path)` references found in assistant markdown.
    pub(super) image_sources: Vec<MarkdownImageSource>,
    /// Rendered fallback rows corresponding to `image_sources`.
    pub(super) image_rows: Vec<usize>,
}
