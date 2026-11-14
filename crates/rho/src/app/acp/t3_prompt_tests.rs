use pretty_assertions::assert_eq;
use rho_protocol::ContentBlock;

use super::split_context;

const INSTRUCTIONS: &str =
    "<t3_code_instructions>\n## T3 Code interaction mode: Default\n</t3_code_instructions>";
const RUNTIME: &str =
    "<runtime_info>In case you're asked: you are running in T3 Code.</runtime_info>";
const LINKING: &str = "<pull_request_linking>\nLink each PR you create.\n</pull_request_linking>";

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text { text: text.into() }
}

fn context(text: &str) -> ContentBlock {
    ContentBlock::Context { text: text.into() }
}

fn image() -> ContentBlock {
    ContentBlock::Image {
        mime_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    }
}

// Covers: T3's leading instructions and its trailing runtime and PR-linking
// blocks become context, and the `<user_request>` wrapping comes off, so
// only what the person typed is their message; a prompt without T3's
// blocks, a known tag beyond T3's one of each, and a known or unknown tag
// that is not whole at the prompt's start or end, stay the person's text
// Owner: ACP `--t3code` prompt mapping
#[test]
fn t3_wrapping_becomes_context_and_the_person_text_stays() {
    let trailer = format!("{RUNTIME}\n\n{LINKING}");
    let mentions = "why is <t3_code_instructions> in my title, \
                    and why does my bubble end in <runtime_info>this</runtime_info>";
    let cases = [
        (
            "a thread's first prompt, with an image",
            vec![
                text(&format!(
                    "{INSTRUCTIONS}\n\n<user_request>\nfix the bug\n</user_request>"
                )),
                image(),
                text(&trailer),
            ],
            vec![
                context(INSTRUCTIONS),
                context(RUNTIME),
                context(LINKING),
                text("fix the bug"),
                image(),
            ],
        ),
        (
            "a later prompt, in one block",
            vec![text(&format!("push it to main\n\n{trailer}"))],
            vec![context(RUNTIME), context(LINKING), text("push it to main")],
        ),
        (
            "the person's own trailing block, before T3's",
            vec![text(&format!("this came back:\n{RUNTIME}\n\n{trailer}"))],
            vec![
                context(RUNTIME),
                context(LINKING),
                text(&format!("this came back:\n{RUNTIME}")),
            ],
        ),
        (
            "a block nested in T3's",
            vec![text(&format!(
                "hello\n<runtime_info>\n{RUNTIME}\n</runtime_info>"
            ))],
            vec![
                context(&format!("<runtime_info>\n{RUNTIME}\n</runtime_info>")),
                text("hello"),
            ],
        ),
        (
            "no T3 blocks",
            vec![text("rename the widget")],
            vec![text("rename the widget")],
        ),
        (
            "tags mentioned mid-text, and an unknown one at the end",
            vec![text(mentions), text("thanks\n<custom>kept</custom>")],
            vec![text(mentions), text("thanks\n<custom>kept</custom>")],
        ),
    ];
    for (label, prompt, expected) in cases {
        assert_eq!(split_context(prompt), expected, "{label}");
    }
}
