use std::io::{Cursor, Write};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{
    codecs::gif::{GifEncoder, Repeat},
    DynamicImage, Frame, ImageFormat, ImageReader, RgbaImage,
};
use pretty_assertions::assert_eq;

use super::*;

fn encoded_image(width: u32, height: u32, format: ImageFormat) -> ImageContent {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgba8(width, height)
        .write_to(&mut bytes, format)
        .unwrap();
    ImageContent {
        data: STANDARD.encode(bytes.into_inner()),
        mime_type: match format {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            _ => panic!("test image format has no MIME mapping"),
        }
        .into(),
    }
}

fn png_image(width: u32, height: u32) -> ImageContent {
    encoded_image(width, height, ImageFormat::Png)
}

fn jpeg_with_rotate_90_orientation(width: u32, height: u32) -> ImageContent {
    let image = encoded_image(width, height, ImageFormat::Jpeg);
    let bytes = STANDARD.decode(image.data).unwrap();
    assert_eq!(&bytes[..2], &[0xff, 0xd8]);

    // APP1 Exif segment with a little-endian orientation tag set to 6.
    let exif = [
        0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0x00, 0x00, b'I', b'I', 0x2a, 0x00, 0x08,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x06, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut oriented = Vec::with_capacity(bytes.len() + exif.len());
    oriented.extend_from_slice(&bytes[..2]);
    oriented.extend_from_slice(&exif);
    oriented.extend_from_slice(&bytes[2..]);
    ImageContent {
        data: STANDARD.encode(oriented),
        mime_type: "image/jpeg".into(),
    }
}

fn animated_gif(width: u32, height: u32) -> ImageContent {
    let mut bytes = Vec::new();
    let mut encoder = GifEncoder::new(&mut bytes);
    encoder.set_repeat(Repeat::Infinite).unwrap();
    encoder
        .encode_frames([
            Frame::new(RgbaImage::new(width, height)),
            Frame::new(RgbaImage::new(width, height)),
        ])
        .unwrap();
    drop(encoder);
    ImageContent {
        data: STANDARD.encode(bytes),
        mime_type: "image/gif".into(),
    }
}

fn test_limits() -> LiteImageLimits {
    LiteImageLimits {
        max_dimension: 64,
        max_patches: 4,
        patch_size: 32,
        max_base64_bytes: 1024 * 1024,
        max_decoded_bytes: 1024 * 1024,
        max_images_per_request: 4,
        max_request_base64_bytes: 1024 * 1024,
    }
}

// Covers: compliant supported images must not lose bytes, metadata, or animation data.
// Owner: OpenAI Responses Lite image policy.
#[test]
fn compliant_lite_image_preserves_original_encoding() {
    let cases = [jpeg_with_rotate_90_orientation(16, 8), animated_gif(2, 1)];

    for image in cases {
        let prepared = prepare_lite_image(image.clone(), test_limits()).unwrap();
        assert_eq!(prepared, image);
    }
}

// Covers: oversized animated GIFs must not be flattened into a static resize.
// Owner: OpenAI Responses Lite image policy.
#[test]
fn oversized_animated_gif_is_rejected_instead_of_flattened() {
    let image = animated_gif(128, 32);

    assert_eq!(prepare_lite_image(image, test_limits()), None);
}

// Covers: Lite images must meet both dimension and patch limits before upload.
// Owner: OpenAI Responses Lite image policy.
#[test]
fn lite_image_resizing_enforces_dimension_and_patch_limits() {
    let dimension_limits = LiteImageLimits {
        max_patches: 100,
        ..test_limits()
    };
    let patch_limits = LiteImageLimits {
        max_dimension: 96,
        ..test_limits()
    };
    let cases = [
        (png_image(128, 32), dimension_limits, (64, 16)),
        (png_image(96, 96), patch_limits, (64, 64)),
    ];

    for (image, limits, expected_dimensions) in cases {
        let prepared = prepare_lite_image(image, limits).unwrap();
        let bytes = STANDARD.decode(prepared.data).unwrap();
        let dimensions = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png)
            .into_dimensions()
            .unwrap();

        assert_eq!(dimensions, expected_dimensions);
        assert!(patch_count(dimensions.0, dimensions.1, 32) <= 4);
        assert_eq!(prepared.mime_type, "image/png");
    }
}

