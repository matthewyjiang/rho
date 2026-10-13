use super::*;
use crate::{
    model::{models_dev, Message, ModelRequest},
    providers::openai_compatible::{CompatibleAuth, OpenAiCompatibleProvider},
};
use pretty_assertions::assert_eq;
use serde_json::json;

// Covers: toggle-only catalog controls must not invent an unsupported max effort,
// while explicit effort values still reach the backend and omission still counts as reasoning.
// Owner: catalog-to-chat-completions wire contract
#[test]
fn catalog_toggle_and_explicit_effort_shape_chat_requests() {
    let cache = tempfile::tempdir().unwrap();
    models_dev::with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
        for (provider, dialect) in [
            ("opencode-go", OpenAiCompatibleDialect::Standard),
            ("qwen-token-plan", OpenAiCompatibleDialect::QwenTokenPlan),
            ("openrouter", OpenAiCompatibleDialect::OpenRouter),
        ] {
            let slug = if provider == "qwen-token-plan" {
                "alibaba-token-plan"
            } else {
                provider
            };
            models_dev::cache_catalog_for_tests(&json!({
                slug: {"npm": "@ai-sdk/openai-compatible", "models": {
                    "qwen3.7-max": {"reasoning": true, "reasoning_options": [
                        {"type": "toggle"}, {"type": "budget_tokens"}
                    ]},
                    "explicit-max": {"reasoning": true, "reasoning_options": [
                        {"type": "toggle"}, {"type": "effort", "values": ["none", "max"]}
                    ]},
                    "explicit-max-only": {"reasoning": true, "reasoning_options": [
                        {"type": "effort", "values": ["max"]}
                    ]}
                }}
            }));
            for (model, level, effort, risk) in [
                (
                    "qwen3.7-max",
                    ReasoningLevel::Max,
                    None,
                    HiddenReasoningRisk::Possible,
                ),
                (
                    "qwen3.7-max",
                    ReasoningLevel::High,
                    None,
                    HiddenReasoningRisk::Possible,
                ),
                (
                    "qwen3.7-max",
                    ReasoningLevel::Off,
                    Some("none"),
                    HiddenReasoningRisk::Unlikely,
                ),
                (
                    "explicit-max",
                    ReasoningLevel::Max,
                    Some("max"),
                    HiddenReasoningRisk::Possible,
                ),
                (
                    "explicit-max-only",
                    ReasoningLevel::Max,
                    Some("max"),
                    HiddenReasoningRisk::Possible,
                ),
            ] {
                let client = OpenAiCompatibleProvider::new(
                    crate::reqwest_client(),
                    provider,
                    model.into(),
                    dialect,
                    CompatibleAuth::None,
                    "http://localhost/v1".into(),
                );
                let messages = [Message::user_text("reply with just: ok")];
                let body = serde_json::to_value(
                    client
                        .request_body(
                            ModelRequest {
                                messages: &messages,
                                tools: &[],
                                cancellation: Default::default(),
                                reasoning_level: level,
                                prompt_cache_key: None,
                            },
                            /*stream*/ true,
                        )
                        .unwrap(),
                )
                .unwrap();
                let mut expected = json!({
                    "model": model, "messages": [{"role": "user", "content": [
                        {"type": "text", "text": "reply with just: ok"}
                    ]}], "stream": true, "stream_options": {"include_usage": true}
                });
                if let Some(effort) = effort {
                    if dialect == OpenAiCompatibleDialect::OpenRouter {
                        expected["reasoning"] = json!({"effort": effort});
                    } else {
                        expected["reasoning_effort"] = json!(effort);
                    }
                }
                assert_eq!(body, expected, "{provider}/{model} {level:?}");
                assert_eq!(client.reasoning.hidden_reasoning_risk(model, level), risk);
            }
        }
    });
}
