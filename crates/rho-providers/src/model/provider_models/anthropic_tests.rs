use serde_json::{json, Value};

use crate::{
    model::{ModelError, ReasoningCapabilities, ReasoningLevelSet},
    reasoning::ReasoningLevel,
};

use super::{
    add_page, finalize_models, model_list_truncated, policy, records_from_page,
    AnthropicModelsResponse, ModelListContinuation,
};

fn list_page(id: &str, has_more: bool, last_id: Option<&str>) -> AnthropicModelsResponse {
    serde_json::from_value(json!({
        "data": [{ "id": id }],
        "has_more": has_more,
        "last_id": last_id,
    }))
    .unwrap()
}

fn collect_pages(
    pages: Vec<AnthropicModelsResponse>,
    max_pages: usize,
) -> Result<Vec<super::super::ProviderModelRecord>, ModelError> {
    let mut models = Vec::new();
    let mut after_id = None::<String>;
    for (index, page) in pages.into_iter().enumerate() {
        if index >= max_pages {
            return Err(model_list_truncated(max_pages));
        }
        match add_page(&mut models, "anthropic", page, after_id.as_deref()) {
            ModelListContinuation::Done => return Ok(finalize_models(models)),
            ModelListContinuation::Next {
                after_id: next_after_id,
            } => after_id = Some(next_after_id),
        }
    }
    Err(model_list_truncated(max_pages))
}

// Covers: list rows keep advertised capabilities and record `{}` when the API
// omitted them so the snapshot is known rather than perpetually incomplete
// Owner: anthropic model discovery
#[test]
fn list_payload_keeps_or_defaults_capabilities_for_the_request_builder() {
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
                "id": "claude-missing-caps",
                "display_name": "Missing"
            },
            {
                "id": "not-claude",
                "display_name": "ignored",
                "capabilities": {"thinking": {"supported": true}}
            }
        ],
        "has_more": false
    }))
    .unwrap();

    let records = records_from_page("anthropic", response);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].model.model, "claude-opus-5");
    assert_eq!(records[0].model.context_window, Some(1_000_000));
    assert_eq!(
        records[0].raw_json["thinking"]["types"]["adaptive"]["supported"],
        true
    );
    assert_eq!(records[1].model.model, "claude-missing-caps");
    assert_eq!(records[1].raw_json, json!({}));
    assert_eq!(
        records[1].model.reasoning_capabilities,
        ReasoningCapabilities::Unknown
    );
    assert!(
        policy::capabilities_json_is_known(Some(&records[1].raw_json.to_string())),
        "omitted API caps must still count as a known empty object"
    );
}

// Covers: picker levels come from the Models API row through records_from_page,
// so a hardcoded Unknown in that wiring cannot silently pass unit coverage
// Owner: anthropic model discovery
#[test]
fn records_from_page_projects_advertised_reasoning_levels() {
    let adaptive_with_effort = json!({
        "thinking": {"types": {"adaptive": {"supported": true}}},
        "effort": {
            "supported": true,
            "low": {"supported": true},
            "medium": {"supported": true},
            "high": {"supported": true},
            "max": {"supported": true}
        }
    });
    let cases: [(&str, &str, Value, ReasoningCapabilities); 5] = [
        (
            "adaptive effort models drop minimal and keep off",
            "claude-opus-5",
            adaptive_with_effort.clone(),
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ])),
        ),
        (
            "a model that cannot disable thinking has no off",
            "claude-mythos-5",
            adaptive_with_effort,
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Max,
            ])),
        ),
        (
            "budget-token models accept the whole ladder",
            "claude-sonnet-4-5",
            json!({
                "thinking": {"types": {
                    "enabled": {"supported": true},
                    "disabled": {"supported": true}
                }}
            }),
            ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
                ReasoningLevel::Off,
                ReasoningLevel::Minimal,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Xhigh,
                ReasoningLevel::Max,
            ])),
        ),
        (
            "adaptive without an effort control stays unknown",
            "claude-opus-5",
            json!({"thinking": {"types": {"adaptive": {"supported": true}}}}),
            ReasoningCapabilities::Unknown,
        ),
        (
            "a row advertising no thinking type stays unknown",
            "claude-haiku-4-5",
            json!({}),
            ReasoningCapabilities::Unknown,
        ),
    ];

    for (name, model, capabilities, expected) in cases {
        let response: AnthropicModelsResponse = serde_json::from_value(json!({
            "data": [{
                "id": model,
                "display_name": model,
                "capabilities": capabilities
            }],
            "has_more": false
        }))
        .unwrap_or_else(|error| panic!("{name}: parse models page: {error}"));
        let records = records_from_page("anthropic", response);
        assert_eq!(records.len(), 1, "{name}");
        assert_eq!(records[0].model.reasoning_capabilities, expected, "{name}");
    }
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
        let result = collect_pages(pages, /*max_pages*/ 2);
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
