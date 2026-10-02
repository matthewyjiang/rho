use super::{output_schema, Rendered};
use schemars::JsonSchema;
use serde::Serialize;

// Covers: omitting oversized script data must not silently cut model-facing text.
// Owner: generic rendered output budgets.
#[test]
fn oversized_data_marks_text_truncation() {
    let limit = crate::DEFAULT_MAX_OUTPUT_BYTES;
    let rendered = Rendered::new("é".repeat(limit / "é".len()), "x".repeat(limit))
        .limit_data(limit)
        .unwrap();
    assert!(rendered.data().is_none());
    assert!(rendered.text().ends_with(crate::tool::TRUNCATION_MARKER));
    assert!(rendered.text().len() <= limit);
}

// Covers: serialize-mode schemas must accept explicit nulls for Option fields.
// Owner: generic tool output schema generation.
#[test]
fn optional_fields_accept_absent_values() {
    #[derive(Serialize, JsonSchema)]
    struct Data {
        label: Option<String>,
        count: Option<u64>,
    }
    let validator = jsonschema::validator_for(&output_schema::<Data>()).unwrap();
    for data in [
        Data {
            label: None,
            count: None,
        },
        Data {
            label: Some("value".into()),
            count: Some(1),
        },
    ] {
        validator
            .validate(&serde_json::to_value(data).unwrap())
            .unwrap();
    }
}
