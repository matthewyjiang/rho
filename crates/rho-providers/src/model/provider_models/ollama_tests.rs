use pretty_assertions::assert_eq;
use reqwest::Url;
use serde_json::{json, Map};

use super::{
    cached_parent_model, context_length_from_show, is_chat_model, native_root, parent_model_from,
    reasoning_capabilities_from, OllamaModelDetails, OllamaShowResponse,
};
use crate::{
    model::{
        provider_models::{
            replace_cached_provider_model_records_for_tests,
            with_provider_models_cache_dir_for_tests, ProviderModel, ProviderModelRecord,
        },
        ReasoningCapabilities, ReasoningLevelSet,
    },
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
            ..OllamaModelDetails::default()
        },
        model_info: Some(model_info),
        ..OllamaShowResponse::default()
    };
    assert_eq!(context_length_from_show(&shown), Some(262_144));

    let empty = OllamaShowResponse {
        details: OllamaModelDetails {
            context_length: Some(0),
            ..OllamaModelDetails::default()
        },
        ..OllamaShowResponse::default()
    };
    assert_eq!(context_length_from_show(&empty), None);
}

// Covers: empty or self parent aliases do not become catalog fallbacks
// Owner: ollama native discovery
#[test]
fn parent_model_ignores_empty_and_self_aliases() {
    assert_eq!(
        parent_model_from(
            &OllamaModelDetails {
                parent_model: Some(String::new()),
                ..OllamaModelDetails::default()
            },
            "gemma4:31b"
        ),
        None
    );
    assert_eq!(
        parent_model_from(
            &OllamaModelDetails {
                parent_model: Some("gemma4:31b".into()),
                ..OllamaModelDetails::default()
            },
            "gemma4:31b"
        ),
        None
    );
    assert_eq!(
        parent_model_from(
            &OllamaModelDetails {
                parent_model: Some("qwen3.8:27b-q4_K_M".into()),
                ..OllamaModelDetails::default()
            },
            "qwen3.8:27b"
        ),
        Some("qwen3.8:27b-q4_K_M".into())
    );
}

// Covers: cached parent_model is the models.dev fallback id
// Owner: ollama native discovery
#[test]
fn cached_parent_model_reads_raw_json() {
    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        replace_cached_provider_model_records_for_tests(
            "ollama",
            &[ProviderModelRecord {
                model: ProviderModel {
                    provider: "ollama".into(),
                    model: "qwen3.8:27b".into(),
                    display_name: "qwen3.8:27b".into(),
                    context_window: Some(262_144),
                    max_output_tokens: None,
                    reasoning_capabilities: ReasoningCapabilities::Unknown,
                },
                raw_json: json!({"parent_model": "qwen3.8:27b-q4_K_M"}),
            }],
        )
        .unwrap();
        assert_eq!(
            cached_parent_model("ollama", "qwen3.8:27b").as_deref(),
            Some("qwen3.8:27b-q4_K_M")
        );
        assert_eq!(cached_parent_model("ollama", "missing"), None);
    });
}
