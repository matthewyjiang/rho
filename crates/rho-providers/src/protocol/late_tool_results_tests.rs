use std::borrow::Cow;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::{normalize_late_tool_results, LATE_PLACEHOLDER};
use crate::model::{ContentBlock, ImageContent, Message, ModelIdentity, ToolCall, ToolResult};

fn call(id: &str, name: &str) -> Message {
    Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: json!({}),
    })])
}

fn calls(entries: &[(&str, &str)]) -> Message {
    Message::Assistant(
        entries
            .iter()
            .map(|(id, name)| {
                ContentBlock::ToolCall(ToolCall {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: json!({}),
                })
            })
            .collect(),
    )
}

fn result(id: &str, content: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: content.into(),
    })
}

fn placeholder(id: &str) -> Message {
    Message::ToolResult(ToolResult {
        id: id.into(),
        ok: true,
        content: LATE_PLACEHOLDER.into(),
    })
}

fn late_user(id: &str, name: &str, content: &str) -> Message {
    Message::user_text(format!("Result for tool call {id} ({name}): {content}"))
}

// Covers: adjacency providers must see a tool result beside each call, with
// delayed results rewritten as user text.
// Owner: protocol late-tool-result pairing
#[test]
fn normalize_late_tool_results_pairs_or_borrows() {
    struct Case {
        name: &'static str,
        input: Vec<Message>,
        expected: Option<Vec<Message>>,
    }
    let cases = [
        Case {
            name: "already paired",
            input: vec![
                Message::user_text("go"),
                call("a", "bash"),
                result("a", "ok"),
            ],
            expected: None,
        },
        Case {
            name: "late after a user steer",
            input: vec![
                call("a", "one_agent"),
                Message::user_text("steer"),
                result("a", "done"),
            ],
            expected: Some(vec![
                call("a", "one_agent"),
                placeholder("a"),
                Message::user_text("steer"),
                late_user("a", "one_agent", "done"),
            ]),
        },
        Case {
            name: "two calls one late",
            input: vec![
                calls(&[("a", "bash"), ("b", "one_agent")]),
                result("a", "files"),
                Message::user_text("steer"),
                result("b", "agent done"),
            ],
            expected: Some(vec![
                calls(&[("a", "bash"), ("b", "one_agent")]),
                result("a", "files"),
                placeholder("b"),
                Message::user_text("steer"),
                late_user("b", "one_agent", "agent done"),
            ]),
        },
        Case {
            name: "never delivered",
            input: vec![call("a", "one_agent"), Message::user_text("next")],
            expected: Some(vec![
                call("a", "one_agent"),
                placeholder("a"),
                Message::user_text("next"),
            ]),
        },
    ];

    for case in cases {
        let normalized = normalize_late_tool_results(&case.input);
        match case.expected {
            None => {
                assert!(
                    matches!(normalized, Cow::Borrowed(_)),
                    "{} should borrow",
                    case.name
                );
                assert_eq!(normalized.as_ref(), case.input.as_slice(), "{}", case.name);
            }
            Some(expected) => {
                assert!(
                    matches!(normalized, Cow::Owned(_)),
                    "{} should own a rewrite",
                    case.name
                );
                assert_eq!(normalized.as_ref(), expected.as_slice(), "{}", case.name);
            }
        }
    }
}

