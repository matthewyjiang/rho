use pretty_assertions::assert_eq;
use reqwest::Url;
use serde_json::{json, Map};

use super::{
    context_length_from_show, is_chat_model, native_root, reasoning_capabilities_from,
    OllamaModelDetails, OllamaShowResponse,
};
use crate::{
    model::{ReasoningCapabilities, ReasoningLevelSet},
    provider::OLLAMA_UNKNOWN_REASONING_LEVELS,
    reasoning::ReasoningLevel,
};

// Covers: only /v1-suffixed bases expose the native API root
// Owner: ollama native discovery
#[test]
fn native_root_strips_a_trailing_v1_segment() {
    let cases = [
        ("http://127.0.0.1:11434/v1", Some("http://127.0.0.1:11434/")),
        (
            "http://127.0.0.1:11434/v1/",
            Some("http://127.0.0.1:11434/"),
        ),
        (
            "http://proxy.example/ollama/v1",
            Some("http://proxy.example/ollama/"),
        ),
        ("http://127.0.0.1:11434", None),
        ("http://127.0.0.1:11434/api", None),
    ];
    for (input, expected) in cases {
        assert_eq!(
            native_root(&Url::parse(input).unwrap())
                .as_ref()
                .map(Url::as_str),
            expected,
            "{input}"
        );
    }
}

// Covers: thinking capability selects Ollama's effort set; absence is known-off
// Owner: ollama native discovery
#[test]
fn thinking_capability_maps_to_ollama_effort_levels() {
    let expected_levels = ReasoningCapabilities::Levels(ReasoningLevelSet::new(
        OLLAMA_UNKNOWN_REASONING_LEVELS.to_vec(),
    ));
    let cases: [(Option<&[&str]>, ReasoningCapabilities); 4] = [
        (None, ReasoningCapabilities::Unknown),
        (
            Some(&["completion", "tools", "thinking"]),
            expected_levels.clone(),
        ),
        (
            Some(&["completion", "tools"]),
            ReasoningCapabilities::NotConfigurable,
        ),
        (Some(&["embedding"]), ReasoningCapabilities::NotConfigurable),
    ];
    for (input, expected) in cases {
        let owned = input.map(|values| {
            values
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            reasoning_capabilities_from(owned.as_deref()),
            expected,
            "{input:?}"
        );
    }
    assert_eq!(
        expected_levels.levels(),
        Some(
            [
                ReasoningLevel::Off,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::Max
            ]
            .as_slice()
        )
    );
}

// Covers: embedding-only tags stay out of the coding-agent picker
// Owner: ollama native discovery
#[test]
fn embedding_only_models_are_not_chat_models() {
    let cases: [(Option<&[&str]>, bool); 4] = [
        (None, true),
        (Some(&["completion", "tools"]), true),
        (Some(&["embedding"]), false),
        (Some(&["embedding", "completion"]), true),
    ];
    for (input, expected) in cases {
        let owned = input.map(|values| {
            values
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(is_chat_model(owned.as_deref()), expected, "{input:?}");
    }
}

// Covers: show prefers model_info.<arch>.context_length over details
// Owner: ollama native discovery
#[test]
fn show_context_length_prefers_model_info() {
    let mut model_info = Map::new();
    model_info.insert("gemma4.context_length".into(), json!(262144));
    let shown = OllamaShowResponse {
        details: OllamaModelDetails {
            context_length: Some(4096),
        },
        model_info: Some(model_info),
        ..OllamaShowResponse::default()
    };
    assert_eq!(context_length_from_show(&shown), Some(262_144));

    let empty = OllamaShowResponse {
        details: OllamaModelDetails {
            context_length: Some(0),
        },
        ..OllamaShowResponse::default()
    };
    assert_eq!(context_length_from_show(&empty), None);
}
