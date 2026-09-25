//! Composer attachment chrome: slots, strip layout, labels, and paint.
//!
//! Ready images may carry a Kitty/halfblock preview. Consecutive previews share
//! width-bounded horizontal strips (wrapping when gaps would overflow); documents
//! and pending items stay full-width label rows. Every label row ends in a
//! `✕` remove button, always painted so it works without motion reporting; a
//! press on the button removes that attachment, hover lifts it, and presses
//! anywhere else on a preview do nothing.

use std::ops::Range;

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    Frame,
};

use super::{
    display_width,
    feed_image::{FeedImage, ImageRowBudget},
    truncate_one_line, App, ChatMedia, ComposerAttachment, ComposerMode, MediaAttachId,
    PendingAttachmentSource, Theme,
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

/// The remove button painted at the end of one attachment's label row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AttachmentTarget {
    /// Slot index in the composer attachment list.
    pub(super) attachment: usize,
    /// Block row (starts at 0) of the label row holding the button.
    pub(super) row: usize,
    /// Block columns the button paints.
    pub(super) columns: Range<u16>,
}

/// Reserved composer attachment chrome: text lines, image placements, and
/// pointer targets.
#[derive(Clone, Debug, Default)]
pub(super) struct ComposerAttachmentLayout {
    pub(super) total_rows: usize,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) images: Vec<ComposerImagePlacement>,
    pub(super) targets: Vec<AttachmentTarget>,
}

/// Remove button under screen cell (`column`, `row`) of a composer painted at
/// `composer_area` from composer line `composer_start`.
///
/// A button is a target only while its label row is on screen, so a press
/// always lands on a `✕` the user can see.
pub(super) fn attachment_target_at(
    layout: &ComposerAttachmentLayout,
    composer_area: Rect,
    composer_start: usize,
    column: u16,
    row: u16,
) -> Option<&AttachmentTarget> {
    if !composer_area.contains(ratatui::layout::Position { x: column, y: row }) {
        return None;
    }
    let block_row = composer_start.saturating_add(usize::from(row - composer_area.y));
    let block_column = column - composer_area.x;
    layout
        .targets
        .iter()
        .find(|target| target.row == block_row && target.columns.contains(&block_column))
}

/// Remove button painted at the end of every attachment label row.
const REMOVE_BUTTON: &str = " ✕ ";
/// Fallback when a label cell is too narrow for [`REMOVE_BUTTON`].
const REMOVE_BUTTON_NARROW: &str = "✕";

/// One attachment label fitted into `width` columns, followed by its remove
/// button. The button always fits; the label truncates first. Returns the
/// spans and the button's columns relative to the label start.
fn label_with_remove_button(label: &str, width: usize) -> (Vec<Span<'static>>, Range<usize>) {
    let button = if width >= display_width(REMOVE_BUTTON) {
        REMOVE_BUTTON
    } else {
        REMOVE_BUTTON_NARROW
    };
    let button_width = display_width(button);
    // One space keeps the label from touching the button.
    let label_budget = width.saturating_sub(button_width + 1);
    let mut spans = Vec::with_capacity(3);
    let mut column = 0;
    if label_budget > 0 {
        let label = truncate_one_line(label, label_budget);
        column = display_width(&label) + 1;
        spans.push(Span::styled(label, Theme::dim()));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        button,
        Theme::markdown_code_copy_button(/*hovered*/ false),
    ));
    (spans, column..column + button_width)
}

fn u16_columns(columns: Range<usize>) -> Range<u16> {
    u16::try_from(columns.start).unwrap_or(u16::MAX)..u16::try_from(columns.end).unwrap_or(u16::MAX)
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

        let label = slots[index].attachment.composer_label(index + 1);
        let (spans, button) = label_with_remove_button(&label, width);
        layout.targets.push(AttachmentTarget {
            attachment: index,
            row: layout.total_rows,
            columns: u16_columns(button),
        });
        layout.lines.push(Line::from(spans));
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
        let (label_spans, button) = label_with_remove_button(&label, cell_width);
        let pad = cell_width.saturating_sub(button.end);
        spans.extend(label_spans);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        let placement = ComposerImagePlacement {
            image: image.clone(),
            row: image_row,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            width: u16::try_from(cell_width).unwrap_or(u16::MAX).max(1),
            height: strip_height,
        };
        layout.targets.push(AttachmentTarget {
            attachment: attachment_index,
            row: image_row.saturating_add(strip_height),
            columns: u16_columns(column + button.start..column + button.end),
        });
        layout.images.push(placement);
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

    /// Remove the attachment at `index`, cancelling its task when it is still
    /// pending. Backspace and the `✕` button both remove through here, so
    /// they report the same status.
    pub(super) fn remove_composer_attachment(&mut self, index: usize) {
        match self.input_ui.remove_attachment(index) {
            Some(ComposerAttachment::Pending { id, .. }) => {
                self.cancel_pending_attachment(id);
                let pending_count = self.input_ui.pending_attachment_count();
                self.set_status(if pending_count == 0 {
                    "document extraction cancelled".to_string()
                } else {
                    format!("extracting files: {pending_count}")
                });
            }
            Some(ComposerAttachment::Ready(_)) => {
                self.set_status(format!(
                    "attachments: {}",
                    self.input_ui.attachment_slots().len()
                ));
            }
            None => {}
        }
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
                // Composer previews stay all-or-nothing so scrolling cannot
                // split a row of side-by-side attachments.
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
        self.lift_hovered_remove_button(frame, composer_area, &layout, composer_start);
    }

    /// Lift the remove button under the pointer. The button row is plain
    /// text for every attachment kind, so the lift never fights a
    /// graphics-protocol image cell.
    fn lift_hovered_remove_button(
        &self,
        frame: &mut Frame<'_>,
        composer_area: Rect,
        layout: &ComposerAttachmentLayout,
        composer_start: usize,
    ) {
        let Some(target) = self.last_mouse_position.and_then(|(column, row)| {
            attachment_target_at(layout, composer_area, composer_start, column, row)
        }) else {
            return;
        };
        // In range: the target row is on screen, inside `composer_area`.
        let y = composer_area.y + (target.row - composer_start) as u16;
        let x = composer_area.x.saturating_add(target.columns.start);
        let width = target
            .columns
            .end
            .min(composer_area.width)
            .saturating_sub(target.columns.start);
        frame.buffer_mut().set_style(
            Rect::new(x, y, width, 1),
            Theme::markdown_code_copy_button(/*hovered*/ true),
        );
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
