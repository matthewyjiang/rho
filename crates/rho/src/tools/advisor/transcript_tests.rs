use pretty_assertions::assert_eq;
use rho_sdk::model::{
    AbortedAssistant, AssistantMessage, ContentBlock, ImageContent, Message, PartialToolCall,
    ToolCall, ToolResult,
};
use serde_json::json;

use super::{render_transcript, TranscriptBudget, DEFAULT_TRANSCRIPT_BUDGET};

fn generous() -> TranscriptBudget {
    TranscriptBudget {
        body_bytes: 1_000_000,
        ..DEFAULT_TRANSCRIPT_BUDGET
    }
}

#[test]
fn renders_requests_replies_tool_calls_and_results_in_order() {
    let messages = vec![
        Message::user_text("fix the failing test"),
        Message::assistant(AssistantMessage::from_content(vec![
            ContentBlock::Text("Looking at the suite.".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "bash".into(),
                arguments: json!({ "command": "cargo test" }),
            }),
        ])),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: false,
            content: "1 failed".into(),
        }),
        Message::Assistant(vec![ContentBlock::Image(ImageContent {
            data: "AAAA".into(),
            mime_type: "image/png".into(),
        })]),
    ];

    let rendered = render_transcript(Some("You are a coding agent."), &messages, generous());

    assert_eq!(
        rendered,
        "# Executor system prompt\n\
         \n\
         You are a coding agent.\n\
         \n\
         # Session transcript\n\
         \n\
         ## user\n\
         \n\
         fix the failing test\n\
         \n\
         ## assistant\n\
         \n\
         Looking at the suite.\n\
         tool call: bash (id call-1)\n\
         arguments: {\"command\":\"cargo test\"}\n\
         \n\
         ## tool result call-1 (error)\n\
         \n\
         1 failed\n\
         \n\
         ## assistant\n\
         \n\
         [image: image/png]\n"
    );
}

// Covers: zero-arg tool payloads must not render as `arguments: {}`, which
// made advisor runs invent "empty args failed" guidance. Only the empty object
// is omitted; null/[]/"" stay visible as diagnostic evidence.
// Owner: advisor transcript renderer
#[test]
fn omits_empty_object_tool_arguments_but_renders_other_emptyish_values() {
    let messages = vec![
        Message::assistant(AssistantMessage::from_content(vec![
            ContentBlock::Text("Checking with the advisor.".into()),
            ContentBlock::ToolCall(ToolCall {
                id: "call-a".into(),
                name: "advisor".into(),
                arguments: json!({}),
            }),
            ContentBlock::ToolCall(ToolCall {
                id: "call-b".into(),
                name: "rho".into(),
                arguments: json!({ "action": "info" }),
            }),
            ContentBlock::ToolCall(ToolCall {
                id: "call-null".into(),
                name: "broken".into(),
                arguments: json!(null),
            }),
            ContentBlock::ToolCall(ToolCall {
                id: "call-array".into(),
                name: "broken".into(),
                arguments: json!([]),
            }),
            ContentBlock::ToolCall(ToolCall {
                id: "call-string".into(),
                name: "broken".into(),
                arguments: json!(""),
            }),
        ])),
        Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: vec![ContentBlock::Text("Interrupted mid-call.".into())],
            tool_calls: vec![
                PartialToolCall {
                    id: Some("call-c".into()),
                    name: Some("advisor".into()),
                    arguments: "{}".into(),
                },
                PartialToolCall {
                    id: Some("call-empty".into()),
                    name: Some("advisor".into()),
                    arguments: "".into(),
                },
                PartialToolCall {
                    id: Some("call-d".into()),
                    name: Some("grep".into()),
                    arguments: "{\"pattern\":".into(),
                },
            ],
            ..AbortedAssistant::default()
        })),
    ];

    let rendered = render_transcript(None, &messages, generous());

    assert_eq!(
        rendered,
        "# Session transcript\n\
         \n\
         ## assistant\n\
         \n\
         Checking with the advisor.\n\
         tool call: advisor (id call-a)\n\
         tool call: rho (id call-b)\n\
         arguments: {\"action\":\"info\"}\n\
         tool call: broken (id call-null)\n\
         arguments: null\n\
         tool call: broken (id call-array)\n\
         arguments: []\n\
         tool call: broken (id call-string)\n\
         arguments: \"\"\n\
         \n\
         ## assistant (interrupted)\n\
         \n\
         Interrupted mid-call.\n\
         tool call (incomplete): advisor\n\
         tool call (incomplete): advisor\n\
         arguments: \n\
         tool call (incomplete): grep\n\
         arguments: {\"pattern\":\n"
    );
}

