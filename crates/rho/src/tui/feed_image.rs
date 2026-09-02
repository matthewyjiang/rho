use std::{cell::RefCell, fmt, io::Cursor, ops::Range, rc::Rc};

use image::{DynamicImage, ImageReader, Limits};
use ratatui::{
    layout::{Rect, Size},
    text::Line,
    Frame,
};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    sliced::{SignedPosition, SlicedImage, SlicedProtocol},
    FontSize, Resize,
};
use rho_sdk::tool::ToolAsset;

/// Compact-terminal floor so reserved images stay paintable in short panes.
pub(super) const COMPACT_IMAGE_HEIGHT: u16 = 12;
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

/// Terminal-height floors and the feed-image band each floor unlocks.
///
/// One table owns both preferred terminal budgeting and content-cap
/// quantization so the tiers cannot drift apart.
const FEED_IMAGE_HEIGHT_TIERS: [(usize, u16); 5] = [
    (0, COMPACT_IMAGE_HEIGHT),
    (25, MIN_IMAGE_HEIGHT),
    (37, DEFAULT_IMAGE_HEIGHT),
    (53, TALL_IMAGE_HEIGHT),
    (69, MAX_IMAGE_HEIGHT),
];

const MAX_THUMBNAIL_WIDTH: u32 = 1_024;
const MAX_THUMBNAIL_HEIGHT: u32 = 768;
const MAX_THUMBNAIL_ALLOCATION: u64 = 8 * 1024 * 1024;
/// Pasted composer images are often full-resolution; decode under the same
/// bound `read_file` uses before shrinking to the feed thumbnail box.
const MAX_COMPOSER_DECODE_DIMENSION: u32 = 4_096;
const MAX_COMPOSER_DECODE_ALLOCATION: u64 = 80 * 1024 * 1024;

/// Max rows one feed image may reserve, from terminal height bands.
///
/// Discrete tiers keep the preferred budget stable across small layout shifts.
/// Call [`feed_image_height_budget`] to also cap by the live history content
/// viewport so a reservation never exceeds what can fully paint.
pub(super) fn max_feed_image_height(terminal_height: usize) -> u16 {
    let mut band = COMPACT_IMAGE_HEIGHT;
    for &(min_terminal_height, next_band) in &FEED_IMAGE_HEIGHT_TIERS {
        if terminal_height >= min_terminal_height {
            band = next_band;
        } else {
            break;
        }
    }
    band
}

/// Preferred terminal-height band, capped by the live history content viewport.
///
/// When `history_content_height` is zero (unknown geometry), the preferred band
/// is kept so tests and recovery budgeting stay deterministic. A nonzero content
/// height never allows a reservation taller than the visible content rows, so
/// [`visible_image_placements`] can still return a fully paintable block after
/// composer chrome shrinks the pane.
///
/// The content cap snaps down to the same discrete bands as
/// [`max_feed_image_height`] (or the raw height when below the compact floor).
/// Without that snap, a one-row composer or activity change rewrites
/// `max_image_height` and forces a full transcript line-cache rebuild.
pub(super) fn feed_image_height_budget(
    terminal_height: usize,
    history_content_height: usize,
) -> u16 {
    let preferred = if terminal_height == 0 {
        DEFAULT_IMAGE_HEIGHT
    } else {
        max_feed_image_height(terminal_height)
    };
    if history_content_height == 0 {
        preferred
    } else {
        preferred.min(quantize_content_image_cap(history_content_height))
    }
}

/// Largest feed-image band that still fits in `history_content_height`.
///
/// Heights below [`COMPACT_IMAGE_HEIGHT`] stay exact so tiny panes keep a tight
/// paintable cap. Larger panes jump between the stable terminal bands.
pub(super) fn quantize_content_image_cap(history_content_height: usize) -> u16 {
    let height = u16::try_from(history_content_height)
        .unwrap_or(u16::MAX)
        .max(1);
    for &(_, band) in FEED_IMAGE_HEIGHT_TIERS.iter().rev() {
        if height >= band {
            return band;
        }
    }
    height
}

/// Row budget for fitting a feed or composer image into the terminal grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ImageRowBudget(u16);

impl ImageRowBudget {
    pub(super) fn get(self) -> u16 {
        self.0.max(1)
    }

    pub(super) fn feed(terminal_height: usize, history_content_height: usize) -> Self {
        Self(feed_image_height_budget(
            terminal_height,
            history_content_height,
        ))
    }

    pub(super) fn composer() -> Self {
        Self(COMPOSER_IMAGE_HEIGHT)
    }
}

#[derive(Clone)]
pub(super) struct FeedImage {
    inner: Rc<FeedImageState>,
}

struct FeedImageState {
    source: DynamicImage,
    picker: Picker,
    protocol: RefCell<Option<SlicedRenderState>>,
}

struct SlicedRenderState {
    size: Size,
    protocol: SlicedProtocol,
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
        let available = Size::new(
            u16::try_from(width).unwrap_or(u16::MAX).max(1),
            max_height.max(1),
        );
        Resize::Fit(None).size_for(&self.inner.source, self.inner.picker.font_size(), available)
    }

    pub(super) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        self.render_partial(frame, area, usize::from(area.height), 0);
    }

    /// Render a fixed-size image while dropping rows outside the visible area.
    pub(super) fn render_partial(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        full_height: usize,
        skip_rows: usize,
    ) {
        let full_height = u16::try_from(full_height).unwrap_or(u16::MAX).max(1);
        let size = self.size_for(usize::from(area.width), full_height);
        let mut cached = self.inner.protocol.borrow_mut();
        if cached.as_ref().is_none_or(|cached| cached.size != size) {
            let Ok(protocol) = SlicedProtocol::new_with_resize(
                &self.inner.picker,
                self.inner.source.clone(),
                size,
                Resize::Fit(None),
            ) else {
                return;
            };
            *cached = Some(SlicedRenderState { size, protocol });
        }

        let Some(cached) = cached.as_ref() else {
            return;
        };
        let skip_rows = i16::try_from(skip_rows)
            .unwrap_or(i16::MAX)
            .saturating_neg();
        frame.render_widget(
            SlicedImage::new(&cached.protocol, SignedPosition { x: 0, y: skip_rows }),
            area,
        );
    }
}

