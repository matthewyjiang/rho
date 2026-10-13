use super::*;
use crate::{
    model::{models_dev, Message, ModelRequest},
    providers::openai_compatible::{CompatibleAuth, OpenAiCompatibleProvider},
};
use pretty_assertions::assert_eq;
use serde_json::json;

// Covers: toggle-only catalog controls must not invent an unsupported max effort,
// while OpenRouter explicitly enables its default budget, explicit efforts still
// reach the backend, and toggle-on still counts as possible hidden reasoning.
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
            let toggle_model = if dialect == OpenAiCompatibleDialect::OpenRouter {
                "anthropic/claude-toggle-test"
            } else {
                "qwen3.7-max"
            };
            let mut catalog = json!({
                slug: {"npm": "@ai-sdk/openai-compatible", "models": {
                    toggle_model: {"reasoning": true, "reasoning_options": [
                        {"type": "toggle"}, {"type": "budget_tokens"}
                    ]},
                    "explicit-max": {"reasoning": true, "reasoning_options": [
                        {"type": "toggle"}, {"type": "effort", "values": ["none", "max"]}
                    ]},
                    "explicit-max-only": {"reasoning": true, "reasoning_options": [
                        {"type": "effort", "values": ["max"]}
                    ]}
                }}
            });
            if dialect == OpenAiCompatibleDialect::OpenRouter {
                // Aggregator ids hydrate reasoning from the upstream owner's document.
                catalog["anthropic"] = json!({
                    "npm": "@ai-sdk/anthropic", "models": {
                        "claude-toggle-test": catalog[slug]["models"][toggle_model].clone()
                    }
                });
            }
            models_dev::cache_catalog_for_tests(&catalog);
            for (model, level, effort, risk) in [
                (
                    toggle_model,
                    ReasoningLevel::Max,
                    None,
                    HiddenReasoningRisk::Possible,
                ),
                (
                    toggle_model,
                    ReasoningLevel::High,
                    None,
                    HiddenReasoningRisk::Possible,
                ),
                (
                    toggle_model,
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
                    "explicit-max",
                    ReasoningLevel::Off,
                    Some("none"),
                    HiddenReasoningRisk::Unlikely,
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
                let prepared = client
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
                    .unwrap();
                let body = serde_json::to_value(prepared.body).unwrap();
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
                } else if dialect == OpenAiCompatibleDialect::OpenRouter {
                    expected["reasoning"] = json!({"enabled": true});
                }
                assert_eq!(body, expected, "{provider}/{model} {level:?}");
                assert_eq!(prepared.hidden_reasoning_risk, risk);
            }
        }
    });
}
