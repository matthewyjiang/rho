use pretty_assertions::assert_eq;
use serde_json::json;

use super::{format_engine_output, EngineOutput};

// Covers: prints and pretty-printed return values share one hard byte cap,
// including UTF-8 boundaries, the notice, and the truncation marker.
// Owner: codemode model-facing output formatting.
#[test]
fn engine_output_has_one_total_byte_budget() {
    let limit = rho_tools::DEFAULT_MAX_OUTPUT_BYTES;
    for output in [
        EngineOutput {
            return_value: json!("é".repeat(limit)),
            prints: Vec::new(),
            nested_calls: 0,
        },
        EngineOutput {
            return_value: json!("returned"),
            prints: vec!["é".repeat(limit / 2)],
            nested_calls: 1,
        },
        EngineOutput {
            return_value: serde_json::Value::Null,
            prints: vec!["x".repeat(limit)],
            nested_calls: 0,
        },
    ] {
        let rendered = format_engine_output(&output);
        assert!(rendered.len() <= limit, "{} > {limit}", rendered.len());
        if rendered.len() == limit && output.return_value.is_null() {
            assert_eq!(rendered, output.prints[0]);
        } else {
            assert!(rendered.ends_with(rho_tools::tool::TRUNCATION_MARKER));
        }
    }
}
