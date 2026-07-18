use std::{cell::RefCell, fmt, io::Cursor, ops::Range, rc::Rc};

use image::{ImageReader, Limits};
use ratatui::{
    layout::{Rect, Size},
    text::Line,
    Frame,
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
    Resize, StatefulImage,
};
use rho_sdk::tool::ToolAsset;

pub(super) const IMAGE_HEIGHT: u16 = 12;
const MAX_THUMBNAIL_WIDTH: u32 = 1_024;
const MAX_THUMBNAIL_HEIGHT: u32 = 768;
const MAX_THUMBNAIL_ALLOCATION: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct FeedImage {
    state: Rc<RefCell<StatefulProtocol>>,
}

impl fmt::Debug for FeedImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("FeedImage").finish_non_exhaustive()
    }
}

impl FeedImage {
    pub(super) fn load(asset: &ToolAsset, picker: &Picker) -> image::ImageResult<Self> {
        let mut reader = ImageReader::new(Cursor::new(asset.bytes())).with_guessed_format()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_THUMBNAIL_WIDTH);
        limits.max_image_height = Some(MAX_THUMBNAIL_HEIGHT);
        limits.max_alloc = Some(MAX_THUMBNAIL_ALLOCATION);
        reader.limits(limits);
        let thumbnail = reader.decode()?;
        Ok(Self {
            state: Rc::new(RefCell::new(picker.new_resize_protocol(thumbnail))),
        })
    }

    pub(super) fn height_for_width(&self, width: usize) -> usize {
        let width = u16::try_from(width).unwrap_or(u16::MAX);
        usize::from(
            self.state
                .borrow()
                .size_for(Resize::Fit(None), Size::new(width, IMAGE_HEIGHT))
                .height
                .max(1),
        )
    }

    pub(super) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        frame.render_stateful_widget(
            StatefulImage::default().resize(Resize::Fit(None)),
            area,
            &mut *self.state.borrow_mut(),
        );
    }
}

#[derive(Clone, Debug)]
pub(super) struct RenderedImagePlacement {
    pub(super) image: FeedImage,
    pub(super) rows: Range<usize>,
}

impl RenderedImagePlacement {
    pub(super) fn offset_rows(mut self, offset: usize) -> Self {
        self.rows = self.rows.start + offset..self.rows.end + offset;
        self
    }
}

pub(super) fn reserve_image_rows(
    lines: &mut Vec<Line<'static>>,
    image: &FeedImage,
    width: usize,
) -> RenderedImagePlacement {
    let start = lines.len();
    let height = image.height_for_width(width);
    lines.extend((0..height).map(|_| Line::raw("")));
    RenderedImagePlacement {
        image: image.clone(),
        rows: start..start + height,
    }
}

pub(super) fn reserve_optional_image_rows(
    lines: &mut Vec<Line<'static>>,
    image: Option<&FeedImage>,
    width: usize,
) {
    if let Some(image) = image {
        reserve_image_rows(lines, image, width);
    }
}

pub(super) fn reserve_entry_image_rows(
    lines: &mut Vec<Line<'static>>,
    entry: &super::Entry,
    width: usize,
) -> Option<RenderedImagePlacement> {
    match entry {
        super::Entry::Tool(tool) => tool
            .image
            .as_ref()
            .map(|image| reserve_image_rows(lines, image, width).offset_rows(1)),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(super) struct VisibleImagePlacement {
    pub(super) image: FeedImage,
    pub(super) row: usize,
    pub(super) height: usize,
}

impl super::App {
    pub(super) fn load_feed_image(
        &mut self,
        asset: &ToolAsset,
    ) -> image::ImageResult<Option<FeedImage>> {
        let Some(picker) = &self.image_picker else {
            return Ok(None);
        };
        let image = FeedImage::load(asset, picker)?;
        Ok(Some(image))
    }

    pub(super) fn visible_history_image_placements(
        &mut self,
        width: usize,
        start: usize,
        count: usize,
    ) -> Vec<VisibleImagePlacement> {
        if count == 0 {
            return Vec::new();
        }
        let header_len = self.session_header_lines(width).len();
        let visible_header_lines = if start < header_len {
            count.min(header_len - start)
        } else {
            0
        };
        let transcript_start = start.saturating_sub(header_len);
        let transcript_count = count.saturating_sub(visible_header_lines);
        let mut placements = self.history_lines.visible_image_placements(
            &self.transcript,
            width,
            self.info.max_tool_output_lines,
            transcript_start,
            transcript_count,
        );
        placements.iter_mut().for_each(|placement| {
            placement.row = placement.row.saturating_add(visible_header_lines);
        });
        placements
    }

    pub(super) fn render_feed_images(
        &self,
        frame: &mut Frame<'_>,
        history_area: Rect,
        visible_images: &[VisibleImagePlacement],
    ) {
        for placement in visible_images {
            let image_y = history_area.y.saturating_add(placement.row as u16);
            let available_height = history_area.bottom().saturating_sub(image_y);
            let visible_height = (placement.height as u16).min(available_height);
            if visible_height == 0 {
                continue;
            }
            placement.image.render(
                frame,
                Rect::new(history_area.x, image_y, history_area.width, visible_height),
            );
        }
    }
}

/// Uses conservative environment hints without probing stdin. Persistent tmux
/// sessions are kept on the text fallback because terminal-specific variables
/// can describe a previous client rather than the active attachment.
pub(super) fn picker_from_environment() -> Option<Picker> {
    let in_tmux = std::env::var_os("TMUX").is_some()
        || std::env::var("TERM_PROGRAM").is_ok_and(|value| value.eq_ignore_ascii_case("tmux"));
    kitty_graphics_environment(
        in_tmux,
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
        std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
    .then(|| {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        picker
    })
}

fn kitty_graphics_environment(
    in_tmux: bool,
    has_kitty_window_id: bool,
    has_ghostty_resources: bool,
    term_program: Option<&str>,
    term: Option<&str>,
) -> bool {
    !in_tmux
        && (has_kitty_window_id
            || has_ghostty_resources
            || term_program.is_some_and(|program| {
                matches!(program.to_ascii_lowercase().as_str(), "kitty" | "ghostty")
            })
            || term.is_some_and(|term| {
                let term = term.to_ascii_lowercase();
                term.contains("kitty") || term.contains("ghostty")
            }))
}

#[cfg(test)]
#[path = "feed_image_tests.rs"]
mod tests;
