//! Composer attachment chrome: slots, strip layout, labels, and paint.
//!
//! Ready images may carry a Kitty/halfblock preview. Consecutive previews share
//! width-bounded horizontal strips (wrapping when gaps would overflow); documents
//! and pending items stay full-width label rows.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    Frame,
};

use super::{
    display_width,
    feed_image::{FeedImage, ImageRowBudget},
    styled_line, truncate_one_line, App, ChatMedia, ComposerAttachment, ComposerMode, LineFill,
    MediaAttachId, PendingAttachmentSource, Theme,
};

/// Gap in columns between side-by-side composer image previews.
pub(super) const COMPOSER_IMAGE_GAP: usize = 2;

/// One ordered composer attachment plus optional graphics preview.
#[derive(Clone, Debug)]
pub(super) struct ComposerAttachmentSlot {
    pub(super) attachment: ComposerAttachment,
    pub(super) image_preview: Option<FeedImage>,
}

impl ComposerAttachmentSlot {
    pub(super) fn ready(media: ChatMedia, image_preview: Option<FeedImage>) -> Self {
        Self {
            attachment: ComposerAttachment::Ready(media),
            image_preview,
        }
    }

    pub(super) fn pending(
        id: MediaAttachId,
        source: PendingAttachmentSource,
        name: String,
    ) -> Self {
        Self {
            attachment: ComposerAttachment::Pending { id, source, name },
            image_preview: None,
        }
    }
}

/// One painted composer image cell inside the attachment band.
#[derive(Clone, Debug)]
pub(super) struct ComposerImagePlacement {
    pub(super) image: FeedImage,
    /// Absolute row inside the composer attachment block (starts at 0).
    pub(super) row: usize,
    pub(super) column: u16,
    pub(super) width: u16,
    pub(super) height: usize,
}

/// Reserved composer attachment chrome: text lines and image placements.
#[derive(Clone, Debug, Default)]
pub(super) struct ComposerAttachmentLayout {
    pub(super) total_rows: usize,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) images: Vec<ComposerImagePlacement>,
}

