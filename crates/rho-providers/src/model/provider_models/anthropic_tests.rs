use serde_json::json;

use crate::model::ModelError;

use super::{models_from_pages, AnthropicModelsResponse};

fn list_page(id: &str, has_more: bool, last_id: Option<&str>) -> AnthropicModelsResponse {
    serde_json::from_value(json!({
        "data": [{ "id": id }],
        "has_more": has_more,
        "last_id": last_id,
    }))
    .unwrap()
}

#[test]
fn list_payload_keeps_thinking_capabilities_for_the_request_builder() {
    let response: AnthropicModelsResponse = serde_json::from_value(json!({
        "data": [
            {
                "id": "claude-opus-5",
                "display_name": "Claude Opus 5",
                "max_input_tokens": 1_000_000,
                "max_tokens": 128_000,
                "capabilities": {
                    "thinking": {
                        "supported": true,
                        "types": {
                            "adaptive": {"supported": true},
                            "enabled": {"supported": false}
                        }
                    },
                    "effort": {
                        "supported": true,
                        "max": {"supported": true}
                    }
                }
            },
            {
                "id": "not-claude",
                "display_name": "ignored"
            }
        ],
        "has_more": false
    }))
    .unwrap();

    assert_eq!(response.data.len(), 2);
    assert_eq!(response.data[0].id, "claude-opus-5");
    assert_eq!(response.data[0].max_input_tokens, Some(1_000_000));
    assert_eq!(
        response.data[0].capabilities.as_ref().unwrap()["thinking"]["types"]["adaptive"]
            ["supported"],
        true
    );
    assert!(response.data[1].capabilities.is_none());
}

// Covers: exhausting the model-list page bound must not succeed with a partial snapshot
// Owner: anthropic model discovery
#[test]
fn model_page_limit_exhaustion_is_not_success() {
    let cases = [
        (
            "has_more on the last allowed page",
            vec![
                list_page("claude-1", true, Some("claude-1")),
                list_page("claude-2", true, Some("claude-2")),
            ],
            false,
        ),
        (
            "final page reports no more",
            vec![
                list_page("claude-1", true, Some("claude-1")),
                list_page("claude-2", false, Some("claude-2")),
            ],
            true,
        ),
        (
            "missing cursor stops without error",
            vec![list_page("claude-1", true, None)],
            true,
        ),
        (
            "repeated cursor stops without error",
            vec![
                list_page("claude-1", true, Some("claude-1")),
                list_page("claude-1", true, Some("claude-1")),
            ],
            true,
        ),
    ];

    for (name, pages, expect_success) in cases {
        let result = models_from_pages("anthropic", pages, /*max_pages*/ 2);
        if expect_success {
            result.unwrap_or_else(|error| panic!("{name}: unexpected error: {error}"));
        } else {
            assert!(
                matches!(result, Err(ModelError::InvalidResponse(_))),
                "{name}: bound exhaustion must not succeed"
            );
        }
    }
}
