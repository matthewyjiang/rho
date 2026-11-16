use std::sync::Arc;

use serde_json::json;

use crate::model::{
    AbortedAssistant, ContentBlock, Message, ModelIdentity, ProviderContextBlock, ReplayScope,
};

const SUMMARY: &str = "weighing whether to keep a uniform buffer";

fn aborted(content: Vec<ContentBlock>, provider_context: Vec<ProviderContextBlock>) -> Message {
    Message::AbortedAssistant(Arc::new(AbortedAssistant {
        content,
        reasoning_summary: Some(SUMMARY.into()),
        provider_context,
        ..AbortedAssistant::default()
    }))
}

/// Lowers `history` for one wire and returns the serialized request messages.
type Lower = fn(&[Message], &ModelIdentity) -> String;

/// Whether the wire's target is the model that produced the opaque reasoning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpaqueReplay {
    Replayed,
    Omitted,
}

// Covers: an interrupted turn must never replay the model's reasoning summary
// as plain assistant text (Anthropic refuses such requests as
// `reasoning_extraction`), whether or not its opaque reasoning survived.
// Owner: provider history conversion, every wire.
#[test]
fn aborted_turns_never_replay_reasoning_summary_as_text() {
    let responses = ModelIdentity::new("cliproxyapi", "openai-responses", "claude-opus-5-5");
    let wires: [(&str, ModelIdentity, OpaqueReplay, Lower); 4] = [
        (
            "responses",
            responses.clone(),
            OpaqueReplay::Replayed,
            |history, target| {
                let input = super::openai_responses::codex_input_items_for_target(
                    history,
                    &mut Vec::new(),
                    Some(target),
                )
                .unwrap();
                serde_json::to_string(&input).unwrap()
            },
        ),
        (
            "chat",
            ModelIdentity::new("openai", "openai-chat-completions", "gpt-chat"),
            OpaqueReplay::Omitted,
            |history, target| {
                let messages =
                    super::openai_chat::to_openai_messages_for_target(history, Some(target))
                        .unwrap();
                serde_json::to_string(&messages).unwrap()
            },
        ),
        (
            "anthropic",
            ModelIdentity::new("anthropic", "anthropic-messages", "claude-opus-5-5"),
            OpaqueReplay::Omitted,
            |history, target| {
                let conversation = super::anthropic_messages::split_system_and_messages(
                    history,
                    target,
                    super::anthropic_messages::ProviderContextReplay::Enabled,
                )
                .unwrap();
                serde_json::to_string(&conversation.messages).unwrap()
            },
        ),
        (
            "gemini",
            ModelIdentity::new("google", "gemini-generate-content", "gemini-pro"),
            OpaqueReplay::Omitted,
            |history, target| {
                let request =
                    super::gemini_generate_content::build_request(history, &[], target, None)
                        .unwrap();
                serde_json::to_string(&request).unwrap()
            },
        ),
    ];
    let encrypted_reasoning = ProviderContextBlock {
        identity: responses,
        kind: "openai_response_output_item".into(),
        position: Some(0),
        data: json!({"type": "reasoning", "status": "incomplete", "encrypted_content": "opaque"}),
        replay: ReplayScope::ExactModel,
    };
    let history = [
        Message::user_text("draw it"),
        // Interrupted mid-reasoning: no opaque reasoning arrived.
        aborted(vec![ContentBlock::Text("partial".into())], Vec::new()),
        Message::user_text("keep going"),
        // Interrupted after the encrypted reasoning item finished.
        aborted(Vec::new(), vec![encrypted_reasoning]),
        Message::user_text("continue"),
    ];

    for (wire, target, opaque, lower) in wires {
        let request = lower(&history, &target);
        assert!(
            !request.contains(SUMMARY),
            "{wire} leaked the summary: {request}"
        );
        assert!(request.contains("[Operation aborted]"), "{wire}: {request}");
        let replayed = if request.contains("\"encrypted_content\":\"opaque\"") {
            OpaqueReplay::Replayed
        } else {
            OpaqueReplay::Omitted
        };
        assert_eq!(replayed, opaque, "{wire}: {request}");
    }
}
