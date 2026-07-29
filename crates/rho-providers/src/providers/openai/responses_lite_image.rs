use std::io::{Cursor, Error, Seek, SeekFrom, Write};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{imageops::FilterType, DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

use crate::model::{ContentBlock, ImageContent, Message, ModelError};

const LITE_IMAGE_LIMITS: LiteImageLimits = LiteImageLimits {
    max_dimension: 2_048,
    max_patches: 2_500,
    patch_size: 32,
    max_base64_bytes: 64 * 1024 * 1024,
    max_decoded_bytes: 128 * 1024 * 1024,
    max_images_per_request: 20,
    max_request_base64_bytes: 64 * 1024 * 1024,
};
const IMAGE_OMITTED_TEXT: &str = "image content omitted because it could not be processed";

#[derive(Clone, Copy)]
struct LiteImageLimits {
    max_dimension: u32,
    max_patches: u64,
    patch_size: u32,
    max_base64_bytes: usize,
    max_decoded_bytes: u64,
    max_images_per_request: usize,
    max_request_base64_bytes: usize,
}

/// Running image count and byte total for one Responses Lite request.
///
/// An image is charged at its input size before preparation and settled at its
/// output size afterwards, so a resize that grows the payload still has to fit.
struct LiteImageBudget {
    limits: LiteImageLimits,
    images: usize,
    base64_bytes: usize,
}

/// One image's outstanding claim on a [`LiteImageBudget`].
///
/// Carrying the charged size makes it impossible to settle an image against
/// the wrong input byte count.
#[must_use = "an unsettled charge leaves the request budget overstated"]
struct LiteImageCharge {
    input_base64_bytes: usize,
}

impl LiteImageBudget {
    fn new(limits: LiteImageLimits) -> Self {
        Self {
            limits,
            images: 0,
            base64_bytes: 0,
        }
    }

    /// Reserves room for one input image, or returns `None` when it does not fit.
    fn charge(&mut self, image: &ImageContent) -> Option<LiteImageCharge> {
        let base64_bytes = self.base64_bytes.checked_add(image.data.len())?;
        if self.images >= self.limits.max_images_per_request
            || base64_bytes > self.limits.max_request_base64_bytes
        {
            return None;
        }
        self.images += 1;
        self.base64_bytes = base64_bytes;
        Some(LiteImageCharge {
            input_base64_bytes: image.data.len(),
        })
    }

    /// Swaps a charge for its prepared result, or returns `None` when it no longer fits.
    fn settle(
        &mut self,
        charge: LiteImageCharge,
        prepared: Option<ImageContent>,
    ) -> Option<ImageContent> {
        self.base64_bytes = self.base64_bytes.saturating_sub(charge.input_base64_bytes);
        let prepared = prepared?;
        let base64_bytes = self.base64_bytes.checked_add(prepared.data.len())?;
        if base64_bytes > self.limits.max_request_base64_bytes {
            return None;
        }
        self.base64_bytes = base64_bytes;
        Some(prepared)
    }
}

/// Applies Responses Lite image limits without blocking a Tokio worker.
///
/// Invalid, unsafe, or over-budget images become text so the request does not
/// silently omit the affected content item or send data Lite cannot process.
///
/// Images are prepared one at a time on the blocking pool. Concurrency across
/// turns is already bounded by the agent executor's run permits, so this adds
/// no limiter of its own.
pub(super) async fn prepare_responses_lite_messages(
    messages: Vec<Message>,
    cancellation: &rho_sdk::CancellationToken,
) -> Result<Vec<Message>, ModelError> {
    if cancellation.is_cancelled() {
        return Err(ModelError::Interrupted);
    }
    let mut prepared_messages = Vec::with_capacity(messages.len());
    let mut budget = LiteImageBudget::new(LITE_IMAGE_LIMITS);

    for message in messages {
        match message {
            Message::User(blocks) => {
                let mut prepared_blocks = Vec::with_capacity(blocks.len());
                for block in blocks {
                    match block {
                        ContentBlock::Image(image) => {
                            if cancellation.is_cancelled() {
                                return Err(ModelError::Interrupted);
                            }
                            let Some(charge) = budget.charge(&image) else {
                                prepared_blocks.push(omitted_image());
                                continue;
                            };

                            let prepared = tokio::task::spawn_blocking(move || {
                                prepare_lite_image(image, LITE_IMAGE_LIMITS)
                            })
                            .await
                            .map_err(|error| {
                                ModelError::InvalidResponse(format!(
                                    "Responses Lite image preparation task failed: {error}"
                                ))
                            })?;
                            if cancellation.is_cancelled() {
                                return Err(ModelError::Interrupted);
                            }
                            let prepared = budget.settle(charge, prepared);
                            prepared_blocks
                                .push(prepared.map_or_else(omitted_image, ContentBlock::Image));
                        }
                        // Non-image blocks stay as-is. New media variants must choose a
                        // budget and validation policy here instead of passing through.
                        block @ (ContentBlock::Text(_) | ContentBlock::ToolCall(_)) => {
                            prepared_blocks.push(block);
                        }
                    }
                }
                prepared_messages.push(Message::User(prepared_blocks));
            }
            Message::System(_)
            | Message::Assistant(_)
            | Message::EnrichedAssistant(_)
            | Message::AbortedAssistant(_)
            | Message::ToolResult(_) => {
                prepared_messages.push(message);
            }
        }
    }

    Ok(prepared_messages)
}

fn omitted_image() -> ContentBlock {
    ContentBlock::Text(IMAGE_OMITTED_TEXT.into())
}

/// Validates and, only when required, resizes an inline image for Lite.
///
/// Compliant images retain their exact base64 data. A resize decodes pixels,
/// applies supported EXIF orientation, and then encodes the requested format.
fn prepare_lite_image(image: ImageContent, limits: LiteImageLimits) -> Option<ImageContent> {
    if !image
        .mime_type
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
        || image.data.is_empty()
        || image.data.len() > limits.max_base64_bytes
    {
        return None;
    }

    let bytes = STANDARD.decode(&image.data).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let format = image::guess_format(&bytes).ok()?;
    let resized_mime_type = supported_mime_type(format)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes.as_slice()), format);
    let mut decode_limits = Limits::default();
    decode_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(decode_limits);
    let mut decoder = reader.into_decoder().ok()?;
    if decoder.total_bytes() > limits.max_decoded_bytes {
        return None;
    }
    let orientation = decoder.orientation().ok()?;
    let mut decoded = DynamicImage::from_decoder(decoder).ok()?;
    decoded.apply_orientation(orientation);

    let target = target_dimensions(
        decoded.width(),
        decoded.height(),
        limits.max_dimension,
        limits.max_patches,
        limits.patch_size,
    )?;
    if target == (decoded.width(), decoded.height()) {
        return Some(image);
    }
    // DynamicImage keeps only one frame. Refuse to flatten animated GIFs into a
    // static resize; callers omit the image rather than send corrupted media.
    if format == ImageFormat::Gif && gif_is_animated(bytes.as_slice()) {
        return None;
    }

    let processed = decoded.resize_exact(target.0, target.1, FilterType::Lanczos3);
    let mut output = CappedCursor::new(max_binary_bytes_for_base64(limits.max_base64_bytes));
    processed.write_to(&mut output, format).ok()?;
    Some(ImageContent {
        data: STANDARD.encode(output.into_inner()),
        mime_type: resized_mime_type.into(),
    })
}