impl DecodedFeedImage {
    pub(super) fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    pub(super) fn to_feed_image(&self, picker: &Picker) -> FeedImage {
        FeedImage {
            inner: Rc::new(FeedImageState {
                source: self.image.clone(),
                picker: picker.clone(),
                protocol: RefCell::new(None),
            }),
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

    pub(super) fn iter(&self) -> impl Iterator<Item = &RenderedImagePlacement> {
        self.placements.iter()
    }
}

#[derive(Clone, Debug)]
pub(super) struct RenderedImagePlacement {
    pub(super) image: FeedImage,
    pub(super) rows: Range<usize>,
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
    /// Number of image rows inside the current viewport.
    pub(super) height: usize,
    /// Total rows occupied by the image before viewport clipping.
    pub(super) total_height: usize,
    /// Number of image rows above the current viewport.
    pub(super) skip_rows: usize,
}

pub(super) fn preview_generated_image(
    image: &rho_providers::model::ImageContent,
    picker: Option<&Picker>,
) -> Result<Option<FeedImage>, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image.data.trim())
        .map_err(|_| "generated image was not valid base64".to_string())?;
    load_optional_feed_image(picker, &ToolAsset::new(&image.mime_type, bytes))
        .map_err(|error| error.to_string())
}

fn load_optional_feed_image(
    picker: Option<&Picker>,
    asset: &ToolAsset,
) -> image::ImageResult<Option<FeedImage>> {
    let Some(picker) = picker else {
        return Ok(None);
    };
    FeedImage::load(asset, picker).map(Some)
}

impl super::App {
    pub(super) fn load_feed_image(
        &mut self,
        asset: &ToolAsset,
    ) -> image::ImageResult<Option<FeedImage>> {
        load_optional_feed_image(self.image_picker.as_ref(), asset)
    }

    pub(super) fn visible_history_image_placements(
        &mut self,
        width: usize,
        settings: super::history_cache::HistoryRenderSettings,
        start: usize,
        count: usize,
    ) -> Vec<VisibleImagePlacement> {
        if count == 0 {
            return Vec::new();
        }
        self.sync_open_stream_tail();
        let header_len = self.visible_session_header_len(width);
        let visible_header_lines = if start < header_len {
            count.min(header_len - start)
        } else {
            0
        };
        let transcript_start = start.saturating_sub(header_len);
        let transcript_count = count.saturating_sub(visible_header_lines);
        let cwd = self.info.runtime.cwd.clone();
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
            // History lines are padded by one column on each side.
            let image_area = Rect::new(
                history_area.x.saturating_add(1),
                image_y,
                history_area.width.saturating_sub(2),
                visible_height,
            );
            placement.image.render_partial(
                frame,
                image_area,
                placement.total_height,
                placement.skip_rows,
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
    let (protocol, cell) = match herdr_graphics {
        crate::herdr::HerdrGraphicsCapability::Unpaintable => (ProtocolType::Halfblocks, None),
        crate::herdr::HerdrGraphicsCapability::NotHerdr => {
            (ProtocolType::Kitty, cell_pixels_from_winsize())
        }
        crate::herdr::HerdrGraphicsCapability::Paintable { cell } => (ProtocolType::Kitty, cell),
    };
    // `from_fontsize` is deprecated in favor of `from_query_stdio`, but the
    // stdio query needs raw mode and cannot see through Herdr's PTY. It is the
    // only constructor that accepts externally known cell metrics.
    #[allow(deprecated)]
    let mut picker = match cell {
        Some(cell) => Picker::from_fontsize(FontSize::new(cell.width, cell.height)),
        None => Picker::halfblocks(),
    };
    picker.set_protocol_type(protocol);
    Some(picker)
}

/// Cell pixel size from the controlling terminal's window size, when the
/// terminal fills in pixel dimensions. Without this, `Picker::halfblocks()`
/// guesses 10x20 and Kitty placements reserve rows the bitmap never fills.
#[cfg(unix)]
fn cell_pixels_from_winsize() -> Option<crate::herdr::CellPixels> {
    use std::os::fd::AsRawFd as _;
    let mut winsize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes into a valid, properly sized `winsize`.
    let status = unsafe {
        libc::ioctl(
            std::io::stdout().as_raw_fd(),
            libc::TIOCGWINSZ,
            &mut winsize as *mut libc::winsize,
        )
    };
    if status != 0
        || winsize.ws_xpixel == 0
        || winsize.ws_ypixel == 0
        || winsize.ws_col == 0
        || winsize.ws_row == 0
    {
        return None;
    }
    Some(crate::herdr::CellPixels {
        width: winsize.ws_xpixel / winsize.ws_col,
        height: winsize.ws_ypixel / winsize.ws_row,
    })
}

#[cfg(not(unix))]
fn cell_pixels_from_winsize() -> Option<crate::herdr::CellPixels> {
    None
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
