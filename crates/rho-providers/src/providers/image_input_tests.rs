use std::borrow::Cow;

use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, ImageContent, Message, ModelIdentity, ToolResult};

use super::{gate_images, IMAGE_UNSUPPORTED_TEXT};
use crate::model::models_dev::{
    with_models_dev_cache_dir_for_tests, write_cached_model_metadata_for_tests, ImageInput,
    ModelMetadata,
};

fn image() -> ImageContent {
    ImageContent {
        data: "iVBORw0KGgo=".into(),
        mime_type: "image/png".into(),
    }
}

// Covers: a text-only catalog model never receives image blocks, while
// image-capable and uncatalogued models keep history byte-for-byte
// Owner: provider request shaping
#[test]
fn gate_strips_images_only_for_models_marked_text_only() {
    let history = vec![
        Message::user_text("look"),
        Message::User(vec![ContentBlock::Image(image())]),
        Message::ToolResult(
            ToolResult::succeeded("call-1", "image/png image (8 bytes)").with_images(vec![image()]),
        ),
    ];
    let stripped = vec![
        Message::user_text("look"),
        Message::User(vec![ContentBlock::Text(IMAGE_UNSUPPORTED_TEXT.into())]),
        Message::ToolResult(ToolResult::succeeded(
            "call-1",
            format!("image/png image (8 bytes)\n{IMAGE_UNSUPPORTED_TEXT}"),
        )),
    ];

    let cache = tempfile::tempdir().unwrap();
    with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
        for (model, image_input) in [
            ("vision", ImageInput::Supported),
            ("text-only", ImageInput::Unsupported),
        ] {
            write_cached_model_metadata_for_tests(
                "openai",
                model,
                &ModelMetadata {
                    image_input,
                    ..ModelMetadata::default()
                },
            );
        }

        for (model, expected) in [
            ("vision", &history),
            ("uncatalogued", &history),
            ("text-only", &stripped),
        ] {
            let identity = ModelIdentity::new("openai", "openai-responses", model);
            let gated = gate_images(&identity, &history);
            assert_eq!(&gated.to_vec(), expected, "{model}");
            // Pass-through must not clone history on every turn.
            if expected == &history {
                assert!(matches!(gated, Cow::Borrowed(_)), "{model}");
            }
        }
    });
}
