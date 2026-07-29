use std::io::{Cursor, Write};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{DynamicImage, ImageFormat, ImageReader};
use pretty_assertions::assert_eq;

use super::*;

fn png_image(width: u32, height: u32) -> ImageContent {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgba8(width, height)
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
    ImageContent {
        data: STANDARD.encode(bytes.into_inner()),
        mime_type: "image/png".into(),
    }
}

fn test_limits() -> LiteImageLimits {
    LiteImageLimits {
        max_dimension: 64,
        max_patches: 4,
        patch_size: 32,
        max_base64_bytes: 1024 * 1024,
        max_decoded_bytes: 1024 * 1024,
    }
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
        let prepared = prepare_lite_image_with_limits(&image, limits).unwrap();
        let bytes = STANDARD.decode(prepared.data).unwrap();
        let dimensions = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png)
            .into_dimensions()
            .unwrap();

        assert_eq!(dimensions, expected_dimensions);
        assert!(patch_count(dimensions.0, dimensions.1, 32) <= 4);
        assert_eq!(prepared.mime_type, "image/png");
    }
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
        assert_eq!(prepare_lite_image_with_limits(&image, test_limits()), None);
    }

    let tiny_base64_limit = LiteImageLimits {
        max_base64_bytes: 4,
        ..test_limits()
    };
    let tiny_decode_limit = LiteImageLimits {
        max_decoded_bytes: 1,
        ..test_limits()
    };
    assert_eq!(
        prepare_lite_image_with_limits(&valid, tiny_base64_limit),
        None
    );
    assert_eq!(
        prepare_lite_image_with_limits(&valid, tiny_decode_limit),
        None
    );
}
