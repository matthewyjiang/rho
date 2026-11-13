use pretty_assertions::assert_eq;
use rho_protocol::ContentBlock;
use rho_sdk::model::{self, ImageContent};

use super::{user_input, InvalidPrompt};

/// A 1x1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
/// `<svg xmlns="http://www.w3.org/2000/svg"/>`.
const SVG: &str = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=";

fn resource_link(uri: &str, description: Option<&str>) -> ContentBlock {
    ContentBlock::ResourceLink {
        uri: uri.into(),
        name: "notes".into(),
        description: description.map(str::to_owned),
    }
}

// Covers: each prompt block reaches the model as typed text or image, or as
// fenced context: a resource as its text, a resource link as its
// description, else its name, and a block rho cannot read as a marker.
// Owner: prompt content conversion (daemon and ACP prompts).
#[test]
fn prompt_blocks_reach_the_model_as_text_images_or_fenced_context() {
    let prompt = [
        ContentBlock::Text {
            text: "hello".into(),
        },
        ContentBlock::Image {
            mime_type: "image/png".into(),
            data: PNG.into(),
        },
        ContentBlock::Resource {
            uri: "file:///workspace/main.rs".into(),
            text: "fn main() {}".into(),
        },
        resource_link("file:///workspace/notes.md", None),
        resource_link("file:///workspace/plan.md", Some("the plan")),
        ContentBlock::Unknown,
    ];

    let input = user_input(&prompt).unwrap();

    assert_eq!(
        input.blocks(),
        [
            model::ContentBlock::Text("hello".into()),
            model::ContentBlock::Image(ImageContent {
                data: PNG.into(),
                mime_type: "image/png".into(),
            }),
            model::ContentBlock::Text(
                "```resource file:///workspace/main.rs\nfn main() {}\n```".into()
            ),
            model::ContentBlock::Text("```resource file:///workspace/notes.md\nnotes\n```".into()),
            model::ContentBlock::Text(
                "```resource file:///workspace/plan.md\nthe plan\n```".into()
            ),
            model::ContentBlock::Text("```context unsupported\n\n```".into()),
        ]
    );
}

// Covers: an image whose data is not base64, not a PNG, JPEG, GIF, or WebP
// (an SVG), or not the type it declares is refused, naming the block, before
// a turn starts; one that checks out reaches the model.
// Owner: prompt content conversion (daemon and ACP prompts).
#[test]
fn prompt_images_must_be_the_image_they_declare() {
    let cases = [
        (
            "not base64",
            "image/png",
            "not base64!",
            Err(InvalidPrompt::ImageNotBase64 { index: 1 }),
        ),
        (
            "an svg",
            "image/svg+xml",
            SVG,
            Err(InvalidPrompt::ImageUnrecognized { index: 1 }),
        ),
        (
            "a png declared jpeg",
            "image/jpeg",
            PNG,
            Err(InvalidPrompt::ImageMismatch {
                index: 1,
                declared: "image/jpeg".into(),
                actual: "image/png",
            }),
        ),
        (
            "a png",
            "image/png",
            PNG,
            Ok(vec![
                model::ContentBlock::Text("look".into()),
                model::ContentBlock::Image(ImageContent {
                    data: PNG.into(),
                    mime_type: "image/png".into(),
                }),
            ]),
        ),
    ];
    for (name, mime_type, data, expected) in cases {
        let prompt = [
            ContentBlock::Text {
                text: "look".into(),
            },
            ContentBlock::Image {
                mime_type: mime_type.into(),
                data: data.into(),
            },
        ];
        assert_eq!(
            user_input(&prompt).map(|input| input.blocks().to_vec()),
            expected,
            "{name}"
        );
    }
}

// Covers: a resource body or URI that holds its own fence cannot close the
// generated fence early.
// Owner: prompt content conversion.
#[test]
fn fenced_context_outgrows_backticks_in_the_body_and_header() {
    for (block, expected) in [
        (
            ContentBlock::Resource {
                uri: "file:///workspace/readme.md".into(),
                text: "```rust\nfn main() {}\n```".into(),
            },
            "````resource file:///workspace/readme.md\n```rust\nfn main() {}\n```\n````",
        ),
        (
            resource_link("file:///workspace/notes.md\n```\ninjected", None),
            "````resource file:///workspace/notes.md\n```\ninjected\nnotes\n````",
        ),
    ] {
        let input = user_input(&[block]).unwrap();
        assert_eq!(input.blocks(), [model::ContentBlock::Text(expected.into())]);
    }
}
