use std::io::Cursor;

use image::{DynamicImage, GenericImageView, ImageFormat};
use pretty_assertions::assert_eq;

use super::decode_preview_image;

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([20, 40, 60, 255]),
    ));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}

// Covers: full-resolution previews must fit the display box without enlarging small sources.
// Owner: shared tool/display image decode policy
#[test]
fn preview_resizes_only_sources_larger_than_the_display_box() {
    for (width, height, expected) in [
        (1_024, 1_024, (768, 768)),
        (1_800, 1_200, (1_024, 683)),
        (320, 240, (320, 240)),
    ] {
        let image = decode_preview_image(&png(width, height)).unwrap();
        assert_eq!(image.dimensions(), expected);
    }
}

// Covers: accepting full-resolution previews must retain decoder safety bounds.
// Owner: shared tool/display image decode policy
#[test]
fn preview_rejects_sources_beyond_decode_dimensions() {
    for (width, height) in [(4_097, 1), (1, 4_097)] {
        assert!(matches!(
            decode_preview_image(&png(width, height)),
            Err(image::ImageError::Limits(_))
        ));
    }
}
