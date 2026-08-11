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

/// Floor for reserved feed-image rows so wide images stay readable.
pub(super) const MIN_IMAGE_HEIGHT: u16 = 16;
/// Ceiling so one image cannot dominate the transcript.
pub(super) const MAX_IMAGE_HEIGHT: u16 = 40;
/// Mid band and default when terminal height is unknown (tests, recovery).
pub(super) const DEFAULT_IMAGE_HEIGHT: u16 = 24;
/// Tall-but-not-max band between default and ceiling.
pub(super) const TALL_IMAGE_HEIGHT: u16 = 32;
/// Max rows for an image preview above the composer text.
pub(super) const COMPOSER_IMAGE_HEIGHT: u16 = 6;

const MAX_THUMBNAIL_WIDTH: u32 = 1_024;
const MAX_THUMBNAIL_HEIGHT: u32 = 768;
const MAX_THUMBNAIL_ALLOCATION: u64 = 8 * 1024 * 1024;
/// Pasted composer images are often full-resolution; decode under the same
/// bound `read_file` uses before shrinking to the feed thumbnail box.
const MAX_COMPOSER_DECODE_DIMENSION: u32 = 4_096;
const MAX_COMPOSER_DECODE_ALLOCATION: u64 = 80 * 1024 * 1024;

/// Max rows one feed image may reserve, from terminal height bands.
///
/// Discrete tiers keep the history line cache stable when the composer grows
/// (wraps, attachment strips) without changing the terminal size.
pub(super) fn max_feed_image_height(terminal_height: usize) -> u16 {
    match terminal_height {
        0..=28 => MIN_IMAGE_HEIGHT,
        29..=44 => DEFAULT_IMAGE_HEIGHT,
        45..=64 => TALL_IMAGE_HEIGHT,
        _ => MAX_IMAGE_HEIGHT,
    }
}

/// Row budget for fitting a feed or composer image into the terminal grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ImageRowBudget(u16);

impl ImageRowBudget {
    pub(super) fn get(self) -> u16 {
        self.0.max(1)
    }

    pub(super) fn feed_from_terminal_height(terminal_height: usize) -> Self {
        if terminal_height == 0 {
            Self::default_feed()
        } else {
            Self(max_feed_image_height(terminal_height))
        }
    }

    pub(super) fn composer() -> Self {
        Self(COMPOSER_IMAGE_HEIGHT)
    }

    pub(super) const fn default_feed() -> Self {
        Self(DEFAULT_IMAGE_HEIGHT)
    }
}

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

    /// Decode a feed/tool asset that is already within the thumbnail box.
    pub(super) fn decode(bytes: &[u8]) -> image::ImageResult<DecodedFeedImage> {
        decode_bounded_image(
            bytes,
            MAX_THUMBNAIL_WIDTH,
            MAX_THUMBNAIL_HEIGHT,
            MAX_THUMBNAIL_ALLOCATION,
        )
    }

    /// Decode a full-resolution composer paste under the larger bound, then
    /// shrink to the feed thumbnail box. Safe to run on a worker thread.
    pub(super) fn decode_composer_base64(
        data: &str,
    ) -> Result<DecodedFeedImage, ComposerImageLoadError> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let bytes = STANDARD
            .decode(data.trim())
            .map_err(|_| ComposerImageLoadError::InvalidBase64)?;
        let decoded = decode_bounded_image(
            &bytes,
            MAX_COMPOSER_DECODE_DIMENSION,
            MAX_COMPOSER_DECODE_DIMENSION,
            MAX_COMPOSER_DECODE_ALLOCATION,
        )
        .map_err(|_| ComposerImageLoadError::Decode)?;
        let image = decoded
            .image
            .thumbnail(MAX_THUMBNAIL_WIDTH, MAX_THUMBNAIL_HEIGHT);
        let estimated_bytes = image.as_bytes().len();
        Ok(DecodedFeedImage {
            image,
            estimated_bytes,
        })
    }

    pub(super) fn height_for_width(&self, width: usize, max_height: u16) -> usize {
        usize::from(self.size_for(width, max_height).height.max(1))
    }

    pub(super) fn size_for(&self, width: usize, max_height: u16) -> Size {
        let width = u16::try_from(width).unwrap_or(u16::MAX).max(1);
        let max_height = max_height.max(1);
        self.state
            .borrow()
            .size_for(Resize::Fit(None), Size::new(width, max_height))
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

fn decode_bounded_image(
    bytes: &[u8],
    max_width: u32,
    max_height: u32,
    max_alloc: u64,
) -> image::ImageResult<DecodedFeedImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_width);
    limits.max_image_height = Some(max_height);
    limits.max_alloc = Some(max_alloc);
    reader.limits(limits);
    let image = reader.decode()?;
    let estimated_bytes = image.as_bytes().len();
    Ok(DecodedFeedImage {
        image,
        estimated_bytes,
    })
}

#[derive(Debug)]
pub(super) enum ComposerImageLoadError {
    InvalidBase64,
    Decode,
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

    pub(super) fn from_placements(placements: Vec<RenderedImagePlacement>) -> Self {
        Self { placements }
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
    max_height: u16,
) -> RenderedImagePlacement {
    let start = lines.len();
    let height = image.height_for_width(width, max_height);
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
    max_height: u16,
) {
    if let Some(image) = image {
        reserve_image_rows(lines, image, width, max_height);
    }
}

/// Replaces loaded markdown image fallback rows with image placements. Images
/// retain their source indices, so failed loads cannot shift later images.
pub(super) fn reserve_markdown_image_rows(
    lines: &mut Vec<Line<'static>>,
    placeholder_rows: &[usize],
    images: &[(usize, FeedImage)],
    width: usize,
    max_height: u16,
) -> Option<RenderedImagePlacements> {
    let mut offset = 0usize;
    let mut placements = Vec::new();
    for (source_index, image) in images {
        let Some(&placeholder_row) = placeholder_rows.get(*source_index) else {
            continue;
        };
        let start = placeholder_row + offset;
        lines[start] = Line::raw("");
        let extra_rows = image.height_for_width(width, max_height).saturating_sub(1);
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
    max_height: u16,
) -> Option<RenderedImagePlacements> {
    match entry {
        super::Entry::Tool(tool) => tool.image.as_ref().map(|image| {
            // Content starts at row 0; trailing spacer is after the image rows.
            RenderedImagePlacements::single(reserve_image_rows(lines, image, width, max_height))
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
        self.sync_open_stream_tail();
        let header_len = self.session_header_lines(width).len();
        let visible_header_lines = if start < header_len {
            count.min(header_len - start)
        } else {
            0
        };
        let transcript_start = start.saturating_sub(header_len);
        let transcript_count = count.saturating_sub(visible_header_lines);
        let cwd = self.info.runtime.cwd.clone();
        let settings = self.history_render_settings(width);
        let mut placements =
            self.history
                .with_lines_and_images_mut(|history_lines, entries, markdown_images| {
                    history_lines.visible_image_placements(
                        entries,
                        settings,
                        transcript_start,
                        transcript_count,
                        &|entry_index, sources| {
                            markdown_images.ready_images(entry_index, sources, &cwd)
                        },
                    )
                });
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
