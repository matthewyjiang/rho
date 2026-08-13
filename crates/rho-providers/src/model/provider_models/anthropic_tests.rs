use serde_json::json;

use super::AnthropicModelsResponse;

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