// Covers: resize dimensions and pixels must follow a camera image's EXIF orientation.
// Owner: OpenAI Responses Lite image policy.
#[test]
fn lite_image_resize_applies_exif_orientation_first() {
    let image = jpeg_with_rotate_90_orientation(80, 40);

    let prepared = prepare_lite_image(image, test_limits()).unwrap();
    let bytes = STANDARD.decode(prepared.data).unwrap();
    let dimensions = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg)
        .into_dimensions()
        .unwrap();

    assert_eq!(dimensions, (32, 64));
}

// Covers: encoded Lite images must not allocate output past the binary upload limit.
// Owner: OpenAI Responses Lite image policy.
#[test]
fn lite_image_output_writer_enforces_base64_derived_limit() {
    let mut output = CappedCursor::new(max_binary_bytes_for_base64(8));
    output.write_all(b"123456").unwrap();

    assert_eq!(output.into_inner(), b"123456");

    let mut output = CappedCursor::new(max_binary_bytes_for_base64(8));
    assert!(output.write_all(b"1234567").is_err());
    assert_eq!(output.into_inner(), b"");
}

// Covers: one request cannot bypass image limits by sending many valid images.
// Owner: OpenAI Responses Lite image policy.
#[test]
fn lite_image_budget_enforces_aggregate_count_and_bytes() {
    let image = png_image(1, 1);
    let count_limits = LiteImageLimits {
        max_images_per_request: 2,
        max_request_base64_bytes: image.data.len() * 2,
        ..test_limits()
    };
    let mut budget = LiteImageBudget::new(count_limits);

    assert!(budget.charge(&image).is_some());
    assert!(budget.charge(&image).is_some());
    assert!(budget.charge(&image).is_none());

    let byte_limits = LiteImageLimits {
        max_images_per_request: 3,
        max_request_base64_bytes: image.data.len() * 2 - 1,
        ..count_limits
    };
    let mut budget = LiteImageBudget::new(byte_limits);
    assert!(budget.charge(&image).is_some());
    assert!(budget.charge(&image).is_none());

    // A resize that grows the payload past the request budget is rejected on settle.
    let output_limits = LiteImageLimits {
        max_request_base64_bytes: image.data.len(),
        ..test_limits()
    };
    let mut budget = LiteImageBudget::new(output_limits);
    let charge = budget.charge(&image).unwrap();
    let grown = ImageContent {
        data: format!("{}=", image.data),
        ..image.clone()
    };
    assert_eq!(budget.settle(charge, Some(grown)), None);

    let charge = budget.charge(&image).unwrap();
    assert_eq!(budget.settle(charge, Some(image.clone())), Some(image));
}

// Covers: malformed or resource-heavy image input must not reach Responses Lite.
// Owner: OpenAI Responses Lite image policy.
#[test]
fn lite_image_policy_rejects_invalid_and_over_limit_inputs() {
    let valid = png_image(1, 1);
    let cases = [
        ImageContent {
            data: String::new(),
            mime_type: "image/png".into(),
        },
        ImageContent {
            data: "not base64".into(),
            mime_type: "image/png".into(),
        },
        ImageContent {
            data: valid.data.clone(),
            mime_type: "text/plain".into(),
        },
        ImageContent {
            data: STANDARD.encode(b"not an image"),
            mime_type: "image/png".into(),
        },
    ];

    for image in cases {
        assert_eq!(prepare_lite_image(image, test_limits()), None);
    }

    let tiny_base64_limit = LiteImageLimits {
        max_base64_bytes: 4,
        ..test_limits()
    };
    let tiny_decode_limit = LiteImageLimits {
        max_decoded_bytes: 1,
        ..test_limits()
    };
    assert_eq!(prepare_lite_image(valid.clone(), tiny_base64_limit), None);
    assert_eq!(prepare_lite_image(valid, tiny_decode_limit), None);
}

// Covers: cancellation before an image job must stop body preparation.
// Owner: OpenAI Responses Lite image preparation.
#[tokio::test]
async fn lite_image_preparation_observes_cancellation() {
    let cancellation = rho_sdk::CancellationToken::new();
    cancellation.cancel();
    let messages = vec![Message::User(vec![ContentBlock::Image(png_image(1, 1))])];

    let error = prepare_responses_lite_messages(messages, &cancellation)
        .await
        .unwrap_err();

    assert!(matches!(error, ModelError::Interrupted));
}
