use std::io::Cursor;

use image::{DynamicImage, ImageReader, Limits};

const MAX_DECODE_DIMENSION: u32 = 4_096;
const MAX_DECODE_ALLOCATION: u64 = 80 * 1024 * 1024;
const THUMBNAIL_WIDTH: u32 = 1_024;
const THUMBNAIL_HEIGHT: u32 = 768;

/// Decode an image for tool output or display without retaining a full-size source.
///
/// Uses the existing `read_file` preview budgets: 4,096 pixels per dimension and
/// 80 MiB of decoder allocation. Larger images are rejected before resizing.
/// The result fits within 1,024 × 768 pixels; smaller images are not enlarged.
/// CPU-bound decoding should run on a blocking worker when called from async code.
pub fn decode_preview_image(bytes: &[u8]) -> image::ImageResult<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
    reader.limits(limits);
    let image = reader.decode()?;
    // `thumbnail` also upscales, so keep sources already inside the preview box.
    if image.width() > THUMBNAIL_WIDTH || image.height() > THUMBNAIL_HEIGHT {
        Ok(image.thumbnail(THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT))
    } else {
        Ok(image)
    }
}

#[cfg(test)]
#[path = "image_preview_tests.rs"]
mod tests;
