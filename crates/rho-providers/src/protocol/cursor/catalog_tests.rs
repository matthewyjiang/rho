use pretty_assertions::assert_eq;

use crate::protocol::cursor::proto::{ModelDetails, ThinkingDetails};
use crate::reasoning::ReasoningLevel;

use super::{
    fallback_models, models_from_details, DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_OUTPUT_TOKENS,
};

// Covers: GetUsableModels rows missing auto still expose Cursor's Auto routing id
// Owner: cursor protocol
#[test]
fn discovered_models_always_include_auto() {
    let models = models_from_details(&[ModelDetails {
        model_id: "composer-1".into(),
        display_model_id: String::new(),
        display_name: "Composer 1".into(),
        display_name_short: String::new(),
        thinking_details: Some(ThinkingDetails {}),
    }]);

    assert_eq!(models[0].id, "auto");
    assert_eq!(models[1].id, "composer-1");
    assert!(models[1].reasoning_levels.is_empty());
}

// Covers: Fast variants must collapse to the base catalog id so /fast is the switch
// Owner: cursor protocol
#[test]
fn discovered_fast_variants_collapse_to_the_base_model() {
    let models = models_from_details(&[
        ModelDetails {
            model_id: "grok-4.6-high-fast".into(),
            display_model_id: String::new(),
            display_name: "Grok 4.6 Fast".into(),
            display_name_short: String::new(),
            thinking_details: None,
        },
        ModelDetails {
            model_id: "grok-4.6-high".into(),
            display_model_id: String::new(),
            display_name: "Grok 4.6".into(),
            display_name_short: String::new(),
            thinking_details: Some(ThinkingDetails {}),
        },
        ModelDetails {
            model_id: "grok-code-fast-1".into(),
            display_model_id: String::new(),
            display_name: "Grok Code Fast 1".into(),
            display_name_short: String::new(),
            thinking_details: None,
        },
    ]);

    let ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
    assert_eq!(ids, ["auto", "grok-4.6", "grok-code-fast-1"]);
    let grok = models.iter().find(|model| model.id == "grok-4.6").unwrap();
    assert_eq!(grok.name, "Grok 4.6");
    assert_eq!(grok.reasoning_levels, vec![ReasoningLevel::High]);
}

// Covers: detected effort suffixes are the only picker levels, including xhigh when present
// Owner: cursor protocol
#[test]
fn discovered_effort_suffixes_become_picker_levels() {
    let cases: &[(&[&str], &str, &[ReasoningLevel])] = &[
        (
            &[
                "grok-4.6-low",
                "grok-4.6-high",
                "grok-4.6-xhigh",
                "grok-4.6-xhigh-fast",
            ],
            "grok-4.6",
            &[
                ReasoningLevel::Low,
                ReasoningLevel::High,
                ReasoningLevel::Xhigh,
            ],
        ),
        (
            &["grok-4.6-high", "grok-4.6-medium"],
            "grok-4.6",
            &[ReasoningLevel::Medium, ReasoningLevel::High],
        ),
        (&["composer-1"], "composer-1", &[]),
    ];

    for (ids, catalog, levels) in cases {
        let models = models_from_details(
            &ids.iter()
                .map(|id| ModelDetails {
                    model_id: (*id).into(),
                    display_model_id: String::new(),
                    display_name: catalog.to_string(),
                    display_name_short: String::new(),
                    thinking_details: None,
                })
                .collect::<Vec<_>>(),
        );
        let model = models.iter().find(|model| model.id == *catalog).unwrap();
        assert_eq!(model.reasoning_levels.as_slice(), *levels, "ids={ids:?}");
    }
}

// Covers: fallback raw ids go through the same suffix detector, so grok 4.6 xhigh is pickable offline
// Owner: cursor protocol
#[test]
fn fallback_detects_grok_46_xhigh_from_suffixed_ids() {
    let grok = fallback_models()
        .into_iter()
        .find(|model| model.id == "grok-4.6")
        .unwrap();
    assert_eq!(
        grok.reasoning_levels,
        vec![
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::Xhigh,
        ]
    );
}

// Covers: a live GetUsableModels row must keep known windows instead of 200k defaults
// Owner: cursor protocol
#[test]
fn live_discovery_overlays_known_context_windows() {
    let models = models_from_details(&[ModelDetails {
        model_id: "gpt-5.2".into(),
        display_model_id: String::new(),
        display_name: "GPT-5.2".into(),
        display_name_short: String::new(),
        thinking_details: None,
    }]);
    let gpt = models.iter().find(|model| model.id == "gpt-5.2").unwrap();
    assert_eq!(gpt.context_window, 400_000);
    assert_eq!(gpt.max_tokens, 128_000);

    let unknown = models_from_details(&[ModelDetails {
        model_id: "brand-new-model".into(),
        display_model_id: String::new(),
        display_name: "Brand New".into(),
        display_name_short: String::new(),
        thinking_details: None,
    }]);
    let row = unknown
        .iter()
        .find(|model| model.id == "brand-new-model")
        .unwrap();
    assert_eq!(row.context_window, DEFAULT_CONTEXT_WINDOW);
    assert_eq!(row.max_tokens, DEFAULT_MAX_OUTPUT_TOKENS);
}
