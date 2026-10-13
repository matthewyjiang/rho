use super::*;
use crate::reasoning::ReasoningLevel;
use pretty_assertions::assert_eq;
use serde_json::json;

// Covers: old complete cache rows must refresh to recover toggle provenance.
// Owner: models.dev cache migration
#[test]
fn pre_toggle_cache_rows_rehydrate_with_control_provenance() {
    let cache = tempfile::tempdir().unwrap();
    super::super::with_models_dev_cache_dir_for_tests(cache.path().to_path_buf(), || {
        let api = json!({"opencode-go": {"models": {"qwen3.7-max": {
            "reasoning": true, "reasoning_options": [{"type": "toggle"}]
        }}}});
        super::super::cache_catalog_for_tests(&api);
        let connection = super::super::open_models_dev_cache().unwrap();
        connection
            .execute(
                "update model_metadata set cache_version = 12,
             metadata_json = json_remove(metadata_json, '$.reasoning_control')",
                [],
            )
            .unwrap();
        assert!(super::super::model_metadata_needs_refresh(
            "opencode-go",
            "qwen3.7-max"
        ));
        assert_eq!(
            current_reasoning_metadata("opencode-go", "qwen3.7-max"),
            (None, ReasoningControl::Effort)
        );

        super::super::cache_catalog_for_tests(&api);
        let (metadata, control) = current_reasoning_metadata("opencode-go", "qwen3.7-max");
        assert_eq!(control, ReasoningControl::Toggle);
        assert_eq!(
            metadata.unwrap().supported_reasoning_levels,
            Some(vec![ReasoningLevel::Off, ReasoningLevel::Max])
        );
        assert!(!super::super::model_metadata_needs_refresh(
            "opencode-go",
            "qwen3.7-max"
        ));
    });
}

// Covers: explicit user effort levels replace a catalog toggle, even when the
// resulting level list is identical; unrelated and malformed overrides do not.
// Owner: models.dev override merge
#[test]
fn explicit_level_overrides_replace_toggle_provenance() {
    for (contents, control, levels) in [
        (
            "supported_reasoning_levels = ['off', 'max']",
            ReasoningControl::Effort,
            vec![ReasoningLevel::Off, ReasoningLevel::Max],
        ),
        (
            "supported_reasoning_levels = ['max']",
            ReasoningControl::Effort,
            vec![ReasoningLevel::Max],
        ),
        (
            "display_name = 'local name'",
            ReasoningControl::Toggle,
            vec![ReasoningLevel::Off, ReasoningLevel::Max],
        ),
        (
            "supported_reasoning_levels = 'max'",
            ReasoningControl::Toggle,
            vec![ReasoningLevel::Off, ReasoningLevel::Max],
        ),
    ] {
        let table = contents.parse::<toml::Table>().unwrap();
        let row = CachedRow {
            metadata: ModelMetadata {
                supported_reasoning_levels: Some(vec![ReasoningLevel::Off, ReasoningLevel::Max]),
                reasoning_capabilities_known: true,
                ..ModelMetadata::default()
            },
            reasoning_control: ReasoningControl::Toggle,
            ..CachedRow::default()
        }
        .with_toml_overrides(Some(&table));
        assert_eq!(
            (
                row.reasoning_control,
                row.metadata.supported_reasoning_levels
            ),
            (control, Some(levels)),
            "{contents}"
        );
    }
}