fn gif_is_animated(bytes: &[u8]) -> bool {
    use image::{codecs::gif::GifDecoder, AnimationDecoder};

    let Ok(decoder) = GifDecoder::new(Cursor::new(bytes)) else {
        return false;
    };
    let mut frames = decoder.into_frames();
    match frames.next() {
        Some(Ok(_)) => matches!(frames.next(), Some(Ok(_))),
        _ => false,
    }
}

fn max_binary_bytes_for_base64(max_base64_bytes: usize) -> usize {
    max_base64_bytes / 4 * 3
}

struct CappedCursor {
    inner: Cursor<Vec<u8>>,
    max_len: usize,
}

impl CappedCursor {
    fn new(max_len: usize) -> Self {
        Self {
            inner: Cursor::new(Vec::new()),
            max_len,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.inner.into_inner()
    }

    fn limit_error() -> Error {
        Error::other("encoded image exceeds size limit")
    }
}

impl Write for CappedCursor {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let position = usize::try_from(self.inner.position()).map_err(|_| Self::limit_error())?;
        let end = position
            .checked_add(buffer.len())
            .ok_or_else(Self::limit_error)?;
        if end > self.max_len {
            return Err(Self::limit_error());
        }
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for CappedCursor {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        let previous = self.inner.position();
        let next = self.inner.seek(position)?;
        if next > self.max_len as u64 {
            self.inner.set_position(previous);
            return Err(Self::limit_error());
        }
        Ok(next)
    }
}

fn supported_mime_type(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

fn target_dimensions(
    width: u32,
    height: u32,
    max_dimension: u32,
    max_patches: u64,
    patch_size: u32,
) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || max_dimension == 0 || max_patches == 0 || patch_size == 0 {
        return None;
    }

    let dimension_scale = f64::from(max_dimension) / f64::from(width.max(height));
    let mut scale = dimension_scale.min(1.0);
    let mut target = scaled_dimensions(width, height, scale);
    let patches = patch_count(target.0, target.1, patch_size);
    if patches > max_patches {
        scale *= (max_patches as f64 / patches as f64).sqrt();
        target = scaled_dimensions(width, height, scale);
    }

    while patch_count(target.0, target.1, patch_size) > max_patches {
        if target.0 >= target.1 && target.0 > 1 {
            target.0 -= 1;
            target.1 = ((u64::from(height) * u64::from(target.0)) / u64::from(width))
                .max(1)
                .try_into()
                .ok()?;
        } else if target.1 > 1 {
            target.1 -= 1;
            target.0 = ((u64::from(width) * u64::from(target.1)) / u64::from(height))
                .max(1)
                .try_into()
                .ok()?;
        } else {
            return None;
        }
    }
    Some(target)
}

fn scaled_dimensions(width: u32, height: u32, scale: f64) -> (u32, u32) {
    (
        (f64::from(width) * scale).floor().max(1.0) as u32,
        (f64::from(height) * scale).floor().max(1.0) as u32,
    )
}

fn patch_count(width: u32, height: u32, patch_size: u32) -> u64 {
    let columns = width.div_ceil(patch_size);
    let rows = height.div_ceil(patch_size);
    u64::from(columns) * u64::from(rows)
}

#[cfg(test)]
#[path = "responses_lite_image_tests.rs"]
mod tests;
