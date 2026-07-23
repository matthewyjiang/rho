use std::{cell::RefCell, fmt, io::Cursor, ops::Range, rc::Rc};

use image::{DynamicImage, ImageReader, Limits};
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

/// A decoded image that can cross a background task boundary before
/// terminal-specific render state is created on the UI thread.
pub(super) struct DecodedFeedImage {
    image: DynamicImage,
    estimated_bytes: usize,
}

impl fmt::Debug for FeedImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("FeedImage").finish_non_exhaustive()
    }
}

impl FeedImage {
    pub(super) fn load(asset: &ToolAsset, picker: &Picker) -> image::ImageResult<Self> {
        Self::decode(asset.bytes()).map(|image| image.to_feed_image(picker))
    }

    pub(super) fn decode(bytes: &[u8]) -> image::ImageResult<DecodedFeedImage> {
        let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(MAX_THUMBNAIL_WIDTH);
        limits.max_image_height = Some(MAX_THUMBNAIL_HEIGHT);
        limits.max_alloc = Some(MAX_THUMBNAIL_ALLOCATION);
        reader.limits(limits);
        let image = reader.decode()?;
        let estimated_bytes = image.as_bytes().len();
        Ok(DecodedFeedImage {
            image,
            estimated_bytes,
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

impl DecodedFeedImage {
    pub(super) fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    pub(super) fn to_feed_image(&self, picker: &Picker) -> FeedImage {
        FeedImage {
            state: Rc::new(RefCell::new(picker.new_resize_protocol(self.image.clone()))),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct RenderedImagePlacements {
    placements: Vec<RenderedImagePlacement>,
}

impl RenderedImagePlacements {
    pub(super) fn single(placement: RenderedImagePlacement) -> Self {
        Self {
            placements: vec![placement],
        }
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &RenderedImagePlacement> {
        self.placements.iter()
    }

    pub(super) fn offset_rows(&self, offset: usize) -> Self {
        Self {
            placements: self
                .placements
                .iter()
                .cloned()
                .map(|placement| placement.offset_rows(offset))
                .collect(),
        }
    }

    /// Keeps only placements that start before `line`, used when the cache
    /// truncates rendered history.
    pub(super) fn retain_starting_before(&self, line: usize) -> Option<Self> {
        let placements: Vec<_> = self
            .placements
            .iter()
            .filter(|placement| placement.rows.start < line)
            .cloned()
            .collect();
        (!placements.is_empty()).then_some(Self { placements })
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

/// Replaces loaded markdown image fallback rows with image placements. Images
/// retain their source indices, so failed loads cannot shift later images.
pub(super) fn reserve_markdown_image_rows(
    lines: &mut Vec<Line<'static>>,
    placeholder_rows: &[usize],
    images: &[(usize, FeedImage)],
    width: usize,
) -> Option<RenderedImagePlacements> {
    let mut offset = 0usize;
    let mut placements = Vec::new();
    for (source_index, image) in images {
        let Some(&placeholder_row) = placeholder_rows.get(*source_index) else {
            continue;
        };
        let start = placeholder_row + offset;
        lines[start] = Line::raw("");
        let extra_rows = image.height_for_width(width).saturating_sub(1);
        lines.splice(start + 1..start + 1, (0..extra_rows).map(|_| Line::raw("")));
        placements.push(RenderedImagePlacement {
            image: image.clone(),
            rows: start..start + 1 + extra_rows,
        });
        offset += extra_rows;
    }
    (!placements.is_empty()).then_some(RenderedImagePlacements { placements })
}

pub(super) fn reserve_entry_image_rows(
    lines: &mut Vec<Line<'static>>,
    entry: &super::Entry,
    width: usize,
) -> Option<RenderedImagePlacements> {
    match entry {
        super::Entry::Tool(tool) => tool.image.as_ref().map(|image| {
            RenderedImagePlacements::single(reserve_image_rows(lines, image, width).offset_rows(1))
        }),
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
        let cwd = self.info.runtime.cwd.clone();
        let markdown_images = &self.history.markdown_images;
        let mut placements = self.history.history_lines.visible_image_placements(
            &self.history.transcript,
            width,
            self.info.runtime.max_tool_output_lines,
            transcript_start,
            transcript_count,
            &|entry_index, sources| markdown_images.ready_images(entry_index, sources, &cwd),
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
                // History lines are padded by one column on each side.
                Rect::new(
                    history_area.x.saturating_add(1),
                    image_y,
                    history_area.width.saturating_sub(2),
                    visible_height,
                ),
            );
        }
    }
}

/// Uses conservative environment hints without probing stdin. Persistent tmux
/// sessions are kept on the text fallback because terminal-specific variables
/// can describe a previous client rather than the active attachment.
///
/// Under Herdr, Ghostty/Kitty environment variables describe the outer host
/// terminal. Herdr intercepts Kitty sequences and only paints them when the
/// active client reports cell metrics. When that path is unavailable, Rho keeps
/// previews on halfblocks so reserved feed rows are not left blank.
pub(super) fn picker_from_environment(
    herdr_graphics: crate::herdr::HerdrGraphicsCapability,
) -> Option<Picker> {
    let in_tmux = std::env::var_os("TMUX").is_some()
        || std::env::var("TERM_PROGRAM").is_ok_and(|value| value.eq_ignore_ascii_case("tmux"));
    let host_supports_kitty = kitty_graphics_environment(
        in_tmux,
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
        std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    );
    picker_for_environment(host_supports_kitty, herdr_graphics)
}

pub(super) fn picker_for_environment(
    host_supports_kitty: bool,
    herdr_graphics: crate::herdr::HerdrGraphicsCapability,
) -> Option<Picker> {
    if !host_supports_kitty {
        return None;
    }
    let protocol = match herdr_graphics {
        crate::herdr::HerdrGraphicsCapability::Unpaintable => ProtocolType::Halfblocks,
        crate::herdr::HerdrGraphicsCapability::NotHerdr
        | crate::herdr::HerdrGraphicsCapability::Paintable => ProtocolType::Kitty,
    };
    let mut picker = Picker::halfblocks();
    picker.set_protocol_type(protocol);
    Some(picker)
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
