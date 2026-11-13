use pretty_assertions::assert_eq;
use rho_protocol::ContentBlock;
use rho_providers::model::ImageContent;

use super::prompt_content;
use crate::tui::{ChatMedia, TurnPrompt};

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text { text: text.into() }
}

fn png() -> ImageContent {
    ImageContent {
        data: "iVBORw0KGgo=".into(),
        mime_type: "image/png".into(),
    }
}

// Covers: a daemon prompt loses neither the skill the TUI loads first (a
// typed `/skill:<name>`, or a generated `/init` prompt) nor the composer's
// images, and sends nothing for an empty text.
// Owner: tui daemon prompt content
#[test]
fn composer_turns_map_to_daemon_prompt_content() {
    let cases = [
        (
            "plain text goes as typed",
            TurnPrompt::standard("explain $review".into(), "explain $review".into()),
            Vec::new(),
            vec![text("explain $review")],
        ),
        (
            "a typed skill command becomes the daemon's",
            TurnPrompt::command(
                "/skill:review the diff".into(),
                "/skill:review the diff".into(),
            )
            .with_skill("review".into()),
            Vec::new(),
            vec![text("/$review the diff")],
        ),
        (
            "a generated prompt follows the skill it loads",
            TurnPrompt::command("Write AGENTS.md".into(), "/init".into()).with_skill("init".into()),
            Vec::new(),
            vec![text("/$init Write AGENTS.md")],
        ),
        (
            "images follow the text",
            TurnPrompt::standard("what is this".into(), "what is this".into()),
            vec![ChatMedia::Image(png())],
            vec![
                text("what is this"),
                ContentBlock::Image {
                    mime_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                },
            ],
        ),
        (
            "an image alone sends no empty text",
            TurnPrompt::standard(String::new(), String::new()),
            vec![ChatMedia::Image(png())],
            vec![ContentBlock::Image {
                mime_type: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
            }],
        ),
    ];

    for (name, turn, media, expected) in cases {
        assert_eq!(prompt_content(&turn, &media), expected, "{name}");
    }
}
