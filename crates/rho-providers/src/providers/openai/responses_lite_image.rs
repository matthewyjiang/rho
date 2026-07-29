use std::io::{Cursor, Error, Seek, SeekFrom, Write};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{imageops::FilterType, ImageFormat, ImageReader, Limits};

use crate::model::{ContentBlock, ImageContent, Message};

const LITE_IMAGE_LIMITS: LiteImageLimits = LiteImageLimits {
    max_dimension: 2_048,
    max_patches: 2_500,
    patch_size: 32,
    max_base64_bytes: 64 * 1024 * 1024,
    max_decoded_bytes: 128 * 1024 * 1024,
};

#[derive(Clone, Copy)]
struct LiteImageLimits {
    max_dimension: u32,
    max_patches: u64,
    patch_size: u32,
    max_base64_bytes: usize,
    max_decoded_bytes: u64,
}

/// Applies Responses Lite image limits to user-message images.
///
/// Invalid or unsafe images become text so the request does not silently omit
/// the affected content item or send data that the endpoint cannot process.
pub(super) fn prepare_responses_lite_messages(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .map(|message| match message {
            Message::User(blocks) => Message::User(
                blocks
                    .into_iter()
                    .map(|block| match block {
                        ContentBlock::Image(image) => prepare_lite_image(&image).map_or_else(
                            || {
                                ContentBlock::Text(
                                    "image content omitted because it could not be processed"
                                        .into(),
                                )
                            },
                            ContentBlock::Image,
                        ),
                        block => block,
                    })
                    .collect(),
            ),
            message => message,
        })
        .collect()
}

/// Validates and normalizes an inline image for the Responses Lite limits.
///
/// Invalid or unsafe inputs become `None` so the message converter can replace
/// them with a text item instead of sending data that Lite cannot process.
fn prepare_lite_image(image: &ImageContent) -> Option<ImageContent> {
    prepare_lite_image_with_limits(image, LITE_IMAGE_LIMITS)
}

fn prepare_lite_image_with_limits(
    image: &ImageContent,
    limits: LiteImageLimits,
) -> Option<ImageContent> {
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
    let mime_type = supported_mime_type(format)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut decode_limits = Limits::default();
    decode_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(decode_limits);
    let decoded = reader.decode().ok()?;
    let (target_width, target_height) = target_dimensions(
        decoded.width(),
        decoded.height(),
        limits.max_dimension,
        limits.max_patches,
        limits.patch_size,
    )?;
    let processed = if (target_width, target_height) == (decoded.width(), decoded.height()) {
        decoded
    } else {
        decoded.resize_exact(target_width, target_height, FilterType::Lanczos3)
    };

    let mut output = CappedCursor::new(max_binary_bytes_for_base64(limits.max_base64_bytes));
    processed.write_to(&mut output, format).ok()?;
    let data = STANDARD.encode(output.into_inner());
    Some(ImageContent {
        data,
        mime_type: mime_type.into(),
    })
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