/// Layout composer attachments into label/image chrome for one width.
///
/// Consecutive slots with previews share horizontal strips packed left-to-right
/// under a shared height. When gaps plus one column per image exceed `width`,
/// the run wraps onto additional strips so every preview stays visible.
/// Documents and pending items stay full-width rows.
pub(super) fn layout_composer_attachments(
    slots: &[ComposerAttachmentSlot],
    width: usize,
    max_height: ImageRowBudget,
) -> ComposerAttachmentLayout {
    let width = width.max(1);
    let max_height = max_height.get();
    let mut layout = ComposerAttachmentLayout::default();
    let mut index = 0usize;
    while index < slots.len() {
        if slots[index].image_preview.is_some() {
            let run_start = index;
            while index < slots.len() && slots[index].image_preview.is_some() {
                index += 1;
            }
            let mut offset = 0usize;
            let run_len = index - run_start;
            while offset < run_len {
                let strip_count = max_images_per_strip(width).min(run_len - offset);
                append_image_strip(
                    &mut layout,
                    slots,
                    run_start + offset,
                    strip_count,
                    width,
                    max_height,
                );
                offset += strip_count;
            }
            continue;
        }

        layout.lines.push(styled_line(
            slots[index].attachment.composer_label(index + 1),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
        layout.total_rows = layout.total_rows.saturating_add(1);
        index += 1;
    }
    debug_assert_eq!(layout.total_rows, layout.lines.len());
    layout
}

/// Max images that fit in one strip at `width` with a 1-column minimum cell.
fn max_images_per_strip(width: usize) -> usize {
    // count + GAP*(count-1) <= width  =>  count <= (width + GAP) / (1 + GAP)
    ((width + COMPOSER_IMAGE_GAP) / (1 + COMPOSER_IMAGE_GAP)).max(1)
}

fn append_image_strip(
    layout: &mut ComposerAttachmentLayout,
    slots: &[ComposerAttachmentSlot],
    run_start: usize,
    count: usize,
    width: usize,
    max_height: u16,
) {
    debug_assert!(count >= 1);
    let images_in_run: Vec<&FeedImage> = (0..count)
        .map(|offset| {
            slots[run_start + offset]
                .image_preview
                .as_ref()
                .expect("run only contains preview images")
        })
        .collect();

    // Shared strip height = tallest natural fit under the max budget.
    let mut strip_height = 1usize;
    for image in &images_in_run {
        let fitted = image.size_for(width, max_height);
        strip_height = strip_height.max(usize::from(fitted.height).max(1));
    }
    strip_height = strip_height.min(usize::from(max_height.max(1)));
    let strip_height_u16 = u16::try_from(strip_height).unwrap_or(u16::MAX).max(1);

    // Preferred width at that shared height (aspect preserved).
    let mut cell_widths: Vec<usize> = images_in_run
        .iter()
        .map(|image| usize::from(image.size_for(width, strip_height_u16).width.max(1)).min(width))
        .collect();

    // If the packed row overflows, shrink cells proportionally.
    let gap_total = COMPOSER_IMAGE_GAP.saturating_mul(count.saturating_sub(1));
    let content_budget = width.saturating_sub(gap_total).max(count);
    // Guarantee the packed columns fit: with strip partitioning, gap_total + count
    // is always <= width, so content_budget + gap_total <= width.
    debug_assert!(content_budget.saturating_add(gap_total) <= width || count == 1);
    let preferred_total: usize = cell_widths.iter().sum();
    if preferred_total > content_budget && preferred_total > 0 {
        let mut assigned = 0usize;
        for (i, cell) in cell_widths.iter_mut().enumerate() {
            if i + 1 == count {
                *cell = content_budget.saturating_sub(assigned).max(1);
            } else {
                let scaled = (*cell)
                    .saturating_mul(content_budget)
                    .saturating_div(preferred_total)
                    .max(1);
                *cell = scaled;
                assigned = assigned.saturating_add(scaled);
            }
        }
    }

    let image_row = layout.total_rows;
    layout
        .lines
        .extend((0..strip_height).map(|_| Line::raw("")));

    let mut spans = Vec::new();
    let mut column = 0usize;
    for (offset, image) in images_in_run.into_iter().enumerate() {
        let cell_width = cell_widths[offset].max(1);
        let attachment_index = run_start + offset;
        if offset > 0 {
            spans.push(Span::raw(" ".repeat(COMPOSER_IMAGE_GAP)));
        }
        let label = slots[attachment_index]
            .attachment
            .composer_label(attachment_index + 1);
        let truncated = truncate_one_line(&label, cell_width);
        let pad = cell_width.saturating_sub(display_width(&truncated));
        spans.push(Span::styled(truncated, Theme::dim()));
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        layout.images.push(ComposerImagePlacement {
            image: image.clone(),
            row: image_row,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            width: u16::try_from(cell_width).unwrap_or(u16::MAX).max(1),
            height: strip_height,
        });
        column = column
            .saturating_add(cell_width)
            .saturating_add(COMPOSER_IMAGE_GAP);
    }
    debug_assert!(
        column.saturating_sub(COMPOSER_IMAGE_GAP) <= width,
        "strip must not overflow width ({column} vs {width})"
    );
    let used = spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum::<usize>();
    if used < width {
        spans.push(Span::raw(" ".repeat(width - used)));
    }
    layout.lines.push(Line::from(spans));
    layout.total_rows = layout
        .total_rows
        .saturating_add(strip_height)
        .saturating_add(1);
}

impl App {
    /// Rows reserved above composer text for attachment labels / image previews.
    pub(super) fn composer_attachment_row_count(&self, width: usize) -> usize {
        self.composer_attachment_layout(width).total_rows
    }

    pub(super) fn composer_attachment_layout(&self, width: usize) -> ComposerAttachmentLayout {
        if let Some(cache) = self.composer_attachment_layout_cache.as_ref() {
            if cache.width == width
                && cache.epoch == self.input_ui.attachment_epoch()
                && cache.theme_generation == Theme::generation()
            {
                return cache.layout.clone();
            }
        }
        debug_assert!(
            false,
            "composer attachment layout should be refreshed for this frame"
        );
        layout_composer_attachments(
            self.input_ui.attachment_slots(),
            width,
            ImageRowBudget::composer(),
        )
    }

    /// Cache layout for this frame so lines, cursor, and paint share one pass.
    pub(super) fn refresh_composer_attachment_layout_cache(&mut self, width: usize) {
        let epoch = self.input_ui.attachment_epoch();
        let theme_generation = Theme::generation();
        if self
            .composer_attachment_layout_cache
            .as_ref()
            .is_some_and(|cache| {
                cache.width == width
                    && cache.epoch == epoch
                    && cache.theme_generation == theme_generation
            })
        {
            return;
        }
        let layout = layout_composer_attachments(
            self.input_ui.attachment_slots(),
            width,
            ImageRowBudget::composer(),
        );
        self.composer_attachment_layout_cache = Some(ComposerAttachmentLayoutCache {
            epoch,
            width,
            theme_generation,
            layout,
        });
    }

    pub(super) fn composer_attachment_lines(&self, width: usize) -> Vec<Line<'static>> {
        self.composer_attachment_layout(width).lines
    }

    pub(super) fn render_composer_images(
        &self,
        frame: &mut Frame<'_>,
        composer_area: Rect,
        width: usize,
        composer_start: usize,
    ) {
        if composer_area.height == 0 || !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return;
        }
        let layout = self.composer_attachment_layout(width);
        let visible_end = composer_start.saturating_add(composer_area.height as usize);
        for placement in &layout.images {
            if placement.row < composer_start
                || placement.row.saturating_add(placement.height) > visible_end
            {
                // Match history: only paint when the full reserved block fits.
                continue;
            }
            let image_y = composer_area
                .y
                .saturating_add((placement.row - composer_start) as u16);
            let available_height = composer_area.bottom().saturating_sub(image_y);
            let visible_height = (placement.height as u16).min(available_height);
            if visible_height == 0 || placement.width == 0 {
                continue;
            }
            let max_width = composer_area
                .width
                .saturating_sub(placement.column.min(composer_area.width));
            let paint_width = placement.width.min(max_width);
            if paint_width == 0 {
                continue;
            }
            placement.image.render(
                frame,
                Rect::new(
                    composer_area.x.saturating_add(placement.column),
                    image_y,
                    paint_width,
                    visible_height,
                ),
            );
        }
    }
}

/// Frame cache so composer lines, cursor offset, and paint share one layout.
#[derive(Clone, Debug)]
pub(super) struct ComposerAttachmentLayoutCache {
    pub(super) epoch: u64,
    pub(super) width: usize,
    pub(super) theme_generation: u64,
    pub(super) layout: ComposerAttachmentLayout,
}

#[cfg(test)]
#[path = "composer_attachments_tests.rs"]
mod tests;
