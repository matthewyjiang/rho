use serde_json::json;

use super::*;

fn identity(provider: &str, api: &str, model: &str) -> ModelIdentity {
    ModelIdentity::new(provider, api, model)
}

#[test]
fn reasoning_summaries_are_portable_but_opaque_context_is_not() {
    let source = identity("openai-codex", "openai-responses", "gpt-test");
    let target = identity("anthropic", "anthropic-messages", "claude-test");
    let message = AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provenance: Some(source.clone()),
        reasoning_summary: Some("checked the arithmetic".into()),
        provider_context: vec![ProviderContextBlock {
            identity: source,
            kind: "openai_response_output_item".into(),
            position: None,
            data: json!({"type": "reasoning", "encrypted_content": "signed"}),
        }],
    };

    let prepared = prepare_assistant(message, &target);

    assert!(prepared.replay_context.is_empty());
    assert!(matches!(
        prepared.content.as_slice(),
        [ContentBlock::Text(answer), ContentBlock::Text(summary)]
            if answer == "answer"
                && summary == "<reasoning_summary>\nchecked the arithmetic\n</reasoning_summary>"
    ));
}

#[test]
fn opaque_context_replays_only_to_exact_model_identity() {
    let source = identity("anthropic", "anthropic-messages", "claude-test");
    let block = ProviderContextBlock {
        identity: source.clone(),
        kind: "anthropic_content_block".into(),
        position: Some(0),
        data: json!({"type": "thinking", "thinking": "private", "signature": "sig"}),
    };
    let message = AssistantMessage {
        content: vec![ContentBlock::Text("answer".into())],
        provider_context: vec![block.clone()],
        ..AssistantMessage::default()
    };

    let same = prepare_assistant(message.clone(), &source);
    let different_model = identity("anthropic", "anthropic-messages", "claude-other");
    let different = prepare_assistant(message, &different_model);

    assert_eq!(same.replay_context.len(), 1);
    assert_eq!(same.replay_context[0].data, block.data);
    assert!(different.replay_context.is_empty());
}

#[test]
fn interrupted_context_is_reported_as_nonportable() {
    let source = identity("openai-codex", "openai-responses", "gpt-test");
    let target = identity("anthropic", "anthropic-messages", "claude-test");
    let messages = [Message::AbortedAssistant(Box::new(
        crate::model::AbortedAssistant {
            provider_context: vec![ProviderContextBlock {
                identity: source,
                kind: "openai_response_output_item".into(),
                position: None,
                data: json!({"type": "reasoning"}),
            }],
            ..crate::model::AbortedAssistant::default()
        },
    ))];

    let report = report_message_omissions(&messages, &target);

    assert_eq!(report.omitted_provider_context, 1);
    assert_eq!(report.omitted_kinds, ["openai_response_output_item"]);
}

#[test]
fn handoff_report_names_every_omitted_context_kind() {
    let source = identity("openai-codex", "openai-responses", "gpt-test");
    let target = identity("anthropic", "anthropic-messages", "claude-test");
    let messages = [AssistantMessage {
        provider_context: vec![
            ProviderContextBlock {
                identity: source.clone(),
                kind: "zeta".into(),
                position: None,
                data: json!(1),
            },
            ProviderContextBlock {
                identity: source,
                kind: "alpha".into(),
                position: None,
                data: json!(2),
            },
        ],
        ..AssistantMessage::default()
    }];

    let report = report_omissions(messages.iter(), &target);

    assert_eq!(report.omitted_provider_context, 2);
    assert_eq!(report.omitted_kinds, ["alpha", "zeta"]);
}