// Covers: committing images before other async results must preserve provider pairing
// and deliver each actual result and image once, both before and after late completion.
// Owner: provider wire conversion, not SDK scheduling.
#[test]
fn image_supplements_allow_late_results_in_every_provider_protocol() {
    use crate::protocol::{anthropic_messages, gemini_generate_content, openai_shared::convert};

    let supplement = Message::tool_image_supplement(
        "capture",
        "a",
        vec![ImageContent {
            data: "aW1hZ2U=".into(),
            mime_type: "image/png".into(),
        }],
    )
    .unwrap();
    let Message::User(blocks) = &supplement else {
        unreachable!()
    };
    let ContentBlock::Text(label) = &blocks[0] else {
        unreachable!()
    };
    let late_supplement = Message::tool_image_supplement(
        "background",
        "b",
        vec![ImageContent {
            data: "bGF0ZQ==".into(),
            mime_type: "image/png".into(),
        }],
    )
    .unwrap();
    let Message::User(blocks) = &late_supplement else {
        unreachable!()
    };
    let ContentBlock::Text(late_label) = &blocks[0] else {
        unreachable!()
    };
    for delivered in [false, true] {
        let mut history = vec![
            calls(&[("a", "capture"), ("b", "background")]),
            result("a", "captured"),
            supplement.clone(),
        ];
        let mut paired = vec![
            history[0].clone(),
            result("a", "captured"),
            placeholder("b"),
            supplement.clone(),
        ];
        if delivered {
            history.push(result("b", "finished"));
            history.push(late_supplement.clone());
            paired.push(late_user("b", "background", "finished"));
            paired.push(late_supplement.clone());
        }
        assert_eq!(normalize_late_tool_results(&history).as_ref(), paired);

        let target = ModelIdentity::new("test", "test", "test");
        let (_, anthropic) = anthropic_messages::split_system_and_messages(
            &history,
            &target,
            anthropic_messages::ProviderContextReplay::Disabled,
        )
        .unwrap();
        let anthropic = serde_json::to_value(anthropic).unwrap();
        let mut expected = vec![
            json!({"type":"tool_result", "tool_use_id":"a", "content":"captured"}),
            json!({"type":"tool_result", "tool_use_id":"b", "content":LATE_PLACEHOLDER}),
            json!({"type":"text", "text":label}),
            json!({"type":"image", "source":{"type":"base64", "media_type":"image/png", "data":"aW1hZ2U="}}),
        ];
        if delivered {
            expected.push(
                json!({"type":"text", "text":"Result for tool call b (background): finished"}),
            );
            expected.push(json!({"type":"text", "text":late_label}));
            expected.push(json!({"type":"image", "source":{"type":"base64", "media_type":"image/png", "data":"bGF0ZQ=="}}));
        }
        assert_eq!(anthropic.as_array().unwrap().len(), 2);
        assert_eq!(anthropic[1], json!({"role":"user", "content":expected}));

        let gemini = gemini_generate_content::build_request(&history, &[], &target, None).unwrap();
        let gemini = serde_json::to_value(gemini).unwrap();
        let mut expected = vec![
            json!({"functionResponse":{"id":"a", "name":"capture", "response":{"output":"captured", "ok":true}}}),
            json!({"functionResponse":{"id":"b", "name":"background", "response":{"output":LATE_PLACEHOLDER, "ok":true}}}),
            json!({"text":label}),
            json!({"inlineData":{"mimeType":"image/png", "data":"aW1hZ2U="}}),
        ];
        if delivered {
            expected.push(json!({"text":"Result for tool call b (background): finished"}));
            expected.push(json!({"text":late_label}));
            expected.push(json!({"inlineData":{"mimeType":"image/png", "data":"bGF0ZQ=="}}));
        }
        assert_eq!(gemini["contents"].as_array().unwrap().len(), 2);
        assert_eq!(
            gemini["contents"][1],
            json!({"role":"user", "parts":expected})
        );

        let chat = convert::to_openai_messages_for_target(&history, None).unwrap();
        let chat = serde_json::to_value(chat).unwrap();
        let mut expected = vec![
            json!({"role":"tool", "tool_call_id":"a", "content":"captured"}),
            json!({"role":"tool", "tool_call_id":"b", "content":LATE_PLACEHOLDER}),
            json!({"role":"user", "content":[{"type":"text", "text":label}, {"type":"image_url", "image_url":{"url":"data:image/png;base64,aW1hZ2U="}}]}),
        ];
        if delivered {
            expected.push(
                json!({"role":"user", "content":[{"type":"text", "text":"Result for tool call b (background): finished"}]}),
            );
            expected.push(json!({"role":"user", "content":[{"type":"text", "text":late_label}, {"type":"image_url", "image_url":{"url":"data:image/png;base64,bGF0ZQ=="}}]}));
        }
        assert_eq!(&chat.as_array().unwrap()[1..], expected);

        // Responses supports late function_call_output directly, with no placeholder.
        let responses = convert::codex_input_items(&history, &mut Vec::new()).unwrap();
        let mut expected = vec![
            json!({"type":"function_call_output", "call_id":"a", "output":"captured"}),
            json!({"role":"user", "content":[{"type":"input_text", "text":label}, {"type":"input_image", "image_url":"data:image/png;base64,aW1hZ2U="}]}),
        ];
        if delivered {
            expected
                .push(json!({"type":"function_call_output", "call_id":"b", "output":"finished"}));
            expected.push(json!({"role":"user", "content":[{"type":"input_text", "text":late_label}, {"type":"input_image", "image_url":"data:image/png;base64,bGF0ZQ=="}]}));
        }
        assert_eq!(&responses[2..], expected);
    }
}