#[test]
fn renders_system_messages_and_interrupted_replies() {
    let messages = vec![
        Message::System("session rules".into()),
        Message::AbortedAssistant(Box::new(AbortedAssistant {
            content: vec![ContentBlock::Text("Partway through.".into())],
            tool_calls: vec![PartialToolCall {
                id: Some("call-9".into()),
                name: Some("grep".into()),
                arguments: "{\"pattern\":".into(),
            }],
            ..AbortedAssistant::default()
        })),
    ];

    let rendered = render_transcript(None, &messages, generous());

    assert_eq!(
        rendered,
        "# Session transcript\n\
         \n\
         ## system\n\
         \n\
         session rules\n\
         \n\
         ## assistant (interrupted)\n\
         \n\
         Partway through.\n\
         tool call (incomplete): grep\n\
         arguments: {\"pattern\":\n"
    );
}

#[test]
fn reports_an_empty_session() {
    let rendered = render_transcript(None, &[], generous());

    assert_eq!(
        rendered,
        "# Session transcript\n\nThe session has no messages yet.\n"
    );
}

#[test]
fn clips_oversized_items_to_their_own_budgets() {
    let budget = TranscriptBudget {
        body_bytes: 1_000_000,
        system_prompt_bytes: 10,
        tool_call_bytes: 12,
        tool_result_bytes: 8,
    };
    let messages = vec![
        Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            arguments: json!({ "command": "echo a very long command line" }),
        })]),
        Message::ToolResult(ToolResult {
            id: "call-1".into(),
            ok: true,
            content: "a very long tool result".into(),
        }),
    ];

    let rendered = render_transcript(
        Some("a system prompt far past the budget"),
        &messages,
        budget,
    );

    assert_eq!(
        rendered,
        "# Executor system prompt\n\
         \n\
         a system p\n\
         [... 25 bytes clipped ...]\n\
         \n\
         # Session transcript\n\
         \n\
         ## assistant\n\
         \n\
         tool call: bash (id call-1)\n\
         arguments: {\"command\":\"\n\
         [... 31 bytes clipped ...]\n\
         \n\
         ## tool result call-1 (ok)\n\
         \n\
         a very l\n\
         [... 15 bytes clipped ...]\n"
    );
}

#[test]
fn elides_the_middle_and_keeps_both_ends_of_a_long_session() {
    let budget = TranscriptBudget {
        body_bytes: 2_000,
        ..DEFAULT_TRANSCRIPT_BUDGET
    };
    let mut messages = vec![Message::user_text("the original request")];
    for index in 0..200 {
        messages.push(Message::assistant_text(format!("step {index}")));
    }
    messages.push(Message::user_text("the latest request"));

    let rendered = render_transcript(None, &messages, budget);

    assert!(rendered.contains("the original request"), "{rendered}");
    assert!(rendered.contains("the latest request"), "{rendered}");
    assert!(rendered.contains("step 0"), "{rendered}");
    assert!(!rendered.contains("step 100"), "{rendered}");
    assert!(
        rendered.contains("bytes of the middle of the session elided"),
        "{rendered}"
    );
    let body = rendered
        .strip_prefix("# Session transcript\n")
        .expect("transcript header");
    assert!(body.len() <= budget.body_bytes, "body was {}", body.len());
}

#[test]
fn clips_on_character_boundaries() {
    let budget = TranscriptBudget {
        body_bytes: 1_000_000,
        system_prompt_bytes: 5,
        ..DEFAULT_TRANSCRIPT_BUDGET
    };

    let rendered = render_transcript(Some("héllo wörld"), &[], budget);

    assert_eq!(
        rendered,
        "# Executor system prompt\n\
         \n\
         héll\n\
         [... 8 bytes clipped ...]\n\
         \n\
         # Session transcript\n\
         \n\
         The session has no messages yet.\n"
    );
}
